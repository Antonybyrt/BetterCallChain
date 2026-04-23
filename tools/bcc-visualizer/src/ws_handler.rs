use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::broadcast;
use tracing::debug;

use crate::event_bus::EventBus;

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(bus): State<Arc<EventBus>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, bus))
}

pub async fn handle_socket_pub(socket: WebSocket, bus: Arc<EventBus>) {
    handle_socket(socket, bus).await;
}

async fn handle_socket(socket: WebSocket, bus: Arc<EventBus>) {
    let (mut sender, mut receiver) = socket.split();
    let mut sub = bus.subscribe();

    // Replay buffered events to the new client
    let recent = bus.recent_sync();
    let replay_count = recent.len();
    for env in recent {
        let Ok(json) = serde_json::to_string(&env) else { continue };
        if sender.send(Message::Text(json.into())).await.is_err() {
            return;
        }
    }

    let ctrl = json!({"ctrl": "connected", "replay_count": replay_count}).to_string();
    if sender.send(Message::Text(ctrl.into())).await.is_err() {
        return;
    }

    loop {
        tokio::select! {
            msg = sub.recv() => match msg {
                Ok(env) => {
                    let Ok(json) = serde_json::to_string(&env) else { continue };
                    if sender.send(Message::Text(json.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    let notice = json!({"ctrl": "lagged", "dropped": n}).to_string();
                    let _ = sender.send(Message::Text(notice.into())).await;
                }
                Err(_) => break,
            },
            msg = receiver.next() => match msg {
                Some(Ok(Message::Text(_text))) => {
                    // Client commands (replay, filter) — ignored for now
                    debug!("ws client message received");
                }
                None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }
}
