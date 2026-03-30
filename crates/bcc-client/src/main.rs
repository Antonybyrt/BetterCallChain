use bcc_client::cli::{self, Cli};
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    // Initialise structured logging. Respects RUST_LOG; defaults to warnings only
    // so the interactive CLI output is not polluted with trace/info noise.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("bcc_client=warn")),
        )
        .init();

    let cli = Cli::parse();
    if let Err(e) = cli::run(cli).await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
