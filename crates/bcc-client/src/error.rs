use thiserror::Error;

/// All errors that can occur in the `bcc-client` binary.
#[derive(Debug, Error)]
pub enum ClientError {
    /// The keystore file could not be read or written.
    #[error("keystore I/O: {0}")]
    KeystoreIo(#[from] std::io::Error),

    /// The keystore JSON is malformed or has an unexpected structure.
    #[error("keystore parse: {0}")]
    KeystoreParse(#[from] serde_json::Error),

    /// The passphrase was incorrect (AES-GCM authentication tag mismatch)
    /// or the keystore file is corrupted.
    #[error("wrong passphrase or corrupted keystore")]
    WrongPassphrase,

    /// Two passphrase prompts did not match during wallet creation.
    #[error("passphrases do not match")]
    PassphraseMismatch,

    /// An HTTP request to the node failed at the transport level.
    #[error("RPC error: {0}")]
    Rpc(#[from] reqwest::Error),

    /// The node returned a non-2xx HTTP status.
    #[error("node returned HTTP {status}: {body}")]
    NodeError { status: u16, body: String },

    /// An address string failed validation.
    #[error("invalid address: {0}")]
    Address(#[from] bcc_core::types::address::AddressError),

    /// Coin selection could not satisfy the requested amount.
    #[error("insufficient funds: have {have}, need {need}")]
    InsufficientFunds { have: u64, need: u64 },

    /// Serde JSON serialization failed (e.g. when building the signing message).
    #[error("serialization: {0}")]
    Serialization(String),

    /// A hex decode operation failed.
    #[error("hex decode: {0}")]
    Hex(#[from] hex::FromHexError),

    /// The keystore file already exists; use `--force` to overwrite.
    #[error("keystore already exists at {0}")]
    KeystoreExists(String),

    /// Configuration loading or validation failed.
    #[error("config: {0}")]
    Config(String),
}
