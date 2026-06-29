use anyhow::Context;
use btc_data_light_server::codec::elias_delta::encode_elias_delta_values;
use btc_data_light_server::index::{
    decode_light_block_spent_ids, decode_stored_light_block, encode_light_block,
    light_block_output_count, to_packed_bytes, LightBlockInput, StoredBlockResponseFilter,
    RESPONSE_LABEL_BUDGET_HUNDRED, RESPONSE_LABEL_BUDGET_TWO,
};
use clap::Parser;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

const CSV_HEADER: &[&str] = &[
    "height",
    "stored_block_capnp_bytes",
    "response_block_capnp_bytes_labels100",
    "response_block_capnp_bytes_labels2",
    "estimated_response_block_bytes_with_full_output_keys_labels100",
    "stored_block_minus_response_labels100_bytes",
    "cutthrough_start",
    "cutthrough_tip",
    "filter_reuse",
    "stored_skipped_tx_count_for_tweaks",
    "response_skipped_tx_count_for_tweaks",
    "stored_static_skipped_p2tr_output_slots",
    "stored_total_p2tr_output_slot_count",
    "stored_indexed_output_count",
    "response_candidate_output_count",
    "filtered_omitted_output_count",
    "stored_tweak_entry_count",
    "response_tweak_entry_count",
    "filtered_omitted_tweak_entry_count",
    "stored_spend_entry_count",
    "response_spent_id_count",
    "filtered_omitted_spent_id_count",
    "response_truncated_output_hash_bits_per_output",
    "response_truncated_output_hash_data_bytes",
    "spent_id_codec",
    "response_spent_ids_data_bytes",
    "response_spent_ids_naive_u64_list_bytes",
    "response_spent_ids_naive_u32_list_bytes",
    "spent_id_elias_delta_ascending_absolute_bytes",
    "spent_id_elias_delta_ascending_absolute_savings_vs_u64_bytes",
    "spent_id_elias_delta_ascending_absolute_savings_vs_u32_bytes",
    "spent_id_elias_delta_descending_absolute_bytes",
    "spent_id_elias_delta_descending_absolute_savings_vs_u64_bytes",
    "spent_id_elias_delta_descending_absolute_savings_vs_u32_bytes",
    "spent_id_current_uid_anchor",
    "spent_id_elias_delta_descending_current_anchor_bytes",
    "spent_id_elias_delta_descending_current_anchor_free_savings_bytes",
    "spent_id_elias_delta_descending_current_anchor_with_u64_savings_bytes",
    "spent_id_cutthrough_uid_anchor",
    "spent_id_elias_delta_descending_cutthrough_anchor_bytes",
    "spent_id_elias_delta_descending_cutthrough_anchor_free_savings_bytes",
    "spent_id_elias_delta_descending_cutthrough_anchor_with_u64_savings_bytes",
];

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

    let Some(&archive_tip) = heights.last() else {
        anyhow::bail!("no .capnp block files found in {}", blocks_dir.display());
    };
    let filter = StoredBlockResponseFilter {
        cutthrough_start: args.cutthrough_start,
        cutthrough_tip: args
            .cutthrough_start
            .map(|_| args.cutthrough_tip.unwrap_or(archive_tip)),
        filter_reuse: args.filter_reuse,
        labels: None,
    };
    let cutthrough_uid_anchor = match args.cutthrough_start {
        Some(cutthrough_start) => {
            let path = blocks_dir.join(format!("{cutthrough_start:010}.capnp"));
            let stored_bytes = fs::read(&path).with_context(|| {
                format!(
                    "failed to read cut-through start block {} at {}",
                    cutthrough_start,
                    path.display()
                )
            })?;
            let stored = decode_stored_light_block(&stored_bytes)?;
            Some(stored.first_uid.saturating_sub(1))
        }
        None => None,
    };

    let mut csv = open_csv(&args.csv, args.truncate)?;
    let mut rows = 0usize;
    for height in heights {
        let path = blocks_dir.join(format!("{height:010}.capnp"));
        let row = block_stats_row(height, &path, filter, cutthrough_uid_anchor)
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
        writeln!(writer, "{}", CSV_HEADER.join(","))?;
    }
    Ok(writer)
}

fn block_stats_row(
    height: u64,
    path: &Path,
    filter: StoredBlockResponseFilter,
    cutthrough_uid_anchor: Option<u64>,
) -> anyhow::Result<String> {
    let stored_bytes = fs::read(path)?;
    let stored = decode_stored_light_block(&stored_bytes)?;

    let response_for_label_budget = |labels| -> anyhow::Result<LightBlockInput> {
        let mut filter = filter;
        filter.labels = Some(labels);
        stored.to_filtered_response_input(filter)
    };

    let response = response_for_label_budget(RESPONSE_LABEL_BUDGET_HUNDRED)?;
    let response_100_bytes = packed_response_bytes(&response)?;
    let response_2_bytes_len =
        packed_response_len(&response_for_label_budget(RESPONSE_LABEL_BUDGET_TWO)?)?;

    let response_output_count = light_block_output_count(&response)?;
    let estimated_response_full_key_bytes = response_100_bytes
        .len()
        .saturating_sub(response.truncated_output_hashes.len())
        .saturating_add(response_output_count.saturating_mul(32));

    let stored_p2tr_output_count = stored.outputs.len() + stored.skipped_outputs.len();
    let spent_ids = decode_light_block_spent_ids(&response)?;
    anyhow::ensure!(
        usize::try_from(response.spent_count)? == spent_ids.len(),
        "response spent_count {} does not match decoded spent IDs {} at block {}",
        response.spent_count,
        spent_ids.len(),
        height
    );

    let stored_outputs = stored.outputs.len();
    let response_outputs = response_output_count;
    let stored_tweaks = stored.tweaks.len();
    let response_tweaks = response.tweaks.len();
    let stored_spends = stored.spends.len();
    let response_spends = spent_ids.len();
    let spent_id_list_bytes = response_spends * std::mem::size_of::<u64>();
    let spent_id_list_u32_bytes = response_spends * std::mem::size_of::<u32>();
    let current_uid_anchor = block_current_uid_anchor(stored.first_uid, stored_p2tr_output_count)?;
    let spent_id_stats =
        spent_id_encoding_stats(&spent_ids, current_uid_anchor, cutthrough_uid_anchor)?;

    let row = [
        height.to_string(),
        stored_bytes.len().to_string(),
        response_100_bytes.len().to_string(),
        response_2_bytes_len.to_string(),
        estimated_response_full_key_bytes.to_string(),
        (stored_bytes.len() as i64 - response_100_bytes.len() as i64).to_string(),
        optional_u64(filter.cutthrough_start),
        optional_u64(filter.cutthrough_tip),
        filter.filter_reuse.to_string(),
        stored.skipped_txs_for_tweaks.len().to_string(),
        response.skipped_txs_for_tweaks.len().to_string(),
        stored.skipped_outputs.len().to_string(),
        stored_p2tr_output_count.to_string(),
        stored_outputs.to_string(),
        response_outputs.to_string(),
        stored_outputs.saturating_sub(response_outputs).to_string(),
        stored_tweaks.to_string(),
        response_tweaks.to_string(),
        stored_tweaks.saturating_sub(response_tweaks).to_string(),
        stored_spends.to_string(),
        response_spends.to_string(),
        stored_spends.saturating_sub(response_spends).to_string(),
        response.truncated_output_hash_bits.to_string(),
        response.truncated_output_hashes.len().to_string(),
        "elias_delta_ascending_absolute".to_string(),
        response.spent_ids.len().to_string(),
        spent_id_list_bytes.to_string(),
        spent_id_list_u32_bytes.to_string(),
        spent_id_stats.ascending_absolute_bytes.to_string(),
        byte_savings(spent_id_list_bytes, spent_id_stats.ascending_absolute_bytes).to_string(),
        byte_savings(spent_id_list_u32_bytes, spent_id_stats.ascending_absolute_bytes).to_string(),
        spent_id_stats.descending_absolute_bytes.to_string(),
        byte_savings(
            spent_id_list_bytes,
            spent_id_stats.descending_absolute_bytes,
        )
            .to_string(),
        byte_savings(
            spent_id_list_u32_bytes,
            spent_id_stats.descending_absolute_bytes,
        )
            .to_string(),
        current_uid_anchor.to_string(),
        spent_id_stats.descending_current_anchor_bytes.to_string(),
        byte_savings(
            spent_id_stats.descending_absolute_bytes,
            spent_id_stats.descending_current_anchor_bytes,
        )
            .to_string(),
        byte_savings_with_u64_anchor(
            spent_id_stats.descending_absolute_bytes,
            spent_id_stats.descending_current_anchor_bytes,
        )
            .to_string(),
        optional_u64(spent_id_stats.cutthrough_anchor),
        optional_usize(spent_id_stats.descending_cutthrough_anchor_bytes),
        optional_i64(
            spent_id_stats
                .descending_cutthrough_anchor_bytes
                .map(|bytes| byte_savings(spent_id_stats.descending_absolute_bytes, bytes)),
        ),
        optional_i64(
            spent_id_stats
                .descending_cutthrough_anchor_bytes
                .map(|bytes| {
                    byte_savings_with_u64_anchor(spent_id_stats.descending_absolute_bytes, bytes)
                }),
        ),
    ];
    Ok(row.join(","))
}

fn packed_response_len(response: &LightBlockInput) -> anyhow::Result<usize> {
    Ok(packed_response_bytes(response)?.len())
}

fn packed_response_bytes(response: &LightBlockInput) -> anyhow::Result<Vec<u8>> {
    to_packed_bytes(&encode_light_block(response)?)
}

#[derive(Debug, Default)]
struct SpentIdEncodingStats {
    ascending_absolute_bytes: usize,
    descending_absolute_bytes: usize,
    descending_current_anchor_bytes: usize,
    cutthrough_anchor: Option<u64>,
    descending_cutthrough_anchor_bytes: Option<usize>,
}

fn spent_id_encoding_stats(
    spent_ids: &[u64],
    current_uid_anchor: u64,
    cutthrough_uid_anchor: Option<u64>,
) -> anyhow::Result<SpentIdEncodingStats> {
    let descending_cutthrough_anchor_bytes = cutthrough_uid_anchor
        .map(|anchor| spent_id_elias_delta_descending_anchor_bytes(spent_ids, anchor))
        .transpose()?;

    Ok(SpentIdEncodingStats {
        ascending_absolute_bytes: spent_id_elias_delta_ascending_absolute_bytes(spent_ids)?,
        descending_absolute_bytes: spent_id_elias_delta_descending_absolute_bytes(spent_ids)?,
        descending_current_anchor_bytes: spent_id_elias_delta_descending_anchor_bytes(
            spent_ids,
            current_uid_anchor,
        )?,
        cutthrough_anchor: cutthrough_uid_anchor,
        descending_cutthrough_anchor_bytes,
    })
}

fn spent_id_elias_delta_ascending_absolute_bytes(spent_ids: &[u64]) -> anyhow::Result<usize> {
    if spent_ids.is_empty() {
        return Ok(0);
    }

    let mut ids = spent_ids.to_vec();
    ids.sort_unstable();

    let mut encoded_values = Vec::with_capacity(ids.len());
    let mut prev: Option<u64> = None;
    for uid in ids {
        let value = match prev {
            None => uid
                .checked_add(1)
                .context("spent uid overflow while Elias-delta encoding first value")?,
            Some(prev_uid) => uid
                .checked_sub(prev_uid)
                .context("spent ids not sorted while ascending delta encoding")?,
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

fn spent_id_elias_delta_descending_absolute_bytes(spent_ids: &[u64]) -> anyhow::Result<usize> {
    if spent_ids.is_empty() {
        return Ok(0);
    }

    let mut ids = spent_ids.to_vec();
    ids.sort_unstable_by(|a, b| b.cmp(a));

    let mut encoded_values = Vec::with_capacity(ids.len());
    let mut prev: Option<u64> = None;
    for uid in ids {
        let value = match prev {
            None => uid
                .checked_add(1)
                .context("spent uid overflow while Elias-delta encoding first value")?,
            Some(prev_uid) => prev_uid
                .checked_sub(uid)
                .context("spent ids not sorted while descending delta encoding")?,
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

fn spent_id_elias_delta_descending_anchor_bytes(
    spent_ids: &[u64],
    anchor: u64,
) -> anyhow::Result<usize> {
    if spent_ids.is_empty() {
        return Ok(0);
    }

    let mut ids = spent_ids.to_vec();
    ids.sort_unstable_by(|a, b| b.cmp(a));

    let mut encoded_values = Vec::with_capacity(ids.len());
    let mut prev: Option<u64> = None;
    for uid in ids {
        let value = match prev {
            None => anchor
                .checked_sub(uid)
                .and_then(|offset| offset.checked_add(1))
                .with_context(|| {
                    format!("spent uid {uid} exceeds anchor {anchor} while Elias-delta encoding")
                })?,
            Some(prev_uid) => prev_uid
                .checked_sub(uid)
                .context("spent ids not sorted while descending anchor delta encoding")?,
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

fn block_current_uid_anchor(first_uid: u64, p2tr_output_count: usize) -> anyhow::Result<u64> {
    if p2tr_output_count == 0 {
        return first_uid
            .checked_sub(1)
            .context("first uid must be positive when deriving empty-block UID anchor");
    }

    first_uid
        .checked_add(u64::try_from(p2tr_output_count)?)
        .and_then(|next_uid| next_uid.checked_sub(1))
        .context("block current UID anchor overflow")
}

fn byte_savings(before: usize, after: usize) -> i64 {
    before as i64 - after as i64
}

fn byte_savings_with_u64_anchor(before: usize, after: usize) -> i64 {
    before as i64 - after as i64 - std::mem::size_of::<u64>() as i64
}

fn optional_u64(value: Option<u64>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn optional_usize(value: Option<usize>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn optional_i64(value: Option<i64>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}
