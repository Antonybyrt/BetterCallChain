use std::net::SocketAddr;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use futures::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use crate::debug_event::DebugEnvelope;

/// Starts a WebSocket server on `addr` that streams [`DebugEnvelope`] JSON to
/// every connected client.  Intended for the `bcc-visualizer` tool only —
/// not part of the consensus protocol.
pub async fn run_debug_ws(
    addr:   SocketAddr,
    tx:     broadcast::Sender<DebugEnvelope>,
    cancel: CancellationToken,
) {
    let app = Router::new()
        .route("/debug", get(ws_upgrade))
        .with_state(tx);

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l)  => l,
        Err(e) => { tracing::error!(%addr, err=%e, "debug WS: failed to bind"); return; }
    };

    info!(%addr, "debug WebSocket listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move { cancel.cancelled().await })
        .await
        .ok();

    info!("debug WebSocket stopped");
}

async fn ws_upgrade(
    ws:              WebSocketUpgrade,
    State(tx):       State<broadcast::Sender<DebugEnvelope>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle(socket, tx.subscribe()))
}

async fn handle(socket: WebSocket, mut rx: broadcast::Receiver<DebugEnvelope>) {
    let (mut sender, mut receiver) = socket.split();
    debug!("debug WS: client connected");

    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Ok(env) => {
                    let Ok(json) = serde_json::to_string(&env) else { continue };
                    if sender.send(Message::Text(json.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    // Slow client — notify and continue
                    let notice = format!(r#"{{"kind":"Lagged","dropped":{n}}}"#);
                    let _ = sender.send(Message::Text(notice.into())).await;
                }
                Err(_) => break,
            },
            msg = receiver.next() => {
                // Client closed or sent a frame — close gracefully
                if msg.is_none() { break; }
            }
        }
    }

    debug!("debug WS: client disconnected");
}
