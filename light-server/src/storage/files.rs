use crate::index::{
    decode_stored_light_block, encode_stored_light_block,
    stored_light_block_to_filtered_response_bytes, stored_light_block_to_response_bytes,
    to_packed_bytes, StoredBlockResponseFilter,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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
    /// Suggested number of recent blocks wallet clients should retain undo/hash
    /// metadata for shallow reorg handling.
    pub suggested_reorg_cache_depth: u64,
    pub max_range_count: u32,
    pub tip: Option<ChainTip>,
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

    pub fn manifest_path(&self) -> PathBuf {
        self.root.join("manifest.json")
    }

    pub fn block_path(&self, height: u64) -> PathBuf {
        self.blocks_dir().join(format!("{height:010}.capnp"))
    }

    pub fn read_stored_block(&self, height: u64) -> anyhow::Result<Vec<u8>> {
        Ok(fs::read(self.block_path(height))?)
    }

    pub fn read_block(&self, height: u64) -> anyhow::Result<Vec<u8>> {
        let stored = self.read_stored_block(height)?;
        let (response, _block_hash) = stored_light_block_to_response_bytes(&stored)?;
        Ok(response)
    }

    pub fn read_block_with_hash(&self, height: u64) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        let stored = self.read_stored_block(height)?;
        let (response, block_hash) = stored_light_block_to_response_bytes(&stored)?;
        Ok((response, block_hash.as_bytes().to_vec()))
    }

    pub fn read_block_filtered_with_hash(
        &self,
        height: u64,
        filter: StoredBlockResponseFilter,
    ) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        let stored = self.read_stored_block(height)?;
        let (response, block_hash) =
            stored_light_block_to_filtered_response_bytes(&stored, filter)?;
        Ok((response, block_hash.as_bytes().to_vec()))
    }

    pub fn read_blocks(&self, start: u64, count: u32) -> anyhow::Result<Vec<Vec<u8>>> {
        (0..count)
            .map(|i| self.read_block(start + u64::from(i)))
            .collect()
    }

    pub fn read_blocks_filtered(
        &self,
        start: u64,
        count: u32,
        filter: StoredBlockResponseFilter,
    ) -> anyhow::Result<Vec<Vec<u8>>> {
        (0..count)
            .map(|i| {
                self.read_block_filtered_with_hash(start + u64::from(i), filter)
                    .map(|(payload, _hash)| payload)
            })
            .collect()
    }

    pub fn write_block_bytes(&self, height: u64, bytes: &[u8]) -> anyhow::Result<PathBuf> {
        fs::create_dir_all(self.blocks_dir())?;
        let path = self.block_path(height);
        let tmp = path.with_extension("capnp.tmp");
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, &path)?;
        Ok(path)
    }

    /// Update storage-only spent heights for outputs that were created in
    /// already-written archive blocks. Missing creation-block files are skipped;
    /// this can happen when the indexer resumes from a height newer than the
    /// archive emit start.
    pub fn mark_spent_outputs(
        &self,
        spends: impl IntoIterator<Item = (u64, u64, u64)>,
    ) -> anyhow::Result<usize> {
        let mut by_creation_height = BTreeMap::<u64, Vec<(u64, u32)>>::new();
        for (creation_height, uid, spent_height) in spends {
            by_creation_height
                .entry(creation_height)
                .or_default()
                .push((uid, u32::try_from(spent_height)?));
        }

        let mut updated = 0usize;
        for (creation_height, spends) in by_creation_height {
            let path = self.block_path(creation_height);
            if !path.exists() {
                continue;
            }

            let stored_bytes = fs::read(&path)?;
            let mut stored = decode_stored_light_block(&stored_bytes)?;
            for (uid, spent_height) in spends {
                let dense_index = dense_output_index_for_uid(&stored, uid).ok_or_else(|| {
                    anyhow::anyhow!(
                        "spent uid {uid} maps outside storage outputs for block {}",
                        creation_height
                    )
                })?;
                let output = stored.outputs.get_mut(dense_index).ok_or_else(|| {
                    anyhow::anyhow!(
                        "spent uid {uid} maps outside storage outputs for block {}",
                        creation_height
                    )
                })?;
                if output.spent_height != spent_height {
                    output.spent_height = spent_height;
                    updated += 1;
                }
            }

            let bytes = to_packed_bytes(&encode_stored_light_block(&stored)?)?;
            self.write_block_bytes(creation_height, &bytes)?;
        }
        Ok(updated)
    }

    pub fn tip(&self) -> anyhow::Result<Option<ChainTip>> {
        let manifest = self.read_manifest().ok();
        if let Some(manifest) = manifest {
            if manifest.tip.is_some() {
                return Ok(manifest.tip);
            }
        }
        self.tip_from_files()
    }

    pub fn tip_from_files(&self) -> anyhow::Result<Option<ChainTip>> {
        let dir = self.blocks_dir();
        if !dir.exists() {
            return Ok(None);
        }
        let mut best = None;
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(prefix) = name.strip_suffix(".capnp") {
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

fn dense_output_index_for_uid(
    stored: &crate::index::StoredLightBlockInput,
    uid: u64,
) -> Option<usize> {
    let flat_index = uid.checked_sub(stored.first_uid)?;
    let flat_index = u16::try_from(flat_index).ok()?;
    let mut skipped_iter = stored.skipped_outputs.iter().copied().peekable();
    let mut current_flat = 0u16;
    let mut dense_index = 0usize;

    while dense_index < stored.outputs.len() {
        while skipped_iter
            .peek()
            .is_some_and(|skipped| *skipped == current_flat)
        {
            skipped_iter.next();
            current_flat = current_flat.checked_add(1)?;
        }
        if current_flat == flat_index {
            return Some(dense_index);
        }
        dense_index += 1;
        current_flat = current_flat.checked_add(1)?;
    }
    None
}

#[async_trait::async_trait]
impl crate::storage::ArchiveBackend for FileArchive {
    async fn manifest(&self) -> anyhow::Result<Manifest> {
        self.read_manifest()
    }

    async fn tip(&self) -> anyhow::Result<Option<ChainTip>> {
        self.tip()
    }

    async fn read_block(&self, height: u64) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        self.read_block_with_hash(height)
    }

    async fn read_block_filtered(
        &self,
        height: u64,
        filter: StoredBlockResponseFilter,
    ) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        self.read_block_filtered_with_hash(height, filter)
    }

    async fn read_blocks(&self, start: u64, count: u32) -> anyhow::Result<Vec<Vec<u8>>> {
        self.read_blocks(start, count)
    }

    async fn read_blocks_filtered(
        &self,
        start: u64,
        count: u32,
        filter: StoredBlockResponseFilter,
    ) -> anyhow::Result<Vec<Vec<u8>>> {
        self.read_blocks_filtered(start, count, filter)
    }
}
