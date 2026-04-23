use std::time::Duration;

use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use crate::state::NodeState;

use super::handler::run_peer;

const DIAL_INTERVAL_SECS: u64 = 5;

/// Maintains persistent outbound connections to all bootstrap peers.
///
/// Every `DIAL_INTERVAL_SECS` seconds, for each configured bootstrap peer that
/// is not currently in the PeerSet, a TCP connection is attempted and handed
/// off to `run_peer` as a long-running task.  When a peer disconnects,
/// `run_peer` removes it from the PeerSet, so the next connector tick will
/// re-dial automatically.
pub async fn run_connector(state: NodeState, cancel: CancellationToken) {
    let peers = state.config.bootstrap_peers.clone();
    if peers.is_empty() {
        debug!("connector: no bootstrap peers configured, skipping");
        return;
    }

    let mut interval = tokio::time::interval(Duration::from_secs(DIAL_INTERVAL_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return,
            _ = interval.tick() => {}
        }

        let connected: std::collections::HashSet<_> =
            state.peers.read().await.addrs().into_iter().collect();

        for &peer_addr in &peers {
            if connected.contains(&peer_addr) {
                continue;
            }

            let state_clone = state.clone();
            let child = cancel.child_token();

            tokio::spawn(async move {
                match TcpStream::connect(peer_addr).await {
                    Ok(stream) => {
                        info!(%peer_addr, "connector: outbound connection established");
                        run_peer(stream, peer_addr, state_clone, child).await;
                    }
                    Err(e) => {
                        debug!(%peer_addr, err = %e, "connector: failed to dial peer");
                    }
                }
            });
        }
    }
}
