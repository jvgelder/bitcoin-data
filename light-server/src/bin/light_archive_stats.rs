use anyhow::Context;
use btc_data_light_server::codec::elias_delta::encode_elias_delta_values;
use btc_data_light_server::index::{
    decode_stored_light_block, encode_light_block, to_packed_bytes, LightBlockInput,
    StoredBlockResponseFilter,
};
use clap::Parser;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(name = "light-archive-stats")]
#[command(about = "Append per-block stored-vs-response LightBlock statistics to a CSV file")]
struct Args {
    /// Archive root containing blocks/0000000000.capnp files.
    #[arg(long, default_value = "lightdata")]
    archive_dir: PathBuf,

    /// CSV file to append statistics to.
    #[arg(long)]
    csv: PathBuf,

    /// Optional first block height to scan.
    #[arg(long)]
    start: Option<u64>,

    /// Optional final block height to scan, inclusive.
    #[arg(long)]
    end: Option<u64>,

    /// Truncate the CSV and write a fresh header before appending rows.
    #[arg(long)]
    truncate: bool,

    /// Derive the same cut-through filtered response the server would send from this start height.
    #[arg(long)]
    cutthrough_start: Option<u64>,

    /// Cut-through tip height. Defaults to the highest block found in the archive.
    #[arg(long)]
    cutthrough_tip: Option<u64>,

    /// Omit reused outputs when deriving the response block.
    #[arg(long)]
    filter_reuse: bool,

    /// Print progress every N blocks. Set 0 to disable.
    #[arg(long, default_value_t = 1000)]
    log_every: usize,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let blocks_dir = args.archive_dir.join("blocks");
    let mut heights = discover_block_heights(&blocks_dir)?;

    if let Some(start) = args.start {
        heights.retain(|height| *height >= start);
    }
    if let Some(end) = args.end {
        heights.retain(|height| *height <= end);
    }

    anyhow::ensure!(
        !heights.is_empty(),
        "no .capnp block files found in {} for requested range",
        blocks_dir.display()
    );

    let archive_tip = *heights
        .last()
        .expect("heights is non-empty after ensure above");
    let filter = StoredBlockResponseFilter {
        cutthrough_start: args.cutthrough_start,
        cutthrough_tip: args.cutthrough_tip.or(Some(archive_tip)),
        filter_reuse: args.filter_reuse,
    };

    let mut csv = open_csv(&args.csv, args.truncate)?;
    let mut rows = 0usize;
    for height in heights {
        let path = blocks_dir.join(format!("{height:010}.capnp"));
        let row = block_stats_row(height, &path, filter)
            .with_context(|| format!("failed to process block {} at {}", height, path.display()))?;
        writeln!(csv, "{row}")?;
        rows += 1;

        if args.log_every != 0 && rows.is_multiple_of(args.log_every) {
            eprintln!("processed {rows} blocks; latest height={height}");
        }
    }
    csv.flush()?;
    eprintln!("appended {rows} rows to {}", args.csv.display());
    Ok(())
}

fn discover_block_heights(blocks_dir: &Path) -> anyhow::Result<Vec<u64>> {
    let mut heights = Vec::new();
    for entry in fs::read_dir(blocks_dir)
        .with_context(|| format!("failed to read blocks dir {}", blocks_dir.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(prefix) = name.strip_suffix(".capnp") else {
            continue;
        };
        if let Ok(height) = prefix.parse::<u64>() {
            heights.push(height);
        }
    }
    heights.sort_unstable();
    Ok(heights)
}

fn open_csv(path: &Path, truncate: bool) -> anyhow::Result<BufWriter<File>> {
    let needs_header = truncate || fs::metadata(path).map(|m| m.len() == 0).unwrap_or(true);
    let file = if truncate {
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?
    } else {
        OpenOptions::new().create(true).append(true).open(path)?
    };
    let mut writer = BufWriter::new(file);
    if needs_header {
        writeln!(
            writer,
            "height,stored_bytes,response_bytes,stored_minus_response_bytes,skipped_txs_for_tweaks,skipped_outputs,total_outputs,total_tweaks,spent_count,spent_id_list_bytes,spent_id_elias_delta_bytes,spent_id_elias_delta_savings_bytes"
        )?;
    }
    Ok(writer)
}

fn block_stats_row(
    height: u64,
    path: &Path,
    filter: StoredBlockResponseFilter,
) -> anyhow::Result<String> {
    let stored_bytes = fs::read(path)?;
    let stored = decode_stored_light_block(&stored_bytes)?;
    let response = stored.to_filtered_response_input(filter)?;
    let response_msg = encode_light_block(&response)?;
    let response_bytes = to_packed_bytes(&response_msg)?;

    let spent_id_list_bytes = response.spends.len() * std::mem::size_of::<u64>();
    let spent_id_elias_delta_bytes = sorted_spent_id_elias_delta_bytes(&response)?;
    let spent_id_elias_delta_savings_bytes =
        spent_id_list_bytes as i64 - spent_id_elias_delta_bytes as i64;

    Ok(format!(
        "{height},{stored_bytes_len},{response_bytes_len},{stored_minus_response_bytes},{skipped_txs},{skipped_outputs},{total_outputs},{total_tweaks},{spent_count},{spent_id_list_bytes},{spent_id_elias_delta_bytes},{spent_id_elias_delta_savings_bytes}",
        stored_bytes_len = stored_bytes.len(),
        response_bytes_len = response_bytes.len(),
        stored_minus_response_bytes = stored_bytes.len() as i64 - response_bytes.len() as i64,
        skipped_txs = response.skipped_txs_for_tweaks.len(),
        skipped_outputs = response.skipped_outputs.len(),
        total_outputs = response.outputs.len(),
        total_tweaks = response.tweaks.len(),
        spent_count = response.spends.len(),
    ))
}

fn sorted_spent_id_elias_delta_bytes(response: &LightBlockInput) -> anyhow::Result<usize> {
    if response.spends.is_empty() {
        return Ok(0);
    }

    let mut spent_ids = response
        .spends
        .iter()
        .map(|entry| entry.spent_uid)
        .collect::<Vec<_>>();
    spent_ids.sort_unstable();

    let mut encoded_values = Vec::with_capacity(spent_ids.len());
    let mut prev = None;
    for uid in spent_ids {
        let value = match prev {
            None => uid
                .checked_add(1)
                .context("spent uid overflow while Elias-delta encoding first value")?,
            Some(prev_uid) => uid
                .checked_sub(prev_uid)
                .context("spent ids not sorted while delta encoding")?,
        };
        anyhow::ensure!(
            value > 0,
            "duplicate spent uid {uid} cannot be Elias-delta encoded as a positive delta"
        );
        encoded_values.push(value);
        prev = Some(uid);
    }

    Ok(encode_elias_delta_values(&encoded_values)?.len())
}
