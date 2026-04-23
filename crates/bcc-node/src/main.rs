use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use tokio_util::sync::CancellationToken;
use tracing::info;
use tracing_subscriber::{fmt::time::UtcTime, EnvFilter};

use bcc_core::store::{BlockStore, UtxoStore, ValidatorStore};
use bcc_node::config::NodeConfig;
use bcc_node::genesis::GenesisConfig;
use bcc_node::state::NodeState;
use bcc_node::storage::sled_store::SledStore;
use bcc_node::{api, genesis, ibd, p2p, slot_ticker};

#[derive(Parser)]
#[command(name = "bcc-node", about = "BetterCallChain full node")]
struct Cli {
    /// Path to the node configuration TOML file.
    #[arg(short, long)]
    config: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Parse CLI args + load config.
    let cli = Cli::parse();
    let config = Arc::new(
        NodeConfig::from_file(&cli.config)
            .context("failed to load node config")?,
    );

    // 2. Initialise tracing.
    //
    // Default filter: INFO for bcc_node, WARN for everything else.
    // Override with RUST_LOG, e.g.: RUST_LOG=bcc_node=debug,bcc_core=debug
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("bcc_node=debug,bcc_core=info,warn"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        // RFC 3339 timestamps with millisecond precision, e.g. 2026-04-23T09:15:00.123Z
        .with_timer(UtcTime::rfc_3339())
        // Include the thread name so concurrent tasks are distinguishable.
        .with_thread_names(true)
        // Include the source file and line number for every log entry.
        .with_file(true)
        .with_line_number(true)
        // Include log LEVEL.
        .with_level(true)
        // Compact single-line output — easier to grep in `docker logs`.
        .compact()
        .init();

    info!(
        node      = %config.my_address,
        http_addr = %config.http_addr,
        p2p_addr  = %config.listen_addr,
        db        = %config.sled_path.display(),
        "BetterCallChain node starting"
    );

    // 3. Open persistent sled store.
    let store = Arc::new(
        SledStore::open(&config.sled_path).context("failed to open sled database")?,
    );

    // 4. Apply genesis state (idempotent — skipped if height-0 block already exists).
    let genesis_cfg = GenesisConfig::from_file(&config.genesis_path)
        .context("failed to load genesis config")?;
    genesis::apply_genesis(&genesis_cfg, &*store, &*store, &*store)
        .context("failed to apply genesis")?;

    // 5. Build shared node state.
    let blocks:     Arc<dyn BlockStore>     = store.clone();
    let utxo:       Arc<dyn UtxoStore>      = store.clone();
    let validators: Arc<dyn ValidatorStore> = store.clone();
    let state = NodeState::new(blocks, utxo, validators, config.clone());

    // 6. Root cancellation token — shared by all tasks.
    let cancel = CancellationToken::new();

    // 7. Initial block download — synchronise with the network before producing blocks.
    ibd::run_ibd(&state, &cancel).await.context("IBD failed")?;

    // 8. Spawn long-running tasks.
    let p2p_handle = tokio::spawn(p2p::server::run_server(
        state.clone(),
        cancel.child_token(),
    ));

    tokio::spawn(p2p::connector::run_connector(
        state.clone(),
        cancel.child_token(),
    ));

    let ticker_handle = tokio::spawn(slot_ticker::run_slot_ticker(
        state.clone(),
        cancel.child_token(),
    ));

    let api_handle = {
        let http_addr  = config.http_addr;
        let api_cancel = cancel.child_token();
        let api_router = api::router(state.clone());
        let resp = tokio::spawn(async move {
            let listener = tokio::net::TcpListener::bind(http_addr)
                .await
                .expect("failed to bind HTTP listener");
            info!(%http_addr, "HTTP API listening");
            axum::serve(listener, api_router)
                .with_graceful_shutdown(async move { api_cancel.cancelled().await })
                .await
                .ok();
        });
        resp
    };

    // 9. Wait for a shutdown signal (Ctrl-C / SIGTERM).
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate())
            .context("failed to install SIGTERM handler")?;
        tokio::select! {
            _ = tokio::signal::ctrl_c() => info!("received Ctrl-C"),
            _ = sigterm.recv()          => info!("received SIGTERM"),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.ok();
        info!("received Ctrl-C");
    }

    // 10. Signal all tasks to stop.
    cancel.cancel();

    // 11. Join tasks with a hard 5-second deadline.
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        let _ = p2p_handle.await;
        let _ = ticker_handle.await;
        let _ = api_handle.await;
    })
    .await;

    info!("BetterCallChain node stopped");
    Ok(())
}
