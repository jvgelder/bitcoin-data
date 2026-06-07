//! Core types and traits for the bitcoin-data workspace.
//!
//! - [`source::BlockSource`] — trait for any height → raw block bytes resolver.
//! - [`block`] — `RawBlockFrame` (height + hash + raw `Bytes`) at the source-pipeline
//!   boundary, plus tip/hash event frames.
//! - [`parse`] — `RawBlockFrame` → `bitcoin::Block` using rust-bitcoin's
//!   consensus decoder. The decoded `bitcoin::Block` is the canonical
//!   in-process representation for the rest of the workspace.
//! - [`pipeline`] — fetch + ordered-delivery streams for raw and decoded blocks.
//!
//! Wire-format messages (capnp / proto / avro / json) live in
//! `crates/encoding/*` and are derived from `bitcoin::Block` at the
//! sink boundary.

pub mod block;
pub mod parse;
pub mod pipeline;
pub mod source;
