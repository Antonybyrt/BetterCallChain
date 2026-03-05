use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};

use crate::types::address::Address;

/// A registered validator that participates in block production.
/// Validators are elected per slot proportionally to their staked amount.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Validator {
    /// The address used to identify this validator on-chain.
    pub address: Address,
    /// The public key used to verify blocks proposed by this validator.
    pub pubkey: VerifyingKey,
    /// Amount of tokens staked by this validator.
    pub stake: u64,
    /// The slot number at which this validator became eligible to propose blocks.
    pub active_since: u64,
}
