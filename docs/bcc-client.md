# bcc-client

CLI wallet and transaction tool. Talks to a `bcc-node` over HTTP.
Has no direct dependency on chain internals — only uses the node's public API.

Depends on: `bcc-core` (for address derivation and transaction signing only)

---

## Commands

```bash
bcc wallet new
```
Generates a new Ed25519 keypair, derives the `bcs1...` address, and saves the key to a local file.

```bash
bcc wallet address --key <keyfile>
```
Prints the address associated with a key file.

```bash
bcc balance <address>
```
Queries `GET /balance/:address` on the node and prints the spendable amount.

```bash
bcc send --from <keyfile> --to <address> --amount <u64>
```
1. Fetches UTXOs for the sender address
2. Builds a `Transaction` with the required inputs and outputs
3. Signs each input with the private key
4. Submits via `POST /tx` to the node

```bash
bcc chain info
```
Prints the current chain tip (height + hash) from `GET /chain/tip`.

```bash
bcc stake --from <keyfile> --amount <u64>
```
Submits a `TxKind::Stake` transaction to register as a validator.

```bash
bcc unstake --from <keyfile> --amount <u64>
```
Submits a `TxKind::Unstake` transaction to withdraw staked tokens.

---

## Key storage

Keys are stored as raw 32-byte hex files locally.
The client never sends private keys over the network — signing is done locally before submission.

---

## Node connection

Default node: `http://127.0.0.1:8080`
Override with `--node <url>` on any command.
