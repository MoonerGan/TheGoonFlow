//! Domain model + storage schema for The Goon Flow.

pub mod db;
pub mod media;
pub mod model;

/// Storage schema version, tracked in `meta.schema_version`.
pub const SCHEMA_VERSION: i64 = 4;
