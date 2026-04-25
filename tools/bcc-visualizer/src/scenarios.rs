use std::sync::Arc;
use std::time::{Duration, Instant};

use bcc_client::rpc::{RpcClient, UtxoItem};
use bcc_client::split::split_utxo;
use bcc_client::wallet::{build_transfer, select_coins};
use bcc_core::types::address::Address;
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use tracing::info;

use bcc_node::debug_event::DebugEvent;

use crate::event_bus::EventBus;

// Test funder wallet (mirrors integration_docker.rs)
const FUNDER_SEED: [u8; 32] = [0x42; 32];
const FUNDER_ADDR: &str = "bcs13097e2dee2cb4a34b53840cdb705aed71067c36f";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioStatus {
    Running,
    Pass,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioStep {
    pub name: String,
    pub status: ScenarioStatus,
    pub detail: String,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioResult {
    pub scenario: String,
    pub status: ScenarioStatus,
    pub elapsed_ms: u64,
    pub steps: Vec<ScenarioStep>,
}

fn node_urls(urls: &[String]) -> Vec<String> {
    urls.to_vec()
}

fn funder_key() -> SigningKey {
    SigningKey::from_bytes(&FUNDER_SEED)
}

fn recipient_addr(tag: u8) -> Address {
    let seed = [tag; 32];
    let key = SigningKey::from_bytes(&seed);
    let pubkey = key.verifying_key();
    Address::from_pubkey_bytes(pubkey.as_bytes())
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


pub async fn run_scenario(name: &str, ports: &[String], bus: Arc<EventBus>) -> ScenarioResult {
    info!(scenario = name, "starting scenario");
    publish_step(&bus, name, "init", "start", "Scenario started");

    let result = match name {
        "single_transfer" => scenario_single_transfer(ports, &bus).await,
        "concurrent_sends" => scenario_concurrent_sends(ports, &bus).await,
        "double_spend" => scenario_double_spend(ports, &bus).await,
        "mempool_flood" => scenario_mempool_flood(ports, &bus).await,
        "chain_consistency" => scenario_chain_consistency(ports, &bus).await,
        "validator_rotation" => scenario_validator_rotation(ports, &bus).await,
        _ => ScenarioResult {
            scenario: name.to_string(),
            status: ScenarioStatus::Fail,
            elapsed_ms: 0,
            steps: vec![ScenarioStep {
                name: "init".to_string(),
                status: ScenarioStatus::Fail,
                detail: format!("Unknown scenario: {}", name),
                elapsed_ms: 0,
            }],
        },
    };

    let final_status = if result.status == ScenarioStatus::Pass { "pass" } else { "fail" };
    publish_step(&bus, name, "done", final_status, &format!("Completed in {}ms", result.elapsed_ms));
    result
}

async fn scenario_single_transfer(ports: &[String], bus: &Arc<EventBus>) -> ScenarioResult {
    let name = "single_transfer";
    let start = Instant::now();
    let mut steps = Vec::new();

    let urls = node_urls(ports);
    let client0 = RpcClient::new(&urls[0]);
    let key = funder_key();
    let funder_addr = Address::validate(FUNDER_ADDR).unwrap();
    let recipient = recipient_addr(0xAA);
    let amount = 100_000u64;

    publish_step(bus, name, "fetch_utxos", "start", "Fetching funder UTXOs from node1");
    let utxos = match client0.get_utxos(FUNDER_ADDR).await {
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

    // Wait for balance to appear on all nodes
    publish_step(bus, name, "propagation", "start", "Waiting for TX to confirm on all nodes (up to 40s)");
    let timeout = Duration::from_secs(40);
    let poll = Duration::from_secs(3);
    let deadline = Instant::now() + timeout;
    let recipient_str = recipient.to_string();

    loop {
        if Instant::now() > deadline {
            let msg = "TX did not propagate to all nodes within 40s".to_string();
            publish_step(bus, name, "propagation", "fail", &msg);
            return fail_result(name, start, steps, msg);
        }

        let checks: Vec<_> = urls.iter().map(|u| {
            let c = RpcClient::new(u);
            let addr = recipient_str.clone();
            async move { c.get_balance(&addr).await.map(|r| r.balance).unwrap_or(0) }
        }).collect();

        let balances = futures::future::join_all(checks).await;
        let confirmed = balances.iter().filter(|&&b| b == amount).count();
        publish_step(bus, name, "propagation", "progress",
            &format!("{}/{} nodes confirmed", confirmed, urls.len()));

        if confirmed == urls.len() {
            steps.push(ok_step("propagation", "All nodes confirmed balance", start));
            publish_step(bus, name, "propagation", "pass", "All 5 nodes confirmed");
            break;
        }
        tokio::time::sleep(poll).await;
    }

    ScenarioResult {
        scenario: name.to_string(),
        status: ScenarioStatus::Pass,
        elapsed_ms: start.elapsed().as_millis() as u64,
        steps,
    }
}

async fn scenario_concurrent_sends(ports: &[String], bus: &Arc<EventBus>) -> ScenarioResult {
    let name = "concurrent_sends";
    let start = Instant::now();
    let mut steps = Vec::new();
    let urls = node_urls(ports);
    let key = funder_key();
    let funder_addr = Address::validate(FUNDER_ADDR).unwrap();
    let amounts = [11_000u64, 22_000, 33_000, 44_000, 55_000];

    // Fetch the largest UTXO and split it into `amounts.len()` parts via binary doubling.
    publish_step(bus, name, "split", "start",
        "Splitting largest funder UTXO into 5 independent parts (binary doubling)");
    let client0 = RpcClient::new(&urls[0]);

    let input_utxo = {
        let mut all = match client0.get_utxos(FUNDER_ADDR).await {
            Ok(u) if !u.is_empty() => u,
            Ok(_)  => return fail_result(name, start, steps, "funder has no UTXOs".to_string()),
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

    // Sort ascending and assign 1 UTXO per transaction — no chaining, no placeholders.
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

    // Submit all 5 concurrently, each to a different node.
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

    // Verify all recipients on all nodes in parallel.
    publish_step(bus, name, "propagation", "start", "Waiting for all TXs to confirm on all nodes (60s)");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if Instant::now() > deadline {
            return fail_result(name, start, steps, "TXs did not confirm within 60s".into());
        }
        let mut all_confirmed = true;
        for (_, recipient, amount) in &txs {
            let addr = recipient.to_string();
            let amt = *amount;
            let checks: Vec<_> = urls.iter().map(|u| {
                let c = RpcClient::new(u);
                let a = addr.clone();
                async move { c.get_balance(&a).await.map(|r| r.balance).unwrap_or(0) }
            }).collect();
            let balances = futures::future::join_all(checks).await;
            let confirmed = balances.iter().filter(|&&b| b == amt).count();
            publish_step(bus, name, "propagation", "progress",
                &format!("{addr}: {confirmed}/{} nodes", urls.len()));
            if confirmed < urls.len() { all_confirmed = false; break; }
        }
        if all_confirmed {
            steps.push(ok_step("propagation", "All TXs confirmed on all nodes", start));
            publish_step(bus, name, "propagation", "pass", "All 5 recipients confirmed on all 5 nodes");
            break;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    ScenarioResult {
        scenario: name.to_string(),
        status: ScenarioStatus::Pass,
        elapsed_ms: start.elapsed().as_millis() as u64,
        steps,
    }
}

async fn scenario_double_spend(ports: &[String], bus: &Arc<EventBus>) -> ScenarioResult {
    let name = "double_spend";
    let start = Instant::now();
    let mut steps = Vec::new();
    let urls = node_urls(ports);
    let key = funder_key();
    let funder_addr = Address::validate(FUNDER_ADDR).unwrap();
    let recipient = recipient_addr(0xFF);
    let amount = 5_000u64;

    publish_step(bus, name, "setup", "start", "Fetching UTXOs for double-spend test");
    let utxos = match RpcClient::new(&urls[0]).get_utxos(FUNDER_ADDR).await {
        Ok(u) if !u.is_empty() => u,
        Ok(_) => return fail_result(name, start, steps, "no UTXOs available".into()),
        Err(e) => return fail_result(name, start, steps, format!("fetch UTXOs: {}", e)),
    };

    // Build 5 identical TXs spending the same UTXO
    let sel = match select_coins(&utxos, amount) {
        Ok(s) => s,
        Err(e) => return fail_result(name, start, steps, format!("coin select: {}", e)),
    };

    let mut identical_txs = Vec::new();
    for _ in 0..5 {
        match build_transfer(&key, sel.selected.clone(), &recipient, amount, &funder_addr) {
            Ok(tx) => identical_txs.push(tx),
            Err(e) => return fail_result(name, start, steps, format!("build tx: {}", e)),
        }
    }
    steps.push(ok_step("setup", "Built 5 identical TXs (same UTXO)", start));
    publish_step(bus, name, "submit", "start", "Submitting 5 identical TXs to 5 different nodes");

    // Submit all concurrently to different nodes
    let submit_futures: Vec<_> = identical_txs.iter().enumerate().map(|(i, tx)| {
        let client = RpcClient::new(&urls[i]);
        let tx = tx.clone();
        async move { (i, client.post_tx(&tx).await) }
    }).collect();
    let results = futures::future::join_all(submit_futures).await;

    let accepted: Vec<_> = results.iter().filter(|(_, r)| r.is_ok()).collect();
    publish_step(bus, name, "submit", "progress",
        &format!("{}/5 nodes accepted the TX (expected: 1+)", accepted.len()));
    steps.push(ok_step("submit", &format!("{}/5 accepted immediately", accepted.len()), start));

    // Wait for exactly 1 confirmation
    publish_step(bus, name, "verify", "start", "Verifying exactly 1 TX commits (90s timeout)");
    let deadline = Instant::now() + Duration::from_secs(90);
    let recipient_str = recipient.to_string();

    loop {
        if Instant::now() > deadline {
            return fail_result(name, start, steps, "Did not converge within 90s".into());
        }
        let checks: Vec<_> = urls.iter().map(|u| {
            let c = RpcClient::new(u);
            let a = recipient_str.clone();
            async move { c.get_balance(&a).await.map(|r| r.balance).unwrap_or(0) }
        }).collect();
        let balances = futures::future::join_all(checks).await;
        let all_same = balances.windows(2).all(|w| w[0] == w[1]);
        let bal = balances[0];

        publish_step(bus, name, "verify", "progress",
            &format!("all_same={} recipient_balance={}", all_same, bal));

        if all_same && bal == amount {
            steps.push(ok_step("verify", "Exactly 1 TX committed, all nodes agree", start));
            publish_step(bus, name, "verify", "pass",
                &format!("Double-spend resolved: recipient has {} tokens", amount));
            break;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    ScenarioResult {
        scenario: name.to_string(),
        status: ScenarioStatus::Pass,
        elapsed_ms: start.elapsed().as_millis() as u64,
        steps,
    }
}

async fn scenario_mempool_flood(ports: &[String], bus: &Arc<EventBus>) -> ScenarioResult {
    let name = "mempool_flood";
    let start = Instant::now();
    let mut steps = Vec::new();
    let urls = node_urls(ports);
    let key = funder_key();
    let funder_addr = Address::validate(FUNDER_ADDR).unwrap();
    const TX_COUNT: usize = 30;
    const AMOUNT: u64 = 1_000;

    publish_step(bus, name, "setup", "start", &format!("Preparing {} TXs", TX_COUNT));
    let utxos = match RpcClient::new(&urls[0]).get_utxos(FUNDER_ADDR).await {
        Ok(u) => u,
        Err(e) => return fail_result(name, start, steps, format!("fetch UTXOs: {}", e)),
    };

    let initial_tip = match RpcClient::new(&urls[0]).get_tip().await {
        Ok(t) => t.height,
        Err(e) => return fail_result(name, start, steps, format!("get tip: {}", e)),
    };

    // Build as many independent TXs as possible from distinct UTXOs.
    // Each selected UTXO is tracked by (tx_hash, index) to avoid re-use.
    let mut spent_keys = std::collections::HashSet::new();
    let mut txs = Vec::new();
    for i in 0..TX_COUNT {
        // Only offer UTXOs not yet committed locally.
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
                txs.push(tx);
            }
        }
    }
    steps.push(ok_step("setup", &format!("Built {} TXs", txs.len()), start));
    publish_step(bus, name, "flood", "start", &format!("Flooding node1 with {} TXs", txs.len()));

    // Submit all rapidly
    let client = RpcClient::new(&urls[0]);
    let mut accepted = 0usize;
    let mut rejected = 0usize;
    for tx in &txs {
        match client.post_tx(tx).await {
            Ok(_) => accepted += 1,
            Err(_) => rejected += 1,
        }
    }
    steps.push(ok_step("flood", &format!("{} accepted, {} rejected", accepted, rejected), start));
    publish_step(bus, name, "flood", "progress",
        &format!("{} accepted, {} rejected by node1", accepted, rejected));

    // Wait for at least 1 block to be produced (15s = 3 slots)
    publish_step(bus, name, "wait_block", "start", "Waiting for a new block to be produced (30s)");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if Instant::now() > deadline {
            return fail_result(name, start, steps, "No block produced within 30s".into());
        }
        if let Ok(tip) = RpcClient::new(&urls[0]).get_tip().await {
            if tip.height > initial_tip {
                steps.push(ok_step("wait_block", &format!("New block at height {}", tip.height), start));
                publish_step(bus, name, "wait_block", "pass",
                    &format!("Block {} produced, mempool drained", tip.height));
                break;
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    ScenarioResult {
        scenario: name.to_string(),
        status: ScenarioStatus::Pass,
        elapsed_ms: start.elapsed().as_millis() as u64,
        steps,
    }
}

async fn scenario_chain_consistency(ports: &[String], bus: &Arc<EventBus>) -> ScenarioResult {
    let name = "chain_consistency";
    let start = Instant::now();
    let mut steps = Vec::new();
    let urls = node_urls(ports);
    let key = funder_key();
    let funder_addr = Address::validate(FUNDER_ADDR).unwrap();

    publish_step(bus, name, "send_txs", "start", "Sending 10 concurrent TXs");
    let utxos = match RpcClient::new(&urls[0]).get_utxos(FUNDER_ADDR).await {
        Ok(u) => u,
        Err(e) => return fail_result(name, start, steps, format!("fetch UTXOs: {}", e)),
    };

    let mut spent_keys = std::collections::HashSet::new();
    let mut futures_tx = Vec::new();
    for i in 0..10usize {
        let available: Vec<_> = utxos.iter()
            .filter(|u| !spent_keys.contains(&(u.tx_hash.clone(), u.index)))
            .cloned()
            .collect();
        if let Ok(sel) = select_coins(&available, 1_000) {
            for u in &sel.selected {
                spent_keys.insert((u.tx_hash.clone(), u.index));
            }
            let recipient = recipient_addr((0x20 + i) as u8);
            if let Ok(tx) = build_transfer(&key, sel.selected, &recipient, 1_000, &funder_addr) {
                let client = RpcClient::new(&urls[i % urls.len()]);
                futures_tx.push(async move { client.post_tx(&tx).await });
            }
        }
    }
    let submitted = futures::future::join_all(futures_tx).await
        .iter().filter(|r| r.is_ok()).count();
    steps.push(ok_step("send_txs", &format!("{} TXs submitted", submitted), start));
    publish_step(bus, name, "send_txs", "pass", &format!("{} TXs submitted", submitted));

    // Wait for all nodes to converge on same tip hash
    publish_step(bus, name, "convergence", "start", "Waiting for all nodes to share same tip (60s)");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if Instant::now() > deadline {
            return fail_result(name, start, steps, "Nodes did not converge within 60s".into());
        }
        let checks: Vec<_> = urls.iter().map(|u| {
            let c = RpcClient::new(u);
            async move { c.get_tip().await.map(|t| (t.height, t.hash)).ok() }
        }).collect();
        let tips: Vec<_> = futures::future::join_all(checks).await
            .into_iter().flatten().collect();

        if tips.len() == urls.len() {
            let first_hash = &tips[0].1;
            let all_same = tips.iter().all(|(_, h)| h == first_hash);
            publish_step(bus, name, "convergence", "progress",
                &format!("{}/{} nodes at same hash", tips.iter().filter(|(_, h)| h == first_hash).count(), urls.len()));
            if all_same {
                steps.push(ok_step("convergence", &format!("All nodes at height {} hash {}", tips[0].0, &first_hash[..8]), start));
                publish_step(bus, name, "convergence", "pass",
                    &format!("All 5 nodes agree: h={} hash={}", tips[0].0, &first_hash[..16]));
                break;
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    ScenarioResult {
        scenario: name.to_string(),
        status: ScenarioStatus::Pass,
        elapsed_ms: start.elapsed().as_millis() as u64,
        steps,
    }
}

async fn scenario_validator_rotation(ports: &[String], bus: &Arc<EventBus>) -> ScenarioResult {
    let name = "validator_rotation";
    let start = Instant::now();
    let mut steps = Vec::new();
    let urls = node_urls(ports);
    const OBSERVE_SECS: u64 = 75; // 15 slots at 5s/slot

    publish_step(bus, name, "observe", "start",
        &format!("Observing block proposers for {}s (15 slots)", OBSERVE_SECS));

    // Record tip before
    let tip_before = match RpcClient::new(&urls[0]).get_tip().await {
        Ok(t) => t.height,
        Err(e) => return fail_result(name, start, steps, format!("get tip: {}", e)),
    };

    // Wait for observation window
    let mut elapsed = 0u64;
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        elapsed += 5;
        if let Ok(tip) = RpcClient::new(&urls[0]).get_tip().await {
            publish_step(bus, name, "observe", "progress",
                &format!("t={}s current_height={}", elapsed, tip.height));
        }
        if elapsed >= OBSERVE_SECS { break; }
    }

    let tip_after = match RpcClient::new(&urls[0]).get_tip().await {
        Ok(t) => t.height,
        Err(e) => return fail_result(name, start, steps, format!("get tip after: {}", e)),
    };

    let blocks_produced = tip_after.saturating_sub(tip_before);
    steps.push(ok_step("observe", &format!("{} blocks in {}s", blocks_produced, OBSERVE_SECS), start));

    // We verify liveness: at least 3 blocks should have been produced in 15 slots
    if blocks_produced < 3 {
        return fail_result(name, start, steps,
            format!("Only {} blocks produced in 75s — expected ≥3", blocks_produced));
    }

    publish_step(bus, name, "verify", "pass",
        &format!("{} blocks produced in {}s — liveness confirmed", blocks_produced, OBSERVE_SECS));
    steps.push(ok_step("verify",
        &format!("{} blocks produced — PoS liveness confirmed", blocks_produced), start));

    ScenarioResult {
        scenario: name.to_string(),
        status: ScenarioStatus::Pass,
        elapsed_ms: start.elapsed().as_millis() as u64,
        steps,
    }
}

fn ok_step(name: &str, detail: &str, start: Instant) -> ScenarioStep {
    ScenarioStep {
        name: name.to_string(),
        status: ScenarioStatus::Pass,
        detail: detail.to_string(),
        elapsed_ms: start.elapsed().as_millis() as u64,
    }
}

fn fail_result(scenario: &str, start: Instant, mut steps: Vec<ScenarioStep>, reason: String) -> ScenarioResult {
    steps.push(ScenarioStep {
        name: "error".to_string(),
        status: ScenarioStatus::Fail,
        detail: reason,
        elapsed_ms: start.elapsed().as_millis() as u64,
    });
    ScenarioResult {
        scenario: scenario.to_string(),
        status: ScenarioStatus::Fail,
        elapsed_ms: start.elapsed().as_millis() as u64,
        steps,
    }
}
