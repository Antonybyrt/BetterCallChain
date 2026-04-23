# bcc-visualizer

Real-time visualizer for the BetterCallChain network. It reads Docker logs from all 5 nodes, parses structured tracing events, and streams them via WebSocket to a multi-tab web UI.

## Quick start

```bash
# Start the cluster first
docker compose up --build -d

# Launch the visualizer (listens on http://127.0.0.1:9090)
cargo run -p bcc-visualizer
```

Open `http://127.0.0.1:9090` in a browser.

### CLI flags

| Flag | Default | Description |
|------|---------|-------------|
| `--bind` | `127.0.0.1:9090` | HTTP listen address |
| `--container-prefix` | `bcc-node` | Docker container name prefix |
| `--node-count` | `5` | Number of nodes to follow |
| `--node-ports` | `8081,…,8085` | HTTP ports of the nodes (used by scenarios) |

---

## Architecture

```
docker logs bcc-nodeN
       │
       ▼
  LogReader          reads stdout+stderr, strips ANSI escape codes
       │
       ▼
  parser.rs          regex → NodeEvent enum (sled internal logs ignored)
       │
       ▼
  EventBus           broadcast channel + 1000-event ring buffer
       │
       ├──▶ WebSocket fan-out  →  browser
       └──▶ replay buffer      →  new clients (last 500 events)
```

The axum server exposes:
- `GET /` — embedded HTML SPA
- `GET /ws` — WebSocket (push + replay on connect)
- `GET /api/events` — JSON snapshot of the recent buffer
- `POST /api/scenario/:name` — triggers a test scenario

---

## Tabs

### ① Event Flow
Real-time Canvas timeline. Each node is split into **4 sub-lanes** by event type:

| Sub-lane | Color | Events |
|----------|-------|--------|
| `block` | green | `BlockProposed`, `BlockFromPeer`, `BlockIgnored` |
| `tx` | blue | `TxAccepted`, `TxGossipAccepted`, `TxRejected`, `TxEvicted`, `MempoolDrain` |
| `slot` | purple | `SlotNotProposer` (thin grey bars = `SlotTick`) |
| `peer` | orange | `PeerConnected`, `PeerDisconnected`, `NodeStarting` |

Click any dot to inspect all structured fields of the event.

Controls: time window (15–300 s), pixel density (px/s), live-follow or pause mode.

### ② Block Propagation
Table of produced blocks. For each height:
- **[P]** = proposer node
- **[✓]** = node that received the block
- **[?]** = node still waiting
- **Spread** = delay between proposal and the last node to receive it

### ③ Mempool
Pool-size bars per node (max 10 000 TXs). Counters for accepts, rejects, and evictions since start. Log of the last 40 mempool events.

### ④ P2P Network
SVG graph of the 5 validators arranged in a pentagon. Edges = active TCP connections maintained by the P2P connector. Animated pulses show live gossip (green = block, blue = TX).

### ⑤ Test Scenarios
Six scenarios runnable directly from the UI. Each scenario publishes its steps as `ScenarioEvent` entries visible in real time (also in tab ①).

| Scenario | Timeout | What it checks |
|----------|---------|----------------|
| Single Transfer | 40 s | 1 TX confirmed on all 5 nodes |
| Concurrent Sends | 60 s | 5 simultaneous TXs to 5 recipients |
| Double Spend | 90 s | 5 identical TXs (same UTXO) → exactly 1 committed |
| Mempool Flood | 30 s | 30 rapid TXs → at least 1 block produced |
| Chain Consistency | 60 s | 10 concurrent TXs → all nodes at the same tip hash |
| Validator Rotation | 75 s | 15 slots observed → ≥3 blocks produced (PoS liveness) |

---

## Log format

Only lines containing `bcc_node` are parsed (sled internal logs are discarded early). Expected format from `docker logs`:

```
2026-04-23T11:11:40.960467119Z  INFO tokio-rt-worker bcc_node::slot_ticker: crates/.../slot_ticker.rs:99: proposed block height=5 hash=abc slot=12 txs=3
```

ANSI color codes injected by Docker's pseudo-TTY are stripped automatically before parsing.

---

## Notes

- The visualizer is read-only except when running scenarios (which submit real transactions to the nodes).
- To disable ANSI colors at the source and reduce parser CPU, add `NO_COLOR=1` under `environment` in `docker-compose.yml`.
