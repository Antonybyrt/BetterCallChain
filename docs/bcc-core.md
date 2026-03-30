# bcc-core

Pure logic library. No I/O, no networking, no side effects.
Every other crate depends on this one.

---

## Modules

### `crypto`
| Item | Description |
|------|-------------|
| `sha256(data)` | Single SHA-256 hash |
| `sha256d(data)` | Double SHA-256 — used for block and transaction IDs |
| `verify(pubkey, msg, sig)` | Ed25519 signature verification |

### `types`

**`Address`** — newtype around `String`, always prefixed with `bcs1`.
Derived from `SHA-256(pubkey)[..20]` encoded as hex.
```
bcs1a3f2b1c9d7e4f6...   (44 chars total)
```

**`BlockHeader`** — hashed to identify a block.
```
prev_hash | merkle_root | timestamp | height | slot | proposer
```

**`Block`** — header + Ed25519 signature of the header + list of transactions.
`Block::hash()` = `sha256d(serialize(header))` — signature excluded by design.

**`Transaction`** — UTXO model.
- `TxInput` references a past unspent output (`TxOutRef`) and carries the spend signature.
- `TxOutput` assigns an amount to an address.
- `TxKind`: `Transfer | Stake { amount } | Unstake { amount }`

**`Validator`** — registered block producer: address, pubkey, stake amount, activation slot.

### `consensus`

`elect_proposer(slot, prev_hash, validators) -> Option<&Validator>`

Deterministic weighted election:
1. Seed = `sha256d(prev_hash || slot)`
2. `pick = seed[..8] as u64 % total_stake`
3. Linear scan over validators — each occupies a segment proportional to their stake.

All nodes reach the same result with no communication.

### `validation`

**`validate_block(block, parent, utxo, validators)`**
Checks in order:
1. `height == parent.height + 1`
2. `prev_hash == parent.hash()`
3. `timestamp >= parent.timestamp`
4. proposer matches `elect_proposer` result
5. block signature valid against header bytes
6. all transactions valid (see below)

**`validate_transaction(tx, utxo)`**
1. outputs not empty
2. no zero-value output
3. all inputs exist in UTXO set
4. `sum(inputs) >= sum(outputs)`

### `store`

Three traits, two implementations:

| Trait | Methods |
|-------|---------|
| `BlockStore` | `get_by_height`, `get_by_hash`, `insert`, `tip`, `iter_from` |
| `UtxoStore` | `get`, `apply_block`, `rollback_block`, `balance` |
| `ValidatorStore` | `get`, `all_active`, `upsert` |

`MemoryStore` — in-memory impl using `BTreeMap` + `HashMap` behind `RwLock`. For tests only.
`SledStore` — persistent impl (planned in `bcc-node`).

### `error`

`BccError` — top-level error enum covering: Crypto, Address, BlockValidation, TxValidation, Store, Consensus.

---

## Tests

| Test | Location |
|------|----------|
| `election_is_deterministic` | `consensus::pos` |
| `sole_validator_always_elected` | `consensus::pos` |
| `empty_set_returns_none` | `consensus::pos` |
| `weighted_distribution` (10 000 elections) | `consensus::pos` |
| `test_bad_height` | `validation::block` |
| `test_bad_parent_hash` | `validation::block` |
