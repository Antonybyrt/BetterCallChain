use std::path::Path;

use sled::Transactional;

use bcc_core::{
    crypto::hash::BlockHash,
    store::{BlockStore, StoreError, StoreResult, UtxoStore, ValidatorStore},
    types::{
        address::Address,
        block::Block,
        transaction::{TxOutRef, TxOutput},
        validator::Validator,
    },
};

/// sled tree names.
const TREE_BLOCKS_H:   &[u8] = b"blocks_h";
const TREE_BLOCKS_IDX: &[u8] = b"blocks_idx";
const TREE_UTXO:       &[u8] = b"utxo";
const TREE_VALIDATORS: &[u8] = b"validators";
const TREE_META:       &[u8] = b"meta";
const TREE_SPENT:      &[u8] = b"spent_h";

const META_TIP:              &[u8] = b"tip";
const META_APPLY_IN_PROGRESS: &[u8] = b"apply_in_progress";

/// Persistent store backed by a [sled](https://docs.rs/sled) embedded database.
///
/// Implements [`BlockStore`], [`UtxoStore`], and [`ValidatorStore`].
/// Uses sled multi-tree transactions for atomic `apply_block` / `rollback_block`.
pub struct SledStore {
    _db:        sled::Db,
    blocks_h:   sled::Tree,
    blocks_idx: sled::Tree,
    utxo:       sled::Tree,
    validators: sled::Tree,
    meta:       sled::Tree,
    spent_h:    sled::Tree,
}

impl SledStore {
    /// Opens (or creates) a sled database at `path` and initialises all trees.
    pub fn open(path: &Path) -> Result<Self, sled::Error> {
        let db = sled::open(path)?;
        Ok(Self {
            blocks_h:   db.open_tree(TREE_BLOCKS_H)?,
            blocks_idx: db.open_tree(TREE_BLOCKS_IDX)?,
            utxo:       db.open_tree(TREE_UTXO)?,
            validators: db.open_tree(TREE_VALIDATORS)?,
            meta:       db.open_tree(TREE_META)?,
            spent_h:    db.open_tree(TREE_SPENT)?,
            _db:        db,
        })
    }

    /// Opens a temporary in-memory sled database. Intended for tests.
    pub fn new_temporary() -> Self {
        let db = sled::Config::new().temporary(true).open().unwrap();
        Self {
            blocks_h:   db.open_tree(TREE_BLOCKS_H).unwrap(),
            blocks_idx: db.open_tree(TREE_BLOCKS_IDX).unwrap(),
            utxo:       db.open_tree(TREE_UTXO).unwrap(),
            validators: db.open_tree(TREE_VALIDATORS).unwrap(),
            meta:       db.open_tree(TREE_META).unwrap(),
            spent_h:    db.open_tree(TREE_SPENT).unwrap(),
            _db:        db,
        }
    }

    /// Returns `true` if the crash sentinel key is set in the meta tree.
    pub fn apply_in_progress_flag(&self) -> bool {
        self.meta.contains_key(META_APPLY_IN_PROGRESS).unwrap_or(false)
    }
}

// ── Encoding helpers ─────────────────────────────────────────────────────────

fn height_key(height: u64) -> [u8; 8] {
    height.to_be_bytes()
}

fn encode<T: serde::Serialize>(value: &T) -> StoreResult<Vec<u8>> {
    bincode::serialize(value).map_err(|e| StoreError::Backend(e.to_string()))
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> StoreResult<T> {
    bincode::deserialize(bytes).map_err(|e| StoreError::Backend(e.to_string()))
}

fn spent_key(height: u64, out_ref: &TxOutRef) -> StoreResult<Vec<u8>> {
    let mut key = height.to_be_bytes().to_vec();
    key.extend(encode(out_ref)?);
    Ok(key)
}

// ── BlockStore ────────────────────────────────────────────────────────────────

impl BlockStore for SledStore {
    fn get_by_height(&self, height: u64) -> StoreResult<Option<Block>> {
        match self.blocks_h.get(height_key(height)) {
            Ok(Some(bytes)) => decode(&bytes).map(Some),
            Ok(None)        => Ok(None),
            Err(e)          => Err(StoreError::Backend(e.to_string())),
        }
    }

    fn get_by_hash(&self, hash: &BlockHash) -> StoreResult<Option<Block>> {
        let height_bytes = match self.blocks_idx.get(hash) {
            Ok(Some(b)) => b,
            Ok(None)    => return Ok(None),
            Err(e)      => return Err(StoreError::Backend(e.to_string())),
        };
        let height = u64::from_be_bytes(
            height_bytes
                .as_ref()
                .try_into()
                .map_err(|_| StoreError::Backend("corrupt height index".into()))?,
        );
        self.get_by_height(height)
    }

    fn insert(&self, block: &Block) -> StoreResult<()> {
        let height_be  = height_key(block.header.height);
        let block_hash = block.hash();
        let encoded    = encode(block)?;

        (&self.blocks_h, &self.blocks_idx, &self.meta)
            .transaction(|(bh, bi, meta)| {
                bh.insert(&height_be, encoded.clone())?;
                bi.insert(&block_hash, &height_be)?;
                meta.insert(META_TIP, encode(&(block.header.height, block_hash)).unwrap())?;
                Ok(())
            })
            .map_err(|e: sled::transaction::TransactionError| {
                StoreError::Backend(e.to_string())
            })
    }

    fn tip(&self) -> StoreResult<Option<(u64, BlockHash)>> {
        match self.meta.get(META_TIP) {
            Ok(Some(bytes)) => decode::<(u64, BlockHash)>(&bytes).map(Some),
            Ok(None)        => Ok(None),
            Err(e)          => Err(StoreError::Backend(e.to_string())),
        }
    }

    fn iter_from(&self, height: u64) -> StoreResult<Vec<Block>> {
        self.blocks_h
            .range(height_key(height)..)
            .map(|res| {
                let (_, v) = res.map_err(|e| StoreError::Backend(e.to_string()))?;
                decode(&v)
            })
            .collect()
    }
}

// ── UtxoStore ─────────────────────────────────────────────────────────────────

impl UtxoStore for SledStore {
    fn get(&self, out_ref: &TxOutRef) -> StoreResult<Option<TxOutput>> {
        let key = encode(out_ref)?;
        match self.utxo.get(&key) {
            Ok(Some(bytes)) => decode(&bytes).map(Some),
            Ok(None)        => Ok(None),
            Err(e)          => Err(StoreError::Backend(e.to_string())),
        }
    }

    /// Atomically applies a block to the UTXO set.
    ///
    /// Uses a crash sentinel in the `meta` tree. On startup, if the sentinel exists:
    /// - Block stored → replay block insertion transaction.
    /// - Block missing → rollback UTXO changes using `spent_h`.
    fn apply_block(&self, block: &Block) -> StoreResult<()> {
        let height_be = height_key(block.header.height);

        // 1. Write crash sentinel before any mutation.
        self.meta
            .insert(META_APPLY_IN_PROGRESS, &height_be)
            .map_err(|e| StoreError::Backend(e.to_string()))?;

        // 2. UTXO update + record spent outputs (for rollback).
        let block_height = block.header.height;
        let txs = block.txs.clone();

        (&self.utxo, &self.spent_h)
            .transaction(|(utxo_txn, spent_txn)| {
                for tx in &txs {
                    let tx_hash = tx.hash();
                    // Record spent inputs before removing (needed for rollback).
                    for input in &tx.inputs {
                        if let Some(prev_out) = utxo_txn.get(encode(&input.out_ref).unwrap())? {
                            let skey = spent_key(block_height, &input.out_ref).unwrap();
                            spent_txn.insert(skey, prev_out)?;
                        }
                        utxo_txn.remove(encode(&input.out_ref).unwrap())?;
                    }
                    // Insert new outputs.
                    for (idx, output) in tx.outputs.iter().enumerate() {
                        let out_ref = TxOutRef { tx_hash, index: idx as u32 };
                        utxo_txn.insert(encode(&out_ref).unwrap(), encode(output).unwrap())?;
                    }
                }
                Ok(())
            })
            .map_err(|e: sled::transaction::TransactionError| {
                StoreError::Backend(e.to_string())
            })?;

        // 3. Clear crash sentinel.
        self.meta
            .remove(META_APPLY_IN_PROGRESS)
            .map_err(|e| StoreError::Backend(e.to_string()))?;

        Ok(())
    }

    /// Atomically reverts a block from the UTXO set.
    /// Restores spent inputs from the `spent_h` tree and removes created outputs.
    fn rollback_block(&self, block: &Block) -> StoreResult<()> {
        let block_height = block.header.height;
        let txs = block.txs.clone();

        (&self.utxo, &self.spent_h)
            .transaction(|(utxo_txn, spent_txn)| {
                for tx in txs.iter().rev() {
                    let tx_hash = tx.hash();
                    // Remove outputs created by this tx.
                    for idx in 0..tx.outputs.len() {
                        let out_ref = TxOutRef { tx_hash, index: idx as u32 };
                        utxo_txn.remove(encode(&out_ref).unwrap())?;
                    }
                    // Restore spent inputs from spent_h.
                    for input in &tx.inputs {
                        let skey = spent_key(block_height, &input.out_ref).unwrap();
                        if let Some(prev_bytes) = spent_txn.get(&skey)? {
                            utxo_txn.insert(encode(&input.out_ref).unwrap(), prev_bytes)?;
                            spent_txn.remove(skey)?;
                        }
                    }
                }
                Ok(())
            })
            .map_err(|e: sled::transaction::TransactionError| {
                StoreError::Backend(e.to_string())
            })?;

        // Remove block from chain.
        let height_be  = height_key(block_height);
        let block_hash = block.hash();
        (&self.blocks_h, &self.blocks_idx, &self.meta)
            .transaction(|(bh, bi, meta)| {
                bh.remove(&height_be)?;
                bi.remove(&block_hash)?;
                // Update tip to parent.
                let parent_height = block_height.saturating_sub(1);
                if let Some(parent_bytes) = bh.get(height_key(parent_height))? {
                    let parent: Block = decode(&parent_bytes).unwrap();
                    meta.insert(
                        META_TIP,
                        encode(&(parent_height, parent.hash())).unwrap(),
                    )?;
                } else {
                    meta.remove(META_TIP)?;
                }
                Ok(())
            })
            .map_err(|e: sled::transaction::TransactionError| {
                StoreError::Backend(e.to_string())
            })
    }

    fn balance(&self, address: &Address) -> StoreResult<u64> {
        self.utxo
            .iter()
            .map(|res| res.map_err(|e| StoreError::Backend(e.to_string())))
            .try_fold(0u64, |acc, res| {
                let (_, v) = res?;
                let output: TxOutput = decode(&v)?;
                Ok(if &output.address == address {
                    acc.saturating_add(output.amount)
                } else {
                    acc
                })
            })
    }

    fn list_utxos(&self, address: &Address) -> StoreResult<Vec<(TxOutRef, TxOutput)>> {
        self.utxo
            .iter()
            .map(|res| res.map_err(|e| StoreError::Backend(e.to_string())))
            .filter_map(|res| match res {
                Err(e) => Some(Err(e)),
                Ok((k, v)) => {
                    let out_ref: TxOutRef = match decode(&k) {
                        Ok(r)  => r,
                        Err(e) => return Some(Err(e)),
                    };
                    let output: TxOutput = match decode(&v) {
                        Ok(o)  => o,
                        Err(e) => return Some(Err(e)),
                    };
                    if &output.address == address {
                        Some(Ok((out_ref, output)))
                    } else {
                        None
                    }
                }
            })
            .collect()
    }
}

// ── ValidatorStore ────────────────────────────────────────────────────────────

impl ValidatorStore for SledStore {
    fn get(&self, address: &Address) -> StoreResult<Option<Validator>> {
        match self.validators.get(address.as_str().as_bytes()) {
            Ok(Some(bytes)) => decode(&bytes).map(Some),
            Ok(None)        => Ok(None),
            Err(e)          => Err(StoreError::Backend(e.to_string())),
        }
    }

    fn all_active(&self, slot: u64) -> StoreResult<Vec<Validator>> {
        self.validators
            .iter()
            .map(|res| res.map_err(|e| StoreError::Backend(e.to_string())))
            .filter_map(|res| match res {
                Ok((_, v)) => match decode::<Validator>(&v) {
                    Ok(validator) if validator.active_since <= slot && validator.stake > 0 => {
                        Some(Ok(validator))
                    }
                    Ok(_)  => None,
                    Err(e) => Some(Err(e)),
                },
                Err(e) => Some(Err(e)),
            })
            .collect()
    }

    fn upsert(&self, validator: &Validator) -> StoreResult<()> {
        let key   = validator.address.as_str().as_bytes().to_vec();
        let value = encode(validator)?;
        self.validators
            .insert(key, value)
            .map(|_| ())
            .map_err(|e| StoreError::Backend(e.to_string()))
    }
}
