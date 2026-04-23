use std::path::PathBuf;

use bcc_core::types::address::Address;
use clap::{Parser, Subcommand};
use ed25519_dalek::SigningKey;
use rand::RngExt;
use serde::Serialize;

use crate::{
    config::ClientConfig,
    error::ClientError,
    keystore::KeystoreFile,
    rpc::RpcClient,
    wallet,
};

// ── CLI structs ───────────────────────────────────────────────────────────────

/// BetterCallChain command-line client.
#[derive(Parser)]
#[command(
    name    = "bcc-client",
    version = env!("CARGO_PKG_VERSION"),
    about   = "BetterCallChain wallet and chain interaction tool",
)]
pub struct Cli {
    /// Node HTTP base URL. Overrides the BCC_RPC_URL env var and the default (http://127.0.0.1:8080).
    #[arg(long, global = true, value_name = "URL")]
    pub rpc_url: Option<String>,

    #[command(subcommand)]
    pub command: Commands,
}

/// Top-level commands.
#[derive(Subcommand)]
pub enum Commands {
    /// Wallet key-management commands.
    Wallet {
        #[command(subcommand)]
        sub: WalletCommands,
    },
    /// Fetch the confirmed UTXO balance of an address.
    Balance {
        /// BetterCallChain address (must start with `bcs1`).
        address: String,
    },
    /// Send tokens from the local wallet to a recipient address.
    Send {
        /// Recipient address.
        to: String,
        /// Amount of tokens to transfer.
        amount: u64,
        /// Path to the keystore file.
        #[arg(long, value_name = "PATH")]
        keystore: Option<PathBuf>,
    },
    /// Chain information commands.
    Chain {
        #[command(subcommand)]
        sub: ChainCommands,
    },
    /// Node configuration commands.
    Node {
        #[command(subcommand)]
        sub: NodeCommands,
    },
}

/// Wallet sub-commands.
#[derive(Subcommand)]
pub enum WalletCommands {
    /// Generate a new Ed25519 keypair and save an encrypted keystore.
    New {
        /// Path to write the keystore file
        /// (default: ~/.bcc/keystore.json).
        #[arg(long, value_name = "PATH")]
        keystore: Option<PathBuf>,
    },
    /// Show the wallet address stored in the keystore.
    ///
    /// Decrypts the keystore to verify the passphrase is correct,
    /// then displays the address. No network call is made.
    Show {
        /// Path to the keystore file
        /// (default: ~/.bcc/keystore.json).
        #[arg(long, value_name = "PATH")]
        keystore: Option<PathBuf>,
    },
}

/// Chain sub-commands.
#[derive(Subcommand)]
pub enum ChainCommands {
    /// Show the current chain tip (height and hash).
    Tip,
}

/// Node sub-commands.
#[derive(Subcommand)]
pub enum NodeCommands {
    /// Generate a node configuration TOML file with a fresh or existing signing key.
    ///
    /// If --keystore is provided, the signing key is read from the (decrypted) keystore.
    /// Otherwise, a fresh Ed25519 keypair is generated and the address + public key are
    /// printed so they can be added to genesis.toml.
    Init {
        /// Path to write the generated node config file.
        #[arg(short, long, value_name = "PATH")]
        output: PathBuf,

        /// Bootstrap peers (repeat for multiple, e.g. --peer 172.30.0.3:8333).
        #[arg(long, value_name = "ADDR")]
        peer: Vec<String>,

        /// Use signing key from an existing keystore instead of generating a new one.
        #[arg(long, value_name = "PATH")]
        keystore: Option<PathBuf>,

        /// P2P listen address.
        #[arg(long, value_name = "ADDR", default_value = "0.0.0.0:8333")]
        listen_addr: String,

        /// HTTP API listen address.
        #[arg(long, value_name = "ADDR", default_value = "0.0.0.0:8080")]
        http_addr: String,

        /// Path to the sled database directory inside the container/host.
        #[arg(long, value_name = "PATH", default_value = "/data/node")]
        sled_path: String,

        /// Path to the genesis TOML file.
        #[arg(long, value_name = "PATH", default_value = "/app/config/genesis.toml")]
        genesis_path: String,

        /// Slot duration in seconds.
        #[arg(long, value_name = "SECS", default_value = "5")]
        slot_duration: u64,

        /// Maximum number of transactions in the mempool.
        #[arg(long, value_name = "N", default_value = "10000")]
        mempool_max_size: usize,
    },
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Dispatches the parsed CLI command to the appropriate handler.
pub async fn run(cli: Cli) -> Result<(), ClientError> {
    let node_url = cli.rpc_url.unwrap_or_else(|| ClientConfig::default().node_url);

    match cli.command {
        Commands::Wallet { sub } => match sub {
            WalletCommands::New { keystore }  => cmd_wallet_new(keystore).await,
            WalletCommands::Show { keystore } => cmd_wallet_show(keystore),
        },
        Commands::Balance { address } => cmd_balance(address, node_url).await,
        Commands::Send { to, amount, keystore } => {
            cmd_send(to, amount, keystore, node_url).await
        }
        Commands::Chain { sub } => match sub {
            ChainCommands::Tip => cmd_chain_tip(node_url).await,
        },
        Commands::Node { sub } => match sub {
            NodeCommands::Init {
                output, peer, keystore, listen_addr, http_addr,
                sled_path, genesis_path, slot_duration, mempool_max_size,
            } => cmd_node_init(
                output, peer, keystore, listen_addr, http_addr,
                sled_path, genesis_path, slot_duration, mempool_max_size,
            ),
        },
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `wallet new` — generates a keypair, prompts for passphrase twice, writes keystore.
async fn cmd_wallet_new(keystore_flag: Option<PathBuf>) -> Result<(), ClientError> {
    let path = keystore_flag.unwrap_or_else(|| ClientConfig::default().keystore_path);

    if path.exists() {
        return Err(ClientError::KeystoreExists(path.display().to_string()));
    }

    let pass1 = rpassword::prompt_password("Enter passphrase: ")
        .map_err(ClientError::KeystoreIo)?;
    let pass2 = rpassword::prompt_password("Confirm passphrase: ")
        .map_err(ClientError::KeystoreIo)?;

    if pass1 != pass2 {
        return Err(ClientError::PassphraseMismatch);
    }

    // path.parent() returns None only for the root path `/`, which cannot happen
    // here since the path is at minimum `./keystore.json`. Still handled properly
    // via ok_or_else to respect the zero-unwrap convention.
    let parent = path
        .parent()
        .ok_or_else(|| ClientError::Config(
            format!("keystore path has no parent directory: {}", path.display())
        ))?;
    std::fs::create_dir_all(parent)?;

    let address = KeystoreFile::create(&path, &pass1)?;
    println!("Wallet created.");
    println!("Address:  {address}");
    println!("Keystore: {}", path.display());
    Ok(())
}

/// `wallet show` — decrypts keystore to verify passphrase, then displays address.
fn cmd_wallet_show(keystore_flag: Option<PathBuf>) -> Result<(), ClientError> {
    let path = keystore_flag.unwrap_or_else(|| ClientConfig::default().keystore_path);

    let passphrase = rpassword::prompt_password("Passphrase: ")
        .map_err(ClientError::KeystoreIo)?;

    // Decrypt to verify the passphrase is correct.
    let key = KeystoreFile::load_and_decrypt(&path, &passphrase)?;
    let address = Address::from_pubkey_bytes(key.verifying_key().as_bytes());

    println!("Address: {address}");
    Ok(())
}

/// `balance <address>` — fetches the confirmed UTXO balance from the node.
async fn cmd_balance(address: String, node_url: String) -> Result<(), ClientError> {
    Address::validate(&address)?;
    let rpc = RpcClient::new(node_url);
    let resp     = rpc.get_balance(&address).await?;
    println!("Address: {}", resp.address);
    println!("Balance: {}", resp.balance);
    Ok(())
}

/// `send <to> <amount>` — builds, signs, and submits a transfer transaction.
async fn cmd_send(
    to:            String,
    amount:        u64,
    keystore_flag: Option<PathBuf>,
    node_url:      String,
) -> Result<(), ClientError> {
    let path = keystore_flag.unwrap_or_else(|| ClientConfig::default().keystore_path);

    let passphrase = rpassword::prompt_password("Passphrase: ")
        .map_err(ClientError::KeystoreIo)?;

    let signing_key = KeystoreFile::load_and_decrypt(&path, &passphrase)?;
    let sender      = Address::from_pubkey_bytes(signing_key.verifying_key().as_bytes());
    let recipient   = Address::validate(&to)?;

    let rpc   = RpcClient::new(node_url);
    let utxos = rpc.get_utxos(sender.as_str()).await?;

    let selection = wallet::select_coins(&utxos, amount)?;
    let tx        = wallet::build_transfer(
        &signing_key,
        selection.selected,
        &recipient,
        amount,
        &sender,
    )?;

    let resp = rpc.post_tx(&tx).await?;
    println!("Transaction submitted.");
    println!("Hash: {}", resp.tx_hash);
    Ok(())
}

/// `chain tip` — displays the current chain tip height and block hash.
async fn cmd_chain_tip(node_url: String) -> Result<(), ClientError> {
    let rpc  = RpcClient::new(node_url);
    let resp = rpc.get_tip().await?;
    println!("Height: {}", resp.height);
    println!("Hash:   {}", resp.hash);
    Ok(())
}

// ── Node config serialization ─────────────────────────────────────────────────

#[derive(Serialize)]
struct NodeConfigToml {
    listen_addr:        String,
    bootstrap_peers:    Vec<String>,
    slot_duration_secs: u64,
    http_addr:          String,
    sled_path:          String,
    mempool_max_size:   usize,
    genesis_path:       String,
    my_address:         String,
    my_signing_key:     String,
}

/// `node init` — generates a node configuration TOML file.
#[allow(clippy::too_many_arguments)]
fn cmd_node_init(
    output:           PathBuf,
    peers:            Vec<String>,
    keystore_flag:    Option<PathBuf>,
    listen_addr:      String,
    http_addr:        String,
    sled_path:        String,
    genesis_path:     String,
    slot_duration:    u64,
    mempool_max_size: usize,
) -> Result<(), ClientError> {
    let (signing_key, address) = match keystore_flag {
        Some(path) => {
            let passphrase = rpassword::prompt_password("Keystore passphrase: ")
                .map_err(ClientError::KeystoreIo)?;
            let sk  = KeystoreFile::load_and_decrypt(&path, &passphrase)?;
            let addr = Address::from_pubkey_bytes(sk.verifying_key().as_bytes());
            (sk, addr)
        }
        None => {
            let mut seed = [0u8; 32];
            rand::rng().fill(&mut seed);
            let sk   = SigningKey::from_bytes(&seed);
            let addr = Address::from_pubkey_bytes(sk.verifying_key().as_bytes());
            println!("Generated new keypair.");
            println!("Address: {addr}");
            println!("Pubkey:  {}", hex::encode(sk.verifying_key().as_bytes()));
            println!("(Add address + pubkey to genesis.toml validators before starting the node.)");
            (sk, addr)
        }
    };

    let cfg = NodeConfigToml {
        listen_addr,
        bootstrap_peers:    peers,
        slot_duration_secs: slot_duration,
        http_addr,
        sled_path,
        mempool_max_size,
        genesis_path,
        my_address:     address.to_string(),
        my_signing_key: hex::encode(signing_key.to_bytes()),
    };

    let toml_str = toml::to_string_pretty(&cfg)
        .map_err(|e| ClientError::Config(e.to_string()))?;

    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(&output, toml_str)?;
    println!("Config written to {}", output.display());
    Ok(())
}
