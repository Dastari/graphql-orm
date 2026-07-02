use crate::{
    BackupError, BackupRepository, BackupSnapshotManifest, DEFAULT_OBJECT_CONCURRENCY,
    ObjectBackupEntry, TableBackupEntry, manifest::sha256_hex, verify_manifest_checksum,
};
use futures::{StreamExt, TryStreamExt, stream};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerificationOptions {
    pub blob_concurrency: usize,
}

impl Default for VerificationOptions {
    fn default() -> Self {
        Self {
            blob_concurrency: DEFAULT_OBJECT_CONCURRENCY,
        }
    }
}

/// Verifies a manifest checksum and all referenced payload checksums.
///
/// # Errors
///
/// Returns [`BackupError`] if the manifest checksum is invalid, a referenced
/// blob is missing, or a payload checksum does not match.
pub async fn verify_manifest_and_objects(
    repository: &dyn BackupRepository,
    manifest: &BackupSnapshotManifest,
) -> Result<(), BackupError> {
    verify_manifest_checksum(manifest)?;
    verify_object_checksums(repository, manifest).await
}

/// Verifies a manifest checksum and payload checksums with explicit options.
///
/// # Errors
///
/// Returns [`BackupError`] if the manifest checksum is invalid, a referenced
/// blob is missing, or a payload checksum does not match.
pub async fn verify_manifest_and_objects_with_options(
    repository: &dyn BackupRepository,
    manifest: &BackupSnapshotManifest,
    options: &VerificationOptions,
) -> Result<(), BackupError> {
    verify_manifest_checksum(manifest)?;
    verify_object_checksums_with_options(repository, manifest, options).await
}

/// Verifies object, table, and change payload checksums in a manifest.
///
/// # Errors
///
/// Returns [`BackupError`] if any referenced blob is missing or its checksum
/// does not match the manifest entry.
pub async fn verify_object_checksums(
    repository: &dyn BackupRepository,
    manifest: &BackupSnapshotManifest,
) -> Result<(), BackupError> {
    verify_object_checksums_with_options(repository, manifest, &VerificationOptions::default())
        .await
}

/// Verifies object, table, and change payload checksums with explicit options.
///
/// # Errors
///
/// Returns [`BackupError`] if any referenced blob is missing or its checksum
/// does not match the manifest entry.
pub async fn verify_object_checksums_with_options(
    repository: &dyn BackupRepository,
    manifest: &BackupSnapshotManifest,
    options: &VerificationOptions,
) -> Result<(), BackupError> {
    let concurrency = options.blob_concurrency.max(1);

    stream::iter(&manifest.objects)
        .map(|object| verify_object_checksum(repository, object))
        .buffer_unordered(concurrency)
        .try_collect::<Vec<_>>()
        .await?;

    stream::iter(
        manifest
            .database
            .tables
            .iter()
            .chain(manifest.database.changes.iter()),
    )
    .map(|entry| verify_entry_checksum(repository, entry))
    .buffer_unordered(concurrency)
    .try_collect::<Vec<_>>()
    .await?;

    Ok(())
}

async fn verify_object_checksum(
    repository: &dyn BackupRepository,
    object: &ObjectBackupEntry,
) -> Result<(), BackupError> {
    let bytes = repository.get_blob(&object.content_key).await?;
    let actual = sha256_hex(&bytes);
    if actual != object.sha256_hex {
        return Err(BackupError::ChecksumMismatch {
            key: object.content_key.clone(),
            expected: object.sha256_hex.clone(),
            actual,
        });
    }

    Ok(())
}

async fn verify_entry_checksum(
    repository: &dyn BackupRepository,
    entry: &TableBackupEntry,
) -> Result<(), BackupError> {
    let bytes = repository.get_blob(&entry.content_key).await?;
    let actual = sha256_hex(&bytes);
    if actual != entry.sha256_hex {
        return Err(BackupError::ChecksumMismatch {
            key: entry.content_key.clone(),
            expected: entry.sha256_hex.clone(),
            actual,
        });
    }

    Ok(())
}
