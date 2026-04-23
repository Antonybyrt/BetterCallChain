use bcc_core::{
    crypto::hash::{sha256d, BlockHash},
    store::{BlockStore, UtxoStore, ValidatorStore},
    types::{
        address::Address,
        block::{Block, BlockHeader},
        transaction::{Transaction, TxKind, TxOutput},
        validator::Validator,
    },
};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;

use crate::error::NodeError;

/// Genesis message embedded in the first block of BetterCallChain.
/// this message is permanently hashed into the chain's identity.
pub const GENESIS_MESSAGE: &str =
    "Slipping Jimmy, counselor at law.";

/// Configuration for the genesis block, loaded from `genesis.toml`.
#[derive(Debug, Deserialize)]
pub struct GenesisConfig {
    /// Unix timestamp of the genesis block.
    pub timestamp: i64,
    /// Initial set of validators: each entry is (address, hex pubkey, stake).
    pub validators: Vec<GenesisValidator>,
    /// Initial token allocations: each entry mints tokens into the UTXO set at genesis.
    #[serde(default)]
    pub accounts: Vec<GenesisAccount>,
}

/// One entry in the genesis validator set.
#[derive(Debug, Deserialize)]
pub struct GenesisValidator {
    /// `bcs1...` address of the validator.
    pub address: String,
    /// Hex-encoded 32-byte Ed25519 public key.
    pub pubkey: String,
    /// Initial stake amount.
    pub stake: u64,
}

/// One entry in the genesis token allocation.
/// Creates a single UTXO owned by `address` with the given `balance`.
#[derive(Debug, Deserialize)]
pub struct GenesisAccount {
    /// `bcs1...` recipient address.
    pub address: String,
    /// Amount of tokens to credit at genesis.
    pub balance: u64,
}

impl GenesisConfig {
    /// Loads a `GenesisConfig` from a TOML file.
    pub fn from_file(path: &std::path::Path) -> Result<Self, NodeError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| NodeError::Config(format!("genesis file: {e}")))?;
        toml::from_str(&content).map_err(|e| NodeError::Config(format!("genesis parse: {e}")))
    }
}

/// Applies the genesis state to fresh stores.
///
/// Idempotent: if a genesis block already exists at height 0, returns immediately.
/// Logs the genesis message — permanently embedded in the chain's Merkle root.
pub fn apply_genesis(
    config: &GenesisConfig,
    blocks: &dyn BlockStore,
    utxo: &dyn UtxoStore,
    validators: &dyn ValidatorStore,
) -> Result<(), NodeError> {
    // Idempotency check.
    if blocks
        .get_by_height(0)
        .map_err(NodeError::Store)?
        .is_some()
    {
        return Ok(());
    }

    tracing::info!(message = GENESIS_MESSAGE, "applying genesis block");

    // Build coinbase transactions for initial account balances.
    let alloc_txs = config
        .accounts
        .iter()
        .map(|entry| {
            let address = Address::validate(&entry.address)
                .map_err(|e| NodeError::Config(format!("genesis account address: {e}")))?;
            Ok(Transaction {
                kind:    TxKind::Transfer,
                inputs:  vec![],
                outputs: vec![TxOutput { amount: entry.balance, address }],
            })
        })
        .collect::<Result<Vec<_>, NodeError>>()?;

    let genesis = build_genesis_block(config.timestamp, alloc_txs);
    blocks.insert(&genesis).map_err(NodeError::Store)?;
    utxo.apply_block(&genesis).map_err(NodeError::Store)?;

    for entry in &config.validators {
        let address = Address::validate(&entry.address)
            .map_err(|e| NodeError::Config(format!("genesis validator address: {e}")))?;

        let pubkey_bytes = hex::decode(&entry.pubkey)
            .map_err(|e| NodeError::Config(format!("genesis validator pubkey hex: {e}")))?;
        let pubkey_array: [u8; 32] = pubkey_bytes
            .try_into()
            .map_err(|_| NodeError::Config("genesis validator pubkey must be 32 bytes".into()))?;
        let pubkey = VerifyingKey::from_bytes(&pubkey_array)
            .map_err(|e| NodeError::Config(format!("genesis validator pubkey: {e}")))?;

        validators
            .upsert(&Validator { address, pubkey, stake: entry.stake, active_since: 0 })
            .map_err(NodeError::Store)?;
    }

    tracing::info!(
        hash = %hex::encode(genesis.hash()),
        accounts = config.accounts.len(),
        validators = config.validators.len(),
        "genesis block applied"
    );
    Ok(())
}

/// Builds the genesis block.
///
/// `prev_hash` is all-zeros. The `merkle_root` is derived from the [`GENESIS_MESSAGE`],
/// permanently encoding it into the chain's identity — like Bitcoin's newspaper headline.
/// The proposer signature is zeroed (genesis has no elected proposer).
/// `alloc_txs` are coinbase-style transactions (no inputs) that mint the initial balances.
fn build_genesis_block(timestamp: i64, alloc_txs: Vec<Transaction>) -> Block {
    Block {
        header: BlockHeader {
            prev_hash:   [0u8; 32],
            merkle_root: genesis_merkle_root(),
            timestamp,
            height:      0,
            slot:        0,
            proposer:    Address::from_pubkey_bytes(&[0u8; 32]),
        },
        signature: Signature::from_bytes(&[0u8; 64]),
        txs: alloc_txs,
    }
}

/// Hashes the genesis message into a [`BlockHash`].
/// This root is unique per chain — changing the message produces a completely different genesis.
fn genesis_merkle_root() -> BlockHash {
    sha256d(GENESIS_MESSAGE.as_bytes())
}
