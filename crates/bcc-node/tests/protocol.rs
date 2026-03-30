use bcc_core::types::{
    address::Address,
    block::{Block, BlockHeader},
    transaction::{Transaction, TxKind, TxOutput},
};
use bcc_node::p2p::protocol::Message;
use ed25519_dalek::Signature;

fn minimal_block() -> Block {
    let addr = Address::from_pubkey_bytes(&[0u8; 32]);
    Block {
        header: BlockHeader {
            prev_hash:   [0u8; 32],
            merkle_root: [0u8; 32],
            timestamp:   0,
            height:      0,
            slot:        0,
            proposer:    addr,
        },
        signature: Signature::from_bytes(&[0u8; 64]),
        txs: vec![],
    }
}

fn minimal_tx() -> Transaction {
    Transaction {
        kind: TxKind::Transfer,
        inputs: vec![],
        outputs: vec![TxOutput {
            amount:  1,
            address: Address::from_pubkey_bytes(&[0u8; 32]),
        }],
    }
}

/// Every `Message` variant must survive a JSON serialise → deserialise round-trip.
#[test]
fn message_serde_roundtrip() {
    let messages: Vec<Message> = vec![
        Message::GetBlocks { from_height: 42 },
        Message::Blocks { blocks: vec![minimal_block()] },
        Message::NewBlock { block: Box::new(minimal_block()) },
        Message::NewTx { tx: minimal_tx() },
        Message::GetPeers,
        Message::Peers { addrs: vec!["127.0.0.1:8333".parse().unwrap()] },
        Message::Ping { nonce: 7 },
        Message::Pong { nonce: 7 },
    ];

    for msg in &messages {
        let json = serde_json::to_string(msg).expect("serialize failed");
        let decoded: Message = serde_json::from_str(&json).expect("deserialize failed");
        let re_json = serde_json::to_string(&decoded).expect("re-serialize failed");
        assert_eq!(json, re_json, "roundtrip mismatch for variant");
    }
}
