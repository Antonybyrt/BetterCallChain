use sha2::{Digest, Sha256};

/// 32-byte digest used as block and transaction identifier.
pub type BlockHash = [u8; 32];

/// Computes SHA-256(SHA-256(data)).
/// Used for block and transaction hashes.
pub fn sha256d(data: &[u8]) -> BlockHash {
    let first: [u8; 32] = Sha256::digest(data).into();
    Sha256::digest(first).into()
}

/// Computes a single SHA-256 hash.
/// Used for address derivation.
pub fn sha256(data: &[u8]) -> BlockHash {
    Sha256::digest(data).into()
}
