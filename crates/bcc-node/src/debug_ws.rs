use std::net::SocketAddr;

use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use crate::debug_event::{DebugEnvelope, DebugEvent};
use crate::state::NodeState;

/// Starts a WebSocket server on `addr` that streams [`DebugEnvelope`] JSON to
/// every connected visualizer client.
///
/// On each new connection a [`DebugEvent::NodeSnapshot`] is sent first so the
/// client can initialise its state even if it connected after events were fired.
pub async fn run_debug_ws(
    addr:   SocketAddr,
    state:  NodeState,
    tx:     broadcast::Sender<DebugEnvelope>,
    cancel: CancellationToken,
) {
    let listener = match TcpListener::bind(addr).await {
        Ok(l)  => l,
        Err(e) => { error!(%addr, err=%e, "debug WS: failed to bind"); return; }
    };

    info!(%addr, "debug WebSocket listening");

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            result = listener.accept() => match result {
                Ok((stream, _peer)) => {
                    let snapshot = build_snapshot(&state).await;
                    let rx = tx.subscribe();
                    tokio::spawn(async move { handle(stream, rx, snapshot).await; });
                }
                Err(e) => debug!(err=%e, "debug WS: accept error"),
            }
        }
    }

    info!("debug WebSocket stopped");
}

/// Queries the current node state and returns it as a [`DebugEvent::NodeSnapshot`].
async fn build_snapshot(state: &NodeState) -> DebugEnvelope {
    let (tip_height, tip_hash) = state
        .blocks
        .tip()
        .ok()
        .flatten()
        .map(|(h, hash)| (h, hex::encode(hash)))
        .unwrap_or((0, "0000000000000000000000000000000000000000000000000000000000000000".to_string()));

    let peers = state
        .peers
        .read()
        .await
        .addrs()
        .into_iter()
        .map(|a| a.to_string())
        .collect();

    let mempool_size = state.mempool.lock().await.len();

    DebugEnvelope::now(DebugEvent::NodeSnapshot {
        tip_height,
        tip_hash,
        peers,
        mempool_size,
    })
}

async fn handle(
    stream:   tokio::net::TcpStream,
    mut rx:   broadcast::Receiver<DebugEnvelope>,
    snapshot: DebugEnvelope,
) {
    let ws = match accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => { debug!(err=%e, "debug WS: handshake failed"); return; }
    };

    debug!("debug WS: client connected");
    let (mut sender, mut receiver) = ws.split();

    // Seed the client with the current node state before the event stream.
    if let Ok(json) = serde_json::to_string(&snapshot) {
        let _ = sender.send(Message::Text(json.into())).await;
    }

    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Ok(env) => {
                    let Ok(json) = serde_json::to_string(&env) else { continue };
                    if sender.send(Message::Text(json.into())).await.is_err() { break; }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    let notice = format!(r#"{{"kind":"Lagged","dropped":{n}}}"#);
                    let _ = sender.send(Message::Text(notice.into())).await;
                }
                Err(_) => break,
            },
            msg = receiver.next() => { if msg.is_none() { break; } }
        }
    }

    debug!("debug WS: client disconnected");
}
