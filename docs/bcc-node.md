# bcc-node

The full node binary. Connects to the P2P network, maintains the chain, runs the slot ticker, and exposes a minimal HTTP API for the client.

Depends on: `bcc-core`

---

## Startup sequence (`main.rs`)

1. Parse CLI (`--config <path>`, default: `node.toml`)
2. Init tracing (`RUST_LOG` / default: `bcc_node=info`)
3. Open `SledStore` at `sled_path`
4. `apply_genesis` — idempotent, skipped if height-0 block already exists
5. Build `NodeState`
6. Run IBD — blocks until synced with bootstrap peers
7. Spawn: P2P server, slot ticker, HTTP API
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

**Lock ordering:** always access UTXO before acquiring the mempool lock.

`PeerSet` holds one `mpsc::Sender<Message>` per connected peer.
`broadcast_except(source, msg)` uses non-blocking `try_send` — a slow peer never stalls the network.

### Genesis (`genesis.rs`)

Applied once at startup via `apply_genesis` (idempotent).

- Builds a genesis block at height 0 with `prev_hash = [0;32]`, no transactions, and a zeroed signature.
- `merkle_root` is `sha256d(GENESIS_MESSAGE)` — permanently encodes the chain's identity into the Merkle root.
- Seeds the `ValidatorStore` with the initial validator set from `genesis.toml`.

### IBD — Initial Block Download (`ibd.rs`)

Synchronises the chain with bootstrap peers before the slot ticker starts.

- Connects to each `bootstrap_peer` in order.
- Requests batches of ≤ 256 blocks via `GetBlocks`.
- Each batch has an independent **10 s timeout**; after 3 consecutive timeouts the next peer is tried.
- An empty `Blocks` response signals sync-complete.
- Returns immediately if no bootstrap peers are configured.

### Mempool (`mempool.rs`)

In-memory pool of unconfirmed transactions.

| Method | Description |
|--------|-------------|
| `add(tx, utxo)` | validate, double-spend check, evict min-value tx if full |
| `drain(max)` | returns top-N txs sorted by descending total output value — does not remove them |
| `remove(hashes)` | evicts after block inclusion |

Eviction: when at `mempool_max_size`, the lowest-value tx is replaced only if the new one has strictly higher total output value.

### Slot ticker (`slot_ticker.rs`)

Async loop running every `slot_duration_secs`.

Each iteration captures `now` once at the top — all slot calculations (`slot`, `slot_end`, `elect_proposer`) use the same timestamp to avoid drift.

If elected and enough time remains in the slot (`MIN_PROPOSE_WINDOW = 1 s`):
1. Drain mempool and release lock.
2. Build `BlockHeader`, serialize and sign outside the mempool lock.
3. `blocks.insert` + `utxo.apply_block`.
4. Re-acquire mempool lock, call `remove` for included txs.
5. `new_block.send` — notifies all peer tasks.

Slots already past their boundary are skipped entirely.

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

**`server.rs`** — TCP listener; spawns one tokio task per peer with a `CancellationToken` child token.

**`handler.rs`** — full-duplex `select!` loop per peer with four arms: shutdown cancellation, local block broadcast, outbound queue drain, and inbound frame dispatch. Inbound dispatch handles `NewBlock` (insert + apply + mempool prune + re-gossip), `NewTx` (mempool add + re-gossip), `GetBlocks` (reply with `iter_from(...).take(256)`), `GetPeers`, and `Ping`.

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

**Crash safety in `apply_block`:** a sentinel key `apply_in_progress` is written to `meta` before any mutation and removed after both sled transactions commit. On restart, if the sentinel is present, the node can determine which transaction completed and replay or rollback accordingly. `spent_h` stores the original outputs of spent inputs to make `rollback_block` possible.

### HTTP API (`api.rs`)

| Endpoint | Response |
|----------|----------|
| `GET /chain/tip` | `{ height, hash }` |
| `GET /balance/:address` | `{ address, balance }` |
| `POST /tx` | `{ tx_hash }` |
| `GET /peers` | `["ip:port", ...]` |

`NodeError::Validation` → 400, store errors → 503.

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
