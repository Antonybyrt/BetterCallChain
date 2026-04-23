mod api_routes;
mod config;
mod event_bus;
mod log_reader;
mod parser;
mod scenarios;
mod ws_handler;

use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use tokio_util::sync::CancellationToken;
use tracing::info;
use tracing_subscriber::EnvFilter;

use api_routes::AppState;
use config::VisualizerConfig;
use event_bus::EventBus;
use log_reader::LogReader;

#[derive(Parser)]
#[command(name = "bcc-visualizer", about = "BetterCallChain event visualizer")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1:9090")]
    bind: SocketAddr,

    #[arg(long, default_value = "bcc-node")]
    container_prefix: String,

    #[arg(long, default_value = "5")]
    node_count: usize,

    /// Comma-separated HTTP ports for the nodes (default: 8081,8082,8083,8084,8085)
    #[arg(long, default_value = "8081,8082,8083,8084,8085")]
    node_ports: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("bcc_visualizer=debug,info")),
        )
        .compact()
        .init();

    let cli = Cli::parse();

    let ports: Vec<u16> = cli.node_ports
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    let cfg = VisualizerConfig {
        bind: cli.bind,
        container_prefix: cli.container_prefix.clone(),
        node_count: cli.node_count,
        node_ports: ports.clone(),
    };

    let cancel = CancellationToken::new();
    let bus = Arc::new(EventBus::new(1000));

    // Start one log reader per node
    for i in 0..cfg.node_count {
        let container = cfg.container_name(i);
        let node_name = cfg.node_name(i);
        LogReader::new(&container, &node_name, Arc::clone(&bus))
            .spawn(cancel.child_token());
    }

    let state = AppState {
        bus: Arc::clone(&bus),
        ports,
    };
    let app = api_routes::router(state);

    info!("bcc-visualizer listening on http://{}", cfg.bind);
    println!("Open http://{} in your browser", cfg.bind);

    let listener = tokio::net::TcpListener::bind(cfg.bind).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            cancel.cancel();
        })
        .await?;

    Ok(())
}
