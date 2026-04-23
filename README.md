# BetterCallChain

A minimalist blockchain written in Rust using **Proof of Stake** consensus.

## Getting Started

```bash
git clone <repo>
cd BetterCallChain

# Generate node configs + genesis.toml for the 5-node test network
./scripts/gen-test-configs.sh

# Start the network
docker compose up --build
```

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