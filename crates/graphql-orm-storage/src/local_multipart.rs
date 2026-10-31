//! Local filesystem multipart uploads.
//!
//! Each upload is a directory `<root>/.graphql-orm-storage/multipart/<id>/`
//! holding `manifest.json`, one data file per stored part and one receipt per
//! part number pointing at its data file. Every file is written to a temporary
//! name, fsynced and renamed into place, so a crash leaves either the previous
//! or the new state. Staged parts survive a restart.
//!
//! Receipt commits, completion and abort hold an exclusive advisory lock on
//! `<id>.lock` beside the directory. The operating system releases it when a
//! process exits, so a crash never leaves an upload locked. A part upload,
//! completion or abort that waited for the lock re-checks that the upload
//! still exists, which gives a completion racing an abort exactly one outcome.

use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::local::{INTERNAL_STORAGE_DIR, LocalStorageBackend, temp_path_for};
use crate::multipart::{check_handle, check_part_number, multipart_error, verify_receipts};
use crate::{
    BlobWriteOutcome, IncompleteMultipartPage, IncompleteMultipartUpload, MultipartBlobStore,
    MultipartCompleteOutcome, MultipartErrorKind, MultipartLimits, MultipartStartOptions,
    MultipartUploadHandle, PartReceipt, StorageBackend, StorageByteStream, StorageError,
    validate_blob_key,
};

const MULTIPART_DIR: &str = "multipart";
const MANIFEST: &str = "manifest.json";
const MANIFEST_FORMAT: u8 = 1;
/// Largest local part by default; only S3 imposes a minimum part size.
pub(crate) const DEFAULT_MAX_PART_SIZE: u64 = 5 * 1024 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format_version: u8,
    key: String,
    content_type: Option<String>,
    if_not_exists: bool,
    started_at_unix: i64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPart {
    receipt: PartReceipt,
    data_file: String,
}

impl LocalStorageBackend {
    fn uploads_dir(&self) -> PathBuf {
        self.root().join(INTERNAL_STORAGE_DIR).join(MULTIPART_DIR)
    }

    /// The upload directory and lock path. Identifiers are v4 UUIDs, so a
    /// handle can never name a path outside the uploads directory.
    fn upload_paths(
        &self,
        handle: &MultipartUploadHandle,
    ) -> Result<(PathBuf, PathBuf), StorageError> {
        check_handle(handle, StorageBackend::Local)?;
        let id = Uuid::parse_str(&handle.upload_id)
            .map_err(|_| multipart_error(&handle.key, MultipartErrorKind::UnknownUpload))?;
        let id = id.hyphenated().to_string();
        if id != handle.upload_id {
            return Err(multipart_error(
                &handle.key,
                MultipartErrorKind::UnknownUpload,
            ));
        }
        let uploads = self.uploads_dir();
        Ok((uploads.join(&id), uploads.join(format!("{id}.lock"))))
    }
}

fn part_receipt_name(part_number: u32) -> String {
    format!("part-{part_number:05}.json")
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> StorageError + '_ {
    move |source| StorageError::io(path, source)
}

fn join_error(error: tokio::task::JoinError) -> StorageError {
    StorageError::Provider {
        backend: StorageBackend::Local.as_str().to_string(),
        message: format!("local multipart task failed: {error}"),
        retryable: false,
    }
}

fn json_error(path: &Path, error: serde_json::Error) -> StorageError {
    StorageError::Provider {
        backend: StorageBackend::Local.as_str().to_string(),
        message: format!(
            "local multipart record {} is invalid: {error}",
            path.display()
        ),
        retryable: false,
    }
}

/// Make a rename or creation inside `dir` durable.
fn sync_dir(dir: &Path) -> Result<(), StorageError> {
    #[cfg(unix)]
    {
        File::open(dir)
            .and_then(|dir| dir.sync_all())
            .map_err(io(dir))?;
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Write `bytes` to `path` through a fsynced temporary file and an atomic rename.
fn write_durably(path: &Path, bytes: &[u8]) -> Result<(), StorageError> {
    let dir = path
        .parent()
        .ok_or_else(|| StorageError::MissingParent { path: path.into() })?;
    let temp = dir.join(format!(".{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = File::create(&temp).map_err(io(&temp))?;
        file.write_all(bytes).map_err(io(&temp))?;
        file.sync_all().map_err(io(&temp))?;
        std::fs::rename(&temp, path).map_err(io(path))?;
        sync_dir(dir)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// Run `operation` while holding the upload's exclusive lock.
async fn locked<T: Send + 'static>(
    lock: PathBuf,
    operation: impl FnOnce() -> Result<T, StorageError> + Send + 'static,
) -> Result<T, StorageError> {
    tokio::task::spawn_blocking(move || {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock)
            .map_err(io(&lock))?;
        file.lock().map_err(io(&lock))?;
        let result = operation();
        drop(file);
        result
    })
    .await
    .map_err(join_error)?
}

fn read_manifest(dir: &Path) -> Result<Option<Manifest>, StorageError> {
    let path = dir.join(MANIFEST);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| json_error(&path, error)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(StorageError::io(path, source)),
    }
}

fn read_stored_parts(dir: &Path) -> Result<Vec<StoredPart>, StorageError> {
    let mut parts = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(parts),
        Err(source) => return Err(StorageError::io(dir, source)),
    };
    for entry in entries {
        let path = entry.map_err(io(dir))?.path();
        let is_receipt = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("part-") && name.ends_with(".json"));
        if !is_receipt {
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => return Err(StorageError::io(path, source)),
        };
        parts.push(serde_json::from_slice(&bytes).map_err(|error| json_error(&path, error))?);
    }
    parts.sort_by_key(|part: &StoredPart| part.receipt.part_number);
    Ok(parts)
}

/// Size and lowercase hex SHA-256 of an existing file.
fn file_digest(path: &Path) -> Result<(u64, String), StorageError> {
    let mut input = File::open(path).map_err(io(path))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut size = 0_u64;
    loop {
        let read = input.read(&mut buffer).map_err(io(path))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((size, format!("{:x}", hasher.finalize())))
}

/// Stream a part into `path`, hashing it and refusing it once it exceeds `max`.
async fn write_part(
    key: &str,
    path: &Path,
    body: StorageByteStream,
    max: u64,
) -> Result<(u64, String), StorageError> {
    let mut file = tokio::fs::File::create(path).await.map_err(io(path))?;
    let mut stream = body.into_inner();
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        size = size.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        if size > max {
            return Err(multipart_error(key, MultipartErrorKind::PartTooLarge));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(io(path))?;
    }
    file.sync_all().await.map_err(io(path))?;
    Ok((size, format!("{:x}", hasher.finalize())))
}

#[async_trait]
impl MultipartBlobStore for LocalStorageBackend {
    fn multipart_limits(&self) -> MultipartLimits {
        MultipartLimits::new(1, self.multipart_max_part_size(), 10_000)
    }

    async fn start_multipart(
        &self,
        key: &str,
        options: MultipartStartOptions,
    ) -> Result<MultipartUploadHandle, StorageError> {
        self.path_for(key)?;
        let id = Uuid::new_v4().hyphenated().to_string();
        let uploads = self.uploads_dir();
        let dir = uploads.join(&id);
        let manifest = Manifest {
            format_version: MANIFEST_FORMAT,
            key: key.to_string(),
            content_type: options.put.content_type,
            if_not_exists: options.if_not_exists,
            started_at_unix: OffsetDateTime::now_utc().unix_timestamp(),
        };
        let bytes = serde_json::to_vec(&manifest).map_err(|error| json_error(&dir, error))?;
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&uploads).map_err(io(&uploads))?;
            std::fs::create_dir(&dir).map_err(io(&dir))?;
            sync_dir(&uploads)?;
            write_durably(&dir.join(MANIFEST), &bytes)
        })
        .await
        .map_err(join_error)??;
        Ok(MultipartUploadHandle {
            backend: StorageBackend::Local,
            key: key.to_string(),
            upload_id: id,
            if_not_exists: options.if_not_exists,
        })
    }

    async fn upload_part(
        &self,
        handle: &MultipartUploadHandle,
        part_number: u32,
        body: StorageByteStream,
    ) -> Result<PartReceipt, StorageError> {
        let (dir, lock) = self.upload_paths(handle)?;
        check_part_number(&handle.key, part_number, self.multipart_limits())?;
        let unknown = || multipart_error(&handle.key, MultipartErrorKind::UnknownUpload);
        let manifest_dir = dir.clone();
        let manifest = tokio::task::spawn_blocking(move || read_manifest(&manifest_dir))
            .await
            .map_err(join_error)??;
        if manifest.is_none_or(|manifest| manifest.key != handle.key) {
            return Err(unknown());
        }

        // The data file has a unique name, so concurrent uploads of one part
        // number never write the same file; the receipt rename picks one.
        let data_file = format!("part-{part_number:05}.{}.data", Uuid::new_v4());
        let data_path = dir.join(&data_file);
        let written = write_part(
            &handle.key,
            &data_path,
            body,
            self.multipart_max_part_size(),
        )
        .await;
        let (size_bytes, sha256) = match written {
            Ok(written) => written,
            Err(error) => {
                let _ = tokio::fs::remove_file(&data_path).await;
                return Err(error);
            }
        };
        let receipt = PartReceipt {
            part_number,
            size_bytes,
            sha256: sha256.clone(),
            provider_tag: sha256,
        };
        let stored = StoredPart {
            receipt: receipt.clone(),
            data_file,
        };
        let key = handle.key.clone();
        locked(lock, move || {
            if read_manifest(&dir)?.is_none() {
                let _ = std::fs::remove_file(&data_path);
                return Err(multipart_error(&key, MultipartErrorKind::UnknownUpload));
            }
            let receipt_path = dir.join(part_receipt_name(part_number));
            let previous = std::fs::read(&receipt_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<StoredPart>(&bytes).ok());
            sync_dir(&dir)?;
            let bytes =
                serde_json::to_vec(&stored).map_err(|error| json_error(&receipt_path, error))?;
            write_durably(&receipt_path, &bytes)?;
            if let Some(previous) = previous
                && previous.data_file != stored.data_file
            {
                let _ = std::fs::remove_file(dir.join(previous.data_file));
            }
            Ok(())
        })
        .await?;
        Ok(receipt)
    }

    async fn list_parts(
        &self,
        handle: &MultipartUploadHandle,
    ) -> Result<Vec<PartReceipt>, StorageError> {
        let (dir, _) = self.upload_paths(handle)?;
        let key = handle.key.clone();
        tokio::task::spawn_blocking(move || {
            if read_manifest(&dir)?.is_none_or(|manifest| manifest.key != key) {
                return Err(multipart_error(&key, MultipartErrorKind::UnknownUpload));
            }
            Ok(read_stored_parts(&dir)?
                .into_iter()
                .map(|part| part.receipt)
                .collect())
        })
        .await
        .map_err(join_error)?
    }

    async fn complete_multipart(
        &self,
        handle: &MultipartUploadHandle,
        parts: &[PartReceipt],
    ) -> Result<MultipartCompleteOutcome, StorageError> {
        let (dir, lock) = self.upload_paths(handle)?;
        let target = self.path_for(&handle.key)?;
        let key = handle.key.clone();
        let parts = parts.to_vec();
        let limits = self.multipart_limits();
        let handle_condition = handle.if_not_exists;
        let lock_path = lock.clone();
        let outcome = locked(lock, move || {
            let manifest = read_manifest(&dir)?
                .filter(|manifest| manifest.key == key)
                .ok_or_else(|| multipart_error(&key, MultipartErrorKind::UnknownUpload))?;
            let stored = read_stored_parts(&dir)?;
            let receipts = stored
                .iter()
                .map(|part| part.receipt.clone())
                .collect::<Vec<_>>();
            verify_receipts(&key, &receipts, &parts, limits)?;

            let parent = target.parent().ok_or_else(|| StorageError::MissingParent {
                path: target.clone(),
            })?;
            std::fs::create_dir_all(parent).map_err(io(parent))?;
            let temp = temp_path_for(&target, &key)?;
            let assembled = (|| {
                let mut output = File::create(&temp).map_err(io(&temp))?;
                let mut hasher = Sha256::new();
                let mut size_bytes = 0_u64;
                let mut buffer = vec![0_u8; 1024 * 1024];
                for part in &stored {
                    let path = dir.join(&part.data_file);
                    let mut input = File::open(&path).map_err(io(&path))?;
                    let mut part_size = 0_u64;
                    let mut part_hasher = Sha256::new();
                    loop {
                        let read = input.read(&mut buffer).map_err(io(&path))?;
                        if read == 0 {
                            break;
                        }
                        part_hasher.update(&buffer[..read]);
                        hasher.update(&buffer[..read]);
                        output.write_all(&buffer[..read]).map_err(io(&temp))?;
                        part_size += read as u64;
                    }
                    // A part file changed on disk after its receipt was written.
                    if part_size != part.receipt.size_bytes
                        || format!("{:x}", part_hasher.finalize()) != part.receipt.sha256
                    {
                        return Err(multipart_error(&key, MultipartErrorKind::ReceiptMismatch));
                    }
                    size_bytes += part_size;
                }
                output.sync_all().map_err(io(&temp))?;
                Ok(BlobWriteOutcome {
                    size_bytes,
                    sha256_hex: format!("{:x}", hasher.finalize()),
                })
            })();
            let outcome = match assembled {
                Ok(outcome) => outcome,
                Err(error) => {
                    let _ = std::fs::remove_file(&temp);
                    return Err(error);
                }
            };

            let published = if manifest.if_not_exists || handle_condition {
                match std::fs::hard_link(&temp, &target) {
                    Ok(()) => Ok(()),
                    // A completion that crashed after publishing left these
                    // staged parts behind. Its retry finds its own object: that
                    // is success, recognised by an identical size and digest.
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        if file_digest(&target)? == (outcome.size_bytes, outcome.sha256_hex.clone())
                        {
                            Ok(())
                        } else {
                            Err(StorageError::PreconditionFailed {
                                key: key.clone(),
                                condition: "object already exists".to_string(),
                            })
                        }
                    }
                    Err(source) => Err(StorageError::io(&target, source)),
                }
            } else {
                std::fs::rename(&temp, &target).map_err(io(&target))
            };
            let _ = std::fs::remove_file(&temp);
            published?;
            sync_dir(parent)?;

            // The object is published; the staged parts are no longer needed.
            std::fs::remove_dir_all(&dir).map_err(io(&dir))?;
            Ok(outcome)
        })
        .await?;
        let _ = tokio::fs::remove_file(&lock_path).await;
        Ok(MultipartCompleteOutcome {
            size_bytes: outcome.size_bytes,
            sha256_hex: Some(outcome.sha256_hex),
        })
    }

    async fn abort_multipart(&self, handle: &MultipartUploadHandle) -> Result<(), StorageError> {
        let (dir, lock) = match self.upload_paths(handle) {
            Ok(paths) => paths,
            // An identifier this backend never issued names no upload.
            Err(StorageError::Multipart {
                kind: MultipartErrorKind::UnknownUpload,
                ..
            }) => return Ok(()),
            Err(error) => return Err(error),
        };
        if !tokio::fs::try_exists(&dir).await.map_err(io(&dir))? {
            let _ = tokio::fs::remove_file(&lock).await;
            return Ok(());
        }
        let lock_path = lock.clone();
        locked(lock, move || match std::fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(StorageError::io(&dir, source)),
        })
        .await?;
        let _ = tokio::fs::remove_file(&lock_path).await;
        Ok(())
    }

    async fn list_incomplete_multipart(
        &self,
        prefix: &str,
        continuation: Option<String>,
        limit: usize,
    ) -> Result<IncompleteMultipartPage, StorageError> {
        crate::blob::validate_blob_prefix(prefix)?;
        let after = continuation
            .map(|token| {
                serde_json::from_str::<(String, String)>(&token).map_err(|_| {
                    StorageError::PreconditionFailed {
                        key: prefix.to_string(),
                        condition: "invalid multipart listing continuation".to_string(),
                    }
                })
            })
            .transpose()?;
        let uploads = self.uploads_dir();
        let prefix = prefix.to_string();
        let mut found = tokio::task::spawn_blocking(move || {
            let mut found = Vec::new();
            let entries = match std::fs::read_dir(&uploads) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(found),
                Err(source) => return Err(StorageError::io(&uploads, source)),
            };
            for entry in entries {
                let path = entry.map_err(io(&uploads))?.path();
                let Some(id) = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| Uuid::parse_str(name).is_ok() && !name.ends_with(".lock"))
                    .map(str::to_string)
                else {
                    continue;
                };
                // A directory without a manifest is a start that never finished.
                let Some(manifest) = read_manifest(&path)? else {
                    continue;
                };
                if manifest.key.starts_with(&prefix) && validate_blob_key(&manifest.key).is_ok() {
                    found.push((
                        manifest.key,
                        id,
                        manifest.started_at_unix,
                        manifest.if_not_exists,
                    ));
                }
            }
            Ok(found)
        })
        .await
        .map_err(join_error)??;
        found.sort();
        let mut page = IncompleteMultipartPage::default();
        let mut remaining = found
            .into_iter()
            .filter(|(key, id, _, _)| {
                after
                    .as_ref()
                    .is_none_or(|(after_key, after_id)| (key, id) > (after_key, after_id))
            })
            .peekable();
        while let Some((key, id, started, if_not_exists)) = remaining.next() {
            if page.uploads.len() == limit.max(1) {
                break;
            }
            let more = remaining.peek().is_some();
            page.uploads.push(IncompleteMultipartUpload {
                handle: MultipartUploadHandle {
                    backend: StorageBackend::Local,
                    key: key.clone(),
                    upload_id: id.clone(),
                    if_not_exists,
                },
                started_at: OffsetDateTime::from_unix_timestamp(started).ok(),
            });
            if page.uploads.len() == limit.max(1) && more {
                page.next_continuation =
                    Some(serde_json::to_string(&(key, id)).expect("a string pair serializes"));
            }
        }
        Ok(page)
    }
}
