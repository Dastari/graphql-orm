//! Backup and restore orchestration primitives for `graphql-orm` applications.
//!
//! This crate coordinates database export/import adapters, stored object indexes,
//! backup repositories, snapshot manifests, verification, and restore planning.
//!
//! `graphql-orm-backup` does not own application authorization, scheduling,
//! cloud credentials, or primary object metadata. Applications provide small
//! adapter implementations and this crate handles repository layout, checksums,
//! compressed payloads, manifest chains, restore orchestration, compaction,
//! locking, and pruning.
//! [`BlobStoreBackupRepository`] adapts `graphql-orm-storage`
//! [`graphql_orm_storage::BlobStore`] implementations so local and cloud
//! provider code can be shared without routing backup keys through primary
//! object metadata.
//!
//! # Full Backup
//!
//! ```no_run
//! use graphql_orm_backup::{
//!     BackupObjectIndex, FullBackupRequest, GraphqlOrmBackupAdapter,
//!     BackupRepository, create_full_backup,
//! };
//! use uuid::Uuid;
//!
//! # async fn example(
//! #     repository: &dyn BackupRepository,
//! #     database: &dyn GraphqlOrmBackupAdapter,
//! #     objects: &dyn BackupObjectIndex,
//! # ) -> Result<(), graphql_orm_backup::BackupError> {
//! let result = create_full_backup(
//!     repository,
//!     database,
//!     objects,
//!     FullBackupRequest {
//!         snapshot_id: Uuid::new_v4(),
//!         created_at: 1_775_174_400,
//!         app_id: "example-app".to_string(),
//!         app_version: "0.1.0".to_string(),
//!     },
//! )
//! .await?;
//!
//! println!("created snapshot {}", result.manifest.snapshot_id);
//! # Ok(())
//! # }
//! ```
//!
//! # Restore
//!
//! ```no_run
//! use graphql_orm_backup::{
//!     BackupRepository, GraphqlOrmBackupAdapter, RestoreContext, restore_snapshot,
//! };
//! use uuid::Uuid;
//!
//! # async fn example(
//! #     repository: &dyn BackupRepository,
//! #     database: &dyn GraphqlOrmBackupAdapter,
//! #     snapshot_id: Uuid,
//! # ) -> Result<(), graphql_orm_backup::BackupError> {
//! restore_snapshot(
//!     repository,
//!     database,
//!     snapshot_id,
//!     RestoreContext::empty_database(),
//! )
//! .await?;
//! # Ok(())
//! # }
//! ```

mod backup;
mod database;
mod error;
#[cfg(feature = "local")]
mod local_repository;
mod lock;
mod manifest;
mod object_index;
#[cfg(feature = "orm")]
mod orm;
mod planner;
mod prune;
mod repository;
mod restore;
mod verify;

pub use backup::{
    BackupExecutionOptions, CompactChainRequest, CompactChainResult, DATABASE_EXPORT_FORMAT,
    DEFAULT_OBJECT_CONCURRENCY, FullBackupRequest, FullBackupResult, IncrementalBackupRequest,
    IncrementalBackupResult, bytes_sha256_hex, compact_chain, compact_chain_with_options,
    create_full_backup, create_full_backup_with_options, create_incremental_backup,
    create_incremental_backup_with_options, database_changes_key, database_table_key,
    object_content_key, snapshot_manifest_key, write_manifest,
};
pub use database::{
    BackupChangeAction, BackupChangeExport, BackupRow, BackupTableExport, GraphqlOrmBackupAdapter,
    GraphqlOrmBackupSchema,
};
pub use error::BackupError;
#[cfg(feature = "local")]
pub use local_repository::LocalBackupRepository;
pub use lock::{DEFAULT_LOCK_STALE_AFTER_SECONDS, RepositoryLock, RepositoryLockOptions};
pub use manifest::{
    BACKUP_FORMAT_VERSION, BackupCompression, BackupKind, BackupSnapshotManifest, BackupTombstone,
    DatabaseBackupManifest, ObjectBackupEntry, TableBackupEntry, compress_payload,
    decompress_payload, load_manifest, load_manifest_chain, manifest_checksum,
    set_manifest_checksum, validate_manifest_chain, verify_manifest_checksum,
};
pub use object_index::{BackupObjectIndex, BackupObjectRef};
#[cfg(feature = "orm")]
pub use orm::{OrmBackupAdapter, OrmBackupObjectIndex, OrmObjectIndexColumns};
pub use planner::{FullBackupPlan, plan_full_backup};
pub use prune::{DeleteSnapshotResult, KeepPolicy, PruneResult, delete_snapshot, prune};
pub use repository::{BackupRepository, BlobStoreBackupRepository};
pub use restore::{
    BlobStoreRestoreObjectSink, RestoreContext, RestoreMode, RestoreObjectSink, RestoreResult,
    ensure_empty_restore_target, restore_objects, restore_snapshot,
};
pub use verify::{
    VerificationOptions, verify_manifest_and_objects, verify_manifest_and_objects_with_options,
    verify_object_checksums, verify_object_checksums_with_options,
};
