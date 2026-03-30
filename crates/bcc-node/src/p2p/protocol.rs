use std::net::SocketAddr;

use bcc_core::types::{block::Block, transaction::Transaction};
use serde::{Deserialize, Serialize};

/// Wire message exchanged between BetterCallChain peers.
///
/// Encoded as JSON and framed with a 4-byte big-endian length prefix
/// (via `LengthDelimitedCodec`). Maximum frame size: 16 MiB.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum Message {
    /// Request blocks starting at `from_height` (peer returns up to 256 at a time).
    GetBlocks { from_height: u64 },
    /// Response to `GetBlocks`. An empty `blocks` vec signals that sync is complete.
    Blocks { blocks: Vec<Block> },
    /// A newly produced block broadcast to the network.
    NewBlock { block: Box<Block> },
    /// A new unconfirmed transaction propagated to the network.
    NewTx { tx: Transaction },
    /// Request the peer's known peer addresses.
    GetPeers,
    /// Response to `GetPeers`.
    Peers { addrs: Vec<SocketAddr> },
    /// Liveness probe.
    Ping { nonce: u64 },
    /// Response to `Ping`.
    Pong { nonce: u64 },
}
