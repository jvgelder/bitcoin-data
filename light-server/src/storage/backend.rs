use crate::profile::Profile;
use crate::storage::{ChainTip, Manifest};
use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct CutthroughSnapshot {
    pub height: u64,
    pub block_hash: String,
    pub block_count: u32,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct CutthroughDeltaBlocks {
    /// Last height included in `messages`.
    pub end_height: u64,
    /// Consecutive LightBlock messages from known_height + 1 through end_height.
    pub messages: Vec<Vec<u8>>,
}

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

    async fn read_block(
        &self,
        height: u64,
        profile: &ServedProfile,
    ) -> anyhow::Result<(Vec<u8>, Vec<u8>)>;

    async fn read_blocks(
        &self,
        start: u64,
        count: u32,
        profile: &ServedProfile,
    ) -> anyhow::Result<Vec<Vec<u8>>>;

    /// Build a cut-through delta range from canonical SQLite lifecycle tables.
    ///
    /// The delta advances a client that is known to have scanned through
    /// `known_height` to the server-selected cut-through boundary `end_height`:
    /// - include outputs created in `(known_height, end_height]` that are still
    ///   live at `end_height`;
    /// - include spends in `(known_height, end_height]` for outputs created at
    ///   or before `known_height`;
    /// - omit outputs created and spent entirely inside the interval.
    ///
    /// Backends without lifecycle tables may return an unsupported error.
    async fn read_cutthrough_delta_blocks(
        &self,
        known_height: u64,
        max_end_height: u64,
        target_response_bytes: usize,
        profile: &ServedProfile,
    ) -> anyhow::Result<CutthroughDeltaBlocks>;

    /// Return a BDSS-framed cut-through live-output snapshot at `height`.
    /// The snapshot contains outputs created at or below `height` that are
    /// still live at `height`, grouped by original creation block.
    async fn read_cutthrough_snapshot(
        &self,
        height: u64,
        profile: &ServedProfile,
    ) -> anyhow::Result<CutthroughSnapshot>;


    async fn block_stats(&self, height: u64) -> anyhow::Result<serde_json::Value>;
}
