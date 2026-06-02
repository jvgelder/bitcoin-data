//! Storage backend boundary for the light-data HTTP server.
//!
//! The server depends only on [`ArchiveBackend`]. Concrete implementations live
//! behind this module:
//! - [`SqliteArchive`] serves cached payload/checkpoint bytes from SQLite via SQLx.
//! - [`FileArchive`] serves fixture/static archives from files per block.

pub mod backend;
pub mod files;
pub mod sqlite;

pub use backend::{ArchiveBackend, ServedProfile};
pub use files::{ChainTip, FileArchive, LightArchive, Manifest, ManifestProfile};
pub use sqlite::SqliteArchive;
