use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use crate::state::NodeState;

use super::handler::run_peer;

/// Hard cap on simultaneous inbound P2P connections.
const MAX_INBOUND_PEERS: usize = 50;

/// Listens for incoming P2P connections and spawns a task for each peer.
///
/// Rejects new connections once `MAX_INBOUND_PEERS` is reached.
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
    info!(%addr, max_peers = MAX_INBOUND_PEERS, "P2P server listening");

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            result = listener.accept() => {
                match result {
                    Ok((stream, peer_addr)) => {
                        let current = state.peers.read().await.len();
                        if current >= MAX_INBOUND_PEERS {
                            debug!(%peer_addr, current, "P2P server: connection limit reached — rejecting");
                            // drop(stream) closes the TCP connection immediately
                            drop(stream);
                            continue;
                        }
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
