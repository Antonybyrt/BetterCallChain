use std::collections::{HashMap, HashSet};

use bcc_core::{
    store::UtxoStore,
    types::transaction::{Transaction, TxHash, TxOutRef},
    validation::transaction::validate_transaction,
};
use indexmap::IndexMap;

use crate::error::NodeError;

/// In-memory pool of unconfirmed transactions waiting to be included in a block.
///
/// # Invariants
/// - No two transactions reference the same `TxOutRef` (double-spend guard via `in_flight`).
/// - The pool never exceeds `max_size` entries.
/// - [`drain`] returns transactions in descending order of total output value (fee priority).
///
/// # Lock ordering
/// **Always acquire the UTXO store lock before the mempool lock.**
pub struct Mempool {
    /// Transactions keyed by hash for O(1) removal.
    txs: IndexMap<TxHash, Transaction>,
    /// All `TxOutRef`s currently referenced by pooled transactions — prevents double spends.
    in_flight: HashSet<TxOutRef>,
    /// Value cache: `tx_hash → total_output_amount`, kept in sync with `txs`.
    values: HashMap<TxHash, u64>,
    /// Maximum number of transactions the pool will hold.
    max_size: usize,
}

impl Mempool {
    /// Creates an empty mempool with the given capacity cap.
    pub fn new(max_size: usize) -> Self {
        Self {
            txs: IndexMap::new(),
            in_flight: HashSet::new(),
            values: HashMap::new(),
            max_size,
        }
    }

    /// Validates and inserts a transaction.
    ///
    /// Rejects if: the tx is invalid, any input is already in-flight (double spend),
    /// or the pool is full and the new tx has lower or equal value than the minimum pooled tx.
    pub fn add(&mut self, tx: Transaction, utxo: &dyn UtxoStore) -> Result<(), NodeError> {
        let tx_hash = tx.hash();

        // Already in pool.
        if self.txs.contains_key(&tx_hash) {
            return Ok(());
        }

        // Validate against UTXO set.
        validate_transaction(&tx, utxo)
            .map_err(|e| NodeError::Validation(e.to_string()))?;

        // Double-spend guard: reject if any input is already claimed by another pooled tx.
        for input in &tx.inputs {
            if self.in_flight.contains(&input.out_ref) {
                return Err(NodeError::Validation(
                    "double spend: input already in mempool".into(),
                ));
            }
        }

        let new_value: u64 = tx.outputs.iter().map(|o| o.amount).sum();

        // Eviction: if at capacity, evict the lowest-value tx if the new one is worth more.
        if self.txs.len() >= self.max_size {
            let (min_hash, min_value) = self
                .values
                .iter()
                .min_by_key(|&(_, &v)| v)
                .map(|(&h, &v)| (h, v))
                .ok_or_else(|| NodeError::Validation("mempool at capacity".into()))?;

            if new_value <= min_value {
                return Err(NodeError::Validation("mempool at capacity".into()));
            }

            self.remove_one(&min_hash);
        }

        // Insert.
        for input in &tx.inputs {
            self.in_flight.insert(input.out_ref.clone());
        }
        self.values.insert(tx_hash, new_value);
        self.txs.insert(tx_hash, tx);

        Ok(())
    }

    /// Removes transactions by their hashes (called after block inclusion).
    pub fn remove(&mut self, hashes: &[TxHash]) {
        for hash in hashes {
            self.remove_one(hash);
        }
    }

    /// Returns up to `max` transactions sorted by descending total output value (fee priority).
    /// The returned transactions are NOT removed from the pool — call [`remove`] after inclusion.
    pub fn drain(&self, max: usize) -> Vec<Transaction> {
        let mut by_value: Vec<(u64, &Transaction)> = self
            .txs
            .values()
            .map(|tx| {
                let value = self.values.get(&tx.hash()).copied().unwrap_or(0);
                (value, tx)
            })
            .collect();

        by_value.sort_unstable_by(|a, b| b.0.cmp(&a.0));
        by_value.into_iter().take(max).map(|(_, tx)| tx.clone()).collect()
    }

    fn remove_one(&mut self, hash: &TxHash) {
        if let Some(tx) = self.txs.swap_remove(hash) {
            for input in &tx.inputs {
                self.in_flight.remove(&input.out_ref);
            }
            self.values.remove(hash);
        }
    }
}
