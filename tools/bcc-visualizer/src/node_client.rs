use std::sync::Arc;
use std::time::Duration;

use bcc_node::debug_event::DebugEnvelope;
use chrono::DateTime;
use futures::StreamExt;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::event_bus::EventBus;

/// Connects to a node's debug WebSocket and relays typed [`DebugEvent`]s
/// directly into the visualizer's [`EventBus`] — no text parsing needed.
pub struct NodeClient {
    node_name: String,
    debug_url: String,
    bus:       Arc<EventBus>,
}

impl NodeClient {
    pub fn new(node_name: &str, debug_url: &str, bus: Arc<EventBus>) -> Self {
        Self {
            node_name: node_name.to_string(),
            debug_url: debug_url.to_string(),
            bus,
        }
    }

    pub fn spawn(self, cancel: CancellationToken) {
        let client = Arc::new(self);
        tokio::spawn(async move {
            let mut backoff = Duration::from_secs(2);
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return,
                    _ = client.run_once() => {}
                }
                warn!(node = %client.node_name, "debug WS stream ended, retrying in {:?}", backoff);
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        });
    }

    async fn run_once(&self) {
        let (ws, _) = match connect_async(&self.debug_url).await {
            Ok(p)  => p,
            Err(e) => { warn!(node = %self.node_name, err = %e, "debug WS: connect failed"); return; }
        };

        info!(node = %self.node_name, url = %self.debug_url, "debug WS: connected");

        let (_, mut read) = ws.split();
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    match serde_json::from_str::<DebugEnvelope>(&text) {
                        Ok(env) => {
                            let ts = env.ts.parse::<DateTime<chrono::Utc>>()
                                .unwrap_or_else(|_| chrono::Utc::now());
                            self.bus.publish(self.node_name.clone(), ts, env.event);
                        }
                        Err(e) => warn!(node = %self.node_name, err = %e, "debug WS: deserialize error"),
                    }
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    }
}
