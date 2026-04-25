use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use bcc_node::debug_event::{DebugEvent};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// Envelope sent to WebSocket clients: the node's event + routing metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsEnvelope {
    pub seq:   u64,
    pub node:  String,
    pub ts:    DateTime<Utc>,
    pub event: DebugEvent,
}

pub struct EventBus {
    tx:       broadcast::Sender<WsEnvelope>,
    recent:   Arc<Mutex<VecDeque<WsEnvelope>>>,
    seq:      Arc<AtomicU64>,
    capacity: usize,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity * 2);
        Self {
            tx,
            recent:   Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            seq:      Arc::new(AtomicU64::new(0)),
            capacity,
        }
    }

    /// Publish an event from `node` with an explicit timestamp (from the node's envelope).
    pub fn publish(&self, node: String, ts: DateTime<Utc>, event: DebugEvent) {
        let seq      = self.seq.fetch_add(1, Ordering::Relaxed);
        let envelope = WsEnvelope { seq, node, ts, event };
        {
            let mut buf = self.recent.lock().unwrap();
            if buf.len() >= self.capacity { buf.pop_front(); }
            buf.push_back(envelope.clone());
        }
        let _ = self.tx.send(envelope);
    }

    /// Publish a visualizer-local event (e.g. ScenarioEvent) with the current time.
    pub fn publish_local(&self, node: String, event: DebugEvent) {
        self.publish(node, Utc::now(), event);
    }

    pub fn recent_sync(&self) -> Vec<WsEnvelope> {
        self.recent.lock().unwrap().iter().cloned().collect()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WsEnvelope> {
        self.tx.subscribe()
    }
}
