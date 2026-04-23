use std::collections::HashMap;

use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

// Strips ANSI escape sequences (ESC [ ... m) from a string
static ANSI_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\x1b\[[0-9;]*[A-Za-z]").unwrap());

fn strip_ansi(s: &str) -> std::borrow::Cow<str> {
    ANSI_RE.replace_all(s, "")
}

// Actual format from `docker logs --follow`:
//   2026-04-23T11:11:40.682966189Z  INFO main bcc_node: crates/.../main.rs:56: message key=val
//   2026-04-23T11:11:40.960467119Z DEBUG tokio-rt-worker bcc_node::slot_ticker: .../slot_ticker.rs:99: slot tick slot=355388540
// Fields: timestamp, level, thread_name (single token for bcc_node), target, file:linenum:, rest
static LINE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"^(?P<ts>\d{4}-\d{2}-\d{2}T[\d:.]+Z)\s+(?P<level>TRACE|DEBUG|INFO|WARN|ERROR)\s+\S+\s+(?P<target>bcc_node[\w:]*):\s+[^\s:]+:\d+:\s+(?P<rest>.+)$",
    )
    .unwrap()
});

static FIELD_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(\w+)=(?:"([^"]*)"|(\S+))"#).unwrap());

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum NodeEvent {
    NodeStarting {
        node: String,
        http_addr: String,
        p2p_addr: String,
    },
    P2pListening {
        addr: String,
    },
    PeerConnected {
        addr: String,
        peer_count: Option<u64>,
    },
    PeerDisconnected {
        addr: String,
        peer_count: Option<u64>,
    },
    BlockFromPeer {
        from: String,
        height: u64,
        hash: String,
        txs: u64,
        proposer: String,
    },
    BlockIgnored {
        from: String,
        block_height: u64,
        local_tip: u64,
        hash: String,
    },
    BlockProposed {
        height: u64,
        hash: String,
        slot: u64,
        txs: u64,
        proposer: String,
    },
    TxGossipAccepted {
        from: String,
        tx_hash: String,
    },
    TxGossipRejected {
        from: String,
        tx_hash: String,
        reason: String,
    },
    SlotTick {
        slot: u64,
    },
    SlotNotProposer {
        slot: u64,
        proposer: String,
    },
    MempoolDrain {
        slot: u64,
        mempool_size: u64,
    },
    TxAccepted {
        tx_hash: String,
        value: u64,
        pool_size: u64,
    },
    TxRejected {
        tx_hash: String,
        reason: String,
    },
    TxEvicted {
        evicted: String,
        new_tx: String,
    },
    ApiTxAccepted {
        tx_hash: String,
    },
    ApiTxRejected {
        tx_hash: String,
        reason: String,
    },
    ApiGetTip {
        height: u64,
        hash: String,
    },
    IbdStarting {
        from_height: u64,
    },
    IbdBatch {
        height: u64,
    },
    IbdComplete {
        synced_to: u64,
    },
    ScenarioEvent {
        scenario: String,
        step: String,
        status: String,
        detail: String,
    },
    Raw {
        level: String,
        target: String,
        message: String,
    },
}

pub struct ParsedLine {
    pub timestamp: DateTime<Utc>,
    pub level: String,
    pub target: String,
    pub event: NodeEvent,
}

pub fn parse_line(raw: &str) -> Option<ParsedLine> {
    // Fast exit: ~90% of lines are sled internals that don't contain "bcc_node"
    if !raw.contains("bcc_node") {
        return None;
    }
    let clean = strip_ansi(raw.trim());
    let caps = LINE_RE.captures(&clean)?;
    let ts_str = &caps["ts"];
    let level = caps["level"].to_string();
    let target = caps["target"].to_string();
    let rest = caps["rest"].to_string();

    let timestamp = ts_str.parse::<DateTime<Utc>>().ok()?;
    let fields = extract_fields(&rest);
    let message = extract_message(&rest);

    let event = build_event(&message, &level, &target, &fields);

    Some(ParsedLine {
        timestamp,
        level,
        target,
        event,
    })
}

fn extract_fields(rest: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for cap in FIELD_RE.captures_iter(rest) {
        let key = cap[1].to_string();
        let val = cap.get(2).or_else(|| cap.get(3)).map(|m| m.as_str()).unwrap_or("").to_string();
        map.insert(key, val);
    }
    map
}

fn extract_message(rest: &str) -> String {
    if let Some(m) = FIELD_RE.find(rest) {
        rest[..m.start()].trim().to_string()
    } else {
        rest.trim().to_string()
    }
}

fn get(fields: &HashMap<String, String>, key: &str) -> String {
    fields.get(key).cloned().unwrap_or_default()
}

fn get_u64(fields: &HashMap<String, String>, key: &str) -> u64 {
    fields.get(key).and_then(|v| v.parse().ok()).unwrap_or(0)
}

fn get_opt_u64(fields: &HashMap<String, String>, key: &str) -> Option<u64> {
    fields.get(key).and_then(|v| v.parse().ok())
}

fn build_event(
    message: &str,
    level: &str,
    target: &str,
    fields: &HashMap<String, String>,
) -> NodeEvent {
    match message {
        "BetterCallChain node starting" => NodeEvent::NodeStarting {
            node: get(fields, "node"),
            http_addr: get(fields, "http_addr"),
            p2p_addr: get(fields, "p2p_addr"),
        },
        "P2P server listening" => NodeEvent::P2pListening {
            addr: get(fields, "addr"),
        },
        "peer connected" => NodeEvent::PeerConnected {
            addr: get(fields, "addr"),
            peer_count: get_opt_u64(fields, "peer_count"),
        },
        "peer disconnected" => NodeEvent::PeerDisconnected {
            addr: get(fields, "addr"),
            peer_count: get_opt_u64(fields, "peer_count"),
        },
        "p2p: block accepted from peer" => NodeEvent::BlockFromPeer {
            from: get(fields, "from"),
            height: get_u64(fields, "height"),
            hash: get(fields, "hash"),
            txs: get_u64(fields, "txs"),
            proposer: get(fields, "proposer"),
        },
        "p2p: block ignored (height mismatch)" => NodeEvent::BlockIgnored {
            from: get(fields, "from"),
            block_height: get_u64(fields, "block_height"),
            local_tip: get_u64(fields, "local_tip"),
            hash: get(fields, "hash"),
        },
        "proposed block" => NodeEvent::BlockProposed {
            height: get_u64(fields, "height"),
            hash: get(fields, "hash"),
            slot: get_u64(fields, "slot"),
            txs: get_u64(fields, "txs"),
            proposer: get(fields, "proposer"),
        },
        "p2p: tx gossip accepted, re-broadcasting" => NodeEvent::TxGossipAccepted {
            from: get(fields, "from"),
            tx_hash: get(fields, "tx_hash"),
        },
        "p2p: tx gossip rejected" => NodeEvent::TxGossipRejected {
            from: get(fields, "from"),
            tx_hash: get(fields, "tx_hash"),
            reason: get(fields, "reason"),
        },
        "slot tick" => NodeEvent::SlotTick {
            slot: get_u64(fields, "slot"),
        },
        "slot: not proposer, skipping" => NodeEvent::SlotNotProposer {
            slot: get_u64(fields, "slot"),
            proposer: get(fields, "proposer"),
        },
        "slot: draining mempool for block" => NodeEvent::MempoolDrain {
            slot: get_u64(fields, "slot"),
            mempool_size: get_u64(fields, "mempool_size"),
        },
        "mempool: tx accepted" => NodeEvent::TxAccepted {
            tx_hash: get(fields, "tx_hash"),
            value: get_u64(fields, "value"),
            pool_size: get_u64(fields, "pool_size"),
        },
        m if m.starts_with("mempool: tx rejected") => NodeEvent::TxRejected {
            tx_hash: get(fields, "tx_hash"),
            reason: get(fields, "reason"),
        },
        "mempool: evicting low-value tx to make room" => NodeEvent::TxEvicted {
            evicted: get(fields, "evicted"),
            new_tx: get(fields, "new_tx"),
        },
        "api: POST /tx accepted" => NodeEvent::ApiTxAccepted {
            tx_hash: get(fields, "tx_hash"),
        },
        "api: POST /tx rejected" => NodeEvent::ApiTxRejected {
            tx_hash: get(fields, "tx_hash"),
            reason: get(fields, "reason"),
        },
        "api: GET /chain/tip" => NodeEvent::ApiGetTip {
            height: get_u64(fields, "height"),
            hash: get(fields, "hash"),
        },
        "IBD starting" => NodeEvent::IbdStarting {
            from_height: get_u64(fields, "from_height"),
        },
        "IBD: batch applied" => NodeEvent::IbdBatch {
            height: get_u64(fields, "height"),
        },
        "IBD complete" => NodeEvent::IbdComplete {
            synced_to: get_u64(fields, "synced_to"),
        },
        _ => NodeEvent::Raw {
            level: level.to_string(),
            target: target.to_string(),
            message: message.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_block_proposed() {
        let line = r#"2026-04-23T11:11:40.976586427Z  INFO tokio-rt-worker bcc_node::slot_ticker: crates/bcc-node/src/slot_ticker.rs:175: proposed block height=5 hash=abc123 slot=12 txs=3 proposer=bcs1abc"#;
        let parsed = parse_line(line).unwrap();
        assert_eq!(parsed.level, "INFO");
        match parsed.event {
            NodeEvent::BlockProposed { height, hash, slot, txs, proposer } => {
                assert_eq!(height, 5);
                assert_eq!(hash, "abc123");
                assert_eq!(slot, 12);
                assert_eq!(txs, 3);
                assert_eq!(proposer, "bcs1abc");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parse_tx_accepted() {
        let line = r#"2026-04-23T11:11:40.123456789Z  INFO tokio-rt-worker bcc_node::mempool: crates/bcc-node/src/mempool.rs:122: mempool: tx accepted tx_hash=deadbeef value=100000 pool_size=5"#;
        let parsed = parse_line(line).unwrap();
        match parsed.event {
            NodeEvent::TxAccepted { tx_hash, value, pool_size } => {
                assert_eq!(tx_hash, "deadbeef");
                assert_eq!(value, 100000);
                assert_eq!(pool_size, 5);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parse_peer_connected_with_count() {
        let line = r#"2026-04-23T11:11:40.975931033Z  INFO tokio-rt-worker bcc_node::p2p::handler: crates/bcc-node/src/p2p/handler.rs:41: peer connected addr=172.30.0.3:12345 peer_count=2"#;
        let parsed = parse_line(line).unwrap();
        match parsed.event {
            NodeEvent::PeerConnected { addr, peer_count } => {
                assert_eq!(addr, "172.30.0.3:12345");
                assert_eq!(peer_count, Some(2));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parse_slot_not_proposer() {
        let line = r#"2026-04-23T11:11:40.960492842Z DEBUG tokio-rt-worker bcc_node::slot_ticker: crates/bcc-node/src/slot_ticker.rs:107: slot: not proposer, skipping slot=355388540 proposer=bcs10875af8741f3ad3034add0fb19dab5aeaaaf2d9f"#;
        let parsed = parse_line(line).unwrap();
        match parsed.event {
            NodeEvent::SlotNotProposer { slot, proposer } => {
                assert_eq!(slot, 355388540);
                assert!(!proposer.is_empty());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn invalid_line_returns_none() {
        assert!(parse_line("not a valid log line").is_none());
        assert!(parse_line("").is_none());
        // sled internal logs should be ignored
        assert!(parse_line("2026-04-23T11:11:40.703447647Z DEBUG main sled::pagecache::snapshot: /usr/local/cargo/...:461: no previous snapshot found").is_none());
    }
}
