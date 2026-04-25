use bcc_core::{
    store::{memory::MemoryStore, UtxoStore},
    types::{
        address::Address,
        block::{Block, BlockHeader},
        transaction::{Transaction, TxInput, TxKind, TxOutRef, TxOutput},
    },
    validation::transaction::tx_signing_bytes,
};
use bcc_node::mempool::Mempool;
use ed25519_dalek::{Signature, SigningKey, Signer};

/// Fixed test signing key — deterministic across all tests.
fn test_key() -> SigningKey {
    SigningKey::from_bytes(&[1u8; 32])
}

/// Address that corresponds to `test_key()`.
fn test_addr() -> Address {
    Address::from_pubkey_bytes(test_key().verifying_key().as_bytes())
}

/// Seeds a `MemoryStore` with N UTXOs owned by `test_addr()`.
fn make_store_with_utxos(amounts: &[u64]) -> (MemoryStore, Vec<TxOutRef>) {
    let store = MemoryStore::new();
    let mut refs = Vec::new();
    for (i, &amount) in amounts.iter().enumerate() {
        let tx = Transaction {
            kind: TxKind::Transfer,
            inputs: vec![],
            outputs: vec![TxOutput { amount, address: test_addr() }],
        };
        let tx_hash = tx.hash();
        let proposer = Address::from_pubkey_bytes(&[i as u8 + 10; 32]);
        let block = Block {
            header: BlockHeader {
                prev_hash: [0u8; 32],
                merkle_root: [0u8; 32],
                timestamp: i as i64,
                height: i as u64,
                slot: i as u64,
                proposer,
            },
            signature: Signature::from_bytes(&[0u8; 64]),
            txs: vec![tx],
        };
        store.apply_block(&block).unwrap();
        refs.push(TxOutRef { tx_hash, index: 0 });
    }
    (store, refs)
}

/// Builds a transaction spending `out_ref` and signs it with `test_key()`.
fn make_tx(out_ref: TxOutRef, amount: u64) -> Transaction {
    let key = test_key();
    // Build unsigned skeleton to compute the signing bytes.
    let mut tx = Transaction {
        kind: TxKind::Transfer,
        inputs: vec![TxInput {
            out_ref: out_ref.clone(),
            signature: Signature::from_bytes(&[0u8; 64]), // placeholder
            pubkey: key.verifying_key(),
        }],
        outputs: vec![TxOutput { amount, address: test_addr() }],
    };
    // Sign the canonical message and replace the placeholder.
    let msg = tx_signing_bytes(&tx);
    tx.inputs[0].signature = key.sign(&msg);
    tx
}

/// Adding two transactions that spend the same input must fail.
#[test]
fn double_spend_rejected() {
    let (store, refs) = make_store_with_utxos(&[100]);
    let mut pool = Mempool::new(10);
    pool.add(make_tx(refs[0].clone(), 90), &store).unwrap();
    assert!(pool.add(make_tx(refs[0].clone(), 80), &store).is_err());
}

/// When the pool is full, adding a higher-value tx evicts the lowest-value one.
#[test]
fn capacity_eviction() {
    let (store, refs) = make_store_with_utxos(&[10, 20, 1000]);
    let mut pool = Mempool::new(2);
    let low_hash = make_tx(refs[0].clone(), 10).hash();
    pool.add(make_tx(refs[0].clone(), 10), &store).unwrap();
    pool.add(make_tx(refs[1].clone(), 20), &store).unwrap();
    pool.add(make_tx(refs[2].clone(), 1000), &store).unwrap();
    let drained: Vec<_> = pool.drain(10).iter().map(|tx| tx.hash()).collect();
    assert!(!drained.contains(&low_hash), "evicted tx must not be in pool");
    assert_eq!(drained.len(), 2);
}

/// When the pool is full, a lower-or-equal-value tx is rejected.
#[test]
fn capacity_full_rejects_lower() {
    let (store, refs) = make_store_with_utxos(&[100, 200, 50]);
    let mut pool = Mempool::new(2);
    pool.add(make_tx(refs[0].clone(), 100), &store).unwrap();
    pool.add(make_tx(refs[1].clone(), 200), &store).unwrap();
    assert!(pool.add(make_tx(refs[2].clone(), 50), &store).is_err());
}

/// `drain` must return transactions in descending total output value order.
#[test]
fn drain_descending_order() {
    let (store, refs) = make_store_with_utxos(&[30, 10, 20]);
    let mut pool = Mempool::new(10);
    pool.add(make_tx(refs[0].clone(), 30), &store).unwrap();
    pool.add(make_tx(refs[1].clone(), 10), &store).unwrap();
    pool.add(make_tx(refs[2].clone(), 20), &store).unwrap();
    let values: Vec<u64> = pool
        .drain(10)
        .iter()
        .map(|tx| tx.outputs.iter().map(|o| o.amount).sum())
        .collect();
    assert_eq!(values, vec![30, 20, 10]);
}
