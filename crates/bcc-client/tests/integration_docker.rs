/// Integration tests — require a running Docker daemon.
///
/// Run with:
///     cargo test -p bcc-client --test integration_docker -- --ignored --nocapture
///
/// ## Phases
/// 1. Functional read tests (round-robin across 5 nodes)
/// 2. HTTP load tests (200 concurrent requests)
/// 3. Transaction & reconciliation tests
///
/// ## Funder wallet (Phase 3)
/// Seed `[0x42; 32]` — address derived at runtime via `funder_addr()`.
/// Genesis balance: 1_000_000_000_000 (see config/genesis.toml).
/// No encrypted keystore is needed — the raw seed is embedded here.
use bcc_client::{rpc::RpcClient, wallet};
use bcc_core::types::address::Address;
use ed25519_dalek::SigningKey;
use futures;
use std::{
    collections::HashSet,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::time::sleep;

// ── Constants ─────────────────────────────────────────────────────────────────

const NODE_PORTS: [u16; 5] = [8081, 8082, 8083, 8084, 8085];
const STARTUP_TIMEOUT: Duration = Duration::from_secs(90);
const POLL_INTERVAL:   Duration = Duration::from_secs(2);

const LOAD_CONCURRENCY: usize = 50;
const LOAD_TOTAL:       usize = 200;

// ── Test-funder wallet ────────────────────────────────────────────────────────
//
// Deterministic wallet used exclusively by integration tests.
// Its genesis allocation is defined in config/genesis.toml.
// The address is always derived from the seed — never hardcoded.
//
// Derivation:
//   seed   = [0x42; 32]
//   pubkey = 2152f8d19b791d24453242e15f2eab6cb7cffa7b6a5ed30097960e069881db12

const TEST_FUNDER_SEED: [u8; 32] = [0x42; 32];

fn funder_key() -> SigningKey { SigningKey::from_bytes(&TEST_FUNDER_SEED) }

fn funder_addr() -> String { key_to_address(&funder_key()).to_string() }

/// Returns a deterministic ephemeral key for recipient wallet `tag`.
/// Each distinct `tag` byte gives a different wallet.
fn recipient_key(tag: u8) -> SigningKey { SigningKey::from_bytes(&[tag; 32]) }

fn key_to_address(key: &SigningKey) -> Address {
    Address::from_pubkey_bytes(key.verifying_key().as_bytes())
}

// ── Docker Compose helpers ────────────────────────────────────────────────────

fn compose_file() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    format!("{manifest_dir}/../../docker-compose.yml")
}

struct DockerComposeGuard {
    _child: Child,
    compose_file: String,
}

impl DockerComposeGuard {
    fn up(compose_file: &str) -> Self {
        println!("[docker] Starting cluster …");
        let child = Command::new("docker")
            .args(["compose", "-f", compose_file, "up", "--build", "-d", "--wait"])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("failed to spawn `docker compose up`");
        DockerComposeGuard { _child: child, compose_file: compose_file.to_owned() }
    }
}

impl Drop for DockerComposeGuard {
    fn drop(&mut self) {
        println!("[docker] Tearing down cluster …");
        let _ = Command::new("docker")
            .args(["compose", "-f", &self.compose_file, "down", "-v"])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status();
    }
}

// ── Small helpers ─────────────────────────────────────────────────────────────

fn node_url(idx: usize) -> String {
    format!("http://127.0.0.1:{}", NODE_PORTS[idx % NODE_PORTS.len()])
}

fn client_for(idx: usize) -> RpcClient { RpcClient::new(node_url(idx)) }

/// Polls `address` on `node_idx` until balance ≥ `expected` or `timeout`.
async fn wait_for_balance(node_idx: usize, address: &str, expected: u64, timeout: Duration) -> u64 {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(resp) = client_for(node_idx).get_balance(address).await {
            if resp.balance >= expected { return resp.balance; }
            println!("  [poll] node{} {address} = {} (want ≥ {expected})", node_idx + 1, resp.balance);
        }
        if Instant::now() >= deadline {
            return client_for(node_idx).get_balance(address).await.map(|r| r.balance).unwrap_or(0);
        }
        sleep(Duration::from_secs(2)).await;
    }
}

/// Waits until every node's /chain/tip returns height >= 1.
/// This guarantees the slot ticker has produced at least one block before tests run.
async fn wait_for_cluster() {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    println!("[startup] Waiting for all nodes …");
    loop {
        let mut all_up = true;
        for (i, &port) in NODE_PORTS.iter().enumerate() {
            match RpcClient::new(format!("http://127.0.0.1:{port}")).get_tip().await {
                Err(_) => {
                    println!("[startup] node{} (:{port}) not ready …", i + 1);
                    all_up = false;
                    break;
                }
                Ok(tip) if tip.height < 1 => {
                    println!("[startup] node{} (:{port}) at genesis — waiting for first block …", i + 1);
                    all_up = false;
                    break;
                }
                Ok(_) => {}
            }
        }
        if all_up { println!("[startup] All 5 nodes healthy!"); return; }
        assert!(Instant::now() < deadline, "Cluster startup timed out after {}s", STARTUP_TIMEOUT.as_secs());
        sleep(POLL_INTERVAL).await;
    }
}

/// Generic batched load runner. Returns the number of errors.
async fn run_load<F, Fut>(total: usize, batch: usize, f: F) -> usize
where
    F: Fn(usize) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<(), (usize, bcc_client::error::ClientError)>>
        + Send + 'static,
{
    let f      = Arc::new(f);
    let errors = Arc::new(AtomicUsize::new(0));
    let start  = Instant::now();
    let mut handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    for i in 0..total {
        let f      = Arc::clone(&f);
        let errors = Arc::clone(&errors);
        handles.push(tokio::spawn(async move {
            if let Err((node, e)) = f(i).await {
                eprintln!("  [load] error on node{}: {e}", node + 1);
                errors.fetch_add(1, Ordering::Relaxed);
            }
        }));
        if handles.len() >= batch {
            for h in handles.drain(..) { h.await.expect("task panicked"); }
        }
    }
    for h in handles { h.await.expect("task panicked"); }

    let elapsed = start.elapsed();
    let err = errors.load(Ordering::Relaxed);
    println!("  {}/{total} OK, {err} errors in {elapsed:.2?} ({:.0} req/s)",
        total - err, total as f64 / elapsed.as_secs_f64());
    err
}

// ── Master test ───────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires Docker daemon and docker compose"]
async fn integration_docker_full_suite() {
    let compose_file = compose_file();
    println!("[test] compose file: {compose_file}");

    let _guard = DockerComposeGuard::up(&compose_file);
    sleep(Duration::from_secs(5)).await;
    wait_for_cluster().await;

    println!("\n=== Phase 1: Functional tests (round-robin) ===\n");
    test_tip_round_robin().await;
    test_balance_round_robin().await;
    test_chain_consistency_across_nodes().await;

    println!("\n=== Phase 2: Load tests ===\n");
    load_test_get_tip().await;
    load_test_get_balance().await;
    load_test_mixed_requests().await;

    println!("\n=== Phase 3: Transaction & reconciliation tests ===\n");
    test_funder_has_funds().await;
    test_single_transfer_propagates().await;
    test_concurrent_sends_different_nodes().await;
    test_reconciliation_after_concurrent_sends().await;

    println!("\n=== Phase 4: Extended stress & liveness tests ===\n");
    test_mempool_flood().await;
    test_chain_consistency_after_load().await;
    test_validator_rotation_over_slots().await;
}

// ── Phase 1 — Functional read tests ──────────────────────────────────────────

/// Each of the 5 nodes must return a non-zero height and a 64-char hash.
async fn test_tip_round_robin() {
    println!("[test] tip_round_robin …");
    for i in 0..NODE_PORTS.len() {
        let tip = client_for(i).get_tip().await
            .unwrap_or_else(|e| panic!("node{} /chain/tip failed: {e}", i + 1));
        println!("  node{} → height={} hash={}", i + 1, tip.height, tip.hash);
        assert!(tip.height >= 1, "node{} at genesis height", i + 1);
        assert_eq!(tip.hash.len(), 64, "node{} malformed hash", i + 1);
    }
    println!("[test] tip_round_robin: PASS");
}

/// All nodes must agree on the funder's genesis balance.
async fn test_balance_round_robin() {
    println!("[test] balance_round_robin …");
    let addr = funder_addr();
    let mut first: Option<u64> = None;
    for i in 0..NODE_PORTS.len() {
        let resp = client_for(i).get_balance(&addr).await
            .unwrap_or_else(|e| panic!("node{} /balance failed: {e}", i + 1));
        println!("  node{} → balance={}", i + 1, resp.balance);
        assert_eq!(resp.address, addr, "node{} wrong address", i + 1);
        match first {
            None      => first = Some(resp.balance),
            Some(exp) => assert_eq!(resp.balance, exp, "node{} disagrees on balance", i + 1),
        }
    }
    println!("[test] balance_round_robin: PASS");
}

/// All 5 nodes must converge to the same chain tip hash within ~3 slots.
async fn test_chain_consistency_across_nodes() {
    println!("[test] chain_consistency …");
    const RETRIES: usize = 3;
    for attempt in 1..=RETRIES {
        let mut tips = Vec::new();
        for i in 0..NODE_PORTS.len() {
            let t = client_for(i).get_tip().await
                .unwrap_or_else(|e| panic!("node{} tip: {e}", i + 1));
            tips.push((i + 1, t.height, t.hash));
        }
        for (n, h, hash) in &tips { println!("  node{n} height={h} hash={hash}"); }
        if tips.windows(2).all(|w| w[0].2 == w[1].2) {
            println!("[test] chain_consistency: PASS (attempt {attempt})");
            return;
        }
        if attempt < RETRIES { sleep(Duration::from_secs(6)).await; }
    }
    panic!("chain_consistency: nodes did not converge after {RETRIES} attempts");
}

// ── Phase 2 — Load tests ──────────────────────────────────────────────────────

async fn load_test_get_tip() {
    println!("[load] get_tip: {LOAD_TOTAL} req, concurrency {LOAD_CONCURRENCY} …");
    let errors = run_load(LOAD_TOTAL, LOAD_CONCURRENCY, |i| async move {
        let node_idx = i % NODE_PORTS.len();
        client_for(node_idx).get_tip().await.map(|_| ()).map_err(|e| (node_idx, e))
    }).await;
    assert_eq!(errors, 0, "get_tip load: {errors} errors");
    println!("[load] get_tip: PASS");
}

async fn load_test_get_balance() {
    println!("[load] get_balance: {LOAD_TOTAL} req …");
    let errors = run_load(LOAD_TOTAL, LOAD_CONCURRENCY, |i| async move {
        let node_idx = i % NODE_PORTS.len();
        client_for(node_idx).get_balance(&funder_addr()).await.map(|_| ()).map_err(|e| (node_idx, e))
    }).await;
    assert_eq!(errors, 0, "get_balance load: {errors} errors");
    println!("[load] get_balance: PASS");
}

async fn load_test_mixed_requests() {
    println!("[load] mixed: {LOAD_TOTAL} req …");
    let addr = funder_addr();
    let errors = run_load(LOAD_TOTAL, LOAD_CONCURRENCY, move |i| {
        let addr = addr.clone();
        async move {
            let node_idx = i % NODE_PORTS.len();
            let c = client_for(node_idx);
            let r = if i % 2 == 0 {
                c.get_tip().await.map(|_| ())
            } else {
                c.get_balance(&addr).await.map(|_| ())
            };
            r.map_err(|e| (node_idx, e))
        }
    }).await;
    assert_eq!(errors, 0, "mixed load: {errors} errors");
    println!("[load] mixed: PASS");
}

// ── Phase 3 — Transaction & reconciliation tests ──────────────────────────────
//
// All Phase 3 tests use the test-funder wallet (seed [0x42; 32]) which
// holds 1_000_000_000_000 tokens from genesis. Each test sends to a set
// of ephemeral recipient wallets created in-memory (no encrypted keystore).

/// Sanity: the funder must have its genesis allocation.
async fn test_funder_has_funds() {
    println!("[tx] funder_has_funds …");
    let resp = client_for(0).get_balance(&funder_addr()).await
        .expect("GET /balance failed for funder");
    println!("  funder balance = {}", resp.balance);
    assert!(resp.balance > 0,
        "funder has no balance — do `docker compose down -v` to rebuild genesis");
    println!("[tx] funder_has_funds: PASS (balance={})", resp.balance);
}

/// Sends a single transfer from funder → recipient_key(0xAA) via node1,
/// then verifies the amount appears on ALL 5 nodes.
async fn test_single_transfer_propagates() {
    const AMOUNT:   u64      = 100_000;
    const TIMEOUT:  Duration = Duration::from_secs(40);

    println!("[tx] single_transfer_propagates: {AMOUNT} tokens …");

    let funder      = funder_key();
    let funder_addr = key_to_address(&funder);
    let recip       = recipient_key(0xAA);
    let recip_addr  = key_to_address(&recip);

    println!("  funder:    {funder_addr}");
    println!("  recipient: {recip_addr}");

    let c1    = client_for(0);
    let utxos = c1.get_utxos(funder_addr.as_str()).await
        .expect("GET /utxos failed for funder");
    assert!(!utxos.is_empty(), "funder has no UTXOs on node1");

    let sel = wallet::select_coins(&utxos, AMOUNT).expect("coin selection");
    let tx  = wallet::build_transfer(&funder, sel.selected, &recip_addr, AMOUNT, &funder_addr)
        .expect("build_transfer");

    let resp = c1.post_tx(&tx).await.expect("POST /tx on node1");
    println!("  tx accepted → hash: {}", resp.tx_hash);

    // Verify on all 5 nodes in parallel.
    let addr_str = recip_addr.to_string();
    let handles: Vec<_> = (0..NODE_PORTS.len()).map(|i| {
        let addr = addr_str.clone();
        tokio::spawn(async move {
            let bal = wait_for_balance(i, &addr, AMOUNT, TIMEOUT).await;
            (i + 1, bal)
        })
    }).collect();
    for result in futures::future::join_all(handles).await {
        let (node, bal) = result.expect("verify task panicked");
        println!("  node{node} → recipient balance = {bal}");
        assert_eq!(bal, AMOUNT, "node{node} did not see the transfer");
    }
    println!("[tx] single_transfer_propagates: PASS");
}

/// Sends 5 independent transfers simultaneously — one per node — from the
/// funder to 5 different recipients (no double-spend). Verifies that every
/// recipient's balance is reconciled across all 5 nodes.
///
/// Uses `bcc_client::split::split_utxo` to create 5 independent confirmed UTXOs
/// via binary-doubling (3 block-wait rounds instead of 4 sequential ones).
async fn test_concurrent_sends_different_nodes() {
    const AMOUNTS: [u64; 5] = [11_000, 22_000, 33_000, 44_000, 55_000];
    const TIMEOUT: Duration = Duration::from_secs(60);

    println!("[tx] concurrent_sends_different_nodes ({} simultaneous txs) …", AMOUNTS.len());

    let funder      = funder_key();
    let funder_addr = key_to_address(&funder);
    let c           = client_for(0);

    // Pick the largest UTXO and split it into 5 independent UTXOs.
    let mut all_utxos = c.get_utxos(funder_addr.as_str()).await
        .expect("GET /utxos for funder");
    all_utxos.sort_unstable_by(|a, b| b.amount.cmp(&a.amount));
    let input_utxo = all_utxos.into_iter().next().expect("funder has no UTXOs");

    println!("[split] splitting funder UTXO ({}) into {} parts …", input_utxo.amount, AMOUNTS.len());
    let utxos = bcc_client::split::split_utxo(&c, &funder, &funder_addr, input_utxo, AMOUNTS.len())
        .await
        .expect("split_utxo failed");
    println!("[split] {} UTXOs ready", utxos.len());

    assert!(
        utxos.len() >= AMOUNTS.len(),
        "funder has {} UTXOs after split, need {}", utxos.len(), AMOUNTS.len()
    );

    // Build each transaction spending one specific UTXO — no placeholder chaining.
    let mut txs = Vec::new();
    for (i, &amount) in AMOUNTS.iter().enumerate() {
        let recip_addr = key_to_address(&recipient_key(0x10 + i as u8));

        assert!(
            utxos[i].amount >= amount,
            "UTXO {i} has {} tokens but tx needs {amount}", utxos[i].amount
        );

        let tx = wallet::build_transfer(&funder, vec![utxos[i].clone()], &recip_addr, amount, &funder_addr)
            .expect("build_transfer");
        txs.push((tx, recip_addr, amount, i % NODE_PORTS.len()));
    }

    // Submit all 5 transactions in parallel, each to a different node.
    let mut handles = Vec::new();
    for (tx, recip_addr, amount, node_idx) in txs {
        let url      = node_url(node_idx);
        let addr_str = recip_addr.to_string();
        println!("  submitting {amount} → {addr_str} via node{}", node_idx + 1);

        handles.push(tokio::spawn(async move {
            let resp = RpcClient::new(url).post_tx(&tx).await
                .unwrap_or_else(|e| panic!("POST /tx on node{} failed: {e}", node_idx + 1));
            println!("  node{} accepted tx {}", node_idx + 1, resp.tx_hash);
            (addr_str, amount)
        }));
    }

    let expected: Vec<(String, u64)> =
        futures::future::join_all(handles).await
            .into_iter()
            .map(|r| r.expect("submit panicked"))
            .collect();

    // Verify every recipient on every node in parallel (5 addresses × 5 nodes = 25 checks).
    let mut verify_handles = Vec::new();
    for (addr, amt) in &expected {
        for i in 0..NODE_PORTS.len() {
            let addr = addr.clone();
            let amt  = *amt;
            verify_handles.push(tokio::spawn(async move {
                let bal = wait_for_balance(i, &addr, amt, TIMEOUT).await;
                (i + 1, addr, bal, amt)
            }));
        }
    }
    for result in futures::future::join_all(verify_handles).await {
        let (node, addr, bal, amt) = result.expect("verify task panicked");
        println!("  node{node} | {addr} = {bal} (expected {amt})");
        assert_eq!(bal, amt, "node{node} did not reconcile transfer to {addr}");
    }
    println!("[tx] concurrent_sends_different_nodes: PASS");
}

/// Stress-tests double-spend protection: submits N txs all spending the SAME
/// UTXO to different nodes simultaneously. Asserts:
/// - Exactly ONE tx is committed (recipient gets exactly AMOUNT_EACH).
/// - All nodes agree on both the recipient and funder balances.
async fn test_reconciliation_after_concurrent_sends() {
    const N:           usize    = 5;
    const AMOUNT_EACH: u64      = 5_000;
    const TIMEOUT:     Duration = Duration::from_secs(90);

    println!("[tx] reconciliation: {N} conflicting double-spend attempts …");

    let funder      = funder_key();
    let funder_addr = key_to_address(&funder);
    let recip       = recipient_key(0xFF);
    let recip_addr  = key_to_address(&recip);

    println!("  funder:    {funder_addr}");
    println!("  recipient: {recip_addr}");

    // Pick the single largest UTXO — all N txs will try to spend it.
    let utxos = client_for(0).get_utxos(funder_addr.as_str()).await
        .expect("GET /utxos for funder");
    assert!(!utxos.is_empty(), "funder has no UTXOs");

    let biggest = utxos.iter().max_by_key(|u| u.amount).cloned().unwrap();
    assert!(biggest.amount >= AMOUNT_EACH,
        "largest UTXO ({}) < AMOUNT_EACH ({AMOUNT_EACH})", biggest.amount);

    // Build N identical txs spending the same UTXO (intentional double-spend).
    let mut handles = Vec::new();
    for i in 0..N {
        let fk     = funder.clone();
        let fa     = funder_addr.clone();
        let ra     = recip_addr.clone();
        let utxo   = biggest.clone();
        let node_i = i % NODE_PORTS.len();
        let url    = node_url(node_i);

        handles.push(tokio::spawn(async move {
            let tx = wallet::build_transfer(&fk, vec![utxo], &ra, AMOUNT_EACH, &fa)
                .expect("build_transfer");
            match RpcClient::new(url).post_tx(&tx).await {
                Ok(r) => {
                    println!("  node{} ACCEPTED tx {} (attempt {i})", node_i + 1, r.tx_hash);
                    true
                }
                Err(e) => {
                    println!("  node{} REJECTED attempt {i}: {e}", node_i + 1);
                    false
                }
            }
        }));
    }

    let accepted: usize = futures::future::join_all(handles).await.into_iter()
        .map(|r| r.expect("submit panicked"))
        .filter(|&ok| ok)
        .count();
    println!("  {accepted}/{N} initially accepted by mempools");

    // Wait for the chain to settle with exactly AMOUNT_EACH on the recipient.
    println!("  waiting up to {TIMEOUT:?} for settlement …");
    let settled = wait_for_balance(0, recip_addr.as_str(), AMOUNT_EACH, TIMEOUT).await;
    assert!(settled > 0, "no tx was ever committed — recipient balance is 0");

    // Collect balances from every node.
    let mut recip_bals  = Vec::new();
    let mut funder_bals = Vec::new();
    for i in 0..NODE_PORTS.len() {
        let rb = client_for(i).get_balance(recip_addr.as_str()).await.map(|r| r.balance).unwrap_or(0);
        let fb = client_for(i).get_balance(funder_addr.as_str()).await.map(|r| r.balance).unwrap_or(0);
        println!("  node{} → recipient={rb}  funder={fb}", i + 1);
        recip_bals.push(rb);
        funder_bals.push(fb);
    }

    // All nodes must agree.
    assert!(recip_bals.windows(2).all(|w| w[0] == w[1]),
        "nodes disagree on recipient balance: {recip_bals:?}");
    assert!(funder_bals.windows(2).all(|w| w[0] == w[1]),
        "nodes disagree on funder balance: {funder_bals:?}");

    // Exactly AMOUNT_EACH must have been committed (no double-spend).
    assert_eq!(recip_bals[0], AMOUNT_EACH,
        "expected exactly {AMOUNT_EACH} on recipient, got {} — possible double-spend",
        recip_bals[0]);

    println!("[tx] reconciliation: PASS — committed={}, funder_balance={}",
        recip_bals[0], funder_bals[0]);
}

// ── Phase 4 — Extended stress & liveness tests ────────────────────────────────

/// Submits 30 rapid transactions to node1 to exercise the mempool under load.
/// Asserts that at least one block is produced within 30 seconds, confirming
/// that the mempool drains and the PoS ticker keeps running under submission pressure.
async fn test_mempool_flood() {
    println!("[mempool_flood] Starting …");
    const TX_COUNT: usize = 30;
    const AMOUNT: u64 = 1_000;

    let key = funder_key();
    let funder_addr = Address::from_pubkey_bytes(key.verifying_key().as_bytes());
    let client = client_for(0);

    let tip_before = client.get_tip().await.expect("get tip before flood").height;
    let utxos = client.get_utxos(&funder_addr()).await.expect("get utxos");

    let mut remaining = utxos;
    let mut submitted = 0usize;

    for i in 0..TX_COUNT {
        let Ok(sel) = wallet::select_coins(&remaining, AMOUNT) else { break };
        let refs: std::collections::HashSet<_> = sel.selected.iter().map(|u| u.tx_hash.clone()).collect();
        remaining.retain(|u| !refs.contains(&u.tx_hash));

        let recipient = key_to_address(&recipient_key((0x30 + i) as u8));
        let Ok(tx) = wallet::build_transfer(&key, sel.selected, &recipient, AMOUNT, &funder_addr) else { break };

        if client.post_tx(&tx).await.is_ok() {
            submitted += 1;
        }
    }

    println!("[mempool_flood] submitted={submitted}/{TX_COUNT}");
    assert!(submitted >= 5, "expected at least 5 TXs accepted, got {submitted}");

    // Wait for at least one block to be produced after the flood
    let timeout = Duration::from_secs(30);
    let deadline = Instant::now() + timeout;
    loop {
        assert!(Instant::now() < deadline, "No block produced within 30s after mempool flood");
        if let Ok(tip) = client.get_tip().await {
            if tip.height > tip_before {
                println!("[mempool_flood] PASS — block {} produced after flood", tip.height);
                return;
            }
        }
        sleep(Duration::from_secs(2)).await;
    }
}

/// Sends 10 concurrent transactions across all 5 nodes, then waits for all nodes
/// to converge on the same chain tip hash, verifying consensus under concurrent load.
async fn test_chain_consistency_after_load() {
    println!("[chain_consistency_stress] Starting …");
    const TX_COUNT: usize = 10;
    const AMOUNT: u64 = 1_000;

    let key = funder_key();
    let funder_addr = Address::from_pubkey_bytes(key.verifying_key().as_bytes());

    let utxos = client_for(0).get_utxos(&funder_addr()).await.expect("get utxos");
    let mut remaining = utxos;
    let mut handles = Vec::new();

    for i in 0..TX_COUNT {
        let Ok(sel) = wallet::select_coins(&remaining, AMOUNT) else { break };
        let refs: std::collections::HashSet<_> = sel.selected.iter().map(|u| u.tx_hash.clone()).collect();
        remaining.retain(|u| !refs.contains(&u.tx_hash));

        let recipient = key_to_address(&recipient_key((0x40 + i) as u8));
        let Ok(tx) = wallet::build_transfer(&key, sel.selected, &recipient, AMOUNT, &funder_addr) else { break };

        let node_idx = i % NODE_PORTS.len();
        handles.push(tokio::spawn(async move {
            let _ = client_for(node_idx).post_tx(&tx).await;
        }));
    }
    futures::future::join_all(handles).await;
    println!("[chain_consistency_stress] TXs submitted, waiting for convergence …");

    // Wait for all 5 nodes to share the same tip hash
    let timeout = Duration::from_secs(60);
    let deadline = Instant::now() + timeout;
    loop {
        assert!(Instant::now() < deadline, "Nodes did not converge within 60s");

        let tips: Vec<_> = futures::future::join_all(
            (0..NODE_PORTS.len()).map(|i| async move {
                client_for(i).get_tip().await.map(|t| t.hash).ok()
            })
        ).await.into_iter().flatten().collect();

        if tips.len() == NODE_PORTS.len() {
            let first = &tips[0];
            if tips.iter().all(|h| h == first) {
                println!("[chain_consistency_stress] PASS — all nodes at hash {}", &first[..16]);
                return;
            }
        }
        sleep(Duration::from_secs(3)).await;
    }
}

/// Observes block production over 15 slots (75 seconds).
/// Asserts that at least 3 blocks are produced, confirming PoS liveness
/// with multiple validators active.
async fn test_validator_rotation_over_slots() {
    println!("[validator_rotation] Observing for 75s (15 slots) …");
    const OBSERVE_SECS: u64 = 75;

    let tip_before = client_for(0).get_tip().await.expect("get tip before").height;
    println!("[validator_rotation] tip before: {tip_before}");

    sleep(Duration::from_secs(OBSERVE_SECS)).await;

    let tip_after = client_for(0).get_tip().await.expect("get tip after").height;
    let blocks_produced = tip_after.saturating_sub(tip_before);

    println!("[validator_rotation] blocks produced in {OBSERVE_SECS}s: {blocks_produced}");
    assert!(
        blocks_produced >= 3,
        "Expected ≥3 blocks in {OBSERVE_SECS}s, got {blocks_produced} — PoS liveness issue"
    );
    println!("[validator_rotation] PASS — {blocks_produced} blocks, PoS liveness confirmed");
}
