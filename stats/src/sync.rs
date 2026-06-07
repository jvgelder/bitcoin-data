//! Durable stats-store synchronization contracts.
//!
//! CSV is an append-oriented sink. SQLite and DuckDB are better modeled as
//! durable stores keyed by `(height, block_hash, stats_version)`, so they can
//! skip already-synced block-local rows and replace rows deterministically.
//!
//! Concrete SQLite/DuckDB implementations are intentionally left out of the
//! default build until their dependencies and schemas settle.

use crate::BlockStats;
use async_trait::async_trait;

/// Store abstraction for idempotent per-block stats persistence.
#[async_trait]
pub trait DurableStatsStore: Send + Sync {
    /// True when this exact block/stat version is already present.
    async fn has_block(
        &self,
        height: u64,
        block_hash: &str,
        stats_version: u32,
    ) -> anyhow::Result<bool>;

    /// Insert or replace one block row. Implementations should key on
    /// `(height, block_hash, stats_version)`.
    async fn upsert_block(&self, row: &BlockStats) -> anyhow::Result<()>;

    /// Remove rows above a restored checkpoint after a reorg/recovery.
    async fn rollback_to_height(&self, height: u64) -> anyhow::Result<()>;

    /// Flush pending batches, if the backend buffers writes.
    async fn flush(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Planned SQLite backend.
pub struct SqliteStatsStore;

/// Planned DuckDB backend.
pub struct DuckDbStatsStore;
