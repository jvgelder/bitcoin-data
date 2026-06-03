//! JSON adapter. Reads capnp Block/Stats messages, emits JSON.
//!
//! Per Option-2 layering: in-process consumers work with rust-bitcoin and
//! the stats.rs Rust structs. Format adapters (this crate, proto, avro) read
//! the capnp form so the schema is the single source of truth for what's
//! shippable on the wire.

pub mod block;
pub mod stats;
