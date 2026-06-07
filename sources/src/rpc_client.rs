//! Thin JSON-RPC client for bitcoind. Only the calls our scanner needs.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

static REQ_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct BitcoinRpc {
    url: String,
    user: String,
    pass: String,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct Req<'a, P: Serialize> {
    jsonrpc: &'static str,
    id: u64,
    method: &'a str,
    params: P,
}

#[derive(Serialize)]
struct BatchReq {
    jsonrpc: &'static str,
    id: u64,
    method: &'static str,
    params: Value,
}

#[derive(Deserialize)]
struct Resp<T> {
    result: Option<T>,
    error: Option<RpcErr>,
}

#[derive(Deserialize)]
struct BatchResp<T> {
    id: u64,
    result: Option<T>,
    error: Option<RpcErr>,
}

#[derive(Deserialize, Debug)]
struct RpcErr {
    code: i64,
    message: String,
}

impl BitcoinRpc {
    pub fn new(url: impl Into<String>, user: impl Into<String>, pass: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            user: user.into(),
            pass: pass.into(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .pool_max_idle_per_host(64)
                .build()
                .expect("http client"),
        }
    }

    async fn call<P, R>(&self, method: &str, params: P) -> anyhow::Result<R>
    where
        P: Serialize,
        R: for<'de> Deserialize<'de>,
    {
        let body = Req {
            jsonrpc: "2.0",
            id: REQ_ID.fetch_add(1, Ordering::Relaxed),
            method,
            params,
        };
        let resp: Resp<R> = self
            .client
            .post(&self.url)
            .basic_auth(&self.user, Some(&self.pass))
            .json(&body)
            .send()
            .await?
            .json()
            .await?;

        if let Some(e) = resp.error {
            anyhow::bail!("RPC {} error {}: {}", method, e.code, e.message);
        }
        resp.result
            .ok_or_else(|| anyhow::anyhow!("empty RPC result"))
    }

    async fn batch_call<R>(&self, requests: Vec<BatchReq>) -> anyhow::Result<Vec<R>>
    where
        R: for<'de> Deserialize<'de>,
    {
        if requests.is_empty() {
            return Ok(Vec::new());
        }

        let ordered_ids = requests.iter().map(|r| r.id).collect::<Vec<_>>();
        let responses: Vec<BatchResp<R>> = self
            .client
            .post(&self.url)
            .basic_auth(&self.user, Some(&self.pass))
            .json(&requests)
            .send()
            .await?
            .json()
            .await?;

        let mut by_id: HashMap<u64, BatchResp<R>> =
            responses.into_iter().map(|resp| (resp.id, resp)).collect();

        let mut out = Vec::with_capacity(ordered_ids.len());
        for id in ordered_ids {
            let Some(resp) = by_id.remove(&id) else {
                anyhow::bail!("missing JSON-RPC batch response id={id}");
            };
            if let Some(e) = resp.error {
                anyhow::bail!("RPC batch id={} error {}: {}", id, e.code, e.message);
            }
            out.push(
                resp.result
                    .ok_or_else(|| anyhow::anyhow!("empty RPC batch result id={id}"))?,
            );
        }
        Ok(out)
    }

    pub async fn get_block_hash(&self, height: u64) -> anyhow::Result<String> {
        self.call("getblockhash", serde_json::json!([height])).await
    }

    pub async fn get_block_hashes(
        &self,
        start_height: u64,
        count: usize,
    ) -> anyhow::Result<Vec<String>> {
        let requests = (0..count)
            .map(|offset| BatchReq {
                jsonrpc: "2.0",
                id: REQ_ID.fetch_add(1, Ordering::Relaxed),
                method: "getblockhash",
                params: serde_json::json!([start_height + offset as u64]),
            })
            .collect();
        self.batch_call(requests).await
    }

    pub async fn get_block_count(&self) -> anyhow::Result<u64> {
        self.call("getblockcount", serde_json::json!([])).await
    }

    /// Fetch raw consensus-encoded block bytes by hex hash (verbosity=0).
    pub async fn get_block_raw_hex(&self, hash_hex: &str) -> anyhow::Result<Vec<u8>> {
        let hex_str: String = self
            .call("getblock", serde_json::json!([hash_hex, 0]))
            .await?;
        Ok(hex::decode(&hex_str)?)
    }

    /// Fetch raw consensus-encoded block bytes for many display-order hex hashes.
    ///
    /// This uses JSON-RPC batch requests to reduce HTTP request/response overhead.
    /// Bitcoin Core still returns each raw block as hex, so REST `.bin` can still
    /// be faster for local nodes.
    pub async fn get_blocks_raw_hex(&self, hashes_hex: &[String]) -> anyhow::Result<Vec<Vec<u8>>> {
        let requests = hashes_hex
            .iter()
            .map(|hash_hex| BatchReq {
                jsonrpc: "2.0",
                id: REQ_ID.fetch_add(1, Ordering::Relaxed),
                method: "getblock",
                params: serde_json::json!([hash_hex, 0]),
            })
            .collect();

        let hex_blocks: Vec<String> = self.batch_call(requests).await?;
        hex_blocks
            .into_iter()
            .map(|hex_block| Ok(hex::decode(hex_block)?))
            .collect()
    }
}
