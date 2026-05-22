//! `btc-data-stats` — runs the scan loop, emits per-block stats to sinks.

use btc_data_core::source::BlockSource;
use btc_data_sources::{EsploraSource, MultiSource, RestSource, RpcSource};
#[cfg(feature = "ipc")]
use btc_data_sources::IpcSource;
use btc_data_stats::checkpoint::CheckpointConfig;
use btc_data_stats::scan::{scan, ScanConfig};
use btc_data_stats::sinks::{Fanout, StatsCsvSink, StatsSink};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "btc-data-stats", about = "Block scanner + stats emitter", version)]
struct Cli {
    // ─── sources ────────────────────────────────────────────────
    /// bitcoind JSON-RPC URL. Repeatable.
    #[arg(long = "rpc-url")]
    rpc_urls: Vec<String>,
    #[arg(long, env = "BTC_RPC_USER", default_value = "")]
    rpc_user: String,
    #[arg(long, env = "BTC_RPC_PASS", default_value = "")]
    rpc_pass: String,
    /// Bitcoin Core RPC cookie file. Used automatically when username/password are omitted.
    #[arg(long, env = "BTC_RPC_COOKIE_FILE")]
    rpc_cookie_file: Option<PathBuf>,
    /// Esplora-compatible HTTP base URL. Repeatable.
    #[arg(long = "esplora")]
    esplora_urls: Vec<String>,
    /// Bitcoin Core REST base URL, e.g. http://127.0.0.1:8332. Repeatable.
    /// Requires bitcoind -rest=1.
    #[arg(long = "rest-url")]
    rest_urls: Vec<String>,
    #[arg(long, default_value_t = 200)]
    esplora_min_gap_ms: u64,
    /// Bitcoin Core multiprocess IPC socket path. Requires building with --features ipc. Repeatable.
    #[arg(long = "ipc")]
    ipc_paths: Vec<String>,
    /// Number of Bitcoin Core IPC worker Thread clients to create per IPC socket.
    #[arg(long = "ipc-threads", env = "BTC_DATA_IPC_THREADS", default_value_t = 8)]
    ipc_threads: usize,

    // ─── scan range ─────────────────────────────────────────────
    #[arg(long, default_value_t = 709_632)]
    start: u64,
    #[arg(long, default_value_t = 1_000)]
    blocks: u64,

    // ─── stats config ───────────────────────────────────────────
    #[arg(long, default_value_t = 0)]
    utxo_hash_window: u64,
    /// Only process blocks at least N blocks behind the current tip.
    #[arg(long, default_value_t = 6)]
    finality_depth: u64,

    // ─── recovery ───────────────────────────────────────────────
    /// Enable restart recovery with periodic scanner checkpoints.
    #[arg(long, default_value_t = false)]
    resume: bool,
    /// Directory where scanner checkpoints are written.
    #[arg(long, default_value = ".btc-data-stats/checkpoints")]
    checkpoint_dir: PathBuf,
    /// Write one checkpoint every N processed blocks.
    #[arg(long, default_value_t = 1)]
    checkpoint_every: u64,
    /// Keep the latest N checkpoints.
    #[arg(long, default_value_t = 10)]
    checkpoint_keep: usize,

    // ─── pipeline tuning ────────────────────────────────────────
    #[arg(long, default_value_t = 10)]
    buffer: usize,
    /// Number of contiguous heights per source-level batch request.
    ///
    /// For JSON-RPC this enables batch getblockhash/getblock calls. Keep at 1
    /// for REST/IPC unless benchmarking shows otherwise.
    #[arg(long, default_value_t = 1)]
    source_batch_size: usize,
    /// Pre-allocate approximately this many UTXO entries to reduce map growth overhead.
    #[arg(long, default_value_t = 0)]
    utxo_reserve: usize,
    /// Pre-allocate approximately this many seen P2TR keys to reduce set growth overhead.
    #[arg(long, default_value_t = 0)]
    seen_keys_reserve: usize,
    #[arg(long, default_value_t = 1000)]
    progress: u64,

    // ─── sinks ──────────────────────────────────────────────────
    /// Write per-block CSV stats here.
    #[arg(long)]
    out_per_block: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Cli::parse();
    let source = build_source(&args)?;
    let sink = build_sinks(&args).await?;

    let cfg = ScanConfig {
        start: args.start,
        blocks: args.blocks,
        buffer: args.buffer,
        source_batch_size: args.source_batch_size,
        utxo_reserve: args.utxo_reserve,
        seen_keys_reserve: args.seen_keys_reserve,
        progress: args.progress,
        utxo_hash_window: args.utxo_hash_window,
        finality_depth: args.finality_depth,
        checkpoints: CheckpointConfig {
            dir: args.checkpoint_dir.clone(),
            every: args.checkpoint_every,
            keep: args.checkpoint_keep,
            enabled: args.resume,
        },
    };

    let stats = scan(source, sink, cfg).await?;
    stats.print_report();
    Ok(())
}

fn build_source(args: &Cli) -> anyhow::Result<Arc<dyn BlockSource>> {
    let mut srcs: Vec<Box<dyn BlockSource>> = Vec::new();
    for url in &args.rpc_urls {
        srcs.push(Box::new(RpcSource::new_auto_auth(
            url.clone(),
            args.rpc_user.clone(),
            args.rpc_pass.clone(),
            args.rpc_cookie_file.clone(),
        )?));
    }
    for base in &args.esplora_urls {
        srcs.push(Box::new(EsploraSource::with_rate_limit(
            base.clone(),
            args.esplora_min_gap_ms,
        )));
    }
    for base in &args.rest_urls {
        srcs.push(Box::new(RestSource::new(base.clone())));
    }

    #[cfg(feature = "ipc")]
    {
        for path in &args.ipc_paths {
            srcs.push(Box::new(IpcSource::connect_with_threads(path.clone(), args.ipc_threads)?));
        }
    }

    #[cfg(not(feature = "ipc"))]
    {
        if !args.ipc_paths.is_empty() {
            anyhow::bail!("--ipc requires building with --features ipc");
        }
    }
    if srcs.is_empty() {
        anyhow::bail!("supply at least one --rpc-url, --rest-url, --esplora, or --ipc source");
    }
    if srcs.len() == 1 {
        Ok(Arc::from(srcs.into_iter().next().unwrap()))
    } else {
        Ok(Arc::new(MultiSource::new(srcs)))
    }
}

async fn build_sinks(args: &Cli) -> anyhow::Result<Arc<dyn StatsSink>> {
    let mut sinks: Vec<Arc<dyn StatsSink>> = Vec::new();
    if let Some(p) = &args.out_per_block {
        eprintln!("Per-block CSV → {}", p.display());
        let csv = if args.resume {
            StatsCsvSink::append(p).await?
        } else {
            StatsCsvSink::create(p).await?
        };
        sinks.push(Arc::new(csv));
    }
    Ok(Arc::new(Fanout::new(sinks)))
}