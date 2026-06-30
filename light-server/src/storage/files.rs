use crate::index::{
    decode_stored_light_block, encode_stored_light_block,
    stored_light_block_to_filtered_response_bytes, stored_light_block_to_response_bytes,
    to_packed_bytes, StoredBlockResponseFilter,
};
use crate::storage::backend::ServedBlock;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct FileArchive {
    root: PathBuf,
}

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

    pub fn read_block_with_hash(&self, height: u64) -> anyhow::Result<ServedBlock> {
        let stored = self.read_stored_block(height)?;
        let (payload, block_hash) = stored_light_block_to_response_bytes(&stored)?;
        Ok(ServedBlock {
            payload,
            block_hash,
        })
    }

    pub fn read_block_filtered_with_hash(
        &self,
        height: u64,
        filter: StoredBlockResponseFilter,
    ) -> anyhow::Result<ServedBlock> {
        let stored = self.read_stored_block(height)?;
        let (payload, block_hash) = stored_light_block_to_filtered_response_bytes(&stored, filter)?;
        Ok(ServedBlock {
            payload,
            block_hash,
        })
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
        spends: impl IntoIterator<Item = (u64, u32, u64)>,
    ) -> anyhow::Result<usize> {
        let mut by_creation_height = BTreeMap::<u64, Vec<(u32, u32)>>::new();
        for (creation_height, output_index, spent_height) in spends {
            by_creation_height
                .entry(creation_height)
                .or_default()
                .push((output_index, u32::try_from(spent_height)?));
        }

        let mut updated = 0usize;
        for (creation_height, spends) in by_creation_height {
            let path = self.block_path(creation_height);
            if !path.exists() {
                continue;
            }

            let stored_bytes = fs::read(&path)
                .with_context(|| format!("failed to read stored block {}", path.display()))?;
            let mut stored = decode_stored_light_block(&stored_bytes)
                .with_context(|| format!("failed to decode stored block {}", path.display()))?;
            for (output_index, spent_height) in spends {
                let output = stored.outputs.get_mut(usize::try_from(output_index)?).ok_or_else(|| {
                    anyhow::anyhow!(
                        "storage output index {output_index} maps outside storage outputs for block {}",
                        creation_height
                    )
                })?;
                if output.spent_height != spent_height {
                    output.spent_height = spent_height;
                    updated += 1;
                }
            }

            let bytes = to_packed_bytes(&encode_stored_light_block(&stored)?)
                .with_context(|| format!("failed to re-encode stored block {}", path.display()))?;
            self.write_block_bytes(creation_height, &bytes)
                .with_context(|| format!("failed to rewrite stored block {}", path.display()))?;
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

#[async_trait::async_trait]
impl crate::storage::ArchiveBackend for FileArchive {
    async fn manifest(&self) -> anyhow::Result<Manifest> {
        self.read_manifest()
    }

    async fn tip(&self) -> anyhow::Result<Option<ChainTip>> {
        FileArchive::tip(self)
    }

    async fn read_block(&self, height: u64) -> anyhow::Result<ServedBlock> {
        self.read_block_with_hash(height)
    }

    async fn read_block_filtered(
        &self,
        height: u64,
        filter: StoredBlockResponseFilter,
    ) -> anyhow::Result<ServedBlock> {
        self.read_block_filtered_with_hash(height, filter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{
        build_stored_truncated_output_hashes, decode_stored_light_block,
        empty_skipped_txs_for_tweaks_bitmap, encode_stored_light_block, to_packed_bytes,
        StoredLightBlockInput, StoredOutputEntryInput, StoredSpendEntryInput, TweakEntryInput,
        RESPONSE_LABEL_BUDGET_HUNDRED, RESPONSE_LABEL_BUDGET_TWO, STORAGE_SPENT_HEIGHT_UNSPENT,
    };
    use crate::types::{BlockHashBytes, TxTweak};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_archive() -> anyhow::Result<(FileArchive, PathBuf)> {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!("btc-data-light-archive-test-{unique}"));
        fs::create_dir_all(&root)?;
        Ok((FileArchive::new(&root), root))
    }

    fn stored_block(height: u64) -> StoredLightBlockInput {
        let mut stored = StoredLightBlockInput {
            height,
            block_hash: BlockHashBytes::from([1u8; 32]),
            previous_block_hash: BlockHashBytes::from([2u8; 32]),
            first_uid: 10,
            tx_count: 1,
            skipped_txs_for_tweaks: empty_skipped_txs_for_tweaks_bitmap(1).unwrap(),
            tweaks: vec![TweakEntryInput {
                output_count: 2,
                tweak: TxTweak::from([9u8; 33]),
            }],
            skipped_outputs: vec![],
            outputs: vec![
                StoredOutputEntryInput {
                    key: [3u8; 32],
                    spent_height: STORAGE_SPENT_HEIGHT_UNSPENT,
                    flags: 0,
                },
                StoredOutputEntryInput {
                    key: [4u8; 32],
                    spent_height: STORAGE_SPENT_HEIGHT_UNSPENT,
                    flags: 0,
                },
            ],
            spends: vec![StoredSpendEntryInput {
                spent_uid: 10,
                creation_height: height as u32,
            }],
            raw_block_bytes: 2_500_000,
            truncated_output_hash_for_two_labels: Vec::new(),
            truncated_output_hash_for_hundred_labels: Vec::new(),
        };
        stored.truncated_output_hash_for_two_labels = build_stored_truncated_output_hashes(
            stored.raw_block_bytes,
            &stored.outputs,
            RESPONSE_LABEL_BUDGET_TWO,
        )
        .unwrap();
        stored.truncated_output_hash_for_hundred_labels = build_stored_truncated_output_hashes(
            stored.raw_block_bytes,
            &stored.outputs,
            RESPONSE_LABEL_BUDGET_HUNDRED,
        )
        .unwrap();
        stored
    }

    #[test]
    fn mark_spent_outputs_updates_creation_block_once_per_call() -> anyhow::Result<()> {
        let (archive, root) = temp_archive()?;
        let stored = stored_block(100);
        let bytes = to_packed_bytes(&encode_stored_light_block(&stored)?)?;
        archive.write_block_bytes(100, &bytes)?;

        let updated = archive.mark_spent_outputs([(100, 1, 250)])?;
        assert_eq!(updated, 1);

        let stored = decode_stored_light_block(&archive.read_stored_block(100)?)?;
        assert_eq!(stored.outputs[0].spent_height, STORAGE_SPENT_HEIGHT_UNSPENT);
        assert_eq!(stored.outputs[1].spent_height, 250);

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn mark_spent_outputs_skips_missing_creation_blocks() -> anyhow::Result<()> {
        let (archive, root) = temp_archive()?;
        let updated = archive.mark_spent_outputs([(999, 0, 250)])?;
        assert_eq!(updated, 0);
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
