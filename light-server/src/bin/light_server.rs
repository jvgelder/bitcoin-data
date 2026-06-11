use btc_data_light_server::server::{serve, ServerConfig};
use btc_data_light_server::storage::ArchiveBackend;
use btc_data_light_server::storage::FileArchive;
use btc_data_light_server::storage::SqliteArchive;
use clap::{Parser, ValueEnum};
use std::sync::Arc;

#[derive(Debug, Clone, ValueEnum)]
enum BackendKind {
    Sqlite,
    Files,
}

#[derive(Debug, Parser)]
struct Args {
    /// Address to bind, for example 127.0.0.1:3000.
    #[arg(long, default_value = "127.0.0.1:3000")]
    bind: String,

    /// Storage backend. SQLite is the default production backend; files are useful for simple fixtures.
    #[arg(long, value_enum, default_value_t = BackendKind::Sqlite)]
    backend: BackendKind,

    /// SQLite database URL, for example sqlite:lightdata.db.
    #[arg(long, default_value = "sqlite:lightdata.db")]
    database_url: String,

    /// File-backed archive root, used when --backend files.
    #[arg(long, default_value = "lightdata")]
    archive: std::path::PathBuf,

    /// Run SQLx migrations before serving. Only valid with --backend sqlite.
    #[arg(long, default_value_t = false)]
    migrate: bool,

    /// Maximum number of blocks served by one range request.
    #[arg(long, default_value_t = btc_data_light_server::DEFAULT_MAX_RANGE_COUNT)]
    max_range_count: u32,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let archive: Arc<dyn ArchiveBackend> = match args.backend {
        BackendKind::Sqlite => {
            let archive = SqliteArchive::connect(&args.database_url, args.migrate).await?;
            if args.migrate {
                archive.migrate().await?;
            }
            archive.validate_schema_compatibility().await?;
            Arc::new(archive)
        }
        BackendKind::Files => Arc::new(FileArchive::new(args.archive)),
    };

    let mut config = ServerConfig::new(args.bind);
    config.max_range_count = args.max_range_count;
    serve(config, archive).await
}
