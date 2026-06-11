use bitcoin::consensus::encode::deserialize;
use bitcoin::hashes::Hash as BitcoinHash;
use bitcoin::Block;
use btc_data_core::block::{BlockSpentTxOuts, SpentTxOut};
use btc_data_core::source::{BlockSource, TipWatcher};
use btc_data_light_server::index::{
    encode_light_block, encode_uid_checkpoint, to_packed_bytes, UidCheckpointInput,
};
use btc_data_light_server::p2tr_indexer::{
    BlockScanInput, BlockScopeStats, OutPointKey, P2trIndexerState, ScopedUtxoEntry, TxInputScan,
    TxOutputScan, TxScanInput,
};
use btc_data_light_server::profile::{ArchiveNetwork, ArchiveScope, Profile};
use btc_data_light_server::script_classify::{classify_script, extract_p2tr_xonly, ScriptKind};
use btc_data_light_server::sp_tweak::{compute_tx_scan_point, PrevoutInfo, PrevoutScript, ScanPointStatus, TxInputContext};
use btc_data_light_server::storage::{
    ArchiveBackend, ChainTip, FileArchive, Manifest, ManifestProfile, SqliteArchive,
};
use btc_data_light_server::types::{BlockHashBytes, TxTweak, TxidBytes};
use btc_data_sources::{IpcSource, PollingTipWatcher, RestSource};
use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};
use futures::{stream, StreamExt, TryStreamExt};
use sqlx::Row;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

const DEFAULT_CATCHUP_BATCH_SIZE: usize = 16;
const DEFAULT_FLUSH_BLOCKS: usize = 128;
const DEFAULT_MAX_BUFFERED_RAW_MB: usize = 256;
const DEFAULT_INDEX_P2TR_KEY_STATS: bool = false;

fn mb_to_bytes(mb: usize) -> usize {
    mb.saturating_mul(1024).saturating_mul(1024)
}

#[derive(Debug, Default)]
struct SqlTiming {
    core_cache_ms: u128,
    p2tr_output_ms: u128,
    p2tr_utxo_insert_ms: u128,
    p2tr_key_stats_ms: u128,
    p2tr_spend_ms: u128,
    p2tr_utxo_delete_ms: u128,
    tx_tweak_ms: u128,
    checkpoint_profile_ms: u128,
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
    /// Bitcoin Core REST source.
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

    /// Bitcoin Core REST base URL. Required for --source rest blocks and for --source ipc undo data.
    #[arg(long)]
    rest_url: Option<String>,

    /// Polling fallback interval in seconds. Only used by REST/fallback watchers.
    #[arg(long, default_value_t = 10)]
    poll_interval_secs: u64,
}

struct SourceBundle {
    source: Arc<dyn BlockSource>,
    watcher: Arc<dyn TipWatcher>,
    undo_source: Arc<dyn BlockSource>,
}

impl SourceCli {
    fn build_source(&self) -> anyhow::Result<Arc<dyn BlockSource>> {
        Ok(self.build_bundle()?.source)
    }

    fn build_bundle(&self) -> anyhow::Result<SourceBundle> {
        match self.source {
            SourceKind::Ipc => self.build_ipc_bundle(),
            SourceKind::Rest => {
                let rest_url = self.rest_url.clone().ok_or_else(|| {
                    anyhow::anyhow!("--rest-url is required when --source rest is selected")
                })?;
                let source: Arc<dyn BlockSource> = Arc::new(RestSource::new(rest_url));
                let watcher: Arc<dyn TipWatcher> = Arc::new(PollingTipWatcher::new(
                    source.clone(),
                    Duration::from_secs(self.poll_interval_secs),
                ));
                Ok(SourceBundle {
                    source: source.clone(),
                    watcher,
                    undo_source: source,
                })
            }
        }
    }

    fn build_ipc_bundle(&self) -> anyhow::Result<SourceBundle> {
        let socket = self.ipc_socket.clone().ok_or_else(|| {
            anyhow::anyhow!("--ipc-socket is required when --source ipc is selected")
        })?;

        let rest_url = self.rest_url.clone().ok_or_else(|| {
            anyhow::anyhow!("--rest-url is required when --source ipc is selected because undo data is fetched from /rest/spenttxouts")
        })?;

        let source: Arc<IpcSource> =
            Arc::new(IpcSource::connect_with_threads(socket, self.ipc_threads)?);
        let undo_source: Arc<dyn BlockSource> = Arc::new(RestSource::new(rest_url));

        Ok(SourceBundle {
            source: source.clone(),
            watcher: source,
            undo_source,
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

        #[arg(long, default_value_t = 6)]
        finality_depth: u64,

        /// Number of blocks fetched and decoded per source request.
        #[arg(long, default_value_t = DEFAULT_CATCHUP_BATCH_SIZE)]
        catchup_batch_size: usize,

        /// Maximum number of applied blocks kept in RAM before one atomic SQLite flush.
        /// The indexer may flush earlier when the raw bytes fetched since the
        /// last commit exceed --max-buffered-raw-mb. Larger values reduce
        /// write/commit overhead but increase crash rework and peak memory.
        #[arg(long, default_value_t = DEFAULT_FLUSH_BLOCKS)]
        flush_blocks: usize,

        /// Byte cap for source data fetched since the last SQLite flush. This
        /// bounds peak RAM even when --flush-blocks is large. The cap is checked
        /// after each source chunk, so peak raw bytes can exceed it by one chunk.
        #[arg(long, default_value_t = DEFAULT_MAX_BUFFERED_RAW_MB)]
        max_buffered_raw_mb: usize,

        /// Maintain historical output-key reuse/debug counters in SQLite.
        /// Disabled by default because it is not needed for served light payload
        /// correctness and is very expensive during high-P2TR ranges.
        #[arg(long, default_value_t = DEFAULT_INDEX_P2TR_KEY_STATS)]
        index_p2tr_key_stats: bool,

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
            finality_depth,
            catchup_batch_size,
            flush_blocks,
            max_buffered_raw_mb,
            index_p2tr_key_stats,
            once,
        } => {
            let emit_start_height = scope.default_start_height(network);
            run_command(
                source,
                database_url,
                network,
                scope,
                emit_start_height,
                finality_depth,
                catchup_batch_size,
                flush_blocks,
                mb_to_bytes(max_buffered_raw_mb),
                index_p2tr_key_stats,
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

async fn configure_indexer_sqlite(archive: &SqliteArchive) -> anyhow::Result<()> {
    // Indexer writes are already committed at finalized range boundaries. WAL +
    // NORMAL keeps crash consistency while avoiding a full fsync-style penalty
    // for every statement inside the large transaction. Temp tables are heavily
    // used for temporary indexer work.
    sqlx::query("PRAGMA journal_mode = WAL")
        .execute(archive.pool())
        .await?;
    sqlx::query("PRAGMA synchronous = NORMAL")
        .execute(archive.pool())
        .await?;
    sqlx::query("PRAGMA temp_store = MEMORY")
        .execute(archive.pool())
        .await?;
    sqlx::query("PRAGMA busy_timeout = 5000")
        .execute(archive.pool())
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_command(
    source: SourceCli,
    database_url: String,
    network: ArchiveNetwork,
    scope: ArchiveScope,
    emit_start_height: u64,
    finality_depth: u64,
    catchup_batch_size: usize,
    flush_blocks: usize,
    max_buffered_raw_bytes: usize,
    index_p2tr_key_stats: bool,
    once: bool,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        catchup_batch_size > 0,
        "catchup_batch_size must be greater than zero"
    );
    anyhow::ensure!(flush_blocks > 0, "flush_blocks must be greater than zero");
    anyhow::ensure!(
        max_buffered_raw_bytes > 0,
        "max_buffered_raw_mb must be greater than zero"
    );

    let bundle = source.build_bundle()?;
    anyhow::ensure!(
        bundle.undo_source.supports_block_spent_txouts(),
        "light-indexer requires a REST undo source with /rest/spenttxouts enabled"
    );
    let archive = SqliteArchive::connect(&database_url, true).await?;
    configure_indexer_sqlite(&archive).await?;
    archive.migrate().await?;
    ensure_archive_meta(&archive, network, scope, emit_start_height).await?;

    let profile = Profile {
        scope,
        cutthrough_blocks: 0,
    };
    let db_profile = archive.resolve_profile(None, Some(profile)).await?;

    validate_resume_boundary(&archive, db_profile.profile_id, emit_start_height).await?;
    let profile_next_height =
        next_index_height(&archive, db_profile.profile_id, emit_start_height).await?;
    let committed_profile_tip = profile_next_height
        .checked_sub(1)
        .filter(|h| *h >= emit_start_height);
    let mut state = restore_indexer_state(&archive, committed_profile_tip).await?;
    let initial_next_height = profile_next_height;
    println!(
        "restored indexer state profile_tip={} next_height={} emit_start_height={} last_uid={} live_p2tr_utxos={} fetch_blocks={} flush_blocks={} decode_concurrency={} max_buffered_raw_mb={} index_p2tr_key_stats={}",
        committed_profile_tip
            .map(|h| h.to_string())
            .unwrap_or_else(|| "none".to_string()),
        initial_next_height,
        emit_start_height,
        state.last_uid(),
        state.live_uids_sorted().len(),
        catchup_batch_size,
        flush_blocks,
        max_buffered_raw_bytes / 1024 / 1024,
        index_p2tr_key_stats,
    );

    let mut chain_next_height = initial_next_height;

    loop {
        let best_height = bundle.source.get_best_height().await?;
        let best_hash = bundle.source.get_block_hash(best_height).await?;
        let finalized_tip = best_height.saturating_sub(finality_depth);
        let next_height = chain_next_height;

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
            chain_next_height = catch_up_ranges(
                &archive,
                bundle.source.as_ref(),
                bundle.undo_source.as_ref(),
                &mut state,
                CatchUpConfig {
                    profile,
                    profile_id: db_profile.profile_id,
                    emit_start_height,
                    fetch_batch_size: catchup_batch_size,
                    flush_blocks,
                    max_buffered_raw_bytes,
                    index_p2tr_key_stats,
                },
                next_height,
                finalized_tip,
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

const SQL_INSERT_CHUNK: usize = 128;
const SQL_UTXO_MUTATION_CHUNK: usize = 512;

// NOTE: source chunks are fetched/decoded in bounded batches, then compact
// applied light data is buffered until the SQLite flush boundary. Silent
// Payment tweaks are computed directly from Bitcoin Core undo data returned
// by /rest/spenttxouts, so no mutable prevout store is needed.
/// Bundles the per-run settings for [`catch_up_ranges`], replacing a long
/// positional argument list. Handles (archive/source/state) and the
/// dynamic range bounds stay as direct parameters.
struct CatchUpConfig {
    profile: Profile,
    profile_id: i64,
    emit_start_height: u64,
    fetch_batch_size: usize,
    flush_blocks: usize,
    max_buffered_raw_bytes: usize,
    index_p2tr_key_stats: bool,
}

async fn catch_up_ranges(
    archive: &SqliteArchive,
    source: &dyn BlockSource,
    undo_source: &dyn BlockSource,
    state: &mut P2trIndexerState,
    config: CatchUpConfig,
    start_height: u64,
    finalized_tip: u64,
) -> anyhow::Result<u64> {
    let CatchUpConfig {
        profile,
        profile_id,
        emit_start_height,
        fetch_batch_size,
        flush_blocks,
        max_buffered_raw_bytes,
        index_p2tr_key_stats,
    } = config;
    let remaining = finalized_tip - start_height + 1;
    let flush_count = usize::try_from(remaining.min(flush_blocks as u64))?;
    let flush_end_height = start_height + u64::try_from(flush_count)? - 1;

    let range_started = Instant::now();

    struct Pending {
        applied: btc_data_light_server::p2tr_indexer::AppliedBlock,
        payload: Vec<u8>,
    }

    let first_height = start_height;
    let mut last_height = None;
    let mut last_hash: Option<BlockHashBytes> = None;
    let mut total_bytes = 0usize;
    let mut total_fetch_elapsed = Duration::ZERO;
    let mut total_decode_elapsed = Duration::ZERO;
    let mut total_apply_elapsed = Duration::ZERO;
    let mut pending: Vec<Pending> = Vec::with_capacity(flush_count);

    // Seeded from the committed profile tip so the first chunk's parent linkage
    // is checked against the last served block, not against itself.
    let mut previous_tip_hash: Option<BlockHashBytes> = committed_tip_hash(archive, profile_id).await?;
    let mut chunk_start = start_height;
    while chunk_start <= flush_end_height {
        let chunk_remaining = flush_end_height - chunk_start + 1;
        let chunk_count = usize::try_from(chunk_remaining.min(fetch_batch_size as u64))?;

        // Stage 1: fetch + decode this bounded source chunk. The helper consumes
        // the raw frames instead of cloning their byte buffers, keeping peak
        // memory bounded to one source chunk plus its decoded block inputs.
        let chunk = fetch_and_decode_chunk(
            source,
            undo_source,
            chunk_start,
            chunk_count,
        )
        .await?;
        total_fetch_elapsed += chunk.fetch_elapsed;
        total_decode_elapsed += chunk.decode_elapsed;
        total_bytes += chunk.raw_bytes;
        let decoded = chunk.blocks;

        validate_decoded_chunk(&decoded, chunk_start, previous_tip_hash)?;

        let chunk_first_height = decoded.first().expect("non-empty").height;
        let chunk_last = decoded.last().expect("non-empty");
        last_height = Some(chunk_last.height);
        last_hash = Some(chunk_last.block_hash);
        previous_tip_hash = Some(chunk_last.block_hash);

        println!(
            "fetched finalized chunk {}..={} count={} bytes={} flush_target={}..={} buffered_blocks={}",
            chunk_first_height,
            last_height.expect("last height set"),
            decoded.len(),
            chunk.raw_bytes,
            first_height,
            flush_end_height,
            pending.len()
        );

        // Stage 2a: serial, ordered apply. State advances in memory until the
        // flush transaction commits. A crash before commit will reprocess from
        // the last committed profile tip.
        let apply_started = Instant::now();
        for scan in decoded {
            let emit_light_payload = scan.height >= emit_start_height;
            if emit_light_payload {
                let applied = state.apply_block_with_stats(scan, profile)?;
                let payload = to_packed_bytes(&encode_light_block(&applied.light_block)?)?;
                pending.push(Pending { applied, payload });
            }
        }
        total_apply_elapsed += apply_started.elapsed();

        let chunk_last_height = last_height.expect("last height set");
        chunk_start = chunk_last_height + 1;

        if total_bytes >= max_buffered_raw_bytes && chunk_start <= flush_end_height {
            println!(
                "raw-byte flush cap reached at height {} buffered_raw_bytes={} cap_bytes={} pending_blocks={}",
                chunk_last_height,
                total_bytes,
                max_buffered_raw_bytes,
                pending.len()
            );
            break;
        }
    }

    let last_height = last_height.expect("last height set");
    let last_hash = last_hash.expect("last hash set");

    // Stage 3: batched multi-row inserts in one transaction.
    let sql_started = Instant::now();
    let mut sql_timing = SqlTiming::default();
    let mut tx = archive.pool().begin().await?;

    let core_cache_started = Instant::now();
    for chunk in pending.chunks(SQL_INSERT_CHUNK) {
        {
            let mut qb = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
                "INSERT INTO blocks \
                 (height, block_hash, previous_block_hash, p2tr_created_count, p2tr_spent_count, anchor_last_uid) ",
            );
            qb.push_values(chunk, |mut b, p| {
                let block = &p.applied.light_block;
                let stats = &p.applied.stats;
                b.push_bind(i64::try_from(block.height).expect("height fits in i64"))
                    .push_bind(block.block_hash.as_bytes().to_vec())
                    .push_bind(block.previous_block_hash.as_bytes().to_vec())
                    .push_bind(i64::from(stats.indexed_output_count))
                    .push_bind(i64::from(stats.indexed_spent_count))
                    .push_bind(
                        i64::try_from(block.block_anchor_last_uid).expect("UID fits in i64"),
                    );
            });
            qb.build().execute(&mut *tx).await?;
        }
        {
            let mut qb = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
                "INSERT INTO block_stats \
                 (height, tx_count, output_count_total, p2tr_output_count, p2tr_sp_candidate_count, \
                  p2tr_nums_count, p2tr_reused_count, p2tr_excluded_by_scope_count, \
                  indexed_output_count, indexed_spent_count, tx_with_p2tr_output_count, \
                  tx_with_indexed_output_count, tweak_count) ",
            );
            qb.push_values(chunk, |mut b, p| {
                let block = &p.applied.light_block;
                let s = &p.applied.stats;
                b.push_bind(i64::try_from(block.height).expect("height fits in i64"))
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
                "INSERT INTO payload_cache \
                 (profile_id, height, block_hash, payload, payload_len, created_at) ",
            );
            qb.push_values(chunk, |mut b, p| {
                let block = &p.applied.light_block;
                b.push_bind(profile_id)
                    .push_bind(i64::try_from(block.height).expect("height fits in i64"))
                    .push_bind(block.block_hash.as_bytes().to_vec())
                    .push_bind(p.payload.clone())
                    .push_bind(i64::try_from(p.payload.len()).expect("payload length fits in i64"))
                    .push("unixepoch()");
            });
            qb.build().execute(&mut *tx).await?;
        }
    }
    sql_timing.core_cache_ms = core_cache_started.elapsed().as_millis();

    // Stage 4: chain UTXO state is already applied in memory. Do not mirror it
    // into SQLite's hot path; it is persisted as a binary checkpoint just before
    // the SQL transaction commits and advances the served profile tip.

    let p2tr_created = pending
        .iter()
        .flat_map(|p| p.applied.created_utxos.iter())
        .collect::<Vec<_>>();
    let p2tr_output_started = Instant::now();
    for chunk in p2tr_created.chunks(SQL_UTXO_MUTATION_CHUNK) {
        let mut qb = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
            r#"INSERT INTO p2tr_outputs
               (uid, created_height, tx_index, vout, value_sat, is_nums, is_reused,
                reuse_count_at_creation, txid, script_pubkey, p2tr_xonly_key) "#,
        );
        qb.push_values(chunk, |mut b, created| {
            let entry = &created.entry;
            b.push_bind(i64::try_from(entry.uid).expect("uid fits in i64"))
                .push_bind(i64::try_from(entry.created_height).expect("height fits in i64"))
                .push_bind(i64::from(entry.tx_index))
                .push_bind(i64::from(entry.outpoint.vout))
                .push_bind(i64::try_from(entry.value_sat).expect("value_sat fits in i64"))
                .push_bind(if created.is_nums { 1_i64 } else { 0_i64 })
                .push_bind(if created.is_reused { 1_i64 } else { 0_i64 })
                .push_bind(
                    i64::try_from(created.reuse_count_at_creation)
                        .expect("reuse_count_at_creation fits in i64"),
                )
                .push_bind(entry.outpoint.txid.as_bytes().to_vec())
                .push_bind(entry.script_pubkey.clone())
                .push_bind(entry.p2tr_xonly_key.to_vec());
        });
        qb.build().execute(&mut *tx).await?;
    }
    sql_timing.p2tr_output_ms = p2tr_output_started.elapsed().as_millis();

    // p2tr_utxo_lookup removed: it was only a durability mirror of the in-RAM
    // live UID set. That set is rebuilt at startup from p2tr_outputs LEFT JOIN
    // p2tr_spends (authoritative, written in this same transaction), so the
    // per-range insert/delete churn on a 16M-row WITHOUT ROWID table is gone.
    sql_timing.p2tr_utxo_insert_ms = 0;

    if index_p2tr_key_stats {
        let p2tr_key_stats_started = Instant::now();
        for chunk in p2tr_created.chunks(SQL_UTXO_MUTATION_CHUNK) {
            let mut qb = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
                r#"INSERT INTO p2tr_key_stats
                   (output_key, first_height, last_height, seen_count, first_uid, last_uid, is_nums) "#,
            );
            qb.push_values(chunk, |mut b, created| {
                let entry = &created.entry;
                b.push_bind(entry.p2tr_xonly_key.to_vec())
                    .push_bind(i64::try_from(entry.created_height).expect("height fits in i64"))
                    .push_bind(i64::try_from(entry.created_height).expect("height fits in i64"))
                    .push_bind(1_i64)
                    .push_bind(i64::try_from(entry.uid).expect("uid fits in i64"))
                    .push_bind(i64::try_from(entry.uid).expect("uid fits in i64"))
                    .push_bind(if created.is_nums { 1_i64 } else { 0_i64 });
            });
            qb.push(
                r#" ON CONFLICT(output_key) DO UPDATE SET
                     last_height = excluded.last_height,
                     seen_count = p2tr_key_stats.seen_count + 1,
                     last_uid = excluded.last_uid,
                     is_nums = CASE WHEN p2tr_key_stats.is_nums != 0 OR excluded.is_nums != 0 THEN 1 ELSE 0 END"#,
            );
            qb.build().execute(&mut *tx).await?;
        }
        sql_timing.p2tr_key_stats_ms = p2tr_key_stats_started.elapsed().as_millis();
    }

    let p2tr_spent = pending
        .iter()
        .flat_map(|p| p.applied.spent_utxos.iter())
        .collect::<Vec<_>>();
    let p2tr_spend_started = Instant::now();
    for chunk in p2tr_spent.chunks(SQL_UTXO_MUTATION_CHUNK) {
        let mut qb = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
            r#"INSERT INTO p2tr_spends
               (uid, spent_height, spent_block_hash, spend_tx_index) "#,
        );
        qb.push_values(chunk, |mut b, spent| {
            let entry = &spent.entry;
            b.push_bind(i64::try_from(entry.uid).expect("uid fits in i64"))
                .push_bind(i64::try_from(spent.spent_height).expect("height fits in i64"))
                .push_bind(spent.spent_block_hash.as_bytes().to_vec())
                .push_bind(i64::from(spent.spend_tx_index));
        });
        qb.build().execute(&mut *tx).await?;
    }
    sql_timing.p2tr_spend_ms = p2tr_spend_started.elapsed().as_millis();

    // See note above: no p2tr_utxo_lookup table to delete spent rows from.
    sql_timing.p2tr_utxo_delete_ms = 0;

    let tx_tweak_started = Instant::now();
    let tx_tweaks = pending
        .iter()
        .flat_map(|p| {
            let block = &p.applied.light_block;
            block
                .tx_tweak_indexes
                .iter()
                .copied()
                .zip(block.tx_tweaks.iter().copied())
                .map(move |(tx_index, tweak)| (block.height, tx_index, tweak))
        })
        .collect::<Vec<_>>();
    for chunk in tx_tweaks.chunks(SQL_UTXO_MUTATION_CHUNK) {
        let mut qb = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
            r#"INSERT INTO tx_tweaks
               (height, tx_index, tweak) "#,
        );
        qb.push_values(chunk, |mut b, (height, tx_index, tweak)| {
            b.push_bind(i64::try_from(*height).expect("height fits in i64"))
                .push_bind(i64::from(*tx_index))
                .push_bind(tweak.as_bytes().to_vec());
        });
        qb.build().execute(&mut *tx).await?;
    }
    sql_timing.tx_tweak_ms = tx_tweak_started.elapsed().as_millis();

    // checkpoint + profile tip: once per emitted range. During genesis warmup
    // before the archive scope begins, only the UTXO checkpoint is advanced.
    let checkpoint_profile_started = Instant::now();
    let checkpoint_hash = last_hash;
    let checkpoint_bytes = if pending.is_empty() {
        Vec::new()
    } else {
        let checkpoint = UidCheckpointInput {
            height: last_height,
            block_hash: checkpoint_hash,
            last_uid: state.last_uid(),
            profile,
            unspent_uids_sorted: state.live_uids_sorted(),
        };
        let checkpoint_bytes = to_packed_bytes(&encode_uid_checkpoint(&checkpoint)?)?;

        sqlx::query(
            r#"INSERT INTO checkpoint_cache
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

        checkpoint_bytes
    };
    sql_timing.checkpoint_profile_ms = checkpoint_profile_started.elapsed().as_millis();

    // SQLite is now the only durable archive state. Undo data is fetched from
    // Bitcoin Core per block, so there is no secondary prevout database to keep
    // in sync with the served profile tip.
    let commit_started = Instant::now();
    tx.commit().await?;
    let commit_elapsed = commit_started.elapsed();


    let sql_elapsed = sql_started.elapsed();
    let total_elapsed = range_started.elapsed();

    let mut totals = BlockScopeStats::default();
    let mut payload_bytes = 0usize;
    for p in &pending {
        let s = &p.applied.stats;
        totals.tx_count += s.tx_count;
        totals.output_count_total += s.output_count_total;
        totals.p2tr_output_count += s.p2tr_output_count;
        totals.p2tr_sp_candidate_count += s.p2tr_sp_candidate_count;
        totals.p2tr_nums_count += s.p2tr_nums_count;
        totals.p2tr_reused_count += s.p2tr_reused_count;
        totals.p2tr_excluded_by_scope_count += s.p2tr_excluded_by_scope_count;
        totals.indexed_output_count += s.indexed_output_count;
        totals.indexed_spent_count += s.indexed_spent_count;
        totals.tx_with_p2tr_output_count += s.tx_with_p2tr_output_count;
        totals.tx_with_indexed_output_count += s.tx_with_indexed_output_count;
        totals.tweak_count += s.tweak_count;
        payload_bytes += p.payload.len();
    }

    let processed_blocks = last_height - first_height + 1;

    println!(
        "indexed finalized range {}..={} blocks={} served_blocks={} flush_blocks={} decode_concurrency={} max_buffered_raw_bytes={} raw_bytes={} payload_bytes={} checkpoint_bytes={} txs={} outputs={} p2tr_outputs={} indexed_created={} indexed_spent={} tx_tweaks={} last_uid={} live_uids={} fetch_ms={} decode_ms={} apply_ms={} sql_ms={} sql_core_cache_ms={} sql_p2tr_output_ms={} sql_p2tr_utxo_insert_ms={} sql_p2tr_key_stats_ms={} sql_p2tr_spend_ms={} sql_p2tr_utxo_delete_ms={} sql_tx_tweak_ms={} sql_checkpoint_profile_ms={} commit_ms={} total_ms={}",
        first_height,
        last_height,
        processed_blocks,
        pending.len(),
        flush_blocks,
        max_buffered_raw_bytes,
        total_bytes,
        payload_bytes,
        checkpoint_bytes.len(),
        totals.tx_count,
        totals.output_count_total,
        totals.p2tr_output_count,
        totals.indexed_output_count,
        totals.indexed_spent_count,
        totals.tweak_count,
        state.last_uid(),
        state.live_uid_count(),
        total_fetch_elapsed.as_millis(),
        total_decode_elapsed.as_millis(),
        total_apply_elapsed.as_millis(),
        sql_elapsed.as_millis(),
        sql_timing.core_cache_ms,
        sql_timing.p2tr_output_ms,
        sql_timing.p2tr_utxo_insert_ms,
        sql_timing.p2tr_key_stats_ms,
        sql_timing.p2tr_spend_ms,
        sql_timing.p2tr_utxo_delete_ms,
        sql_timing.tx_tweak_ms,
        sql_timing.checkpoint_profile_ms,
        commit_elapsed.as_millis(),
        total_elapsed.as_millis(),
    );

    Ok(last_height + 1)
}


async fn committed_tip_hash(
    archive: &SqliteArchive,
    profile_id: i64,
) -> anyhow::Result<Option<BlockHashBytes>> {
    let row = sqlx::query(
        r#"SELECT served_tip_hash
           FROM profiles
           WHERE profile_id = ? AND served_tip_hash IS NOT NULL"#,
    )
    .bind(profile_id)
    .fetch_optional(archive.pool())
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };
    let bytes: Vec<u8> = row.try_get("served_tip_hash")?;
    anyhow::ensure!(
        bytes.len() == 32,
        "profile {profile_id} has invalid served_tip_hash length {}",
        bytes.len()
    );
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&bytes);
    Ok(Some(BlockHashBytes::from(hash)))
}

/// One fetched-and-decoded source chunk plus the fetch/decode telemetry the
/// caller needs for range logging. Frames are consumed during decode so peak
/// memory stays at one source chunk's worth of bytes.
struct DecodedChunk {
    blocks: Vec<BlockScanInput>,
    raw_bytes: usize,
    fetch_elapsed: Duration,
    decode_elapsed: Duration,
}

async fn fetch_and_decode_chunk(
    source: &dyn BlockSource,
    undo_source: &dyn BlockSource,
    chunk_start: u64,
    chunk_count: usize,
) -> anyhow::Result<DecodedChunk> {
    let fetch_started = Instant::now();
    let mut frames = source
        .get_block_range_by_height(chunk_start, chunk_count)
        .await?;
    anyhow::ensure!(
        !frames.is_empty(),
        "source returned an empty block range at height {chunk_start}"
    );

    // REST frames already carry undo data. IPC frames carry block bytes only,
    // so attach missing undo data here before decode. The decoder only sees
    // completed frames and no longer performs a second undo pass.
    let missing_undo = frames
        .iter()
        .enumerate()
        .filter_map(|(index, frame)| frame.spent_txouts.is_none().then_some((index, frame.hash)))
        .collect::<Vec<_>>();
    if !missing_undo.is_empty() {
        let hashes = missing_undo
            .iter()
            .map(|(_, hash)| *hash)
            .collect::<Vec<_>>();
        let fetched = undo_source.get_blocks_spent_txouts(&hashes).await?;
        anyhow::ensure!(
            fetched.len() == missing_undo.len(),
            "undo source returned {} spenttxouts entries for {} missing blocks",
            fetched.len(),
            missing_undo.len()
        );
        for ((index, _), undo) in missing_undo.into_iter().zip(fetched) {
            frames[index].spent_txouts = undo;
        }
    }

    let fetch_elapsed = fetch_started.elapsed();
    let raw_bytes: usize = frames.iter().map(|frame| frame.bytes.len()).sum();

    let decode_started = Instant::now();
    let mut blocks: Vec<BlockScanInput> = stream::iter(frames.into_iter())
        .map(|frame| {
            let height = frame.height;
            let hash = frame.hash;
            let bytes = frame.bytes;
            let spent_txouts = frame.spent_txouts;
            async move {
                let spent_txouts = spent_txouts.ok_or_else(|| {
                    anyhow::anyhow!(
                        "undo source did not return spenttxouts for block {} at height {}",
                        display_hash(hash),
                        height
                    )
                })?;
                tokio::task::spawn_blocking(move || {
                    decode_block_frame(height, hash, bytes.as_ref(), Some(&spent_txouts))
                })
                .await
                .map_err(|e| anyhow::anyhow!("decode worker failed: {e}"))?
            }
        })
        .buffer_unordered(chunk_count)
        .collect::<Vec<anyhow::Result<BlockScanInput>>>()
        .await
        .into_iter()
        .collect::<anyhow::Result<Vec<_>>>()?;
    blocks.sort_by_key(|block| block.height);
    let decode_elapsed = decode_started.elapsed();

    Ok(DecodedChunk {
        blocks,
        raw_bytes,
        fetch_elapsed,
        decode_elapsed,
    })
}

fn validate_decoded_chunk(
    decoded: &[BlockScanInput],
    expected_start_height: u64,
    expected_previous_hash: Option<BlockHashBytes>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !decoded.is_empty(),
        "decoded block chunk at height {expected_start_height} is empty"
    );

    let first = &decoded[0];
    anyhow::ensure!(
        first.height == expected_start_height,
        "decoded block chunk starts at height {}, expected {expected_start_height}",
        first.height
    );

    if let Some(expected_previous_hash) = expected_previous_hash {
        anyhow::ensure!(
            first.previous_block_hash == expected_previous_hash,
            "decoded block at height {} does not connect to previous chunk tip",
            first.height
        );
    }

    for pair in decoded.windows(2) {
        let prev = &pair[0];
        let current = &pair[1];
        anyhow::ensure!(
            current.height == prev.height + 1,
            "decoded block heights are not contiguous: {} followed by {}",
            prev.height,
            current.height
        );
        anyhow::ensure!(
            current.previous_block_hash == prev.block_hash,
            "decoded block at height {} does not connect to height {}",
            current.height,
            prev.height
        );
    }

    Ok(())
}

fn decode_block_frame(
    height: u64,
    source_hash: [u8; 32],
    bytes: &[u8],
    spent_txouts: Option<&BlockSpentTxOuts>,
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

        let silent_payment_tweak = compute_sp_tweak_from_undo(tx_index, &inputs, &outputs, spent_txouts)?;

        txs.push(TxScanInput {
            txid,
            tx_index,
            inputs,
            outputs,
            // Filled directly from Bitcoin Core undo data. Missing undo data is
            // an indexer error, not a signal to use a local prevout database.
            silent_payment_tweak,
        });
    }

    Ok(BlockScanInput {
        height,
        block_hash: BlockHashBytes::from(source_hash),
        previous_block_hash: BlockHashBytes::from(block.header.prev_blockhash.to_byte_array()),
        txs,
    })
}

fn compute_sp_tweak_from_undo(
    tx_index: u32,
    inputs: &[TxInputScan],
    outputs: &[TxOutputScan],
    spent_txouts: Option<&BlockSpentTxOuts>,
) -> anyhow::Result<Option<TxTweak>> {
    // Do not touch undo data unless this transaction creates at least one
    // Taproot output. Only those transactions can need a served SP tweak.
    if !outputs.iter().any(|output| output.is_p2tr) {
        return Ok(None);
    }
    if tx_index == 0 {
        return Ok(None);
    }
    let spent_txouts = spent_txouts.ok_or_else(|| {
        anyhow::anyhow!(
            "missing spenttxouts undo data for tx_index={tx_index}; light-indexer requires /rest/spenttxouts"
        )
    })?;

    let undo_tx_index = usize::try_from(tx_index)?;
    let undo_inputs = spent_txouts.txs.get(undo_tx_index).ok_or_else(|| {
        anyhow::anyhow!(
            "spenttxouts missing tx entry for tx_index={tx_index}"
        )
    })?;
    anyhow::ensure!(
        undo_inputs.len() == inputs.len(),
        "spenttxouts input count mismatch for tx_index={tx_index}: undo={} tx={}",
        undo_inputs.len(),
        inputs.len()
    );

    let mut input_context = Vec::with_capacity(inputs.len());
    for (input, prevout) in inputs.iter().zip(undo_inputs) {
        input_context.push(TxInputContext {
            previous_output: input.previous_output,
            script_sig: input.script_sig.clone(),
            witness: input.witness.clone(),
            prevout: Some(prevout_info_from_spent_txout(prevout)?),
        });
    }

    match compute_tx_scan_point(&input_context)? {
        ScanPointStatus::Computed(tweak) => Ok(Some(tweak)),
        ScanPointStatus::Ineligible | ScanPointStatus::MissingPrevout { .. } => Ok(None),
    }
}

fn prevout_info_from_spent_txout(prevout: &SpentTxOut) -> anyhow::Result<PrevoutInfo> {
    let script = match classify_script(&prevout.script_pubkey) {
        ScriptKind::P2pkh { hash160 } => PrevoutScript::P2pkh { hash160 },
        ScriptKind::P2sh { hash160 } => PrevoutScript::P2sh { hash160 },
        ScriptKind::P2wpkh { hash160 } => PrevoutScript::P2wpkh { hash160 },
        ScriptKind::P2tr { xonly_key } => PrevoutScript::P2tr {
            xonly_key: bitcoin::secp256k1::XOnlyPublicKey::from_slice(&xonly_key)?,
        },
        ScriptKind::WitnessUnknown { version, .. } if version >= 2 => {
            PrevoutScript::WitnessUnknown { version }
        }
        ScriptKind::P2wsh { .. }
        | ScriptKind::WitnessUnknown { .. }
        | ScriptKind::OpReturn
        | ScriptKind::Other => PrevoutScript::Other,
    };
    Ok(PrevoutInfo { script })
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
        cutthrough_snapshots: Vec::new(),
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
    configure_indexer_sqlite(&archive).await?;
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
            r#"INSERT INTO blocks
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
            r#"INSERT INTO block_stats
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
            r#"INSERT INTO payload_cache
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
        r#"INSERT INTO checkpoint_cache
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
    committed_tip: Option<u64>,
) -> anyhow::Result<P2trIndexerState> {
    let last_uid = if let Some(height) = committed_tip {
        let value = sqlx::query_scalar::<_, Option<i64>>(
            r#"SELECT anchor_last_uid FROM blocks WHERE height = ?"#,
        )
        .bind(i64::try_from(height)?)
        .fetch_one(archive.pool())
        .await?;
        value
            .ok_or_else(|| anyhow::anyhow!("missing committed tip block row at height {height}"))?
    } else {
        0
    };

    // Rebuild the live UID set from the authoritative archive tables: every
    // created P2TR output that has no matching spend row is still live. This
    // replaces the former p2tr_utxo_lookup mirror table.
    let mut rows = sqlx::query(
        r#"SELECT o.txid, o.vout, o.uid, o.value_sat, o.script_pubkey, o.p2tr_xonly_key,
                  o.created_height, b.block_hash AS created_block_hash, o.tx_index AS created_tx_index
           FROM p2tr_outputs o
           JOIN blocks b ON b.height = o.created_height
           LEFT JOIN p2tr_spends s ON s.uid = o.uid
           WHERE s.uid IS NULL
           ORDER BY o.uid"#,
    )
    .fetch(archive.pool());

    let mut entries = Vec::new();
    while let Some(row) = rows.try_next().await? {
        let txid: Vec<u8> = row.try_get("txid")?;
        let txid: [u8; 32] = txid.try_into().map_err(|v: Vec<u8>| {
            anyhow::anyhow!("invalid txid length in p2tr_utxo_lookup: {}", v.len())
        })?;
        let key: Vec<u8> = row.try_get("p2tr_xonly_key")?;
        let p2tr_xonly_key: [u8; 32] = key.try_into().map_err(|v: Vec<u8>| {
            anyhow::anyhow!(
                "invalid p2tr_xonly_key length in p2tr_utxo_lookup: {}",
                v.len()
            )
        })?;
        let vout: i64 = row.try_get("vout")?;
        let uid: i64 = row.try_get("uid")?;
        let value_sat: i64 = row.try_get("value_sat")?;
        let created_height: i64 = row.try_get("created_height")?;
        let created_tx_index: i64 = row.try_get("created_tx_index")?;
        let created_block_hash: Vec<u8> = row.try_get("created_block_hash")?;
        let created_block_hash: [u8; 32] =
            created_block_hash.try_into().map_err(|v: Vec<u8>| {
                anyhow::anyhow!(
                    "invalid created_block_hash length in p2tr_utxo_lookup: {}",
                    v.len()
                )
            })?;
        let script_pubkey: Vec<u8> = row.try_get("script_pubkey")?;

        entries.push(ScopedUtxoEntry {
            outpoint: OutPointKey {
                txid: TxidBytes::from(txid),
                vout: u32::try_from(vout)?,
            },
            uid: u64::try_from(uid)?,
            created_height: u64::try_from(created_height)?,
            created_block_hash: BlockHashBytes::from(created_block_hash),
            tx_index: u32::try_from(created_tx_index)?,
            value_sat: u64::try_from(value_sat)?,
            script_pubkey,
            p2tr_xonly_key,
        });
    }

    // Do not load p2tr_key_stats into memory for live indexing. The historical
    // seen-key set grows with total P2TR usage and is only needed for reuse
    // debug counters, not for UID assignment or light payload correctness.
    Ok(P2trIndexerState::restore_without_reuse_tracking(
        u64::try_from(last_uid)?,
        entries,
    ))
}

async fn get_meta(archive: &SqliteArchive, key: &str) -> anyhow::Result<Option<String>> {
    let value = sqlx::query_scalar::<_, String>(r#"SELECT value FROM meta WHERE key = ?"#)
        .bind(key)
        .fetch_optional(archive.pool())
        .await?;

    Ok(value)
}

async fn validate_resume_boundary(
    archive: &SqliteArchive,
    profile_id: i64,
    start_height: u64,
) -> anyhow::Result<()> {
    let row = sqlx::query(
        r#"SELECT served_tip_height, served_tip_hash
           FROM profiles
           WHERE profile_id = ?"#,
    )
    .bind(profile_id)
    .fetch_one(archive.pool())
    .await?;

    let served_tip_height: i64 = row.try_get("served_tip_height")?;
    let served_tip_hash: Option<Vec<u8>> = row.try_get("served_tip_hash")?;

    if served_tip_height <= 0 {
        return Ok(());
    }

    let served_tip = u64::try_from(served_tip_height)?;
    anyhow::ensure!(
        served_tip >= start_height,
        "profile served tip {} is below configured start height {}",
        served_tip,
        start_height
    );

    let expected_count = served_tip - start_height + 1;
    let block_count = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*)
           FROM blocks
           WHERE height >= ? AND height <= ?"#,
    )
    .bind(i64::try_from(start_height)?)
    .bind(i64::try_from(served_tip)?)
    .fetch_one(archive.pool())
    .await?;
    anyhow::ensure!(
        u64::try_from(block_count)? == expected_count,
        "database has a gap in committed blocks for {}..={} (rows={}, expected={})",
        start_height,
        served_tip,
        block_count,
        expected_count
    );

    let payload_count = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*)
           FROM payload_cache
           WHERE profile_id = ? AND height >= ? AND height <= ?"#,
    )
    .bind(profile_id)
    .bind(i64::try_from(start_height)?)
    .bind(i64::try_from(served_tip)?)
    .fetch_one(archive.pool())
    .await?;
    anyhow::ensure!(
        u64::try_from(payload_count)? == expected_count,
        "database has a gap in committed payloads for profile {} over {}..={} (rows={}, expected={})",
        profile_id,
        start_height,
        served_tip,
        payload_count,
        expected_count
    );

    let block_hash: Vec<u8> =
        sqlx::query_scalar(r#"SELECT block_hash FROM blocks WHERE height = ?"#)
            .bind(i64::try_from(served_tip)?)
            .fetch_one(archive.pool())
            .await?;

    if let Some(served_tip_hash) = served_tip_hash {
        anyhow::ensure!(
            served_tip_hash == block_hash,
            "profile served tip hash does not match blocks row at height {}",
            served_tip
        );
    }

    let checkpoint_exists = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*)
           FROM checkpoint_cache
           WHERE profile_id = ? AND height = ?"#,
    )
    .bind(profile_id)
    .bind(i64::try_from(served_tip)?)
    .fetch_one(archive.pool())
    .await?;
    anyhow::ensure!(
        checkpoint_exists == 1,
        "missing checkpoint at committed profile tip height {} for profile {}",
        served_tip,
        profile_id
    );

    Ok(())
}

async fn next_index_height(
    archive: &SqliteArchive,
    profile_id: i64,
    start_height: u64,
) -> anyhow::Result<u64> {
    let served_tip_height = sqlx::query_scalar::<_, i64>(
        r#"SELECT served_tip_height FROM profiles WHERE profile_id = ?"#,
    )
    .bind(profile_id)
    .fetch_one(archive.pool())
    .await?;

    if served_tip_height <= 0 {
        Ok(start_height)
    } else {
        Ok(u64::try_from(served_tip_height)? + 1)
    }
}

fn deterministic_txid(height: u64, n: u8) -> TxidBytes {
    let mut txid = [n; 32];
    txid[..8].copy_from_slice(&height.to_le_bytes());
    txid[8] = n;
    TxidBytes::from(txid)
}
