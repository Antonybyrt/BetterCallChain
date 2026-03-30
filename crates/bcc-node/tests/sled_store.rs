use bcc_core::{
    store::{BlockStore, UtxoStore},
    types::{
        address::Address,
        block::{Block, BlockHeader},
        transaction::{Transaction, TxInput, TxKind, TxOutRef, TxOutput},
    },
};
use bcc_node::storage::sled_store::SledStore;
use ed25519_dalek::{Signature, SigningKey};

fn any_addr() -> Address {
    Address::from_pubkey_bytes(&[0u8; 32])
}

fn make_coinbase_block(height: u64, prev_hash: [u8; 32], amount: u64) -> (Block, TxOutRef) {
    let tx = Transaction {
        kind: TxKind::Transfer,
        inputs: vec![],
        outputs: vec![TxOutput { amount, address: any_addr() }],
    };
    let tx_hash = tx.hash();
    let block = Block {
        header: BlockHeader {
            prev_hash,
            merkle_root: Block::compute_merkle_root(&[tx.clone()]),
            timestamp: height as i64,
            height,
            slot: height,
            proposer: any_addr(),
        },
        signature: Signature::from_bytes(&[0u8; 64]),
        txs: vec![tx],
    };
    (block, TxOutRef { tx_hash, index: 0 })
}

fn make_spending_block(
    height: u64,
    prev_hash: [u8; 32],
    spending: TxOutRef,
    amount: u64,
) -> (Block, TxOutRef) {
    let tx = Transaction {
        kind: TxKind::Transfer,
        inputs: vec![TxInput {
            out_ref:   spending,
            signature: Signature::from_bytes(&[0u8; 64]),
            pubkey:    SigningKey::from_bytes(&[0u8; 32]).verifying_key(),
        }],
        outputs: vec![TxOutput { amount, address: any_addr() }],
    };
    let tx_hash = tx.hash();
    let block = Block {
        header: BlockHeader {
            prev_hash,
            merkle_root: Block::compute_merkle_root(&[tx.clone()]),
            timestamp: height as i64,
            height,
            slot: height,
            proposer: any_addr(),
        },
        signature: Signature::from_bytes(&[0u8; 64]),
        txs: vec![tx],
    };
    (block, TxOutRef { tx_hash, index: 0 })
}

/// `insert` stores the block and `tip` reflects it; retrieval by height and hash works.
#[test]
fn insert_and_get() {
    let store = SledStore::new_temporary();
    let (block, _) = make_coinbase_block(0, [0u8; 32], 100);
    let hash = block.hash();

    store.insert(&block).unwrap();

    assert_eq!(store.get_by_height(0).unwrap().unwrap().hash(), hash);
    assert_eq!(store.get_by_hash(&hash).unwrap().unwrap().hash(), hash);
    assert_eq!(store.tip().unwrap(), Some((0, hash)));
}

/// `apply_block` updates the UTXO set; `rollback_block` restores the previous state.
#[test]
fn apply_and_rollback() {
    let store = SledStore::new_temporary();

    let (block_a, out_a) = make_coinbase_block(0, [0u8; 32], 100);
    store.apply_block(&block_a).unwrap();
    assert!(UtxoStore::get(&store, &out_a).unwrap().is_some(), "out_a must exist after apply");

    let (block_b, out_b) = make_spending_block(1, block_a.hash(), out_a.clone(), 90);
    store.apply_block(&block_b).unwrap();
    assert!(UtxoStore::get(&store, &out_a).unwrap().is_none(), "out_a must be spent");
    assert!(UtxoStore::get(&store, &out_b).unwrap().is_some(), "out_b must exist");

    store.rollback_block(&block_b).unwrap();
    assert!(UtxoStore::get(&store, &out_a).unwrap().is_some(), "out_a must be restored");
    assert!(UtxoStore::get(&store, &out_b).unwrap().is_none(), "out_b must be removed");
}

/// The crash sentinel must be absent after a successful `apply_block`.
#[test]
fn crash_sentinel_cleared() {
    let store = SledStore::new_temporary();
    let (block, _) = make_coinbase_block(0, [0u8; 32], 50);
    store.apply_block(&block).unwrap();
    assert!(
        !store.apply_in_progress_flag(),
        "sentinel must be cleared after apply_block"
    );
}
