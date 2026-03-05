use thiserror::Error;

/// Top-level error type for the BetterCallChain core library.
/// All sub-module errors can be wrapped into this type for uniform error propagation.
#[derive(Debug, Error)]
pub enum BccError {
    /// A cryptographic operation failed (e.g. signature verification).
    #[error("cryptographic error: {0}")]
    Crypto(String),

    /// An address is malformed or fails validation.
    #[error("address error: {0}")]
    Address(String),

    /// A block failed validation.
    #[error("block validation error: {0}")]
    BlockValidation(String),

    /// A transaction failed validation.
    #[error("transaction validation error: {0}")]
    TxValidation(String),

    /// A storage operation failed.
    #[error("store error: {0}")]
    Store(String),

    /// A consensus rule was violated (e.g. wrong proposer).
    #[error("consensus error: {0}")]
    Consensus(String),
}
