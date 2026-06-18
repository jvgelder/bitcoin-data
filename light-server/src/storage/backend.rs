use crate::index::StoredBlockResponseFilter;
use crate::storage::{ChainTip, Manifest};
use async_trait::async_trait;

#[async_trait]
pub trait ArchiveBackend: Send + Sync {
    async fn manifest(&self) -> anyhow::Result<Manifest>;

    async fn tip(&self) -> anyhow::Result<Option<ChainTip>>;

    async fn read_block(&self, height: u64) -> anyhow::Result<(Vec<u8>, Vec<u8>)>;

    async fn read_block_filtered(
        &self,
        height: u64,
        filter: StoredBlockResponseFilter,
    ) -> anyhow::Result<(Vec<u8>, Vec<u8>)>;

    async fn read_blocks(&self, start: u64, count: u32) -> anyhow::Result<Vec<Vec<u8>>>;

    async fn read_blocks_filtered(
        &self,
        start: u64,
        count: u32,
        filter: StoredBlockResponseFilter,
    ) -> anyhow::Result<Vec<Vec<u8>>>;
}
