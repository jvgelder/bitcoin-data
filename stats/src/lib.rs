//! Bitcoin block / transaction statistics accumulators.
//!
//! Reads `bitcoin::Block` (the canonical in-process form) and produces
//! Rust-side stats structs. Wire-format conversions (capnp/proto/avro/json)
//! happen in the encoding crates at the sink boundary — this crate
//! is wire-format-agnostic.
//!
//! Modules:
//! - [`block`] — block-level [`Stats`], [`BlockStats`], [`PerBlock`].
//! - [`transaction`] — per-input classification (P2TR paths, NUMS).
//! - [`script`] — script-type counters (planned).
//! - [`mempool`] — mempool-side stats (planned).
//! - [`histogram`] — log2 histogram for distribution analysis.
//! - [`utxo_hash`] — rolling commutative hash for oracle-agreement checks.

pub mod block;
pub mod checkpoint;
pub mod histogram;
pub mod scan;
pub mod script;
pub mod sinks;
pub mod sync;
pub mod transaction;
pub mod utxo_hash;

pub const STATS_STATE_VERSION: u32 = 6;

pub use block::{
    elias_delta_bits, elias_fano_bits, leb128_len_u64, measure_sorted_uid_stream,
    rice_bits, BlockLocalStats, BlockStats, ChainDerivedStats, CodecEstimate, PerBlock,
    RiceChoice, SortedUidBlockEstimate, SortedUidCodecStats, SpendContext, Stats,
};
pub use histogram::Log2Hist;
pub use checkpoint::{CheckpointConfig, StatsCheckpoint};
pub use scan::{scan, ScanConfig, StatsScannerState};
pub use script::{classify_script, ScriptType, TypeCounts};
pub use transaction::{
    classify_p2tr_spend, is_inscription_envelope, SpendClass, SpendPath, NUMS_H_XONLY,
};
pub use utxo_hash::RollingUtxoHash;