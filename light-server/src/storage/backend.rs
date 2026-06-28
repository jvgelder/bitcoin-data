use crate::index::StoredBlockResponseFilter;
use crate::storage::{ChainTip, Manifest};
use crate::types::BlockHashBytes;
use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct ServedBlock {
    pub payload: Vec<u8>,
    pub block_hash: BlockHashBytes,
}

#[async_trait]
pub trait ArchiveBackend: Send + Sync {
    async fn manifest(&self) -> anyhow::Result<Manifest>;

    async fn tip(&self) -> anyhow::Result<Option<ChainTip>>;

    async fn read_block(&self, height: u64) -> anyhow::Result<ServedBlock>;

    async fn read_block_filtered(
        &self,
        height: u64,
        filter: StoredBlockResponseFilter,
    ) -> anyhow::Result<ServedBlock>;
}
