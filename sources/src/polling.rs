//! Generic polling tip watcher for sources without native notifications.
//!
//! IPC sources should implement `TipWatcher` with Core notifications. This
//! fallback is useful for REST/RPC-style sources and for local development.

use async_trait::async_trait;
use btc_data_core::source::{BlockSource, TipWatcher};
use std::sync::Arc;
use std::time::Duration;

pub struct PollingTipWatcher {
    source: Arc<dyn BlockSource>,
    interval: Duration,
    name: String,
}

impl PollingTipWatcher {
    pub fn new(source: Arc<dyn BlockSource>, interval: Duration) -> Self {
        Self {
            source,
            interval,
            name: "polling".to_owned(),
        }
    }

    pub fn with_name(
        source: Arc<dyn BlockSource>,
        interval: Duration,
        name: impl Into<String>,
    ) -> Self {
        Self {
            source,
            interval,
            name: name.into(),
        }
    }
}

#[async_trait]
impl TipWatcher for PollingTipWatcher {
    async fn wait_for_tip_change(&self, old_tip: Option<[u8; 32]>) -> anyhow::Result<()> {
        loop {
            tokio::time::sleep(self.interval).await;

            let height = self.source.get_best_height().await?;
            let hash = self.source.get_block_hash(height).await?;

            if old_tip != Some(hash) {
                return Ok(());
            }
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}
