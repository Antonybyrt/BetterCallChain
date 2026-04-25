use std::time::Duration;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use bcc_core::validation::block::validate_block;

use crate::{error::NodeError, p2p::protocol::Message, state::NodeState};

const BATCH_TIMEOUT_SECS: u64 = 10;
const MAX_CONSECUTIVE_TIMEOUTS: usize = 3;
const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;
const BLOCKS_PER_REQUEST: u64 = 256;
/// Retry IBD from scratch if all peers fail before we sync any block.
/// Covers the Docker startup race where peers aren't ready yet.
const MAX_IBD_RETRIES: usize = 10;
const IBD_RETRY_DELAY_SECS: u64 = 3;

/// Downloads the canonical chain from bootstrap peers until we are in sync.
///
/// Each batch (up to 256 blocks) has an independent 10 s timeout.
/// After 3 consecutive timeouts from one peer, the next bootstrap peer is tried.
/// An empty `Blocks` response signals that the peer considers us in sync.
///
/// Returns immediately if no bootstrap peers are configured.
pub async fn run_ibd(state: &NodeState, cancel: &CancellationToken) -> Result<(), NodeError> {
    let peers = state.config.bootstrap_peers.clone();
    if peers.is_empty() {
        info!("IBD: no bootstrap peers — skipping");
        return Ok(());
    }

    let local_tip = state
        .blocks
        .tip()
        .map_err(NodeError::Store)?
        .map(|(h, _)| h)
        .unwrap_or(0);

    let initial_from_height = local_tip + 1;
    let mut from_height = initial_from_height;
    let mut peer_idx = 0;
    let mut consecutive_timeouts = 0;
    let mut retry_count = 0;

    info!(from_height, "IBD starting");

    'outer: loop {
        if peer_idx >= peers.len() {
            // If we haven't synced a single block yet, the peers were likely not
            // ready (Docker startup race).  Wait and retry a few times.
            if from_height == initial_from_height && retry_count < MAX_IBD_RETRIES {
                retry_count += 1;
                warn!(
                    attempt = retry_count, max = MAX_IBD_RETRIES,
                    delay = IBD_RETRY_DELAY_SECS,
                    "IBD: all peers unavailable before syncing — retrying"
                );
                tokio::time::sleep(Duration::from_secs(IBD_RETRY_DELAY_SECS)).await;
                peer_idx = 0;
                consecutive_timeouts = 0;
                continue 'outer;
            }
            info!("IBD: all peers exhausted");
            break;
        }

        let addr = peers[peer_idx];
        let stream = match TcpStream::connect(addr).await {
            Ok(s) => s,
            Err(e) => {
                warn!(%addr, err = %e, "IBD: connect failed — trying next peer");
                peer_idx += 1;
                continue;
            }
        };

        let codec = LengthDelimitedCodec::builder()
            .max_frame_length(MAX_FRAME_LEN)
            .new_codec();
        let mut framed = Framed::new(stream, codec);

        loop {
            if cancel.is_cancelled() {
                return Err(NodeError::Shutdown);
            }

            // Request next batch.
            let req = Message::GetBlocks { from_height };
            let req_bytes = serde_json::to_vec(&req)
                .map_err(|e| NodeError::P2p(e.to_string()))?;
            if framed.send(Bytes::from(req_bytes)).await.is_err() {
                warn!(%addr, "IBD: send error — trying next peer");
                peer_idx += 1;
                break;
            }

            // Await response with per-batch timeout.
            let frame = tokio::time::timeout(
                Duration::from_secs(BATCH_TIMEOUT_SECS),
                framed.next(),
            )
            .await;

            match frame {
                Err(_elapsed) => {
                    warn!(%addr, "IBD: batch timeout");
                    consecutive_timeouts += 1;
                    if consecutive_timeouts >= MAX_CONSECUTIVE_TIMEOUTS {
                        peer_idx += 1;
                        consecutive_timeouts = 0;
                        break; // try next peer
                    }
                    continue;
                }
                Ok(None) => {
                    warn!(%addr, "IBD: peer disconnected");
                    peer_idx += 1;
                    break;
                }
                Ok(Some(Err(e))) => {
                    warn!(%addr, err = %e, "IBD: framing error");
                    peer_idx += 1;
                    break;
                }
                Ok(Some(Ok(bytes))) => {
                    consecutive_timeouts = 0;

                    let msg: Message = match serde_json::from_slice(&bytes) {
                        Ok(m) => m,
                        Err(e) => {
                            warn!(%addr, err = %e, "IBD: decode error — skipping frame");
                            continue;
                        }
                    };

                    match msg {
                        // Empty batch → we are in sync.
                        Message::Blocks { blocks } if blocks.is_empty() => {
                            info!(synced_to = from_height - 1, "IBD complete");
                            break 'outer;
                        }
                        Message::Blocks { blocks } => {
                            for block in &blocks {
                                // Genesis block (height 0) has no parent to validate against.
                                if block.header.height > 0 {
                                    let parent_h = block.header.height - 1;
                                    let parent = state
                                        .blocks
                                        .get_by_height(parent_h)
                                        .map_err(NodeError::Store)?
                                        .ok_or_else(|| NodeError::P2p(format!(
                                            "IBD: parent block at height {} not found", parent_h
                                        )))?;
                                    validate_block(
                                        block, &parent,
                                        &*state.utxo, &*state.validators,
                                    )
                                    .map_err(|e| NodeError::Validation(format!(
                                        "IBD: invalid block at height {}: {}", block.header.height, e
                                    )))?;
                                }
                                state.blocks.insert(block).map_err(NodeError::Store)?;
                                state.utxo.apply_block(block).map_err(NodeError::Store)?;
                            }
                            if let Some(last) = blocks.last() {
                                from_height = last.header.height + 1;
                                info!(height = last.header.height, "IBD: batch applied");
                            }
                            // If the peer sent fewer blocks than we requested, sync is done.
                            if (blocks.len() as u64) < BLOCKS_PER_REQUEST {
                                info!(synced_to = from_height - 1, "IBD complete");
                                break 'outer;
                            }
                        }
                        _ => continue,
                    }
                }
            }
        }
    }

    Ok(())
}
