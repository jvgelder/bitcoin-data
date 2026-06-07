//! Stats-specific sinks.
//!
//! Distinct from the top-level `sinks/` (which handle factual block events).
//! Stats sinks consume `BlockStats` and `Stats` only — no Block/Tx/Tip
//! payloads. Each impl is wire-format-specific (CSV here; future: kafka,
//! prometheus, influx).

pub mod csv;
pub mod fanout;

use crate::{BlockStats, Stats};
use async_trait::async_trait;
use std::sync::Arc;

pub use csv::StatsCsvSink;
pub use fanout::Fanout;

#[async_trait]
pub trait StatsSink: Send + Sync {
    async fn emit_block(&self, row: &BlockStats) -> anyhow::Result<()> {
        let _ = row;
        Ok(())
    }
    async fn emit_run(&self, stats: &Stats) -> anyhow::Result<()> {
        let _ = stats;
        Ok(())
    }
    async fn rollback_to_height(&self, height: u64) -> anyhow::Result<()> {
        let _ = height;
        Ok(())
    }
    async fn flush(&self) -> anyhow::Result<()> {
        Ok(())
    }
    fn name(&self) -> &str;
}

pub type DynStatsSink = Arc<dyn StatsSink>;
