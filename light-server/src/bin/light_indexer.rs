use bitcoin::consensus::encode::deserialize;
use bitcoin::hashes::Hash as BitcoinHash;
use bitcoin::Block;
use btc_data_core::block::{BlockSpentTxOuts, SpentTxOut};
use btc_data_core::source::{BlockSource, TipWatcher};
use btc_data_light_server::index::{encode_stored_light_block, to_packed_bytes};
use btc_data_light_server::index_store::RocksIndexStore;
use btc_data_light_server::p2tr_indexer::{
    BlockScanInput, BlockScopeStats, OutPointKey, P2trIndexerState, TxInputScan, TxOutputScan,
    TxScanInput,
};
use btc_data_light_server::profile::ArchiveNetwork;
use btc_data_light_server::script_classify::{classify_script, extract_p2tr_xonly, ScriptKind};
use btc_data_light_server::sp_tweak::{
    compute_tx_scan_point, PrevoutInfo, PrevoutScript, ScanPointIneligibleReason, ScanPointStatus,
    TxInputContext,
};
use btc_data_light_server::storage::{ChainTip, FileArchive, Manifest};
use btc_data_light_server::types::{BlockHashBytes, TxTweak, TxidBytes};
use btc_data_sources::{IpcSource, PollingTipWatcher, RestSource};
use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};
use futures::{stream, StreamExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing_subscriber::EnvFilter;

const DEFAULT_CATCHUP_BATCH_SIZE: usize = 16;
const DEFAULT_FLUSH_BLOCKS: usize = 128;
const DEFAULT_MEMORY_BUDGET_MB: usize = 4096;
fn mb_to_bytes(mb: usize) -> usize {
    mb.saturating_mul(1024).saturating_mul(1024)
}

const ESTIMATED_PENDING_RAW_MULTIPLIER: usize = 4;
const ESTIMATED_PENDING_PAYLOAD_MULTIPLIER: usize = 2;
const ESTIMATED_PENDING_BLOCK_OVERHEAD_BYTES: usize = 64 * 1024;

fn estimate_pending_memory_bytes(
    processed_raw_bytes_since_flush: usize,
    pending_payload_bytes: usize,
    pending_blocks: usize,
) -> usize {
    processed_raw_bytes_since_flush
        .saturating_mul(ESTIMATED_PENDING_RAW_MULTIPLIER)
        .saturating_add(pending_payload_bytes.saturating_mul(ESTIMATED_PENDING_PAYLOAD_MULTIPLIER))
        .saturating_add(pending_blocks.saturating_mul(ESTIMATED_PENDING_BLOCK_OVERHEAD_BYTES))
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

        /// Override the default start height for the selected network.
        #[arg(long)]
        start_height: Option<u64>,

        #[arg(long, default_value_t = 3)]
        count: u64,
    },

    /// Continuously watch the source tip and process newly finalized block ranges.
    Run {
        #[command(flatten)]
        source: SourceCli,

        /// File-backed payload archive root. Encoded block payloads are written here.
        #[arg(long, default_value = "lightdata")]
        archive_dir: PathBuf,

        /// RocksDB directory for indexer-only prevout and metadata state.
        #[arg(long, default_value = "light-indexer-rocksdb")]
        index_db_dir: PathBuf,

        #[arg(long, default_value_t = ArchiveNetwork::Mainnet)]
        network: ArchiveNetwork,

        #[arg(long, default_value_t = 6)]
        finality_depth: u64,

        /// Number of blocks fetched and decoded per source request.
        #[arg(long, default_value_t = DEFAULT_CATCHUP_BATCH_SIZE)]
        catchup_batch_size: usize,

        /// Maximum number of applied blocks kept in RAM before one archive/index flush.
        /// Larger values reduce write/commit overhead but increase crash rework
        /// and peak memory. The indexer may flush earlier when estimated pending
        /// memory reaches --memory-budget-mb.
        #[arg(long, default_value_t = DEFAULT_FLUSH_BLOCKS)]
        flush_blocks: usize,

        /// Estimated pending-memory budget in MiB. The indexer checks this after
        /// each fetched+decoded chunk and flushes early when the pending batch is
        /// estimated to be at or above the budget. Set to 0 to disable
        /// memory-budget flushing.
        ///
        /// Deprecated alias: --max-buffered-raw-mb.
        #[arg(long, default_value_t = DEFAULT_MEMORY_BUDGET_MB, alias = "max-buffered-raw-mb")]
        memory_budget_mb: usize,

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

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .try_init();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let args = Args::parse();

    match args.command {
        Command::Fixture {
            archive,
            network,
            start_height,
            count,
        } => {
            let start_height = start_height.unwrap_or_else(|| network.default_start_height());
            write_fixture_archive(archive, network, start_height, count)
        }

        Command::Run {
            source,
            archive_dir,
            index_db_dir,
            network,
            finality_depth,
            catchup_batch_size,
            flush_blocks,
            memory_budget_mb,
            once,
        } => {
            let emit_start_height = network.default_start_height();
            run_command(
                source,
                archive_dir,
                index_db_dir,
                network,
                emit_start_height,
                finality_depth,
                catchup_batch_size,
                flush_blocks,
                memory_budget_mb,
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
    archive_dir: PathBuf,
    index_db_dir: PathBuf,
    network: ArchiveNetwork,
    emit_start_height: u64,
    finality_depth: u64,
    catchup_batch_size: usize,
    flush_blocks: usize,
    memory_budget_mb: usize,
    once: bool,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        catchup_batch_size > 0,
        "catchup_batch_size must be greater than zero"
    );
    anyhow::ensure!(flush_blocks > 0, "flush_blocks must be greater than zero");
    let memory_budget_bytes = mb_to_bytes(memory_budget_mb);

    let bundle = source.build_bundle()?;
    anyhow::ensure!(
        bundle.undo_source.supports_block_spent_txouts(),
        "light-indexer requires a REST undo source with /rest/spenttxouts enabled"
    );
    let file_archive = FileArchive::new(archive_dir);
    let index_store = RocksIndexStore::open(&index_db_dir)?;
    index_store.ensure_meta(&network.to_string(), emit_start_height)?;

    let tip = index_store.tip()?;
    let committed_tip = tip.map(|tip| tip.height);
    let initial_next_height = committed_tip
        .map(|height| height + 1)
        .unwrap_or(emit_start_height);
    let mut state = P2trIndexerState::restore_at_uid(tip.map(|tip| tip.last_uid).unwrap_or(0));
    println!(
        "restored indexer state tip={} next_height={} emit_start_height={} last_uid={} fetch_blocks={} flush_blocks={} memory_budget_mb={} archive_dir={} index_db_dir={}",
        committed_tip
            .map(|h| h.to_string())
            .unwrap_or_else(|| "none".to_string()),
        initial_next_height,
        emit_start_height,
        state.last_uid(),
        catchup_batch_size,
        flush_blocks,
        memory_budget_bytes / 1024 / 1024,
        file_archive.root().display(),
        index_db_dir.display(),
    );

    let mut chain_next_height = initial_next_height;

    loop {
        let best_height = bundle.source.get_best_height().await?;
        let best_hash = bundle.source.get_block_hash(best_height).await?;
        let finalized_tip = best_height.saturating_sub(finality_depth);
        let next_height = chain_next_height;

        println!(
            "source={} watcher={} best_height={} best_hash={} finalized_tip={} next_height={} archive_dir={} index_db_dir={}",
            bundle.source.name(),
            bundle.watcher.name(),
            best_height,
            display_hash(best_hash),
            finalized_tip,
            next_height,
            file_archive.root().display(),
            index_db_dir.display(),
        );

        if next_height <= finalized_tip {
            chain_next_height = catch_up_ranges(
                &file_archive,
                &index_store,
                network,
                bundle.source.as_ref(),
                bundle.undo_source.as_ref(),
                &mut state,
                CatchUpConfig {
                    emit_start_height,
                    finality_depth,
                    fetch_batch_size: catchup_batch_size,
                    flush_blocks,
                    memory_budget_bytes,
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

// NOTE: source chunks are fetched/decoded in bounded batches, then compact
// applied light data is buffered until the archive/index flush boundary. Silent
// Payment tweaks are computed directly from Bitcoin Core undo data returned
// by /rest/spenttxouts, so no mutable prevout store is needed.
/// Bundles the per-run settings for [`catch_up_ranges`], replacing a long
/// positional argument list. Handles (archive/source/state) and the
/// dynamic range bounds stay as direct parameters.
struct CatchUpConfig {
    emit_start_height: u64,
    finality_depth: u64,
    fetch_batch_size: usize,
    flush_blocks: usize,
    memory_budget_bytes: usize,
}

#[allow(clippy::too_many_arguments)]
async fn catch_up_ranges(
    file_archive: &FileArchive,
    index_store: &RocksIndexStore,
    network: ArchiveNetwork,
    source: &dyn BlockSource,
    undo_source: &dyn BlockSource,
    state: &mut P2trIndexerState,
    config: CatchUpConfig,
    start_height: u64,
    finalized_tip: u64,
) -> anyhow::Result<u64> {
    let CatchUpConfig {
        emit_start_height,
        finality_depth,
        fetch_batch_size,
        flush_blocks,
        memory_budget_bytes,
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
    let mut processed_raw_bytes_since_flush = 0usize;
    let mut pending_payload_bytes = 0usize;
    let mut total_fetch_elapsed = Duration::ZERO;
    let mut total_decode_elapsed = Duration::ZERO;
    let mut total_apply_elapsed = Duration::ZERO;
    let mut pending: Vec<Pending> = Vec::with_capacity(flush_count);

    // Seeded from the committed tip so the first chunk's parent linkage
    // is checked against the last served block, not against itself.
    let mut previous_tip_hash: Option<BlockHashBytes> = index_store.previous_tip_hash()?;
    let mut chunk_start = start_height;
    while chunk_start <= flush_end_height {
        let chunk_remaining = flush_end_height - chunk_start + 1;
        let chunk_count = usize::try_from(chunk_remaining.min(fetch_batch_size as u64))?;

        // Stage 1: fetch + decode this bounded source chunk. The helper consumes
        // the raw frames instead of cloning their byte buffers, keeping peak
        // memory bounded to one source chunk plus its decoded block inputs.
        let chunk = fetch_and_decode_chunk(source, undo_source, chunk_start, chunk_count).await?;
        total_fetch_elapsed += chunk.fetch_elapsed;
        total_decode_elapsed += chunk.decode_elapsed;
        processed_raw_bytes_since_flush += chunk.raw_bytes;
        let decoded = chunk.blocks;

        validate_decoded_chunk(&decoded, chunk_start, previous_tip_hash)?;

        let chunk_first_height = decoded.first().expect("non-empty").height;
        let chunk_last = decoded.last().expect("non-empty");
        last_height = Some(chunk_last.height);
        last_hash = Some(chunk_last.block_hash);
        previous_tip_hash = Some(chunk_last.block_hash);

        let estimated_pending_memory_before_apply = estimate_pending_memory_bytes(
            processed_raw_bytes_since_flush,
            pending_payload_bytes,
            pending.len(),
        );
        println!(
            "fetched finalized chunk {}..={} count={} bytes={} live_raw_bytes=0 processed_raw_bytes_since_flush={} flush_target={}..={} pending_blocks={} estimated_pending_mb={}",
            chunk_first_height,
            last_height.expect("last height set"),
            decoded.len(),
            chunk.raw_bytes,
            processed_raw_bytes_since_flush,
            first_height,
            flush_end_height,
            pending.len(),
            estimated_pending_memory_before_apply / 1024 / 1024
        );

        // Stage 2a: resolve historical spend UIDs missing from the small
        // in-process map. RocksDB owns historical outpoint lookup state. The resolved candidates are temporary and are
        // cleared immediately after this chunk is applied.
        let lookup_started = Instant::now();
        let rocksdb_spend_outpoints =
            lookup_spend_uids_for_chunk(index_store, &mut *state, &decoded)?;
        let lookup_elapsed = lookup_started.elapsed();
        if !rocksdb_spend_outpoints.is_empty() {
            println!(
                "resolved RocksDB spend UID candidates chunk_start={} entries={} lookup_ms={}",
                chunk_first_height,
                rocksdb_spend_outpoints.len(),
                lookup_elapsed.as_millis()
            );
        }

        // Stage 2b: serial, ordered apply. State advances in memory until the
        // flush transaction commits. A crash before commit will reprocess from
        // the last committed tip.
        let apply_started = Instant::now();
        for scan in decoded {
            let emit_light_payload = scan.height >= emit_start_height;
            if emit_light_payload {
                let applied = state.apply_block_with_stats(scan)?;
                let payload = to_packed_bytes(&encode_stored_light_block(&applied.storage_block)?)?;
                pending_payload_bytes = pending_payload_bytes.saturating_add(payload.len());
                pending.push(Pending { applied, payload });
            }
        }
        let cleared_rocksdb_candidates = state.evict_spend_uid_candidates(rocksdb_spend_outpoints);
        if cleared_rocksdb_candidates > 0 {
            println!(
                "cleared unresolved RocksDB spend UID candidates chunk_start={} entries={}",
                chunk_first_height, cleared_rocksdb_candidates
            );
        }
        total_apply_elapsed += apply_started.elapsed();

        let chunk_last_height = last_height.expect("last height set");
        chunk_start = chunk_last_height + 1;

        if memory_budget_bytes > 0 && chunk_start <= flush_end_height {
            let estimated_pending_memory = estimate_pending_memory_bytes(
                processed_raw_bytes_since_flush,
                pending_payload_bytes,
                pending.len(),
            );
            if estimated_pending_memory >= memory_budget_bytes {
                println!(
                    "memory budget reached at height {} estimated_pending_mb={} budget_mb={} live_raw_bytes=0 processed_raw_bytes_since_flush={} pending_payload_bytes={} pending_blocks={}",
                    chunk_last_height,
                    estimated_pending_memory / 1024 / 1024,
                    memory_budget_bytes / 1024 / 1024,
                    processed_raw_bytes_since_flush,
                    pending_payload_bytes,
                    pending.len()
                );
                break;
            }
        }
    }

    let last_height = last_height.expect("last height set");
    let last_hash = last_hash.expect("last hash set");

    // Write served block payloads to the file-backed archive before advancing the
    // tip. Extra files above the committed tip are harmless after a crash;
    // the RocksDB tip remains the recovery boundary.
    let payload_file_started = Instant::now();
    for p in &pending {
        let block = &p.applied.light_block;
        file_archive.write_block_bytes(block.height, &p.payload)?;
    }

    // Populate storage-only spent heights in the creation block files before
    // committing the RocksDB transaction that deletes spent outpoint lookup rows.
    // The AppliedBlock values still carry the RocksDB-derived
    // creation_height/output_index pointers needed to rewrite older archive
    // files for output-side cut-through.
    let marked_spent_outputs = file_archive.mark_spent_outputs(pending.iter().flat_map(|p| {
        p.applied.spent_utxos.iter().map(|spent| {
            (
                spent.creation_height,
                spent.output_index,
                spent.spent_height,
            )
        })
    }))?;
    let payload_file_ms = payload_file_started.elapsed().as_millis();

    // Stage 3: commit indexer working state after payload files are durable.
    let index_started = Instant::now();
    index_store.commit_applied_blocks(
        pending.iter().map(|p| &p.applied),
        last_height,
        last_hash,
        state.last_uid(),
    )?;
    index_store.flush()?;
    let index_ms = index_started.elapsed().as_millis();

    file_archive.write_manifest(&Manifest {
        version: btc_data_light_server::WIRE_VERSION,
        network: network.to_string(),
        genesis_hash: None,
        finality_depth,
        suggested_reorg_cache_depth: finality_depth.max(144),
        max_range_count: btc_data_light_server::DEFAULT_MAX_RANGE_COUNT,
        tip: Some(ChainTip {
            height: last_height,
            block_hash: display_hash(*last_hash.as_bytes()),
        }),
    })?;

    let total_elapsed = range_started.elapsed();

    let mut totals = BlockScopeStats::default();
    let mut total_tx_count = 0u64;
    let mut payload_bytes = 0usize;
    for p in &pending {
        let s = &p.applied.stats;
        total_tx_count += u64::from(s.tx_count);
        totals.output_count_total += s.output_count_total;
        totals.p2tr_output_count += s.p2tr_output_count;
        totals.p2tr_count += s.p2tr_count;
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

    let estimated_pending_memory = estimate_pending_memory_bytes(
        processed_raw_bytes_since_flush,
        pending_payload_bytes,
        pending.len(),
    );
    tracing::info!(
        first_height,
        last_height,
        blocks = processed_blocks,
        served_blocks = pending.len(),
        flush_blocks,
        memory_budget_mb = memory_budget_bytes / 1024 / 1024,
        estimated_pending_mb = estimated_pending_memory / 1024 / 1024,
        processed_raw_bytes = processed_raw_bytes_since_flush,
        live_raw_bytes = 0usize,
        payload_bytes,
        txs = total_tx_count,
        outputs = totals.output_count_total,
        p2tr_outputs = totals.p2tr_output_count,
        indexed_created = totals.indexed_output_count,
        indexed_spent = totals.indexed_spent_count,
        tx_tweaks = totals.tweak_count,
        marked_spent_outputs,
        last_uid = state.last_uid(),
        cached_outpoints = state.cached_outpoint_count(),
        fetch_ms = total_fetch_elapsed.as_millis(),
        decode_ms = total_decode_elapsed.as_millis(),
        apply_ms = total_apply_elapsed.as_millis(),
        index_ms,
        payload_file_ms,
        total_ms = total_elapsed.as_millis(),
        "indexed finalized range"
    );

    Ok(last_height + 1)
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

fn lookup_spend_uids_for_chunk(
    index_store: &RocksIndexStore,
    state: &mut P2trIndexerState,
    decoded: &[BlockScanInput],
) -> anyhow::Result<Vec<OutPointKey>> {
    let mut outpoints = Vec::<OutPointKey>::new();
    for block in decoded {
        for tx in &block.txs {
            for input in &tx.inputs {
                let outpoint = input.previous_output;
                if !outpoint.is_coinbase() && !state.contains_outpoint(&outpoint) {
                    outpoints.push(outpoint);
                }
            }
        }
    }

    outpoints.sort_by(|a, b| {
        a.txid
            .as_bytes()
            .cmp(b.txid.as_bytes())
            .then_with(|| a.vout.cmp(&b.vout))
    });
    outpoints.dedup_by(|a, b| a.txid == b.txid && a.vout == b.vout);

    if outpoints.is_empty() {
        return Ok(Vec::new());
    }

    let found = index_store.lookup_outpoint_uids(&outpoints)?;
    Ok(state.cache_spend_uid_candidates(found))
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
    let mut blocks: Vec<BlockScanInput> = stream::iter(frames)
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

        let silent_payment_tweak =
            compute_sp_tweak_from_undo(tx_index, txid, &inputs, &outputs, spent_txouts)?;
        let index: u16 = tx_index as u16;
        txs.push(TxScanInput {
            txid,
            tx_index: index,
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
        raw_block_bytes: u32::try_from(bytes.len())?,
        txs,
    })
}

fn compute_sp_tweak_from_undo(
    tx_index: usize,
    txid: TxidBytes,
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

    // /rest/spenttxouts is emitted with a coinbase slot at index 0.
    // Coinbase has no spent prevouts, so non-coinbase tx_index maps directly
    // to the same index in spent_txouts.txs.
    let undo_tx_index = tx_index;
    let undo_inputs = spent_txouts.txs.get(undo_tx_index).ok_or_else(|| {
        anyhow::anyhow!("spenttxouts missing undo tx entry for tx_index={tx_index}")
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

    let p2tr_output_count = outputs.iter().filter(|output| output.is_p2tr).count();

    match compute_tx_scan_point(&input_context)? {
        ScanPointStatus::Computed(tweak) => Ok(Some(tweak)),
        ScanPointStatus::Ineligible {
            reason: ScanPointIneligibleReason::PublicKeySumInfinity,
        } => {
            tracing::warn!(
                tx_index,
                txid = %display_txid(txid),
                input_count = inputs.len(),
                p2tr_output_count,
                "eligible input public-key sum is infinity; skipping silent-payment tweak"
            );
            Ok(None)
        }
        ScanPointStatus::Ineligible { reason } => {
            tracing::debug!(
                tx_index,
                txid = %display_txid(txid),
                ?reason,
                input_count = inputs.len(),
                p2tr_output_count,
                "transaction is ineligible for silent-payment tweak; skipping tweak"
            );
            Ok(None)
        }
        ScanPointStatus::MissingPrevout { missing_count } => {
            tracing::warn!(
                tx_index,
                txid = %display_txid(txid),
                missing_count,
                input_count = inputs.len(),
                p2tr_output_count,
                "missing prevout while computing silent-payment tweak; skipping tweak"
            );
            Ok(None)
        }
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

fn display_txid(txid: TxidBytes) -> String {
    display_hash(txid.into_inner())
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
    u64,
    BlockHashBytes,
    u64,
);

fn fixture_blocks(start_height: u64, count: u64) -> anyhow::Result<FixtureBlocks> {
    anyhow::ensure!(count > 0, "count must be greater than zero");

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

        if true {
            previous_indexed_outpoint = Some(OutPointKey {
                txid: txid_b,
                vout: 0,
            });
        }

        let applied = state.apply_block_with_stats(BlockScanInput {
            height,
            block_hash: hash,
            previous_block_hash: prev_hash,
            raw_block_bytes: 1_000,
            txs,
        })?;

        let bytes = to_packed_bytes(&encode_stored_light_block(&applied.storage_block)?)?;

        blocks.push((
            height,
            hash,
            prev_hash,
            state.last_uid(),
            bytes,
            applied.stats,
        ));
    }

    let tip_height = start_height + count - 1;

    Ok((blocks, tip_height, tip_hash, state.last_uid()))
}

fn write_fixture_archive(
    root: PathBuf,
    network: ArchiveNetwork,
    start_height: u64,
    count: u64,
) -> anyhow::Result<()> {
    let archive = FileArchive::new(root);

    let (blocks, tip_height, tip_hash, _last_uid) = fixture_blocks(start_height, count)?;

    for (height, _hash, _prev_hash, _anchor_last_uid, bytes, _stats) in blocks {
        archive.write_block_bytes(height, &bytes)?;
    }

    archive.write_manifest(&Manifest {
        version: btc_data_light_server::WIRE_VERSION,
        network: network.to_string(),
        genesis_hash: None,
        finality_depth: 6,
        suggested_reorg_cache_depth: 144,
        max_range_count: btc_data_light_server::DEFAULT_MAX_RANGE_COUNT,
        tip: Some(ChainTip {
            height: tip_height,
            block_hash: hex::encode(tip_hash.as_bytes()),
        }),
    })?;

    println!(
        "wrote file-backed fixture archive at {} starting from height {}",
        archive.root().display(),
        start_height
    );

    Ok(())
}

fn deterministic_txid(height: u64, n: u8) -> TxidBytes {
    let mut txid = [n; 32];
    txid[..8].copy_from_slice(&height.to_le_bytes());
    txid[8] = n;
    TxidBytes::from(txid)
}
