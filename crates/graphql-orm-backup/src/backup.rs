use std::{
    collections::{BTreeMap, HashMap},
    io::Write,
};

use bytes::Bytes;
use futures::{StreamExt, stream};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use graphql_orm_storage::StorageByteStream;

use crate::{
    BACKUP_FORMAT_VERSION, BackupByteLimit, BackupCapacityLimitKind, BackupCapacityLimits,
    BackupChangeAction, BackupChangeExport, BackupError, BackupKind, BackupObjectIndex,
    BackupRepository, BackupRow, BackupSnapshotManifest, BackupTombstone, DatabaseBackupManifest,
    GraphqlOrmBackupAdapter, GraphqlOrmBackupSchema, ObjectBackupEntry, RepositoryLock,
    RepositoryLockOptions, TableBackupEntry, authenticate_manifest, load_manifest_chain,
    manifest::{required_authenticator, sha256_hex, validate_sha256_hex, validate_table_name},
    plan_full_backup,
};

pub const DATABASE_EXPORT_FORMAT: &str = "jsonl";
/// Default number of concurrent object/payload operations.
pub const DEFAULT_OBJECT_CONCURRENCY: usize = 8;

/// Runtime tuning options for backup, incremental, and compaction operations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupExecutionOptions {
    /// Maximum number of concurrent object writes/checks.
    pub object_concurrency: usize,
    /// Advisory repository lock settings.
    pub lock: RepositoryLockOptions,
    /// Cooperative cancellation signal, checked at safe points.
    pub cancellation: BackupCancellation,
}

impl Default for BackupExecutionOptions {
    fn default() -> Self {
        Self {
            object_concurrency: DEFAULT_OBJECT_CONCURRENCY,
            lock: RepositoryLockOptions::default(),
            cancellation: BackupCancellation::default(),
        }
    }
}

/// Cooperative cancellation for backup, incremental backup, compaction,
/// restore and verification.
///
/// Clones share one signal. After [`BackupCancellation::cancel`], a writer
/// stops at its next safe point: before a payload, object or manifest write,
/// never during one. Object uploads already in flight run to completion and
/// no new one starts. It then removes the snapshot payloads it wrote,
/// releases the repository lock and returns [`BackupError::Cancelled`]. Once
/// the manifest is written the snapshot is committed and cancellation no
/// longer applies. Restore stops before its next manifest, payload, apply call
/// or object, and verification before its next checksum read; both release
/// the lock. If releasing the lock fails, the operation instead returns
/// [`BackupError::LockNotReleased`] with the cancellation as its `operation`
/// and the run that still holds the lock.
///
/// Dropping the operation's future instead cannot release the lock, because
/// release is asynchronous. The lock then keeps its owner record; see
/// [`crate::break_repository_lock`].
#[derive(Clone, Debug, Default)]
pub struct BackupCancellation {
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl BackupCancellation {
    /// Creates a signal that is not cancelled.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation of every operation that uses this signal.
    pub fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Whether cancellation was requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// A safe point: fails with [`BackupError::Cancelled`] once cancelled.
    pub(crate) fn check(&self) -> Result<(), BackupError> {
        if self.is_cancelled() {
            Err(BackupError::Cancelled)
        } else {
            Ok(())
        }
    }
}

impl PartialEq for BackupCancellation {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.cancelled, &other.cancelled)
    }
}

impl Eq for BackupCancellation {}

/// Request metadata for a full backup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FullBackupRequest {
    /// Snapshot id to write.
    pub snapshot_id: Uuid,
    /// Snapshot creation time as UTC Unix seconds.
    pub created_at: i64,
    /// Stable application identifier.
    pub app_id: String,
    /// Application version that produced the snapshot.
    pub app_version: String,
}

/// Result returned after writing a full backup.
#[derive(Clone, Debug, PartialEq)]
pub struct FullBackupResult {
    /// Manifest written for the full backup.
    pub manifest: BackupSnapshotManifest,
}

/// Request metadata for an incremental backup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncrementalBackupRequest {
    /// Snapshot id to write.
    pub snapshot_id: Uuid,
    /// Parent full, synthetic-full, or incremental snapshot id.
    pub parent_snapshot_id: Uuid,
    /// Snapshot creation time as UTC Unix seconds.
    pub created_at: i64,
    /// Stable application identifier.
    pub app_id: String,
    /// Application version that produced the snapshot.
    pub app_version: String,
}

/// Result returned after writing an incremental backup.
#[derive(Clone, Debug, PartialEq)]
pub struct IncrementalBackupResult {
    /// Manifest written for the incremental backup.
    pub manifest: BackupSnapshotManifest,
}

/// Request metadata for synthetic-full compaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactChainRequest {
    /// Synthetic-full snapshot id to write.
    pub snapshot_id: Uuid,
    /// Source snapshot id whose chain should be compacted.
    pub source_snapshot_id: Uuid,
    /// Snapshot creation time as UTC Unix seconds.
    pub created_at: i64,
    /// Stable application identifier.
    pub app_id: String,
    /// Application version that produced the synthetic full.
    pub app_version: String,
}

/// Result returned after synthetic-full compaction.
#[derive(Clone, Debug, PartialEq)]
pub struct CompactChainResult {
    /// Manifest written for the synthetic-full snapshot.
    pub manifest: BackupSnapshotManifest,
}

/// Returns the manifest key for a snapshot id.
#[must_use]
pub fn snapshot_manifest_key(snapshot_id: Uuid) -> String {
    format!("snapshots/{snapshot_id}/manifest.json")
}

/// Returns the reserved table export key.
///
/// Returns the zstd-compressed table export key used by the snapshot format.
#[must_use]
pub fn database_table_key(snapshot_id: Uuid, table_name: &str) -> String {
    format!("snapshots/{snapshot_id}/database/tables/{table_name}.jsonl.zst")
}

#[must_use]
pub fn database_changes_key(snapshot_id: Uuid, table_name: &str) -> String {
    format!("snapshots/{snapshot_id}/database/changes/{table_name}.jsonl.zst")
}

#[must_use]
pub fn object_content_key(sha256_hex: &str) -> String {
    let shard_a = sha256_hex.get(0..2).unwrap_or("00");
    let shard_b = sha256_hex.get(2..4).unwrap_or("00");
    format!("objects/sha256/{shard_a}/{shard_b}/{sha256_hex}")
}

/// Creates a full snapshot in the repository.
///
/// Under the repository lock it refuses a snapshot id that already has a
/// manifest or payloads, and checks every payload against the repository
/// handle's [`crate::BackupCapacityLimits`] before writing it, so the snapshot
/// can be restored with the same limits. A run that fails or is cancelled
/// before its manifest write removes the snapshot payloads it wrote; object
/// blobs it added stay content-addressed and unreferenced until [`crate::prune`].
///
/// # Errors
///
/// Returns [`BackupError::SnapshotExists`] for a used snapshot id,
/// [`BackupError::BackupCapacityExceeded`] for a payload over a limit,
/// [`BackupError::Cancelled`], [`BackupError::LockReleaseFailed`] if the
/// snapshot was committed but the lock was not released, or another
/// [`BackupError`] if planning fails, table serialization or compression
/// fails, object loading/checksum validation fails, or any repository write
/// fails.
pub async fn create_full_backup(
    repository: &dyn BackupRepository,
    database: &dyn GraphqlOrmBackupAdapter,
    objects: &dyn BackupObjectIndex,
    request: FullBackupRequest,
) -> Result<FullBackupResult, BackupError> {
    create_full_backup_with_options(
        repository,
        database,
        objects,
        request,
        &BackupExecutionOptions::default(),
    )
    .await
}

/// Creates a full snapshot with explicit execution options.
///
/// # Errors
///
/// Returns [`BackupError`] if planning fails, table serialization or
/// compression fails, object loading/checksum validation fails, locking fails,
/// or any repository write fails.
pub async fn create_full_backup_with_options(
    repository: &dyn BackupRepository,
    database: &dyn GraphqlOrmBackupAdapter,
    objects: &dyn BackupObjectIndex,
    request: FullBackupRequest,
    options: &BackupExecutionOptions,
) -> Result<FullBackupResult, BackupError> {
    // Refuse before writing payloads that no sealed manifest would reference.
    required_authenticator(repository)?;
    let lock = RepositoryLock::acquire_for(repository, &options.lock, "full_backup").await?;
    let snapshot_id = request.snapshot_id;
    let mut write = SnapshotWrite::default();
    let result =
        create_full_backup_inner(repository, database, objects, request, options, &mut write).await;
    let result = remove_failed_snapshot(repository, snapshot_id, &write, result).await;
    lock.finish(result, |result| Some(result.manifest.snapshot_id))
        .await
}

async fn create_full_backup_inner(
    repository: &dyn BackupRepository,
    database: &dyn GraphqlOrmBackupAdapter,
    objects: &dyn BackupObjectIndex,
    request: FullBackupRequest,
    options: &BackupExecutionOptions,
    write: &mut SnapshotWrite,
) -> Result<FullBackupResult, BackupError> {
    claim_snapshot_id(repository, request.snapshot_id, write).await?;
    options.cancellation.check()?;
    let limits = repository.capacity_limits();
    let plan = plan_full_backup(database, objects).await?;
    for table in &plan.tables {
        ensure_table_name(request.snapshot_id, &table.table_name)?;
    }
    validate_object_refs(request.snapshot_id, &plan.objects)?;
    check_declared_object_sizes(limits, &plan.objects)?;
    let mut stage = Vec::with_capacity(plan.tables.len());
    for table in &plan.tables {
        stage.push((
            database_table_key(request.snapshot_id, &table.table_name),
            decoded_jsonl_len(&table.rows)?,
        ));
    }
    let decode_limits = check_database_stage(limits, request.snapshot_id, &stage)?;

    let mut table_entries = Vec::with_capacity(plan.tables.len());
    let mut row_count = 0_u64;
    for (table, decode_limit) in plan.tables.iter().zip(decode_limits) {
        options.cancellation.check()?;
        let content_key = database_table_key(request.snapshot_id, &table.table_name);
        let bytes = serialize_jsonl_entries(
            &table.rows,
            &content_key,
            limits.buffered_bytes,
            decode_limit,
        )?;
        let sha256_hex = sha256_hex(&bytes);
        repository
            .put_blob(&content_key, Bytes::from(bytes))
            .await?;

        let table_row_count = table.rows.len() as u64;
        row_count += table_row_count;
        table_entries.push(TableBackupEntry {
            table_name: table.table_name.clone(),
            row_count: table_row_count,
            content_key,
            sha256_hex,
        });
    }

    let object_entries = write_object_entries(repository, objects, &plan.objects, options).await?;
    options.cancellation.check()?;

    let mut manifest = BackupSnapshotManifest {
        format_version: BACKUP_FORMAT_VERSION,
        snapshot_id: request.snapshot_id,
        parent_snapshot_id: None,
        created_at: request.created_at,
        app_id: request.app_id,
        app_version: request.app_version,
        graphql_orm_schema_version: plan.schema.migration_version,
        graphql_orm_schema_hash: plan.schema.schema_hash,
        database_backend: plan.schema.backend,
        backup_kind: BackupKind::Full,
        database: DatabaseBackupManifest {
            export_format: DATABASE_EXPORT_FORMAT.to_string(),
            compression: crate::BackupCompression::Zstd,
            row_count,
            table_count: table_entries.len() as u64,
            tables: table_entries,
            changes: Vec::new(),
        },
        objects: object_entries,
        tombstones: Vec::new(),
        checksum: String::new(),
        authentication: None,
    };

    write_manifest_tracked(repository, &mut manifest, write).await?;

    Ok(FullBackupResult { manifest })
}

/// Creates an incremental snapshot in the repository.
///
/// Under the repository lock it refuses a snapshot id already in use and
/// loads and authenticates the parent's whole chain, refusing a parent that is
/// missing, invalid or recorded for another application, backend or schema,
/// all before exporting or writing anything. Payloads are checked against the
/// same limits as [`create_full_backup`], and a failed or cancelled run removes
/// the payloads it wrote.
///
/// # Errors
///
/// Returns [`BackupError`] if schema lookup, incremental export, object
/// discovery/loading, payload serialization/compression, checksum validation,
/// or repository writes fail.
pub async fn create_incremental_backup(
    repository: &dyn BackupRepository,
    database: &dyn GraphqlOrmBackupAdapter,
    objects: &dyn BackupObjectIndex,
    request: IncrementalBackupRequest,
) -> Result<IncrementalBackupResult, BackupError> {
    create_incremental_backup_with_options(
        repository,
        database,
        objects,
        request,
        &BackupExecutionOptions::default(),
    )
    .await
}

/// Creates an incremental snapshot with explicit execution options.
///
/// # Errors
///
/// Returns [`BackupError`] if schema lookup, incremental export, object
/// discovery/loading, payload serialization/compression, checksum validation,
/// locking, or repository writes fail.
pub async fn create_incremental_backup_with_options(
    repository: &dyn BackupRepository,
    database: &dyn GraphqlOrmBackupAdapter,
    objects: &dyn BackupObjectIndex,
    request: IncrementalBackupRequest,
    options: &BackupExecutionOptions,
) -> Result<IncrementalBackupResult, BackupError> {
    required_authenticator(repository)?;
    let lock = RepositoryLock::acquire_for(repository, &options.lock, "incremental_backup").await?;
    let snapshot_id = request.snapshot_id;
    let mut write = SnapshotWrite::default();
    let result = create_incremental_backup_inner(
        repository, database, objects, request, options, &mut write,
    )
    .await;
    let result = remove_failed_snapshot(repository, snapshot_id, &write, result).await;
    lock.finish(result, |result| Some(result.manifest.snapshot_id))
        .await
}

async fn create_incremental_backup_inner(
    repository: &dyn BackupRepository,
    database: &dyn GraphqlOrmBackupAdapter,
    objects: &dyn BackupObjectIndex,
    request: IncrementalBackupRequest,
    options: &BackupExecutionOptions,
    write: &mut SnapshotWrite,
) -> Result<IncrementalBackupResult, BackupError> {
    claim_snapshot_id(repository, request.snapshot_id, write).await?;
    if request.parent_snapshot_id == request.snapshot_id {
        return Err(BackupError::InvalidManifestEntry {
            snapshot_id: request.snapshot_id,
            reason: "snapshot is its own parent".to_string(),
        });
    }
    options.cancellation.check()?;
    let limits = repository.capacity_limits();
    // Restore rejects a chain whose members disagree, so refuse to extend a
    // parent the new snapshot could not be restored on top of.
    let parent_chain = load_manifest_chain(repository, request.parent_snapshot_id).await?;
    let schema = database.schema_snapshot().await?;
    ensure_extends_parent(&parent_chain, &request, &schema)?;
    let changes = database
        .export_incremental(request.parent_snapshot_id)
        .await?;
    let object_refs = objects
        .list_objects_for_incremental_backup(request.parent_snapshot_id)
        .await?;

    for change in &changes {
        ensure_table_name(request.snapshot_id, &change.table_name)?;
    }
    validate_object_refs(request.snapshot_id, &object_refs)?;
    check_declared_object_sizes(limits, &object_refs)?;

    let mut change_entries = Vec::new();
    let mut change_count = 0_u64;
    let tombstones = changes_to_tombstones(&changes);
    let groups = group_changes_by_table(changes);
    let mut stage = Vec::with_capacity(groups.len());
    for group in &groups {
        stage.push((
            database_changes_key(request.snapshot_id, &group.table_name),
            decoded_jsonl_len(&group.changes)?,
        ));
    }
    let decode_limits = check_database_stage(limits, request.snapshot_id, &stage)?;
    for (group, decode_limit) in groups.into_iter().zip(decode_limits) {
        options.cancellation.check()?;
        let content_key = database_changes_key(request.snapshot_id, &group.table_name);
        let bytes = serialize_jsonl_entries(
            &group.changes,
            &content_key,
            limits.buffered_bytes,
            decode_limit,
        )?;
        let sha256_hex = sha256_hex(&bytes);
        repository
            .put_blob(&content_key, Bytes::from(bytes))
            .await?;

        let table_change_count = group.changes.len() as u64;
        change_count += table_change_count;
        change_entries.push(TableBackupEntry {
            table_name: group.table_name,
            row_count: table_change_count,
            content_key,
            sha256_hex,
        });
    }

    let object_entries = write_object_entries(repository, objects, &object_refs, options).await?;
    options.cancellation.check()?;

    let mut manifest = BackupSnapshotManifest {
        format_version: BACKUP_FORMAT_VERSION,
        snapshot_id: request.snapshot_id,
        parent_snapshot_id: Some(request.parent_snapshot_id),
        created_at: request.created_at,
        app_id: request.app_id,
        app_version: request.app_version,
        graphql_orm_schema_version: schema.migration_version,
        graphql_orm_schema_hash: schema.schema_hash,
        database_backend: schema.backend,
        backup_kind: BackupKind::Incremental,
        database: DatabaseBackupManifest {
            export_format: DATABASE_EXPORT_FORMAT.to_string(),
            compression: crate::BackupCompression::Zstd,
            row_count: change_count,
            table_count: 0,
            tables: Vec::new(),
            changes: change_entries,
        },
        objects: object_entries,
        tombstones,
        checksum: String::new(),
        authentication: None,
    };

    write_manifest_tracked(repository, &mut manifest, write).await?;

    Ok(IncrementalBackupResult { manifest })
}

/// Compacts a full-plus-incremental chain into a synthetic full snapshot.
///
/// Like [`create_full_backup`], it refuses a snapshot id already in use, checks
/// the merged synthetic full against the repository's capacity limits before
/// writing it, and removes its payloads if it fails or is cancelled before the
/// manifest write.
///
/// # Errors
///
/// Returns [`BackupError`] if the source chain cannot be loaded or verified,
/// payloads cannot be parsed, or synthetic table/manifest blobs cannot be
/// written.
pub async fn compact_chain(
    repository: &dyn BackupRepository,
    request: CompactChainRequest,
) -> Result<CompactChainResult, BackupError> {
    compact_chain_with_options(repository, request, &BackupExecutionOptions::default()).await
}

/// Compacts a chain with explicit execution options.
///
/// # Errors
///
/// Returns [`BackupError`] if the source chain cannot be loaded or verified,
/// locking fails, payloads cannot be parsed, or synthetic table/manifest blobs
/// cannot be written.
pub async fn compact_chain_with_options(
    repository: &dyn BackupRepository,
    request: CompactChainRequest,
    options: &BackupExecutionOptions,
) -> Result<CompactChainResult, BackupError> {
    required_authenticator(repository)?;
    let lock = RepositoryLock::acquire_for(repository, &options.lock, "compaction").await?;
    let snapshot_id = request.snapshot_id;
    let mut write = SnapshotWrite::default();
    let result = compact_chain_inner(repository, request, options, &mut write).await;
    let result = remove_failed_snapshot(repository, snapshot_id, &write, result).await;
    lock.finish(result, |result| Some(result.manifest.snapshot_id))
        .await
}

async fn compact_chain_inner(
    repository: &dyn BackupRepository,
    request: CompactChainRequest,
    options: &BackupExecutionOptions,
    write: &mut SnapshotWrite,
) -> Result<CompactChainResult, BackupError> {
    // Checked under the repository lock, before any payload is written.
    claim_snapshot_id(repository, request.snapshot_id, write).await?;
    options.cancellation.check()?;
    let limits = repository.capacity_limits();
    let chain = load_manifest_chain(repository, request.source_snapshot_id).await?;
    for manifest in &chain {
        crate::verify::verify_manifest_and_objects_unlocked(
            repository,
            manifest,
            &crate::VerificationOptions::default(),
        )
        .await?;
    }

    let mut table_rows = BTreeMap::<String, BTreeMap<String, BackupRow>>::new();
    let full_manifest = chain
        .first()
        .ok_or_else(|| BackupError::InvalidManifestChain {
            reason: "manifest chain is empty".to_string(),
        })?;
    for table in crate::restore::load_table_exports(repository, full_manifest).await? {
        let rows = table_rows.entry(table.table_name).or_default();
        for row in table.rows {
            rows.insert(row.primary_key.clone(), row);
        }
    }

    // A composite-key table's rows are identified by a canonical JSON array of key parts. The
    // change journal cannot name such rows, so a chain that changes one would resurrect or
    // duplicate rows. Refuse it instead. Manifests carry no key descriptors, so a table counts as
    // composite when every base row's identity is such an array; a single-key table whose every
    // key happens to look like one is refused too, which fails closed. The journal already refuses
    // composite-key events, so this guards chains written by other producers.
    let composite = table_rows
        .iter()
        .filter(|(_, rows)| {
            !rows.is_empty()
                && rows.keys().all(|key| {
                    serde_json::from_str::<Vec<String>>(key).is_ok_and(|parts| parts.len() >= 2)
                })
        })
        .map(|(table, _)| table.clone())
        .collect::<std::collections::BTreeSet<_>>();
    for manifest in chain.iter().skip(1) {
        for change in crate::restore::load_change_exports(repository, manifest).await? {
            if composite.contains(&change.table_name) {
                return Err(BackupError::InvalidManifestChain {
                    reason: format!(
                        "incremental changes to composite-key table {} cannot be compacted",
                        change.table_name
                    ),
                });
            }
            let rows = table_rows.entry(change.table_name.clone()).or_default();
            match change.action {
                BackupChangeAction::Create | BackupChangeAction::Update => {
                    if let Some(row) = change.row {
                        rows.insert(change.primary_key, row);
                    }
                }
                BackupChangeAction::Delete => {
                    rows.remove(&change.primary_key);
                }
            }
        }
    }

    let table_rows = table_rows
        .into_iter()
        .map(|(table_name, rows)| (table_name, rows.into_values().collect::<Vec<_>>()))
        .collect::<Vec<_>>();
    let mut stage = Vec::with_capacity(table_rows.len());
    for (table_name, rows) in &table_rows {
        stage.push((
            database_table_key(request.snapshot_id, table_name),
            decoded_jsonl_len(rows)?,
        ));
    }
    let decode_limits = check_database_stage(limits, request.snapshot_id, &stage)?;

    let mut table_entries = Vec::with_capacity(table_rows.len());
    let mut row_count = 0_u64;
    for ((table_name, rows), decode_limit) in table_rows.into_iter().zip(decode_limits) {
        options.cancellation.check()?;
        let content_key = database_table_key(request.snapshot_id, &table_name);
        let bytes =
            serialize_jsonl_entries(&rows, &content_key, limits.buffered_bytes, decode_limit)?;
        let sha256_hex = sha256_hex(&bytes);
        repository
            .put_blob(&content_key, Bytes::from(bytes))
            .await?;

        let table_row_count = rows.len() as u64;
        row_count += table_row_count;
        table_entries.push(TableBackupEntry {
            table_name,
            row_count: table_row_count,
            content_key,
            sha256_hex,
        });
    }

    let mut object_entries = compact_object_entries(&chain);
    for object in &mut object_entries {
        object.size_bytes = crate::verify::verify_blob_checksum(
            repository,
            object.content_key.clone(),
            object.sha256_hex.clone(),
            limits.object_bytes,
        )
        .await?;
    }
    options.cancellation.check()?;
    let latest = chain
        .last()
        .ok_or_else(|| BackupError::InvalidManifestChain {
            reason: "manifest chain is empty".to_string(),
        })?;
    let mut manifest = BackupSnapshotManifest {
        format_version: BACKUP_FORMAT_VERSION,
        snapshot_id: request.snapshot_id,
        parent_snapshot_id: None,
        created_at: request.created_at,
        app_id: request.app_id,
        app_version: request.app_version,
        graphql_orm_schema_version: latest.graphql_orm_schema_version.clone(),
        graphql_orm_schema_hash: latest.graphql_orm_schema_hash.clone(),
        database_backend: latest.database_backend.clone(),
        backup_kind: BackupKind::SyntheticFull,
        database: DatabaseBackupManifest {
            export_format: DATABASE_EXPORT_FORMAT.to_string(),
            compression: crate::BackupCompression::Zstd,
            row_count,
            table_count: table_entries.len() as u64,
            tables: table_entries,
            changes: Vec::new(),
        },
        objects: object_entries,
        tombstones: Vec::new(),
        checksum: String::new(),
        authentication: None,
    };

    write_manifest_tracked(repository, &mut manifest, write).await?;

    Ok(CompactChainResult { manifest })
}

/// Seals a manifest with the repository's host key and writes it as the final
/// snapshot blob.
///
/// This is a low-level building block: it takes no repository lock, so the
/// caller must coordinate with other operations. It never overwrites: the
/// manifest is created with an atomic conditional write and an existing
/// snapshot id fails with [`BackupError::SnapshotExists`]. A sealed manifest
/// is immutable; write a new snapshot instead of rewriting one.
///
/// # Errors
///
/// Returns [`BackupError::SnapshotExists`] if the snapshot already has a
/// manifest, [`BackupError::ManifestAuthenticationRequired`] if the repository
/// has no authenticator, [`BackupError::UnsupportedManifestFormat`] unless the
/// manifest uses [`crate::BACKUP_FORMAT_VERSION`],
/// [`BackupError::InvalidManifestEntry`] for an entry outside its namespace,
/// [`BackupError::BackupCapacityExceeded`] if the manifest is larger than
/// [`crate::BackupCapacityLimits::buffered_bytes`], which manifest loading
/// enforces, or another [`BackupError`] if the manifest cannot be serialized or
/// written.
pub async fn write_manifest(
    repository: &dyn BackupRepository,
    manifest: &mut BackupSnapshotManifest,
) -> Result<(), BackupError> {
    write_manifest_tracked(repository, manifest, &mut SnapshotWrite::default()).await
}

async fn write_manifest_tracked(
    repository: &dyn BackupRepository,
    manifest: &mut BackupSnapshotManifest,
    write: &mut SnapshotWrite,
) -> Result<(), BackupError> {
    authenticate_manifest(required_authenticator(repository)?, manifest)?;
    let body = serde_json::to_vec_pretty(manifest)?;
    let key = snapshot_manifest_key(manifest.snapshot_id);
    check_backup_limit(
        repository.capacity_limits().buffered_bytes,
        u64::try_from(body.len()).unwrap_or(u64::MAX),
        &key,
        BackupCapacityLimitKind::StoredPayload,
    )?;
    // From here the manifest may reach storage even if the write reports an
    // error, so a failed run must not remove the payloads it would reference.
    write.manifest_attempted = true;
    if repository
        .put_blob_if_absent(&key, Bytes::from(body))
        .await?
    {
        Ok(())
    } else {
        Err(BackupError::SnapshotExists {
            snapshot_id: manifest.snapshot_id,
        })
    }
}

/// What a snapshot writer has done, so a failed run removes only its own
/// payloads.
#[derive(Default)]
struct SnapshotWrite {
    /// The id was verified unused under the lock: payloads below it are ours.
    claimed: bool,
    /// The manifest write was attempted, so the outcome may be committed.
    manifest_attempted: bool,
}

fn snapshot_prefix(snapshot_id: Uuid) -> String {
    format!("snapshots/{snapshot_id}/")
}

/// Every stored blob below a snapshot's directory, including its manifest.
pub(crate) async fn snapshot_blob_keys(
    repository: &dyn BackupRepository,
    snapshot_id: Uuid,
) -> Result<Vec<String>, BackupError> {
    let prefix = snapshot_prefix(snapshot_id);
    let mut keys = repository
        .list_blobs(&format!("snapshots/{snapshot_id}"))
        .await?;
    keys.retain(|key| key.starts_with(&prefix));
    Ok(keys)
}

/// Refuses a snapshot id that has a manifest or any payload, before any write.
async fn claim_snapshot_id(
    repository: &dyn BackupRepository,
    snapshot_id: Uuid,
    write: &mut SnapshotWrite,
) -> Result<(), BackupError> {
    if repository
        .blob_exists(&snapshot_manifest_key(snapshot_id))
        .await?
        || !snapshot_blob_keys(repository, snapshot_id)
            .await?
            .is_empty()
    {
        return Err(BackupError::SnapshotExists { snapshot_id });
    }
    write.claimed = true;
    Ok(())
}

/// Removes the payloads a failed run wrote below its claimed snapshot id.
///
/// Nothing is removed once the manifest write was attempted: its outcome is
/// unknown, and if it landed the payloads belong to a committed snapshot.
/// Removal is best effort; [`crate::prune`] removes whatever remains.
async fn remove_failed_snapshot<T>(
    repository: &dyn BackupRepository,
    snapshot_id: Uuid,
    write: &SnapshotWrite,
    result: Result<T, BackupError>,
) -> Result<T, BackupError> {
    if result.is_err()
        && write.claimed
        && !write.manifest_attempted
        && let Ok(keys) = snapshot_blob_keys(repository, snapshot_id).await
    {
        for key in keys {
            let _ = repository.delete_blob(&key).await;
        }
    }
    result
}

/// Refuses an incremental whose parent chain restore would not accept it on.
fn ensure_extends_parent(
    parent_chain: &[BackupSnapshotManifest],
    request: &IncrementalBackupRequest,
    schema: &GraphqlOrmBackupSchema,
) -> Result<(), BackupError> {
    let parent = parent_chain
        .last()
        .ok_or_else(|| BackupError::InvalidManifestChain {
            reason: "manifest chain is empty".to_string(),
        })?;
    let mismatch = if parent.app_id != request.app_id {
        Some("app_id")
    } else if parent.database_backend != schema.backend {
        Some("database backend")
    } else if parent.graphql_orm_schema_hash != schema.schema_hash {
        Some("schema hash")
    } else {
        None
    };
    match mismatch {
        Some(field) => Err(BackupError::InvalidManifestChain {
            reason: format!(
                "incremental snapshot {} {field} does not match parent {}",
                request.snapshot_id, parent.snapshot_id
            ),
        }),
        None => Ok(()),
    }
}

fn check_backup_limit(
    limit: BackupByteLimit,
    observed_bytes: u64,
    payload: &str,
    kind: BackupCapacityLimitKind,
) -> Result<(), BackupError> {
    match limit {
        BackupByteLimit::Bytes(limit_bytes) if observed_bytes > limit_bytes => {
            Err(BackupError::BackupCapacityExceeded {
                payload: payload.to_string(),
                limit: kind,
                limit_bytes,
                observed_bytes,
            })
        }
        _ => Ok(()),
    }
}

/// Applies restore's decoded per-payload and per-stage limits to a stage, in
/// restore order, before any of it is written.
///
/// Returns the decode limit restore applies to each payload: the per-payload
/// limit, or the stage budget remaining when it reaches that payload if lower.
fn check_database_stage(
    limits: BackupCapacityLimits,
    snapshot_id: Uuid,
    payloads: &[(String, u64)],
) -> Result<Vec<BackupByteLimit>, BackupError> {
    let mut total = 0_u64;
    let mut decode_limits = Vec::with_capacity(payloads.len());
    for (content_key, decoded) in payloads {
        check_backup_limit(
            limits.decompressed_bytes,
            *decoded,
            content_key,
            BackupCapacityLimitKind::DecodedPayload,
        )?;
        let remaining = match limits.stage_bytes {
            BackupByteLimit::Bytes(stage) => BackupByteLimit::Bytes(stage.saturating_sub(total)),
            BackupByteLimit::Unbounded => BackupByteLimit::Unbounded,
        };
        decode_limits.push(limits.decompressed_bytes.min(remaining));
        total = total.saturating_add(*decoded);
    }
    check_backup_limit(
        limits.stage_bytes,
        total,
        &format!("snapshots/{snapshot_id}/database"),
        BackupCapacityLimitKind::Stage,
    )?;
    Ok(decode_limits)
}

/// zstd window log of the default compression level for large inputs.
const DEFAULT_ZSTD_WINDOW_LOG: u32 = 21;

/// Window log no larger than [`crate::decompress_payload_with_limit`] accepts
/// for `decode_limit`, or `None` to keep the level default.
fn compatible_window_log(decode_limit: BackupByteLimit) -> Option<u32> {
    match decode_limit {
        BackupByteLimit::Bytes(limit) => {
            let accepted = crate::manifest::decode_window_log(limit);
            (accepted < DEFAULT_ZSTD_WINDOW_LOG).then_some(accepted)
        }
        BackupByteLimit::Unbounded => None,
    }
}

/// Refuses objects whose declared size is over the object limit, before any
/// write. Streamed sizes are checked again while writing.
fn check_declared_object_sizes(
    limits: BackupCapacityLimits,
    object_refs: &[crate::BackupObjectRef],
) -> Result<(), BackupError> {
    for object in object_refs {
        check_backup_limit(
            limits.object_bytes,
            object.size_bytes,
            &object.storage_key,
            BackupCapacityLimitKind::Object,
        )?;
    }
    Ok(())
}

#[must_use]
pub fn bytes_sha256_hex(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}

fn ensure_table_name(snapshot_id: Uuid, table_name: &str) -> Result<(), BackupError> {
    validate_table_name(table_name).map_err(|reason| BackupError::InvalidManifestEntry {
        snapshot_id,
        reason,
    })
}

/// Refuses object references that restore could not accept, before any write.
fn validate_object_refs(
    snapshot_id: Uuid,
    object_refs: &[crate::BackupObjectRef],
) -> Result<(), BackupError> {
    for object in object_refs {
        validate_sha256_hex(&object.sha256_hex).map_err(|reason| {
            BackupError::InvalidManifestEntry {
                snapshot_id,
                reason,
            }
        })?;
        crate::restore::validate_storage_key(&object.storage_key)?;
    }
    Ok(())
}

/// Exact size of the JSON Lines payload that restore decodes for `entries`.
fn decoded_jsonl_len<T>(entries: &[T]) -> Result<u64, BackupError>
where
    T: Serialize,
{
    struct Counter(u64);
    impl Write for Counter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .saturating_add(u64::try_from(buf.len()).unwrap_or(u64::MAX));
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    for entry in entries {
        serde_json::to_writer(&mut counter, entry)?;
        counter.write_all(b"\n").map_err(BackupError::compression)?;
    }
    Ok(counter.0)
}

/// Compressed output that stops growing at the stored-payload limit.
struct LimitedBuffer<'a> {
    bytes: Vec<u8>,
    limit: BackupByteLimit,
    exceeded: &'a std::cell::Cell<Option<u64>>,
}

impl Write for LimitedBuffer<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let next = u64::try_from(self.bytes.len().saturating_add(buf.len())).unwrap_or(u64::MAX);
        if let BackupByteLimit::Bytes(limit) = self.limit
            && next > limit
        {
            self.exceeded.set(Some(next));
            return Err(std::io::Error::other("stored payload limit exceeded"));
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Serializes and compresses entries, failing as soon as the compressed
/// payload would exceed `limit`, so it is never buffered beyond it.
///
/// The zstd window is kept within what restore's decoder accepts for
/// `decode_limit`, which bounds the window by the decoded-size limit.
fn serialize_jsonl_entries<T>(
    entries: &[T],
    content_key: &str,
    limit: BackupByteLimit,
    decode_limit: BackupByteLimit,
) -> Result<Vec<u8>, BackupError>
where
    T: Serialize,
{
    let exceeded = std::cell::Cell::new(None);
    let output = LimitedBuffer {
        bytes: Vec::new(),
        limit,
        exceeded: &exceeded,
    };
    let encoded = (move || {
        let mut encoder =
            zstd::stream::Encoder::new(output, 0).map_err(BackupError::compression)?;
        if let Some(window_log) = compatible_window_log(decode_limit) {
            encoder
                .window_log(window_log)
                .map_err(BackupError::compression)?;
        }
        for entry in entries {
            serde_json::to_writer(&mut encoder, entry)?;
            encoder.write_all(b"\n").map_err(BackupError::compression)?;
        }
        encoder.finish().map_err(BackupError::compression)
    })();
    if let Some(observed) = exceeded.get() {
        check_backup_limit(
            limit,
            observed,
            content_key,
            BackupCapacityLimitKind::StoredPayload,
        )?;
    }
    Ok(encoded?.bytes)
}

async fn write_object_entries(
    repository: &dyn BackupRepository,
    objects: &dyn BackupObjectIndex,
    object_refs: &[crate::BackupObjectRef],
    options: &BackupExecutionOptions,
) -> Result<Vec<ObjectBackupEntry>, BackupError> {
    let concurrency = options.object_concurrency.max(1);
    let limit = repository.capacity_limits().object_bytes;
    // Owned refs keep the stream free of higher-ranked borrows, which
    // otherwise break `Send` future inference in async resolvers that await
    // backup creation.
    let owned_refs = object_refs.to_vec();
    let cancellation = options.cancellation.clone();
    // Set by the first failure. Uploads already in flight are awaited rather
    // than dropped mid-request, so none can land after the lock is released.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let outcomes = stream::iter(owned_refs.into_iter().enumerate())
        .map(|(index, object)| {
            let cancellation = cancellation.clone();
            let stop = std::sync::Arc::clone(&stop);
            async move {
                // A safe point: an object that has not started is never begun.
                if cancellation.is_cancelled() || stop.load(std::sync::atomic::Ordering::SeqCst) {
                    return None;
                }
                let outcome = write_object_entry(repository, objects, index, object, limit).await;
                if outcome.is_err() {
                    stop.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                Some(outcome)
            }
        })
        .buffer_unordered(concurrency)
        .collect::<Vec<_>>()
        .await;
    let mut object_entries = Vec::with_capacity(outcomes.len());
    let mut skipped = false;
    for outcome in outcomes {
        match outcome {
            Some(Ok(entry)) => object_entries.push(entry),
            Some(Err(error)) => return Err(error),
            None => skipped = true,
        }
    }
    if skipped {
        return Err(BackupError::Cancelled);
    }
    object_entries.sort_by_key(|(index, _)| *index);
    let object_entries = object_entries.into_iter().map(|(_, entry)| entry).collect();
    Ok(object_entries)
}

/// Streamed hash, byte count and whether the object limit stopped the stream.
type ObjectStreamState = (Sha256, u64, bool);

async fn write_object_entry(
    repository: &dyn BackupRepository,
    objects: &dyn BackupObjectIndex,
    index: usize,
    object: crate::BackupObjectRef,
    limit: BackupByteLimit,
) -> Result<(usize, ObjectBackupEntry), BackupError> {
    let content_key = object_content_key(&object.sha256_hex);
    let source = objects.load_object_stream(&object).await?;
    let initial: ObjectStreamState = (Sha256::new(), 0_u64, false);
    let hash_state = std::sync::Arc::new(std::sync::Mutex::new(initial));
    let stream_state = std::sync::Arc::clone(&hash_state);
    let stream = source.into_inner().map(move |chunk| {
        let chunk = chunk?;
        let mut state = match stream_state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.1 = state.1.checked_add(chunk.len() as u64).ok_or_else(|| {
            graphql_orm_storage::StorageError::PreconditionFailed {
                key: "object stream".to_string(),
                condition: "byte count overflow".to_string(),
            }
        })?;
        if let BackupByteLimit::Bytes(limit_bytes) = limit
            && state.1 > limit_bytes
        {
            // Failing the body before its end aborts the conditional create.
            state.2 = true;
            return Err(graphql_orm_storage::StorageError::PreconditionFailed {
                key: "object stream".to_string(),
                condition: "object exceeds the configured object limit".to_string(),
            });
        }
        state.0.update(&chunk);
        Ok(chunk)
    });
    let written = repository
        .put_blob_stream_if_absent(&content_key, StorageByteStream::new(Box::pin(stream)))
        .await;
    let (limit_hit, streamed) = {
        let state = match hash_state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        (state.2, state.1)
    };
    if limit_hit {
        if matches!(written, Ok(true)) {
            repository.delete_blob(&content_key).await?;
        }
        check_backup_limit(
            limit,
            streamed,
            &object.storage_key,
            BackupCapacityLimitKind::Object,
        )?;
    }
    let written = written?;
    let (actual, observed_size) = if written {
        let hasher = match hash_state.lock() {
            Ok(hasher) => hasher.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        (format!("{:x}", hasher.0.finalize()), hasher.1)
    } else {
        // A matching stored blob does not establish that current source
        // bytes still match the index checksum. Preserve independent
        // source validation before reusing verified repository content.
        let (source_checksum, source_size) =
            hash_object_source(objects.load_object_stream(&object).await?).await?;
        if source_checksum != object.sha256_hex {
            return Err(BackupError::ChecksumMismatch {
                key: content_key,
                expected: object.sha256_hex.clone(),
                actual: source_checksum,
            });
        }
        check_backup_limit(
            limit,
            source_size,
            &object.storage_key,
            BackupCapacityLimitKind::Object,
        )?;
        let size = crate::verify::verify_blob_checksum(
            repository,
            content_key.clone(),
            object.sha256_hex.clone(),
            limit,
        )
        .await?;
        (object.sha256_hex.clone(), size)
    };
    if actual != object.sha256_hex {
        if written {
            repository.delete_blob(&content_key).await?;
        }
        return Err(BackupError::ChecksumMismatch {
            key: content_key,
            expected: object.sha256_hex.clone(),
            actual,
        });
    }

    Ok((
        index,
        ObjectBackupEntry {
            object_id: object.object_id,
            storage_key: object.storage_key.clone(),
            content_key,
            sha256_hex: object.sha256_hex.clone(),
            size_bytes: observed_size,
            mime_type: object.mime_type.clone(),
        },
    ))
}

async fn hash_object_source(source: StorageByteStream) -> Result<(String, u64), BackupError> {
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut stream = source.into_inner();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        size = size
            .checked_add(chunk.len() as u64)
            .ok_or(BackupError::CapacityExceeded {
                limit_bytes: u64::MAX,
            })?;
        hasher.update(&chunk);
    }
    Ok((format!("{:x}", hasher.finalize()), size))
}

struct ChangeGroup {
    table_name: String,
    changes: Vec<BackupChangeExport>,
}

fn group_changes_by_table(changes: Vec<BackupChangeExport>) -> Vec<ChangeGroup> {
    let mut groups = Vec::<ChangeGroup>::new();
    for change in changes {
        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.table_name == change.table_name)
        {
            group.changes.push(change);
        } else {
            groups.push(ChangeGroup {
                table_name: change.table_name.clone(),
                changes: vec![change],
            });
        }
    }
    groups
}

fn changes_to_tombstones(changes: &[BackupChangeExport]) -> Vec<BackupTombstone> {
    changes
        .iter()
        .filter(|change| matches!(change.action, BackupChangeAction::Delete))
        .map(|change| BackupTombstone {
            table_name: Some(change.table_name.clone()),
            primary_key: Some(change.primary_key.clone()),
            object_id: None,
            deleted_at: change.changed_at,
        })
        .collect()
}

fn compact_object_entries(chain: &[BackupSnapshotManifest]) -> Vec<ObjectBackupEntry> {
    let tombstoned_objects = chain
        .iter()
        .flat_map(|manifest| &manifest.tombstones)
        .filter_map(|tombstone| tombstone.object_id)
        .collect::<std::collections::HashSet<_>>();
    let mut objects_by_id = HashMap::<Uuid, ObjectBackupEntry>::new();

    for manifest in chain {
        for object in &manifest.objects {
            if !tombstoned_objects.contains(&object.object_id) {
                objects_by_id.insert(object.object_id, object.clone());
            }
        }
    }

    let mut objects = objects_by_id.into_values().collect::<Vec<_>>();
    objects.sort_by_key(|object| object.object_id);
    objects
}
