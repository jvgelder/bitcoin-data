//! Storage backend boundary for the light-data HTTP server.
//!
//! The server depends only on [`ArchiveBackend`]. The file-backed implementation
//! serves static archive blocks from disk.

pub mod backend;
pub mod files;

pub use backend::{ArchiveBackend, ServedBlock};
pub use files::{ChainTip, FileArchive, Manifest};
