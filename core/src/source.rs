//! `BlockSource` — trait for anything that resolves a height to raw block bytes.
//!
//! Concrete implementations live in `btc-data-sources`.

use crate::block::RawBlockFrame;
use async_trait::async_trait;
use bytes::Bytes;

#[async_trait]
pub trait BlockSource: Send + Sync {
    async fn get_block_hash(&self, height: u64) -> anyhow::Result<[u8; 32]>;
    async fn get_block_raw(&self, hash: [u8; 32]) -> anyhow::Result<Bytes>;

    /// Current best chain height known by this source.
    ///
    /// Required by finalized scans that intentionally stay N blocks behind
    /// the tip to avoid most reorg handling in analytical consumers.
    async fn get_best_height(&self) -> anyhow::Result<u64>;

    /// Fetch one block frame by height.
    ///
    /// Implementations can override this to avoid a separate height->hash
    /// lookup. The default implementation preserves existing behavior by
    /// resolving the hash first and then fetching the raw block by hash.
    async fn get_block_by_height(&self, height: u64) -> anyhow::Result<RawBlockFrame> {
        let hash = self.get_block_hash(height).await?;
        let bytes = self.get_block_raw(hash).await?;
        Ok(RawBlockFrame {
            height,
            hash,
            bytes,
        })
    }

    /// Fetch an ordered contiguous range of block frames by height.
    ///
    /// Sources can override this to use native batching. The default fallback
    /// calls `get_block_by_height` one height at a time and returns ordered
    /// frames.
    async fn get_block_range_by_height(
        &self,
        start_height: u64,
        count: usize,
    ) -> anyhow::Result<Vec<RawBlockFrame>> {
        let mut frames = Vec::with_capacity(count);
        for offset in 0..count {
            frames.push(
                self.get_block_by_height(start_height + offset as u64)
                    .await?,
            );
        }
        Ok(frames)
    }

    /// True when `get_block_range_by_height` is backed by a real source-native
    /// batch operation and should be used by the pipeline for `source_batch_size > 1`.
    ///
    /// The default range fallback is intentionally not considered a batch, because
    /// grouping non-batch sources can reduce concurrency and delay ordered output.
    fn supports_block_range_batches(&self) -> bool {
        false
    }

    /// True when this source can fetch hash + raw block data efficiently in one
    /// height-based call. IPC uses this to avoid an extra height->hash prefetch phase.
    fn prefers_height_fetch(&self) -> bool {
        false
    }

    fn name(&self) -> &str;
}

/// Wake-up source for long-running indexers.
///
/// `BlockSource` answers "what is the current chain state?" and fetches blocks.
/// `TipWatcher` answers "when should I check again?". IPC implementations can
/// use Core's chain notification API, while fallback implementations may poll.
#[async_trait]
pub trait TipWatcher: Send + Sync {
    /// Wait until the chain tip may have changed.
    ///
    /// `old_tip` is the caller's current best-tip hash in internal byte order.
    /// Implementations may use it to avoid returning immediately when no tip
    /// change has happened yet.
    async fn wait_for_tip_change(&self, old_tip: Option<[u8; 32]>) -> anyhow::Result<()>;

    fn name(&self) -> &str;
}
