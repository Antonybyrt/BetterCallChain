use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::crypto::hash::sha256;

/// The human-readable prefix for all BetterCallChain addresses.
const PREFIX: &str = "bcs1";

/// A BetterCallChain wallet address.
///
/// Always starts with `bcs1` followed by a hex-encoded public key hash.
/// Use [`Address::from_pubkey_bytes`] to derive one, or [`Address::validate`] to parse a string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Address(String);

impl Address {
    /// Derives an address from the raw bytes of an Ed25519 public key.
    /// Hashes the key with SHA-256 and encodes the first 20 bytes as hex.
    pub fn from_pubkey_bytes(pubkey_bytes: &[u8]) -> Self {
        let hash = sha256(pubkey_bytes);
        let payload = hex::encode(&hash[..20]);
        Address(format!("{}{}", PREFIX, payload))
    }

    /// Parses and validates a raw address string.
    /// Returns an error if the prefix or length is invalid.
    pub fn validate(s: &str) -> Result<Self, AddressError> {
        if !s.starts_with(PREFIX) {
            return Err(AddressError::InvalidPrefix);
        }
        let payload = &s[PREFIX.len()..];
        if payload.len() != 40 {
            return Err(AddressError::InvalidLength);
        }
        if payload.chars().any(|c| !c.is_ascii_hexdigit()) {
            return Err(AddressError::InvalidCharacter);
        }
        Ok(Address(s.to_string()))
    }

    /// Returns the address as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Errors that can occur when parsing or validating a [`Address`].
#[derive(Debug, Error)]
pub enum AddressError {
    #[error("address must start with '{}'", PREFIX)]
    InvalidPrefix,
    #[error("address payload must be 40 hex characters")]
    InvalidLength,
    #[error("address contains non-hex character")]
    InvalidCharacter,
}
