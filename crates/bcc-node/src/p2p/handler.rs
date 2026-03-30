use std::net::SocketAddr;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::state::NodeState;

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
    info!(%addr, "peer connected");

    let mut framed = Framed::new(stream, make_codec());
    let (tx, mut rx) = mpsc::channel::<Message>(OUTBOUND_QUEUE);

    state.peers.write().await.insert(addr, tx.clone());
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
                        info!(%addr, "peer disconnected");
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
    info!(%addr, "peer task done");
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
            let tip_height = state
                .blocks
                .tip()
                .map_err(|e| e.to_string())?
                .map(|(h, _)| h)
                .unwrap_or(0);

            // Only accept blocks that extend the current tip.
            if block.header.height == tip_height + 1 {
                state.blocks.insert(&block).map_err(|e| e.to_string())?;
                state.utxo.apply_block(&block).map_err(|e| e.to_string())?;

                let hashes: Vec<_> = block.txs.iter().map(|tx| tx.hash()).collect();
                state.mempool.lock().await.remove(&hashes);

                state
                    .peers
                    .read()
                    .await
                    .broadcast_except(&source, Message::NewBlock { block });
            }
        }

        Message::NewTx { tx } => {
            let _ = state.mempool.lock().await.add(tx.clone(), &*state.utxo);
            state
                .peers
                .read()
                .await
                .broadcast_except(&source, Message::NewTx { tx });
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
