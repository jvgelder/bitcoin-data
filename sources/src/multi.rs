//! Round-robin over multiple sources.
//!
//! On error from one source, falls through to the next. Distributes load
//! across sources for higher aggregate throughput against rate-limited
//! public endpoints.

use async_trait::async_trait;
use btc_data_core::block::{BlockSpentTxOuts, RawBlockFrame};
use btc_data_core::source::BlockSource;
use bytes::Bytes;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct MultiSource {
    sources: Vec<Box<dyn BlockSource>>,
    cursor: AtomicUsize,
}

impl MultiSource {
    pub fn new(sources: Vec<Box<dyn BlockSource>>) -> Self {
        assert!(!sources.is_empty(), "need at least one source");
        Self {
            sources,
            cursor: AtomicUsize::new(0),
        }
    }

    fn pick(&self) -> &dyn BlockSource {
        let i = self.cursor.fetch_add(1, Ordering::Relaxed) % self.sources.len();
        &*self.sources[i]
    }
}

#[async_trait]
impl BlockSource for MultiSource {
    async fn get_block_hash(&self, height: u64) -> anyhow::Result<[u8; 32]> {
        let n = self.sources.len();
        let mut last_err = None;
        for _ in 0..n {
            let src = self.pick();
            match src.get_block_hash(height).await {
                Ok(h) => return Ok(h),
                Err(e) => {
                    eprintln!("[{}] hash {height}: {e}", src.name());
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap())
    }

    async fn get_block_raw(&self, hash: [u8; 32]) -> anyhow::Result<Bytes> {
        let n = self.sources.len();
        let mut last_err = None;
        for _ in 0..n {
            let src = self.pick();
            match src.get_block_raw(hash).await {
                Ok(b) => return Ok(b),
                Err(e) => {
                    eprintln!("[{}] block fetch: {e}", src.name());
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap())
    }


    async fn get_block_spent_txouts(
        &self,
        hash: [u8; 32],
    ) -> anyhow::Result<Option<BlockSpentTxOuts>> {
        let n = self.sources.len();
        let mut last_err = None;
        for _ in 0..n {
            let src = self.pick();
            match src.get_block_spent_txouts(hash).await {
                Ok(undo) => return Ok(undo),
                Err(e) => {
                    eprintln!("[{}] spenttxouts fetch: {e}", src.name());
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap())
    }

    async fn get_blocks_spent_txouts(
        &self,
        hashes: &[[u8; 32]],
    ) -> anyhow::Result<Vec<Option<BlockSpentTxOuts>>> {
        let n = self.sources.len();
        let mut last_err = None;
        for _ in 0..n {
            let src = self.pick();
            match src.get_blocks_spent_txouts(hashes).await {
                Ok(undo) => return Ok(undo),
                Err(e) => {
                    eprintln!("[{}] spenttxouts batch fetch: {e}", src.name());
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap())
    }

    async fn get_best_height(&self) -> anyhow::Result<u64> {
        let n = self.sources.len();
        let mut last_err = None;
        for _ in 0..n {
            let src = self.pick();
            match src.get_best_height().await {
                Ok(h) => return Ok(h),
                Err(e) => {
                    eprintln!("[{}] best height: {e}", src.name());
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap())
    }

    async fn get_block_by_height(&self, height: u64) -> anyhow::Result<RawBlockFrame> {
        let n = self.sources.len();
        let mut last_err = None;
        for _ in 0..n {
            let src = self.pick();
            match src.get_block_by_height(height).await {
                Ok(frame) => return Ok(frame),
                Err(e) => {
                    eprintln!("[{}] block height {height}: {e}", src.name());
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap())
    }

    async fn get_block_range_by_height(
        &self,
        start_height: u64,
        count: usize,
    ) -> anyhow::Result<Vec<RawBlockFrame>> {
        let n = self.sources.len();
        let mut last_err = None;
        for _ in 0..n {
            let src = self.pick();
            match src.get_block_range_by_height(start_height, count).await {
                Ok(frames) => return Ok(frames),
                Err(e) => {
                    eprintln!(
                        "[{}] block range {}..{}: {e}",
                        src.name(),
                        start_height,
                        start_height + count.saturating_sub(1) as u64
                    );
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap())
    }

    fn supports_block_range_batches(&self) -> bool {
        self.sources
            .iter()
            .all(|source| source.supports_block_range_batches())
    }

    fn supports_block_spent_txouts(&self) -> bool {
        self.sources
            .iter()
            .all(|source| source.supports_block_spent_txouts())
    }

    fn prefers_height_fetch(&self) -> bool {
        self.sources
            .iter()
            .all(|source| source.prefers_height_fetch())
    }

    fn name(&self) -> &str {
        "multi"
    }
}
