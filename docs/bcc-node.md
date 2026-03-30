# bcc-node

The full node binary. Connects to the P2P network, maintains the chain, runs the slot ticker, and exposes a minimal HTTP API for the client.

Depends on: `bcc-core`

---

## Responsibilities

### Chain state (`state.rs`)
Central `NodeState<B, U, V>` struct holding Arc-wrapped stores and shared concurrency primitives:
```
NodeState {
    blocks:     Arc<B: BlockStore>
    utxo:       Arc<U: UtxoStore>
    validators: Arc<V: ValidatorStore>
    mempool:    Arc<Mutex<Mempool>>
    peers:      Arc<RwLock<PeerSet>>
    config:     NodeConfig
}
```

### Mempool (`mempool.rs`)
In-memory pool of unconfirmed transactions.
- `add(tx)` — validate and insert
- `remove(tx_hashes)` — evict after block inclusion
- `drain(max)` — collect transactions for block building

### Slot ticker (`slot_ticker.rs`)
Async loop running every `slot_duration_secs`:
1. Compute current slot from wall clock
2. Fetch active validators, call `elect_proposer`
3. If elected: build block from mempool, sign header, broadcast `NewBlock`
4. Sleep until next slot boundary

### P2P network (`p2p/`)
Plain TCP sockets with length-prefixed JSON messages.

**Protocol messages:**
| Message | Direction |
|---------|-----------|
| `GetBlocks { from_height }` | → peer |
| `Blocks { blocks }` | ← peer |
| `NewBlock { block }` | broadcast |
| `NewTx { tx }` | broadcast |
| `GetPeers` | → peer |
| `Peers { addrs }` | ← peer |
| `Ping / Pong` | keepalive |

**`server.rs`** — TCP listener, spawns one tokio task per incoming peer.
**`handler.rs`** — per-peer read loop: deserialize `Message`, dispatch to `NodeState`.

### Storage (`storage/`)
`SledStore` — persistent implementation of `BlockStore`, `UtxoStore`, `ValidatorStore` using [sled](https://github.com/spacejam/sled).
- Keys are big-endian `u64` heights for ordered scans.
- `apply_block` and `rollback_block` use sled transactions for atomicity.

### HTTP API (`api.rs`)
Minimal REST endpoints for the client:
| Endpoint | Description |
|----------|-------------|
| `GET /chain/tip` | current height and hash |
| `GET /balance/:address` | spendable balance |
| `POST /tx` | submit a signed transaction |
| `GET /peers` | connected peer list |

### Config (`config.rs`)
```
NodeConfig {
    listen_addr:        SocketAddr
    bootstrap_peers:    Vec<SocketAddr>
    slot_duration_secs: u64
    my_address:         Address
    my_signing_key:     SigningKey
}
```

---

## Deployment

Each node is a single binary launched via Docker:
```bash
docker-compose up   # starts 3 nodes (1 miner + 2 peers)
```
