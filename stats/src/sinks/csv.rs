//! CSV sink for `BlockStats` rows.

use crate::sinks::StatsSink;
use crate::BlockStats;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::Mutex;

/// CSV header. Columns are grouped by concern:
/// - identity & totals
/// - P2TR breakdown including inscriptions
/// - non-SP-eligible tx filter
/// - outputs by script type (created in this block)
/// - inputs by spent script type (prevout type)
/// - config snapshot + utxo accumulator
pub const BLOCK_STATS_HEADER: &str = concat!(
"height,block_hash,stats_version,last_global_id,output_count,p2tr_output_count,p2tr_reused_count,",
"spends,",
"p2tr_spends,p2tr_keypath_spends,p2tr_scriptpath_spends,p2tr_nums_spends,",
"p2tr_sp_eligible_spends,p2tr_inscription_spends,p2tr_nums_and_inscription_spends,",
"nonsp_txs,nonsp_tx_outputs,",
"out_p2pk,out_p2pkh,out_p2sh,out_p2wpkh,out_p2wsh,out_p2tr,out_p2a,out_op_return,out_unknown,",
"in_p2pk,in_p2pkh,in_p2sh,in_p2wpkh,in_p2wsh,in_p2tr,in_p2a,in_op_return,in_unknown,",
"sorted_spent_values,sorted_spent_leb128_bytes,",
"sorted_spent_rice_best_k,sorted_spent_rice_best_bits,sorted_spent_elias_delta_bits,sorted_spent_ef_bits_with_64bit_base,",
"sorted_p2tr_spent_values,sorted_p2tr_spent_leb128_bytes,",
"sorted_p2tr_spent_rice_best_k,sorted_p2tr_spent_rice_best_bits,sorted_p2tr_spent_elias_delta_bits,sorted_p2tr_spent_ef_bits_with_64bit_base,",
"utxo_hash,utxo_hash_window",
);

pub struct StatsCsvSink {
    path: PathBuf,
    name: String,
    inner: Mutex<BufWriter<File>>,
}

impl StatsCsvSink {
    pub async fn create(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new().create(true).write(true).truncate(true).open(&path).await?;
        let mut w = BufWriter::new(file);
        w.write_all(BLOCK_STATS_HEADER.as_bytes()).await?;
        w.write_all(b"\n").await?;
        let name = format!("csv:{}", path.display());
        Ok(Self { path, name, inner: Mutex::new(w) })
    }

    pub async fn append(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new().create(true).append(true).read(true).open(&path).await?;
        let len = file.metadata().await?.len();
        let mut w = BufWriter::new(file);
        if len == 0 {
            w.write_all(BLOCK_STATS_HEADER.as_bytes()).await?;
            w.write_all(b"\n").await?;
        }
        let name = format!("csv:{}", path.display());
        Ok(Self { path, name, inner: Mutex::new(w) })
    }

    pub fn path(&self) -> &Path { &self.path }
}

#[async_trait]
impl StatsSink for StatsCsvSink {
    async fn emit_block(&self, r: &BlockStats) -> anyhow::Result<()> {
        let so = &r.script_outputs;
        let si = &r.script_inputs;
        let fields = [
            r.height.to_string(),
            r.block_hash.clone(),
            r.stats_version.to_string(),
            r.last_global_id.to_string(),
            r.output_count.to_string(),
            r.p2tr_output_count.to_string(),
            r.p2tr_reused_count.to_string(),
            r.spends.to_string(),
            r.p2tr_spends.to_string(),
            r.p2tr_keypath_spends.to_string(),
            r.p2tr_scriptpath_spends.to_string(),
            r.p2tr_nums_spends.to_string(),
            r.p2tr_sp_eligible_spends.to_string(),
            r.p2tr_inscription_spends.to_string(),
            r.p2tr_nums_and_inscription_spends.to_string(),
            r.nonsp_txs.to_string(),
            r.nonsp_tx_outputs.to_string(),
            so.p2pk.to_string(),
            so.p2pkh.to_string(),
            so.p2sh.to_string(),
            so.p2wpkh.to_string(),
            so.p2wsh.to_string(),
            so.p2tr.to_string(),
            so.p2a.to_string(),
            so.op_return.to_string(),
            so.unknown.to_string(),
            si.p2pk.to_string(),
            si.p2pkh.to_string(),
            si.p2sh.to_string(),
            si.p2wpkh.to_string(),
            si.p2wsh.to_string(),
            si.p2tr.to_string(),
            si.p2a.to_string(),
            si.op_return.to_string(),
            si.unknown.to_string(),
            r.sorted_spent_values.to_string(),
            r.sorted_spent_leb128_bytes.to_string(),
            r.sorted_spent_rice_best_k.to_string(),
            r.sorted_spent_rice_best_bits.to_string(),
            r.sorted_spent_elias_delta_bits.to_string(),
            r.sorted_spent_ef_bits_with_64bit_base.to_string(),
            r.sorted_p2tr_spent_values.to_string(),
            r.sorted_p2tr_spent_leb128_bytes.to_string(),
            r.sorted_p2tr_spent_rice_best_k.to_string(),
            r.sorted_p2tr_spent_rice_best_bits.to_string(),
            r.sorted_p2tr_spent_elias_delta_bits.to_string(),
            r.sorted_p2tr_spent_ef_bits_with_64bit_base.to_string(),
            r.utxo_hash.clone(),
            r.utxo_hash_window.to_string(),
        ];
        let line = format!("{}\n", fields.join(","));
        self.inner.lock().await.write_all(line.as_bytes()).await?;
        Ok(())
    }

    async fn rollback_to_height(&self, height: u64) -> anyhow::Result<()> {
        self.inner.lock().await.flush().await?;
        rollback_csv_to_height(&self.path, height).await
    }

    async fn flush(&self) -> anyhow::Result<()> {
        self.inner.lock().await.flush().await?;
        Ok(())
    }

    fn name(&self) -> &str { &self.name }
}

async fn rollback_csv_to_height(path: &Path, height: u64) -> anyhow::Result<()> {
    let content = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };

    let mut out = String::new();
    for (i, line) in content.lines().enumerate() {
        if i == 0 {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let Some(first) = line.split(',').next() else { continue; };
        let Ok(row_height) = first.parse::<u64>() else { continue; };
        if row_height <= height {
            out.push_str(line);
            out.push('\n');
        }
    }

    tokio::fs::write(path, out).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_column_count_matches_row() {
        let header_cols = BLOCK_STATS_HEADER.split(',').count();
        assert_eq!(header_cols, 49, "header column count drift");
    }
}