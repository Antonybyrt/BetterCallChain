use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;

use bcc_core::types::{address::Address, transaction::Transaction};

use crate::{error::NodeError, state::NodeState};

/// Builds the axum router with all HTTP API routes.
pub fn router(state: NodeState) -> Router {
    Router::new()
        .route("/chain/tip",         get(get_tip))
        .route("/balance/{address}",  get(get_balance))
        .route("/utxos/{address}",    get(get_utxos))
        .route("/tx",                post(post_tx))
        .route("/peers",             get(get_peers))
        .with_state(state)
}

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct TipResponse {
    height: u64,
    hash:   String,
}

#[derive(Serialize)]
struct BalanceResponse {
    address: String,
    balance: u64,
}

#[derive(Serialize)]
struct TxResponse {
    tx_hash: String,
}

#[derive(Serialize)]
struct UtxoItem {
    tx_hash: String,
    index:   u32,
    amount:  u64,
}

// ── Error wrapper ─────────────────────────────────────────────────────────────

/// Wraps `NodeError` so it can be returned from axum handlers.
struct AppError(NodeError);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self.0 {
            NodeError::Validation(m) => (StatusCode::BAD_REQUEST, m.clone()),
            other => (StatusCode::SERVICE_UNAVAILABLE, other.to_string()),
        };
        (status, msg).into_response()
    }
}

impl From<NodeError> for AppError {
    fn from(e: NodeError) -> Self {
        Self(e)
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `GET /chain/tip` — returns the current chain tip height and hash.
async fn get_tip(State(state): State<NodeState>) -> Result<Json<TipResponse>, AppError> {
    match state.blocks.tip().map_err(NodeError::Store)? {
        Some((height, hash)) => Ok(Json(TipResponse { height, hash: hex::encode(hash) })),
        None => Err(AppError(NodeError::Validation("chain is empty".into()))),
    }
}

/// `GET /balance/:address` — returns the confirmed UTXO balance for `address`.
async fn get_balance(
    State(state):    State<NodeState>,
    Path(address):   Path<String>,
) -> Result<Json<BalanceResponse>, AppError> {
    let addr = Address::validate(&address)
        .map_err(|e| NodeError::Validation(e.to_string()))?;
    let balance = state.utxo.balance(&addr).map_err(NodeError::Store)?;
    Ok(Json(BalanceResponse { address, balance }))
}

/// `GET /utxos/:address` — returns all unspent outputs owned by `address`.
///
/// Used by `bcc-client` for coin selection when building a transfer transaction.
async fn get_utxos(
    State(state):  State<NodeState>,
    Path(address): Path<String>,
) -> Result<Json<Vec<UtxoItem>>, AppError> {
    let addr  = Address::validate(&address)
        .map_err(|e| NodeError::Validation(e.to_string()))?;
    let utxos = state.utxo.list_utxos(&addr).map_err(NodeError::Store)?;
    Ok(Json(utxos
        .into_iter()
        .map(|(out_ref, output)| UtxoItem {
            tx_hash: hex::encode(out_ref.tx_hash),
            index:   out_ref.index,
            amount:  output.amount,
        })
        .collect()))
}

/// `POST /tx` — validates and submits a transaction to the mempool.
///
/// Returns `{ tx_hash }` so the client can track the transaction.
async fn post_tx(
    State(state): State<NodeState>,
    Json(tx):     Json<Transaction>,
) -> Result<Json<TxResponse>, AppError> {
    let tx_hash = tx.hash();
    state.mempool.lock().await.add(tx, &*state.utxo)?;
    Ok(Json(TxResponse { tx_hash: hex::encode(tx_hash) }))
}

/// `GET /peers` — returns the list of currently connected peer addresses.
async fn get_peers(State(state): State<NodeState>) -> Json<Vec<String>> {
    let addrs = state.peers.read().await.addrs();
    Json(addrs.iter().map(|a| a.to_string()).collect())
}
