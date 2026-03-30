use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bcc_core::{
    consensus::pos::elect_proposer,
    crypto::hash::BlockHash,
    types::{
        block::{Block, BlockHeader},
        transaction::Transaction,
    },
};
use ed25519_dalek::Signer;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::state::NodeState;

/// Maximum transactions included in a single proposed block.
const MAX_TXS_PER_BLOCK: usize = 512;
/// Minimum seconds remaining in a slot before we will propose a block.
/// Prevents nodes from receiving a block after the slot boundary.
const MIN_PROPOSE_WINDOW_SECS: i64 = 1;

/// Returns the current Unix time in whole seconds.
fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Sleeps until `target_unix_secs`, honouring `cancel`.
/// Returns `true` if the sleep completed normally, `false` if cancelled.
async fn sleep_until_secs(target: i64, cancel: &CancellationToken) -> bool {
    let now = unix_seconds();
    let remaining = (target - now).max(0) as u64;
    tokio::select! {
        biased;
        _ = cancel.cancelled()                        => false,
        _ = tokio::time::sleep(Duration::from_secs(remaining)) => true,
    }
}

/// Runs the PoS slot production loop until `cancel` is triggered.
///
/// Each iteration:
/// 1. Captures `now` **once** — avoids slot drift between checks.
/// 2. Skips the current slot if we are already past its boundary.
/// 3. Elects the proposer; if it is this node, calls [`propose_block`].
/// 4. Sleeps until the end of the slot.
pub async fn run_slot_ticker(state: NodeState, cancel: CancellationToken) {
    let slot_duration = state.config.slot_duration_secs as i64;

    loop {
        // Single timestamp capture — used for all calculations in this iteration.
        let now = unix_seconds();
        let slot = now / slot_duration;
        let slot_end = (slot + 1) * slot_duration;

        // Skip if we are already past the slot boundary (drift / slow startup).
        if now >= slot_end {
            if !sleep_until_secs(slot_end, &cancel).await {
                return;
            }
            continue;
        }

        // Read current chain tip.
        let (tip_height, tip_hash) = match state.blocks.tip() {
            Ok(Some((h, hash))) => (h, hash),
            Ok(None) => {
                warn!("slot_ticker: no chain tip — genesis not applied?");
                if !sleep_until_secs(slot_end, &cancel).await {
                    return;
                }
                continue;
            }
            Err(e) => {
                error!(err = %e, "slot_ticker: failed to read chain tip");
                if !sleep_until_secs(slot_end, &cancel).await {
                    return;
                }
                continue;
            }
        };

        // Load active validators and elect proposer using the same `slot` and `tip_hash`.
        let validators = match state.validators.all_active(slot as u64) {
            Ok(v) => v,
            Err(e) => {
                error!(err = %e, "slot_ticker: failed to read validators");
                if !sleep_until_secs(slot_end, &cancel).await {
                    return;
                }
                continue;
            }
        };

        let time_remaining = slot_end - now;

        if let Some(proposer) = elect_proposer(slot as u64, &tip_hash, &validators) {
            if proposer.address == state.config.my_address
                && time_remaining > MIN_PROPOSE_WINDOW_SECS
            {
                propose_block(&state, tip_height, tip_hash, slot as u64, now).await;
            }
        }

        if !sleep_until_secs(slot_end, &cancel).await {
            return;
        }
    }
}

/// Builds, signs, stores, and broadcasts a new block.
///
/// The mempool lock is acquired only for `drain` and `remove` — never held across signing.
async fn propose_block(
    state:      &NodeState,
    tip_height: u64,
    tip_hash:   BlockHash,
    slot:       u64,
    timestamp:  i64,
) {
    // Drain mempool; release the lock before signing.
    let txs: Vec<Transaction> = {
        let mempool = state.mempool.lock().await;
        mempool.drain(MAX_TXS_PER_BLOCK)
    };

    let merkle_root = Block::compute_merkle_root(&txs);
    let header = BlockHeader {
        prev_hash: tip_hash,
        merkle_root,
        timestamp,
        height: tip_height + 1,
        slot,
        proposer: state.config.my_address.clone(),
    };

    let header_bytes = match bincode::serialize(&header) {
        Ok(b) => b,
        Err(e) => {
            error!(err = %e, "slot_ticker: failed to serialize block header");
            return;
        }
    };

    let signature = state.config.my_signing_key.sign(&header_bytes);
    let block = Block { header, signature, txs };

    if let Err(e) = state.blocks.insert(&block) {
        error!(err = %e, "slot_ticker: failed to insert proposed block");
        return;
    }
    if let Err(e) = state.utxo.apply_block(&block) {
        error!(err = %e, "slot_ticker: failed to apply block to UTXO");
        return;
    }

    // Remove included transactions from mempool.
    let hashes: Vec<_> = block.txs.iter().map(|tx| tx.hash()).collect();
    state.mempool.lock().await.remove(&hashes);

    let height = block.header.height;
    let hash   = hex::encode(block.hash());

    // Notify all peer tasks via the broadcast channel.
    let _ = state.new_block.send(block);
    info!(height, %hash, slot, "proposed block");
}
