//! Stats accumulators → capnp Stats messages.

use crate::stats_capnp::{block_stats, log2_hist, stats};
use crate::{BlockStats, Log2Hist, Stats};
use capnp::message::{Builder, HeapAllocator};

/// Encode the per-block snapshot.
pub fn encode_block_stats(src: &BlockStats) -> Builder<HeapAllocator> {
    let mut msg = Builder::new_default();
    {
        let b = msg.init_root::<block_stats::Builder>();
        fill_block_stats(b, src);
    }
    msg
}

/// Encode a log2 histogram on its own.
pub fn encode_log2_hist(src: &Log2Hist) -> Builder<HeapAllocator> {
    let mut msg = Builder::new_default();
    {
        let b = msg.init_root::<log2_hist::Builder>();
        fill_log2_hist(b, src);
    }
    msg
}

/// Encode the run-level Stats rollup. The contained `BlockStats` is filled
/// from the totals carried in `src`; height/last_global_id/utxo metadata
/// must be supplied by the caller because `Stats` doesn't track them.
pub fn encode_stats(
    src: &Stats,
    height: u64,
    last_global_id: u64,
    utxo_hash: &[u8],
    utxo_hash_window: u64,
) -> Builder<HeapAllocator> {
    let mut msg = Builder::new_default();
    {
        let mut b = msg.init_root::<stats::Builder>();
        {
            let bs = b.reborrow().init_block();
            let snap =
                stats_to_block_snapshot(src, height, last_global_id, utxo_hash, utxo_hash_window);
            fill_block_stats(bs, &snap);
        }
    }
    msg
}

/// Project a run-level `Stats` into a `BlockStats` snapshot.
fn stats_to_block_snapshot(
    s: &Stats,
    height: u64,
    last_global_id: u64,
    utxo_hash: &[u8],
    utxo_hash_window: u64,
) -> BlockStats {
    let sorted_spent_rice = s.sorted_spent_ids.combined.best_rice();
    let sorted_p2tr_spent_rice = s.sorted_p2tr_spent_ids.combined.best_rice();

    BlockStats {
        height,
        last_global_id,
        output_count: s.outputs,
        p2tr_output_count: s.p2tr_outputs,
        p2tr_reused_count: s.p2tr_reused,
        spends: s.spends,
        p2tr_spends: s.p2tr_spends,
        p2tr_keypath_spends: s.p2tr_keypath_spends,
        p2tr_scriptpath_spends: s.p2tr_scriptpath_spends,
        p2tr_nums_spends: s.p2tr_nums_spends,
        p2tr_sp_eligible_spends: s.p2tr_sp_eligible_spends,
        p2tr_inscription_spends: s.p2tr_inscription_spends,
        p2tr_nums_and_inscription_spends: s.p2tr_nums_and_inscription_spends,
        nonsp_txs: s.nonsp_txs,
        nonsp_tx_outputs: s.nonsp_tx_outputs,
        sorted_spent_values: s.sorted_spent_ids.combined.values,
        sorted_spent_leb128_bytes: s.sorted_spent_ids.combined.leb128_bytes,
        sorted_spent_rice_best_k: sorted_spent_rice.k,
        sorted_spent_rice_best_bits: sorted_spent_rice.bits,
        sorted_spent_elias_delta_bits: s.sorted_spent_ids.combined.elias_delta_bits,
        sorted_spent_ef_bits_with_64bit_base: s.sorted_spent_ids.ef_bits_with_64bit_base,
        sorted_p2tr_spent_values: s.sorted_p2tr_spent_ids.combined.values,
        sorted_p2tr_spent_leb128_bytes: s.sorted_p2tr_spent_ids.combined.leb128_bytes,
        sorted_p2tr_spent_rice_best_k: sorted_p2tr_spent_rice.k,
        sorted_p2tr_spent_rice_best_bits: sorted_p2tr_spent_rice.bits,
        sorted_p2tr_spent_elias_delta_bits: s.sorted_p2tr_spent_ids.combined.elias_delta_bits,
        sorted_p2tr_spent_ef_bits_with_64bit_base: s.sorted_p2tr_spent_ids.ef_bits_with_64bit_base,
        script_outputs: Default::default(),
        script_inputs: Default::default(),
        utxo_hash: hex::encode(utxo_hash),
        utxo_hash_window,
        ..Default::default()
    }
}

fn fill_block_stats(mut b: block_stats::Builder<'_>, src: &BlockStats) {
    b.set_height(src.height);
    b.set_last_global_id(src.last_global_id);
    b.set_output_count(src.output_count);
    b.set_p2tr_output_count(src.p2tr_output_count);
    b.set_p2tr_reused_count(src.p2tr_reused_count);
    b.set_spends(src.spends);
    b.set_p2tr_spends(src.p2tr_spends);
    b.set_p2tr_keypath_spends(src.p2tr_keypath_spends);
    b.set_p2tr_scriptpath_spends(src.p2tr_scriptpath_spends);
    b.set_p2tr_nums_spends(src.p2tr_nums_spends);
    b.set_p2tr_sp_eligible_spends(src.p2tr_sp_eligible_spends);
    b.set_nonsp_txs(src.nonsp_txs);
    b.set_nonsp_tx_outputs(src.nonsp_tx_outputs);
    // utxo_hash on the BlockStats Rust struct is a hex string; we
    // re-decode to bytes for the wire form to keep the schema typed `Data`.
    let bytes = hex::decode(&src.utxo_hash).unwrap_or_default();
    b.set_utxo_hash(&bytes);
    b.set_utxo_hash_window(src.utxo_hash_window);
    b.set_sorted_spent_values(src.sorted_spent_values);
    b.set_sorted_spent_leb128_bytes(u128_to_u64(src.sorted_spent_leb128_bytes));
    b.set_sorted_spent_rice_best_k(src.sorted_spent_rice_best_k);
    b.set_sorted_spent_rice_best_bits(u128_to_u64(src.sorted_spent_rice_best_bits));
    b.set_sorted_spent_elias_delta_bits(u128_to_u64(src.sorted_spent_elias_delta_bits));
    b.set_sorted_spent_ef_bits_with64_bit_base(u128_to_u64(
        src.sorted_spent_ef_bits_with_64bit_base,
    ));
    b.set_sorted_p2tr_spent_values(src.sorted_p2tr_spent_values);
    b.set_sorted_p2tr_spent_leb128_bytes(u128_to_u64(src.sorted_p2tr_spent_leb128_bytes));
    b.set_sorted_p2tr_spent_rice_best_k(src.sorted_p2tr_spent_rice_best_k);
    b.set_sorted_p2tr_spent_rice_best_bits(u128_to_u64(src.sorted_p2tr_spent_rice_best_bits));
    b.set_sorted_p2tr_spent_elias_delta_bits(u128_to_u64(src.sorted_p2tr_spent_elias_delta_bits));
    b.set_sorted_p2tr_spent_ef_bits_with64_bit_base(u128_to_u64(
        src.sorted_p2tr_spent_ef_bits_with_64bit_base,
    ));
}

fn fill_log2_hist(mut b: log2_hist::Builder<'_>, src: &Log2Hist) {
    b.set_zeros(src.zeros);
    {
        let mut buckets = b.reborrow().init_buckets(64);
        for (i, &v) in src.buckets.iter().enumerate() {
            buckets.set(i as u32, v);
        }
    }
    // `sum` is u128 in the Rust accumulator; clamp into u64 for the wire
    // (u64::MAX sats × bucket count is unreachable in practice).
    b.set_sum(src.sum.min(u64::MAX as u128) as u64);
    b.set_max(src.max);
}
fn u128_to_u64(value: u128) -> u64 {
    value.min(u64::MAX as u128) as u64
}
