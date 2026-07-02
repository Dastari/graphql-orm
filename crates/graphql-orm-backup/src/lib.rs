//! Backup and restore orchestration primitives for graphql-orm applications.
//!
//! This crate coordinates database export/import adapters, stored object indexes,
//! backup repositories, snapshot manifests, verification, and restore planning.

mod backup;
mod database;
mod error;
#[cfg(feature = "local")]
mod local_repository;
mod manifest;
mod object_index;
mod planner;
mod repository;
mod restore;
mod verify;

pub use backup::{
    CompactChainRequest, CompactChainResult, DATABASE_EXPORT_FORMAT, FullBackupRequest,
    FullBackupResult, IncrementalBackupRequest, IncrementalBackupResult, bytes_sha256_hex,
    compact_chain, create_full_backup, create_incremental_backup, database_changes_key,
    database_table_key, object_content_key, snapshot_manifest_key, write_manifest,
};
pub use database::{
    BackupChangeAction, BackupChangeExport, BackupRow, BackupTableExport, GraphqlOrmBackupAdapter,
    GraphqlOrmBackupSchema,
};
pub use error::BackupError;
#[cfg(feature = "local")]
pub use local_repository::LocalBackupRepository;
pub use manifest::{
    BACKUP_FORMAT_VERSION, BackupCompression, BackupKind, BackupSnapshotManifest, BackupTombstone,
    DatabaseBackupManifest, ObjectBackupEntry, TableBackupEntry, compress_payload,
    decompress_payload, load_manifest, load_manifest_chain, manifest_checksum,
    set_manifest_checksum, validate_manifest_chain, verify_manifest_checksum,
};
pub use object_index::{BackupObjectIndex, BackupObjectRef};
pub use planner::{FullBackupPlan, plan_full_backup};
pub use repository::BackupRepository;
pub use restore::{
    RestoreContext, RestoreMode, RestoreObjectSink, RestoreResult, ensure_empty_restore_target,
    restore_objects, restore_snapshot,
};
pub use verify::{verify_manifest_and_objects, verify_object_checksums};
