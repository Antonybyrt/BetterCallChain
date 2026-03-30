use bcc_core::store::StoreError;
use thiserror::Error;

/// All error variants that can occur within the `bcc-node` binary.
#[derive(Debug, Error)]
pub enum NodeError {
    /// A storage operation failed (propagated from bcc-core store traits).
    #[error("store: {0}")]
    Store(#[from] StoreError),

    /// A block or transaction failed validation.
    #[error("validation: {0}")]
    Validation(String),

    /// A P2P networking or serialization error.
    #[error("p2p: {0}")]
    P2p(String),

    /// A low-level I/O error (TCP read/write failure).
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),

    /// A sled database error.
    #[error("sled: {0}")]
    Sled(#[from] sled::Error),

    /// Configuration file loading or parsing failed.
    #[error("config: {0}")]
    Config(String),

    /// The node received a shutdown signal and is stopping gracefully.
    #[error("node shutting down")]
    Shutdown,
}
