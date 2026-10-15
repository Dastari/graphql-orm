use std::{fmt, sync::Arc};

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use graphql_orm_storage::{BlobPutOptions, BlobStore, StorageByteStream, collect_storage_stream};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    BackupChangeExport, BackupCompression, BackupError, BackupObjectRef, BackupRepository,
    BackupSnapshotManifest, BackupTableExport, GraphqlOrmBackupAdapter, ObjectBackupEntry,
    TableBackupEntry, decompress_payload, load_manifest_chain, verify_manifest_and_objects,
};

#[derive(Clone, Debug, Eq, PartialEq)]
/// Restore execution mode.
pub enum RestoreMode {
    /// Apply restore only if the target is empty.
    EmptyDatabase,
    /// Validate and parse without applying database changes.
    DryRun,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Restore behavior flags passed to database adapters.
pub struct RestoreContext {
    /// Restore mode.
    pub mode: RestoreMode,
    /// Whether application policies should be disabled during restore.
    pub disable_policies: bool,
    /// Whether change journaling should be disabled during restore.
    pub disable_change_journal: bool,
}

/// Phase in which a database snapshot-chain restore failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreFailurePhase {
    /// Chain, integrity, schema or target validation failed before apply calls.
    Validation,
    /// Downloading, decompressing or parsing a stage failed without applying it.
    Preparation,
    /// An adapter apply call failed and may have partially changed its target.
    Apply,
}

/// Progress reported when a database snapshot-chain restore fails.
///
/// Completed stages are calls that the adapter reported as successful. They are
/// not a safe resume checkpoint: the failing apply stage may be partially applied,
/// and orchestration does not roll back earlier stages.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreFailureProgress {
    /// Snapshot requested by the caller.
    pub requested_snapshot_id: Uuid,
    /// Manifest being processed, when it was available at the failure boundary.
    pub failed_manifest_id: Option<Uuid>,
    /// Failure boundary; only apply can modify the failing stage's target.
    pub phase: RestoreFailurePhase,
    /// Manifests whose apply calls returned success, in execution order.
    /// This remains empty during dry run.
    pub completed_manifest_ids: Vec<Uuid>,
}

impl RestoreFailureProgress {
    /// Whether this restore attempt may have changed the target.
    ///
    /// Preparation never applies its failing stage, but prior completed stages
    /// may remain. An apply failure has an unknown outcome even with no completed
    /// stages. This says nothing about data that existed before the attempt.
    #[must_use]
    pub fn target_may_have_changed(&self) -> bool {
        self.phase == RestoreFailurePhase::Apply || !self.completed_manifest_ids.is_empty()
    }
}

impl fmt::Display for RestoreFailureProgress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.phase {
            RestoreFailurePhase::Validation => {
                formatter.write_str("restore validation failed before any writes were attempted")
            }
            RestoreFailurePhase::Preparation if self.completed_manifest_ids.is_empty() => {
                formatter.write_str("restore preparation failed before any writes were attempted")
            }
            RestoreFailurePhase::Preparation => write!(formatter,
                "restore preparation failed after {} completed stages; target may contain earlier restored stages",
                self.completed_manifest_ids.len()),
            RestoreFailurePhase::Apply => write!(formatter,
                "restore apply failed after {} completed stages; failing stage may be partially applied",
                self.completed_manifest_ids.len()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Summary returned by `restore_snapshot`.
pub struct RestoreResult {
    /// Number of manifests in the restored chain.
    pub manifest_chain_len: usize,
    /// Number of full table payloads parsed.
    pub full_table_count: u64,
    /// Number of full rows parsed.
    pub full_row_count: u64,
    /// Number of incremental changes parsed.
    pub incremental_change_count: u64,
}

#[async_trait]
/// Sink used by `restore_objects` to rehydrate application object stores.
pub trait RestoreObjectSink: Send + Sync {
    /// Restores one object loaded from a backup repository.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the caller-supplied sink cannot persist the
    /// object bytes.
    async fn restore_object(
        &self,
        object: BackupObjectRef,
        bytes: Bytes,
    ) -> Result<(), BackupError>;

    /// Restores one object from a stream.
    ///
    /// Existing sinks remain compatible through the buffered default. Native
    /// streaming sinks should override this method.
    async fn restore_object_stream(
        &self,
        object: BackupObjectRef,
        body: StorageByteStream,
    ) -> Result<(), BackupError> {
        self.restore_object(object, collect_storage_stream(body).await?)
            .await
    }
}

/// [`RestoreObjectSink`] that writes object bytes back to a
/// `graphql-orm-storage` [`BlobStore`] at each object's original storage key.
#[derive(Clone)]
pub struct BlobStoreRestoreObjectSink {
    store: Arc<dyn BlobStore>,
}

impl BlobStoreRestoreObjectSink {
    /// Creates a sink over the application's primary object blob store.
    #[must_use]
    pub fn new(store: Arc<dyn BlobStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl RestoreObjectSink for BlobStoreRestoreObjectSink {
    async fn restore_object(
        &self,
        object: BackupObjectRef,
        bytes: Bytes,
    ) -> Result<(), BackupError> {
        self.store
            .put_blob(
                &object.storage_key,
                StorageByteStream::from_bytes(bytes),
                BlobPutOptions {
                    content_type: object.mime_type.clone(),
                },
            )
            .await?;
        Ok(())
    }

    async fn restore_object_stream(
        &self,
        object: BackupObjectRef,
        body: StorageByteStream,
    ) -> Result<(), BackupError> {
        self.store
            .put_blob(
                &object.storage_key,
                body,
                BlobPutOptions {
                    content_type: object.mime_type.clone(),
                },
            )
            .await?;
        Ok(())
    }
}

impl RestoreContext {
    /// Builds the default empty-database restore context.
    #[must_use]
    pub fn empty_database() -> Self {
        Self {
            mode: RestoreMode::EmptyDatabase,
            disable_policies: true,
            disable_change_journal: true,
        }
    }

    /// Builds a dry-run restore context.
    #[must_use]
    pub fn dry_run() -> Self {
        Self {
            mode: RestoreMode::DryRun,
            disable_policies: true,
            disable_change_journal: true,
        }
    }
}

/// Ensures an empty-target restore is only applied to an empty database.
///
/// # Errors
///
/// Returns [`BackupError::RestoreTargetNotEmpty`] when the context requires an
/// empty database and the target is not empty.
pub fn ensure_empty_restore_target(
    target_is_empty: bool,
    context: &RestoreContext,
) -> Result<(), BackupError> {
    match context.mode {
        RestoreMode::EmptyDatabase if target_is_empty => Ok(()),
        RestoreMode::EmptyDatabase => Err(BackupError::RestoreTargetNotEmpty),
        RestoreMode::DryRun => Ok(()),
    }
}

fn manifest_schema(manifest: &BackupSnapshotManifest) -> crate::GraphqlOrmBackupSchema {
    crate::GraphqlOrmBackupSchema {
        backend: manifest.database_backend.clone(),
        migration_version: manifest.graphql_orm_schema_version.clone(),
        schema_hash: manifest.graphql_orm_schema_hash.clone(),
    }
}

async fn verify_restore_schema(
    database: &dyn GraphqlOrmBackupAdapter,
    manifest: &BackupSnapshotManifest,
) -> Result<crate::GraphqlOrmBackupSchema, BackupError> {
    let backup = manifest_schema(manifest);
    let target = database.schema_snapshot().await?;
    if backup.backend != target.backend || backup.schema_hash != target.schema_hash {
        return Err(BackupError::RestoreSchemaMismatch {
            backup_backend: backup.backend,
            backup_schema_hash: backup.schema_hash,
            target_backend: target.backend,
            target_schema_hash: target.schema_hash,
        });
    }
    Ok(backup)
}

/// Restores a database snapshot chain through a `graphql-orm` backup adapter.
///
/// In [`RestoreMode::DryRun`] this validates, verifies, downloads, decompresses,
/// and parses all database payloads without calling adapter restore methods.
///
/// Applying stages is not a whole-chain transaction. Earlier successful stages
/// may remain when a later stage fails, and an adapter apply failure can itself
/// be partial. Use a disposable staging database and publish it only after all
/// required database and object restore work succeeds. A progress report is not
/// permission to resume or retry in place; reconcile with the adapter or discard
/// the failed staging target and restart from an empty one. Cancellation or
/// dropping this future cannot return progress or roll back an active adapter
/// call; treat cancellation during apply as an unknown target outcome.
///
/// # Errors
///
/// Returns [`BackupError::RestoreStageFailed`] with phase/progress metadata and
/// the original cause when validation, payload preparation or adapter apply fails.
/// Validation and preparation never apply their failing stage. Earlier completed
/// stages may remain after later preparation failure; failed apply may be partial.
/// The primary display does not interpolate adapter/provider error details.
pub async fn restore_snapshot(
    repository: &dyn BackupRepository,
    database: &dyn GraphqlOrmBackupAdapter,
    snapshot_id: Uuid,
    context: RestoreContext,
) -> Result<RestoreResult, BackupError> {
    let chain = load_manifest_chain(repository, snapshot_id).await.map_err(|source| {
        restore_stage_error(snapshot_id, None, RestoreFailurePhase::Validation, &[], source)
    })?;
    for manifest in &chain {
        verify_manifest_and_objects(repository, manifest).await.map_err(|source| {
            restore_stage_error(snapshot_id, Some(manifest.snapshot_id), RestoreFailurePhase::Validation, &[], source)
        })?;
    }

    let full_manifest = chain.first().ok_or_else(|| {
        restore_stage_error(snapshot_id, None, RestoreFailurePhase::Validation, &[],
            BackupError::InvalidManifestChain { reason: "manifest chain is empty".to_string() })
    })?;
    let full_id = full_manifest.snapshot_id;
    let backup_schema = verify_restore_schema(database, full_manifest).await.map_err(|source| {
        restore_stage_error(snapshot_id, Some(full_id), RestoreFailurePhase::Validation, &[], source)
    })?;

    let target_is_empty = match context.mode {
        RestoreMode::DryRun => true,
        RestoreMode::EmptyDatabase => database.restore_target_is_empty().await.map_err(|source| {
            restore_stage_error(snapshot_id, Some(full_id), RestoreFailurePhase::Validation, &[], source)
        })?,
    };
    ensure_empty_restore_target(target_is_empty, &context).map_err(|source| {
        restore_stage_error(snapshot_id, Some(full_id), RestoreFailurePhase::Validation, &[], source)
    })?;

    let full_export = load_table_exports(repository, full_manifest).await.map_err(|source| {
        restore_stage_error(snapshot_id, Some(full_id), RestoreFailurePhase::Preparation, &[], source)
    })?;
    let full_table_count = full_export.len() as u64;
    let full_row_count = full_export.iter().map(|table| table.rows.len() as u64).sum::<u64>();
    let mut completed = Vec::new();
    if !matches!(context.mode, RestoreMode::DryRun) {
        database.restore_full(backup_schema, full_export, context.clone()).await.map_err(|source| {
            restore_stage_error(snapshot_id, Some(full_id), RestoreFailurePhase::Apply, &completed, source)
        })?;
        completed.push(full_id);
    }

    let mut incremental_change_count = 0_u64;
    for manifest in chain.iter().skip(1) {
        let changes = load_change_exports(repository, manifest).await.map_err(|source| {
            restore_stage_error(snapshot_id, Some(manifest.snapshot_id), RestoreFailurePhase::Preparation, &completed, source)
        })?;
        incremental_change_count += changes.len() as u64;
        if !matches!(context.mode, RestoreMode::DryRun) {
            database.restore_incremental(changes, context.clone()).await.map_err(|source| {
                restore_stage_error(snapshot_id, Some(manifest.snapshot_id), RestoreFailurePhase::Apply, &completed, source)
            })?;
            completed.push(manifest.snapshot_id);
        }
    }

    Ok(RestoreResult {
        manifest_chain_len: chain.len(),
        full_table_count,
        full_row_count,
        incremental_change_count,
    })
}

fn restore_stage_error(
    requested_snapshot_id: Uuid,
    failed_manifest_id: Option<Uuid>,
    phase: RestoreFailurePhase,
    completed_manifest_ids: &[Uuid],
    source: BackupError,
) -> BackupError {
    BackupError::RestoreStageFailed {
        progress: RestoreFailureProgress {
            requested_snapshot_id,
            failed_manifest_id,
            phase,
            completed_manifest_ids: completed_manifest_ids.to_vec(),
        },
        source: Box::new(source),
    }
}

/// Restores object blobs from a manifest through a caller-supplied sink.
///
/// This is a separate boundary from database restore. Earlier objects may remain
/// after a later failure, and the failing object can be partially written by a
/// custom sink. Checksum errors can be reported after sink writes. Use an owned
/// staging object namespace and publish only after all required verification and
/// database/object work succeeds; no cross-store rollback is provided.
///
/// # Errors
///
/// Returns [`BackupError`] if an object blob is missing, checksum verification
/// fails, or the sink rejects an object.
pub async fn restore_objects(
    repository: &dyn BackupRepository,
    manifest: &BackupSnapshotManifest,
    sink: &dyn RestoreObjectSink,
) -> Result<(), BackupError> {
    for object in &manifest.objects {
        let source = repository.get_blob_stream(&object.content_key).await?;
        let expected = object.sha256_hex.clone();
        let key = object.content_key.clone();
        let state = std::sync::Arc::new(std::sync::Mutex::new(Sha256::new()));
        let hash_state = std::sync::Arc::clone(&state);
        let stream = source.into_inner().map(move |chunk| {
            let chunk = chunk?;
            let mut hasher = match hash_state.lock() {
                Ok(hasher) => hasher,
                Err(poisoned) => poisoned.into_inner(),
            };
            hasher.update(&chunk);
            Ok::<_, graphql_orm_storage::StorageError>(chunk)
        });
        sink.restore_object_stream(
            object_ref_from_entry(object),
            StorageByteStream::new(Box::pin(stream)),
        )
        .await?;
        let actual = format!(
            "{:x}",
            match state.lock() {
                Ok(hasher) => hasher.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            }
            .finalize()
        );
        if actual != expected {
            return Err(BackupError::ChecksumMismatch {
                key,
                expected,
                actual,
            });
        }
    }

    Ok(())
}

pub(crate) async fn load_table_exports(
    repository: &dyn BackupRepository,
    manifest: &BackupSnapshotManifest,
) -> Result<Vec<BackupTableExport>, BackupError> {
    let mut exports = Vec::with_capacity(manifest.database.tables.len());
    for table in &manifest.database.tables {
        let rows = load_jsonl_entries(repository, manifest, table).await?;
        exports.push(BackupTableExport {
            table_name: table.table_name.clone(),
            rows,
        });
    }
    Ok(exports)
}

pub(crate) async fn load_change_exports(
    repository: &dyn BackupRepository,
    manifest: &BackupSnapshotManifest,
) -> Result<Vec<BackupChangeExport>, BackupError> {
    let mut changes = Vec::new();
    for table in &manifest.database.changes {
        changes.extend(load_jsonl_entries(repository, manifest, table).await?);
    }
    Ok(changes)
}

async fn load_jsonl_entries<T>(
    repository: &dyn BackupRepository,
    manifest: &BackupSnapshotManifest,
    entry: &TableBackupEntry,
) -> Result<Vec<T>, BackupError>
where
    T: DeserializeOwned,
{
    if manifest.database.export_format != crate::DATABASE_EXPORT_FORMAT {
        return Err(BackupError::UnsupportedOperation {
            operation: format!("database export format {}", manifest.database.export_format),
        });
    }

    let stored = repository.get_blob(&entry.content_key).await?;
    let payload = decode_payload(&stored, &manifest.database.compression)?;
    parse_jsonl(&payload)
}

fn decode_payload(bytes: &[u8], compression: &BackupCompression) -> Result<Vec<u8>, BackupError> {
    match compression {
        BackupCompression::None => Ok(bytes.to_vec()),
        BackupCompression::Zstd => decompress_payload(bytes),
    }
}

fn parse_jsonl<T>(payload: &[u8]) -> Result<Vec<T>, BackupError>
where
    T: DeserializeOwned,
{
    let mut entries = Vec::new();
    for line in payload.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        entries.push(serde_json::from_slice(line)?);
    }
    Ok(entries)
}

fn object_ref_from_entry(entry: &ObjectBackupEntry) -> BackupObjectRef {
    BackupObjectRef {
        object_id: entry.object_id,
        storage_key: entry.storage_key.clone(),
        sha256_hex: entry.sha256_hex.clone(),
        size_bytes: entry.size_bytes,
        mime_type: entry.mime_type.clone(),
    }
}
