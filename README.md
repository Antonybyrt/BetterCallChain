# BetterCallChain

A minimalist blockchain written in Rust using **Proof of Stake** consensus.

## Getting Started

### 1. Generate node configs

```bash
git clone <repo>
cd BetterCallChain
./scripts/gen-test-configs.sh
```

This creates `config/node{1..5}.toml` and `config/genesis.toml` with fresh validator keypairs.
The test funder wallet (seed `[0x42;32]`) is always included automatically.

### 2. (Optional) Fund your own wallet at genesis

If you want your wallet to have tokens from block 0, add your address to `config/genesis.toml` **before** starting the network:

```toml
[[accounts]]
address = "bcs1<your address here>"
balance = 1000000000000
```

Retrieve your address with:

```bash
bcc-client wallet new   # create a wallet if you don't have one yet
bcc-client wallet show  # print the address
```

> This step must be done **before** `docker compose up`. If you forgot, restart with `docker compose down -v && docker compose up --build` after editing the file.

### 3. Start the network

```bash
docker compose up --build
```

The visualizer is available at **http://localhost:9090** once the cluster is up.

## Sending tokens

```bash
# Send tokens to an address
bcc-client --rpc-url http://localhost:8081 send <recipient_address> <amount>
```

The `--rpc-url` defaults to `http://127.0.0.1:8080`. When using docker-compose the nodes listen on ports `8081–8085` on the host.

## Workspace

| Crate | Role | Docs |
|-------|------|------|
| `bcc-core` | Pure logic library — types, crypto, consensus, validation, store traits | [docs/bcc-core.md](docs/bcc-core.md) |
| `bcc-node` | Full node binary — P2P, slot ticker, HTTP API, sled persistence | [docs/bcc-node.md](docs/bcc-node.md) |
| `bcc-client` | CLI wallet — key management, transaction building, node interaction | [docs/bcc-client.md](docs/bcc-client.md) |
| `bcc-visualizer` | Web UI for real-time event flow, block propagation, mempool and test scenarios | [docs/bcc-visualizer.md](docs/bcc-visualizer.md) |

## Address Format

BetterCallChain addresses use the `bcs1` prefix followed by 40 hex characters:

```
bcs1a3f2b1c9d7e4f6a8b2c3d4e5f6a7b8c9d0e1f2
```

Derived as `"bcs1" + hex(sha256(pubkey_bytes)[0..20])`.

## License

AGPL-3.0

It's all good man 😉
