use thiserror::Error;

use crate::store::{StoreError, UtxoStore};
use crate::types::transaction::Transaction;

/// All the ways a transaction can fail validation.
#[derive(Debug, Error)]
pub enum TxValidationError {
    #[error("transaction has no outputs")]
    EmptyOutputs,

    #[error("output at index {0} has zero value")]
    ZeroValueOutput(usize),

    #[error("input references an unknown or already spent UTXO")]
    InputNotFound,

    #[error("sum of inputs ({inputs}) is less than sum of outputs ({outputs})")]
    InsufficientFunds { inputs: u64, outputs: u64 },

    #[error("store error: {0}")]
    Store(#[from] StoreError),
}

/// Validates a transaction against the current UTXO set.
/// Enforces: all inputs exist, no zero-value outputs, sum(inputs) >= sum(outputs).
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

    let mut total_in: u64 = 0;
    for input in &tx.inputs {
        let unspent = utxo
            .get(&input.out_ref)?
            .ok_or(TxValidationError::InputNotFound)?;
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
