use bcc_core::types::{
    address::Address,
    transaction::{Transaction, TxInput, TxKind, TxOutRef, TxOutput},
};
use ed25519_dalek::{Signer, SigningKey};

use crate::{error::ClientError, rpc::UtxoItem};

// ── Coin selection ────────────────────────────────────────────────────────────

/// The result of a coin selection: the UTXOs chosen as inputs and their combined value.
pub struct CoinSelection {
    /// UTXOs selected as inputs, in ascending-amount order.
    pub selected: Vec<UtxoItem>,
    /// Sum of all selected UTXO amounts. Always `>= target`.
    pub total: u64,
}

/// Selects a minimal set of UTXOs whose combined value covers `target`
/// using a first-fit ascending strategy.
///
/// The UTXOs are sorted by `amount` ascending, then accumulated until
/// `total >= target`.  Ascending order is chosen because it:
/// - Consolidates small UTXOs first, keeping the UTXO set lean.
/// - Is deterministic and straightforward to unit-test.
///
/// Returns `ClientError::InsufficientFunds` if the full set is insufficient.
pub fn select_coins(utxos: &[UtxoItem], target: u64) -> Result<CoinSelection, ClientError> {
    let mut sorted: Vec<&UtxoItem> = utxos.iter().collect();
    sorted.sort_unstable_by_key(|u| u.amount);

    let mut selected = Vec::new();
    let mut total    = 0u64;

    for utxo in sorted {
        selected.push(utxo.clone());
        total = total.saturating_add(utxo.amount);
        if total >= target {
            return Ok(CoinSelection { selected, total });
        }
    }

    let have: u64 = utxos.iter().map(|u| u.amount).sum();
    Err(ClientError::InsufficientFunds { have, need: target })
}

// ── Transaction construction ──────────────────────────────────────────────────

/// Builds and signs a `TxKind::Transfer` transaction.
///
/// # Arguments
/// - `signing_key` — the decrypted Ed25519 signing key.
/// - `utxos` — UTXOs to spend (already coin-selected).
/// - `recipient` — destination address.
/// - `amount` — tokens to transfer.
/// - `change_address` — address receiving any excess (typically the sender's address).
///
/// # Signing scheme
///
/// Each `TxInput` is signed independently over:
/// ```text
/// bincode::serialize( (&kind, &outputs, &out_ref, input_amount) )
/// ```
/// This tuple binds the signature to:
/// 1. `kind` — prevents replaying a `Transfer` signature in a `Stake` transaction.
/// 2. `outputs` — prevents output substitution after signing: changing any recipient
///    or amount invalidates all signatures.
/// 3. `out_ref` — prevents copying a signature from one input to another, even when
///    both UTXOs belong to the same key.
/// 4. `input_amount` — prevents a malicious node from substituting a different UTXO
///    (of larger value) owned by the same address; the signature binds to the exact
///    amount of the UTXO being spent.
///
/// All outputs (including the change output) are computed **before** any input is
/// signed, so every signature covers the complete final output set.
///
/// # Change output
/// If `total > amount`, a change output is appended at index 1:
/// `TxOutput { amount: total - amount, address: change_address }`.
/// A single exact output (no change) is produced when `total == amount`.
pub fn build_transfer(
    signing_key:    &SigningKey,
    utxos:          Vec<UtxoItem>,
    recipient:      &Address,
    amount:         u64,
    change_address: &Address,
) -> Result<Transaction, ClientError> {
    let total: u64 = utxos.iter().map(|u| u.amount).sum();
    let kind       = TxKind::Transfer;

    // Build the complete output list before signing.
    let mut outputs: Vec<TxOutput> = vec![TxOutput {
        amount,
        address: recipient.clone(),
    }];
    if total > amount {
        // Change output at index 1. The index convention is documented here so
        // future tooling (block explorers, wallet recovery) can rely on it.
        outputs.push(TxOutput {
            amount:  total - amount,
            address: change_address.clone(),
        });
    }

    // Sign each input over (kind, outputs, out_ref, input_amount).
    let mut inputs: Vec<TxInput> = Vec::with_capacity(utxos.len());
    for utxo in &utxos {
        let tx_hash_bytes: [u8; 32] = hex::decode(&utxo.tx_hash)?
            .try_into()
            .map_err(|_| ClientError::Serialization("tx_hash is not 32 bytes".into()))?;

        let out_ref = TxOutRef { tx_hash: tx_hash_bytes, index: utxo.index };

        let msg = bincode::serialize(&(&kind, &outputs, &out_ref, utxo.amount))
            .map_err(|e| ClientError::Serialization(e.to_string()))?;

        let signature = signing_key.sign(&msg);

        inputs.push(TxInput {
            out_ref,
            signature,
            pubkey: signing_key.verifying_key(),
        });
    }

    Ok(Transaction { kind, inputs, outputs })
}
