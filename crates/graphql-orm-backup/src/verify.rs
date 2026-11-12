use crate::{
    BackupByteLimit, BackupError, BackupRepository, BackupSnapshotManifest,
    DEFAULT_OBJECT_CONCURRENCY, RepositoryLock, RepositoryLockOptions,
    manifest::required_authenticator, verify_manifest_authenticity,
};
use futures::{StreamExt, TryStreamExt, stream};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Eq, PartialEq)]
/// Verification concurrency settings.
pub struct VerificationOptions {
    /// Maximum number of concurrent blob checksum reads.
    pub blob_concurrency: usize,
    /// Repository lock settings. Verification holds the repository lock so
    /// that no prune, deletion or compaction removes blobs while it reads.
    pub lock: RepositoryLockOptions,
}

impl Default for VerificationOptions {
    fn default() -> Self {
        Self {
            blob_concurrency: DEFAULT_OBJECT_CONCURRENCY,
            lock: RepositoryLockOptions::default(),
        }
    }
}

/// Authenticates a manifest with the repository's host key, then verifies all
/// referenced payload checksums.
///
/// It holds the repository lock while reading, so a concurrent prune, snapshot
/// deletion or compaction cannot remove the blobs it verifies; it fails with
/// [`BackupError::RepositoryLocked`] while another operation holds the lock.
/// Objects larger than [`crate::BackupCapacityLimits::object_bytes`] fail
/// with [`BackupError::CapacityExceeded`], as object restore would.
///
/// # Errors
///
/// Returns [`BackupError::ManifestAuthenticationRequired`] if the repository
/// has no authenticator, an error from [`verify_manifest_authenticity`], or
/// another [`BackupError`] if a referenced blob is missing or a payload
/// checksum does not match.
pub async fn verify_manifest_and_objects(
    repository: &dyn BackupRepository,
    manifest: &BackupSnapshotManifest,
) -> Result<(), BackupError> {
    verify_manifest_and_objects_with_options(repository, manifest, &VerificationOptions::default())
        .await
}

/// Authenticates a manifest and verifies payload checksums with explicit
/// options.
///
/// # Errors
///
/// Returns [`BackupError::ManifestAuthenticationRequired`] if the repository
/// has no authenticator, an error from [`verify_manifest_authenticity`], or
/// another [`BackupError`] if a referenced blob is missing or a payload
/// checksum does not match.
pub async fn verify_manifest_and_objects_with_options(
    repository: &dyn BackupRepository,
    manifest: &BackupSnapshotManifest,
    options: &VerificationOptions,
) -> Result<(), BackupError> {
    required_authenticator(repository)?;
    let lock = RepositoryLock::acquire_for(repository, &options.lock, "verify").await?;
    let result = verify_manifest_and_objects_unlocked(repository, manifest, options).await;
    lock.finish(result, |()| None).await
}

/// [`verify_manifest_and_objects_with_options`] for callers that already
/// hold the repository lock.
pub(crate) async fn verify_manifest_and_objects_unlocked(
    repository: &dyn BackupRepository,
    manifest: &BackupSnapshotManifest,
    options: &VerificationOptions,
) -> Result<(), BackupError> {
    verify_manifest_authenticity(required_authenticator(repository)?, manifest)?;
    verify_object_checksums_unlocked(repository, manifest, options).await
}

/// Verifies object, table, and change payload checksums in a manifest.
///
/// This checks stored bytes against the manifest's digests only. It does not
/// authenticate the manifest; use [`verify_manifest_and_objects`] for that.
/// Like that function, it holds the repository lock while reading.
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
    let lock = RepositoryLock::acquire_for(repository, &options.lock, "verify").await?;
    let result = verify_object_checksums_unlocked(repository, manifest, options).await;
    lock.finish(result, |()| None).await
}

async fn verify_object_checksums_unlocked(
    repository: &dyn BackupRepository,
    manifest: &BackupSnapshotManifest,
    options: &VerificationOptions,
) -> Result<(), BackupError> {
    crate::validate_manifest_entries(manifest)?;
    let concurrency = options.blob_concurrency.max(1);
    let object_limit = repository.capacity_limits().object_bytes;

    // Owned (key, checksum) pairs keep the streams free of higher-ranked
    // borrows, which otherwise break `Send` future inference in async
    // resolvers that await this function.
    let mut object_checks = Vec::with_capacity(manifest.objects.len());
    for object in &manifest.objects {
        object_checks.push((object.content_key.clone(), object.sha256_hex.clone()));
    }
    stream::iter(object_checks)
        .map(|(content_key, sha256)| {
            verify_blob_checksum(repository, content_key, sha256, object_limit)
        })
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
        .map(|(content_key, sha256)| {
            verify_blob_checksum(repository, content_key, sha256, BackupByteLimit::Unbounded)
        })
        .buffer_unordered(concurrency)
        .try_collect::<Vec<_>>()
        .await?;

    Ok(())
}

pub(crate) async fn verify_blob_checksum(
    repository: &dyn BackupRepository,
    content_key: String,
    expected_sha256_hex: String,
    limit: BackupByteLimit,
) -> Result<u64, BackupError> {
    let mut body = repository.get_blob_stream(&content_key).await?.into_inner();
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        size = size
            .checked_add(chunk.len() as u64)
            .ok_or(BackupError::CapacityExceeded {
                limit_bytes: u64::MAX,
            })?;
        limit.check(size)?;
        hasher.update(&chunk);
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected_sha256_hex {
        return Err(BackupError::ChecksumMismatch {
            key: content_key,
            expected: expected_sha256_hex,
            actual,
        });
    }

    Ok(size)
}
