use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::state::NodeState;

use super::handler::run_peer;

/// Listens for incoming P2P connections and spawns a task for each peer.
///
/// Exits cleanly when `cancel` is cancelled.
pub async fn run_server(state: NodeState, cancel: CancellationToken) {
    let addr = state.config.listen_addr;
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            error!(err = %e, %addr, "P2P server: failed to bind");
            return;
        }
    };
    info!(%addr, "P2P server listening");

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            result = listener.accept() => {
                match result {
                    Ok((stream, peer_addr)) => {
                        let child = cancel.child_token();
                        let peer_state = state.clone();
                        tokio::spawn(run_peer(stream, peer_addr, peer_state, child));
                    }
                    Err(e) => error!(err = %e, "P2P server: accept error"),
                }
            }
        }
    }

    info!("P2P server stopped");
}
