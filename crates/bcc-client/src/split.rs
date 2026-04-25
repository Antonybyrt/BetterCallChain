use std::collections::HashSet;
use std::time::{Duration, Instant};

use ed25519_dalek::SigningKey;
use futures::future::join_all;

use bcc_core::types::address::Address;

use crate::{
    rpc::{RpcClient, UtxoItem},
    wallet::build_transfer,
};

const CONFIRM_TIMEOUT_SECS: u64 = 30;

/// Splits `input_utxo` into exactly `target_count` independent UTXOs using a
/// **binary-doubling** strategy:
///
/// ```text
/// Round 1 :  1 UTXO  → submit 1  tx  → wait block →  2 UTXOs
/// Round 2 :  2 UTXOs → submit 2  txs → wait block →  4 UTXOs
/// Round 3 :  4 UTXOs → submit N  txs → wait block →  target_count UTXOs
/// ```
///
/// All transactions in a round are submitted in parallel; the function waits
/// for the resulting UTXOs to appear on-chain before starting the next round.
///
/// Each split divides a UTXO's value into two equal halves
/// (`amount / 2` and `amount - amount / 2`), both assigned to `addr`.
///
/// Returns the final list of `target_count` UTXOs, or an error string on failure.
pub async fn split_utxo(
    client:       &RpcClient,
    key:          &SigningKey,
    addr:         &Address,
    input_utxo:   UtxoItem,
    target_count: usize,
) -> Result<Vec<UtxoItem>, String> {
    if target_count <= 1 {
        return Ok(vec![input_utxo]);
    }

    let mut current: Vec<UtxoItem> = vec![input_utxo];

    while current.len() < target_count {
        // How many UTXOs to split this round: just enough to reach the target.
        let to_split = (target_count - current.len()).min(current.len());
        let (splitting, keeping) = current.split_at(to_split);
        let mut next: Vec<UtxoItem> = keeping.to_vec();

        // Build one split transaction per UTXO and record the two expected outputs.
        type SplitEntry = (bcc_core::types::transaction::Transaction, UtxoItem, UtxoItem);
        let mut built: Vec<SplitEntry> = Vec::new();

        for utxo in splitting {
            if utxo.amount < 2 {
                next.push(utxo.clone());
                continue;
            }
            let half = utxo.amount / 2;
            let tx = build_transfer(key, vec![utxo.clone()], addr, half, addr)
                .map_err(|e| format!("split: build_transfer: {e}"))?;
            let hash_hex = hex::encode(tx.hash());
            let out0 = UtxoItem { tx_hash: hash_hex.clone(), index: 0, amount: half };
            let out1 = UtxoItem { tx_hash: hash_hex,          index: 1, amount: utxo.amount - half };
            built.push((tx, out0, out1));
        }

        if built.is_empty() {
            break;
        }

        // Submit all transactions in parallel.
        let futs: Vec<_> = built.iter().map(|(tx, _, _)| {
            let c = client.clone();
            let t = tx.clone();
            async move { c.post_tx(&t).await }
        }).collect();

        let results = join_all(futs).await;
        for (i, result) in results.into_iter().enumerate() {
            result.map_err(|e| format!("split: POST /tx failed: {e}"))?;
            next.push(built[i].1.clone());
            next.push(built[i].2.clone());
        }

        // Wait until every expected UTXO's tx_hash appears in the on-chain store.
        let expected: HashSet<String> = built.iter()
            .flat_map(|(_, o0, o1)| [o0.tx_hash.clone(), o1.tx_hash.clone()])
            .collect();

        let deadline = Instant::now() + Duration::from_secs(CONFIRM_TIMEOUT_SECS);
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let live = client.get_utxos(addr.as_str()).await.unwrap_or_default();
            if expected.iter().all(|h| live.iter().any(|u| &u.tx_hash == h)) {
                break;
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "split: timed out waiting for {} UTXOs to confirm ({}s)",
                    expected.len(), CONFIRM_TIMEOUT_SECS
                ));
            }
        }

        current = next;
    }

    Ok(current.into_iter().take(target_count).collect())
}
