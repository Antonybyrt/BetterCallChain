use std::net::SocketAddr;
use std::path::PathBuf;

use bcc_core::types::address::Address;
use ed25519_dalek::SigningKey;
use serde::Deserialize;

use crate::error::NodeError;

/// Full node configuration, loaded from a TOML file and overridable via `BCC_*` env variables.
#[derive(Clone)]
pub struct NodeConfig {
    /// TCP address the P2P server listens on (e.g. `0.0.0.0:8333`).
    pub listen_addr: SocketAddr,
    /// List of peer addresses to connect to on startup.
    pub bootstrap_peers: Vec<SocketAddr>,
    /// Duration of one PoS slot in seconds.
    pub slot_duration_secs: u64,
    /// TCP address the HTTP API listens on (e.g. `0.0.0.0:8080`).
    pub http_addr: SocketAddr,
    /// Path to the sled database directory.
    pub sled_path: PathBuf,
    /// Maximum number of transactions held in the mempool.
    pub mempool_max_size: usize,
    /// Path to the genesis configuration file.
    pub genesis_path: PathBuf,
    /// This node's wallet address (must correspond to `my_signing_key`).
    pub my_address: Address,
    /// Ed25519 signing key used to sign proposed blocks.
    /// Never printed in Debug output.
    pub my_signing_key: SigningKey,
}

/// Manual `Debug` implementation that redacts the signing key.
impl std::fmt::Debug for NodeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeConfig")
            .field("listen_addr", &self.listen_addr)
            .field("bootstrap_peers", &self.bootstrap_peers)
            .field("slot_duration_secs", &self.slot_duration_secs)
            .field("http_addr", &self.http_addr)
            .field("sled_path", &self.sled_path)
            .field("mempool_max_size", &self.mempool_max_size)
            .field("my_address", &self.my_address.as_str())
            .field("my_signing_key", &"[REDACTED]")
            .finish()
    }
}

/// Raw deserialization target — all fields are strings or primitives for easy TOML/env parsing.
#[derive(Debug, Deserialize)]
struct RawConfig {
    listen_addr:        String,
    bootstrap_peers:    Vec<String>,
    slot_duration_secs: u64,
    http_addr:          String,
    sled_path:          String,
    mempool_max_size:   usize,
    genesis_path:       String,
    my_address:         String,
    /// Hex-encoded 32-byte Ed25519 secret key.
    my_signing_key:     String,
}

impl NodeConfig {
    /// Loads configuration from a TOML file at `path`, then applies `BCC_*` environment overrides.
    pub fn from_file(path: &str) -> Result<Self, NodeError> {
        let raw: RawConfig = config::Config::builder()
            .add_source(config::File::with_name(path))
            .add_source(config::Environment::with_prefix("BCC").separator("__"))
            .build()
            .map_err(|e| NodeError::Config(e.to_string()))?
            .try_deserialize()
            .map_err(|e| NodeError::Config(e.to_string()))?;

        Self::from_raw(raw)
    }

    fn from_raw(raw: RawConfig) -> Result<Self, NodeError> {
        let listen_addr = raw
            .listen_addr
            .parse::<SocketAddr>()
            .map_err(|e| NodeError::Config(format!("listen_addr: {e}")))?;

        let http_addr = raw
            .http_addr
            .parse::<SocketAddr>()
            .map_err(|e| NodeError::Config(format!("http_addr: {e}")))?;

        let bootstrap_peers = raw
            .bootstrap_peers
            .iter()
            .map(|s| {
                s.parse::<SocketAddr>()
                    .map_err(|e| NodeError::Config(format!("bootstrap_peers: {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let my_address = Address::validate(&raw.my_address)
            .map_err(|e| NodeError::Config(format!("my_address: {e}")))?;

        let key_bytes = hex::decode(&raw.my_signing_key)
            .map_err(|e| NodeError::Config(format!("my_signing_key hex: {e}")))?;
        let mut key_array: [u8; 32] = key_bytes
            .try_into()
            .map_err(|_| NodeError::Config("my_signing_key must be 32 bytes".into()))?;
        let my_signing_key = SigningKey::from_bytes(&key_array);

        // Zero raw key bytes — SigningKey holds the material from here on.
        key_array.fill(0);

        // Verify the signing key actually corresponds to the declared address.
        // Prevents silent misconfiguration where address and key are mismatched.
        let derived = bcc_core::types::address::Address::from_pubkey_bytes(
            my_signing_key.verifying_key().as_bytes(),
        );
        if derived != my_address {
            return Err(NodeError::Config(format!(
                "my_signing_key does not match my_address: \
                 key derives to {derived} but config declares {my_address}"
            )));
        }

        Ok(Self {
            listen_addr,
            bootstrap_peers,
            slot_duration_secs: raw.slot_duration_secs,
            http_addr,
            sled_path: PathBuf::from(&raw.sled_path),
            mempool_max_size: raw.mempool_max_size,
            genesis_path: PathBuf::from(&raw.genesis_path),
            my_address,
            my_signing_key,
        })
    }
}
