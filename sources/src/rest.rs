//! Bitcoin Core REST source.
//!
//! Uses Bitcoin Core's REST interface instead of JSON-RPC for block payloads.
//! This is intended for local/trusted nodes started with `-rest=1`.
//!
//! Endpoints used:
//! - `GET /rest/blockhashbyheight/<height>.hex`
//! - `GET /rest/block/<hash>.bin`
//! - `GET /rest/chaininfo.json`
//!
//! Compared with JSON-RPC `getblock <hash> 0`, `/rest/block/<hash>.bin`
//! returns raw consensus bytes directly, avoiding the JSON envelope and hex
//! decoding of the full block body.

use async_trait::async_trait;
use bytes::Bytes;
use btc_data_core::source::BlockSource;
use serde::Deserialize;

pub struct RestSource {
    base: String,
    client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct ChainInfo {
    blocks: u64,
}

impl RestSource {
    /// `base` is the Bitcoin Core HTTP base URL, usually the RPC/REST port,
    /// e.g. `http://127.0.0.1:8332` or `http://127.0.0.1:18443`.
    pub fn new(base: impl Into<String>) -> Self {
        let base = normalize_base(base.into());
        Self {
            base,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .pool_max_idle_per_host(64)
                .user_agent("bitcoin-data/0.1")
                .build()
                .expect("http client"),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    async fn get_text(&self, path: &str) -> anyhow::Result<String> {
        let url = self.url(path);
        let resp = self.client.get(&url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("REST {} for {}: {}", status, url, body.trim());
        }
        Ok(resp.text().await?)
    }

    async fn get_bytes(&self, path: &str) -> anyhow::Result<Bytes> {
        let url = self.url(path);
        let resp = self.client.get(&url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("REST {} for {}: {}", status, url, body.trim());
        }
        Ok(resp.bytes().await?)
    }
}

fn normalize_base(mut base: String) -> String {
    while base.ends_with('/') {
        base.pop();
    }
    base
}

#[async_trait]
impl BlockSource for RestSource {
    async fn get_block_hash(&self, height: u64) -> anyhow::Result<[u8; 32]> {
        let text = self
            .get_text(&format!("/rest/blockhashbyheight/{height}.hex"))
            .await?;
        let trimmed = text.trim();
        if trimmed.len() != 64 || !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
            anyhow::bail!(
                "{}: expected 64-char hex block hash for height {}, got {:?}",
                self.base,
                height,
                trimmed.chars().take(120).collect::<String>()
            );
        }

        let mut out = [0u8; 32];
        hex::decode_to_slice(trimmed, &mut out)?;
        // Bitcoin Core REST returns display-order hashes; internally this
        // project uses consensus/hash byte order to match rust-bitcoin hashes.
        out.reverse();
        Ok(out)
    }

    async fn get_block_raw(&self, hash: [u8; 32]) -> anyhow::Result<Bytes> {
        let mut display = hash;
        display.reverse();
        let hash_hex = hex::encode(display);
        self.get_bytes(&format!("/rest/block/{hash_hex}.bin")).await
    }


    async fn get_block_by_height(&self, height: u64) -> anyhow::Result<btc_data_core::block::RawBlockFrame> {
        let hash = self.get_block_hash(height).await?;
        let mut display = hash;
        display.reverse();
        let hash_hex = hex::encode(display);
        let bytes = self.get_bytes(&format!("/rest/block/{hash_hex}.bin")).await?;
        Ok(btc_data_core::block::RawBlockFrame { height, hash, bytes })
    }

    async fn get_best_height(&self) -> anyhow::Result<u64> {
        let text = self.get_text("/rest/chaininfo.json").await?;
        let info: ChainInfo = serde_json::from_str(&text)?;
        Ok(info.blocks)
    }

    fn name(&self) -> &str {
        &self.base
    }
}
