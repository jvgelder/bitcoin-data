//! Block-level stats accumulators.
//!
//! Three types live here:
//! - [`Stats`] — overall run summary.
//! - [`PerBlock`] — per-block accumulator, drained into [`BlockStats`] each block.
//! - [`BlockStats`] — frozen per-block snapshot (the row written to CSV /
//!   serialized into capnp / shipped to Kafka).

use crate::histogram::Log2Hist;
use crate::script::{ScriptType, TypeCounts};
use crate::transaction::{SpendClass, SpendPath};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum SpendContext {
    SameTx,
    SameBlock,
    Earlier,
}

#[derive(Default, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct RiceChoice {
    pub k: u32,
    pub bits: u128,
}

#[derive(Default, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct EscapedRiceChoice {
    pub threshold: u64,
    pub k: u32,
    pub bits: u128,
}

pub const RICE_K_COUNT: usize = 33;
pub const ESCAPED_RICE_THRESHOLDS: [u64; 4] = [128, 2048, 16384, 65536];
pub const ESCAPED_RICE_THRESHOLD_COUNT: usize = ESCAPED_RICE_THRESHOLDS.len();
pub const ESCAPED_RICE_MAX_K: usize = 16;
pub const ESCAPED_RICE_K_COUNT: usize = ESCAPED_RICE_MAX_K + 1;
pub const PER_BLOCK_RICE_K_OVERHEAD_BITS: u128 = 5;

mod serde_rice_bits_by_k {
    use super::RICE_K_COUNT;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(value: &[u128; RICE_K_COUNT], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        value.as_slice().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u128; RICE_K_COUNT], D::Error>
    where
        D: Deserializer<'de>,
    {
        let values = Vec::<u128>::deserialize(deserializer)?;
        if values.len() != RICE_K_COUNT {
            return Err(serde::de::Error::invalid_length(
                values.len(),
                &"exactly 33 Rice buckets",
            ));
        }

        let mut out = [0u128; RICE_K_COUNT];
        out.copy_from_slice(&values);
        Ok(out)
    }
}

/// Precomputed codec costs for one value.
///
/// A value is often accounted into multiple estimates (block-local combined,
/// block-local deltas, run-level deltas, run-level combined). Keep the costly
/// codec calculations here so each value is priced once and then accumulated
/// into all required destinations.
#[derive(Clone, Debug)]
pub struct CodecValueCosts {
    pub leb128_bytes: u128,
    pub elias_delta_bits: u128,
    pub rice_bits_by_k: [u128; RICE_K_COUNT],
    pub escaped_rice_bits: [[u128; ESCAPED_RICE_K_COUNT]; ESCAPED_RICE_THRESHOLD_COUNT],
}

impl CodecValueCosts {
    #[inline]
    pub fn new(value: u64, zeroable: bool) -> Self {
        let leb128_bytes = leb128_len_u64(value) as u128;
        let elias_value = if zeroable {
            value.saturating_add(1)
        } else {
            value
        };
        let elias_delta_bits = elias_delta_bits(elias_value) as u128;

        let mut rice_bits_by_k = [0u128; RICE_K_COUNT];
        for (k, slot) in rice_bits_by_k.iter_mut().enumerate() {
            *slot = rice_bits(value, k as u32) as u128;
        }

        let mut escaped_rice_bits = [[0u128; ESCAPED_RICE_K_COUNT]; ESCAPED_RICE_THRESHOLD_COUNT];
        for (threshold_idx, &threshold) in ESCAPED_RICE_THRESHOLDS.iter().enumerate() {
            for (k, bits) in escaped_rice_bits[threshold_idx].iter_mut().enumerate() {
                *bits = if value < threshold {
                    1 + rice_bits(value, k as u32) as u128
                } else {
                    1 + 8 * leb128_bytes
                };
            }
        }

        Self {
            leb128_bytes,
            elias_delta_bits,
            rice_bits_by_k,
            escaped_rice_bits,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CodecEstimate {
    pub values: u64,
    pub leb128_bytes: u128,
    pub elias_delta_bits: u128,
    #[serde(with = "serde_rice_bits_by_k")]
    pub rice_bits_by_k: [u128; RICE_K_COUNT],
    pub escaped_rice_bits: [[u128; ESCAPED_RICE_K_COUNT]; ESCAPED_RICE_THRESHOLD_COUNT],
    pub hist: Log2Hist,
}

impl Default for CodecEstimate {
    fn default() -> Self {
        Self {
            values: 0,
            leb128_bytes: 0,
            elias_delta_bits: 0,
            rice_bits_by_k: [0; RICE_K_COUNT],
            escaped_rice_bits: [[0; ESCAPED_RICE_K_COUNT]; ESCAPED_RICE_THRESHOLD_COUNT],
            hist: Log2Hist::default(),
        }
    }
}

impl CodecEstimate {
    /// Add a value where zero is valid. Elias-delta is charged on `value + 1`.
    pub fn add_zeroable(&mut self, value: u64) {
        let costs = CodecValueCosts::new(value, true);
        self.add_costs(value, &costs);
    }

    /// Add a value known to be >= 1. Elias-delta is charged directly.
    pub fn add_nonzero(&mut self, value: u64) {
        debug_assert!(value > 0);
        let costs = CodecValueCosts::new(value, false);
        self.add_costs(value, &costs);
    }

    /// Backwards-compatible default for zeroable values.
    pub fn add(&mut self, value: u64) {
        self.add_zeroable(value);
    }

    #[inline]
    pub fn add_costs(&mut self, value: u64, costs: &CodecValueCosts) {
        self.add_costs_no_hist(costs);
        self.hist.record(value);
    }

    #[inline]
    pub fn add_costs_no_hist(&mut self, costs: &CodecValueCosts) {
        self.values += 1;
        self.leb128_bytes += costs.leb128_bytes;
        self.elias_delta_bits += costs.elias_delta_bits;
        for (dst, src) in self
            .rice_bits_by_k
            .iter_mut()
            .zip(costs.rice_bits_by_k.iter())
        {
            *dst += *src;
        }
        for (dst_row, src_row) in self
            .escaped_rice_bits
            .iter_mut()
            .zip(costs.escaped_rice_bits.iter())
        {
            for (dst, src) in dst_row.iter_mut().zip(src_row.iter()) {
                *dst += *src;
            }
        }
    }

    pub fn merge(&mut self, other: &CodecEstimate) {
        self.values += other.values;
        self.leb128_bytes += other.leb128_bytes;
        self.elias_delta_bits += other.elias_delta_bits;
        for (dst, src) in self
            .rice_bits_by_k
            .iter_mut()
            .zip(other.rice_bits_by_k.iter())
        {
            *dst += *src;
        }
        for (dst_row, src_row) in self
            .escaped_rice_bits
            .iter_mut()
            .zip(other.escaped_rice_bits.iter())
        {
            for (dst, src) in dst_row.iter_mut().zip(src_row.iter()) {
                *dst += *src;
            }
        }
        self.hist.merge(&other.hist);
    }

    pub fn best_rice(&self) -> RiceChoice {
        let mut best = RiceChoice {
            k: 0,
            bits: u128::MAX,
        };
        for (k, &bits) in self.rice_bits_by_k.iter().enumerate() {
            if bits < best.bits {
                best = RiceChoice { k: k as u32, bits };
            }
        }
        if self.values == 0 {
            RiceChoice { k: 0, bits: 0 }
        } else {
            best
        }
    }

    pub fn best_escaped_rice(&self) -> EscapedRiceChoice {
        if self.values == 0 {
            return EscapedRiceChoice::default();
        }
        let mut best = EscapedRiceChoice {
            threshold: 0,
            k: 0,
            bits: u128::MAX,
        };
        for (threshold_idx, &threshold) in ESCAPED_RICE_THRESHOLDS.iter().enumerate() {
            for (k, &bits) in self.escaped_rice_bits[threshold_idx].iter().enumerate() {
                if bits < best.bits {
                    best = EscapedRiceChoice {
                        threshold,
                        k: k as u32,
                        bits,
                    };
                }
            }
        }
        best
    }

    pub fn print_codec(&self, label: &str) {
        if self.values == 0 {
            println!("{label}: (empty)");
            return;
        }
        let n = self.values as f64;
        let best = self.best_rice();
        let escaped = self.best_escaped_rice();
        println!("{label}:");
        println!("  values:             {}", self.values);
        println!(
            "  LEB128:             {} bytes ({:.3} bytes/value)",
            self.leb128_bytes,
            self.leb128_bytes as f64 / n
        );
        println!(
            "  Elias delta:        {} bits ({:.3} bytes/value)",
            self.elias_delta_bits,
            self.elias_delta_bits as f64 / 8.0 / n
        );
        println!(
            "  Golomb-Rice best:   k={} M={} {} bits ({:.3} bytes/value)",
            best.k,
            1u128 << best.k,
            best.bits,
            best.bits as f64 / 8.0 / n
        );
        println!(
            "  Rice+LEB128 escape: threshold={} k={} {} bits ({:.3} bytes/value)",
            escaped.threshold,
            escaped.k,
            escaped.bits,
            escaped.bits as f64 / 8.0 / n
        );
        self.hist.print(label);
    }
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct SortedUidCodecStats {
    pub blocks_with_spends: u64,
    pub ef_bits_with_64bit_base: u128,
    pub delta_per_block_rice_best_bits_with_k_overhead: u128,
    pub combined_per_block_rice_best_bits_with_k_overhead: u128,
    pub first_offsets: CodecEstimate,
    pub deltas: CodecEstimate,
    pub combined: CodecEstimate,
}

impl SortedUidCodecStats {
    pub fn print_report(&self, label: &str) {
        println!("{label}:");
        println!("  blocks with spends: {}", self.blocks_with_spends);
        let values = self.combined.values.max(1) as f64;
        println!(
            "  Elias-Fano + 64-bit base: {} bits ({:.3} bytes/value)",
            self.ef_bits_with_64bit_base,
            self.ef_bits_with_64bit_base as f64 / 8.0 / values
        );
        if self.deltas.values > 0 {
            println!(
                "  per-block Rice best on deltas (+{} bits/block k): {} bits ({:.3} bytes/value)",
                PER_BLOCK_RICE_K_OVERHEAD_BITS,
                self.delta_per_block_rice_best_bits_with_k_overhead,
                self.delta_per_block_rice_best_bits_with_k_overhead as f64
                    / 8.0
                    / self.deltas.values as f64
            );
        }
        if self.combined.values > 0 {
            println!(
                "  per-block Rice best on combined (+{} bits/block k): {} bits ({:.3} bytes/value)",
                PER_BLOCK_RICE_K_OVERHEAD_BITS,
                self.combined_per_block_rice_best_bits_with_k_overhead,
                self.combined_per_block_rice_best_bits_with_k_overhead as f64
                    / 8.0
                    / self.combined.values as f64
            );
        }
        self.first_offsets
            .print_codec("  first offset from block_anchor_last_uid");
        self.deltas.print_codec("  sorted intra-block deltas");
        self.combined
            .print_codec("  combined first-offset + deltas");
    }
}

#[derive(Default, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SortedUidBlockEstimate {
    pub values: u64,
    pub leb128_bytes: u128,
    pub rice_best_k: u32,
    pub rice_best_bits: u128,
    pub elias_delta_bits: u128,
    pub ef_bits_with_64bit_base: u128,
}

pub fn leb128_len_u64(mut value: u64) -> u64 {
    let mut bytes = 1;
    while value >= 0x80 {
        value >>= 7;
        bytes += 1;
    }
    bytes
}

pub fn rice_bits(value: u64, k: u32) -> u64 {
    (value >> k) + 1 + k as u64
}

pub fn elias_delta_bits(value: u64) -> u64 {
    debug_assert!(value > 0);
    let len = 64 - value.leading_zeros() as u64;
    let len_len = 64 - len.leading_zeros() as u64;
    len + 2 * len_len - 1
}

pub fn elias_fano_bits(universe: u64, n: u64) -> u128 {
    if n == 0 {
        return 0;
    }
    let lower_bits = if universe <= n {
        0
    } else {
        (universe / n).ilog2() as u64
    };
    let lower = n as u128 * lower_bits as u128;
    let upper = n as u128 + (universe >> lower_bits) as u128 + 1;
    lower + upper
}

pub fn measure_sorted_uid_stream(
    uids: &mut [u64],
    block_anchor_last_uid: u64,
    stats: &mut SortedUidCodecStats,
) -> SortedUidBlockEstimate {
    if uids.is_empty() {
        return SortedUidBlockEstimate::default();
    }

    uids.sort_unstable();
    for pair in uids.windows(2) {
        assert_ne!(
            pair[0], pair[1],
            "duplicate spent UID in same sorted UID stream: {}",
            pair[0]
        );
    }

    stats.blocks_with_spends += 1;

    let first_uid = uids[0];
    let first_offset = block_anchor_last_uid
        .checked_sub(first_uid)
        .expect("spent UID exceeds block_anchor_last_uid");

    let mut block_combined = CodecEstimate::default();
    let mut block_deltas = CodecEstimate::default();

    let first_offset_costs = CodecValueCosts::new(first_offset, true);
    block_combined.add_costs_no_hist(&first_offset_costs);
    stats
        .first_offsets
        .add_costs(first_offset, &first_offset_costs);
    stats.combined.add_costs(first_offset, &first_offset_costs);

    let mut prev = first_uid;
    for &uid in &uids[1..] {
        let delta = uid - prev;
        prev = uid;

        let delta_costs = CodecValueCosts::new(delta, false);
        block_combined.add_costs_no_hist(&delta_costs);
        block_deltas.add_costs_no_hist(&delta_costs);
        stats.deltas.add_costs(delta, &delta_costs);
        stats.combined.add_costs(delta, &delta_costs);
    }

    if block_deltas.values > 0 {
        stats.delta_per_block_rice_best_bits_with_k_overhead +=
            block_deltas.best_rice().bits + PER_BLOCK_RICE_K_OVERHEAD_BITS;
    }
    stats.combined_per_block_rice_best_bits_with_k_overhead +=
        block_combined.best_rice().bits + PER_BLOCK_RICE_K_OVERHEAD_BITS;

    let n = uids.len() as u64;
    let max_uid = *uids.last().unwrap();
    let universe = max_uid - first_uid + 1;
    let ef_bits_with_64bit_base = 64 + elias_fano_bits(universe, n);
    stats.ef_bits_with_64bit_base += ef_bits_with_64bit_base;

    let best = block_combined.best_rice();
    SortedUidBlockEstimate {
        values: block_combined.values,
        leb128_bytes: block_combined.leb128_bytes,
        rice_best_k: best.k,
        rice_best_bits: best.bits,
        elias_delta_bits: block_combined.elias_delta_bits,
        ef_bits_with_64bit_base,
    }
}

/// Overall run summary — counters only, no per-spend retention.
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct Stats {
    pub spends: u64,
    pub outputs: u64,

    // Reuse / SP-eligibility on P2TR.
    pub p2tr_outputs: u64,
    pub p2tr_reused: u64,
    pub p2tr_spends: u64,
    pub p2tr_keypath_spends: u64,
    pub p2tr_scriptpath_spends: u64,
    pub p2tr_nums_spends: u64,
    pub p2tr_sp_eligible_spends: u64,

    // Inscription detection (orthogonal to NUMS).
    pub p2tr_inscription_spends: u64,
    pub p2tr_nums_and_inscription_spends: u64,

    // Spend context.
    pub same_tx_spends: u64,
    pub p2tr_same_tx_spends: u64,
    pub same_block_spends: u64,
    pub p2tr_same_block_spends: u64,

    // Tx-level filterability for SP outputs.
    pub nonsp_txs: u64,
    pub nonsp_tx_outputs: u64,

    // Script-type breakdown across the whole run.
    pub script_outputs: TypeCounts,
    pub script_inputs: TypeCounts,

    pub sorted_spent_ids: SortedUidCodecStats,
    pub sorted_p2tr_spent_ids: SortedUidCodecStats,
}

impl Stats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_spend(&mut self, ctx: SpendContext) {
        self.spends += 1;
        match ctx {
            SpendContext::SameTx => self.same_tx_spends += 1,
            SpendContext::SameBlock => self.same_block_spends += 1,
            SpendContext::Earlier => {}
        }
    }

    pub fn record_p2tr_spend(&mut self, class: SpendClass, ctx: SpendContext) {
        self.p2tr_spends += 1;
        match ctx {
            SpendContext::SameTx => self.p2tr_same_tx_spends += 1,
            SpendContext::SameBlock => self.p2tr_same_block_spends += 1,
            SpendContext::Earlier => {}
        }
        match class.path {
            SpendPath::Key => {
                self.p2tr_keypath_spends += 1;
                self.p2tr_sp_eligible_spends += 1;
            }
            SpendPath::ScriptNonNums => {
                self.p2tr_scriptpath_spends += 1;
                self.p2tr_sp_eligible_spends += 1;
            }
            SpendPath::ScriptNums => {
                self.p2tr_scriptpath_spends += 1;
                self.p2tr_nums_spends += 1;
            }
        }
        if class.inscription {
            self.p2tr_inscription_spends += 1;
            if class.path == SpendPath::ScriptNums {
                self.p2tr_nums_and_inscription_spends += 1;
            }
        }
    }

    pub fn record_output_script(&mut self, t: ScriptType) {
        self.script_outputs.record(t);
    }

    pub fn record_input_script(&mut self, t: ScriptType) {
        self.script_inputs.record(t);
    }

    pub fn print_report(&self) {
        println!("\n==== Scan Summary ====");
        println!("Total outputs:       {}", self.outputs);
        println!("  P2TR outputs:      {}", self.p2tr_outputs);
        println!(
            "  P2TR reused:       {} ({:.1}%)",
            self.p2tr_reused,
            100.0 * self.p2tr_reused as f64 / self.p2tr_outputs.max(1) as f64
        );
        println!("Total spends:        {}", self.spends);

        let p2tr_denom = self.p2tr_spends.max(1) as f64;
        println!("P2TR spends:         {}", self.p2tr_spends);
        println!(
            "  Key-path:          {} ({:.1}%)",
            self.p2tr_keypath_spends,
            100.0 * self.p2tr_keypath_spends as f64 / p2tr_denom
        );
        println!(
            "  Script-path:       {} ({:.1}%)",
            self.p2tr_scriptpath_spends,
            100.0 * self.p2tr_scriptpath_spends as f64 / p2tr_denom
        );
        println!(
            "    NUMS internal key:    {} ({:.2}%)",
            self.p2tr_nums_spends,
            100.0 * self.p2tr_nums_spends as f64 / p2tr_denom
        );
        println!(
            "    Inscriptions (any):   {} ({:.2}%)",
            self.p2tr_inscription_spends,
            100.0 * self.p2tr_inscription_spends as f64 / p2tr_denom
        );
        println!(
            "    NUMS ∩ inscription:   {} ({:.2}%)",
            self.p2tr_nums_and_inscription_spends,
            100.0 * self.p2tr_nums_and_inscription_spends as f64 / p2tr_denom
        );
        println!(
            "  SP-eligible:       {} ({:.1}%)",
            self.p2tr_sp_eligible_spends,
            100.0 * self.p2tr_sp_eligible_spends as f64 / p2tr_denom
        );

        println!();
        println!("Same-tx spends (canary, should be 0):");
        println!("  all:               {}", self.same_tx_spends);
        println!("  P2TR:              {}", self.p2tr_same_tx_spends);
        println!("Same-block spends (later tx, same block):");
        println!(
            "  all:               {} ({:.2}% of spends)",
            self.same_block_spends,
            100.0 * self.same_block_spends as f64 / self.spends.max(1) as f64
        );
        println!(
            "  P2TR:              {} ({:.2}% of P2TR spends)",
            self.p2tr_same_block_spends,
            100.0 * self.p2tr_same_block_spends as f64 / p2tr_denom
        );

        println!();
        println!(
            "SP-ineligible txs:   {} (all P2TR inputs NUMS, no other input types)",
            self.nonsp_txs
        );
        println!(
            "  Their outputs:     {} ({:.2}% of all outputs, filterable)",
            self.nonsp_tx_outputs,
            100.0 * self.nonsp_tx_outputs as f64 / self.outputs.max(1) as f64
        );

        println!();
        print_script_type_table("Outputs by script type", &self.script_outputs, self.outputs);
        let total_inputs: u64 = self.script_inputs.p2pk
            + self.script_inputs.p2pkh
            + self.script_inputs.p2sh
            + self.script_inputs.p2wpkh
            + self.script_inputs.p2wsh
            + self.script_inputs.p2tr
            + self.script_inputs.p2a
            + self.script_inputs.op_return
            + self.script_inputs.unknown;
        print_script_type_table(
            "Inputs by spent script type",
            &self.script_inputs,
            total_inputs,
        );

        println!();
        println!("==== Spent-ID Sorted Delta Encoding ====");
        self.sorted_spent_ids.print_report("All spends");
        println!();
        self.sorted_p2tr_spent_ids.print_report("P2TR spends");
    }
}

fn print_script_type_table(label: &str, c: &TypeCounts, total: u64) {
    let denom = total.max(1) as f64;
    let row = |name: &str, n: u64| {
        println!(
            "  {:<12} {:>10}  ({:.2}%)",
            name,
            n,
            100.0 * n as f64 / denom
        );
    };
    println!("{label}: total={}", total);
    row("p2pk", c.p2pk);
    row("p2pkh", c.p2pkh);
    row("p2sh", c.p2sh);
    row("p2wpkh", c.p2wpkh);
    row("p2wsh", c.p2wsh);
    row("p2tr", c.p2tr);
    row("p2a", c.p2a);
    row("op_return", c.op_return);
    row("unknown", c.unknown);
}

/// Metrics that can be computed from the block itself.
///
/// These are restart-friendly and can be keyed by `(height, block_hash,
/// stats_version)` in durable sinks.
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct BlockLocalStats {
    pub output_count: u64,
    pub p2tr_output_count: u64,
    pub script_outputs: TypeCounts,
}

/// Metrics that depend on previous-chain state such as the UTXO map,
/// historical P2TR keys, or global output IDs.
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct ChainDerivedStats {
    pub last_global_id: u64,
    pub p2tr_reused_count: u64,
    pub spends: u64,
    pub p2tr_spends: u64,
    pub p2tr_keypath_spends: u64,
    pub p2tr_scriptpath_spends: u64,
    pub p2tr_nums_spends: u64,
    pub p2tr_sp_eligible_spends: u64,
    pub p2tr_inscription_spends: u64,
    pub p2tr_nums_and_inscription_spends: u64,
    pub nonsp_txs: u64,
    pub nonsp_tx_outputs: u64,
    pub script_inputs: TypeCounts,
    pub utxo_hash: String,
    pub utxo_hash_window: u64,
}

/// Frozen per-block snapshot. One per scanned block.
///
/// The flat fields are retained for CSV/backwards compatibility, while
/// `local` and `chain` make the two stats classes explicit for future
/// SQLite/DuckDB sinks.
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct BlockStats {
    pub height: u64,
    pub block_hash: String,
    pub stats_version: u32,
    pub local: BlockLocalStats,
    pub chain: ChainDerivedStats,
    pub last_global_id: u64,
    pub output_count: u64,
    pub p2tr_output_count: u64,
    pub p2tr_reused_count: u64,
    pub spends: u64,
    pub p2tr_spends: u64,
    pub p2tr_keypath_spends: u64,
    pub p2tr_scriptpath_spends: u64,
    pub p2tr_nums_spends: u64,
    pub p2tr_sp_eligible_spends: u64,
    pub p2tr_inscription_spends: u64,
    pub p2tr_nums_and_inscription_spends: u64,
    pub nonsp_txs: u64,
    pub nonsp_tx_outputs: u64,

    /// Per-block script-type breakdowns.
    pub script_outputs: TypeCounts,
    pub script_inputs: TypeCounts,

    pub sorted_spent_values: u64,
    pub sorted_spent_leb128_bytes: u128,
    pub sorted_spent_rice_best_k: u32,
    pub sorted_spent_rice_best_bits: u128,
    pub sorted_spent_elias_delta_bits: u128,
    pub sorted_spent_ef_bits_with_64bit_base: u128,

    pub sorted_p2tr_spent_values: u64,
    pub sorted_p2tr_spent_leb128_bytes: u128,
    pub sorted_p2tr_spent_rice_best_k: u32,
    pub sorted_p2tr_spent_rice_best_bits: u128,
    pub sorted_p2tr_spent_elias_delta_bits: u128,
    pub sorted_p2tr_spent_ef_bits_with_64bit_base: u128,

    pub utxo_hash: String,
    pub utxo_hash_window: u64,
}

impl BlockStats {
    pub fn new(height: u64) -> Self {
        Self {
            height,
            stats_version: crate::STATS_STATE_VERSION,
            ..Default::default()
        }
    }

    /// Refresh typed stats classes from the flat compatibility fields.
    pub fn refresh_classes(&mut self) {
        self.local = BlockLocalStats {
            output_count: self.output_count,
            p2tr_output_count: self.p2tr_output_count,
            script_outputs: self.script_outputs.clone(),
        };
        self.chain = ChainDerivedStats {
            last_global_id: self.last_global_id,
            p2tr_reused_count: self.p2tr_reused_count,
            spends: self.spends,
            p2tr_spends: self.p2tr_spends,
            p2tr_keypath_spends: self.p2tr_keypath_spends,
            p2tr_scriptpath_spends: self.p2tr_scriptpath_spends,
            p2tr_nums_spends: self.p2tr_nums_spends,
            p2tr_sp_eligible_spends: self.p2tr_sp_eligible_spends,
            p2tr_inscription_spends: self.p2tr_inscription_spends,
            p2tr_nums_and_inscription_spends: self.p2tr_nums_and_inscription_spends,
            nonsp_txs: self.nonsp_txs,
            nonsp_tx_outputs: self.nonsp_tx_outputs,
            script_inputs: self.script_inputs.clone(),
            utxo_hash: self.utxo_hash.clone(),
            utxo_hash_window: self.utxo_hash_window,
        };
    }
}

/// Per-block counter accumulator.
#[derive(Default)]
pub struct PerBlock {
    pub spends: u64,
    pub output_count: u64,
    pub p2tr_output_count: u64,
    pub p2tr_reused_count: u64,
    pub p2tr_spends: u64,
    pub p2tr_keypath_spends: u64,
    pub p2tr_scriptpath_spends: u64,
    pub p2tr_nums_spends: u64,
    pub p2tr_sp_eligible_spends: u64,
    pub p2tr_inscription_spends: u64,
    pub p2tr_nums_and_inscription_spends: u64,
    pub nonsp_txs: u64,
    pub nonsp_tx_outputs: u64,
    pub script_outputs: TypeCounts,
    pub script_inputs: TypeCounts,
    pub spent_uids: Vec<u64>,
    pub p2tr_spent_uids: Vec<u64>,
}

impl PerBlock {
    pub fn record_spend_uid(&mut self, uid: u64, is_p2tr: bool) {
        self.spends += 1;
        self.spent_uids.push(uid);
        if is_p2tr {
            self.p2tr_spent_uids.push(uid);
        }
    }

    pub fn measure_sorted_uid_encoding(
        &mut self,
        block_anchor_last_uid: u64,
        run_stats: &mut Stats,
        row: &mut BlockStats,
    ) {
        let all = measure_sorted_uid_stream(
            &mut self.spent_uids,
            block_anchor_last_uid,
            &mut run_stats.sorted_spent_ids,
        );
        row.sorted_spent_values = all.values;
        row.sorted_spent_leb128_bytes = all.leb128_bytes;
        row.sorted_spent_rice_best_k = all.rice_best_k;
        row.sorted_spent_rice_best_bits = all.rice_best_bits;
        row.sorted_spent_elias_delta_bits = all.elias_delta_bits;
        row.sorted_spent_ef_bits_with_64bit_base = all.ef_bits_with_64bit_base;

        let p2tr = measure_sorted_uid_stream(
            &mut self.p2tr_spent_uids,
            block_anchor_last_uid,
            &mut run_stats.sorted_p2tr_spent_ids,
        );
        row.sorted_p2tr_spent_values = p2tr.values;
        row.sorted_p2tr_spent_leb128_bytes = p2tr.leb128_bytes;
        row.sorted_p2tr_spent_rice_best_k = p2tr.rice_best_k;
        row.sorted_p2tr_spent_rice_best_bits = p2tr.rice_best_bits;
        row.sorted_p2tr_spent_elias_delta_bits = p2tr.elias_delta_bits;
        row.sorted_p2tr_spent_ef_bits_with_64bit_base = p2tr.ef_bits_with_64bit_base;
    }

    pub fn record_p2tr_spend(&mut self, class: SpendClass) {
        self.p2tr_spends += 1;
        match class.path {
            SpendPath::Key => {
                self.p2tr_keypath_spends += 1;
                self.p2tr_sp_eligible_spends += 1;
            }
            SpendPath::ScriptNonNums => {
                self.p2tr_scriptpath_spends += 1;
                self.p2tr_sp_eligible_spends += 1;
            }
            SpendPath::ScriptNums => {
                self.p2tr_scriptpath_spends += 1;
                self.p2tr_nums_spends += 1;
            }
        }
        if class.inscription {
            self.p2tr_inscription_spends += 1;
            if class.path == SpendPath::ScriptNums {
                self.p2tr_nums_and_inscription_spends += 1;
            }
        }
    }

    pub fn record_output(&mut self, is_p2tr: bool, reused: bool) {
        self.output_count += 1;
        if is_p2tr {
            self.p2tr_output_count += 1;
            if reused {
                self.p2tr_reused_count += 1;
            }
        }
    }

    pub fn record_output_script(&mut self, t: ScriptType) {
        self.script_outputs.record(t);
    }

    pub fn record_input_script(&mut self, t: ScriptType) {
        self.script_inputs.record(t);
    }

    pub fn drain_into(&mut self, row: &mut BlockStats) {
        row.spends = self.spends;
        row.output_count = self.output_count;
        row.p2tr_output_count = self.p2tr_output_count;
        row.p2tr_reused_count = self.p2tr_reused_count;
        row.p2tr_spends = self.p2tr_spends;
        row.p2tr_keypath_spends = self.p2tr_keypath_spends;
        row.p2tr_scriptpath_spends = self.p2tr_scriptpath_spends;
        row.p2tr_nums_spends = self.p2tr_nums_spends;
        row.p2tr_sp_eligible_spends = self.p2tr_sp_eligible_spends;
        row.p2tr_inscription_spends = self.p2tr_inscription_spends;
        row.p2tr_nums_and_inscription_spends = self.p2tr_nums_and_inscription_spends;
        row.nonsp_txs = self.nonsp_txs;
        row.nonsp_tx_outputs = self.nonsp_tx_outputs;
        row.script_outputs = std::mem::take(&mut self.script_outputs);
        row.script_inputs = std::mem::take(&mut self.script_inputs);

        // Preserve UID vector allocations across blocks. These buffers are used
        // only by the sequential scanner, so clearing them here keeps memory
        // bounded by the largest observed block without introducing sharing
        // across parallel prepare tasks.
        self.spent_uids.clear();
        self.p2tr_spent_uids.clear();

        self.spends = 0;
        self.output_count = 0;
        self.p2tr_output_count = 0;
        self.p2tr_reused_count = 0;
        self.p2tr_spends = 0;
        self.p2tr_keypath_spends = 0;
        self.p2tr_scriptpath_spends = 0;
        self.p2tr_nums_spends = 0;
        self.p2tr_sp_eligible_spends = 0;
        self.p2tr_inscription_spends = 0;
        self.p2tr_nums_and_inscription_spends = 0;
        self.nonsp_txs = 0;
        self.nonsp_tx_outputs = 0;
    }
}
