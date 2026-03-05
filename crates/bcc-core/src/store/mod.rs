use thiserror::Error;

use crate::crypto::hash::BlockHash;
use crate::types::address::Address;
use crate::types::block::Block;
use crate::types::transaction::{TxOutRef, TxOutput};
use crate::types::validator::Validator;

/// Error returned by any storage operation.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("entry not found")]
    NotFound,
    #[error("storage backend error: {0}")]
    Backend(String),
}

/// Convenience alias for storage results.
pub type StoreResult<T> = Result<T, StoreError>;

/// Persistent storage interface for blocks.
/// Implementations may be in-memory (tests) or on disk (sled).
pub trait BlockStore: Send + Sync {
    /// Returns the block at the given height, or `None` if not yet stored.
    fn get_by_height(&self, height: u64) -> StoreResult<Option<Block>>;

    /// Returns the block matching the given hash, or `None` if not found.
    fn get_by_hash(&self, hash: &BlockHash) -> StoreResult<Option<Block>>;

    /// Persists a block. Overwrites any existing entry at the same height.
    fn insert(&self, block: &Block) -> StoreResult<()>;

    /// Returns the height and hash of the current chain tip, or `None` for an empty chain.
    fn tip(&self) -> StoreResult<Option<(u64, BlockHash)>>;

    /// Returns all blocks starting from the given height, in ascending order.
    fn iter_from(&self, height: u64) -> StoreResult<Vec<Block>>;
}

/// Persistent storage interface for the UTXO set.
/// Tracks all unspent transaction outputs.
pub trait UtxoStore: Send + Sync {
    /// Returns the unspent output at the given reference, or `None` if already spent or unknown.
    fn get(&self, out_ref: &TxOutRef) -> StoreResult<Option<TxOutput>>;

    /// Atomically applies a block: removes spent outputs and inserts new ones.
    fn apply_block(&self, block: &Block) -> StoreResult<()>;

    /// Atomically reverts a block: restores spent outputs and removes created ones.
    /// Used during chain reorganisations.
    fn rollback_block(&self, block: &Block) -> StoreResult<()>;

    /// Returns the total spendable balance of an address across all its UTXOs.
    fn balance(&self, address: &Address) -> StoreResult<u64>;
}

/// Persistent storage interface for the validator registry.
pub trait ValidatorStore: Send + Sync {
    /// Returns the validator registered at the given address, or `None` if unknown.
    fn get(&self, address: &Address) -> StoreResult<Option<Validator>>;

    /// Returns all validators that were active (staked) before the given slot.
    fn all_active(&self, slot: u64) -> StoreResult<Vec<Validator>>;

    /// Inserts or updates a validator entry.
    fn upsert(&self, validator: &Validator) -> StoreResult<()>;
}
