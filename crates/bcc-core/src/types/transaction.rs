use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::crypto::hash::BlockHash;
use crate::types::address::Address;

/// 32-byte identifier of a transaction.
pub type TxHash = BlockHash;

/// Points to a specific output within a past transaction (UTXO model).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TxOutRef {
    /// Hash of the transaction containing the output.
    pub tx_hash: TxHash,
    /// Index of the output within that transaction.
    pub index: u32,
}

/// A signed reference to an existing unspent output being consumed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxInput {
    /// The unspent output being spent.
    pub out_ref: TxOutRef,
    /// Ed25519 signature authorising the spend.
    pub signature: Signature,
    /// Public key of the spender, used to verify the signature.
    pub pubkey: VerifyingKey,
}

/// A new output created by a transaction, assigning an amount to an address.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxOutput {
    /// Amount of tokens assigned to this output.
    pub amount: u64,
    /// Recipient address.
    pub address: Address,
}

/// Classifies the intent of a transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TxKind {
    /// Simple token transfer between addresses.
    Transfer,
    /// Locks tokens as validator stake.
    Stake { amount: u64 },
    /// Unlocks previously staked tokens.
    Unstake { amount: u64 },
}

/// An atomic unit of value transfer on the BetterCallChain.
/// Follows a UTXO model: inputs reference past outputs, outputs create new ones.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    /// The type and intent of this transaction.
    pub kind: TxKind,
    /// Unspent outputs consumed by this transaction.
    pub inputs: Vec<TxInput>,
    /// New outputs created by this transaction.
    pub outputs: Vec<TxOutput>,
}

impl Transaction {
    /// Computes the unique hash identifying this transaction.
    pub fn hash(&self) -> TxHash {
        todo!()
    }
}
