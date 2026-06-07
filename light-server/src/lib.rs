#![allow(clippy::needless_lifetimes)]

pub mod light_capnp {
    include!(concat!(env!("OUT_DIR"), "/light_capnp.rs"));
}

pub mod codec;
pub mod index;
pub mod json_wire;
pub mod output_id;
pub mod p2tr_indexer;
pub mod profile;
pub mod range;
pub mod server;
pub mod storage;
pub mod types;

pub const WIRE_VERSION: u16 = 1;
pub const RANGE_MAGIC: &[u8; 4] = b"BDSR";
pub const RANGE_VERSION: u16 = 1;
pub const DEFAULT_MAX_RANGE_COUNT: u32 = 1_000;
