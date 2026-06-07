//! Restart recovery for the stats scanner.
//!
//! Checkpoints are postcard-encoded snapshots of the state needed by
//! chain-derived stats. They are named with both height and block hash because
//! height alone is not stable across a reorg.

use crate::scan::StatsScannerState;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const CHECKPOINT_FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug)]
pub struct CheckpointConfig {
    pub dir: PathBuf,
    pub every: u64,
    pub keep: usize,
    pub enabled: bool,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::from(".btc-data-stats/checkpoints"),
            every: 1,
            keep: 10,
            enabled: false,
        }
    }
}

/// Writes checkpoints after a configured number of committed blocks.
///
/// A checkpoint is only written after the caller has fully processed a block
/// and all configured sinks have committed that block. Therefore a checkpoint
/// at height H means scanner state and sink output are committed through H,
/// and resume should start at H + 1.
#[derive(Debug)]
pub struct CheckpointWriter {
    cfg: CheckpointConfig,
    processed_since_checkpoint: u64,
}

impl CheckpointWriter {
    pub fn new(cfg: CheckpointConfig) -> Self {
        Self {
            cfg,
            processed_since_checkpoint: 0,
        }
    }

    pub fn enabled(&self) -> bool {
        self.cfg.enabled && self.cfg.every > 0
    }

    /// Records that one more block has been fully committed.
    ///
    /// Returns true exactly when the caller should build and write a full
    /// checkpoint. This method intentionally does not accept or store a
    /// `StatsCheckpoint`, because that would clone the full scanner state even
    /// when no checkpoint is due.
    pub fn on_committed_block(&mut self) -> bool {
        if !self.enabled() {
            return false;
        }

        self.processed_since_checkpoint += 1;

        if self.processed_since_checkpoint >= self.cfg.every {
            self.processed_since_checkpoint = 0;
            true
        } else {
            false
        }
    }

    pub fn needs_final_flush(&self) -> bool {
        self.enabled() && self.processed_since_checkpoint > 0
    }

    pub async fn write_checkpoint(&self, checkpoint: &StatsCheckpoint) -> anyhow::Result<()> {
        if !self.enabled() {
            return Ok(());
        }

        save_checkpoint(&self.cfg.dir, checkpoint).await?;
        prune_checkpoints(&self.cfg.dir, self.cfg.keep).await?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsCheckpoint {
    pub checkpoint_format_version: u32,
    pub stats_state_version: u32,

    pub start_height: u64,
    pub next_height: u64,
    pub height: u64,
    pub block_hash: [u8; 32],

    pub utxo_hash_window: u64,
    pub finality_depth: u64,

    pub scanner_state: StatsScannerState,
}

pub fn block_hash_hex(hash: &[u8; 32]) -> String {
    let mut display = *hash;
    display.reverse();
    hex::encode(display)
}

pub fn checkpoint_path(dir: &Path, height: u64, hash: &[u8; 32]) -> PathBuf {
    dir.join(format!(
        "checkpoint-{height:012}-{}.postcard",
        block_hash_hex(hash)
    ))
}

pub async fn save_checkpoint(dir: &Path, checkpoint: &StatsCheckpoint) -> anyhow::Result<PathBuf> {
    tokio::fs::create_dir_all(dir).await?;
    let path = checkpoint_path(dir, checkpoint.height, &checkpoint.block_hash);
    let tmp = path.with_extension("postcard.tmp");
    let bytes = postcard::to_allocvec(checkpoint)?;
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, &path).await?;
    Ok(path)
}

pub async fn load_checkpoint(path: &Path) -> anyhow::Result<StatsCheckpoint> {
    let bytes = tokio::fs::read(path).await?;
    Ok(postcard::from_bytes(&bytes)?)
}

pub async fn list_checkpoints_newest_first(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut rd = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e.into()),
    };
    while let Some(ent) = rd.next_entry().await? {
        let p = ent.path();
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.starts_with("checkpoint-") && name.ends_with(".postcard") {
            out.push(p);
        }
    }
    out.sort();
    out.reverse();
    Ok(out)
}

pub async fn prune_checkpoints(dir: &Path, keep: usize) -> anyhow::Result<()> {
    if keep == 0 {
        return Ok(());
    }
    let mut checkpoints = list_checkpoints_newest_first(dir).await?;
    if checkpoints.len() <= keep {
        return Ok(());
    }
    checkpoints.reverse();
    let remove_count = checkpoints.len() - keep;
    for p in checkpoints.into_iter().take(remove_count) {
        tokio::fs::remove_file(p).await?;
    }
    Ok(())
}
