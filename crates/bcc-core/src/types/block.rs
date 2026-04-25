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
    /// Computes the hash of this block's header (signature excluded by design).
    pub fn hash(&self) -> BlockHash {
        let bytes = serde_json::to_vec(&self.header).expect("BlockHeader serialization is infallible");
        crate::crypto::hash::sha256d(&bytes)
    }

    /// Computes a binary Merkle root from a slice of transactions.
    /// Returns all-zeros for an empty transaction list.
    /// Odd-length layers duplicate the last leaf (Bitcoin convention).
    pub fn compute_merkle_root(txs: &[Transaction]) -> BlockHash {
        if txs.is_empty() {
            return [0u8; 32];
        }

        let mut layer: Vec<BlockHash> = txs.iter().map(|tx| tx.hash()).collect();

        while layer.len() > 1 {
            if layer.len() % 2 != 0 {
                if let Some(&last) = layer.last() {
                    layer.push(last);
                }
            }
            layer = layer
                .chunks(2)
                .map(|pair| {
                    let mut combined = [0u8; 64];
                    combined[..32].copy_from_slice(&pair[0]);
                    combined[32..].copy_from_slice(&pair[1]);
                    crate::crypto::hash::sha256d(&combined)
                })
                .collect();
        }

        layer[0]
    }
}
