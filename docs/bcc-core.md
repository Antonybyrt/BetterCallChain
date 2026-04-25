# bcc-core

Pure logic library. No I/O, no networking, no side effects.
Every other crate depends on this one.

---

## Core Invariants

| Invariant | Checked by |
|-----------|------------|
| `sum(inputs) ≥ sum(outputs)` | `validate_transaction` — `InsufficientFunds` if violated |
| No double spend | UTXO set (`InputNotFound` if already spent) + mempool `in_flight` |
| `hash(prev_block)` correct | `validate_block` step 2 — `BadParentHash` |
| Monotonic height | `validate_block` step 1 — `BadHeight` |
| Valid input signature | `validate_transaction` — `InvalidSignature` per input |
| Correct UTXO owner | `validate_transaction` — `InvalidOwner` if pubkey ≠ UTXO address |
| Consistent Merkle root | `validate_block` step 7 — `BadMerkleRoot` |
| Timestamp within declared slot | `validate_block` step 4 — `TimestampBeyondSlot` |

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
- `TxInput` references a past unspent output (`TxOutRef`) and carries the spend signature and public key.
- `TxOutput` assigns an amount to an address.
- `TxKind`: `Transfer | Stake { amount } | Unstake { amount }`

**`Validator`** — registered block producer: address, pubkey, stake amount, activation slot.

### `consensus`

`elect_proposer(slot, prev_hash, validators) -> Option<&Validator>`

Deterministic weighted election:
1. Seed = `sha256d(prev_hash || slot)`
2. `pick = seed[..8] as u64 % total_stake`
3. Linear scan over validators sorted by address — each occupies a segment proportional to their stake.

All nodes reach the same result with no communication.

### `validation`

**`validate_block(block, parent, utxo, validators, slot_duration_secs)`**
Checks in order:
1. `height == parent.height + 1`
2. `prev_hash == parent.hash()`
3. `timestamp >= parent.timestamp`
4. `timestamp < (slot + 1) * slot_duration_secs` — prevents far-future timestamps
5. proposer matches `elect_proposer` result
6. block signature valid against header bytes
7. `merkle_root == compute_merkle_root(txs)` — detects transaction tampering
8. all transactions valid (see below)

**`validate_transaction(tx, utxo)`**
1. outputs not empty
2. no zero-value output
3. for each input:
   - input exists in UTXO set
   - `Address::from_pubkey_bytes(input.pubkey)` matches the UTXO owner
   - Ed25519 signature valid over `TxSigningData` (see below)
4. `sum(inputs) >= sum(outputs)`

**`tx_signing_bytes(tx) -> Vec<u8>`** — public helper.
Returns the canonical signing message: `serde_json::to_vec(TxSigningData { kind, input_out_refs, outputs })`.
Signatures excluded from the message to avoid circular dependency.
Use this when building transactions.

### `store`

Three traits, two implementations:

| Trait | Methods |
|-------|---------|
| `BlockStore` | `get_by_height`, `get_by_hash`, `insert`, `tip`, `iter_from` |
| `UtxoStore` | `get`, `apply_block`, `rollback_block`, `balance`, `list_utxos` |
| `ValidatorStore` | `get`, `all_active`, `upsert` |

`ValidatorStore::upsert` validates that `validator.address` is derived from `validator.pubkey` — mismatched pairs are rejected with `StoreError::Backend`.

`MemoryStore` — in-memory impl using `BTreeMap` + `HashMap` behind `RwLock`. For tests only.
Stores spent outputs keyed by block height so `rollback_block` can fully restore the UTXO set.

`SledStore` — persistent impl backed by [sled](https://github.com/spacejam/sled), lives in `bcc-node` (see bcc-node.md).

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
