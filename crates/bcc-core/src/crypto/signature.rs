use ed25519_dalek::{Signature, Verifier, VerifyingKey};

use crate::error::BccError;

/// Verifies an Ed25519 signature against a message and public key.
/// Returns `Ok(())` if valid, or [`BccError::Crypto`] if not.
pub fn verify(pubkey: &VerifyingKey, msg: &[u8], sig: &Signature) -> Result<(), BccError> {
    pubkey
        .verify(msg, sig)
        .map_err(|e| BccError::Crypto(e.to_string()))
}
