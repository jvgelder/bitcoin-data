//! Esplora-compatible HTTP source (mempool.space, blockstream.info, etc.).
//!
//! Public Esplora endpoints rate-limit aggressively. This source supports:
//! - Per-source min-gap throttle.
//! - Source-wide dormancy on 429/5xx (all in-flight tasks back off together).
//! - `Retry-After` header honored when present.

use async_trait::async_trait;
use btc_data_core::source::BlockSource;
use bytes::Bytes;

pub struct EsploraSource {
    base: String,
    client: reqwest::Client,
    /// Min gap between outgoing requests (ms). 0 = disabled.
    min_gap_ms: u64,
    /// Timestamp of last completed request (ms since UNIX epoch).
    last_req: tokio::sync::Mutex<u64>,
    /// Source-wide dormant-until timestamp (ms). All requests wait past this.
    /// Set when we hit 429 or 5xx — applies to new requests AND retries.
    dormant_until: std::sync::atomic::AtomicU64,
}

impl EsploraSource {
    /// `base` like "https://mempool.space/api" or "https://blockstream.info/api".
    pub fn new(base: impl Into<String>) -> Self {
        Self::with_rate_limit(base, 0)
    }

    /// `min_gap_ms` enforces at least that many ms between requests to this source.
    /// mempool.space: ~200ms is safe; blockstream.info: ~1000ms recommended.
    pub fn with_rate_limit(base: impl Into<String>, min_gap_ms: u64) -> Self {
        Self {
            base: base.into(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .pool_max_idle_per_host(32)
                .user_agent("btctxostats/0.1")
                .build()
                .expect("http client"),
            min_gap_ms,
            last_req: tokio::sync::Mutex::new(0),
            dormant_until: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    /// Wait until `dormant_until` has passed AND the min-gap has elapsed.
    async fn throttle(&self) {
        loop {
            let dormant = self
                .dormant_until
                .load(std::sync::atomic::Ordering::Relaxed);
            let now = Self::now_ms();
            if now >= dormant {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(dormant - now)).await;
        }

        if self.min_gap_ms == 0 {
            return;
        }
        let mut last = self.last_req.lock().await;
        let now = Self::now_ms();
        let elapsed = now.saturating_sub(*last);
        if elapsed < self.min_gap_ms {
            tokio::time::sleep(std::time::Duration::from_millis(self.min_gap_ms - elapsed)).await;
        }
        *last = Self::now_ms();
    }

    /// Push the source's dormant-until forward (never backward).
    fn extend_dormancy(&self, ms_from_now: u64) {
        let target = Self::now_ms() + ms_from_now;
        self.dormant_until
            .fetch_max(target, std::sync::atomic::Ordering::Relaxed);
    }

    /// Parse Retry-After header (seconds integer; HTTP-date form not supported).
    fn parse_retry_after(resp: &reqwest::Response) -> Option<u64> {
        let v = resp.headers().get(reqwest::header::RETRY_AFTER)?;
        let s = v.to_str().ok()?.trim();
        s.parse::<u64>().ok().map(|secs| secs * 1000)
    }

    /// GET with retry + exponential backoff on 429/5xx and connection errors.
    async fn get_with_retry(&self, url: &str) -> anyhow::Result<reqwest::Response> {
        let mut delay_ms: u64 = 2_000;
        for attempt in 0..8 {
            self.throttle().await;
            match self.client.get(url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        return Ok(resp);
                    }
                    if status.as_u16() == 429 || status.is_server_error() {
                        let wait_ms = Self::parse_retry_after(&resp).unwrap_or(delay_ms);
                        eprintln!(
                            "[{}] {} → {} (attempt {}, dormant {}ms)",
                            self.base,
                            url,
                            status,
                            attempt + 1,
                            wait_ms
                        );
                        self.extend_dormancy(wait_ms);
                        tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
                        delay_ms = (delay_ms * 2).min(60_000);
                        continue;
                    }
                    anyhow::bail!("HTTP {} for {}", status, url);
                }
                Err(e) => {
                    eprintln!(
                        "[{}] {} → {} (attempt {}, backoff {}ms)",
                        self.base,
                        url,
                        e,
                        attempt + 1,
                        delay_ms
                    );
                    self.extend_dormancy(delay_ms);
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    delay_ms = (delay_ms * 2).min(60_000);
                }
            }
        }
        anyhow::bail!("giving up on {} after retries", url)
    }
}

#[async_trait]
impl BlockSource for EsploraSource {
    async fn get_block_hash(&self, height: u64) -> anyhow::Result<[u8; 32]> {
        let url = format!("{}/block-height/{height}", self.base);
        let resp = self.get_with_retry(&url).await?;
        let text = resp.text().await?;
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
        out.reverse();
        Ok(out)
    }

    async fn get_block_raw(&self, hash: [u8; 32]) -> anyhow::Result<Bytes> {
        let mut display = hash;
        display.reverse();
        let hash_hex = hex::encode(display);
        let url = format!("{}/block/{hash_hex}/raw", self.base);
        let resp = self.get_with_retry(&url).await?;
        Ok(resp.bytes().await?)
    }

    async fn get_best_height(&self) -> anyhow::Result<u64> {
        let url = format!("{}/blocks/tip/height", self.base);
        let resp = self.get_with_retry(&url).await?;
        let text = resp.text().await?;
        Ok(text.trim().parse::<u64>()?)
    }

    fn name(&self) -> &str {
        &self.base
    }
}
