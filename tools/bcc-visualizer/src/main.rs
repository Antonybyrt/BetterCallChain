mod event_bus;
mod node_client;
mod scenarios;
mod server;

use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

use event_bus::EventBus;
use node_client::NodeClient;

#[derive(Parser)]
#[command(name = "bcc-visualizer", about = "BetterCallChain event visualizer")]
struct Cli {
    /// HTTP address the visualizer UI listens on.
    #[arg(long, default_value = "127.0.0.1:9090")]
    bind: SocketAddr,

    /// Explicit list of debug WebSocket URLs, one per node (comma-separated).
    /// When provided, --node-ports is ignored.
    /// Example: ws://172.30.0.2:9080/debug,ws://172.30.0.3:9080/debug,...
    #[arg(long)]
    debug_urls: Option<String>,

    /// Comma-separated HTTP ports of the nodes (debug WS = HTTP port + 1000).
    /// Used when --debug-urls is not set (local mode).
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

    // Build (node_name, debug_ws_url) pairs from either --debug-urls or --node-ports
    let node_urls: Vec<(String, String)> = if let Some(raw) = cli.debug_urls {
        raw.split(',')
            .enumerate()
            .map(|(i, url)| (format!("node{}", i + 1), url.trim().to_string()))
            .collect()
    } else {
        cli.node_ports
            .split(',')
            .enumerate()
            .filter_map(|(i, s)| s.trim().parse::<u16>().ok().map(|p| {
                (format!("node{}", i + 1), format!("ws://127.0.0.1:{}/debug", p + 1000))
            }))
            .collect()
    };

    // HTTP ports for scenario execution (extract from node_ports, or derive from debug URLs)
    let scenario_ports: Vec<u16> = cli.node_ports
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    let cancel = CancellationToken::new();
    let bus    = Arc::new(EventBus::new(1000));

    for (node_name, debug_url) in &node_urls {
        NodeClient::new(node_name, debug_url, Arc::clone(&bus))
            .spawn(cancel.child_token());
    }

    server::run_server(cli.bind, Arc::clone(&bus), scenario_ports, cancel.child_token()).await;

    Ok(())
}
