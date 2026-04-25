use serde::{Deserialize, Serialize};

/// All structured events a node can emit to connected debug clients.
///
/// Every variant maps to one observable action in the node.  The enum is
/// `#[serde(tag = "kind")]` so JSON consumers can dispatch on the `"kind"` field
/// without any extra wrapper object.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum DebugEvent {
    // ── Lifecycle ────────────────────────────────────────────────────
    NodeStarting   { node: String, http_addr: String, p2p_addr: String },
    HttpApiReady   { http_addr: String },
    NodeStopping,

    // ── IBD ──────────────────────────────────────────────────────────
    IbdStarting  { from_height: u64 },
    IbdBatch     { height: u64 },
    IbdComplete  { synced_to: u64 },

    // ── Slot ticker ───────────────────────────────────────────────────
    SlotTick        { slot: u64, tip_height: u64 },
    SlotNotProposer { slot: u64, elected_proposer: String },
    MempoolDrain    { slot: u64, mempool_size: usize },

    // ── Block production ──────────────────────────────────────────────
    BlockProposed {
        height:   u64,
        hash:     String,
        slot:     u64,
        txs:      usize,
        proposer: String,
    },

    // ── P2P – peers ───────────────────────────────────────────────────
    PeerConnected    { addr: String, peer_count: usize },
    PeerDisconnected { addr: String, peer_count: usize },

    // ── P2P – blocks ──────────────────────────────────────────────────
    BlockFromPeer {
        from:     String,
        height:   u64,
        hash:     String,
        txs:      usize,
        proposer: String,
    },
    BlockRejected {
        from:   String,
        height: u64,
        hash:   String,
        reason: String,
    },
    BlockIgnored {
        from:         String,
        block_height: u64,
        local_tip:    u64,
        hash:         String,
    },
    BlockReorged {
        height:  u64,
        new_tip: String,
        evicted: String,
    },

    // ── P2P – transactions ────────────────────────────────────────────
    TxGossipAccepted { from: String, tx_hash: String },
    TxGossipRejected { from: String, tx_hash: String, reason: String },

    // ── Mempool ───────────────────────────────────────────────────────
    TxAccepted { tx_hash: String, value: u64, pool_size: usize },
    TxRejected { tx_hash: String, reason: String },
    TxEvicted  { evicted: String, new_tx: String },

    // ── HTTP API ──────────────────────────────────────────────────────
    ApiTxAccepted { tx_hash: String },
    ApiTxRejected { tx_hash: String, reason: String },
    ApiGetTip     { height: u64, hash: String },

    // ── Visualizer scenarios (published by bcc-visualizer, not the node) ─────
    ScenarioEvent { scenario: String, step: String, status: String, detail: String },
}

/// Timestamped wrapper around [`DebugEvent`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugEnvelope {
    /// RFC-3339 UTC timestamp at the moment the event was emitted.
    pub ts:    String,
    pub event: DebugEvent,
}

impl DebugEnvelope {
    pub fn now(event: DebugEvent) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        // Format as ISO-8601 string compatible with JS Date
        let ts = format_ts(secs);
        Self { ts, event }
    }
}

fn format_ts(millis: u128) -> String {
    let secs  = millis / 1000;
    let ms    = millis % 1000;
    let mins  = secs / 60;
    let s     = secs % 60;
    let hours = mins / 60;
    let m     = mins % 60;
    let days  = hours / 24;
    let h     = hours % 24;
    // Epoch-relative date (crude but dependency-free)
    let (year, month, day) = epoch_to_ymd(days as u32);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}.{ms:03}Z")
}

fn epoch_to_ymd(mut days: u32) -> (u32, u32, u32) {
    // Tomohiko Sakamoto algorithm (simplified)
    let mut y = 1970u32;
    loop {
        let leap = if y % 400 == 0 { 1 } else if y % 100 == 0 { 0 } else if y % 4 == 0 { 1 } else { 0 };
        let year_days = 365 + leap;
        if days < year_days { break; }
        days -= year_days;
        y += 1;
    }
    let leap = if y % 400 == 0 { 1u32 } else if y % 100 == 0 { 0 } else if y % 4 == 0 { 1 } else { 0 };
    let month_days = [31,28+leap,31,30,31,30,31,31,30,31,30,31];
    let mut m = 0u32;
    for &md in &month_days {
        if days < md { break; }
        days -= md;
        m += 1;
    }
    (y, m + 1, days + 1)
}
