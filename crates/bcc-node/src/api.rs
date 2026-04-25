use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use bcc_core::types::{address::Address, transaction::Transaction};

use crate::{
    debug_event::DebugEvent,
    error::NodeError,
    p2p::protocol::Message,
    state::NodeState,
};

/// Starts the HTTP REST API on `state.config.http_addr` and runs until cancelled.
pub async fn run_api(state: NodeState, cancel: CancellationToken) {
    let addr = state.config.http_addr;
    let listener = match TcpListener::bind(addr).await {
        Ok(l)  => l,
        Err(e) => { tracing::error!(%addr, err=%e, "HTTP API: failed to bind"); return; }
    };
    info!(%addr, "HTTP API listening");
    state.emit(DebugEvent::HttpApiReady { http_addr: addr.to_string() });

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            result = listener.accept() => match result {
                Ok((stream, _)) => {
                    let s = state.clone();
                    tokio::spawn(async move { handle_conn(stream, s).await; });
                }
                Err(e) => warn!(err=%e, "HTTP API: accept error"),
            }
        }
    }
}

// ── Connection handler ────────────────────────────────────────────────────────

async fn handle_conn(stream: tokio::net::TcpStream, state: NodeState) {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    // Request line
    let mut req_line = String::new();
    if reader.read_line(&mut req_line).await.unwrap_or(0) == 0 { return; }

    // Headers — find Content-Length
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => return,
            _ => {}
        }
        if line == "\r\n" || line.trim().is_empty() { break; }
        if line.to_lowercase().starts_with("content-length:") {
            content_length = line[15..].trim().parse().unwrap_or(0);
        }
    }

    // Body (for POST /tx)
    let body = if content_length > 0 {
        let mut buf = vec![0u8; content_length.min(4 * 1024 * 1024)];
        if AsyncReadExt::read_exact(&mut reader, &mut buf).await.is_err() { return; }
        buf
    } else {
        vec![]
    };

    // Parse "METHOD /path HTTP/1.1"
    let parts: Vec<&str> = req_line.trim().splitn(3, ' ').collect();
    if parts.len() < 2 { return; }
    let method = parts[0];
    let path   = parts[1];

    let resp = route(method, path, &body, &state).await;
    let _ = write_half.write_all(&resp).await;
}

// ── Router ────────────────────────────────────────────────────────────────────

async fn route(method: &str, path: &str, body: &[u8], state: &NodeState) -> Vec<u8> {
    match (method, path) {
        ("GET",  "/chain/tip")                                   => get_tip(state).await,
        ("GET",  p) if p.starts_with("/balance/")               => get_balance(state, &p[9..]).await,
        ("GET",  p) if p.starts_with("/utxos/")                 => get_utxos(state, &p[7..]).await,
        ("POST", "/tx")                                          => post_tx(state, body).await,
        ("GET",  "/peers")                                       => get_peers(state).await,
        ("OPTIONS", _)                                           => ok_no_content(),
        _                                                        => not_found(),
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn get_tip(state: &NodeState) -> Vec<u8> {
    match state.blocks.tip().map_err(NodeError::Store) {
        Ok(Some((height, hash))) => {
            let hash_hex = hex::encode(hash);
            info!(height, hash = %hash_hex, "api: GET /chain/tip");
            state.emit(DebugEvent::ApiGetTip { height, hash: hash_hex.clone() });
            json_ok(&serde_json::json!({ "height": height, "hash": hash_hex }))
        }
        Ok(None) => {
            warn!("api: GET /chain/tip — chain is empty");
            err_response(503, "chain is empty")
        }
        Err(e) => err_response(503, &e.to_string()),
    }
}

async fn get_balance(state: &NodeState, raw_addr: &str) -> Vec<u8> {
    let addr = match Address::validate(raw_addr) {
        Ok(a)  => a,
        Err(e) => return err_response(400, &e.to_string()),
    };
    match state.utxo.balance(&addr).map_err(NodeError::Store) {
        Ok(balance) => {
            info!(address = %raw_addr, balance, "api: GET /balance");
            json_ok(&serde_json::json!({ "address": raw_addr, "balance": balance }))
        }
        Err(e) => err_response(503, &e.to_string()),
    }
}

async fn get_utxos(state: &NodeState, raw_addr: &str) -> Vec<u8> {
    let addr = match Address::validate(raw_addr) {
        Ok(a)  => a,
        Err(e) => return err_response(400, &e.to_string()),
    };
    match state.utxo.list_utxos(&addr).map_err(NodeError::Store) {
        Ok(utxos) => {
            info!(address = %raw_addr, count = utxos.len(), "api: GET /utxos");
            let items: Vec<_> = utxos.iter().map(|(r, o)| serde_json::json!({
                "tx_hash": hex::encode(r.tx_hash),
                "index":   r.index,
                "amount":  o.amount,
            })).collect();
            json_ok(&serde_json::json!(items))
        }
        Err(e) => err_response(503, &e.to_string()),
    }
}

async fn post_tx(state: &NodeState, body: &[u8]) -> Vec<u8> {
    let tx: Transaction = match serde_json::from_slice(body) {
        Ok(t)  => t,
        Err(e) => return err_response(400, &format!("invalid JSON: {e}")),
    };
    let tx_hash     = tx.hash();
    let tx_hash_hex = hex::encode(tx_hash);
    // Keep a copy for gossip — mempool.add() consumes the original.
    let tx_for_gossip = tx.clone();

    match state.mempool.lock().await.add(tx, &*state.utxo) {
        Ok(added) => {
            info!(tx_hash = %tx_hash_hex, "api: POST /tx accepted");
            if let Some(ev) = &added.evicted {
                state.emit(DebugEvent::TxEvicted { evicted: ev.clone(), new_tx: added.tx_hash.clone() });
            }
            state.emit(DebugEvent::TxAccepted {
                tx_hash:   added.tx_hash.clone(),
                value:     added.value,
                pool_size: added.pool_size,
            });
            state.emit(DebugEvent::ApiTxAccepted { tx_hash: tx_hash_hex.clone() });
            // Propagate to all P2P peers only if this is a new transaction.
            // Duplicates must not be re-broadcast to avoid gossip storms.
            if added.newly_added {
                state.peers.read().await
                    .broadcast_all(Message::NewTx { tx: tx_for_gossip });
            }
            json_ok(&serde_json::json!({ "tx_hash": tx_hash_hex }))
        }
        Err(e) => {
            warn!(tx_hash = %tx_hash_hex, reason = %e, "api: POST /tx rejected");
            state.emit(DebugEvent::TxRejected { tx_hash: tx_hash_hex.clone(), reason: e.to_string() });
            state.emit(DebugEvent::ApiTxRejected { tx_hash: tx_hash_hex, reason: e.to_string() });
            let status = match e {
                NodeError::Validation(_) => 400,
                _                        => 503,
            };
            err_response(status, &e.to_string())
        }
    }
}

async fn get_peers(state: &NodeState) -> Vec<u8> {
    let addrs: Vec<String> = state.peers.read().await.addrs()
        .iter().map(|a| a.to_string()).collect();
    json_ok(&serde_json::json!(addrs))
}

// ── HTTP response helpers ─────────────────────────────────────────────────────

fn json_ok(body: &serde_json::Value) -> Vec<u8> {
    let json = serde_json::to_vec(body).unwrap_or_default();
    build_response(200, "OK", "application/json", &json)
}

fn err_response(status: u16, msg: &str) -> Vec<u8> {
    let body = serde_json::to_vec(&serde_json::json!({ "error": msg })).unwrap_or_default();
    let reason = if status == 400 { "Bad Request" } else { "Service Unavailable" };
    build_response(status, reason, "application/json", &body)
}

fn not_found() -> Vec<u8> {
    err_response(404, "not found")
}

fn ok_no_content() -> Vec<u8> {
    b"HTTP/1.1 204 No Content\r\n\
      Access-Control-Allow-Origin: *\r\n\
      Access-Control-Allow-Methods: GET, POST\r\n\
      Connection: close\r\n\r\n".to_vec()
}

fn build_response(status: u16, reason: &str, content_type: &str, body: &[u8]) -> Vec<u8> {
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {len}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Connection: close\r\n\r\n",
        len = body.len(),
    );
    let mut resp = header.into_bytes();
    resp.extend_from_slice(body);
    resp
}
