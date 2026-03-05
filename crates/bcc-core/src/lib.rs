/// BetterCallChain core library.
/// Contains all pure blockchain logic: types, cryptography, consensus, validation, and storage traits.
/// No I/O or networking — this crate is fully side-effect free.
pub mod types;
pub mod crypto;
pub mod consensus;
pub mod validation;
pub mod store;
pub mod error;
