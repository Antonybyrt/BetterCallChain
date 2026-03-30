use bcc_client::{error::ClientError, rpc::UtxoItem, wallet};
use bcc_core::types::address::Address;
use ed25519_dalek::SigningKey;

fn make_utxo(tx_hash: &str, index: u32, amount: u64) -> UtxoItem {
    UtxoItem { tx_hash: tx_hash.to_string(), index, amount }
}

fn dummy_hash(byte: u8) -> String {
    hex::encode([byte; 32])
}

// ── select_coins ──────────────────────────────────────────────────────────────

#[test]
fn select_coins_picks_ascending_order() {
    let utxos = vec![
        make_utxo(&dummy_hash(0xcc), 0, 60),
        make_utxo(&dummy_hash(0xaa), 0, 10),
        make_utxo(&dummy_hash(0xbb), 0, 30),
    ];
    // Ascending: [10, 30, 60]. Target 40 → picks 10+30 = 40.
    let sel = wallet::select_coins(&utxos, 40).unwrap();
    assert_eq!(sel.total, 40);
    assert_eq!(sel.selected.len(), 2);
    assert_eq!(sel.selected[0].amount, 10);
    assert_eq!(sel.selected[1].amount, 30);
}

#[test]
fn select_coins_exact_match_no_remainder() {
    let utxos = vec![make_utxo(&dummy_hash(0x01), 0, 100)];
    let sel   = wallet::select_coins(&utxos, 100).unwrap();
    assert_eq!(sel.total, 100);
    assert_eq!(sel.selected.len(), 1);
}

#[test]
fn select_coins_insufficient_funds() {
    let utxos = vec![make_utxo(&dummy_hash(0x01), 0, 5)];
    let result = wallet::select_coins(&utxos, 100);
    assert!(matches!(
        result,
        Err(ClientError::InsufficientFunds { have: 5, need: 100 })
    ));
}

#[test]
fn select_coins_empty_utxo_set() {
    let result = wallet::select_coins(&[], 1);
    assert!(matches!(
        result,
        Err(ClientError::InsufficientFunds { have: 0, need: 1 })
    ));
}

// ── build_transfer ────────────────────────────────────────────────────────────

fn make_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

#[test]
fn build_transfer_produces_change_output() {
    let key           = make_signing_key(1);
    let sender        = Address::from_pubkey_bytes(key.verifying_key().as_bytes());
    let recipient     = Address::from_pubkey_bytes(&[2u8; 32]);
    let utxo          = make_utxo(&dummy_hash(0x01), 0, 100);

    let tx = wallet::build_transfer(&key, vec![utxo], &recipient, 70, &sender).unwrap();

    assert_eq!(tx.outputs.len(), 2);
    assert_eq!(tx.outputs[0].amount, 70);
    assert_eq!(tx.outputs[0].address.to_string(), recipient.to_string());
    assert_eq!(tx.outputs[1].amount, 30);
    assert_eq!(tx.outputs[1].address.to_string(), sender.to_string());
}

#[test]
fn build_transfer_exact_amount_no_change() {
    let key       = make_signing_key(1);
    let sender    = Address::from_pubkey_bytes(key.verifying_key().as_bytes());
    let recipient = Address::from_pubkey_bytes(&[2u8; 32]);
    let utxo      = make_utxo(&dummy_hash(0x01), 0, 50);

    let tx = wallet::build_transfer(&key, vec![utxo], &recipient, 50, &sender).unwrap();

    assert_eq!(tx.outputs.len(), 1);
    assert_eq!(tx.outputs[0].amount, 50);
}

#[test]
fn build_transfer_correct_input_count() {
    let key       = make_signing_key(1);
    let sender    = Address::from_pubkey_bytes(key.verifying_key().as_bytes());
    let recipient = Address::from_pubkey_bytes(&[2u8; 32]);
    let utxos = vec![
        make_utxo(&dummy_hash(0x01), 0, 30),
        make_utxo(&dummy_hash(0x02), 0, 20),
    ];

    let tx = wallet::build_transfer(&key, utxos, &recipient, 40, &sender).unwrap();

    assert_eq!(tx.inputs.len(), 2);
}
