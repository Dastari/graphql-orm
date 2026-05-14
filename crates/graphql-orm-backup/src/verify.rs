use crate::{
    BackupError, BackupRepository, BackupSnapshotManifest, manifest::sha256_hex,
    verify_manifest_checksum,
};

pub async fn verify_manifest_and_objects(
    repository: &dyn BackupRepository,
    manifest: &BackupSnapshotManifest,
) -> Result<(), BackupError> {
    verify_manifest_checksum(manifest)?;
    verify_object_checksums(repository, manifest).await
}

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

    for table in &manifest.database.tables {
        let bytes = repository.get_blob(&table.content_key).await?;
        let actual = sha256_hex(&bytes);
        if actual != table.sha256_hex {
            return Err(BackupError::ChecksumMismatch {
                key: table.content_key.clone(),
                expected: table.sha256_hex.clone(),
                actual,
            });
        }
    }

    Ok(())
}
