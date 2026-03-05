use ed25519_dalek::Signature;
use serde::{Deserialize, Serialize};

use crate::crypto::hash::BlockHash;
use crate::types::address::Address;
use crate::types::transaction::Transaction;

/// The fixed-size header of a block, used for hashing and chain linking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockHeader {
    /// Hash of the parent block. All-zeros for the genesis block.
    pub prev_hash: BlockHash,
    /// Merkle root of all transactions in this block.
    pub merkle_root: BlockHash,
    /// Unix timestamp (seconds) at the time of block production.
    pub timestamp: i64,
    /// Height of this block in the chain (genesis = 0).
    pub height: u64,
    /// PoS slot number during which this block was proposed.
    pub slot: u64,
    /// Address of the validator that proposed this block.
    pub proposer: Address,
}

/// A complete block containing a header, a proposer signature, and transactions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    /// Block metadata used for chain linking and consensus verification.
    pub header: BlockHeader,
    /// Ed25519 signature of the header by the proposer.
    pub signature: Signature,
    /// Ordered list of transactions included in this block.
    pub txs: Vec<Transaction>,
}

impl Block {
    /// Computes the hash of this block's header.
    pub fn hash(&self) -> BlockHash {
        todo!()
    }

    /// Computes the Merkle root of a list of transactions.
    pub fn compute_merkle_root(_txs: &[Transaction]) -> BlockHash {
        todo!()
    }
}
