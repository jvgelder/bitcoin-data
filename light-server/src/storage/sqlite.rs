use crate::index::{
    encode_light_block, encode_snapshot_block, LightBlockInput, OutputRefInput, SnapshotBlockInput,
    SnapshotOutputRefInput, SnapshotTxInput, MAX_P2TR_OUTPUT_ID_BYTES,
};
use crate::output_id::{choose_output_id_bytes, truncate_into_packed};
use crate::p2tr_indexer::output_identifier_hash;
use crate::profile::{ArchiveScope, Profile};
use crate::range::frame_snapshot;
use crate::storage::{ArchiveBackend, CutthroughDeltaBlocks, CutthroughSnapshot, ServedProfile};
use crate::storage::{ChainTip, Manifest, ManifestCutthroughSnapshot, ManifestProfile};
use crate::types::{BlockHashBytes, OutputIdHash, TxTweak};
use crate::{DEFAULT_MAX_RANGE_COUNT, WIRE_VERSION};
use async_trait::async_trait;
use serde_json::json;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, SqlitePool};
use std::collections::HashSet;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct SqliteArchive {
    pool: SqlitePool,
}

// Conservative row budgets used to aim cut-through delta responses at about
// 100 KiB without splitting synthetic LightBlocks. These are not exact byte
// limits: Cap'n Proto framing, tweak density, and output-id width vary by block.
const MIN_CUTTHROUGH_DELTA_BLOCKS: u64 = 1;
const APPROX_BYTES_PER_DELTA_OUTPUT: usize = 96;
const APPROX_BYTES_PER_DELTA_SPEND: usize = 16;
const MIN_DELTA_CREATED_ROW_LIMIT: usize = 128;
const MIN_DELTA_SPENT_ROW_LIMIT: usize = 512;

impl SqliteArchive {
    pub async fn connect(database_url: &str, create_if_missing: bool) -> anyhow::Result<Self> {
        let options = SqliteConnectOptions::from_str(database_url)?
            .create_if_missing(create_if_missing)
            .foreign_keys(true)
            // Write-heavy indexer: WAL + NORMAL fsync removes the per-commit
            // rollback-journal + FULL-sync cost (dominant `commit_ms`), a large
            // page cache absorbs the B-tree churn, and temp tables stay in RAM.
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(std::time::Duration::from_secs(30))
            .pragma("cache_size", "-1048576")
            .pragma("mmap_size", "268435456")
            .pragma("temp_store", "MEMORY");
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await?;
        Ok(Self { pool })
    }

    pub async fn migrate(&self) -> anyhow::Result<()> {
        sqlx::migrate!("./src/storage/migrations")
            .run(&self.pool)
            .await?;
        Ok(())
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn validate_schema_compatibility(&self) -> anyhow::Result<()> {
        async fn table_columns(pool: &SqlitePool, table: &str) -> anyhow::Result<HashSet<String>> {
            let rows = match table {
                "blocks" => sqlx::query("PRAGMA table_info(blocks)")
                    .fetch_all(pool)
                    .await?,
                "p2tr_outputs" => sqlx::query("PRAGMA table_info(p2tr_outputs)")
                    .fetch_all(pool)
                    .await?,
                "p2tr_spends" => sqlx::query("PRAGMA table_info(p2tr_spends)")
                    .fetch_all(pool)
                    .await?,
                "tx_tweaks" => sqlx::query("PRAGMA table_info(tx_tweaks)")
                    .fetch_all(pool)
                    .await?,
                "payload_cache" => sqlx::query("PRAGMA table_info(payload_cache)")
                    .fetch_all(pool)
                    .await?,
                "profiles" => sqlx::query("PRAGMA table_info(profiles)")
                    .fetch_all(pool)
                    .await?,
                _ => anyhow::bail!("internal error: unsupported schema table `{table}`"),
            };
            anyhow::ensure!(!rows.is_empty(), "missing required table `{table}`");
            let mut columns = HashSet::with_capacity(rows.len());
            for row in rows {
                let name: String = row.try_get("name")?;
                columns.insert(name);
            }
            Ok(columns)
        }

        fn require_columns(
            table: &str,
            columns: &HashSet<String>,
            required: &[&str],
        ) -> anyhow::Result<()> {
            let missing = required
                .iter()
                .copied()
                .filter(|name| !columns.contains(*name))
                .collect::<Vec<_>>();
            anyhow::ensure!(
                missing.is_empty(),
                "database table `{table}` is missing required column(s): {}",
                missing.join(", ")
            );
            Ok(())
        }

        let blocks = table_columns(&self.pool, "blocks").await?;
        require_columns(
            "blocks",
            &blocks,
            &[
                "height",
                "block_hash",
                "previous_block_hash",
                "anchor_last_uid",
            ],
        )?;

        let p2tr_outputs = table_columns(&self.pool, "p2tr_outputs").await?;
        require_columns(
            "p2tr_outputs",
            &p2tr_outputs,
            &[
                "uid",
                "txid",
                "created_height",
                "tx_index",
                "vout",
                "value_sat",
                "script_pubkey",
                "p2tr_xonly_key",
            ],
        )?;

        let p2tr_spends = table_columns(&self.pool, "p2tr_spends").await?;
        require_columns(
            "p2tr_spends",
            &p2tr_spends,
            &["uid", "spent_height", "spent_block_hash", "spend_tx_index"],
        )?;

        let tx_tweaks = table_columns(&self.pool, "tx_tweaks").await?;
        require_columns("tx_tweaks", &tx_tweaks, &["height", "tx_index", "tweak"])?;

        let payload_cache = table_columns(&self.pool, "payload_cache").await?;
        require_columns(
            "payload_cache",
            &payload_cache,
            &["profile_id", "height", "block_hash", "payload", "payload_len"],
        )?;


        let profiles = table_columns(&self.pool, "profiles").await?;
        require_columns(
            "profiles",
            &profiles,
            &[
                "profile_id",
                "name",
                "scope",
                "cutthrough_blocks",
                "served_tip_height",
                "served_tip_hash",
            ],
        )?;

        Ok(())
    }

    pub async fn meta_text(&self, key: &str) -> anyhow::Result<Option<String>> {
        let row = sqlx::query("SELECT value FROM meta WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let value: Vec<u8> = row.try_get("value")?;
        Ok(Some(String::from_utf8_lossy(&value).into_owned()))
    }

    async fn choose_cutthrough_delta_end_height(
        &self,
        known_height: u64,
        max_end_height: u64,
        target_response_bytes: usize,
    ) -> anyhow::Result<u64> {
        anyhow::ensure!(
            known_height < max_end_height,
            "known_height must be below max_end_height"
        );

        let created_limit = (target_response_bytes / APPROX_BYTES_PER_DELTA_OUTPUT)
            .max(MIN_DELTA_CREATED_ROW_LIMIT);
        let spent_limit =
            (target_response_bytes / APPROX_BYTES_PER_DELTA_SPEND).max(MIN_DELTA_SPENT_ROW_LIMIT);

        let mut candidate_end = self
            .end_height_from_created_row_limit(known_height, max_end_height, created_limit)
            .await?;
        candidate_end = self
            .end_height_from_spent_row_limit(known_height, candidate_end, spent_limit)
            .await?;

        let min_end = (known_height + MIN_CUTTHROUGH_DELTA_BLOCKS).min(max_end_height);
        Ok(candidate_end.max(min_end).min(max_end_height))
    }

    async fn end_height_from_created_row_limit(
        &self,
        known_height: u64,
        max_end_height: u64,
        row_limit: usize,
    ) -> anyhow::Result<u64> {
        let rows = sqlx::query(
            r#"SELECT created_height
               FROM p2tr_outputs
               WHERE created_height > ? AND created_height <= ?
               ORDER BY created_height, tx_index, vout
               LIMIT ?"#,
        )
        .bind(i64::try_from(known_height)?)
        .bind(i64::try_from(max_end_height)?)
        .bind(i64::try_from(row_limit.saturating_add(1))?)
        .fetch_all(&self.pool)
        .await?;

        if rows.len() <= row_limit {
            return Ok(max_end_height);
        }

        let overflow_height: i64 = rows[row_limit].try_get("created_height")?;
        let overflow_height = u64::try_from(overflow_height)?;
        Ok(overflow_height.saturating_sub(1).max(known_height + 1))
    }

    async fn end_height_from_spent_row_limit(
        &self,
        known_height: u64,
        max_end_height: u64,
        row_limit: usize,
    ) -> anyhow::Result<u64> {
        let rows = sqlx::query(
            r#"SELECT s.spent_height
               FROM p2tr_spends s
               JOIN p2tr_outputs o ON o.uid = s.uid
               WHERE o.created_height <= ?
                 AND s.spent_height > ?
                 AND s.spent_height <= ?
               ORDER BY s.spent_height, s.uid
               LIMIT ?"#,
        )
        .bind(i64::try_from(known_height)?)
        .bind(i64::try_from(known_height)?)
        .bind(i64::try_from(max_end_height)?)
        .bind(i64::try_from(row_limit.saturating_add(1))?)
        .fetch_all(&self.pool)
        .await?;

        if rows.len() <= row_limit {
            return Ok(max_end_height);
        }

        let overflow_height: i64 = rows[row_limit].try_get("spent_height")?;
        let overflow_height = u64::try_from(overflow_height)?;
        Ok(overflow_height.saturating_sub(1).max(known_height + 1))
    }
}

fn blob32(row: &sqlx::sqlite::SqliteRow, column: &str) -> anyhow::Result<BlockHashBytes> {
    let bytes: Vec<u8> = row.try_get(column)?;
    anyhow::ensure!(bytes.len() == 32, "column {column} must be 32 bytes");
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(BlockHashBytes::from(out))
}

fn tx_tweak_from_blob(bytes: Vec<u8>) -> anyhow::Result<TxTweak> {
    anyhow::ensure!(
        bytes.len() == TxTweak::LEN,
        "tx tweak must be {} bytes",
        TxTweak::LEN
    );
    let mut out = [0u8; TxTweak::LEN];
    out.copy_from_slice(&bytes);
    Ok(TxTweak::from(out))
}

fn xonly_from_blob(bytes: Vec<u8>) -> anyhow::Result<[u8; 32]> {
    anyhow::ensure!(bytes.len() == 32, "p2tr x-only key must be 32 bytes");
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

#[async_trait]
impl ArchiveBackend for SqliteArchive {
    async fn manifest(&self) -> anyhow::Result<Manifest> {
        let network = self
            .meta_text("network")
            .await?
            .unwrap_or_else(|| "unknown".to_string());
        let genesis_hash = self.meta_text("genesis_hash").await?;
        let finality_depth = self
            .meta_text("finality_depth")
            .await?
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(6);
        let suggested_reorg_cache_depth = self
            .meta_text("suggested_reorg_cache_depth")
            .await?
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(24);

        let rows = sqlx::query(
            r#"SELECT scope, cutthrough_blocks, served_tip_height, served_tip_hash
               FROM profiles
               WHERE enabled = 1
               ORDER BY profile_id"#,
        )
        .fetch_all(&self.pool)
        .await?;

        let mut profiles = Vec::with_capacity(rows.len());
        for row in rows {
            let scope_text: String = row.try_get("scope")?;
            let scope = ArchiveScope::from_str(&scope_text)?;
            let ct: i64 = row.try_get("cutthrough_blocks")?;
            let tip_height: i64 = row.try_get("served_tip_height")?;
            let tip_hash: Option<Vec<u8>> = row.try_get("served_tip_hash")?;
            let tip = if tip_height > 0 {
                Some(ChainTip {
                    height: tip_height as u64,
                    block_hash: tip_hash.map(hex::encode).unwrap_or_default(),
                })
            } else {
                None
            };
            profiles.push(ManifestProfile {
                scope,
                cutthrough_blocks: ct.try_into()?,
                tip,
            });
        }

        let cutthrough_snapshots = profiles
            .iter()
            .filter_map(|p| {
                if p.cutthrough_blocks == 0 {
                    return None;
                }
                let tip = p.tip.as_ref()?;
                Some(ManifestCutthroughSnapshot {
                    scope: p.scope,
                    cutthrough_blocks: p.cutthrough_blocks,
                    height: tip.height,
                    block_hash: tip.block_hash.clone(),
                    latest_endpoint: "/blocks/light/cutthrough/snapshot/latest".to_string(),
                    endpoint: format!("/blocks/light/cutthrough/snapshot/{}.bdss", tip.height),
                })
            })
            .collect();

        Ok(Manifest {
            version: WIRE_VERSION,
            network,
            genesis_hash,
            finality_depth,
            suggested_reorg_cache_depth,
            max_range_count: DEFAULT_MAX_RANGE_COUNT,
            profiles,
            cutthrough_snapshots,
        })
    }

    async fn resolve_profile(
        &self,
        name: Option<&str>,
        profile: Option<Profile>,
    ) -> anyhow::Result<ServedProfile> {
        let row = if let Some(name) = name {
            sqlx::query(
                r#"SELECT profile_id, name, scope, cutthrough_blocks, materialization_interval_blocks,
                          served_tip_height, served_tip_hash
                   FROM profiles
                   WHERE name = ? AND enabled = 1"#,
            )
            .bind(name)
            .fetch_optional(&self.pool)
            .await?
        } else {
            let profile = profile.unwrap_or_default();
            sqlx::query(
                r#"SELECT profile_id, name, scope, cutthrough_blocks, materialization_interval_blocks,
                          served_tip_height, served_tip_hash
                   FROM profiles
                   WHERE scope = ? AND cutthrough_blocks = ? AND enabled = 1"#,
            )
            .bind(profile.scope.as_str())
            .bind(i64::from(profile.cutthrough_blocks))
            .fetch_optional(&self.pool)
            .await?
        };

        let row = row.ok_or_else(|| anyhow::anyhow!("profile not found"))?;
        let profile_id: i64 = row.try_get("profile_id")?;
        let name: String = row.try_get("name")?;
        let scope_text: String = row.try_get("scope")?;
        let cutthrough_blocks: i64 = row.try_get("cutthrough_blocks")?;
        let materialization_interval_blocks: i64 =
            row.try_get("materialization_interval_blocks")?;
        let served_tip_height: i64 = row.try_get("served_tip_height")?;
        let served_tip_hash: Option<Vec<u8>> = row.try_get("served_tip_hash")?;

        Ok(ServedProfile {
            profile_id,
            name,
            profile: Profile {
                scope: ArchiveScope::from_str(&scope_text)?,
                cutthrough_blocks: cutthrough_blocks.try_into()?,
            },
            materialization_interval_blocks: materialization_interval_blocks.try_into()?,
            served_tip: if served_tip_height > 0 {
                Some(ChainTip {
                    height: served_tip_height as u64,
                    block_hash: served_tip_hash.map(hex::encode).unwrap_or_default(),
                })
            } else {
                None
            },
        })
    }

    async fn tip(&self, db_profile: &ServedProfile) -> anyhow::Result<Option<ChainTip>> {
        Ok(db_profile.served_tip.clone())
    }

    async fn read_block(
        &self,
        height: u64,
        profile: &ServedProfile,
    ) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        let profile_id = profile.profile_id;
        let row = sqlx::query(
            r#"SELECT block_hash, payload
               FROM payload_cache
               WHERE profile_id = ? AND height = ?"#,
        )
        .bind(profile_id)
        .bind(i64::try_from(height)?)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| anyhow::anyhow!("block payload not found for height {height}"))?;
        Ok((row.try_get("payload")?, row.try_get("block_hash")?))
    }

    async fn read_blocks(
        &self,
        start: u64,
        count: u32,
        profile: &ServedProfile,
    ) -> anyhow::Result<Vec<Vec<u8>>> {
        let profile_id = profile.profile_id;
        let end = start + u64::from(count.saturating_sub(1));
        let rows = sqlx::query(
            r#"SELECT height, payload
               FROM payload_cache
               WHERE profile_id = ? AND height BETWEEN ? AND ?
               ORDER BY height"#,
        )
        .bind(profile_id)
        .bind(i64::try_from(start)?)
        .bind(i64::try_from(end)?)
        .fetch_all(&self.pool)
        .await?;

        anyhow::ensure!(
            rows.len() == count as usize,
            "range contains missing payloads"
        );
        let mut out = Vec::with_capacity(rows.len());
        for (idx, row) in rows.into_iter().enumerate() {
            let height: i64 = row.try_get("height")?;
            let expected = i64::try_from(start)? + idx as i64;
            anyhow::ensure!(height == expected, "range contains non-contiguous payloads");
            out.push(row.try_get("payload")?);
        }
        Ok(out)
    }

    async fn read_cutthrough_delta_blocks(
        &self,
        known_height: u64,
        max_end_height: u64,
        target_response_bytes: usize,
        profile: &ServedProfile,
    ) -> anyhow::Result<CutthroughDeltaBlocks> {
        anyhow::ensure!(
            known_height < max_end_height,
            "known_height must be below max_end_height"
        );
        anyhow::ensure!(
            profile.profile.cutthrough_blocks != 0,
            "cut-through delta must be requested with a cut-through profile"
        );

        let end_height = self
            .choose_cutthrough_delta_end_height(known_height, max_end_height, target_response_bytes)
            .await?;

        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"CREATE TEMP TABLE IF NOT EXISTS tmp_cutthrough_interval_spends (
                   uid INTEGER PRIMARY KEY
               ) WITHOUT ROWID"#,
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM tmp_cutthrough_interval_spends")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            r#"INSERT INTO tmp_cutthrough_interval_spends(uid)
               SELECT uid
               FROM p2tr_spends
               WHERE spent_height > ? AND spent_height <= ?"#,
        )
        .bind(i64::try_from(known_height)?)
        .bind(i64::try_from(end_height)?)
        .execute(&mut *tx)
        .await?;

        let block_rows = sqlx::query(
            r#"SELECT height, block_hash, previous_block_hash, anchor_last_uid
               FROM blocks
               WHERE height > ? AND height <= ?
               ORDER BY height"#,
        )
        .bind(i64::try_from(known_height)?)
        .bind(i64::try_from(end_height)?)
        .fetch_all(&mut *tx)
        .await?;

        let expected_count = usize::try_from(end_height - known_height)?;
        anyhow::ensure!(
            block_rows.len() == expected_count,
            "cut-through delta range contains missing block metadata"
        );

        let mut out = Vec::with_capacity(block_rows.len());
        for (idx, block_row) in block_rows.into_iter().enumerate() {
            let height_i: i64 = block_row.try_get("height")?;
            let height = u64::try_from(height_i)?;
            let expected_height = known_height + 1 + idx as u64;
            anyhow::ensure!(
                height == expected_height,
                "cut-through delta range contains non-contiguous block metadata"
            );

            let block_hash = blob32(&block_row, "block_hash")?;
            let previous_block_hash = blob32(&block_row, "previous_block_hash")?;
            let block_anchor_last_uid: i64 = block_row.try_get("anchor_last_uid")?;

            let output_rows = sqlx::query(
                r#"SELECT o.uid, o.tx_index, o.vout, o.p2tr_xonly_key
                   FROM p2tr_outputs o
                   LEFT JOIN tmp_cutthrough_interval_spends interval_spend
                     ON interval_spend.uid = o.uid
                   WHERE o.created_height = ?
                     AND interval_spend.uid IS NULL
                   ORDER BY o.tx_index, o.vout"#,
            )
            .bind(i64::try_from(height)?)
            .fetch_all(&mut *tx)
            .await?;

            let mut outputs = Vec::with_capacity(output_rows.len());
            let mut full_output_hashes = Vec::<OutputIdHash>::with_capacity(output_rows.len());
            for row in output_rows {
                let uid: i64 = row.try_get("uid")?;
                let tx_index: i64 = row.try_get("tx_index")?;
                let vout: i64 = row.try_get("vout")?;
                let xonly = xonly_from_blob(row.try_get("p2tr_xonly_key")?)?;

                outputs.push(OutputRefInput {
                    tx_index: u32::try_from(tx_index)?,
                    vout: u32::try_from(vout)?,
                    uid: u64::try_from(uid)?,
                });
                full_output_hashes.push(output_identifier_hash(&xonly));
            }

            let tweak_rows = sqlx::query(
                r#"SELECT DISTINCT t.tx_index, t.tweak
                   FROM tx_tweaks t
                   JOIN p2tr_outputs o
                     ON o.created_height = t.height
                    AND o.tx_index = t.tx_index
                   LEFT JOIN tmp_cutthrough_interval_spends interval_spend
                     ON interval_spend.uid = o.uid
                   WHERE t.height = ?
                     AND interval_spend.uid IS NULL
                   ORDER BY t.tx_index"#,
            )
            .bind(i64::try_from(height)?)
            .fetch_all(&mut *tx)
            .await?;

            let mut tx_tweak_indexes = Vec::with_capacity(tweak_rows.len());
            let mut tx_tweaks = Vec::with_capacity(tweak_rows.len());
            for row in tweak_rows {
                let tx_index: i64 = row.try_get("tx_index")?;
                tx_tweak_indexes.push(u32::try_from(tx_index)?);
                tx_tweaks.push(tx_tweak_from_blob(row.try_get("tweak")?)?);
            }

            let spent_rows = sqlx::query(
                r#"SELECT s.uid
                   FROM p2tr_spends s
                   JOIN tmp_cutthrough_interval_spends interval_spend
                     ON interval_spend.uid = s.uid
                   JOIN p2tr_outputs o ON o.uid = s.uid
                   WHERE s.spent_height = ?
                     AND o.created_height <= ?
                   ORDER BY s.uid"#,
            )
            .bind(i64::try_from(height)?)
            .bind(i64::try_from(known_height)?)
            .fetch_all(&mut *tx)
            .await?;

            let mut spent_uids_sorted = Vec::with_capacity(spent_rows.len());
            for row in spent_rows {
                let uid: i64 = row.try_get("uid")?;
                spent_uids_sorted.push(u64::try_from(uid)?);
            }

            let output_id_bytes = choose_output_id_bytes(
                outputs.len() as u64,
                crate::p2tr_indexer::OUTPUT_ID_COLLISION_PROBABILITY_LOG2,
            )
            .clamp(1, MAX_P2TR_OUTPUT_ID_BYTES);
            let output_ids = truncate_into_packed(&full_output_hashes, output_id_bytes)?;

            let block = LightBlockInput {
                height,
                block_hash,
                previous_block_hash,
                block_anchor_last_uid: u64::try_from(block_anchor_last_uid)?,
                profile: profile.profile,
                output_id_bytes,
                tx_tweak_indexes,
                tx_tweaks,
                outputs,
                output_ids,
                spent_uids_sorted,
            };
            let bytes = crate::index::to_packed_bytes(&encode_light_block(&block)?)?;
            out.push(bytes);
        }

        sqlx::query("DELETE FROM tmp_cutthrough_interval_spends")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        Ok(CutthroughDeltaBlocks {
            end_height,
            messages: out,
        })
    }

    async fn read_cutthrough_snapshot(
        &self,
        height: u64,
        profile: &ServedProfile,
    ) -> anyhow::Result<CutthroughSnapshot> {
        anyhow::ensure!(
            profile.profile.cutthrough_blocks != 0,
            "cut-through snapshot must be requested with a cut-through profile"
        );
        if let Some(row) = sqlx::query(
            r#"SELECT block_hash, payload, block_count
               FROM cutthrough_snapshot_cache
               WHERE profile_id = ? AND height = ?"#,
        )
        .bind(profile.profile_id)
        .bind(i64::try_from(height)?)
        .fetch_optional(&self.pool)
        .await?
        {
            let block_hash: Vec<u8> = row.try_get("block_hash")?;
            let block_count: i64 = row.try_get("block_count")?;
            return Ok(CutthroughSnapshot {
                height,
                block_hash: hex::encode(block_hash),
                block_count: u32::try_from(block_count)?,
                payload: row.try_get("payload")?,
            });
        }

        let tip_row = sqlx::query(
            r#"SELECT block_hash
               FROM blocks
               WHERE height = ?"#,
        )
        .bind(i64::try_from(height)?)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| anyhow::anyhow!("snapshot height {height} is not indexed"))?;
        let snapshot_block_hash: Vec<u8> = tip_row.try_get("block_hash")?;

        #[derive(Debug)]
        struct SnapshotRow {
            height: u64,
            block_hash: BlockHashBytes,
            previous_block_hash: BlockHashBytes,
            anchor_last_uid: u64,
            tx_index: u32,
            vout: u32,
            uid: u64,
            xonly: [u8; 32],
            tweak: TxTweak,
        }

        fn flush_snapshot_block(
            rows: &mut Vec<SnapshotRow>,
            messages: &mut Vec<Vec<u8>>,
        ) -> anyhow::Result<()> {
            if rows.is_empty() {
                return Ok(());
            }
            let height = rows[0].height;
            let block_hash = rows[0].block_hash;
            let previous_block_hash = rows[0].previous_block_hash;
            let block_anchor_last_uid = rows[0].anchor_last_uid;
            let output_id_bytes = choose_output_id_bytes(
                rows.len() as u64,
                crate::p2tr_indexer::OUTPUT_ID_COLLISION_PROBABILITY_LOG2,
            )
            .clamp(1, MAX_P2TR_OUTPUT_ID_BYTES);

            let mut txs = Vec::<SnapshotTxInput>::new();
            let mut i = 0usize;
            while i < rows.len() {
                let tx_index = rows[i].tx_index;
                let tweak = rows[i].tweak;
                let mut outputs = Vec::<SnapshotOutputRefInput>::new();
                let mut full_output_hashes = Vec::<OutputIdHash>::new();
                while i < rows.len() && rows[i].tx_index == tx_index {
                    outputs.push(SnapshotOutputRefInput {
                        vout: rows[i].vout,
                        uid: rows[i].uid,
                    });
                    full_output_hashes.push(output_identifier_hash(&rows[i].xonly));
                    i += 1;
                }
                let output_ids = truncate_into_packed(&full_output_hashes, output_id_bytes)?;
                txs.push(SnapshotTxInput {
                    tx_index,
                    tweak,
                    outputs,
                    output_ids,
                });
            }

            let block = SnapshotBlockInput {
                height,
                block_hash,
                previous_block_hash,
                block_anchor_last_uid,
                output_id_bytes,
                txs,
            };
            messages.push(crate::index::to_packed_bytes(&encode_snapshot_block(
                &block,
            )?)?);
            rows.clear();
            Ok(())
        }

        let mut cursor = sqlx::query(
            r#"SELECT b.height, b.block_hash, b.previous_block_hash, b.anchor_last_uid,
                      o.tx_index, o.vout, o.uid, o.p2tr_xonly_key, t.tweak
               FROM p2tr_outputs o
               JOIN blocks b ON b.height = o.created_height
               JOIN tx_tweaks t ON t.height = o.created_height AND t.tx_index = o.tx_index
               LEFT JOIN p2tr_spends s ON s.uid = o.uid
               WHERE o.created_height <= ?
                 AND (s.uid IS NULL OR s.spent_height > ?)
               ORDER BY o.created_height, o.tx_index, o.vout"#,
        )
        .bind(i64::try_from(height)?)
        .bind(i64::try_from(height)?)
        .fetch(&self.pool);

        let mut current_rows = Vec::<SnapshotRow>::new();
        let mut messages = Vec::<Vec<u8>>::new();
        while let Some(row) = futures::TryStreamExt::try_next(&mut cursor).await? {
            let row_height: i64 = row.try_get("height")?;
            let row_height = u64::try_from(row_height)?;
            if current_rows
                .first()
                .is_some_and(|first| first.height != row_height)
            {
                flush_snapshot_block(&mut current_rows, &mut messages)?;
            }
            let anchor_last_uid: i64 = row.try_get("anchor_last_uid")?;
            let tx_index: i64 = row.try_get("tx_index")?;
            let vout: i64 = row.try_get("vout")?;
            let uid: i64 = row.try_get("uid")?;
            current_rows.push(SnapshotRow {
                height: row_height,
                block_hash: blob32(&row, "block_hash")?,
                previous_block_hash: blob32(&row, "previous_block_hash")?,
                anchor_last_uid: u64::try_from(anchor_last_uid)?,
                tx_index: u32::try_from(tx_index)?,
                vout: u32::try_from(vout)?,
                uid: u64::try_from(uid)?,
                xonly: xonly_from_blob(row.try_get("p2tr_xonly_key")?)?,
                tweak: tx_tweak_from_blob(row.try_get("tweak")?)?,
            });
        }
        flush_snapshot_block(&mut current_rows, &mut messages)?;

        let block_count = u32::try_from(messages.len())?;
        let payload = frame_snapshot(height, &messages)?;
        sqlx::query(
            r#"INSERT OR REPLACE INTO cutthrough_snapshot_cache
               (profile_id, height, block_hash, payload, payload_len, block_count, created_at)
               VALUES (?, ?, ?, ?, ?, ?, unixepoch())"#,
        )
        .bind(profile.profile_id)
        .bind(i64::try_from(height)?)
        .bind(snapshot_block_hash.clone())
        .bind(payload.clone())
        .bind(i64::try_from(payload.len())?)
        .bind(i64::from(block_count))
        .execute(&self.pool)
        .await?;

        Ok(CutthroughSnapshot {
            height,
            block_hash: hex::encode(snapshot_block_hash),
            block_count,
            payload,
        })
    }


    async fn block_stats(&self, height: u64) -> anyhow::Result<serde_json::Value> {
        let row = sqlx::query(r#"SELECT * FROM block_stats WHERE height = ?"#)
            .bind(i64::try_from(height)?)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| anyhow::anyhow!("block stats not found for height {height}"))?;

        let reasons = sqlx::query(
            r#"SELECT reason, count
               FROM block_exclusion_stats
               WHERE height = ?
               ORDER BY reason"#,
        )
        .bind(i64::try_from(height)?)
        .fetch_all(&self.pool)
        .await?;

        let mut reason_map = serde_json::Map::new();
        for reason in reasons {
            let key: String = reason.try_get("reason")?;
            let count: i64 = reason.try_get("count")?;
            reason_map.insert(key, json!(count));
        }

        Ok(json!({
            "height": height,
            "tx_count": row.try_get::<i64, _>("tx_count")?,
            "output_count_total": row.try_get::<i64, _>("output_count_total")?,
            "p2tr_output_count": row.try_get::<i64, _>("p2tr_output_count")?,
            "p2tr_sp_candidate_count": row.try_get::<i64, _>("p2tr_sp_candidate_count")?,
            "p2tr_nums_count": row.try_get::<i64, _>("p2tr_nums_count")?,
            "p2tr_reused_count": row.try_get::<i64, _>("p2tr_reused_count")?,
            "p2tr_excluded_by_scope_count": row.try_get::<i64, _>("p2tr_excluded_by_scope_count")?,
            "indexed_output_count": row.try_get::<i64, _>("indexed_output_count")?,
            "indexed_spent_count": row.try_get::<i64, _>("indexed_spent_count")?,
            "tx_with_p2tr_output_count": row.try_get::<i64, _>("tx_with_p2tr_output_count")?,
            "tx_with_indexed_output_count": row.try_get::<i64, _>("tx_with_indexed_output_count")?,
            "tweak_count": row.try_get::<i64, _>("tweak_count")?,
            "exclusions": reason_map,
        }))
    }
}
