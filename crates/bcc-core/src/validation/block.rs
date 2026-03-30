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

    // 6. Every transaction must be valid against the current UTXO set.
    for tx in &block.txs {
        validate_transaction(tx, utxo)
            .map_err(|e| BlockValidationError::InvalidTransaction(e.to_string()))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::memory::MemoryStore;
    use crate::types::block::{Block, BlockHeader};
    use ed25519_dalek::{Signature, SigningKey};

    /// Builds a minimal Block for testing — signature is zeroed, txs empty.
    fn make_block(height: u64, slot: u64, prev_hash: [u8; 32], timestamp: i64) -> Block {
        let signing_key = SigningKey::from_bytes(&[42u8; 32]);
        let proposer = crate::types::address::Address::from_pubkey_bytes(
            signing_key.verifying_key().as_bytes(),
        );
        Block {
            header: BlockHeader {
                prev_hash,
                merkle_root: [0u8; 32],
                timestamp,
                height,
                slot,
                proposer,
            },
            signature: Signature::from_bytes(&[0u8; 64]),
            txs: vec![],
        }
    }

    /// A block whose height is not parent + 1 must be rejected.
    #[test]
    fn test_bad_height() {
        let store = MemoryStore::new();
        let parent = make_block(0, 0, [0u8; 32], 1000);
        let child = make_block(5, 1, parent.hash(), 1001); // height 5 != 0 + 1
        let err = validate_block(&child, &parent, &store, &store).unwrap_err();
        assert!(matches!(err, BlockValidationError::BadHeight));
    }

    /// A block whose prev_hash does not match the parent must be rejected.
    #[test]
    fn test_bad_parent_hash() {
        let store = MemoryStore::new();
        let parent = make_block(0, 0, [0u8; 32], 1000);
        let child = make_block(1, 1, [0xFFu8; 32], 1001); // wrong prev_hash
        let err = validate_block(&child, &parent, &store, &store).unwrap_err();
        assert!(matches!(err, BlockValidationError::BadParentHash));
    }
}
