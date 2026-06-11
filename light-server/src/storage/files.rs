use crate::profile::{ArchiveScope, Profile};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct FileArchive {
    root: PathBuf,
}

/// Backwards-compatible alias for older fixture/indexer code.
pub type LightArchive = FileArchive;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainTip {
    pub height: u64,
    pub block_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u16,
    pub network: String,
    pub genesis_hash: Option<String>,
    /// Depth after which server responses can be treated as practically immutable
    /// for caching. Payloads closer to tip remain replaceable on reorg.
    pub finality_depth: u64,
    /// Suggested number of recent blocks wallet clients should sync from the full
    /// endpoint and retain undo metadata for. This is not a requirement to cache
    /// full payload bytes.
    pub suggested_reorg_cache_depth: u64,
    pub max_range_count: u32,
    pub profiles: Vec<ManifestProfile>,
    #[serde(default)]
    pub cutthrough_snapshots: Vec<ManifestCutthroughSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestProfile {
    pub scope: ArchiveScope,
    pub cutthrough_blocks: u32,
    pub tip: Option<ChainTip>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestCutthroughSnapshot {
    pub scope: ArchiveScope,
    pub cutthrough_blocks: u32,
    pub height: u64,
    pub block_hash: String,
    pub latest_endpoint: String,
    pub endpoint: String,
}

impl FileArchive {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn blocks_dir(&self) -> PathBuf {
        self.root.join("blocks")
    }
    pub fn stats_dir(&self) -> PathBuf {
        self.root.join("block_stats")
    }
    pub fn manifest_path(&self) -> PathBuf {
        self.root.join("manifest.json")
    }

    pub fn block_path(&self, height: u64, profile: Profile) -> PathBuf {
        self.blocks_dir()
            .join(format!("{height:010}.{}.capnp", profile.file_tag()))
    }


    pub fn block_stats_path(&self, height: u64) -> PathBuf {
        self.stats_dir().join(format!("{height:010}.stats.json"))
    }

    pub fn read_block(&self, height: u64, profile: Profile) -> anyhow::Result<Vec<u8>> {
        let path = self.block_path(height, profile);
        Ok(fs::read(path)?)
    }

    pub fn read_blocks(
        &self,
        start: u64,
        count: u32,
        profile: Profile,
    ) -> anyhow::Result<Vec<Vec<u8>>> {
        (0..count)
            .map(|i| self.read_block(start + u64::from(i), profile))
            .collect()
    }

    pub fn write_block_bytes(
        &self,
        height: u64,
        profile: Profile,
        bytes: &[u8],
    ) -> anyhow::Result<PathBuf> {
        fs::create_dir_all(self.blocks_dir())?;
        let path = self.block_path(height, profile);
        fs::write(&path, bytes)?;
        Ok(path)
    }


    pub fn tip(&self, profile: Profile) -> anyhow::Result<Option<ChainTip>> {
        let manifest = self.read_manifest().ok();
        if let Some(manifest) = manifest {
            for p in manifest.profiles {
                if p.scope == profile.scope
                    && p.cutthrough_blocks == profile.cutthrough_blocks
                    && p.tip.is_some()
                {
                    return Ok(p.tip);
                }
            }
        }
        self.tip_from_files(profile)
    }

    pub fn tip_from_files(&self, profile: Profile) -> anyhow::Result<Option<ChainTip>> {
        let dir = self.blocks_dir();
        if !dir.exists() {
            return Ok(None);
        }
        let suffix = format!(".{}.capnp", profile.file_tag());
        let mut best = None;
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(prefix) = name.strip_suffix(&suffix) {
                if let Ok(height) = prefix.parse::<u64>() {
                    if best.is_none_or(|b| height > b) {
                        best = Some(height);
                    }
                }
            }
        }
        Ok(best.map(|height| ChainTip {
            height,
            block_hash: String::new(),
        }))
    }

    pub fn write_block_stats<T: Serialize>(
        &self,
        height: u64,
        stats: &T,
    ) -> anyhow::Result<PathBuf> {
        fs::create_dir_all(self.stats_dir())?;
        let path = self.block_stats_path(height);
        fs::write(&path, serde_json::to_vec_pretty(stats)?)?;
        Ok(path)
    }

    pub fn read_block_stats(&self, height: u64) -> anyhow::Result<serde_json::Value> {
        let bytes = fs::read(self.block_stats_path(height))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn read_manifest(&self) -> anyhow::Result<Manifest> {
        let bytes = fs::read(self.manifest_path())?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn write_manifest(&self, manifest: &Manifest) -> anyhow::Result<()> {
        fs::create_dir_all(&self.root)?;
        let bytes = serde_json::to_vec_pretty(manifest)?;
        fs::write(self.manifest_path(), bytes)?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl crate::storage::ArchiveBackend for FileArchive {
    async fn manifest(&self) -> anyhow::Result<Manifest> {
        self.read_manifest()
    }

    async fn resolve_profile(
        &self,
        name: Option<&str>,
        profile: Option<Profile>,
    ) -> anyhow::Result<crate::storage::ServedProfile> {
        let profile = if let Some(profile) = profile {
            profile
        } else if let Some(name) = name {
            parse_profile_name(name)?
        } else {
            Profile::default()
        };
        let tip = self.tip(profile)?;
        let name = if profile.cutthrough_blocks == 0 {
            format!("{}-raw", profile.scope.as_str())
        } else {
            format!("{}-ct{}", profile.scope.as_str(), profile.cutthrough_blocks)
        };
        Ok(crate::storage::ServedProfile {
            profile_id: 0,
            name,
            profile,
            materialization_interval_blocks: if profile.cutthrough_blocks == 0 {
                1
            } else {
                144
            },
            served_tip: tip,
        })
    }

    async fn tip(
        &self,
        profile: &crate::storage::ServedProfile,
    ) -> anyhow::Result<Option<ChainTip>> {
        self.tip(profile.profile)
    }

    async fn read_block(
        &self,
        height: u64,
        profile: &crate::storage::ServedProfile,
    ) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        Ok((self.read_block(height, profile.profile)?, Vec::new()))
    }

    async fn read_blocks(
        &self,
        start: u64,
        count: u32,
        profile: &crate::storage::ServedProfile,
    ) -> anyhow::Result<Vec<Vec<u8>>> {
        self.read_blocks(start, count, profile.profile)
    }

    async fn read_cutthrough_delta_blocks(
        &self,
        _known_height: u64,
        _max_end_height: u64,
        _target_response_bytes: usize,
        _profile: &crate::storage::ServedProfile,
    ) -> anyhow::Result<crate::storage::CutthroughDeltaBlocks> {
        anyhow::bail!("file-backed archives do not support SQLite cut-through delta ranges")
    }

    async fn read_cutthrough_snapshot(
        &self,
        _height: u64,
        _profile: &crate::storage::ServedProfile,
    ) -> anyhow::Result<crate::storage::CutthroughSnapshot> {
        anyhow::bail!("file-backed archives do not support cut-through snapshots")
    }


    async fn block_stats(&self, height: u64) -> anyhow::Result<serde_json::Value> {
        self.read_block_stats(height)
    }
}

fn parse_profile_name(name: &str) -> anyhow::Result<Profile> {
    if name == "raw-sp" || name == "p2tr-sp-raw" {
        return Ok(Profile {
            scope: ArchiveScope::P2trSp,
            cutthrough_blocks: 0,
        });
    }
    if let Some(rest) = name.strip_prefix("ct").and_then(|s| s.strip_suffix("-sp")) {
        return Ok(Profile {
            scope: ArchiveScope::P2trSp,
            cutthrough_blocks: rest.parse()?,
        });
    }
    anyhow::bail!("file backend cannot resolve profile name {name:?}; use scope+ct instead")
}
