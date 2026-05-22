//! `btc-data` — fetches blocks from one or more sources and (eventually)
//! emits factual events (RawBlock, Tip, Hash) to sinks.
//!
//! Stats live in the `btc-data-stats` binary (in the `stats` crate).
//!
//! Today this binary just downloads blocks and prints a summary. Block /
//! event sinks (capnp-IPC, kafka, etc.) plug into the `// TODO sinks`
//! comment below once their crates are implemented.

use btc_data_core::pipeline;
use btc_data_core::source::BlockSource;
use btc_data_sources::{EsploraSource, MultiSource, RestSource, RpcSource};
#[cfg(feature = "ipc")]
use btc_data_sources::IpcSource;
use clap::Parser;
use futures::StreamExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

#[derive(Parser)]
#[command(name = "btc-data", about = "Fetch Bitcoin blocks and emit events", version)]
struct Cli {
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

    #[arg(long, default_value_t = 709_632)]
    start: u64,
    #[arg(long, default_value_t = 1_000)]
    blocks: u64,
    #[arg(long, default_value_t = 10)]
    buffer: usize,
    /// Number of contiguous heights per source-level batch request.
    ///
    /// For JSON-RPC this enables batch getblockhash/getblock calls. Keep at 1
    /// for REST/IPC unless benchmarking shows otherwise.
    #[arg(long, default_value_t = 1)]
    source_batch_size: usize,
    #[arg(long, default_value_t = 1000)]
    progress: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Cli::parse();
    let source = build_source(&args)?;

    eprintln!(
        "Fetch {} blocks (in-flight={}, source-batch-size={})...",
        args.blocks,
        args.buffer,
        args.source_batch_size
    );
    let t1 = Instant::now();
    let mut stream = pipeline::raw_block_stream_by_height_batched(
        source.clone(),
        args.start,
        args.blocks,
        args.buffer,
        args.source_batch_size,
    );
    let mut count: u64 = 0;
    let mut bytes: u64 = 0;
    while let Some(item) = stream.next().await {
        let frame = item?;
        count += 1;
        bytes += frame.bytes.len() as u64;
        // TODO: emit to sinks (capnp IPC, kafka, websocket, ...).
        if args.progress > 0 && count % args.progress == 0 {
            let bps = count as f64 / t1.elapsed().as_secs_f64().max(0.001);
            eprintln!("  fetched={count:>6} bytes={bytes:>12} {bps:>5.1} blk/s");
        }
    }
    eprintln!(
        "Done: {} blocks, {} bytes in {:.1}s",
        count, bytes, t1.elapsed().as_secs_f64()
    );
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