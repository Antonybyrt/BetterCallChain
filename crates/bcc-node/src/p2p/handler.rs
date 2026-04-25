use std::net::SocketAddr;

use bcc_core::validation::block::validate_block;
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::{debug_event::DebugEvent, state::NodeState};

use super::protocol::Message;

/// Maximum encoded frame size: 16 MiB.
const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;
/// Outbound message queue depth per peer.
const OUTBOUND_QUEUE: usize = 64;

fn make_codec() -> LengthDelimitedCodec {
    LengthDelimitedCodec::builder()
        .max_frame_length(MAX_FRAME_LEN)
        .new_codec()
}

/// Runs the full-duplex read/write loop for a single TCP peer connection.
///
/// Registers the peer in the `PeerSet` on entry and deregisters on exit,
/// so the peer list always reflects live connections.
pub async fn run_peer(
    stream: TcpStream,
    addr:   SocketAddr,
    state:  NodeState,
    cancel: CancellationToken,
) {
    let mut framed = Framed::new(stream, make_codec());
    let (tx, mut rx) = mpsc::channel::<Message>(OUTBOUND_QUEUE);

    state.peers.write().await.insert(addr, tx.clone());
    let peer_count = state.peers.read().await.len();
    info!(%addr, peer_count, "peer connected");
    state.emit(DebugEvent::PeerConnected { addr: addr.to_string(), peer_count });
    let mut block_sub = state.new_block.subscribe();

    loop {
        tokio::select! {
            biased;

            // Graceful shutdown.
            _ = cancel.cancelled() => break,

            // New block produced locally — forward to this peer.
            result = block_sub.recv() => {
                match result {
                    Ok(block) => {
                        let msg = Message::NewBlock { block: Box::new(block) };
                        if encode_send(&mut framed, &msg).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(%addr, missed = n, "block broadcast lagged — some blocks skipped");
                    }
                    Err(_) => break,
                }
            }

            // Outbound messages queued by dispatch (replies, gossip re-sends).
            Some(msg) = rx.recv() => {
                if encode_send(&mut framed, &msg).await.is_err() {
                    break;
                }
            }

            // Inbound frame from the peer.
            result = framed.next() => {
                match result {
                    None => {
                        break;
                    }
                    Some(Err(e)) => {
                        warn!(%addr, err = %e, "framing error");
                        break;
                    }
                    Some(Ok(bytes)) => {
                        if let Err(e) = dispatch(&bytes, addr, &state, &tx).await {
                            debug!(%addr, err = %e, "dispatch error");
                        }
                    }
                }
            }
        }
    }

    state.peers.write().await.remove(&addr);
    let peer_count = state.peers.read().await.len();
    info!(%addr, peer_count, "peer disconnected");
    state.emit(DebugEvent::PeerDisconnected { addr: addr.to_string(), peer_count });
}

/// Serialises `msg` as JSON and writes it as a length-delimited frame.
async fn encode_send(
    framed: &mut Framed<TcpStream, LengthDelimitedCodec>,
    msg:    &Message,
) -> Result<(), ()> {
    match serde_json::to_vec(msg) {
        Ok(bytes) => framed.send(Bytes::from(bytes)).await.map_err(|_| ()),
        Err(_) => Err(()),
    }
}

/// Decodes and handles one inbound message from `source`.
///
/// Replies are queued via `reply_tx` (the peer's own outbound channel).
/// Re-broadcast uses `broadcast_except` so the originating peer is skipped.
async fn dispatch(
    bytes:    &[u8],
    source:   SocketAddr,
    state:    &NodeState,
    reply_tx: &mpsc::Sender<Message>,
) -> Result<(), String> {
    let msg: Message = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;

    match msg {
        Message::NewBlock { block } => {
            let incoming_hash = block.hash();
            let block_hash    = hex::encode(incoming_hash);
            let block_height  = block.header.height;
            let tx_count      = block.txs.len();
            let proposer      = block.header.proposer.to_string();

            let (tip_height, tip_hash) = match state.blocks.tip().map_err(|e| e.to_string())? {
                Some(t) => t,
                None    => (0, [0u8; 32]),
            };

            if block.header.height == tip_height + 1 {
                // ── Normal path: block extends our canonical tip ─────────────
                let parent = state
                    .blocks
                    .get_by_height(tip_height)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("parent block at height {} not found", tip_height))?;

                if let Err(e) = validate_block(
                    &block, &parent, &*state.utxo, &*state.validators,
                ) {
                    warn!(
                        from = %source, height = block_height,
                        hash = %block_hash, reason = %e,
                        "p2p: block rejected — validation failed"
                    );
                    state.emit(DebugEvent::BlockRejected {
                        from:   source.to_string(),
                        height: block_height,
                        hash:   block_hash.clone(),
                        reason: e.to_string(),
                    });
                    return Ok(());
                }

                state.blocks.insert(&block).map_err(|e| e.to_string())?;
                state.utxo.apply_block(&block).map_err(|e| e.to_string())?;

                let hashes: Vec<_> = block.txs.iter().map(|tx| tx.hash()).collect();
                state.mempool.lock().await.remove(&hashes);

                state.peers.read().await
                    .broadcast_except(&source, Message::NewBlock { block });

                info!(
                    from = %source, height = block_height,
                    hash = %block_hash, txs = tx_count, proposer = %proposer,
                    "p2p: block accepted from peer"
                );
                state.emit(DebugEvent::BlockFromPeer {
                    from:     source.to_string(),
                    height:   block_height,
                    hash:     block_hash.clone(),
                    txs:      tx_count,
                    proposer: proposer.clone(),
                });

            } else if block.header.height == tip_height {
                // ── Fork Choice Rule: competing block at the same height ──────
                //
                // Two validators proposed a block for the same slot.  Both chains
                // are the same length so we use a deterministic tiebreaker:
                // the block with the *lower* hash wins.  All nodes see the same
                // two hashes and converge to the same choice without coordination.
                if incoming_hash == tip_hash {
                    return Ok(()); // duplicate
                }
                if incoming_hash >= tip_hash {
                    debug!(
                        from = %source, height = block_height,
                        hash = %block_hash, our_tip = %hex::encode(tip_hash),
                        "p2p: fork at tip — keeping ours (lower hash)"
                    );
                    return Ok(());
                }

                // Incoming block has lower hash → reorg to it.
                // Require both blocks share the same parent (simple tip-level fork).
                let parent_height = tip_height.saturating_sub(1);
                let parent = match state.blocks.get_by_height(parent_height)
                    .map_err(|e| e.to_string())?
                {
                    Some(p) => p,
                    None => {
                        warn!(from = %source, height = block_height,
                              "p2p: fork reorg — parent block not found");
                        return Ok(());
                    }
                };

                if block.header.prev_hash != parent.hash() {
                    // Deep fork (different parent) — out of scope for now, skip.
                    debug!(
                        from = %source, height = block_height,
                        "p2p: fork at tip — different parent, complex reorg not supported"
                    );
                    return Ok(());
                }

                if let Err(e) = validate_block(
                    &block, &parent, &*state.utxo, &*state.validators,
                ) {
                    warn!(
                        from = %source, height = block_height,
                        hash = %block_hash, reason = %e,
                        "p2p: fork candidate rejected — validation failed"
                    );
                    return Ok(());
                }

                // Roll back the current tip (restores UTXOs + removes block from chain).
                let current_tip = state.blocks
                    .get_by_height(tip_height)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "current tip block missing".to_string())?;

                let evicted_hash = hex::encode(current_tip.hash());

                state.utxo.rollback_block(&current_tip).map_err(|e| e.to_string())?;

                // Apply the winning block.
                state.blocks.insert(&block).map_err(|e| e.to_string())?;
                state.utxo.apply_block(&block).map_err(|e| e.to_string())?;

                let hashes: Vec<_> = block.txs.iter().map(|tx| tx.hash()).collect();
                state.mempool.lock().await.remove(&hashes);

                state.peers.read().await
                    .broadcast_except(&source, Message::NewBlock { block });

                warn!(
                    from = %source, height = block_height,
                    new_tip = %block_hash, evicted = %evicted_hash,
                    "p2p: fork reorg — switched to lower-hash block"
                );
                state.emit(DebugEvent::BlockReorged {
                    height:  block_height,
                    new_tip: block_hash.clone(),
                    evicted: evicted_hash,
                });

            } else if block.header.height > tip_height + 1 {
                // ── We are behind by more than one block ─────────────────────
                // The peer has a longer chain.  Log so the operator can see the
                // gap; the P2P connector and IBD will resync the node.
                debug!(
                    from = %source, block_height, local_tip = tip_height,
                    gap = block_height - tip_height - 1,
                    "p2p: block ahead of tip — node may need resync"
                );
            } else {
                // block_height < tip_height: stale block from a shorter chain
                debug!(
                    from = %source, block_height, local_tip = tip_height,
                    hash = %block_hash,
                    "p2p: stale block ignored"
                );
            }
        }

        Message::NewTx { tx } => {
            let tx_hash = hex::encode(tx.hash());
            match state.mempool.lock().await.add(tx.clone(), &*state.utxo) {
                Ok(added) => {
                    debug!(from = %source, tx_hash = %tx_hash, "p2p: tx gossip accepted, re-broadcasting");
                    if let Some(ev) = &added.evicted {
                        state.emit(DebugEvent::TxEvicted { evicted: ev.clone(), new_tx: added.tx_hash.clone() });
                    }
                    state.emit(DebugEvent::TxAccepted {
                        tx_hash:   added.tx_hash.clone(),
                        value:     added.value,
                        pool_size: added.pool_size,
                    });
                    state.emit(DebugEvent::TxGossipAccepted {
                        from:    source.to_string(),
                        tx_hash: tx_hash.clone(),
                    });
                    state.peers.read().await.broadcast_except(&source, Message::NewTx { tx });
                }
                Err(e) => {
                    debug!(from = %source, tx_hash = %tx_hash, reason = %e, "p2p: tx gossip rejected");
                    state.emit(DebugEvent::TxRejected { tx_hash: tx_hash.clone(), reason: e.to_string() });
                    state.emit(DebugEvent::TxGossipRejected {
                        from:    source.to_string(),
                        tx_hash: tx_hash.clone(),
                        reason:  e.to_string(),
                    });
                }
            }
        }

        Message::GetBlocks { from_height } => {
            let blocks = state
                .blocks
                .iter_from(from_height)
                .map_err(|e| e.to_string())?;
            let batch: Vec<_> = blocks.into_iter().take(256).collect();
            let _ = reply_tx.try_send(Message::Blocks { blocks: batch });
        }

        Message::GetPeers => {
            let addrs = state.peers.read().await.addrs();
            let _ = reply_tx.try_send(Message::Peers { addrs });
        }

        Message::Ping { nonce } => {
            let _ = reply_tx.try_send(Message::Pong { nonce });
        }

        _ => {}
    }

    Ok(())
}
