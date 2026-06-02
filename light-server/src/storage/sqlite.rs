use crate::storage::{ChainTip, Manifest, ManifestProfile};
use crate::storage::{ArchiveBackend, ServedProfile};
use async_trait::async_trait;
use crate::profile::{ArchiveScope, Profile};
use crate::{DEFAULT_MAX_RANGE_COUNT, WIRE_VERSION};
use serde_json::json;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct SqliteArchive {
    pool: SqlitePool,
}

impl SqliteArchive {
    pub async fn connect(database_url: &str, create_if_missing: bool) -> anyhow::Result<Self> {
        let options = SqliteConnectOptions::from_str(database_url)?
            .create_if_missing(create_if_missing)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await?;
        Ok(Self { pool })
    }

    pub async fn migrate(&self) -> anyhow::Result<()> {
        sqlx::migrate!("./src/storage/migrations").run(&self.pool).await?;
        Ok(())
    }

    pub fn pool(&self) -> &SqlitePool { &self.pool }

    pub async fn meta_text(&self, key: &str) -> anyhow::Result<Option<String>> {
        let row = sqlx::query("SELECT value FROM meta WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else { return Ok(None); };
        let value: Vec<u8> = row.try_get("value")?;
        Ok(Some(String::from_utf8_lossy(&value).into_owned()))
    }


}

#[async_trait]
impl ArchiveBackend for SqliteArchive {
    async fn manifest(&self) -> anyhow::Result<Manifest> {
        let network = self.meta_text("network").await?.unwrap_or_else(|| "unknown".to_string());
        let genesis_hash = self.meta_text("genesis_hash").await?;
        let checkpoint_interval = self.meta_text("checkpoint_interval").await?
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(10_000);

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

        Ok(Manifest {
            version: WIRE_VERSION,
            network,
            genesis_hash,
            checkpoint_interval,
            max_range_count: DEFAULT_MAX_RANGE_COUNT,
            profiles,
        })
    }

    async fn resolve_profile(&self, name: Option<&str>, profile: Option<Profile>) -> anyhow::Result<ServedProfile> {
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
        let materialization_interval_blocks: i64 = row.try_get("materialization_interval_blocks")?;
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

    async fn read_block(&self, height: u64, profile: &ServedProfile) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
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

    async fn read_blocks(&self, start: u64, count: u32, profile: &ServedProfile) -> anyhow::Result<Vec<Vec<u8>>> {
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

        anyhow::ensure!(rows.len() == count as usize, "range contains missing payloads");
        let mut out = Vec::with_capacity(rows.len());
        for (idx, row) in rows.into_iter().enumerate() {
            let height: i64 = row.try_get("height")?;
            let expected = i64::try_from(start)? + idx as i64;
            anyhow::ensure!(height == expected, "range contains non-contiguous payloads");
            out.push(row.try_get("payload")?);
        }
        Ok(out)
    }

    async fn read_checkpoint(&self, height: u64, profile: &ServedProfile) -> anyhow::Result<Vec<u8>> {
        let profile_id = profile.profile_id;
        let row = sqlx::query(
            r#"SELECT checkpoint
               FROM checkpoint_cache
               WHERE profile_id = ? AND height = ?"#,
        )
        .bind(profile_id)
        .bind(i64::try_from(height)?)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| anyhow::anyhow!("checkpoint not found for height {height}"))?;
        Ok(row.try_get("checkpoint")?)
    }

    async fn latest_checkpoint_height(&self, height_lte: u64, profile: &ServedProfile) -> anyhow::Result<Option<u64>> {
        let profile_id = profile.profile_id;
        let row = sqlx::query(
            r#"SELECT height
               FROM checkpoint_cache
               WHERE profile_id = ? AND height <= ?
               ORDER BY height DESC
               LIMIT 1"#,
        )
        .bind(profile_id)
        .bind(i64::try_from(height_lte)?)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => {
                let height: i64 = row.try_get("height")?;
                Ok(Some(height.try_into()?))
            }
            None => Ok(None),
        }
    }

    async fn block_stats(&self, height: u64) -> anyhow::Result<serde_json::Value> {
        let row = sqlx::query(
            r#"SELECT * FROM block_stats WHERE height = ?"#,
        )
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
