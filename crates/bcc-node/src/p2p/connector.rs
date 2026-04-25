use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use crate::state::NodeState;

use super::handler::run_peer;

const DIAL_INTERVAL_SECS: u64 = 5;
/// Initial backoff on first connection failure (seconds).
const BASE_BACKOFF_SECS: u64 = 2;
/// Maximum backoff per peer (seconds).
const MAX_BACKOFF_SECS: u64 = 64;

/// Per-peer backoff state: (next allowed dial attempt, current backoff duration).
type BackoffMap = Arc<Mutex<HashMap<SocketAddr, (Instant, u64)>>>;

/// Maintains persistent outbound connections to all bootstrap peers.
///
/// On connection failure the peer enters an exponential backoff (2 s → 4 s → … → 64 s).
/// On successful reconnect the backoff resets to the base value.
pub async fn run_connector(state: NodeState, cancel: CancellationToken) {
    let peers = state.config.bootstrap_peers.clone();
    if peers.is_empty() {
        debug!("connector: no bootstrap peers configured, skipping");
        return;
    }

    let backoff: BackoffMap = Arc::new(Mutex::new(HashMap::new()));

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

        // Snapshot backoff state so we don't hold the lock across spawns.
        let backoff_snapshot: HashMap<SocketAddr, (Instant, u64)> =
            backoff.lock().await.clone();
        let now = Instant::now();

        for &peer_addr in &peers {
            if connected.contains(&peer_addr) {
                continue;
            }

            // Skip if still within the backoff window.
            if let Some(&(next_attempt, _)) = backoff_snapshot.get(&peer_addr) {
                if now < next_attempt {
                    continue;
                }
            }

            let state_clone = state.clone();
            let child      = cancel.child_token();
            let backoff_cl = backoff.clone();

            tokio::spawn(async move {
                match TcpStream::connect(peer_addr).await {
                    Ok(stream) => {
                        // Reset backoff on successful connect.
                        backoff_cl.lock().await.remove(&peer_addr);
                        info!(%peer_addr, "connector: outbound connection established");
                        run_peer(stream, peer_addr, state_clone, child).await;
                    }
                    Err(e) => {
                        // Double the backoff (capped), schedule next attempt.
                        let mut map = backoff_cl.lock().await;
                        let cur = map.get(&peer_addr).map(|&(_, b)| b).unwrap_or(BASE_BACKOFF_SECS);
                        let nxt = (cur * 2).min(MAX_BACKOFF_SECS);
                        map.insert(peer_addr, (Instant::now() + Duration::from_secs(cur), nxt));
                        debug!(%peer_addr, err = %e, backoff_secs = cur,
                               "connector: failed to dial peer — backing off");
                    }
                }
            });
        }
    }
}
