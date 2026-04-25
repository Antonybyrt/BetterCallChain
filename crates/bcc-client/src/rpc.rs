use bcc_core::types::transaction::Transaction;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::error::ClientError;

// ── Response types ────────────────────────────────────────────────────────────

/// Response from `GET /chain/tip`.
#[derive(Debug, Deserialize)]
pub struct TipResponse {
    pub height: u64,
    pub hash:   String,
}

/// Response from `GET /balance/:address`.
#[derive(Debug, Deserialize)]
pub struct BalanceResponse {
    pub address: String,
    pub balance: u64,
}

/// One unspent output returned by `GET /utxos/:address`.
#[derive(Debug, Clone, Deserialize)]
pub struct UtxoItem {
    pub tx_hash: String,
    pub index:   u32,
    pub amount:  u64,
}

/// Response from `POST /tx`.
#[derive(Debug, Deserialize)]
pub struct TxResponse {
    pub tx_hash: String,
}

// ── Client ────────────────────────────────────────────────────────────────────

/// Thin async HTTP client for the `bcc-node` REST API.
///
/// All methods return `ClientError::Rpc` on transport failure and
/// `ClientError::NodeError` if the server responds with a non-2xx status.
#[derive(Clone)]
pub struct RpcClient {
    client:   reqwest::Client,
    base_url: String,
}

impl RpcClient {
    /// Creates a new `RpcClient` pointing at `base_url`
    /// (e.g. `"http://127.0.0.1:8080"`).
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client:   reqwest::Client::new(),
            base_url: base_url.into(),
        }
    }

    /// `GET /chain/tip` — returns the current chain tip height and hash.
    pub async fn get_tip(&self) -> Result<TipResponse, ClientError> {
        let resp = self.client
            .get(format!("{}/chain/tip", self.base_url))
            .send()
            .await?;
        check_response(resp).await
    }

    /// `GET /balance/:address` — returns the confirmed UTXO balance for `address`.
    pub async fn get_balance(&self, address: &str) -> Result<BalanceResponse, ClientError> {
        let resp = self.client
            .get(format!("{}/balance/{}", self.base_url, address))
            .send()
            .await?;
        check_response(resp).await
    }

    /// `GET /utxos/:address` — returns all unspent outputs for `address`.
    pub async fn get_utxos(&self, address: &str) -> Result<Vec<UtxoItem>, ClientError> {
        let resp = self.client
            .get(format!("{}/utxos/{}", self.base_url, address))
            .send()
            .await?;
        check_response(resp).await
    }

    /// `POST /tx` — submits a signed transaction. Returns the transaction hash.
    pub async fn post_tx(&self, tx: &Transaction) -> Result<TxResponse, ClientError> {
        let resp = self.client
            .post(format!("{}/tx", self.base_url))
            .json(tx)
            .send()
            .await?;
        check_response(resp).await
    }
}

/// Deserializes a successful response or converts a non-2xx response to
/// `ClientError::NodeError`.
///
/// The `unwrap_or_default()` on `resp.text()` is intentional: at this point
/// we are already constructing a `NodeError` from a bad status code.  If reading
/// the error body itself fails (rare secondary I/O error), an empty string is an
/// acceptable fallback — the primary error (the status code) is preserved.
async fn check_response<T: DeserializeOwned>(
    resp: reqwest::Response,
) -> Result<T, ClientError> {
    if resp.status().is_success() {
        Ok(resp.json::<T>().await?)
    } else {
        let status = resp.status().as_u16();
        let body   = resp.text().await.unwrap_or_default();
        Err(ClientError::NodeError { status, body })
    }
}
