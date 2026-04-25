# bcc-visualizer

Real-time visualizer for the BetterCallChain network. Subscribes to the debug WebSocket of each node, aggregates structured events, and streams them via WebSocket to a multi-tab web UI.

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
| `--bind` | `127.0.0.1:9090` | HTTP + WebSocket listen address for the UI |
| `--node-ports` | `8081,8082,8083,8084,8085` | HTTP ports of the nodes (local mode: derives debug WS as `port + 1000` and scenario URLs as `http://127.0.0.1:{port}`) |
| `--debug-urls` | — | Explicit debug WebSocket URLs (Docker mode). Comma-separated, e.g. `ws://172.30.0.2:9080/debug,...`. When set, scenario HTTP URLs are derived from these (port − 1000). |

In Docker, `--debug-urls` is set in `docker-compose.yml` and takes precedence over `--node-ports` for both event collection and scenario execution.

---

## Architecture

```
bcc-node debug WebSocket  (ws://HOST:PORT/debug)
       │
       ▼
  NodeClient          subscribes to each node's DebugEnvelope stream
       │
       ▼
  EventBus            broadcast channel + 1000-event ring buffer
       │
       ├──▶ WebSocket fan-out  →  browser  (live stream)
       └──▶ replay buffer      →  new clients (last ~500 events)
```

The HTTP server exposes:
- `GET /` — embedded HTML SPA
- `GET /ws` — WebSocket (push + full replay on connect)
- `GET /api/events` — JSON snapshot of recent events
- `POST /api/scenario/:name` — triggers a named test scenario

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

### ② Chain & Mempool
Block graph (top): one card per block, coloured dots show propagation across the 5 nodes, orphan/reorged blocks are dimmed in red.

Mempool (bottom): live pool size per node + TX event log (accepts · rejects · gossip · evictions).

### ③ P2P Network
Graph of the 5 validators. Edges = active TCP connections. Animated pulses show live gossip (green = block, blue = TX). Heights shown under each node.

Seeded from `NodeSnapshot` on connect — reflects the current state even when the visualizer joins after the cluster has been running.

### ④ Test Scenarios
Six scenarios runnable directly from the UI. Each publishes step-level `ScenarioEvent` entries visible in real time in tab ①.

| Scenario | Timeout | What it checks |
|----------|---------|----------------|
| Single Transfer | 40 s | 1 TX confirmed on all 5 nodes |
| Concurrent Sends | 60 s | 5 simultaneous TXs to 5 different recipients; uses `split_utxo` (binary doubling) to pre-create 5 independent UTXOs |
| Double Spend | 90 s | 5 identical TXs (same UTXO) → exactly 1 committed, all nodes agree |
| Mempool Flood | 30 s | up to 30 rapid TXs → at least 1 block produced |
| Chain Consistency | 60 s | concurrent TXs → all nodes converge to the same tip hash |
| Validator Rotation | 75 s | 15 slots observed → ≥3 blocks produced (PoS liveness) |

Scenarios connect to nodes using the derived HTTP URLs (see CLI flags). In Docker they use the internal IPs; locally they use `127.0.0.1`.

---

## Notes

- The visualizer is read-only except when running scenarios (which submit real transactions to the nodes).
