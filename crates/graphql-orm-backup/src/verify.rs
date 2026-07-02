use crate::{
    BackupError, BackupRepository, BackupSnapshotManifest, TableBackupEntry, manifest::sha256_hex,
    verify_manifest_checksum,
};

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
    for object in &manifest.objects {
        let bytes = repository.get_blob(&object.content_key).await?;
        let actual = sha256_hex(&bytes);
        if actual != object.sha256_hex {
            return Err(BackupError::ChecksumMismatch {
                key: object.content_key.clone(),
                expected: object.sha256_hex.clone(),
                actual,
            });
        }
    }

    for table in manifest
        .database
        .tables
        .iter()
        .chain(manifest.database.changes.iter())
    {
        verify_entry_checksum(repository, table).await?;
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
