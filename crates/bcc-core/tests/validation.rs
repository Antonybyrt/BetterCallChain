use bcc_core::{
    store::memory::MemoryStore,
    types::{
        address::Address,
        block::{Block, BlockHeader},
    },
    validation::block::{validate_block, BlockValidationError},
};
use ed25519_dalek::{Signature, SigningKey};

fn make_block(height: u64, slot: u64, prev_hash: [u8; 32], timestamp: i64) -> Block {
    let signing_key = SigningKey::from_bytes(&[42u8; 32]);
    let proposer = Address::from_pubkey_bytes(signing_key.verifying_key().as_bytes());
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
    let store  = MemoryStore::new();
    let parent = make_block(0, 0, [0u8; 32], 1000);
    let child  = make_block(5, 1, parent.hash(), 1001);
    let err = validate_block(&child, &parent, &store, &store, 5).unwrap_err();
    assert!(matches!(err, BlockValidationError::BadHeight));
}

/// A block whose prev_hash does not match the parent must be rejected.
#[test]
fn test_bad_parent_hash() {
    let store  = MemoryStore::new();
    let parent = make_block(0, 0, [0u8; 32], 1000);
    let child  = make_block(1, 1, [0xFFu8; 32], 1001);
    let err = validate_block(&child, &parent, &store, &store, 5).unwrap_err();
    assert!(matches!(err, BlockValidationError::BadParentHash));
}
