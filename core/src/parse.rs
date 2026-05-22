//! Raw block bytes → `bitcoin::Block`.
//!
//! Uses rust-bitcoin's consensus decoder. The decoded form is the canonical
//! in-process representation everything else in the workspace consumes:
//! stats.rs walks `bitcoin::Block`, the capnp / proto / avro / json adapters
//! convert from `bitcoin::Block` at the wire boundary.

use crate::block::RawBlockFrame;
use bitcoin::consensus::Decodable;

/// A parsed block carrying the source-side metadata (height, hash) plus the
/// rust-bitcoin decoded form. `bitcoin::Block` itself doesn't carry height,
/// so the wrapper preserves it.
#[derive(Debug)]
pub struct DecodedBlockFrame {
    pub height: u64,
    pub hash: [u8; 32],
    pub block: bitcoin::Block,
}

/// Backwards-compatible alias for existing encoders/sinks.
pub type ParsedBlock = DecodedBlockFrame;

pub fn decode_raw_block(frame: RawBlockFrame) -> anyhow::Result<DecodedBlockFrame> {
    let mut bytes = frame.bytes.as_ref();
    let block = bitcoin::Block::consensus_decode(&mut bytes)?;
    Ok(DecodedBlockFrame { height: frame.height, hash: frame.hash, block })
}

/// Backwards-compatible alias for callers that still use the old name.
pub fn parse(frame: RawBlockFrame) -> anyhow::Result<DecodedBlockFrame> {
    decode_raw_block(frame)
}