use std::collections::{BTreeMap, HashMap};
use std::sync::RwLock;

use crate::crypto::hash::BlockHash;
use crate::store::{BlockStore, StoreResult, UtxoStore, ValidatorStore};
use crate::types::address::Address;
use crate::types::block::Block;
use crate::types::transaction::{TxOutRef, TxOutput};
use crate::types::validator::Validator;

/// In-memory implementation of [`BlockStore`], [`UtxoStore`], and [`ValidatorStore`].
/// Intended for unit tests — data is not persisted across restarts.
pub struct MemoryStore {
    blocks:     RwLock<BTreeMap<u64, Block>>,
    index:      RwLock<HashMap<BlockHash, u64>>,
    utxo:       RwLock<HashMap<TxOutRef, TxOutput>>,
    /// Spent outputs captured during apply_block, keyed by block height.
    /// Required to restore UTXOs on rollback.
    spent:      RwLock<HashMap<u64, Vec<(TxOutRef, TxOutput)>>>,
    validators: RwLock<HashMap<Address, Validator>>,
}

impl MemoryStore {
    /// Creates an empty in-memory store.
    pub fn new() -> Self {
        Self {
            blocks:     RwLock::new(BTreeMap::new()),
            index:      RwLock::new(HashMap::new()),
            utxo:       RwLock::new(HashMap::new()),
            spent:      RwLock::new(HashMap::new()),
            validators: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockStore for MemoryStore {
    fn get_by_height(&self, height: u64) -> StoreResult<Option<Block>> {
        Ok(self.blocks.read().unwrap().get(&height).cloned())
    }

    fn get_by_hash(&self, hash: &BlockHash) -> StoreResult<Option<Block>> {
        let index = self.index.read().unwrap();
        let blocks = self.blocks.read().unwrap();
        Ok(index.get(hash).and_then(|h| blocks.get(h)).cloned())
    }

    fn insert(&self, block: &Block) -> StoreResult<()> {
        let hash = block.hash();
        let height = block.header.height;
        self.blocks.write().unwrap().insert(height, block.clone());
        self.index.write().unwrap().insert(hash, height);
        Ok(())
    }

    fn tip(&self) -> StoreResult<Option<(u64, BlockHash)>> {
        Ok(self
            .blocks
            .read()
            .unwrap()
            .iter()
            .next_back()
            .map(|(h, b)| (*h, b.hash())))
    }

    fn iter_from(&self, height: u64) -> StoreResult<Vec<Block>> {
        Ok(self
            .blocks
            .read()
            .unwrap()
            .range(height..)
            .map(|(_, b)| b.clone())
            .collect())
    }
}

impl UtxoStore for MemoryStore {
    fn get(&self, out_ref: &TxOutRef) -> StoreResult<Option<TxOutput>> {
        Ok(self.utxo.read().unwrap().get(out_ref).cloned())
    }

    /// Removes inputs spent by the block and inserts newly created outputs.
    /// Captures spent outputs so rollback_block can restore them.
    fn apply_block(&self, block: &Block) -> StoreResult<()> {
        let mut utxo  = self.utxo.write().unwrap();
        let mut spent = self.spent.write().unwrap();
        let mut block_spent: Vec<(TxOutRef, TxOutput)> = Vec::new();
        for tx in &block.txs {
            let tx_hash = tx.hash();
            for input in &tx.inputs {
                if let Some(prev_out) = utxo.remove(&input.out_ref) {
                    block_spent.push((input.out_ref.clone(), prev_out));
                }
            }
            for (index, output) in tx.outputs.iter().enumerate() {
                utxo.insert(
                    TxOutRef { tx_hash, index: index as u32 },
                    output.clone(),
                );
            }
        }
        spent.insert(block.header.height, block_spent);
        Ok(())
    }

    /// Re-inserts inputs removed by the block and deletes outputs it created.
    fn rollback_block(&self, block: &Block) -> StoreResult<()> {
        let mut utxo  = self.utxo.write().unwrap();
        let mut spent = self.spent.write().unwrap();
        for tx in block.txs.iter().rev() {
            let tx_hash = tx.hash();
            for index in 0..tx.outputs.len() {
                utxo.remove(&TxOutRef { tx_hash, index: index as u32 });
            }
        }
        if let Some(restored) = spent.remove(&block.header.height) {
            for (out_ref, output) in restored {
                utxo.insert(out_ref, output);
            }
        }
        Ok(())
    }

    fn balance(&self, address: &Address) -> StoreResult<u64> {
        let total = self
            .utxo
            .read()
            .unwrap()
            .values()
            .filter(|o| &o.address == address)
            .map(|o| o.amount)
            .sum();
        Ok(total)
    }

    fn list_utxos(&self, address: &Address) -> StoreResult<Vec<(TxOutRef, TxOutput)>> {
        let guard = self
            .utxo
            .read()
            .map_err(|e| crate::store::StoreError::Backend(e.to_string()))?;
        Ok(guard
            .iter()
            .filter(|(_, output)| &output.address == address)
            .map(|(out_ref, output)| (out_ref.clone(), output.clone()))
            .collect())
    }
}

impl ValidatorStore for MemoryStore {
    fn get(&self, address: &Address) -> StoreResult<Option<Validator>> {
        Ok(self.validators.read().unwrap().get(address).cloned())
    }

    fn all_active(&self, slot: u64) -> StoreResult<Vec<Validator>> {
        let mut validators: Vec<Validator> = self
            .validators
            .read()
            .unwrap()
            .values()
            .filter(|v| v.active_since <= slot && v.stake > 0)
            .cloned()
            .collect();
        // Sort by address so every node iterates validators in the same order.
        // elect_proposer does a weighted linear scan — the order determines who
        // is elected, so all nodes must agree on it.
        validators.sort_unstable_by(|a, b| a.address.as_str().cmp(b.address.as_str()));
        Ok(validators)
    }

    fn upsert(&self, validator: &Validator) -> StoreResult<()> {
        self.validators
            .write()
            .unwrap()
            .insert(validator.address.clone(), validator.clone());
        Ok(())
    }
}
