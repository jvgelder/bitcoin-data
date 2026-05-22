//! Minimal in-memory shapes for blocks at the source-pipeline boundary.
//!
//! These are intentionally tiny — just enough metadata to route a block
//! through the system. Decoded block contents are produced by the shared `core::parse` module.
//! Sources never decode blocks themselves; they only hand raw consensus bytes
//! to the core pipeline.

use bytes::Bytes;

/// A raw, consensus-encoded block plus the metadata needed to route it.
///
/// The block payload is stored as [`Bytes`] instead of `Vec<u8>` so sources
/// that already receive shared byte buffers, such as HTTP clients, can hand
/// them through the pipeline without copying. Sources that naturally produce
/// owned `Vec<u8>` can convert with `Bytes::from(vec)`.
#[derive(Clone, Debug)]
pub struct RawBlockFrame {
    pub height: u64,
    pub hash: [u8; 32],
    pub bytes: Bytes,
}

/// Tip / chain-tip notification (no block body).
#[derive(Clone, Copy, Debug)]
pub struct BlockTipFrame {
    pub height: u64,
    pub hash: [u8; 32],
}

/// Block-hash-only event (e.g. ZMQ `hashblock`).
#[derive(Clone, Copy, Debug)]
pub struct BlockHashFrame {
    pub hash: [u8; 32],
}