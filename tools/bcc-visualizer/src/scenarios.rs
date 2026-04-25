use std::sync::Arc;
use std::time::{Duration, Instant};

use bcc_client::rpc::RpcClient;
use bcc_client::split::split_utxo;
use bcc_client::wallet::{build_transfer, select_coins};
use bcc_core::types::address::Address;
use bcc_core::types::transaction::{Transaction, TxInput, TxKind, TxOutRef, TxOutput};
use bcc_core::validation::transaction::tx_signing_bytes;
use ed25519_dalek::{Signature, SigningKey, Signer};
use serde::{Deserialize, Serialize};
use tracing::info;

use bcc_node::debug_event::DebugEvent;

use crate::event_bus::EventBus;

const FUNDER_SEED: [u8; 32] = [0x42; 32];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioStatus {
    Running,
    Pass,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioStep {
    pub name:       String,
    pub status:     ScenarioStatus,
    pub detail:     String,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioResult {
    pub scenario:   String,
    pub status:     ScenarioStatus,
    pub elapsed_ms: u64,
    pub steps:      Vec<ScenarioStep>,
}

fn funder_key() -> SigningKey {
    SigningKey::from_bytes(&FUNDER_SEED)
}

fn funder_addr() -> Address {
    Address::from_pubkey_bytes(funder_key().verifying_key().as_bytes())
}

fn recipient_addr(tag: u8) -> Address {
    let key = SigningKey::from_bytes(&[tag; 32]);
    Address::from_pubkey_bytes(key.verifying_key().as_bytes())
}

fn publish_step(bus: &Arc<EventBus>, scenario: &str, step: &str, status: &str, detail: &str) {
    bus.publish_local(
        "visualizer".to_string(),
        DebugEvent::ScenarioEvent {
            scenario: scenario.to_string(),
            step:     step.to_string(),
            status:   status.to_string(),
            detail:   detail.to_string(),
        },
    );
}

fn ok_step(name: &str, detail: &str, start: Instant) -> ScenarioStep {
    ScenarioStep {
        name:       name.to_string(),
        status:     ScenarioStatus::Pass,
        detail:     detail.to_string(),
        elapsed_ms: start.elapsed().as_millis() as u64,
    }
}

fn fail_result(
    scenario: &str,
    start: Instant,
    mut steps: Vec<ScenarioStep>,
    reason: String,
) -> ScenarioResult {
    steps.push(ScenarioStep {
        name:       "error".to_string(),
        status:     ScenarioStatus::Fail,
        detail:     reason,
        elapsed_ms: start.elapsed().as_millis() as u64,
    });
    ScenarioResult {
        scenario:   scenario.to_string(),
        status:     ScenarioStatus::Fail,
        elapsed_ms: start.elapsed().as_millis() as u64,
        steps,
    }
}

/// Decodes a 64-char hex string into a 32-byte array.
fn hex_to_hash(s: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(s).map_err(|e| format!("hex decode: {e}"))?;
    bytes.try_into().map_err(|_| "tx_hash must be 32 bytes".to_string())
}

/// Polls all nodes until `predicate` is satisfied or `timeout` elapses.
/// Publishes a "progress" step every poll cycle.
async fn wait_until<F, Fut>(
    bus: &Arc<EventBus>,
    scenario: &str,
    step: &str,
    timeout: Duration,
    poll: Duration,
    mut predicate: F,
) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<String>>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() > deadline {
            return false;
        }
        match predicate().await {
            Some(progress) => {
                publish_step(bus, scenario, step, "progress", &progress);
            }
            None => return true,
        }
        tokio::time::sleep(poll).await;
    }
}

pub async fn run_scenario(name: &str, ports: &[String], bus: Arc<EventBus>) -> ScenarioResult {
    info!(scenario = name, "starting scenario");
    publish_step(&bus, name, "init", "start", "Scenario started");

    let result = match name {
        "single_transfer"     => scenario_single_transfer(ports, &bus).await,
        "concurrent_sends"    => scenario_concurrent_sends(ports, &bus).await,
        "double_spend"        => scenario_double_spend(ports, &bus).await,
        "mempool_flood"       => scenario_mempool_flood(ports, &bus).await,
        "balance_conservation" => scenario_balance_conservation(ports, &bus).await,
        "invalid_tx_rejection" => scenario_invalid_tx_rejection(ports, &bus).await,
        _ => ScenarioResult {
            scenario:   name.to_string(),
            status:     ScenarioStatus::Fail,
            elapsed_ms: 0,
            steps: vec![ScenarioStep {
                name:       "init".to_string(),
                status:     ScenarioStatus::Fail,
                detail:     format!("Unknown scenario: {}", name),
                elapsed_ms: 0,
            }],
        },
    };

    let final_status = if result.status == ScenarioStatus::Pass { "pass" } else { "fail" };
    publish_step(&bus, name, "done", final_status, &format!("Completed in {}ms", result.elapsed_ms));
    result
}

// ── single_transfer ───────────────────────────────────────────────────────────

async fn scenario_single_transfer(ports: &[String], bus: &Arc<EventBus>) -> ScenarioResult {
    let name = "single_transfer";
    let start = Instant::now();
    let mut steps = Vec::new();
    let urls = ports.to_vec();
    let client0 = RpcClient::new(&urls[0]);
    let key = funder_key();
    let funder_addr = funder_addr();
    let recipient = recipient_addr(0xAA);
    let amount = 100_000u64;

    publish_step(bus, name, "fetch_utxos", "start", "Fetching funder UTXOs from node1");
    let utxos = match client0.get_utxos(funder_addr.as_str()).await {
        Ok(u) => u,
        Err(e) => {
            let msg = format!("Failed to fetch UTXOs: {}", e);
            publish_step(bus, name, "fetch_utxos", "fail", &msg);
            return fail_result(name, start, steps, msg);
        }
    };
    steps.push(ok_step("fetch_utxos", &format!("Got {} UTXOs", utxos.len()), start));
    publish_step(bus, name, "fetch_utxos", "pass", &format!("{} UTXOs found", utxos.len()));

    let selected = match select_coins(&utxos, amount) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("Coin selection failed: {}", e);
            publish_step(bus, name, "coin_select", "fail", &msg);
            return fail_result(name, start, steps, msg);
        }
    };

    let tx = match build_transfer(&key, selected.selected, &recipient, amount, &funder_addr) {
        Ok(t) => t,
        Err(e) => {
            let msg = format!("Build tx failed: {}", e);
            publish_step(bus, name, "build_tx", "fail", &msg);
            return fail_result(name, start, steps, msg);
        }
    };

    publish_step(bus, name, "submit_tx", "start", "Submitting TX to node1");
    let tx_hash = match client0.post_tx(&tx).await {
        Ok(r) => r.tx_hash,
        Err(e) => {
            let msg = format!("Submit TX failed: {}", e);
            publish_step(bus, name, "submit_tx", "fail", &msg);
            return fail_result(name, start, steps, msg);
        }
    };
    steps.push(ok_step("submit_tx", &format!("TX submitted: {}", &tx_hash[..8]), start));
    publish_step(bus, name, "submit_tx", "pass", &format!("tx_hash={}", &tx_hash[..16]));

    publish_step(bus, name, "propagation", "start", "Waiting for TX to confirm on all nodes (40s)");
    let recipient_str = recipient.to_string();
    let urls_c = urls.clone();
    let converged = wait_until(bus, name, "propagation", Duration::from_secs(40), Duration::from_secs(3), || {
        let us = urls_c.clone();
        let addr = recipient_str.clone();
        async move {
            let balances = futures::future::join_all(
                us.iter().map(|u| { let c = RpcClient::new(u); let a = addr.clone();
                    async move { c.get_balance(&a).await.map(|r| r.balance).unwrap_or(0) }
                })
            ).await;
            let confirmed = balances.iter().filter(|&&b| b == amount).count();
            if confirmed == us.len() { None }
            else { Some(format!("{}/{} nodes confirmed", confirmed, us.len())) }
        }
    }).await;

    if !converged {
        let msg = "TX did not propagate to all nodes within 40s".to_string();
        publish_step(bus, name, "propagation", "fail", &msg);
        return fail_result(name, start, steps, msg);
    }
    steps.push(ok_step("propagation", "All nodes confirmed balance", start));
    publish_step(bus, name, "propagation", "pass", "All 5 nodes confirmed");

    ScenarioResult { scenario: name.to_string(), status: ScenarioStatus::Pass,
        elapsed_ms: start.elapsed().as_millis() as u64, steps }
}

// ── concurrent_sends ──────────────────────────────────────────────────────────

async fn scenario_concurrent_sends(ports: &[String], bus: &Arc<EventBus>) -> ScenarioResult {
    let name = "concurrent_sends";
    let start = Instant::now();
    let mut steps = Vec::new();
    let urls = ports.to_vec();
    let key = funder_key();
    let funder_addr = funder_addr();
    let amounts = [11_000u64, 22_000, 33_000, 44_000, 55_000];

    publish_step(bus, name, "split", "start",
        "Splitting largest funder UTXO into 5 independent parts");
    let client0 = RpcClient::new(&urls[0]);

    let input_utxo = {
        let mut all = match client0.get_utxos(funder_addr.as_str()).await {
            Ok(u) if !u.is_empty() => u,
            Ok(_)  => return fail_result(name, start, steps, "funder has no UTXOs".into()),
            Err(e) => return fail_result(name, start, steps, format!("GET /utxos: {e}")),
        };
        all.sort_unstable_by(|a, b| b.amount.cmp(&a.amount));
        all.remove(0)
    };

    let mut utxos = match split_utxo(&client0, &key, &funder_addr, input_utxo, amounts.len()).await {
        Ok(u) => u,
        Err(e) => {
            publish_step(bus, name, "split", "fail", &e);
            return fail_result(name, start, steps, e);
        }
    };
    steps.push(ok_step("split", &format!("{} UTXOs ready", utxos.len()), start));
    publish_step(bus, name, "split", "pass", &format!("{} UTXOs confirmed", utxos.len()));

    if utxos.len() < amounts.len() {
        let msg = format!("Not enough UTXOs: have {}, need {}", utxos.len(), amounts.len());
        return fail_result(name, start, steps, msg);
    }

    utxos.sort_unstable_by_key(|u| u.amount);
    let mut txs = Vec::new();
    for (i, &amount) in amounts.iter().enumerate() {
        let utxo = vec![utxos[i].clone()];
        let recipient = recipient_addr(0x10 + i as u8);
        match build_transfer(&key, utxo, &recipient, amount, &funder_addr) {
            Ok(tx) => txs.push((tx, recipient, amount)),
            Err(e) => return fail_result(name, start, steps, format!("build tx {i}: {e}")),
        }
    }

    publish_step(bus, name, "submit", "start", "Submitting 5 TXs to 5 different nodes simultaneously");
    let submit_futures: Vec<_> = txs.iter().enumerate().map(|(i, (tx, _, _))| {
        let client = RpcClient::new(&urls[i % urls.len()]);
        let tx = tx.clone();
        async move { (i, client.post_tx(&tx).await) }
    }).collect();

    let results = futures::future::join_all(submit_futures).await;
    let mut submitted = 0usize;
    for (i, res) in &results {
        match res {
            Ok(r) => {
                submitted += 1;
                publish_step(bus, name, "submit", "progress",
                    &format!("TX {i} accepted: {}", &r.tx_hash[..16]));
            }
            Err(e) => publish_step(bus, name, "submit", "progress",
                &format!("TX {i} rejected: {e}")),
        }
    }
    steps.push(ok_step("submit", &format!("{submitted}/{} TXs submitted", amounts.len()), start));

    publish_step(bus, name, "propagation", "start", "Waiting for all TXs to confirm on all nodes (60s)");
    let txs_c = txs.clone();
    let urls_c = urls.clone();
    let converged = wait_until(bus, name, "propagation", Duration::from_secs(60), Duration::from_secs(3), || {
        let us = urls_c.clone();
        let ts = txs_c.clone();
        async move {
            for (_, recipient, amount) in &ts {
                let addr = recipient.to_string();
                let amt = *amount;
                let balances = futures::future::join_all(
                    us.iter().map(|u| { let c = RpcClient::new(u); let a = addr.clone();
                        async move { c.get_balance(&a).await.map(|r| r.balance).unwrap_or(0) }
                    })
                ).await;
                let confirmed = balances.iter().filter(|&&b| b == amt).count();
                if confirmed < us.len() {
                    return Some(format!("{addr}: {confirmed}/{} nodes", us.len()));
                }
            }
            None
        }
    }).await;

    if !converged {
        return fail_result(name, start, steps, "TXs did not confirm within 60s".into());
    }
    steps.push(ok_step("propagation", "All TXs confirmed on all nodes", start));
    publish_step(bus, name, "propagation", "pass", "All 5 recipients confirmed on all 5 nodes");

    ScenarioResult { scenario: name.to_string(), status: ScenarioStatus::Pass,
        elapsed_ms: start.elapsed().as_millis() as u64, steps }
}

// ── double_spend ──────────────────────────────────────────────────────────────

async fn scenario_double_spend(ports: &[String], bus: &Arc<EventBus>) -> ScenarioResult {
    let name = "double_spend";
    let start = Instant::now();
    let mut steps = Vec::new();
    let urls = ports.to_vec();
    let key = funder_key();
    let funder_addr = funder_addr();
    let recipient = recipient_addr(0xFF);
    let amount = 5_000u64;

    publish_step(bus, name, "setup", "start", "Fetching UTXOs for double-spend test");
    let utxos = match RpcClient::new(&urls[0]).get_utxos(funder_addr.as_str()).await {
        Ok(u) if !u.is_empty() => u,
        Ok(_)  => return fail_result(name, start, steps, "no UTXOs available".into()),
        Err(e) => return fail_result(name, start, steps, format!("fetch UTXOs: {}", e)),
    };

    let sel = match select_coins(&utxos, amount) {
        Ok(s) => s,
        Err(e) => return fail_result(name, start, steps, format!("coin select: {}", e)),
    };

    // Build 5 identical TXs spending the same UTXO.
    let mut identical_txs = Vec::new();
    for _ in 0..5 {
        match build_transfer(&key, sel.selected.clone(), &recipient, amount, &funder_addr) {
            Ok(tx) => identical_txs.push(tx),
            Err(e) => return fail_result(name, start, steps, format!("build tx: {}", e)),
        }
    }
    steps.push(ok_step("setup", "Built 5 identical TXs (same UTXO)", start));
    publish_step(bus, name, "submit", "start", "Submitting 5 identical TXs to 5 different nodes");

    let submit_futures: Vec<_> = identical_txs.iter().enumerate().map(|(i, tx)| {
        let client = RpcClient::new(&urls[i]);
        let tx = tx.clone();
        async move { (i, client.post_tx(&tx).await) }
    }).collect();
    let results = futures::future::join_all(submit_futures).await;

    let accepted = results.iter().filter(|(_, r)| r.is_ok()).count();
    publish_step(bus, name, "submit", "progress",
        &format!("{}/5 nodes accepted the TX (expected: 1+)", accepted));
    steps.push(ok_step("submit", &format!("{}/5 accepted immediately", accepted), start));

    publish_step(bus, name, "verify", "start", "Verifying exactly 1 TX commits on all nodes (90s)");
    let recipient_str = recipient.to_string();
    let urls_c = urls.clone();
    let converged = wait_until(bus, name, "verify", Duration::from_secs(90), Duration::from_secs(3), || {
        let us = urls_c.clone();
        let addr = recipient_str.clone();
        async move {
            let balances = futures::future::join_all(
                us.iter().map(|u| { let c = RpcClient::new(u); let a = addr.clone();
                    async move { c.get_balance(&a).await.map(|r| r.balance).unwrap_or(0) }
                })
            ).await;
            let all_same = balances.windows(2).all(|w| w[0] == w[1]);
            if all_same && balances[0] == amount { None }
            else { Some(format!("all_same={all_same} balance={}", balances[0])) }
        }
    }).await;

    if !converged {
        return fail_result(name, start, steps, "Did not converge within 90s".into());
    }
    steps.push(ok_step("verify", "Exactly 1 TX committed, all nodes agree", start));
    publish_step(bus, name, "verify", "pass",
        &format!("Double-spend resolved: recipient has {} tokens", amount));

    ScenarioResult { scenario: name.to_string(), status: ScenarioStatus::Pass,
        elapsed_ms: start.elapsed().as_millis() as u64, steps }
}

// ── mempool_flood ─────────────────────────────────────────────────────────────

async fn scenario_mempool_flood(ports: &[String], bus: &Arc<EventBus>) -> ScenarioResult {
    let name = "mempool_flood";
    let start = Instant::now();
    let mut steps = Vec::new();
    let urls = ports.to_vec();
    let key = funder_key();
    let funder_addr = funder_addr();
    const TX_COUNT: usize = 30;
    const AMOUNT: u64 = 1_000;

    publish_step(bus, name, "setup", "start", &format!("Preparing up to {} TXs", TX_COUNT));
    let utxos = match RpcClient::new(&urls[0]).get_utxos(funder_addr.as_str()).await {
        Ok(u) => u,
        Err(e) => return fail_result(name, start, steps, format!("fetch UTXOs: {}", e)),
    };

    let initial_tip = match RpcClient::new(&urls[0]).get_tip().await {
        Ok(t) => t.height,
        Err(e) => return fail_result(name, start, steps, format!("get tip: {}", e)),
    };

    // Build independent TXs from distinct UTXOs.
    let mut spent_keys = std::collections::HashSet::new();
    let mut txs: Vec<(Transaction, Address, u64)> = Vec::new();
    for i in 0..TX_COUNT {
        let available: Vec<_> = utxos.iter()
            .filter(|u| !spent_keys.contains(&(u.tx_hash.clone(), u.index)))
            .cloned()
            .collect();
        if let Ok(sel) = select_coins(&available, AMOUNT) {
            for u in &sel.selected {
                spent_keys.insert((u.tx_hash.clone(), u.index));
            }
            let recipient = recipient_addr(i as u8);
            if let Ok(tx) = build_transfer(&key, sel.selected, &recipient, AMOUNT, &funder_addr) {
                txs.push((tx, recipient, AMOUNT));
            }
        }
    }
    steps.push(ok_step("setup", &format!("Built {} TXs", txs.len()), start));
    publish_step(bus, name, "flood", "start", &format!("Flooding node1 with {} TXs", txs.len()));

    let client = RpcClient::new(&urls[0]);
    let mut accepted_txs: Vec<(Address, u64)> = Vec::new();
    let mut rejected = 0usize;
    for (tx, recipient, amount) in &txs {
        match client.post_tx(tx).await {
            Ok(_)  => accepted_txs.push((recipient.clone(), *amount)),
            Err(_) => rejected += 1,
        }
    }
    steps.push(ok_step("flood",
        &format!("{} accepted, {} rejected", accepted_txs.len(), rejected), start));
    publish_step(bus, name, "flood", "progress",
        &format!("{} accepted, {} rejected by node1", accepted_txs.len(), rejected));

    // Wait for 2 new blocks to ensure the mempool is fully drained.
    publish_step(bus, name, "wait_blocks", "start",
        "Waiting for 2 new blocks to drain the mempool (45s)");
    let urls_c = urls.clone();
    let blocks_done = wait_until(bus, name, "wait_blocks", Duration::from_secs(45), Duration::from_secs(2), || {
        let us = urls_c.clone();
        async move {
            if let Ok(tip) = RpcClient::new(&us[0]).get_tip().await {
                if tip.height >= initial_tip + 2 { return None; }
                return Some(format!("height={} (need {})", tip.height, initial_tip + 2));
            }
            Some("waiting for tip".into())
        }
    }).await;

    if !blocks_done {
        return fail_result(name, start, steps, "2 new blocks not produced within 45s".into());
    }
    steps.push(ok_step("wait_blocks", "2 new blocks produced", start));

    // Verify accepted TXs are confirmed on ALL nodes.
    publish_step(bus, name, "verify", "start",
        &format!("Verifying {} accepted TXs confirmed on all nodes (30s)", accepted_txs.len()));
    let urls_c = urls.clone();
    let accepted_c = accepted_txs.clone();
    let verified = wait_until(bus, name, "verify", Duration::from_secs(30), Duration::from_secs(3), || {
        let us = urls_c.clone();
        let txs = accepted_c.clone();
        async move {
            for (recipient, amount) in &txs {
                let addr = recipient.to_string();
                let amt = *amount;
                let balances = futures::future::join_all(
                    us.iter().map(|u| { let c = RpcClient::new(u); let a = addr.clone();
                        async move { c.get_balance(&a).await.map(|r| r.balance).unwrap_or(0) }
                    })
                ).await;
                let ok = balances.iter().filter(|&&b| b == amt).count();
                if ok < us.len() {
                    return Some(format!("{addr}: {ok}/{} nodes", us.len()));
                }
            }
            None
        }
    }).await;

    if !verified {
        return fail_result(name, start, steps,
            format!("Not all {} TXs confirmed on all nodes within 30s", accepted_txs.len()));
    }
    steps.push(ok_step("verify",
        &format!("All {} accepted TXs confirmed on all 5 nodes", accepted_txs.len()), start));
    publish_step(bus, name, "verify", "pass",
        &format!("{} TXs confirmed on all nodes", accepted_txs.len()));

    ScenarioResult { scenario: name.to_string(), status: ScenarioStatus::Pass,
        elapsed_ms: start.elapsed().as_millis() as u64, steps }
}

// ── balance_conservation ──────────────────────────────────────────────────────

/// Verifies the fundamental economic invariant: no tokens are created or destroyed.
/// After any set of transfers, funder_balance + sum(recipient_balances) must equal
/// the funder's initial balance on every node.
async fn scenario_balance_conservation(ports: &[String], bus: &Arc<EventBus>) -> ScenarioResult {
    let name = "balance_conservation";
    let start = Instant::now();
    let mut steps = Vec::new();
    let urls = ports.to_vec();
    let key = funder_key();
    let funder_addr = funder_addr();
    let amounts = [5_000u64, 7_000, 9_000, 11_000, 13_000];
    let recipients: Vec<Address> = (0..amounts.len())
        .map(|i| recipient_addr(0x50 + i as u8))
        .collect();

    // Snapshot the funder's balance before any transfers.
    publish_step(bus, name, "snapshot", "start", "Reading initial funder balance on all nodes");
    let funder_str = funder_addr.to_string();
    let initial_balances = futures::future::join_all(
        urls.iter().map(|u| { let c = RpcClient::new(u); let a = funder_str.clone();
            async move { c.get_balance(&a).await.map(|r| r.balance) }
        })
    ).await;

    let initial_total = match initial_balances.iter().find(|r| r.is_err()) {
        Some(Err(e)) => return fail_result(name, start, steps, format!("get balance: {e}")),
        _ => initial_balances[0].as_ref().copied().unwrap(),
    };
    // All nodes must agree on the initial balance before we start.
    let all_agree = initial_balances.iter().all(|r| r.as_ref().ok().copied() == Some(initial_total));
    if !all_agree {
        return fail_result(name, start, steps,
            "Nodes disagree on initial funder balance — run after chain convergence".into());
    }
    steps.push(ok_step("snapshot", &format!("Initial funder balance: {initial_total}"), start));
    publish_step(bus, name, "snapshot", "pass", &format!("All nodes agree: {initial_total} tokens"));

    // Split the largest funder UTXO into amounts.len() independent parts first.
    // This guarantees one distinct UTXO per transfer, avoiding the single-UTXO bottleneck.
    publish_step(bus, name, "split", "start",
        &format!("Splitting funder UTXO into {} independent parts", amounts.len()));
    let client0 = RpcClient::new(&urls[0]);
    let input_utxo = {
        let mut all = match client0.get_utxos(funder_addr.as_str()).await {
            Ok(u) if !u.is_empty() => u,
            Ok(_)  => return fail_result(name, start, steps, "funder has no UTXOs".into()),
            Err(e) => return fail_result(name, start, steps, format!("fetch UTXOs: {e}")),
        };
        all.sort_unstable_by(|a, b| b.amount.cmp(&a.amount));
        all.remove(0)
    };
    let mut utxos = match split_utxo(&client0, &key, &funder_addr, input_utxo, amounts.len()).await {
        Ok(u) => u,
        Err(e) => {
            publish_step(bus, name, "split", "fail", &e);
            return fail_result(name, start, steps, e);
        }
    };
    if utxos.len() < amounts.len() {
        return fail_result(name, start, steps,
            format!("Split produced {} UTXOs, need {}", utxos.len(), amounts.len()));
    }
    utxos.sort_unstable_by_key(|u| u.amount);
    steps.push(ok_step("split", &format!("{} UTXOs ready", utxos.len()), start));
    publish_step(bus, name, "split", "pass", &format!("{} UTXOs confirmed", utxos.len()));

    // Send one transfer per split UTXO.
    publish_step(bus, name, "transfers", "start",
        &format!("Sending {} transfers totalling {} tokens", amounts.len(), amounts.iter().sum::<u64>()));
    let mut submitted = 0usize;
    for (i, &amount) in amounts.iter().enumerate() {
        let tx = match build_transfer(&key, vec![utxos[i].clone()], &recipients[i], amount, &funder_addr) {
            Ok(t) => t,
            Err(e) => return fail_result(name, start, steps, format!("build tx #{i}: {e}")),
        };
        let node = &urls[i % urls.len()];
        match RpcClient::new(node).post_tx(&tx).await {
            Ok(_)  => submitted += 1,
            Err(e) => return fail_result(name, start, steps, format!("submit tx #{i}: {e}")),
        }
    }
    steps.push(ok_step("transfers", &format!("{submitted} TXs submitted"), start));
    publish_step(bus, name, "transfers", "pass", &format!("{submitted} transfers submitted"));

    // Wait for all transfers to confirm on all nodes.
    publish_step(bus, name, "confirm", "start", "Waiting for all transfers to confirm (60s)");
    let recipients_c = recipients.clone();
    let urls_c = urls.clone();
    let confirmed = wait_until(bus, name, "confirm", Duration::from_secs(60), Duration::from_secs(3), || {
        let us = urls_c.clone();
        let rs = recipients_c.clone();
        let ams = amounts;
        async move {
            for (recipient, &amount) in rs.iter().zip(ams.iter()) {
                let addr = recipient.to_string();
                let balances = futures::future::join_all(
                    us.iter().map(|u| { let c = RpcClient::new(u); let a = addr.clone();
                        async move { c.get_balance(&a).await.map(|r| r.balance).unwrap_or(0) }
                    })
                ).await;
                let ok = balances.iter().filter(|&&b| b == amount).count();
                if ok < us.len() {
                    return Some(format!("{addr}: {ok}/{} confirmed", us.len()));
                }
            }
            None
        }
    }).await;

    if !confirmed {
        return fail_result(name, start, steps, "Transfers did not confirm within 60s".into());
    }
    steps.push(ok_step("confirm", "All transfers confirmed on all nodes", start));

    // Verify conservation on every node: funder + recipients = initial_total.
    publish_step(bus, name, "conservation", "start",
        "Checking funder + recipients == initial_total on all nodes");
    let mut violations = Vec::new();
    for (ni, url) in urls.iter().enumerate() {
        let client = RpcClient::new(url);
        let funder_bal = match client.get_balance(funder_addr.as_str()).await {
            Ok(r) => r.balance,
            Err(e) => return fail_result(name, start, steps, format!("node{ni} funder balance: {e}")),
        };
        let mut total = funder_bal;
        for recipient in &recipients {
            let addr = recipient.to_string();
            match client.get_balance(&addr).await {
                Ok(r)  => total += r.balance,
                Err(e) => return fail_result(name, start, steps,
                    format!("node{ni} recipient balance: {e}")),
            }
        }
        publish_step(bus, name, "conservation", "progress",
            &format!("node{}: funder={} total={} expected={}", ni + 1, funder_bal, total, initial_total));
        if total != initial_total {
            violations.push(format!("node{}: got {total}, expected {initial_total}", ni + 1));
        }
    }

    if !violations.is_empty() {
        return fail_result(name, start, steps,
            format!("Conservation violated: {}", violations.join("; ")));
    }
    steps.push(ok_step("conservation",
        &format!("All nodes: funder + recipients = {initial_total}"), start));
    publish_step(bus, name, "conservation", "pass",
        &format!("Invariant holds on all 5 nodes (total = {initial_total})"));

    ScenarioResult { scenario: name.to_string(), status: ScenarioStatus::Pass,
        elapsed_ms: start.elapsed().as_millis() as u64, steps }
}

// ── invalid_tx_rejection ──────────────────────────────────────────────────────

/// Verifies that every node rejects malformed transactions before they enter the mempool.
/// Tests three cases: invalid signature, sum(outputs) > sum(inputs), non-existent UTXO.
async fn scenario_invalid_tx_rejection(ports: &[String], bus: &Arc<EventBus>) -> ScenarioResult {
    let name = "invalid_tx_rejection";
    let start = Instant::now();
    let mut steps = Vec::new();
    let urls = ports.to_vec();
    let key = funder_key();
    let funder_addr = funder_addr();

    // Fetch a real UTXO to use as a valid reference in our malformed TXs.
    publish_step(bus, name, "setup", "start", "Fetching a real UTXO as base for invalid TXs");
    let utxos = match RpcClient::new(&urls[0]).get_utxos(funder_addr.as_str()).await {
        Ok(u) if !u.is_empty() => u,
        Ok(_)  => return fail_result(name, start, steps, "funder has no UTXOs".into()),
        Err(e) => return fail_result(name, start, steps, format!("fetch UTXOs: {e}")),
    };
    let utxo = &utxos[0];
    let out_ref = match hex_to_hash(&utxo.tx_hash) {
        Ok(h)  => TxOutRef { tx_hash: h, index: utxo.index },
        Err(e) => return fail_result(name, start, steps, e),
    };
    steps.push(ok_step("setup", &format!("Using UTXO {} (amount={})", &utxo.tx_hash[..8], utxo.amount), start));
    publish_step(bus, name, "setup", "pass", "UTXO ready");

    // Helper: submit tx to all nodes and assert all reject it (non-2xx).
    let assert_all_reject = |tx: Transaction, case: &'static str| {
        let us = urls.clone();
        async move {
            let results = futures::future::join_all(
                us.iter().map(|u| { let c = RpcClient::new(u); let t = tx.clone();
                    async move { c.post_tx(&t).await }
                })
            ).await;
            let accepted: Vec<_> = results.iter().filter(|r| r.is_ok()).collect();
            if accepted.is_empty() {
                Ok(format!("All {} nodes rejected", us.len()))
            } else {
                Err(format!("{case}: {}/{} nodes accepted (expected 0)", accepted.len(), us.len()))
            }
        }
    };

    // Case 1 — Invalid signature: valid structure, signature bytes zeroed.
    publish_step(bus, name, "bad_signature", "start",
        "Submitting TX with all-zero signature to all nodes");
    let bad_sig_tx = Transaction {
        kind:    TxKind::Transfer,
        inputs:  vec![TxInput {
            out_ref:   out_ref.clone(),
            signature: Signature::from_bytes(&[0u8; 64]),
            pubkey:    key.verifying_key(),
        }],
        outputs: vec![TxOutput {
            amount:  utxo.amount / 2,
            address: recipient_addr(0xB1),
        }],
    };
    match assert_all_reject(bad_sig_tx, "bad_signature").await {
        Ok(detail) => {
            steps.push(ok_step("bad_signature", &detail, start));
            publish_step(bus, name, "bad_signature", "pass", &detail);
        }
        Err(e) => {
            publish_step(bus, name, "bad_signature", "fail", &e);
            return fail_result(name, start, steps, e);
        }
    }

    // Case 2 — Overspend: sum(outputs) > sum(inputs), but correctly signed.
    publish_step(bus, name, "overspend", "start",
        "Submitting TX where output exceeds input amount (correctly signed)");
    let overspend_amount = utxo.amount + 1;
    // Build with placeholder signature first (needed to compute the signing message).
    let mut overspend_tx = Transaction {
        kind:    TxKind::Transfer,
        inputs:  vec![TxInput {
            out_ref:   out_ref.clone(),
            signature: Signature::from_bytes(&[0u8; 64]),
            pubkey:    key.verifying_key(),
        }],
        outputs: vec![TxOutput {
            amount:  overspend_amount,
            address: recipient_addr(0xB2),
        }],
    };
    let msg = tx_signing_bytes(&overspend_tx);
    overspend_tx.inputs[0].signature = key.sign(&msg);
    match assert_all_reject(overspend_tx, "overspend").await {
        Ok(detail) => {
            steps.push(ok_step("overspend", &detail, start));
            publish_step(bus, name, "overspend", "pass", &detail);
        }
        Err(e) => {
            publish_step(bus, name, "overspend", "fail", &e);
            return fail_result(name, start, steps, e);
        }
    }

    // Case 3 — Non-existent UTXO: correctly signed TX spending a phantom UTXO.
    publish_step(bus, name, "phantom_utxo", "start",
        "Submitting TX spending a UTXO that does not exist");
    let phantom_ref = TxOutRef { tx_hash: [0xDE; 32], index: 0 };
    let mut phantom_tx = Transaction {
        kind:    TxKind::Transfer,
        inputs:  vec![TxInput {
            out_ref:   phantom_ref,
            signature: Signature::from_bytes(&[0u8; 64]),
            pubkey:    key.verifying_key(),
        }],
        outputs: vec![TxOutput {
            amount:  1_000,
            address: recipient_addr(0xB3),
        }],
    };
    let msg = tx_signing_bytes(&phantom_tx);
    phantom_tx.inputs[0].signature = key.sign(&msg);
    match assert_all_reject(phantom_tx, "phantom_utxo").await {
        Ok(detail) => {
            steps.push(ok_step("phantom_utxo", &detail, start));
            publish_step(bus, name, "phantom_utxo", "pass", &detail);
        }
        Err(e) => {
            publish_step(bus, name, "phantom_utxo", "fail", &e);
            return fail_result(name, start, steps, e);
        }
    }

    ScenarioResult { scenario: name.to_string(), status: ScenarioStatus::Pass,
        elapsed_ms: start.elapsed().as_millis() as u64, steps }
}
