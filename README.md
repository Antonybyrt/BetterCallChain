# BetterCallChain

A minimalist blockchain written in Rust using **Proof of Stake** consensus.

## Getting Started

```bash
git clone <repo>
cd BetterCallChain
cargo run
```

## Workspace

| Crate | Role |
|-------|------|
| `bcc-core` | Pure logic library — types, crypto, consensus, validation, store traits |
| `bcc-node` | Full node binary — P2P, slot ticker, HTTP API, sled persistence |
| `bcc-client` | CLI wallet — key management, transaction building, node interaction |

## Address Format

BetterCallChain addresses use the `bcs1` prefix followed by 40 hex characters:

```
bcs1a3f2b1c9d7e4f6a8b2c3d4e5f6a7b8c9d0e1f2
```

Derived as `"bcs1" + hex(sha256(pubkey_bytes)[0..20])`.

## License

AGPL-3.0

It's all good man 😉