use std::path::PathBuf;

use bcc_core::types::address::Address;
use clap::{Parser, Subcommand};

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
        /// Node HTTP base URL (default: http://127.0.0.1:8080).
        #[arg(long, value_name = "URL")]
        node: Option<String>,
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
        /// Node HTTP base URL.
        #[arg(long, value_name = "URL")]
        node: Option<String>,
    },
    /// Chain information commands.
    Chain {
        #[command(subcommand)]
        sub: ChainCommands,
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
    Tip {
        /// Node HTTP base URL.
        #[arg(long, value_name = "URL")]
        node: Option<String>,
    },
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Dispatches the parsed CLI command to the appropriate handler.
pub async fn run(cli: Cli) -> Result<(), ClientError> {
    match cli.command {
        Commands::Wallet { sub } => match sub {
            WalletCommands::New { keystore }  => cmd_wallet_new(keystore).await,
            WalletCommands::Show { keystore } => cmd_wallet_show(keystore),
        },
        Commands::Balance { address, node } => cmd_balance(address, node).await,
        Commands::Send { to, amount, keystore, node } => {
            cmd_send(to, amount, keystore, node).await
        }
        Commands::Chain { sub } => match sub {
            ChainCommands::Tip { node } => cmd_chain_tip(node).await,
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
async fn cmd_balance(address: String, node_flag: Option<String>) -> Result<(), ClientError> {
    Address::validate(&address)?;
    let node_url = node_flag.unwrap_or_else(|| ClientConfig::default().node_url);
    let rpc      = RpcClient::new(node_url);
    let resp     = rpc.get_balance(&address).await?;
    println!("Address: {}", resp.address);
    println!("Balance: {}", resp.balance);
    Ok(())
}

/// `send <to> <amount>` — builds, signs, and submits a transfer transaction.
async fn cmd_send(
    to:           String,
    amount:       u64,
    keystore_flag: Option<PathBuf>,
    node_flag:    Option<String>,
) -> Result<(), ClientError> {
    let path = keystore_flag.unwrap_or_else(|| ClientConfig::default().keystore_path);
    let node_url = node_flag.unwrap_or_else(|| ClientConfig::default().node_url);

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
async fn cmd_chain_tip(node_flag: Option<String>) -> Result<(), ClientError> {
    let node_url = node_flag.unwrap_or_else(|| ClientConfig::default().node_url);
    let rpc  = RpcClient::new(node_url);
    let resp = rpc.get_tip().await?;
    println!("Height: {}", resp.height);
    println!("Hash:   {}", resp.hash);
    Ok(())
}
