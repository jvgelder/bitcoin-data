//! Storage backend boundary for the light-data HTTP server.
//!
//! The server depends only on [`ArchiveBackend`]. Concrete implementations live
//! behind this module:
//! - [`SqliteArchive`] serves cached payload bytes from SQLite via SQLx.
//! - [`FileArchive`] serves fixture/static archives from files per block.

pub mod backend;
pub mod files;
pub mod sqlite;

pub use backend::{ArchiveBackend, CutthroughDeltaBlocks, CutthroughSnapshot, ServedProfile};
pub use files::{
    ChainTip, FileArchive, LightArchive, Manifest, ManifestCutthroughSnapshot, ManifestProfile,
};
pub use sqlite::SqliteArchive;
