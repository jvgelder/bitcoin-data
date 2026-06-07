use bitcoin::consensus::encode::deserialize;
use bitcoin::hashes::Hash as BitcoinHash;
use bitcoin::Block;
use btc_data_core::source::{BlockSource, TipWatcher};
use btc_data_light_server::index::{
    encode_light_block, encode_uid_checkpoint, to_packed_bytes, UidCheckpointInput,
};
use btc_data_light_server::p2tr_indexer::{
    BlockScanInput, BlockScopeStats, OutPointKey, P2trIndexerState, ScopedUtxoEntry,
    TxInputScan, TxOutputScan, TxScanInput,
};
use btc_data_light_server::profile::{ArchiveNetwork, ArchiveScope, Profile};
use btc_data_light_server::script_classify::extract_p2tr_xonly;
use btc_data_light_server::sp_tweak::{compute_tx_scan_point, PrevoutInfo, ScanPointStatus, TxInputContext};
use btc_data_light_server::storage::{
    ArchiveBackend, ChainTip, FileArchive, Manifest, ManifestProfile, SqliteArchive,
};
use btc_data_light_server::types::{BlockHashBytes, TxTweak, TxidBytes};
use btc_data_sources::{IpcSource, PollingTipWatcher, RestSource};
use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};
use futures::{stream, StreamExt};
use sqlx::Row;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_CATCHUP_BATCH_SIZE: usize = 128;

#[derive(Debug, Clone)]
struct ChainUtxoEntry {
    outpoint: OutPointKey,
    value_sat: u64,
    script_pubkey: Vec<u8>,
    created_height: u64,
    created_block_hash: BlockHashBytes,
    created_tx_index: u32,
}

#[derive(Debug, Default)]
struct ChainUtxoState {
    entries: HashMap<OutPointKey, ChainUtxoEntry>,
}

impl ChainUtxoState {
    fn restore(entries: impl IntoIterator<Item = ChainUtxoEntry>) -> Self {
        Self {
            entries: entries.into_iter().map(|entry| (entry.outpoint, entry)).collect(),
        }
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn enrich_block_scan_points(&mut self, block: &mut BlockScanInput) -> anyhow::Result<ChainUtxoDelta> {
        let mut delta = ChainUtxoDelta::default();

        for tx in &mut block.txs {
            let input_context = tx
                .inputs
                .iter()
                .map(|input| {
                    let prevout = self.entries.get(&input.previous_output).map(|entry| PrevoutInfo {
                        value_sat: entry.value_sat,
                        script_pubkey: entry.script_pubkey.clone(),
                    });
                    TxInputContext {
                        previous_output: input.previous_output,
                        script_sig: input.script_sig.clone(),
                        witness: input.witness.clone(),
                        prevout,
                    }
                })
                .collect::<Vec<_>>();

            match compute_tx_scan_point(&input_context)? {
                ScanPointStatus::Computed(tweak) => tx.silent_payment_tweak = Some(tweak),
                ScanPointStatus::Ineligible | ScanPointStatus::MissingPrevout { .. } => {
                    tx.silent_payment_tweak = None
                }
            }

            for input in &tx.inputs {
                if let Some(spent) = self.entries.remove(&input.previous_output) {
                    delta.spent.push(spent);
                }
            }

            for output in &tx.outputs {
                let entry = ChainUtxoEntry {
                    outpoint: OutPointKey {
                        txid: tx.txid,
                        vout: output.vout,
                    },
                    value_sat: output.value_sat,
                    script_pubkey: output.script_pubkey.clone(),
                    created_height: block.height,
                    created_block_hash: block.block_hash,
                    created_tx_index: tx.tx_index,
                };
                self.entries.insert(entry.outpoint, entry.clone());
                delta.created.push(entry);
            }
        }

        Ok(delta)
    }
}

#[derive(Debug, Default)]
struct ChainUtxoDelta {
    created: Vec<ChainUtxoEntry>,
    spent: Vec<ChainUtxoEntry>,
}


#[derive(Debug, Parser)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum SourceKind {
    /// Bitcoin Core multiprocess IPC source from btc-data-sources.
    Ipc,
    /// Bitcoin Core REST source. Useful fallback/local development mode.
    Rest,
}

#[derive(Debug, Clone, ClapArgs)]
struct SourceCli {
    /// Block source used by this indexer command.
    #[arg(long, value_enum, default_value = "ipc")]
    source: SourceKind,

    /// Bitcoin Core IPC Unix socket path. Required for --source ipc.
    #[arg(long, env = "BITCOIN_IPC_SOCKET")]
    ipc_socket: Option<PathBuf>,

    /// Number of IPC worker threads to request from Bitcoin Core.
    #[arg(long, default_value_t = 8)]
    ipc_threads: usize,

    /// Bitcoin Core REST base URL. Only used with --source rest.
    #[arg(long, default_value = "http://127.0.0.1:8332")]
    rest_url: String,

    /// Polling fallback interval in seconds. Only used by REST/fallback watchers.
    #[arg(long, default_value_t = 10)]
    poll_interval_secs: u64,
}

struct SourceBundle {
    source: Arc<dyn BlockSource>,
    watcher: Arc<dyn TipWatcher>,
}

impl SourceCli {
    fn build_source(&self) -> anyhow::Result<Arc<dyn BlockSource>> {
        Ok(self.build_bundle()?.source)
    }

    fn build_bundle(&self) -> anyhow::Result<SourceBundle> {
        match self.source {
            SourceKind::Ipc => self.build_ipc_bundle(),
            SourceKind::Rest => {
                let source: Arc<dyn BlockSource> = Arc::new(RestSource::new(self.rest_url.clone()));
                let watcher: Arc<dyn TipWatcher> = Arc::new(PollingTipWatcher::new(
                    source.clone(),
                    Duration::from_secs(self.poll_interval_secs),
                ));
                Ok(SourceBundle { source, watcher })
            }
        }
    }

    fn build_ipc_bundle(&self) -> anyhow::Result<SourceBundle> {
        let socket = self.ipc_socket.clone().ok_or_else(|| {
            anyhow::anyhow!("--ipc-socket is required when --source ipc is selected")
        })?;

        let source: Arc<IpcSource> =
            Arc::new(IpcSource::connect_with_threads(socket, self.ipc_threads)?);

        Ok(SourceBundle {
            source: source.clone(),
            watcher: source,
        })
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Generate a deterministic file-backed fixture archive for decoder tests.
    Fixture {
        #[arg(long, default_value = "lightdata")]
        archive: PathBuf,

        #[arg(long, default_value_t = ArchiveNetwork::Fixture)]
        network: ArchiveNetwork,

        #[arg(long, default_value_t = ArchiveScope::P2trSp)]
        scope: ArchiveScope,

        /// Override the default start height for the selected network/scope.
        #[arg(long)]
        start_height: Option<u64>,

        #[arg(long, default_value_t = 3)]
        count: u64,
    },

    /// Generate a deterministic SQLite fixture archive for the Axum/SQLx server.
    FixtureDb {
        #[arg(long, default_value = "sqlite:lightdata.db")]
        database_url: String,

        #[arg(long, default_value_t = ArchiveNetwork::Fixture)]
        network: ArchiveNetwork,

        #[arg(long, default_value_t = ArchiveScope::P2trSp)]
        scope: ArchiveScope,

        /// Override the default start height for the selected network/scope.
        #[arg(long)]
        start_height: Option<u64>,

        #[arg(long, default_value_t = 3)]
        count: u64,
    },

    /// Continuously watch the source tip and process newly finalized block ranges.
    Run {
        #[command(flatten)]
        source: SourceCli,

        #[arg(long, default_value = "sqlite:lightdata.db")]
        database_url: String,

        #[arg(long, default_value_t = ArchiveNetwork::Mainnet)]
        network: ArchiveNetwork,

        #[arg(long, default_value_t = ArchiveScope::P2trSp)]
        scope: ArchiveScope,

        /// Override the default start height for the selected network/scope.
        #[arg(long)]
        start_height: Option<u64>,

        #[arg(long, default_value_t = 6)]
        finality_depth: u64,

        /// Number of blocks fetched per catch-up batch.
        #[arg(long, default_value_t = DEFAULT_CATCHUP_BATCH_SIZE)]
        catchup_batch_size: usize,

        /// Do one catch-up pass and exit without waiting for another tip signal.
        #[arg(long, default_value_t = false)]
        once: bool,
    },

    /// Connect to the configured block source and print its best height.
    SourceTip {
        #[command(flatten)]
        source: SourceCli,
    },

    /// Fetch one raw block from the configured source and optionally write it to disk.
    FetchBlock {
        #[command(flatten)]
        source: SourceCli,

        #[arg(long)]
        height: u64,

        #[arg(long)]
        output: Option<PathBuf>,
    },

    /// Fetch a contiguous raw block range from the configured source.
    FetchRange {
        #[command(flatten)]
        source: SourceCli,

        #[arg(long)]
        start_height: u64,

        #[arg(long, default_value_t = 1)]
        count: usize,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    match args.command {
        Command::Fixture {
            archive,
            network,
            scope,
            start_height,
            count,
        } => {
            let start_height = start_height.unwrap_or_else(|| scope.default_start_height(network));
            write_fixture_archive(archive, network, scope, start_height, count)
        }

        Command::FixtureDb {
            database_url,
            network,
            scope,
            start_height,
            count,
        } => {
            let start_height = start_height.unwrap_or_else(|| scope.default_start_height(network));
            write_fixture_db(database_url, network, scope, start_height, count).await
        }

        Command::Run {
            source,
            database_url,
            network,
            scope,
            start_height,
            finality_depth,
            catchup_batch_size,
            once,
        } => {
            let start_height = start_height.unwrap_or_else(|| scope.default_start_height(network));
            run_command(
                source,
                database_url,
                network,
                scope,
                start_height,
                finality_depth,
                catchup_batch_size,
                once,
            )
            .await
        }

        Command::SourceTip { source } => source_tip_command(source).await,

        Command::FetchBlock {
            source,
            height,
            output,
        } => fetch_block_command(source, height, output).await,

        Command::FetchRange {
            source,
            start_height,
            count,
        } => fetch_range_command(source, start_height, count).await,
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_command(
    source: SourceCli,
    database_url: String,
    network: ArchiveNetwork,
    scope: ArchiveScope,
    start_height: u64,
    finality_depth: u64,
    catchup_batch_size: usize,
    once: bool,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        catchup_batch_size > 0,
        "catchup_batch_size must be greater than zero"
    );

    let bundle = source.build_bundle()?;
    let archive = SqliteArchive::connect(&database_url, true).await?;
    archive.migrate().await?;
    ensure_archive_meta(&archive, network, scope, start_height).await?;

    let profile = Profile {
        scope,
        cutthrough_blocks: 0,
    };
    let db_profile = archive.resolve_profile(None, Some(profile)).await?;

    let initial_next_height = next_index_height(&archive, start_height).await?;
    let mut state = restore_indexer_state(&archive, start_height).await?;
    let mut chain_utxos = restore_chain_utxo_state(&archive).await?;
    println!(
        "restored indexer state next_height={} last_uid={} live_p2tr_utxos={} live_chain_utxos={}",
        initial_next_height,
        state.last_uid(),
        state.live_uids_sorted().len(),
        chain_utxos.len()
    );

    loop {
        let best_height = bundle.source.get_best_height().await?;
        let best_hash = bundle.source.get_block_hash(best_height).await?;
        let finalized_tip = best_height.saturating_sub(finality_depth);
        let next_height = next_index_height(&archive, start_height).await?;

        println!(
            "source={} watcher={} best_height={} best_hash={} finalized_tip={} next_height={} database={}",
            bundle.source.name(),
            bundle.watcher.name(),
            best_height,
            display_hash(best_hash),
            finalized_tip,
            next_height,
            database_url,
        );

        if next_height <= finalized_tip {
            catch_up_ranges(
                &archive,
                bundle.source.as_ref(),
                &mut state,
                &mut chain_utxos,
                profile,
                db_profile.profile_id,
                next_height,
                finalized_tip,
                catchup_batch_size,
            )
            .await?;

            // Continue immediately after catch-up work. While processing a large
            // historical range, Core may advance again; do not wait until there
            // is no finalized work left.
            if once {
                return Ok(());
            }
            continue;
        }

        if once {
            return Ok(());
        }

        // Only once fully caught up do we block on Core notifications.
        bundle.watcher.wait_for_tip_change(Some(best_hash)).await?;
    }
}

const SQL_INSERT_CHUNK: usize = 64;

// NOTE: one batch (batch_size blocks) is fetched, decoded, applied, and held in
// RAM (applied structs + encoded payloads) before flushing to SQL and fetching
// the next batch. Larger batch = fewer/larger inserts but more memory.
#[allow(clippy::too_many_arguments)]
async fn catch_up_ranges(
    archive: &SqliteArchive,
    source: &dyn BlockSource,
    state: &mut P2trIndexerState,
    chain_utxos: &mut ChainUtxoState,
    profile: Profile,
    profile_id: i64,
    start_height: u64,
    finalized_tip: u64,
    batch_size: usize,
) -> anyhow::Result<()> {
    let remaining = finalized_tip - start_height + 1;
    let count = usize::try_from(remaining.min(batch_size as u64))?;

    // Stage 1: parallel fetch + decode (stateless).
    let frames = source
        .get_block_range_by_height(start_height, count)
        .await?;
    if frames.is_empty() {
        anyhow::bail!("source returned an empty block range at height {start_height}");
    }

    let mut decoded: Vec<BlockScanInput> = stream::iter(frames.iter())
        .map(|frame| {
            let height = frame.height;
            let hash = frame.hash;
            let bytes = frame.bytes.clone();
            async move {
                tokio::task::spawn_blocking(move || {
                    decode_block_frame(height, hash, bytes.as_ref())
                })
                .await
                .map_err(|e| anyhow::anyhow!("decode worker failed: {e}"))?
            }
        })
        .buffer_unordered(8)
        .collect::<Vec<anyhow::Result<BlockScanInput>>>()
        .await
        .into_iter()
        .collect::<anyhow::Result<Vec<_>>>()?;
    decoded.sort_by_key(|d| d.height);

    let first_height = frames.first().expect("non-empty").height;
    let last_frame = frames.last().expect("non-empty");
    let last_height = last_frame.height;
    let last_hash = last_frame.hash;
    let total_bytes: usize = frames.iter().map(|f| f.bytes.len()).sum();
    println!(
        "fetched finalized range {}..={} count={} bytes={}",
        first_height,
        last_height,
        decoded.len(),
        total_bytes
    );

    // Stage 2: serial, ordered apply (advances last_uid).
    struct Pending {
        applied: btc_data_light_server::p2tr_indexer::AppliedBlock,
        payload: Vec<u8>,
        chain_delta: ChainUtxoDelta,
    }
    let mut pending: Vec<Pending> = Vec::with_capacity(decoded.len());
    for mut scan in decoded {
        let chain_delta = chain_utxos.enrich_block_scan_points(&mut scan)?;
        let applied = state.apply_block_with_stats(scan, profile)?;
        let payload = to_packed_bytes(&encode_light_block(&applied.light_block)?)?;
        pending.push(Pending {
            applied,
            payload,
            chain_delta,
        });
    }

    // Stage 3: batched multi-row inserts in one transaction.
    let mut tx = archive.pool().begin().await?;
    for chunk in pending.chunks(SQL_INSERT_CHUNK) {
        {
            let mut qb = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
                "INSERT OR REPLACE INTO blocks \
                 (height, block_hash, previous_block_hash, p2tr_created_count, p2tr_spent_count, anchor_last_uid) ",
            );
            qb.push_values(chunk, |mut b, p| {
                let block = &p.applied.light_block;
                let stats = &p.applied.stats;
                b.push_bind(block.height as i64)
                    .push_bind(block.block_hash.as_bytes().to_vec())
                    .push_bind(block.previous_block_hash.as_bytes().to_vec())
                    .push_bind(i64::from(stats.indexed_output_count))
                    .push_bind(i64::from(stats.indexed_spent_count))
                    .push_bind(block.block_anchor_last_uid as i64);
            });
            qb.build().execute(&mut *tx).await?;
        }
        {
            let mut qb = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
                "INSERT OR REPLACE INTO block_stats \
                 (height, tx_count, output_count_total, p2tr_output_count, p2tr_sp_candidate_count, \
                  p2tr_nums_count, p2tr_reused_count, p2tr_excluded_by_scope_count, \
                  indexed_output_count, indexed_spent_count, tx_with_p2tr_output_count, \
                  tx_with_indexed_output_count, tweak_count) ",
            );
            qb.push_values(chunk, |mut b, p| {
                let block = &p.applied.light_block;
                let s = &p.applied.stats;
                b.push_bind(block.height as i64)
                    .push_bind(i64::from(s.tx_count))
                    .push_bind(i64::from(s.output_count_total))
                    .push_bind(i64::from(s.p2tr_output_count))
                    .push_bind(i64::from(s.p2tr_sp_candidate_count))
                    .push_bind(i64::from(s.p2tr_nums_count))
                    .push_bind(i64::from(s.p2tr_reused_count))
                    .push_bind(i64::from(s.p2tr_excluded_by_scope_count))
                    .push_bind(i64::from(s.indexed_output_count))
                    .push_bind(i64::from(s.indexed_spent_count))
                    .push_bind(i64::from(s.tx_with_p2tr_output_count))
                    .push_bind(i64::from(s.tx_with_indexed_output_count))
                    .push_bind(i64::from(s.tweak_count));
            });
            qb.build().execute(&mut *tx).await?;
        }
        {
            let mut qb = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
                "INSERT OR REPLACE INTO payload_cache \
                 (profile_id, height, block_hash, payload, payload_len, created_at) ",
            );
            qb.push_values(chunk, |mut b, p| {
                let block = &p.applied.light_block;
                b.push_bind(profile_id)
                    .push_bind(block.height as i64)
                    .push_bind(block.block_hash.as_bytes().to_vec())
                    .push_bind(p.payload.clone())
                    .push_bind(p.payload.len() as i64)
                    .push("unixepoch()");
            });
            qb.build().execute(&mut *tx).await?;
        }
    }

    // Stage 4: persist the scoped UTXO delta needed for resume and later
    // BIP352 prevout-aware scan-point construction. This is deliberately kept
    // in the same SQL transaction as the payload cache and profile tip update:
    // either the served block and its indexer state both advance, or neither does.
    for p in &pending {
        let block = &p.applied.light_block;

        for spent in &p.chain_delta.spent {
            sqlx::query(r#"DELETE FROM chain_utxo_lookup WHERE txid = ? AND vout = ?"#)
                .bind(spent.outpoint.txid.as_bytes().to_vec())
                .bind(i64::from(spent.outpoint.vout))
                .execute(&mut *tx)
                .await?;
        }

        for created in &p.chain_delta.created {
            sqlx::query(
                r#"INSERT OR REPLACE INTO chain_utxo_lookup
                   (txid, vout, value_sat, script_pubkey, created_height,
                    created_block_hash, created_tx_index)
                   VALUES (?, ?, ?, ?, ?, ?, ?)"#,
            )
            .bind(created.outpoint.txid.as_bytes().to_vec())
            .bind(i64::from(created.outpoint.vout))
            .bind(i64::try_from(created.value_sat)?)
            .bind(created.script_pubkey.clone())
            .bind(i64::try_from(created.created_height)?)
            .bind(created.created_block_hash.as_bytes().to_vec())
            .bind(i64::from(created.created_tx_index))
            .execute(&mut *tx)
            .await?;
        }

        for created in &p.applied.created_utxos {
            let entry = &created.entry;
            sqlx::query(
                r#"INSERT OR REPLACE INTO p2tr_outputs
                   (uid, created_height, created_block_hash, tx_index, vout, value_sat, is_nums, is_reused,
                    reuse_count_at_creation, txid, script_pubkey, p2tr_xonly_key)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
            )
            .bind(i64::try_from(entry.uid)?)
            .bind(i64::try_from(entry.created_height)?)
            .bind(entry.created_block_hash.as_bytes().to_vec())
            .bind(i64::from(entry.tx_index))
            .bind(i64::from(entry.outpoint.vout))
            .bind(i64::try_from(entry.value_sat)?)
            .bind(if created.is_nums { 1_i64 } else { 0_i64 })
            .bind(if created.is_reused { 1_i64 } else { 0_i64 })
            .bind(i64::try_from(created.reuse_count_at_creation)?)
            .bind(entry.outpoint.txid.as_bytes().to_vec())
            .bind(entry.script_pubkey.clone())
            .bind(entry.p2tr_xonly_key.to_vec())
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                r#"INSERT OR REPLACE INTO p2tr_utxo_lookup
                   (txid, vout, uid, value_sat, script_pubkey, p2tr_xonly_key,
                    created_height, created_block_hash, created_tx_index)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
            )
            .bind(entry.outpoint.txid.as_bytes().to_vec())
            .bind(i64::from(entry.outpoint.vout))
            .bind(i64::try_from(entry.uid)?)
            .bind(i64::try_from(entry.value_sat)?)
            .bind(entry.script_pubkey.clone())
            .bind(entry.p2tr_xonly_key.to_vec())
            .bind(i64::try_from(entry.created_height)?)
            .bind(entry.created_block_hash.as_bytes().to_vec())
            .bind(i64::from(entry.tx_index))
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                r#"INSERT INTO p2tr_key_stats
                   (output_key, first_height, last_height, seen_count, first_uid, last_uid, is_nums)
                   VALUES (?, ?, ?, 1, ?, ?, ?)
                   ON CONFLICT(output_key) DO UPDATE SET
                     last_height = excluded.last_height,
                     seen_count = p2tr_key_stats.seen_count + 1,
                     last_uid = excluded.last_uid,
                     is_nums = CASE WHEN p2tr_key_stats.is_nums != 0 OR excluded.is_nums != 0 THEN 1 ELSE 0 END"#,
            )
            .bind(entry.p2tr_xonly_key.to_vec())
            .bind(i64::try_from(entry.created_height)?)
            .bind(i64::try_from(entry.created_height)?)
            .bind(i64::try_from(entry.uid)?)
            .bind(i64::try_from(entry.uid)?)
            .bind(if created.is_nums { 1_i64 } else { 0_i64 })
            .execute(&mut *tx)
            .await?;
        }

        for spent in &p.applied.spent_utxos {
            let entry = &spent.entry;
            sqlx::query(
                r#"INSERT OR REPLACE INTO p2tr_spends
                   (uid, spent_height, spent_block_hash, spend_tx_index)
                   VALUES (?, ?, ?, ?)"#,
            )
            .bind(i64::try_from(entry.uid)?)
            .bind(i64::try_from(spent.spent_height)?)
            .bind(spent.spent_block_hash.as_bytes().to_vec())
            .bind(i64::from(spent.spend_tx_index))
            .execute(&mut *tx)
            .await?;

            sqlx::query(r#"DELETE FROM p2tr_utxo_lookup WHERE txid = ? AND vout = ?"#)
                .bind(entry.outpoint.txid.as_bytes().to_vec())
                .bind(i64::from(entry.outpoint.vout))
                .execute(&mut *tx)
                .await?;
        }

        for (tx_index, tweak) in block.tx_tweak_indexes.iter().zip(block.tx_tweaks.iter()) {
            sqlx::query(
                r#"INSERT OR REPLACE INTO tx_tweaks
                   (height, tx_index, tweak)
                   VALUES (?, ?, ?)"#,
            )
            .bind(i64::try_from(block.height)?)
            .bind(i64::from(*tx_index))
            .bind(tweak.as_bytes().to_vec())
            .execute(&mut *tx)
            .await?;
        }
    }

    // checkpoint + profile tip: once per range.
    let checkpoint_hash = BlockHashBytes::from(last_hash);
    let checkpoint = UidCheckpointInput {
        height: last_height,
        block_hash: checkpoint_hash,
        last_uid: state.last_uid(),
        profile,
        unspent_uids_sorted: state.live_uids_sorted(),
    };
    let checkpoint_bytes = to_packed_bytes(&encode_uid_checkpoint(&checkpoint)?)?;

    sqlx::query(
        r#"INSERT OR REPLACE INTO checkpoint_cache
           (profile_id, height, block_hash, checkpoint, checkpoint_len, created_at)
           VALUES (?, ?, ?, ?, ?, unixepoch())"#,
    )
    .bind(profile_id)
    .bind(i64::try_from(last_height)?)
    .bind(checkpoint_hash.as_bytes().to_vec())
    .bind(checkpoint_bytes.clone())
    .bind(i64::try_from(checkpoint_bytes.len())?)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"UPDATE profiles
           SET served_tip_height = ?, served_tip_hash = ?
           WHERE profile_id = ?"#,
    )
    .bind(i64::try_from(last_height)?)
    .bind(checkpoint_hash.as_bytes().to_vec())
    .bind(profile_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    println!(
        "indexed finalized range {}..={} last_uid={} live_uids={}",
        first_height,
        last_height,
        state.last_uid(),
        state.live_uids_sorted().len(),
    );

    Ok(())
}

fn decode_block_frame(
    height: u64,
    source_hash: [u8; 32],
    bytes: &[u8],
) -> anyhow::Result<BlockScanInput> {
    let block: Block = deserialize(bytes)?;
    let computed_hash = block.block_hash().to_byte_array();
    anyhow::ensure!(
        computed_hash == source_hash,
        "source hash mismatch at height {height}: source={} decoded={}",
        display_hash(source_hash),
        display_hash(computed_hash)
    );

    let mut txs = Vec::with_capacity(block.txdata.len());

    for (tx_index, tx) in block.txdata.iter().enumerate() {
        let tx_index = u32::try_from(tx_index)?;
        let txid = TxidBytes::from(tx.compute_txid().to_byte_array());

        let inputs = tx
            .input
            .iter()
            .map(|input| TxInputScan {
                previous_output: OutPointKey {
                    txid: TxidBytes::from(input.previous_output.txid.to_byte_array()),
                    vout: input.previous_output.vout,
                },
                script_sig: input.script_sig.as_bytes().to_vec(),
                witness: input.witness.iter().map(|item| item.to_vec()).collect(),
            })
            .collect::<Vec<_>>();

        let mut outputs = Vec::with_capacity(tx.output.len());

        for (vout, output) in tx.output.iter().enumerate() {
            let vout = u32::try_from(vout)?;
            let script = &output.script_pubkey;
            let script_bytes = script.as_bytes();
            let p2tr_key = extract_p2tr_xonly(script_bytes);
            let is_p2tr = p2tr_key.is_some();
            outputs.push(TxOutputScan {
                vout,
                value_sat: output.value.to_sat(),
                script_pubkey: script_bytes.to_vec(),
                p2tr_xonly_key: p2tr_key,
                is_p2tr,
                // NUMS is not an output-level property for UID assignment. It is
                // detected from Taproot script-path spend control blocks when
                // computing BIP352 input eligibility. Raw block ingestion does not
                // currently have enough prevout/script-path context to quantify it.
                is_nums: false,
            });
        }

        txs.push(TxScanInput {
            txid,
            tx_index,
            inputs,
            outputs,
            // Filled later by the ordered indexer pass after prevout context is
            // available from chain_utxo_lookup. Decoding raw blocks is kept
            // stateless and must not fabricate scan data.
            silent_payment_tweak: None,
        });
    }

    Ok(BlockScanInput {
        height,
        block_hash: BlockHashBytes::from(source_hash),
        previous_block_hash: BlockHashBytes::from(block.header.prev_blockhash.to_byte_array()),
        txs,
    })
}

async fn source_tip_command(source: SourceCli) -> anyhow::Result<()> {
    let src = source.build_source()?;
    let tip = src.get_best_height().await?;
    let hash = src.get_block_hash(tip).await?;
    println!(
        "source={} best_height={} best_hash={}",
        src.name(),
        tip,
        display_hash(hash)
    );
    Ok(())
}

async fn fetch_block_command(
    source: SourceCli,
    height: u64,
    output: Option<PathBuf>,
) -> anyhow::Result<()> {
    let src = source.build_source()?;
    let frame = src.get_block_by_height(height).await?;

    if let Some(path) = output {
        std::fs::write(&path, frame.bytes.as_ref())?;
        println!(
            "source={} height={} hash={} bytes={} wrote={}",
            src.name(),
            frame.height,
            display_hash(frame.hash),
            frame.bytes.len(),
            path.display()
        );
    } else {
        println!(
            "source={} height={} hash={} bytes={}",
            src.name(),
            frame.height,
            display_hash(frame.hash),
            frame.bytes.len()
        );
    }

    Ok(())
}

async fn fetch_range_command(
    source: SourceCli,
    start_height: u64,
    count: usize,
) -> anyhow::Result<()> {
    anyhow::ensure!(count > 0, "count must be greater than zero");

    let src = source.build_source()?;
    let frames = src.get_block_range_by_height(start_height, count).await?;
    let total_bytes: usize = frames.iter().map(|frame| frame.bytes.len()).sum();
    let end = frames
        .last()
        .map(|frame| frame.height)
        .unwrap_or(start_height);

    println!(
        "source={} start={} end={} count={} total_bytes={}",
        src.name(),
        start_height,
        end,
        frames.len(),
        total_bytes
    );

    Ok(())
}

fn display_hash(mut hash: [u8; 32]) -> String {
    hash.reverse();
    hex::encode(hash)
}

type FixtureBlocks = (
    Vec<(
        u64,
        BlockHashBytes,
        BlockHashBytes,
        u64,
        Vec<u8>,
        BlockScopeStats,
    )>,
    Vec<u8>,
    u64,
    BlockHashBytes,
    u64,
    Vec<u64>,
);

fn fixture_blocks(
    scope: ArchiveScope,
    start_height: u64,
    count: u64,
) -> anyhow::Result<FixtureBlocks> {
    anyhow::ensure!(count > 0, "count must be greater than zero");

    let profile = Profile {
        scope,
        cutthrough_blocks: 0,
    };

    let mut state = P2trIndexerState::new();
    let mut tip_hash = BlockHashBytes::from([0u8; 32]);
    let mut previous_indexed_outpoint: Option<OutPointKey> = None;
    let mut blocks = Vec::new();

    for i in 0..count {
        let height = start_height + i;
        let prev_hash = tip_hash;

        let mut hash_bytes = [0u8; 32];
        hash_bytes[..8].copy_from_slice(&height.to_le_bytes());
        let hash = BlockHashBytes::from(hash_bytes);
        tip_hash = hash;

        let txid_a = deterministic_txid(height, 0);
        let txid_b = deterministic_txid(height, 1);
        let mut txs = Vec::new();

        txs.push(TxScanInput {
            txid: txid_a,
            tx_index: 0,
            inputs: vec![],
            outputs: vec![
                TxOutputScan {
                    vout: 0,
                    value_sat: 0,
                    script_pubkey: Vec::new(),
                    p2tr_xonly_key: None,
                    is_p2tr: false,
                    is_nums: false,
                },
                TxOutputScan {
                    vout: 1,
                    value_sat: 0,
                    script_pubkey: Vec::new(),
                    p2tr_xonly_key: Some([height as u8; 32]),
                    is_p2tr: true,
                    is_nums: false,
                },
            ],
            silent_payment_tweak: Some(TxTweak::from([1u8; 33])),
        });

        let inputs = previous_indexed_outpoint
            .take()
            .map(|previous_output| {
                vec![TxInputScan {
                    previous_output,
                    script_sig: Vec::new(),
                    witness: Vec::new(),
                }]
            })
            .unwrap_or_default();

        txs.push(TxScanInput {
            txid: txid_b,
            tx_index: 1,
            inputs,
            outputs: vec![TxOutputScan {
                vout: 0,
                value_sat: 0,
                script_pubkey: Vec::new(),
                p2tr_xonly_key: Some([height.wrapping_add(1) as u8; 32]),
                is_p2tr: true,
                is_nums: false,
            }],
            silent_payment_tweak: Some(TxTweak::from([2u8; 33])),
        });

        if scope.include_p2tr_output(false) {
            previous_indexed_outpoint = Some(OutPointKey {
                txid: txid_b,
                vout: 0,
            });
        }

        let applied = state.apply_block_with_stats(
            BlockScanInput {
                height,
                block_hash: hash,
                previous_block_hash: prev_hash,
                txs,
            },
            profile,
        )?;

        let bytes = to_packed_bytes(&encode_light_block(&applied.light_block)?)?;

        blocks.push((
            height,
            hash,
            prev_hash,
            applied.light_block.block_anchor_last_uid,
            bytes,
            applied.stats,
        ));
    }

    let checkpoint_height = start_height + count - 1;
    let checkpoint = UidCheckpointInput {
        height: checkpoint_height,
        block_hash: tip_hash,
        last_uid: state.last_uid(),
        profile,
        unspent_uids_sorted: state.live_uids_sorted(),
    };

    let checkpoint_bytes = to_packed_bytes(&encode_uid_checkpoint(&checkpoint)?)?;

    Ok((
        blocks,
        checkpoint_bytes,
        checkpoint_height,
        tip_hash,
        state.last_uid(),
        state.live_uids_sorted(),
    ))
}

fn write_fixture_archive(
    root: PathBuf,
    network: ArchiveNetwork,
    scope: ArchiveScope,
    start_height: u64,
    count: u64,
) -> anyhow::Result<()> {
    let archive = FileArchive::new(root);
    let profile = Profile {
        scope,
        cutthrough_blocks: 0,
    };

    let (blocks, checkpoint_bytes, checkpoint_height, tip_hash, _last_uid, _live) =
        fixture_blocks(scope, start_height, count)?;

    for (height, _hash, _prev_hash, _anchor_last_uid, bytes, stats) in blocks {
        archive.write_block_bytes(height, profile, &bytes)?;
        archive.write_block_stats(height, &stats)?;
    }

    archive.write_checkpoint_bytes(checkpoint_height, profile, &checkpoint_bytes)?;

    archive.write_manifest(&Manifest {
        version: btc_data_light_server::WIRE_VERSION,
        network: network.to_string(),
        genesis_hash: None,
        checkpoint_interval: count,
        finality_depth: 6,
        suggested_reorg_cache_depth: 144,
        max_range_count: btc_data_light_server::DEFAULT_MAX_RANGE_COUNT,
        profiles: vec![ManifestProfile {
            scope,
            cutthrough_blocks: 0,
            tip: Some(ChainTip {
                height: checkpoint_height,
                block_hash: hex::encode(tip_hash.as_bytes()),
            }),
        }],
    })?;

    println!(
        "wrote file-backed {} fixture archive at {} starting from height {}",
        scope,
        archive.root().display(),
        start_height
    );

    Ok(())
}

async fn write_fixture_db(
    database_url: String,
    network: ArchiveNetwork,
    scope: ArchiveScope,
    start_height: u64,
    count: u64,
) -> anyhow::Result<()> {
    let archive = SqliteArchive::connect(&database_url, true).await?;
    archive.migrate().await?;
    ensure_archive_meta(&archive, network, scope, start_height).await?;

    let profile = Profile {
        scope,
        cutthrough_blocks: 0,
    };

    let db_profile = archive.resolve_profile(None, Some(profile)).await?;

    let (blocks, checkpoint_bytes, checkpoint_height, tip_hash, _last_uid, _live) =
        fixture_blocks(scope, start_height, count)?;

    let mut tx = archive.pool().begin().await?;

    for (height, hash, prev_hash, anchor_last_uid, bytes, stats) in blocks {
        sqlx::query(
            r#"INSERT OR REPLACE INTO blocks
               (height, block_hash, previous_block_hash, p2tr_created_count, p2tr_spent_count, anchor_last_uid)
               VALUES (?, ?, ?, ?, ?, ?)"#,
        )
            .bind(i64::try_from(height)?)
            .bind(hash.as_bytes().to_vec())
            .bind(prev_hash.as_bytes().to_vec())
            .bind(i64::from(stats.indexed_output_count))
            .bind(i64::from(stats.indexed_spent_count))
            .bind(i64::try_from(anchor_last_uid)?)
            .execute(&mut *tx)
            .await?;

        sqlx::query(
            r#"INSERT OR REPLACE INTO block_stats
               (height, tx_count, output_count_total, p2tr_output_count, p2tr_sp_candidate_count,
                p2tr_nums_count, p2tr_reused_count, p2tr_excluded_by_scope_count,
                indexed_output_count, indexed_spent_count, tx_with_p2tr_output_count,
                tx_with_indexed_output_count, tweak_count)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(i64::try_from(height)?)
        .bind(i64::from(stats.tx_count))
        .bind(i64::from(stats.output_count_total))
        .bind(i64::from(stats.p2tr_output_count))
        .bind(i64::from(stats.p2tr_sp_candidate_count))
        .bind(i64::from(stats.p2tr_nums_count))
        .bind(i64::from(stats.p2tr_reused_count))
        .bind(i64::from(stats.p2tr_excluded_by_scope_count))
        .bind(i64::from(stats.indexed_output_count))
        .bind(i64::from(stats.indexed_spent_count))
        .bind(i64::from(stats.tx_with_p2tr_output_count))
        .bind(i64::from(stats.tx_with_indexed_output_count))
        .bind(i64::from(stats.tweak_count))
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"INSERT OR REPLACE INTO payload_cache
               (profile_id, height, block_hash, payload, payload_len, created_at)
               VALUES (?, ?, ?, ?, ?, unixepoch())"#,
        )
        .bind(db_profile.profile_id)
        .bind(i64::try_from(height)?)
        .bind(hash.as_bytes().to_vec())
        .bind(bytes.clone())
        .bind(i64::try_from(bytes.len())?)
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query(
        r#"INSERT OR REPLACE INTO checkpoint_cache
           (profile_id, height, block_hash, checkpoint, checkpoint_len, created_at)
           VALUES (?, ?, ?, ?, ?, unixepoch())"#,
    )
    .bind(db_profile.profile_id)
    .bind(i64::try_from(checkpoint_height)?)
    .bind(tip_hash.as_bytes().to_vec())
    .bind(checkpoint_bytes.clone())
    .bind(i64::try_from(checkpoint_bytes.len())?)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"UPDATE profiles
           SET served_tip_height = ?, served_tip_hash = ?
           WHERE profile_id = ?"#,
    )
    .bind(i64::try_from(checkpoint_height)?)
    .bind(tip_hash.as_bytes().to_vec())
    .bind(db_profile.profile_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    println!(
        "wrote SQLite {} fixture archive to {} starting from height {}",
        scope, database_url, start_height
    );

    Ok(())
}

async fn ensure_archive_meta(
    archive: &SqliteArchive,
    network: ArchiveNetwork,
    scope: ArchiveScope,
    start_height: u64,
) -> anyhow::Result<()> {
    let existing_network = get_meta(archive, "network").await?;
    let existing_scope = get_meta(archive, "scope").await?;
    let existing_start_height = get_meta(archive, "start_height").await?;

    if let Some(value) = existing_network {
        anyhow::ensure!(
            value == network.to_string(),
            "archive network mismatch: db={} requested={}",
            value,
            network
        );
    }

    if let Some(value) = existing_scope {
        anyhow::ensure!(
            value == scope.to_string(),
            "archive scope mismatch: db={} requested={}",
            value,
            scope
        );
    }

    if let Some(value) = existing_start_height {
        anyhow::ensure!(
            value == start_height.to_string(),
            "archive start_height mismatch: db={} requested={}",
            value,
            start_height
        );
    }

    sqlx::query(
        r#"INSERT OR IGNORE INTO meta(key, value)
           VALUES
             ('network', ?),
             ('scope', ?),
             ('start_height', ?)"#,
    )
    .bind(network.to_string())
    .bind(scope.to_string())
    .bind(start_height.to_string())
    .execute(archive.pool())
    .await?;

    Ok(())
}

async fn restore_indexer_state(
    archive: &SqliteArchive,
    start_height: u64,
) -> anyhow::Result<P2trIndexerState> {
    let last_uid = sqlx::query_scalar::<_, i64>(
        r#"SELECT COALESCE(MAX(anchor_last_uid), 0) FROM blocks"#,
    )
    .fetch_one(archive.pool())
    .await?;

    let rows = sqlx::query(
        r#"SELECT txid, vout, uid, value_sat, script_pubkey, p2tr_xonly_key,
                  created_height, created_block_hash, created_tx_index
           FROM p2tr_utxo_lookup
           ORDER BY uid"#,
    )
    .fetch_all(archive.pool())
    .await?;

    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        let txid: Vec<u8> = row.try_get("txid")?;
        let txid: [u8; 32] = txid
            .try_into()
            .map_err(|v: Vec<u8>| anyhow::anyhow!("invalid txid length in p2tr_utxo_lookup: {}", v.len()))?;
        let key: Vec<u8> = row.try_get("p2tr_xonly_key")?;
        let p2tr_xonly_key = key.try_into().map_err(|v: Vec<u8>| {
            anyhow::anyhow!("invalid p2tr_xonly_key length in p2tr_utxo_lookup: {}", v.len())
        })?;
        let vout: i64 = row.try_get("vout")?;
        let uid: i64 = row.try_get("uid")?;
        let value_sat: Option<i64> = row.try_get("value_sat")?;
        let created_height: Option<i64> = row.try_get("created_height")?;
        let created_tx_index: Option<i64> = row.try_get("created_tx_index")?;
        let created_block_hash: Vec<u8> = row.try_get("created_block_hash")?;
        let created_block_hash: [u8; 32] = created_block_hash.try_into().map_err(|v: Vec<u8>| {
            anyhow::anyhow!("invalid created_block_hash length in p2tr_utxo_lookup: {}", v.len())
        })?;
        let script_pubkey: Option<Vec<u8>> = row.try_get("script_pubkey")?;

        entries.push(ScopedUtxoEntry {
            outpoint: OutPointKey {
                txid: TxidBytes::from(txid),
                vout: u32::try_from(vout)?,
            },
            uid: u64::try_from(uid)?,
            created_height: u64::try_from(created_height.unwrap_or(start_height as i64))?,
            created_block_hash: BlockHashBytes::from(created_block_hash),
            tx_index: u32::try_from(created_tx_index.unwrap_or(0))?,
            value_sat: u64::try_from(value_sat.unwrap_or(0))?,
            script_pubkey: script_pubkey.unwrap_or_default(),
            p2tr_xonly_key,
        });
    }

    let seen_rows = sqlx::query(r#"SELECT output_key FROM p2tr_key_stats"#)
        .fetch_all(archive.pool())
        .await?;
    let mut seen_keys = Vec::with_capacity(seen_rows.len());
    for row in seen_rows {
        seen_keys.push(row.try_get::<Vec<u8>, _>("output_key")?);
    }

    Ok(P2trIndexerState::restore(
        u64::try_from(last_uid)?,
        entries,
        seen_keys,
    ))
}


async fn restore_chain_utxo_state(archive: &SqliteArchive) -> anyhow::Result<ChainUtxoState> {
    let rows = sqlx::query(
        r#"SELECT txid, vout, value_sat, script_pubkey,
                  created_height, created_block_hash, created_tx_index
           FROM chain_utxo_lookup"#,
    )
    .fetch_all(archive.pool())
    .await?;

    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        let txid: Vec<u8> = row.try_get("txid")?;
        let txid: [u8; 32] = txid.try_into().map_err(|v: Vec<u8>| {
            anyhow::anyhow!("invalid txid length in chain_utxo_lookup: {}", v.len())
        })?;
        let block_hash: Vec<u8> = row.try_get("created_block_hash")?;
        let block_hash: [u8; 32] = block_hash.try_into().map_err(|v: Vec<u8>| {
            anyhow::anyhow!("invalid created_block_hash length in chain_utxo_lookup: {}", v.len())
        })?;
        let vout: i64 = row.try_get("vout")?;
        let value_sat: i64 = row.try_get("value_sat")?;
        let created_height: i64 = row.try_get("created_height")?;
        let created_tx_index: i64 = row.try_get("created_tx_index")?;

        entries.push(ChainUtxoEntry {
            outpoint: OutPointKey {
                txid: TxidBytes::from(txid),
                vout: u32::try_from(vout)?,
            },
            value_sat: u64::try_from(value_sat)?,
            script_pubkey: row.try_get("script_pubkey")?,
            created_height: u64::try_from(created_height)?,
            created_block_hash: BlockHashBytes::from(block_hash),
            created_tx_index: u32::try_from(created_tx_index)?,
        });
    }

    Ok(ChainUtxoState::restore(entries))
}

async fn get_meta(archive: &SqliteArchive, key: &str) -> anyhow::Result<Option<String>> {
    let value = sqlx::query_scalar::<_, String>(r#"SELECT value FROM meta WHERE key = ?"#)
        .bind(key)
        .fetch_optional(archive.pool())
        .await?;

    Ok(value)
}

async fn next_index_height(archive: &SqliteArchive, start_height: u64) -> anyhow::Result<u64> {
    let indexed_tip =
        sqlx::query_scalar::<_, i64>(r#"SELECT COALESCE(MAX(height), -1) FROM blocks"#)
            .fetch_one(archive.pool())
            .await?;

    if indexed_tip < 0 {
        Ok(start_height)
    } else {
        Ok(u64::try_from(indexed_tip)? + 1)
    }
}

fn deterministic_txid(height: u64, n: u8) -> TxidBytes {
    let mut txid = [n; 32];
    txid[..8].copy_from_slice(&height.to_le_bytes());
    txid[8] = n;
    TxidBytes::from(txid)
}
