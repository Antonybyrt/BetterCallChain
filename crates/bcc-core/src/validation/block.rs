use thiserror::Error;

use crate::store::{StoreError, UtxoStore, ValidatorStore};
use crate::types::block::Block;

/// All the ways a block can fail validation.
#[derive(Debug, Error)]
pub enum BlockValidationError {
    #[error("block signature is invalid")]
    BadSignature,

    #[error("proposer mismatch: expected {expected}, got {got}")]
    ProposerMismatch { expected: String, got: String },

    #[error("parent hash does not match")]
    BadParentHash,

    #[error("block height is not monotonically increasing")]
    BadHeight,

    #[error("block timestamp is before parent timestamp")]
    BadTimestamp,

    #[error("transaction validation failed: {0}")]
    InvalidTransaction(String),

    #[error("storage error: {0}")]
    Store(#[from] StoreError),
}

/// Validates a block against its parent and the current chain state.
///
/// Checks in order: height, parent hash, timestamp, proposer election, signature, transactions.
/// Returns `Ok(())` if all invariants hold.
pub fn validate_block(
    block: &Block,
    parent: &Block,
    _utxo: &dyn UtxoStore,
    _validators: &dyn ValidatorStore,
) -> Result<(), BlockValidationError> {
    // 1. Height must be exactly parent + 1.
    if block.header.height != parent.header.height + 1 {
        return Err(BlockValidationError::BadHeight);
    }

    // 2. prev_hash must match the parent block's hash.
    if block.header.prev_hash != parent.hash() {
        return Err(BlockValidationError::BadParentHash);
    }

    // 3. Timestamp must not be before parent's.
    if block.header.timestamp < parent.header.timestamp {
        return Err(BlockValidationError::BadTimestamp);
    }

    // TODO: elect proposer from validator store and verify it matches block.header.proposer
    // TODO: verify block.signature against block.header using proposer pubkey
    // TODO: validate each transaction against the UTXO set

    Ok(())
}
