# bcc-client

CLI wallet and transaction tool. Communicates with `bcc-node` exclusively over HTTP.
Has no P2P connection — only uses the node's public REST API.

Depends on: `bcc-core` (address derivation, transaction types, signing)

---

## Commands

### `wallet new`

```bash
bcc-client wallet new [--keystore <path>]
```

Generates a fresh Ed25519 keypair, derives the `bcs1…` address, and saves an
encrypted keystore to disk. Prompts for a passphrase twice (must match).

Default keystore path: `~/.bcc/keystore.json`

---

### `wallet show`

```bash
bcc-client wallet show [--keystore <path>]
```

Decrypts the keystore (prompts for passphrase) to verify the passphrase is correct,
then prints the wallet address. No network call is made.

---

### `balance`

```bash
bcc-client balance <address>
```

Calls `GET /balance/{address}` on the node and prints the confirmed spendable balance.

---

### `send`

```bash
bcc-client send <to> <amount> [--keystore <path>]
```

Full UTXO transfer flow:

1. Decrypt the local keystore (prompts for passphrase)
2. Fetch UTXOs for the sender address via `GET /utxos/:address`
3. Select inputs with **first-fit ascending** coin selection (smallest UTXOs first)
4. Build a `TxKind::Transfer` transaction:
   - Output 0: `amount` → `<to>`
   - Output 1 (if change > 0): `total_selected - amount` → sender address
5. Sign each input over `bincode(kind, outputs, out_ref, input_amount)`
6. Submit via `POST /tx` — prints the resulting `tx_hash`

---

### `chain tip`

```bash
bcc-client chain tip
```

Calls `GET /chain/tip` and prints the current height and block hash.

---

### `node init`

```bash
bcc-client node init --output <path> [--peer <addr>]... [options]
```

Generates a `bcc-node` configuration TOML file ready to use with `--config`.

If `--keystore` is provided the signing key is read from the (decrypted) keystore.
Otherwise a fresh Ed25519 keypair is generated and the **address + public key are printed** — add them to `genesis.toml` `[[validators]]` and/or `[[accounts]]` before starting the network.

```bash
# New keypair
bcc-client node init \
  --output config/node1.toml \
  --peer 172.30.0.3:8333 --peer 172.30.0.4:8333 \
  --sled-path /data/node1

# From existing keystore
bcc-client node init \
  --output config/node1.toml \
  --keystore ~/.bcc/keystore.json \
  --peer 172.30.0.3:8333 --peer 172.30.0.4:8333 \
  --sled-path /data/node1 \
  --genesis-path /app/config/genesis.toml
```

| Flag | Default | Description |
|------|---------|-------------|
| `--output <path>` | — | **Required.** Path to write the config file. |
| `--peer <addr>` | — | Bootstrap peer (`host:port`). Repeat for multiple. |
| `--keystore <path>` | — | Use signing key from this keystore instead of generating a new one. |
| `--listen-addr <addr>` | `0.0.0.0:8333` | P2P listen address. |
| `--http-addr <addr>` | `0.0.0.0:8080` | HTTP API listen address. |
| `--sled-path <path>` | `/data/node` | Sled database directory. |
| `--genesis-path <path>` | `/app/config/genesis.toml` | Path to the genesis TOML file. |
| `--slot-duration <secs>` | `5` | Slot duration in seconds. |
| `--mempool-max-size <n>` | `10000` | Maximum pending transactions. |

For generating all 5 test-network configs at once, see [`scripts/gen-test-configs.sh`](../scripts/gen-test-configs.sh).

---

## Global flags

| Flag | Env var | Default | Description |
|------|---------|---------|-------------|
| `--rpc-url <url>` | `RPC_URL` | `http://127.0.0.1:8080` | Node HTTP base URL for all network commands. |

---

## Keystore format

The keystore is a JSON file on disk. The signing key is **never stored in plaintext**.

```json
{
  "version": 1,
  "address": "bcs1<40 hex chars>",
  "salt":       "<64 hex — 32-byte Argon2id salt>",
  "nonce":      "<24 hex — 12-byte AES-256-GCM nonce>",
  "ciphertext": "<96 hex — 32-byte key seed + 16-byte GCM tag>"
}
```

**Key derivation:** Argon2id with `m=65536` (64 MiB), `t=3`, `p=4` — OWASP 2023
interactive-login minimum. Produces a 32-byte AES-256 key from the passphrase and salt.

**Encryption:** AES-256-GCM over the raw 32-byte signing key seed. The GCM
authentication tag ensures that a wrong passphrase or corrupted file is detected
immediately (returns `error: wrong passphrase or corrupted keystore`).

**Atomic write:** the keystore is written to a `.tmp` file and renamed atomically,
preventing partial writes from corrupting an existing keystore.

---

## Signing scheme

Each `TxInput` is signed over:

```
bincode::serialize( (&kind, &outputs, &out_ref, input_amount) )
```

| Field | Protection |
|-------|------------|
| `kind` | Prevents replaying a Transfer signature as a Stake |
| `outputs` | Prevents output substitution after signing |
| `out_ref` | Prevents copying a signature across inputs of the same key |
| `input_amount` | Prevents a malicious node substituting a larger UTXO of the same address |

All outputs (including change) are finalised **before** any input is signed.

---

## Node connection

Default node: `http://127.0.0.1:8080`

Override with `--rpc-url <url>` (applies to all network commands) or the `RPC_URL` environment variable.
Resolution order: `--rpc-url` flag → `$RPC_URL` → default.

```bash
# Via flag
bcc-client --rpc-url http://172.30.0.2:8080 chain tip

# Via env var
RPC_URL=http://172.30.0.2:8080 bcc-client balance bcs1...
```

---

## Endpoints used

| Command | Endpoint |
|---------|----------|
| `balance` | `GET /balance/{address}` |
| `send` | `GET /utxos/{address}`, `POST /tx` |
| `chain tip` | `GET /chain/tip` |
