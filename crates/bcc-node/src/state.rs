use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use bcc_core::{
    store::{BlockStore, UtxoStore, ValidatorStore},
    types::block::Block,
};
use tokio::sync::{broadcast, Mutex, RwLock};

use crate::{config::NodeConfig, mempool::Mempool, p2p::protocol::Message};

/// Central shared state of a running node.
///
/// All fields are reference-counted and cheap to clone — cloning a `NodeState` is the
/// standard way to share access across tasks (P2P handler, slot ticker, HTTP API).
///
/// # Synchronization
/// | Field | Primitive | Rationale |
/// |-------|-----------|-----------|
/// | `mempool` | `Mutex` | Writes dominate; critical sections are short and synchronous |
/// | `peers` | `RwLock` | Reads (broadcast) dominate; peer connections change rarely |
/// | Stores | `Arc<dyn …>` | Stores manage their own interior mutability |
///
/// # Lock ordering
/// **Always acquire the UTXO store access before the mempool lock.**
#[derive(Clone)]
pub struct NodeState {
    /// Persistent block storage.
    pub blocks: Arc<dyn BlockStore>,
    /// Persistent UTXO set.
    pub utxo: Arc<dyn UtxoStore>,
    /// Persistent validator registry.
    pub validators: Arc<dyn ValidatorStore>,
    /// In-memory transaction pool.
    pub mempool: Arc<Mutex<Mempool>>,
    /// Active peer connections.
    pub peers: Arc<RwLock<PeerSet>>,
    /// Node configuration (immutable after startup).
    pub config: Arc<NodeConfig>,
    /// Broadcast channel notifying all peer tasks of a newly produced block.
    pub new_block: broadcast::Sender<Block>,
}

impl NodeState {
    /// Constructs a `NodeState` from already-opened stores and config.
    pub fn new(
        blocks: Arc<dyn BlockStore>,
        utxo: Arc<dyn UtxoStore>,
        validators: Arc<dyn ValidatorStore>,
        config: Arc<NodeConfig>,
    ) -> Self {
        let mempool = Arc::new(Mutex::new(Mempool::new(config.mempool_max_size)));
        let peers = Arc::new(RwLock::new(PeerSet::new()));
        let (new_block, _) = broadcast::channel(64);

        Self { blocks, utxo, validators, mempool, peers, config, new_block }
    }
}

/// Registry of active peer connections.
///
/// Each peer has a dedicated outbound `mpsc` channel so broadcast never blocks on TCP I/O.
pub struct PeerSet {
    peers: HashMap<SocketAddr, tokio::sync::mpsc::Sender<Message>>,
}

impl PeerSet {
    /// Creates an empty peer set.
    pub fn new() -> Self {
        Self { peers: HashMap::new() }
    }

    /// Registers a new peer with its outbound message sender.
    pub fn insert(&mut self, addr: SocketAddr, tx: tokio::sync::mpsc::Sender<Message>) {
        self.peers.insert(addr, tx);
    }

    /// Removes a disconnected peer.
    pub fn remove(&mut self, addr: &SocketAddr) {
        self.peers.remove(addr);
    }

    /// Returns all currently connected peer addresses.
    pub fn addrs(&self) -> Vec<SocketAddr> {
        self.peers.keys().copied().collect()
    }

    /// Sends `msg` to every connected peer except `source`.
    pub fn broadcast_except(&self, source: &SocketAddr, msg: Message) {
        for (addr, tx) in &self.peers {
            if addr != source {
                // Non-blocking try_send: drops the message if the peer's outbound buffer is full.
                // This is intentional — a slow peer should not stall the whole network.
                let _ = tx.try_send(msg.clone());
            }
        }
    }

}

impl Default for PeerSet {
    fn default() -> Self {
        Self::new()
    }
}
