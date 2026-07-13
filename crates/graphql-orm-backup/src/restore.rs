use std::sync::Arc;

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

/// Restores a database snapshot chain through a `graphql-orm` backup adapter.
///
/// In [`RestoreMode::DryRun`] this validates, verifies, downloads, decompresses,
/// and parses all database payloads without calling adapter restore methods.
///
/// # Errors
///
/// Returns [`BackupError`] if the manifest chain is invalid, checksum
/// verification fails, target safety checks fail, payload parsing fails, or the
/// adapter restore call fails.
pub async fn restore_snapshot(
    repository: &dyn BackupRepository,
    database: &dyn GraphqlOrmBackupAdapter,
    snapshot_id: Uuid,
    context: RestoreContext,
) -> Result<RestoreResult, BackupError> {
    let chain = load_manifest_chain(repository, snapshot_id).await?;
    for manifest in &chain {
        verify_manifest_and_objects(repository, manifest).await?;
    }

    let target_is_empty = match context.mode {
        RestoreMode::DryRun => true,
        RestoreMode::EmptyDatabase => database.restore_target_is_empty().await?,
    };
    ensure_empty_restore_target(target_is_empty, &context)?;

    let full_manifest = chain
        .first()
        .ok_or_else(|| BackupError::InvalidManifestChain {
            reason: "manifest chain is empty".to_string(),
        })?;
    let full_export = load_table_exports(repository, full_manifest).await?;
    let full_table_count = full_export.len() as u64;
    let full_row_count = full_export
        .iter()
        .map(|table| table.rows.len() as u64)
        .sum::<u64>();

    if !matches!(context.mode, RestoreMode::DryRun) {
        database.restore_full(full_export, context.clone()).await?;
    }

    let mut incremental_change_count = 0_u64;
    for manifest in chain.iter().skip(1) {
        let changes = load_change_exports(repository, manifest).await?;
        incremental_change_count += changes.len() as u64;
        if !matches!(context.mode, RestoreMode::DryRun) {
            database
                .restore_incremental(changes, context.clone())
                .await?;
        }
    }

    Ok(RestoreResult {
        manifest_chain_len: chain.len(),
        full_table_count,
        full_row_count,
        incremental_change_count,
    })
}

/// Restores object blobs from a manifest through a caller-supplied sink.
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
