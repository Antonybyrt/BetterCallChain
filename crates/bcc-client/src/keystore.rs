use std::path::Path;

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use argon2::{Algorithm, Argon2, Params, Version};
use bcc_core::types::address::Address;
use ed25519_dalek::SigningKey;
use rand::RngExt;
use serde::{Deserialize, Serialize};

use crate::error::ClientError;

/// On-disk representation of an encrypted Ed25519 keypair.
///
/// All sensitive material is stored hex-encoded and AES-256-GCM encrypted.
/// The `address` field is stored in plaintext so commands that only need the
/// address (e.g. `balance`) do not require decryption.
///
/// JSON schema:
/// ```json
/// {
///   "version": 1,
///   "address": "bcs1<40 hex chars>",
///   "salt":    "<64 hex — 32-byte Argon2id salt>",
///   "nonce":   "<24 hex — 12-byte AES-GCM nonce>",
///   "ciphertext": "<96 hex — 32-byte key seed + 16-byte GCM tag>"
/// }
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct KeystoreFile {
    /// Schema version. Always `1` for this implementation.
    pub version: u8,
    /// Wallet address derived from the public key (plaintext).
    pub address: String,
    /// Hex-encoded 32-byte Argon2id salt.
    pub salt: String,
    /// Hex-encoded 12-byte AES-256-GCM nonce.
    pub nonce: String,
    /// Hex-encoded ciphertext: AES-GCM(signing_key.to_bytes(), key=argon2(passphrase, salt)).
    /// Length = 96 hex chars (32-byte plaintext + 16-byte GCM tag = 48 bytes).
    pub ciphertext: String,
}

impl KeystoreFile {
    /// Generates a fresh Ed25519 keypair, encrypts the signing key with `passphrase`,
    /// and writes the keystore atomically to `path`.
    ///
    /// Returns the derived wallet address so the caller can display it immediately.
    /// The parent directory of `path` must already exist (create with
    /// `std::fs::create_dir_all` before calling).
    pub fn create(path: &Path, passphrase: &str) -> Result<Address, ClientError> {
        // 1. Generate keypair from a 32-byte random seed.
        let mut seed = [0u8; 32];
        rand::rng().fill(&mut seed);
        let signing_key = SigningKey::from_bytes(&seed);
        let address = Address::from_pubkey_bytes(signing_key.verifying_key().as_bytes());

        // 2. Generate random salt (32 bytes) for Argon2id.
        let mut salt = [0u8; 32];
        rand::rng().fill(&mut salt);

        // 3. Generate random nonce (12 bytes) for AES-256-GCM via OsRng.
        //    OsRng is used here (not thread_rng) because nonces require a CSPRNG
        //    with no possibility of seed reuse across processes.
        let nonce_arr = Aes256Gcm::generate_nonce(&mut OsRng);

        // 4. Derive 32-byte AES key from passphrase + salt using Argon2id.
        let aes_key = derive_key(passphrase, &salt)?;

        // 5. Encrypt the raw 32-byte signing key seed.
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&aes_key));
        let ciphertext = cipher
            .encrypt(&nonce_arr, signing_key.to_bytes().as_slice())
            .map_err(|_| ClientError::Config("AES-GCM encryption failed".into()))?;

        let ks = KeystoreFile {
            version:    1,
            address:    address.to_string(),
            salt:       hex::encode(salt),
            nonce:      hex::encode(nonce_arr),
            ciphertext: hex::encode(ciphertext),
        };

        // 6. Atomic write: serialize → tmp file → rename.
        //    On Windows, rename fails if the destination exists; remove it first.
        let json = serde_json::to_string_pretty(&ks)?;
        let tmp  = path.with_extension("tmp");
        std::fs::write(&tmp, &json)?;
        #[cfg(target_os = "windows")]
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        std::fs::rename(&tmp, path)?;

        Ok(address)
    }

    /// Loads the keystore at `path` and decrypts the signing key with `passphrase`.
    ///
    /// Returns `ClientError::WrongPassphrase` if the AES-GCM tag verification fails,
    /// which covers both wrong passphrase and corrupted ciphertext.
    pub fn load_and_decrypt(path: &Path, passphrase: &str) -> Result<SigningKey, ClientError> {
        let json = std::fs::read_to_string(path)?;
        let ks: KeystoreFile = serde_json::from_str(&json)?;

        let salt        = hex::decode(&ks.salt)?;
        let nonce_bytes = hex::decode(&ks.nonce)?;
        let ciphertext  = hex::decode(&ks.ciphertext)?;

        let nonce   = Nonce::from_slice(&nonce_bytes);
        let aes_key = derive_key(passphrase, &salt)?;
        let cipher  = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&aes_key));

        let plaintext = cipher
            .decrypt(nonce, ciphertext.as_slice())
            .map_err(|_| ClientError::WrongPassphrase)?;

        let seed: [u8; 32] = plaintext
            .try_into()
            .map_err(|_| ClientError::Config("decrypted seed is not 32 bytes".into()))?;

        Ok(SigningKey::from_bytes(&seed))
    }

    /// Reads only the `address` field from the keystore without decryption.
    ///
    /// Useful for commands that only need the address (e.g. displaying balance)
    /// without prompting for a passphrase.
    pub fn read_address(path: &Path) -> Result<Address, ClientError> {
        let json = std::fs::read_to_string(path)?;
        let ks: KeystoreFile = serde_json::from_str(&json)?;
        Address::validate(&ks.address).map_err(ClientError::Address)
    }
}

/// Derives a 32-byte AES-256 key from `passphrase` and `salt` using Argon2id.
///
/// Parameters (OWASP 2023 interactive-login minimum, ~0.5 s on a modern laptop):
/// - Memory: 64 MiB (`m_cost = 65536`)
/// - Iterations: 3 (`t_cost = 3`)
/// - Parallelism: 4 lanes (`p_cost = 4`)
/// - Output: 32 bytes (one AES-256 key)
fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; 32], ClientError> {
    let params = Params::new(65536, 3, 4, Some(32))
        .map_err(|e| ClientError::Config(e.to_string()))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut output = [0u8; 32];
    argon2
        .hash_password_into(passphrase.as_bytes(), salt, &mut output)
        .map_err(|e| ClientError::Config(e.to_string()))?;
    Ok(output)
}
