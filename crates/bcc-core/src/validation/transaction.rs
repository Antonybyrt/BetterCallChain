use serde::Serialize;
use thiserror::Error;

use crate::crypto::signature::verify;
use crate::store::{StoreError, UtxoStore};
use crate::types::address::Address;
use crate::types::transaction::{TxKind, TxOutRef, TxOutput, Transaction};

/// All the ways a transaction can fail validation.
#[derive(Debug, Error)]
pub enum TxValidationError {
    #[error("transaction has no outputs")]
    EmptyOutputs,

    #[error("output at index {0} has zero value")]
    ZeroValueOutput(usize),

    #[error("input references an unknown or already spent UTXO")]
    InputNotFound,

    #[error("input {0} pubkey does not match UTXO owner address")]
    InvalidOwner(usize),

    #[error("input {0} signature is invalid")]
    InvalidSignature(usize),

    #[error("sum of inputs ({inputs}) is less than sum of outputs ({outputs})")]
    InsufficientFunds { inputs: u64, outputs: u64 },

    #[error("store error: {0}")]
    Store(#[from] StoreError),
}

/// The subset of transaction data that a spender signs.
/// Excludes signatures to avoid circular dependency.
#[derive(Serialize)]
struct TxSigningData<'a> {
    kind:    &'a TxKind,
    inputs:  Vec<&'a TxOutRef>,
    outputs: &'a [TxOutput],
}

/// Returns the canonical bytes that every input of `tx` must sign.
///
/// Callers building transactions should sign these bytes with their Ed25519
/// key and store the resulting `Signature` in each `TxInput`.
pub fn tx_signing_bytes(tx: &Transaction) -> Vec<u8> {
    let data = TxSigningData {
        kind:    &tx.kind,
        inputs:  tx.inputs.iter().map(|i| &i.out_ref).collect(),
        outputs: &tx.outputs,
    };
    serde_json::to_vec(&data).expect("TxSigningData serialization is infallible")
}

/// Validates a transaction against the current UTXO set.
/// Enforces: all inputs exist, pubkeys match UTXO owners, signatures are valid,
/// no zero-value outputs, sum(inputs) >= sum(outputs).
pub fn validate_transaction(
    tx: &Transaction,
    utxo: &dyn UtxoStore,
) -> Result<(), TxValidationError> {
    if tx.outputs.is_empty() {
        return Err(TxValidationError::EmptyOutputs);
    }

    for (i, output) in tx.outputs.iter().enumerate() {
        if output.amount == 0 {
            return Err(TxValidationError::ZeroValueOutput(i));
        }
    }

    let msg = tx_signing_bytes(tx);

    let mut total_in: u64 = 0;
    for (idx, input) in tx.inputs.iter().enumerate() {
        let unspent = utxo
            .get(&input.out_ref)?
            .ok_or(TxValidationError::InputNotFound)?;

        // The pubkey embedded in the input must derive to the UTXO owner's address.
        let expected = Address::from_pubkey_bytes(input.pubkey.as_bytes());
        if unspent.address != expected {
            return Err(TxValidationError::InvalidOwner(idx));
        }

        // The signature must be valid over the signing message.
        verify(&input.pubkey, &msg, &input.signature)
            .map_err(|_| TxValidationError::InvalidSignature(idx))?;

        total_in = total_in.saturating_add(unspent.amount);
    }

    let total_out: u64 = tx.outputs.iter().map(|o| o.amount).sum();

    if total_in < total_out {
        return Err(TxValidationError::InsufficientFunds {
            inputs: total_in,
            outputs: total_out,
        });
    }

    Ok(())
}
