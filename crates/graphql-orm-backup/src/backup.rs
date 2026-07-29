use std::{
    collections::{BTreeMap, HashMap},
    io::Write,
};

use bytes::Bytes;
use futures::{StreamExt, TryStreamExt, stream};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use graphql_orm_storage::StorageByteStream;

use crate::{
    BACKUP_FORMAT_VERSION, BackupChangeAction, BackupChangeExport, BackupError, BackupKind,
    BackupObjectIndex, BackupRepository, BackupRow, BackupSnapshotManifest, BackupTableExport,
    BackupTombstone, DatabaseBackupManifest, GraphqlOrmBackupAdapter, ObjectBackupEntry,
    RepositoryLock, RepositoryLockOptions, TableBackupEntry, load_manifest_chain,
    manifest::sha256_hex, plan_full_backup, set_manifest_checksum, verify_manifest_and_objects,
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
}

impl Default for BackupExecutionOptions {
    fn default() -> Self {
        Self {
            object_concurrency: DEFAULT_OBJECT_CONCURRENCY,
            lock: RepositoryLockOptions::default(),
        }
    }
}

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
/// # Errors
///
/// Returns [`BackupError`] if planning fails, table serialization or
/// compression fails, object loading/checksum validation fails, or any
/// repository write fails.
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
    let lock = RepositoryLock::acquire(repository, &options.lock).await?;
    let result = create_full_backup_inner(repository, database, objects, request, options).await;
    release_lock(repository, lock, result).await
}

async fn create_full_backup_inner(
    repository: &dyn BackupRepository,
    database: &dyn GraphqlOrmBackupAdapter,
    objects: &dyn BackupObjectIndex,
    request: FullBackupRequest,
    options: &BackupExecutionOptions,
) -> Result<FullBackupResult, BackupError> {
    let plan = plan_full_backup(database, objects).await?;

    let mut table_entries = Vec::with_capacity(plan.tables.len());
    let mut row_count = 0_u64;
    for table in &plan.tables {
        let bytes = serialize_table_export(table)?;
        let content_key = database_table_key(request.snapshot_id, &table.table_name);
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

    let object_entries = write_object_entries(
        repository,
        objects,
        &plan.objects,
        options.object_concurrency,
    )
    .await?;

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
    };

    write_manifest(repository, &mut manifest).await?;

    Ok(FullBackupResult { manifest })
}

/// Creates an incremental snapshot in the repository.
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
    let lock = RepositoryLock::acquire(repository, &options.lock).await?;
    let result =
        create_incremental_backup_inner(repository, database, objects, request, options).await;
    release_lock(repository, lock, result).await
}

async fn create_incremental_backup_inner(
    repository: &dyn BackupRepository,
    database: &dyn GraphqlOrmBackupAdapter,
    objects: &dyn BackupObjectIndex,
    request: IncrementalBackupRequest,
    options: &BackupExecutionOptions,
) -> Result<IncrementalBackupResult, BackupError> {
    let schema = database.schema_snapshot().await?;
    let changes = database
        .export_incremental(request.parent_snapshot_id)
        .await?;
    let object_refs = objects
        .list_objects_for_incremental_backup(request.parent_snapshot_id)
        .await?;

    let mut change_entries = Vec::new();
    let mut change_count = 0_u64;
    let tombstones = changes_to_tombstones(&changes);
    for group in group_changes_by_table(changes) {
        let bytes = serialize_jsonl_entries(&group.changes)?;
        let content_key = database_changes_key(request.snapshot_id, &group.table_name);
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

    let object_entries = write_object_entries(
        repository,
        objects,
        &object_refs,
        options.object_concurrency,
    )
    .await?;

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
    };

    write_manifest(repository, &mut manifest).await?;

    Ok(IncrementalBackupResult { manifest })
}

/// Compacts a full-plus-incremental chain into a synthetic full snapshot.
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
    let lock = RepositoryLock::acquire(repository, &options.lock).await?;
    let result = compact_chain_inner(repository, request).await;
    release_lock(repository, lock, result).await
}

async fn compact_chain_inner(
    repository: &dyn BackupRepository,
    request: CompactChainRequest,
) -> Result<CompactChainResult, BackupError> {
    let chain = load_manifest_chain(repository, request.source_snapshot_id).await?;
    for manifest in &chain {
        verify_manifest_and_objects(repository, manifest).await?;
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

    for manifest in chain.iter().skip(1) {
        for change in crate::restore::load_change_exports(repository, manifest).await? {
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

    let mut table_entries = Vec::with_capacity(table_rows.len());
    let mut row_count = 0_u64;
    for (table_name, rows) in table_rows {
        let rows = rows.into_values().collect::<Vec<_>>();
        let bytes = serialize_jsonl_entries(&rows)?;
        let content_key = database_table_key(request.snapshot_id, &table_name);
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

    let object_entries = compact_object_entries(&chain);
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
    };

    write_manifest(repository, &mut manifest).await?;

    Ok(CompactChainResult { manifest })
}

/// Writes a manifest as the final snapshot blob.
///
/// # Errors
///
/// Returns [`BackupError`] if the checksum cannot be computed, the manifest
/// cannot be serialized, or the repository cannot write the blob.
pub async fn write_manifest(
    repository: &dyn BackupRepository,
    manifest: &mut BackupSnapshotManifest,
) -> Result<(), BackupError> {
    set_manifest_checksum(manifest)?;
    let body = serde_json::to_vec_pretty(manifest)?;
    repository
        .put_blob(
            &snapshot_manifest_key(manifest.snapshot_id),
            Bytes::from(body),
        )
        .await
}

#[must_use]
pub fn bytes_sha256_hex(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}

fn serialize_table_export(table: &BackupTableExport) -> Result<Vec<u8>, BackupError> {
    serialize_jsonl_entries(&table.rows)
}

fn serialize_jsonl_entries<T>(entries: &[T]) -> Result<Vec<u8>, BackupError>
where
    T: Serialize,
{
    let mut encoder =
        zstd::stream::Encoder::new(Vec::new(), 0).map_err(BackupError::compression)?;
    for entry in entries {
        serde_json::to_writer(&mut encoder, entry)?;
        encoder.write_all(b"\n").map_err(BackupError::compression)?;
    }
    encoder.finish().map_err(BackupError::compression)
}

async fn write_object_entries(
    repository: &dyn BackupRepository,
    objects: &dyn BackupObjectIndex,
    object_refs: &[crate::BackupObjectRef],
    object_concurrency: usize,
) -> Result<Vec<ObjectBackupEntry>, BackupError> {
    let concurrency = object_concurrency.max(1);
    // Owned refs keep the stream free of higher-ranked borrows, which
    // otherwise break `Send` future inference in async resolvers that await
    // backup creation.
    let owned_refs = object_refs.to_vec();
    let mut object_entries = stream::iter(owned_refs.into_iter().enumerate())
        .map(|(index, object)| async move {
            let content_key = object_content_key(&object.sha256_hex);
            let source = objects.load_object_stream(&object).await?;
            let hash_state = std::sync::Arc::new(std::sync::Mutex::new(Sha256::new()));
            let stream_state = std::sync::Arc::clone(&hash_state);
            let stream = source.into_inner().map(move |chunk| {
                let chunk = chunk?;
                let mut hasher = match stream_state.lock() {
                    Ok(hasher) => hasher,
                    Err(poisoned) => poisoned.into_inner(),
                };
                hasher.update(&chunk);
                Ok(chunk)
            });
            let written = repository
                .put_blob_stream_if_absent(&content_key, StorageByteStream::new(Box::pin(stream)))
                .await?;
            let actual = if written {
                let hasher = match hash_state.lock() {
                    Ok(hasher) => hasher.clone(),
                    Err(poisoned) => poisoned.into_inner().clone(),
                };
                format!("{:x}", hasher.finalize())
            } else {
                hash_object_source(objects.load_object_stream(&object).await?).await?
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
                    size_bytes: object.size_bytes,
                    mime_type: object.mime_type.clone(),
                },
            ))
        })
        .buffer_unordered(concurrency)
        .try_collect::<Vec<_>>()
        .await?;
    object_entries.sort_by_key(|(index, _)| *index);
    let object_entries = object_entries.into_iter().map(|(_, entry)| entry).collect();
    Ok(object_entries)
}

async fn hash_object_source(source: StorageByteStream) -> Result<String, BackupError> {
    let mut hasher = Sha256::new();
    let mut stream = source.into_inner();
    while let Some(chunk) = stream.next().await {
        hasher.update(&chunk?);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

async fn release_lock<T>(
    repository: &dyn BackupRepository,
    lock: RepositoryLock,
    result: Result<T, BackupError>,
) -> Result<T, BackupError> {
    let release_result = lock.release(repository).await;
    match (result, release_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(err), _) => Err(err),
        (Ok(_), Err(err)) => Err(err),
    }
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
