use btc_data_light_server::server::{serve, ServerConfig};
use btc_data_light_server::storage::{ArchiveBackend, FileArchive};
use clap::Parser;
use std::sync::Arc;

#[derive(Debug, Parser)]
struct Args {
    /// Address to bind, for example 127.0.0.1:3000.
    #[arg(long, default_value = "127.0.0.1:3000")]
    bind: String,

    /// File-backed archive root.
    #[arg(long, default_value = "lightdata")]
    archive: std::path::PathBuf,

    /// Maximum number of blocks served by one range request.
    #[arg(long, default_value_t = btc_data_light_server::DEFAULT_MAX_RANGE_COUNT)]
    max_range_count: u32,

    /// Maximum encoded response size for /blocks/light range responses.
    #[arg(long, default_value_t = btc_data_light_server::DEFAULT_MAX_RESPONSE_BYTES)]
    max_response_bytes: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let archive: Arc<dyn ArchiveBackend> = Arc::new(FileArchive::new(args.archive));

    let config = ServerConfig {
        bind: args.bind,
        max_range_count: args.max_range_count,
        max_response_bytes: args.max_response_bytes,
    };
    serve(config, archive).await
}
