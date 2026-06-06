//! The block scan loop.
//!
//! Reusable library function: takes a `BlockSource`, runs the prefetch +
//! parse + sequential commit pipeline, drives a `StatsSink` per block.
//!
//! Used by the `btc-data-stats` binary; can also be embedded in tests or
//! other tools.

use crate::checkpoint::{self, CheckpointConfig, CheckpointWriter, StatsCheckpoint, CHECKPOINT_FORMAT_VERSION};
use crate::sinks::StatsSink;
use crate::{
    classify_p2tr_spend, BlockStats, PerBlock, RollingUtxoHash, SpendContext, SpendPath, Stats,
};
use bitcoin::Script;
use btc_data_core::block::RawBlockFrame;
use btc_data_core::parse::{decode_raw_block, DecodedBlockFrame};
use btc_data_core::source::BlockSource;
use futures::{stream, StreamExt};
use ahash::RandomState;
use hashbrown::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use crate::script::{classify_script, ScriptType};

/// Fast internal hash map for scanner state.
///
/// The scanner owns these maps and keys are not attacker-chosen API input, so
/// using `ahash` avoids the high constant cost of the standard SipHash-based
/// hasher on the UTXO hot path.
pub type FastHashMap<K, V> = HashMap<K, V, RandomState>;
pub type FastHashSet<K> = HashSet<K, RandomState>;

/// Scan configuration.
pub struct ScanConfig {
    pub start: u64,
    pub blocks: u64,
    pub buffer: usize,
    /// Number of contiguous heights to request per source call.
    ///
    /// RPC sources use this to send JSON-RPC batch requests. Use 1 for
    /// one request per height, which is usually best for REST/IPC.
    pub source_batch_size: usize,
    /// Pre-allocate approximately this many UTXO entries in scanner state.
    /// This can reduce HashMap growth/rehash overhead during long scans.
    pub utxo_reserve: usize,
    /// Pre-allocate approximately this many seen P2TR keys.
    pub seen_keys_reserve: usize,
    pub progress: u64,
    pub utxo_hash_window: u64,
    /// Only process blocks at least this many blocks behind the current tip.
    pub finality_depth: u64,
    pub checkpoints: CheckpointConfig,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            start: 709_632,
            blocks: 1_000,
            buffer: 10,
            source_batch_size: 1,
            utxo_reserve: 0,
            seen_keys_reserve: 0,
            progress: 1_000,
            utxo_hash_window: 0,
            finality_depth: 6,
            checkpoints: CheckpointConfig::default(),
        }
    }
}


#[derive(Debug, Default, Clone, Copy)]
struct PrepareMetrics {
    txid_compute: Duration,
    output_classify: Duration,
    p2tr_spend_classify: Duration,
}

impl PrepareMetrics {
    fn saturating_delta(self, previous: Self) -> Self {
        Self {
            txid_compute: self.txid_compute.saturating_sub(previous.txid_compute),
            output_classify: self.output_classify.saturating_sub(previous.output_classify),
            p2tr_spend_classify: self.p2tr_spend_classify.saturating_sub(previous.p2tr_spend_classify),
        }
    }
}

#[derive(Debug)]
struct PreparedInput {
    previous_output: bitcoin::OutPoint,
    p2tr_spend_class: crate::transaction::SpendClass,
}

#[derive(Debug)]
struct PreparedTx {
    txid: bitcoin::Txid,
    is_coinbase: bool,
    inputs: Vec<PreparedInput>,
    output_script_types: Vec<ScriptType>,
    output_xonly_keys: Vec<Option<[u8; 32]>>,
}

#[derive(Debug)]
struct PreparedBlockFrame {
    height: u64,
    hash: [u8; 32],
    txs: Vec<PreparedTx>,
    prepare_metrics: PrepareMetrics,
}

fn prepare_decoded_block(frame: DecodedBlockFrame) -> anyhow::Result<PreparedBlockFrame> {
    let mut prepare_metrics = PrepareMetrics::default();
    let mut txs = Vec::with_capacity(frame.block.txdata.len());

    // Consume the decoded bitcoin::Block and keep only the compact data needed
    // by the sequential stateful stage. This avoids buffering full decoded
    // blocks, scripts, and witnesses when --buffer is greater than 1.
    for tx in frame.block.txdata {
        let txid_t = Instant::now();
        let txid = tx.compute_txid();
        prepare_metrics.txid_compute += txid_t.elapsed();

        let is_coinbase = tx.is_coinbase();

        let mut inputs = if is_coinbase {
            Vec::new()
        } else {
            Vec::with_capacity(tx.input.len())
        };

        if !is_coinbase {
            for vin in tx.input {
                let classify_t = Instant::now();
                let p2tr_spend_class = classify_p2tr_spend(&vin.witness);
                prepare_metrics.p2tr_spend_classify += classify_t.elapsed();

                inputs.push(PreparedInput {
                    previous_output: vin.previous_output,
                    p2tr_spend_class,
                });
            }
        }

        let mut output_script_types = Vec::with_capacity(tx.output.len());
        let mut output_xonly_keys = Vec::with_capacity(tx.output.len());

        for out in tx.output {
            let classify_t = Instant::now();
            let script_type = classify_script(&out.script_pubkey);
            let xonly_key = if matches!(script_type, ScriptType::P2tr) {
                extract_xonly_script(&out.script_pubkey)
            } else {
                None
            };
            prepare_metrics.output_classify += classify_t.elapsed();

            output_script_types.push(script_type);
            output_xonly_keys.push(xonly_key);
        }

        txs.push(PreparedTx {
            txid,
            is_coinbase,
            inputs,
            output_script_types,
            output_xonly_keys,
        });
    }

    Ok(PreparedBlockFrame {
        height: frame.height,
        hash: frame.hash,
        txs,
        prepare_metrics,
    })
}

fn prepare_raw_block(frame: RawBlockFrame) -> anyhow::Result<PreparedBlockFrame> {
    prepare_decoded_block(decode_raw_block(frame)?)
}

async fn prepare_raw_block_blocking(frame: RawBlockFrame) -> anyhow::Result<PreparedBlockFrame> {
    tokio::task::spawn_blocking(move || prepare_raw_block(frame))
        .await
        .map_err(|err| anyhow::anyhow!("prepare worker task failed: {err}"))?
}

async fn prepare_raw_blocks_blocking(frames: Vec<RawBlockFrame>) -> anyhow::Result<Vec<PreparedBlockFrame>> {
    tokio::task::spawn_blocking(move || {
        frames
            .into_iter()
            .map(prepare_raw_block)
            .collect::<anyhow::Result<Vec<_>>>()
    })
        .await
        .map_err(|err| anyhow::anyhow!("prepare batch worker task failed: {err}"))?
}


#[derive(Debug, Default, Clone, Copy)]
struct ProcessMetrics {
    input_remove: Duration,
    input_accounting: Duration,
    p2tr_spend: Duration,
    output_classify: Duration,
    output_accounting: Duration,
    seen_keys: Duration,
    utxo_insert: Duration,
    utxo_hash: Duration,
    row_build: Duration,
}

impl ProcessMetrics {
    fn saturating_delta(self, previous: Self) -> Self {
        Self {
            input_remove: self.input_remove.saturating_sub(previous.input_remove),
            input_accounting: self.input_accounting.saturating_sub(previous.input_accounting),
            p2tr_spend: self.p2tr_spend.saturating_sub(previous.p2tr_spend),
            output_classify: self.output_classify.saturating_sub(previous.output_classify),
            output_accounting: self.output_accounting.saturating_sub(previous.output_accounting),
            seen_keys: self.seen_keys.saturating_sub(previous.seen_keys),
            utxo_insert: self.utxo_insert.saturating_sub(previous.utxo_insert),
            utxo_hash: self.utxo_hash.saturating_sub(previous.utxo_hash),
            row_build: self.row_build.saturating_sub(previous.row_build),
        }
    }

    fn total_accounted(self) -> Duration {
        self.input_remove
            + self.input_accounting
            + self.p2tr_spend
            + self.output_classify
            + self.output_accounting
            + self.seen_keys
            + self.utxo_insert
            + self.utxo_hash
            + self.row_build
    }
}

#[derive(Debug, Default, Clone)]
struct ScanMetrics {
    fetch_wait: Duration,
    process: Duration,
    sink: Duration,
    checkpoint: Duration,
    process_detail: ProcessMetrics,
    prepare_detail: PrepareMetrics,
    blocks: u64,
    last_report_fetch_wait: Duration,
    last_report_process: Duration,
    last_report_sink: Duration,
    last_report_checkpoint: Duration,
    last_report_process_detail: ProcessMetrics,
    last_report_prepare_detail: PrepareMetrics,
    last_report_blocks: u64,
}

impl ScanMetrics {
    fn add_fetch_wait(&mut self, dt: Duration) { self.fetch_wait += dt; }
    fn add_process(&mut self, dt: Duration) { self.process += dt; }
    fn add_sink(&mut self, dt: Duration) { self.sink += dt; }
    fn add_checkpoint(&mut self, dt: Duration) { self.checkpoint += dt; }

    fn print_progress(&mut self, elapsed: Duration, state: &StatsScannerState, height: u64, scan_start: u64) {
        let bps = (height - scan_start) as f64 / elapsed.as_secs_f64().max(0.001);
        let interval_blocks = self.blocks.saturating_sub(self.last_report_blocks).max(1);
        let fetch_delta = self.fetch_wait.saturating_sub(self.last_report_fetch_wait);
        let process_delta = self.process.saturating_sub(self.last_report_process);
        let sink_delta = self.sink.saturating_sub(self.last_report_sink);
        let checkpoint_delta = self.checkpoint.saturating_sub(self.last_report_checkpoint);
        let detail_delta = self.process_detail.saturating_delta(self.last_report_process_detail);
        let prepare_delta = self.prepare_detail.saturating_delta(self.last_report_prepare_detail);
        let other_process = process_delta.saturating_sub(detail_delta.total_accounted());

        eprintln!(
            "  h={height:>7} blocks={:>7} last_id={:>12} utxo={:>10} seen_keys={:>10} missing={:>6} {bps:>5.1} blk/s | fetch+decode+prepare={:.1}s (+{:.2}s/{interval_blocks}) process={:.1}s (+{:.2}s/{interval_blocks}) sink={:.1}s checkpoint={:.1}s",
            self.blocks,
            state.last_id,
            state.utxo.len(),
            state.seen_keys.len(),
            state.missing,
            self.fetch_wait.as_secs_f64(),
            fetch_delta.as_secs_f64(),
            self.process.as_secs_f64(),
            process_delta.as_secs_f64(),
            self.sink.as_secs_f64(),
            self.checkpoint.as_secs_f64(),
        );
        eprintln!(
            "      prepare delta: txid_compute={:.2}s output_classify={:.2}s",
            prepare_delta.txid_compute.as_secs_f64(),
            prepare_delta.output_classify.as_secs_f64(),
        );
        eprintln!(
            "      process delta: input_remove={:.2}s input_accounting={:.2}s p2tr_spend={:.2}s output_classify={:.2}s output_accounting={:.2}s seen_keys={:.2}s utxo_insert={:.2}s utxo_hash={:.2}s row={:.2}s other={:.2}s sink_delta={:.2}s checkpoint_delta={:.2}s",
            detail_delta.input_remove.as_secs_f64(),
            detail_delta.input_accounting.as_secs_f64(),
            detail_delta.p2tr_spend.as_secs_f64(),
            detail_delta.output_classify.as_secs_f64(),
            detail_delta.output_accounting.as_secs_f64(),
            detail_delta.seen_keys.as_secs_f64(),
            detail_delta.utxo_insert.as_secs_f64(),
            detail_delta.utxo_hash.as_secs_f64(),
            detail_delta.row_build.as_secs_f64(),
            other_process.as_secs_f64(),
            sink_delta.as_secs_f64(),
            checkpoint_delta.as_secs_f64(),
        );

        self.last_report_fetch_wait = self.fetch_wait;
        self.last_report_process = self.process;
        self.last_report_sink = self.sink;
        self.last_report_checkpoint = self.checkpoint;
        self.last_report_process_detail = self.process_detail;
        self.last_report_prepare_detail = self.prepare_detail;
        self.last_report_blocks = self.blocks;
    }

    fn print_summary(&self, elapsed: Duration) {
        eprintln!(
            "Timing: total={:.1}s fetch_decode_prepare_wait={:.1}s process={:.1}s sink={:.1}s checkpoint={:.1}s blocks={}",
            elapsed.as_secs_f64(),
            self.fetch_wait.as_secs_f64(),
            self.process.as_secs_f64(),
            self.sink.as_secs_f64(),
            self.checkpoint.as_secs_f64(),
            self.blocks,
        );
        eprintln!(
            "Prepare detail: txid_compute={:.1}s output_classify={:.1}s p2tr_spend_classify={:.1}s",
            self.prepare_detail.txid_compute.as_secs_f64(),
            self.prepare_detail.output_classify.as_secs_f64(),
            self.prepare_detail.p2tr_spend_classify.as_secs_f64(),
        );
        eprintln!(
            "Process detail: input_remove={:.1}s input_accounting={:.1}s p2tr_spend={:.1}s output_classify={:.1}s output_accounting={:.1}s seen_keys={:.1}s utxo_insert={:.1}s utxo_hash={:.1}s row={:.1}s other={:.1}s",
            self.process_detail.input_remove.as_secs_f64(),
            self.process_detail.input_accounting.as_secs_f64(),
            self.process_detail.p2tr_spend.as_secs_f64(),
            self.process_detail.output_classify.as_secs_f64(),
            self.process_detail.output_accounting.as_secs_f64(),
            self.process_detail.seen_keys.as_secs_f64(),
            self.process_detail.utxo_insert.as_secs_f64(),
            self.process_detail.utxo_hash.as_secs_f64(),
            self.process_detail.row_build.as_secs_f64(),
            self.process.saturating_sub(self.process_detail.total_accounted()).as_secs_f64(),
        );
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct UtxoEntry {
    /// Packed as: upper 60 bits = output id, lower 4 bits = ScriptType.
    ///
    /// Keeping the value to one word improves hash-table cache density in the
    /// UTXO hot path. ScriptType currently has 9 variants, so 4 bits is enough.
    packed: u64,
}

impl UtxoEntry {
    const SCRIPT_TYPE_BITS: u64 = 4;
    const SCRIPT_TYPE_MASK: u64 = (1 << Self::SCRIPT_TYPE_BITS) - 1;
    const MAX_ID: u64 = u64::MAX >> Self::SCRIPT_TYPE_BITS;

    fn new(id: u64, script_type: ScriptType) -> Self {
        debug_assert!(id <= Self::MAX_ID, "UTXO id too large to pack");
        Self { packed: (id << Self::SCRIPT_TYPE_BITS) | u64::from(script_type.to_u8()) }
    }

    fn id(self) -> u64 {
        self.packed >> Self::SCRIPT_TYPE_BITS
    }

    fn script_type(self) -> ScriptType {
        ScriptType::from_u8((self.packed & Self::SCRIPT_TYPE_MASK) as u8)
    }

}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StatsScannerState {
    pub last_id: u64,
    pub utxo: FastHashMap<bitcoin::OutPoint, UtxoEntry>,
    pub seen_keys: FastHashSet<[u8; 32]>,
    pub stats: Stats,
    pub utxo_hash: RollingUtxoHash,
    pub missing: u64,
}

impl StatsScannerState {
    pub fn new(utxo_hash_window: u64) -> Self {
        Self {
            last_id: 0,
            utxo: FastHashMap::with_hasher(RandomState::new()),
            seen_keys: FastHashSet::with_hasher(RandomState::new()),
            stats: Stats::new(),
            utxo_hash: RollingUtxoHash::new(utxo_hash_window),
            missing: 0,
        }
    }

    pub fn reserve(&mut self, utxo_reserve: usize, seen_keys_reserve: usize) {
        if utxo_reserve > self.utxo.capacity() {
            self.utxo.reserve(utxo_reserve - self.utxo.capacity());
        }
        if seen_keys_reserve > self.seen_keys.capacity() {
            self.seen_keys.reserve(seen_keys_reserve - self.seen_keys.capacity());
        }
    }
}

/// Stage 1+2: fetch + stateless-prepare a contiguous run of heights in
/// parallel, then return them ordered by height for the serial commit.
///
/// Stateless per-block work (decode, txid compute, script/p2tr-spend
/// classification) happens concurrently as blocks arrive. The batch is then
/// sorted by height so the stateful UTXO commit sees blocks in ascending order.
async fn fetch_and_prepare_batch(
    source: Arc<dyn BlockSource>,
    start_height: u64,
    count: u64,
    in_flight: usize,
) -> anyhow::Result<Vec<PreparedBlockFrame>> {
    // Native batch path: one source call returns the whole range (e.g. JSON-RPC
    // batch request, or REST concurrent fan-out). Stateless prepare then runs
    // on the returned frames in one blocking-pool pass.
    if source.supports_block_range_batches() {
        let frames = source
            .get_block_range_by_height(start_height, count as usize)
            .await?;
        return prepare_raw_blocks_blocking(frames).await;
    }

    // Fallback path: per-height fetch, concurrent via buffer_unordered, then
    // each block stateless-prepared as it arrives. Used by IPC and any source
    // without a native range API.
    let mut prepared: Vec<PreparedBlockFrame> =
        stream::iter(start_height..(start_height + count))
            .map(|height| {
                let source = source.clone();
                async move {
                    let raw = source.get_block_by_height(height).await?;
                    prepare_raw_block_blocking(raw).await
                }
            })
            .buffer_unordered(in_flight)
            .collect::<Vec<anyhow::Result<PreparedBlockFrame>>>()
            .await
            .into_iter()
            .collect::<anyhow::Result<Vec<_>>>()?;

    prepared.sort_by_key(|f| f.height);
    Ok(prepared)
}

pub async fn scan(
    source: Arc<dyn BlockSource>,
    sink: Arc<dyn StatsSink>,
    cfg: ScanConfig,
) -> anyhow::Result<Stats> {
    let tip_height = source.get_best_height().await?;
    let safe_height = tip_height.saturating_sub(cfg.finality_depth);
    eprintln!(
        "Tip height: {tip_height}; finality depth: {}; safe height: {safe_height}",
        cfg.finality_depth
    );

    let requested_end = cfg.start.saturating_add(cfg.blocks.saturating_sub(1));
    if safe_height < cfg.start {
        eprintln!("No finalized blocks to scan: start={} safe_height={safe_height}", cfg.start);
        return Ok(Stats::new());
    }

    let max_end = requested_end.min(safe_height);
    let mut scan_start = cfg.start;
    let mut state = StatsScannerState::new(cfg.utxo_hash_window);

    if cfg.checkpoints.enabled {
        if let Some(checkpoint) = restore_newest_valid_checkpoint(&source, &cfg).await? {
            eprintln!(
                "Restored checkpoint at height {} hash {}",
                checkpoint.height,
                checkpoint::block_hash_hex(&checkpoint.block_hash)
            );
            sink.rollback_to_height(checkpoint.height).await?;
            scan_start = checkpoint.next_height;
            state = checkpoint.scanner_state;
        }
    }

    if cfg.utxo_reserve > 0 || cfg.seen_keys_reserve > 0 {
        eprintln!(
            "Scanner reserves: utxo_reserve={} seen_keys_reserve={}",
            cfg.utxo_reserve, cfg.seen_keys_reserve
        );
        state.reserve(cfg.utxo_reserve, cfg.seen_keys_reserve);
    }

    let mut checkpoint_writer = CheckpointWriter::new(cfg.checkpoints.clone());
    let mut last_committed_height: Option<u64> = None;
    let mut last_committed_hash: Option<[u8; 32]> = None;

    if scan_start > max_end {
        eprintln!("Nothing to scan after recovery: next_height={scan_start} max_end={max_end}");
        return Ok(state.stats);
    }

    let scan_blocks = max_end - scan_start + 1;

    if cfg.utxo_hash_window > 0 {
        eprintln!("UTXO hash window: {} blocks", cfg.utxo_hash_window);
    }

    // batch_size = blocks fetched + stateless-prepared in parallel before each
    // join + serial commit. in_flight caps concurrent fetches within a batch.
    let batch_size = (cfg.source_batch_size.max(1)) as u64;
    let in_flight = cfg.buffer.max(1).min(batch_size as usize);

    eprintln!(
        "Parallel fetch+stateless-prepare (batch={batch_size}, in-flight={in_flight}) \
         then join + serial UTXO commit; {scan_blocks} blocks",
    );

    let t1 = Instant::now();
    let mut metrics = ScanMetrics::default();

    let mut next_height = scan_start;
    while next_height <= max_end {
        let remaining = max_end - next_height + 1;
        let this_count = remaining.min(batch_size);

        // ---- Stage 1+2: parallel fetch + stateless stats, joined in order ----
        let fetch_t = Instant::now();
        let batch = fetch_and_prepare_batch(source.clone(), next_height, this_count, in_flight).await?;
        metrics.add_fetch_wait(fetch_t.elapsed());

        // ---- Stage 3: serial, ordered UTXO state commit ----
        for frame in batch {
            let h = frame.height;
            let frame_hash = frame.hash;
            let block_hash_hex = checkpoint::block_hash_hex(&frame_hash);
            let prepared_txs = frame.txs;
            metrics.prepare_detail.txid_compute += frame.prepare_metrics.txid_compute;
            metrics.prepare_detail.output_classify += frame.prepare_metrics.output_classify;
            metrics.prepare_detail.p2tr_spend_classify += frame.prepare_metrics.p2tr_spend_classify;

            let process_t = Instant::now();
            let last_id_at_block_start = state.last_id;
            let mut per_block = PerBlock::default();

            for tx in prepared_txs {
                let is_coinbase = tx.is_coinbase;
                let txid = tx.txid;
                let last_id_at_tx_start = state.last_id;

                let mut any_sp_eligible_input = false;
                let mut has_non_p2tr_input = false;
                let mut p2tr_input_seen = false;

                if !is_coinbase {
                    for input in &tx.inputs {
                        let input_remove_t = Instant::now();
                        let prevout_entry = state.utxo.remove(&input.previous_output);
                        metrics.process_detail.input_remove += input_remove_t.elapsed();

                        match prevout_entry {
                            Some(entry) => {
                                let input_accounting_t = Instant::now();
                                let prev_id = entry.id();
                                let prev_script_type = entry.script_type();
                                let ctx = if prev_id > last_id_at_tx_start {
                                    SpendContext::SameTx
                                } else if prev_id > last_id_at_block_start {
                                    SpendContext::SameBlock
                                } else {
                                    SpendContext::Earlier
                                };
                                let is_p2tr_spend = matches!(prev_script_type, ScriptType::P2tr);
                                state.stats.record_spend(ctx);
                                per_block.record_spend_uid(prev_id, is_p2tr_spend);
                                state.stats.record_input_script(prev_script_type);
                                per_block.record_input_script(prev_script_type);
                                metrics.process_detail.input_accounting += input_accounting_t.elapsed();

                                if is_p2tr_spend {
                                    p2tr_input_seen = true;
                                    let p2tr_spend_t = Instant::now();
                                    let class = input.p2tr_spend_class;
                                    if !matches!(class.path, SpendPath::ScriptNums) {
                                        any_sp_eligible_input = true;
                                    }
                                    state.stats.record_p2tr_spend(class, ctx);
                                    per_block.record_p2tr_spend(class);
                                    metrics.process_detail.p2tr_spend += p2tr_spend_t.elapsed();
                                } else {
                                    has_non_p2tr_input = true;
                                }
                                let utxo_hash_t = Instant::now();
                                state.utxo_hash.remove_output(prev_id);
                                metrics.process_detail.utxo_hash += utxo_hash_t.elapsed();
                            }
                            None => {
                                let input_accounting_t = Instant::now();
                                has_non_p2tr_input = true;
                                state.missing += 1;
                                metrics.process_detail.input_accounting += input_accounting_t.elapsed();
                            }
                        }
                    }
                }

                let tx_is_nonsp = !is_coinbase
                    && p2tr_input_seen
                    && !any_sp_eligible_input
                    && !has_non_p2tr_input;
                if tx_is_nonsp {
                    state.stats.nonsp_txs += 1;
                    per_block.nonsp_txs += 1;
                }

                for (vout_idx, (&script_type, &xonly_key)) in tx
                    .output_script_types
                    .iter()
                    .zip(tx.output_xonly_keys.iter())
                    .enumerate()
                {
                    state.last_id += 1;
                    let is_p2tr = matches!(script_type, ScriptType::P2tr);

                    let reused = if let Some(k) = xonly_key {
                        let seen_keys_t = Instant::now();
                        let reused = !state.seen_keys.insert(k);
                        metrics.process_detail.seen_keys += seen_keys_t.elapsed();
                        reused
                    } else {
                        false
                    };

                    let output_accounting_t = Instant::now();
                    state.stats.outputs += 1;
                    state.stats.record_output_script(script_type);
                    per_block.record_output_script(script_type);
                    if is_p2tr {
                        state.stats.p2tr_outputs += 1;
                        if reused { state.stats.p2tr_reused += 1; }
                    }
                    per_block.record_output(is_p2tr, reused);

                    if tx_is_nonsp {
                        state.stats.nonsp_tx_outputs += 1;
                        per_block.nonsp_tx_outputs += 1;
                    }
                    metrics.process_detail.output_accounting += output_accounting_t.elapsed();

                    let op = bitcoin::OutPoint { txid, vout: vout_idx as u32 };
                    let utxo_insert_t = Instant::now();
                    state.utxo.insert(op, UtxoEntry::new(state.last_id, script_type));
                    metrics.process_detail.utxo_insert += utxo_insert_t.elapsed();

                    let utxo_hash_t = Instant::now();
                    state.utxo_hash.add_output(state.last_id);
                    metrics.process_detail.utxo_hash += utxo_hash_t.elapsed();
                }
            }

            let utxo_hash_t = Instant::now();
            let utxo_hash_hex = state.utxo_hash.finalize_block();
            metrics.process_detail.utxo_hash += utxo_hash_t.elapsed();

            let row_build_t = Instant::now();
            let mut row = BlockStats::new(h);
            row.block_hash = block_hash_hex;
            row.last_global_id = state.last_id;
            row.utxo_hash = utxo_hash_hex;
            row.utxo_hash_window = cfg.utxo_hash_window;
            per_block.measure_sorted_uid_encoding(state.last_id, &mut state.stats, &mut row);
            per_block.drain_into(&mut row);
            row.output_count = state.last_id - last_id_at_block_start;
            row.refresh_classes();
            metrics.process_detail.row_build += row_build_t.elapsed();
            metrics.add_process(process_t.elapsed());

            let sink_t = Instant::now();
            sink.emit_block(&row).await?;
            metrics.add_sink(sink_t.elapsed());
            metrics.blocks += 1;

            last_committed_height = Some(h);
            last_committed_hash = Some(frame_hash);

            if checkpoint_writer.on_committed_block() {
                let checkpoint_t = Instant::now();
                let checkpoint = make_checkpoint(&cfg, &state, h, frame_hash);
                checkpoint_writer.write_checkpoint(&checkpoint).await?;
                metrics.add_checkpoint(checkpoint_t.elapsed());
            }

            if cfg.progress > 0 && (h - scan_start) % cfg.progress == 0 && h > scan_start {
                metrics.print_progress(t1.elapsed(), &state, h, scan_start);
            }
        }

        next_height += this_count;
    }

    if checkpoint_writer.needs_final_flush() {
        if let (Some(height), Some(hash)) = (last_committed_height, last_committed_hash) {
            let checkpoint_t = Instant::now();
            let checkpoint = make_checkpoint(&cfg, &state, height, hash);
            checkpoint_writer.write_checkpoint(&checkpoint).await?;
            metrics.add_checkpoint(checkpoint_t.elapsed());
        }
    }

    let sink_t = Instant::now();
    sink.emit_run(&state.stats).await?;
    sink.flush().await?;
    metrics.add_sink(sink_t.elapsed());

    if state.missing > 0 {
        eprintln!("\nNote: {} inputs referenced pre-scan outputs.", state.missing);
    }
    eprintln!("Scan loop: {:.1}s", t1.elapsed().as_secs_f64());
    metrics.print_summary(t1.elapsed());

    Ok(state.stats)
}


fn make_checkpoint(
    cfg: &ScanConfig,
    state: &StatsScannerState,
    height: u64,
    block_hash: [u8; 32],
) -> StatsCheckpoint {
    StatsCheckpoint {
        checkpoint_format_version: CHECKPOINT_FORMAT_VERSION,
        stats_state_version: crate::STATS_STATE_VERSION,
        start_height: cfg.start,
        next_height: height + 1,
        height,
        block_hash,
        utxo_hash_window: cfg.utxo_hash_window,
        finality_depth: cfg.finality_depth,
        scanner_state: state.clone(),
    }
}



async fn restore_newest_valid_checkpoint(
    source: &Arc<dyn BlockSource>,
    cfg: &ScanConfig,
) -> anyhow::Result<Option<StatsCheckpoint>> {
    let paths = checkpoint::list_checkpoints_newest_first(&cfg.checkpoints.dir).await?;
    for path in paths {
        let checkpoint = match checkpoint::load_checkpoint(&path).await {
            Ok(checkpoint) => checkpoint,
            Err(err) => {
                eprintln!(
                    "Skipping checkpoint {}: could not load with current checkpoint format: {err}",
                    path.display()
                );
                continue;
            }
        };
        if checkpoint.stats_state_version != crate::STATS_STATE_VERSION {
            eprintln!(
                "Skipping checkpoint {}: stats version {} != {}",
                path.display(),
                checkpoint.stats_state_version,
                crate::STATS_STATE_VERSION,
            );
            continue;
        }
        if checkpoint.utxo_hash_window != cfg.utxo_hash_window {
            eprintln!("Skipping checkpoint {}: scanner config differs", path.display());
            continue;
        }
        let canonical_hash = source.get_block_hash(checkpoint.height).await?;
        if canonical_hash == checkpoint.block_hash {
            return Ok(Some(checkpoint));
        }
        eprintln!(
            "Checkpoint {} is not canonical at height {}; trying older checkpoint",
            path.display(),
            checkpoint.height,
        );
    }
    Ok(None)
}

fn extract_xonly_script(script: &Script) -> Option<[u8; 32]> {
    let bytes = script.as_bytes();
    if bytes.len() == 34 && bytes[0] == 0x51 && bytes[1] == 0x20 {
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes[2..34]);
        Some(out)
    } else {
        None
    }
}