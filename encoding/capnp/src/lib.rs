//! Cap'n Proto wire-format adapter.
//!
//! Encodes the canonical in-process types into capnp messages for transport:
//! - `bitcoin::Block` (via `btc_data_core::parse::ParsedBlock`) → capnp Block.
//!
//! Two schemas, two file IDs, generated separately:
//! - `schema/bitcoin_block.capnp` → [`block_schema`]
//!
//! Used by sinks that ship capnp on the wire (ipc, optionally kafka/grpc),
//! and by the json/proto/avro adapters as the upstream schema source of
//! truth (Option-2 layering — see workspace README).

#![allow(clippy::needless_lifetimes)]

pub mod block_capnp {
    include!(concat!(env!("OUT_DIR"), "/block_capnp.rs"));
}

pub mod block;

pub use block::encode_block;

use capnp::message::{Builder, HeapAllocator};

/// Serialize a capnp builder to a packed byte buffer (no segment table
/// padding; smallest wire form). Use `unpacked` if peers expect it.
pub fn to_packed_bytes(msg: &Builder<HeapAllocator>) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    capnp::serialize_packed::write_message(&mut out, msg)?;
    Ok(out)
}

/// Serialize a capnp builder to the standard (unpacked) framing used by
/// most capnp-RPC and IPC peers.
pub fn to_bytes(msg: &Builder<HeapAllocator>) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    capnp::serialize::write_message(&mut out, msg)?;
    Ok(out)
}
