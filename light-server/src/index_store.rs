//! RocksDB-backed indexer working state.
//!
//! Served block bytes live in the file archive. RocksDB only keeps the small
//! mutable indexes needed while deriving later blocks from Bitcoin prevouts.

use crate::p2tr_indexer::{OutPointKey, SpendLookup};
use crate::types::BlockHashBytes;
use rocksdb::{Options, WriteBatch, DB};
use std::path::Path;
use std::sync::Arc;

const KEY_TIP_HEIGHT: &[u8] = b"m:tip_height";
const KEY_TIP_HASH: &[u8] = b"m:tip_hash";
const KEY_LAST_UID: &[u8] = b"m:last_uid";
const KEY_NETWORK: &[u8] = b"m:network";
const KEY_START_HEIGHT: &[u8] = b"m:start_height";

#[derive(Debug, Clone, Copy)]
pub struct IndexTip {
    pub height: u64,
    pub block_hash: BlockHashBytes,
    pub last_uid: u64,
}

#[derive(Debug, Clone)]
pub struct RocksIndexStore {
    db: Arc<DB>,
}

impl RocksIndexStore {
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.set_max_open_files(256);
        opts.set_use_fsync(false);
        opts.set_keep_log_file_num(4);
        let db = DB::open(&opts, path)?;
        Ok(Self { db: Arc::new(db) })
    }

    pub fn ensure_meta(&self, network: &str, start_height: u64) -> anyhow::Result<()> {
        self.ensure_meta_value(KEY_NETWORK, network.as_bytes())?;
        self.ensure_meta_value(KEY_START_HEIGHT, &start_height.to_be_bytes())?;
        Ok(())
    }

    fn ensure_meta_value(&self, key: &[u8], value: &[u8]) -> anyhow::Result<()> {
        if let Some(existing) = self.db.get(key)? {
            anyhow::ensure!(
                existing.as_slice() == value,
                "index metadata mismatch for {}",
                String::from_utf8_lossy(key)
            );
        } else {
            self.db.put(key, value)?;
        }
        Ok(())
    }

    pub fn tip(&self) -> anyhow::Result<Option<IndexTip>> {
        let Some(height) = self.db.get(KEY_TIP_HEIGHT)? else {
            return Ok(None);
        };
        let Some(hash) = self.db.get(KEY_TIP_HASH)? else {
            return Ok(None);
        };
        let Some(last_uid) = self.db.get(KEY_LAST_UID)? else {
            return Ok(None);
        };
        Ok(Some(IndexTip {
            height: read_u64(&height, "tip_height")?,
            block_hash: BlockHashBytes::from(read_32(&hash, "tip_hash")?),
            last_uid: read_u64(&last_uid, "last_uid")?,
        }))
    }

    pub fn previous_tip_hash(&self) -> anyhow::Result<Option<BlockHashBytes>> {
        Ok(self.tip()?.map(|tip| tip.block_hash))
    }

    pub fn lookup_outpoint_uids(
        &self,
        outpoints: &[OutPointKey],
    ) -> anyhow::Result<Vec<(OutPointKey, SpendLookup)>> {
        let mut found = Vec::new();
        for outpoint in outpoints {
            if let Some(value) = self.db.get(outpoint_key(*outpoint))? {
                found.push((*outpoint, read_spend_lookup(&value)?));
            }
        }
        Ok(found)
    }

    pub fn commit_applied_blocks<'a, I>(
        &self,
        blocks: I,
        tip_height: u64,
        tip_hash: BlockHashBytes,
        last_uid: u64,
    ) -> anyhow::Result<()>
    where
        I: IntoIterator<Item = &'a crate::p2tr_indexer::AppliedBlock>,
    {
        let mut batch = WriteBatch::default();

        for block in blocks {
            for created in &block.created_utxos {
                batch.put(
                    outpoint_key(created.entry.outpoint),
                    encode_spend_lookup(SpendLookup {
                        uid: created.entry.uid,
                        creation_height: created.entry.created_height,
                        flags: if created.is_reused {
                            crate::index::OUTPUT_FLAG_REUSED
                        } else {
                            0
                        },
                    }),
                );

                let key = seen_key(created.entry.p2tr_xonly_key);
                let next_count = self
                    .db
                    .get(key)?
                    .map(|bytes| read_u64(&bytes, "seen key count"))
                    .transpose()?
                    .unwrap_or(0)
                    .saturating_add(1);
                batch.put(
                    seen_key(created.entry.p2tr_xonly_key),
                    next_count.to_be_bytes(),
                );
            }

            for spent in &block.spent_utxos {
                batch.delete(outpoint_key(spent.outpoint));
            }
        }

        batch.put(KEY_TIP_HEIGHT, tip_height.to_be_bytes());
        batch.put(KEY_TIP_HASH, tip_hash.as_bytes());
        batch.put(KEY_LAST_UID, last_uid.to_be_bytes());
        self.db.write(batch)?;
        Ok(())
    }

    pub fn flush(&self) -> anyhow::Result<()> {
        self.db.flush()?;
        Ok(())
    }
}

fn outpoint_key(outpoint: OutPointKey) -> [u8; 38] {
    let mut key = [0u8; 38];
    key[0] = b'o';
    key[1] = b':';
    key[2..34].copy_from_slice(outpoint.txid.as_bytes());
    key[34..].copy_from_slice(&outpoint.vout.to_be_bytes());
    key
}

fn seen_key(xonly: [u8; 32]) -> [u8; 34] {
    let mut key = [0u8; 34];
    key[0] = b'k';
    key[1] = b':';
    key[2..].copy_from_slice(&xonly);
    key
}

fn encode_spend_lookup(spend: SpendLookup) -> [u8; 17] {
    let mut out = [0u8; 17];
    out[..8].copy_from_slice(&spend.uid.to_be_bytes());
    out[8..16].copy_from_slice(&spend.creation_height.to_be_bytes());
    out[16] = spend.flags;
    out
}

fn read_spend_lookup(bytes: &[u8]) -> anyhow::Result<SpendLookup> {
    match bytes.len() {
        // Backwards-compatible staged value from the first RocksDB patch.
        8 => Ok(SpendLookup {
            uid: read_u64(bytes, "outpoint uid")?,
            creation_height: 0,
            flags: 0,
        }),
        17 => {
            let uid = read_u64(&bytes[..8], "outpoint uid")?;
            let creation_height = read_u64(&bytes[8..16], "outpoint creation height")?;
            Ok(SpendLookup {
                uid,
                creation_height,
                flags: bytes[16],
            })
        }
        len => anyhow::bail!("invalid outpoint lookup value length {len}"),
    }
}

fn read_u64(bytes: &[u8], label: &str) -> anyhow::Result<u64> {
    anyhow::ensure!(bytes.len() == 8, "invalid {label} length {}", bytes.len());
    let mut out = [0u8; 8];
    out.copy_from_slice(bytes);
    Ok(u64::from_be_bytes(out))
}

fn read_32(bytes: &[u8], label: &str) -> anyhow::Result<[u8; 32]> {
    anyhow::ensure!(bytes.len() == 32, "invalid {label} length {}", bytes.len());
    let mut out = [0u8; 32];
    out.copy_from_slice(bytes);
    Ok(out)
}
