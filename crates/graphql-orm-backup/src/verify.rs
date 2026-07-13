use crate::{
    BackupError, BackupRepository, BackupSnapshotManifest, DEFAULT_OBJECT_CONCURRENCY,
    verify_manifest_checksum,
};
use futures::{StreamExt, TryStreamExt, stream};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Eq, PartialEq)]
/// Verification concurrency settings.
pub struct VerificationOptions {
    /// Maximum number of concurrent blob checksum reads.
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

    // Owned (key, checksum) pairs keep the streams free of higher-ranked
    // borrows, which otherwise break `Send` future inference in async
    // resolvers that await this function.
    let mut object_checks = Vec::with_capacity(manifest.objects.len());
    for object in &manifest.objects {
        object_checks.push((object.content_key.clone(), object.sha256_hex.clone()));
    }
    stream::iter(object_checks)
        .map(|(content_key, sha256)| verify_blob_checksum(repository, content_key, sha256))
        .buffer_unordered(concurrency)
        .try_collect::<Vec<_>>()
        .await?;

    let mut entry_checks =
        Vec::with_capacity(manifest.database.tables.len() + manifest.database.changes.len());
    for entry in manifest
        .database
        .tables
        .iter()
        .chain(manifest.database.changes.iter())
    {
        entry_checks.push((entry.content_key.clone(), entry.sha256_hex.clone()));
    }
    stream::iter(entry_checks)
        .map(|(content_key, sha256)| verify_blob_checksum(repository, content_key, sha256))
        .buffer_unordered(concurrency)
        .try_collect::<Vec<_>>()
        .await?;

    Ok(())
}

async fn verify_blob_checksum(
    repository: &dyn BackupRepository,
    content_key: String,
    expected_sha256_hex: String,
) -> Result<(), BackupError> {
    let mut body = repository.get_blob_stream(&content_key).await?.into_inner();
    let mut hasher = Sha256::new();
    while let Some(chunk) = body.next().await {
        hasher.update(&chunk?);
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected_sha256_hex {
        return Err(BackupError::ChecksumMismatch {
            key: content_key,
            expected: expected_sha256_hex,
            actual,
        });
    }

    Ok(())
}
