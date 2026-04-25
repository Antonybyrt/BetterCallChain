use thiserror::Error;

use crate::consensus::pos::elect_proposer;
use crate::crypto::signature::verify;
use crate::store::{StoreError, UtxoStore, ValidatorStore};
use crate::types::block::Block;
use crate::validation::transaction::validate_transaction;

/// All the ways a block can fail validation.
#[derive(Debug, Error)]
pub enum BlockValidationError {
    #[error("block signature is invalid")]
    BadSignature,

    #[error("proposer mismatch: expected {expected}, got {got}")]
    ProposerMismatch { expected: String, got: String },

    #[error("no eligible validators for slot {0}")]
    NoValidators(u64),

    #[error("parent hash does not match")]
    BadParentHash,

    #[error("block height is not monotonically increasing")]
    BadHeight,

    #[error("block timestamp is before parent timestamp")]
    BadTimestamp,

    #[error("transaction validation failed: {0}")]
    InvalidTransaction(String),

    #[error("merkle root mismatch")]
    BadMerkleRoot,

    #[error("storage error: {0}")]
    Store(#[from] StoreError),
}

/// Validates a block against its parent and the current chain state.
/// Checks in order: height, parent hash, timestamp, proposer election, signature, transactions.
pub fn validate_block(
    block: &Block,
    parent: &Block,
    utxo: &dyn UtxoStore,
    validators: &dyn ValidatorStore,
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

    // 4. Proposer must match the deterministic election result.
    let active = validators.all_active(block.header.slot)?;
    let elected = elect_proposer(block.header.slot, &parent.hash(), &active)
        .ok_or(BlockValidationError::NoValidators(block.header.slot))?;

    if elected.address != block.header.proposer {
        return Err(BlockValidationError::ProposerMismatch {
            expected: elected.address.as_str().to_string(),
            got: block.header.proposer.as_str().to_string(),
        });
    }

    // 5. Block signature must be valid against the header bytes.
    let header_bytes =
        bincode::serialize(&block.header).expect("BlockHeader serialization is infallible");
    verify(&elected.pubkey, &header_bytes, &block.signature)
        .map_err(|_| BlockValidationError::BadSignature)?;

    // 6. Merkle root must match the transactions actually in this block.
    let computed = Block::compute_merkle_root(&block.txs);
    if block.header.merkle_root != computed {
        return Err(BlockValidationError::BadMerkleRoot);
    }

    // 7. Every transaction must be valid against the current UTXO set.
    for tx in &block.txs {
        validate_transaction(tx, utxo)
            .map_err(|e| BlockValidationError::InvalidTransaction(e.to_string()))?;
    }

    Ok(())
}
