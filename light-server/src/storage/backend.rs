use crate::storage::{ChainTip, Manifest};
use crate::profile::Profile;
use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct ServedProfile {
    /// Backend-local identifier. SQLite uses the profiles.profile_id value;
    /// file-backed archives may use a synthetic id.
    pub profile_id: i64,
    pub name: String,
    pub profile: Profile,
    pub materialization_interval_blocks: u32,
    pub served_tip: Option<ChainTip>,
}

#[async_trait]
pub trait ArchiveBackend: Send + Sync {
    async fn manifest(&self) -> anyhow::Result<Manifest>;

    async fn resolve_profile(
        &self,
        name: Option<&str>,
        profile: Option<Profile>,
    ) -> anyhow::Result<ServedProfile>;

    async fn tip(&self, profile: &ServedProfile) -> anyhow::Result<Option<ChainTip>>;

    async fn read_block(&self, height: u64, profile: &ServedProfile) -> anyhow::Result<(Vec<u8>, Vec<u8>)>;

    async fn read_blocks(&self, start: u64, count: u32, profile: &ServedProfile) -> anyhow::Result<Vec<Vec<u8>>>;

    async fn read_checkpoint(&self, height: u64, profile: &ServedProfile) -> anyhow::Result<Vec<u8>>;

    async fn latest_checkpoint_height(&self, height_lte: u64, profile: &ServedProfile) -> anyhow::Result<Option<u64>>;

    async fn block_stats(&self, height: u64) -> anyhow::Result<serde_json::Value>;
}
