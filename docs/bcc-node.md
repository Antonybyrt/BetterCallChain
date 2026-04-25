# bcc-node

The full node binary. Connects to the P2P network, maintains the chain, runs the slot ticker, and exposes a minimal HTTP API for the client.

Depends on: `bcc-core`

---

## Startup sequence (`main.rs`)

1. Parse CLI (`--config <path>` — **required**, no default)
2. Init tracing (`RUST_LOG` / default: `bcc_node=info`)
3. Open `SledStore` at `sled_path`
4. `apply_genesis` — idempotent, skipped if height-0 block already exists
5. Build `NodeState`
6. Run IBD — blocks until synced with bootstrap peers
7. Spawn: debug WebSocket, P2P server, connector, slot ticker, HTTP API
8. Wait for Ctrl-C or SIGTERM
9. Cancel all tasks (5 s hard deadline)

---

## Components

### Chain state (`state.rs`)

Central `NodeState` — cheap to clone, shared across all tasks via `Arc`.

| Field | Type | Rationale |
|-------|------|-----------|
| `blocks` | `Arc<dyn BlockStore>` | store manages its own interior mutability |
| `utxo` | `Arc<dyn UtxoStore>` | same |
| `validators` | `Arc<dyn ValidatorStore>` | same |
| `mempool` | `Arc<Mutex<Mempool>>` | writes dominate, short critical sections |
| `peers` | `Arc<RwLock<PeerSet>>` | reads dominate, connections are rare |
| `config` | `Arc<NodeConfig>` | immutable after startup |
| `new_block` | `broadcast::Sender<Block>` | notifies all peer tasks of a new local block |
| `debug_tx` | `broadcast::Sender<DebugEnvelope>` | streams events to the debug WebSocket |

**Lock ordering:** always access UTXO before acquiring the mempool lock.

`PeerSet` holds one `mpsc::Sender<Message>` per connected peer.
- `broadcast_except(source, msg)` — re-gossip from a peer, excludes the source.
- `broadcast_all(msg)` — used by the HTTP API when a transaction is submitted directly.
- `has_ip(ip)` — returns `true` if a peer with the given IP is already connected (dedup guard).

`NodeState::reorg_block(old, new)` — atomic fork swap: `rollback_block(old)` → `blocks.insert(new)` → `apply_block(new)`.

### Genesis (`genesis.rs`)

Applied once at startup via `apply_genesis` (idempotent).

- Builds a genesis block at height 0 with `prev_hash = [0;32]` and a zeroed signature.
- `merkle_root` is `sha256d(GENESIS_MESSAGE)` — permanently encodes the chain's identity.
- Each `[[accounts]]` entry becomes a coinbase-style transaction (no inputs, one output). Each entry must have a **distinct balance** — identical amounts produce the same `tx_hash` and would overwrite each other in the UTXO set.
- Seeds the `ValidatorStore` with the initial validator set from `genesis.toml`.

#### `genesis.toml` format

```toml
timestamp = 1775029012

[[validators]]
address = "bcs1b523d93ee39930528b034952aa1b5b52710f11f8"
pubkey  = "6a9ca12cd7203309c3a92b3a7b94536db8a38e1ceca7fde354f30e26f19d7d79"
stake   = 1000000000000

[[accounts]]
address = "bcs1b523d93ee39930528b034952aa1b5b52710f11f8"
balance = 5000000000000
```

| Section | Field | Description |
|---------|-------|-------------|
| (root) | `timestamp` | Unix timestamp of the genesis block. |
| `[[validators]]` | `address` | `bcs1...` address — must match `my_address` in the node config. |
| `[[validators]]` | `pubkey` | Hex-encoded 32-byte Ed25519 public key. |
| `[[validators]]` | `stake` | Initial stake in base units. Determines block proposal probability. |
| `[[accounts]]` | `address` | `bcs1...` address to credit at genesis. |
| `[[accounts]]` | `balance` | Tokens minted into the UTXO set. Must be unique per entry. |

### IBD — Initial Block Download (`ibd.rs`)

Synchronises the chain with bootstrap peers before the slot ticker starts.

- Connects to each `bootstrap_peer` in order.
- Requests batches of ≤ 256 blocks via `GetBlocks`.
- Each block (height > 0) is validated via `validate_block` before insertion — invalid blocks from a malicious peer abort IBD with `NodeError::Validation`.
- Each batch has an independent **10 s timeout**; after 3 consecutive timeouts the next peer is tried.
- An empty `Blocks` response signals sync-complete.
- If all peers fail before any block is synced (Docker startup race), waits 1 s and retries — up to 5 attempts.

### Mempool (`mempool.rs`)

In-memory pool of unconfirmed transactions.

| Method | Description |
|--------|-------------|
| `add(tx, utxo)` | validate, double-spend check, evict min-value tx if full |
| `drain(max)` | returns top-N txs sorted by descending total output value — does not remove them |
| `remove(hashes)` | evicts after block inclusion |

`add` returns `AddedToPool { newly_added, … }`. Re-gossip is only performed when `newly_added == true`; duplicates are acknowledged without re-broadcasting, preventing gossip storms.

Eviction: when at `mempool_max_size`, the lowest-value tx is replaced only if the new one has strictly higher total output value.

### Slot ticker (`slot_ticker.rs`)

Async loop running every `slot_duration_secs`.

Each iteration captures `now` once at the top. Before evaluating the election:
- If `tip_block.slot >= current_slot`, the slot is already covered by an existing block — skip to avoid proposing a competing fork.

If elected and enough time remains (`MIN_PROPOSE_WINDOW = 1 s`):
1. Drain mempool and release lock.
2. Build `BlockHeader`, serialize and sign outside the mempool lock.
3. `blocks.insert` + `utxo.apply_block`.
4. Re-acquire mempool lock, call `remove` for included txs.
5. `new_block.send` — notifies all peer tasks (including newly connected ones via initial tip push).

### P2P network (`p2p/`)

Plain TCP with `LengthDelimitedCodec` (4-byte BE length + JSON, max 16 MiB per frame).

**Protocol messages:**
| Message | Direction |
|---------|-----------|
| `GetBlocks { from_height }` | → peer |
| `Blocks { blocks }` | ← peer |
| `NewBlock { block }` | broadcast |
| `NewTx { tx }` | broadcast |
| `GetPeers` | → peer |
| `Peers { addrs }` | ← peer |
| `Ping { nonce }` / `Pong { nonce }` | keepalive |

**`server.rs`** — TCP listener; max `MAX_INBOUND_PEERS = 50` simultaneous connections. Spawns one tokio task per peer.

**`connector.rs`** — maintains outbound connections to bootstrap peers. Uses exponential backoff per peer on failure: 2 s → 4 s → … → 64 s. Resets on reconnect.

**`handler.rs`** — full-duplex `select!` loop per peer. Key behaviours:
- On connect: pushes the current tip block to the new peer immediately (covers blocks proposed before the peer connected).
- Dedup: `get_by_hash` at the top of `NewBlock` — already-known blocks are dropped without re-broadcast.
- IP dedup: if a peer with the same IP is already in `PeerSet`, the connection is closed (prevents bidirectional duplicate connections).
- Catch-up: when `block.height > tip + 1`, sends `GetBlocks { from: tip + 1 }` to the source peer.
- Fork choice: competing blocks at the same height use the lower hash as tiebreaker; the winner is applied via `reorg_block`.

### Storage (`storage/sled_store.rs`)

`SledStore` implements all three store traits over six sled trees:

| Tree | Key | Value |
|------|-----|-------|
| `blocks_h` | height (BE u64) | `Block` |
| `blocks_idx` | block hash | height (BE u64) |
| `utxo` | `TxOutRef` | `TxOutput` |
| `validators` | address bytes | `Validator` |
| `meta` | `b"tip"` / `b"apply_in_progress"` | metadata |
| `spent_h` | height + `TxOutRef` | `TxOutput` |

**Crash safety in `apply_block`:** a sentinel key `apply_in_progress` is written to `meta` before any mutation and removed after both sled transactions commit. `spent_h` stores spent inputs so `rollback_block` can restore the full UTXO set.

### HTTP API (`api.rs`)

| Endpoint | Response |
|----------|----------|
| `GET /chain/tip` | `{ height, hash }` |
| `GET /balance/{address}` | `{ address, balance }` |
| `GET /utxos/{address}` | `[{ tx_hash, index, amount }, …]` |
| `POST /tx` | `{ tx_hash }` |
| `GET /peers` | `["ip:port", ...]` |

`POST /tx` adds the transaction to the local mempool **and gossips it to all connected peers** (`broadcast_all`) so block proposers on other nodes can include it promptly.

`NodeError::Validation` → 400, store errors → 503.

### Debug WebSocket (`debug_ws.rs`)

Streams `DebugEnvelope { ts, event: DebugEvent }` as JSON to every connected client.

- Listens on `http_addr + 1000` (e.g. `8080 → 9080`). Accepts **localhost connections only** (`127.0.0.1` / `::1`).
- On each new connection, immediately sends a `NodeSnapshot { tip_height, tip_hash, peers, mempool_size }` so clients connecting after startup can seed their initial state.

### Config (`config.rs`)

Loaded from a TOML file with `BCC__*` environment variable overrides.

| Field | Description |
|-------|-------------|
| `listen_addr` | P2P TCP bind address |
| `bootstrap_peers` | peers to connect to on startup |
| `slot_duration_secs` | PoS slot length |
| `http_addr` | HTTP API bind address |
| `sled_path` | sled database directory |
| `mempool_max_size` | max transaction count in mempool |
| `genesis_path` | path to `genesis.toml` |
| `my_address` | this node's `bcs1…` address |
| `my_signing_key` | Ed25519 secret key — never printed in `Debug` output |

At load time, `my_signing_key` is verified to derive the same address as `my_address`; a mismatch aborts startup with a clear error. Raw key bytes are zeroed immediately after constructing the `SigningKey`.
