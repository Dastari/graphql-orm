//! Local filesystem multipart uploads.
//!
//! Each upload is a directory `<root>/.graphql-orm-storage/multipart/<id>/`
//! holding `manifest.json`, one data file per stored part and one receipt per
//! part number pointing at its data file. Every file is written to a temporary
//! name, fsynced and renamed into place, so a crash leaves either the previous
//! or the new state. Staged parts survive a restart.
//!
//! Every staging file is reached the way the rest of local storage reaches
//! its files: from the root, one component at a time relative to held
//! directory handles, never following a link. A manifest, receipt or part
//! file that is a link, or that shares its data with another hard link, is
//! refused with `InvalidStorageKey`, so completion never assembles bytes
//! from outside the store.
//!
//! Receipt commits, completion and abort hold an exclusive advisory lock on
//! `<id>.lock` beside the directory. The operating system releases it when a
//! process exits, so a crash never leaves an upload locked. A part upload,
//! completion or abort that waited for the lock re-checks that the upload
//! still exists, which gives a completion racing an abort exactly one outcome.

use std::{
    io::{Read, Write},
    path::Path,
    sync::Arc,
};

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::key::temp_file_name;
use crate::local::{
    DurabilityStep, INTERNAL_STORAGE_DIR, LocalStorageBackend, check_leaf, fs_error, sync_dir,
    trace, trace_published,
};
use crate::local_fs::{Dir, Kind};
use crate::multipart::{check_handle, check_part_number, multipart_error, verify_receipts};
use crate::{
    BlobWriteOutcome, IncompleteMultipartPage, IncompleteMultipartUpload, MultipartBlobStore,
    MultipartCompleteOutcome, MultipartErrorKind, MultipartLimits, MultipartStartOptions,
    MultipartUploadHandle, PartReceipt, StorageBackend, StorageByteStream, StorageError,
    validate_blob_key,
};

const MULTIPART_DIR: &str = "multipart";
/// The uploads directory below the root.
const UPLOADS: [&str; 2] = [INTERNAL_STORAGE_DIR, MULTIPART_DIR];
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

/// The held uploads directory and the upload's own directory in it.
struct Upload {
    uploads: Arc<Dir>,
    dir: Arc<Dir>,
}

impl LocalStorageBackend {
    /// The upload's identifier. Identifiers are v4 UUIDs in their canonical
    /// form, so a handle can never name a path outside the uploads
    /// directory.
    fn upload_id(&self, handle: &MultipartUploadHandle) -> Result<String, StorageError> {
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
        Ok(id)
    }

    /// The uploads directory, or `None` when no upload was ever started.
    fn uploads_blocking(&self, key: &str) -> Result<Option<Arc<Dir>>, StorageError> {
        Ok(self.open_dir_blocking(&UPLOADS, key)?.map(Arc::new))
    }

    /// The upload `id`, or `None` when it does not exist.
    fn upload_blocking(&self, id: &str, key: &str) -> Result<Option<Upload>, StorageError> {
        let Some(uploads) = self.uploads_blocking(key)? else {
            return Ok(None);
        };
        let dir = uploads
            .child_dir(id)
            .map_err(|error| fs_error(error, key, &uploads.child_path(id)))?;
        Ok(dir.map(|dir| Upload {
            uploads,
            dir: Arc::new(dir),
        }))
    }
}

fn lock_name(id: &str) -> String {
    format!("{id}.lock")
}

fn part_receipt_name(part_number: u32) -> String {
    format!("part-{part_number:05}.json")
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> StorageError + '_ {
    move |source| StorageError::io(path, source)
}

fn not_found(path: &Path) -> StorageError {
    StorageError::io(path, std::io::ErrorKind::NotFound.into())
}

pub(crate) fn join_error(error: tokio::task::JoinError) -> StorageError {
    StorageError::Provider {
        backend: StorageBackend::Local.as_str().to_string(),
        message: format!("local storage task failed: {error}"),
        retryable: false,
    }
}

fn json_error(error: serde_json::Error) -> StorageError {
    StorageError::Provider {
        backend: StorageBackend::Local.as_str().to_string(),
        message: format!("local multipart record is invalid: {error}"),
        retryable: false,
    }
}

/// Write `bytes` to `name` in `dir` through a fsynced temporary file and an
/// atomic rename.
fn write_durably(dir: &Dir, name: &str, bytes: &[u8]) -> Result<(), StorageError> {
    let temp = format!(".{}.tmp", Uuid::new_v4());
    let temp_path = dir.child_path(&temp);
    let path = dir.child_path(name);
    let result = (|| {
        let mut file = dir.create_new(&temp).map_err(io(&temp_path))?;
        file.write_all(bytes).map_err(io(&temp_path))?;
        file.sync_all().map_err(io(&temp_path))?;
        dir.rename(&temp, name).map_err(io(&path))?;
        sync_dir(dir)
    })();
    if result.is_err() {
        let _ = dir.remove_file(&temp);
    }
    result
}

/// Run `operation` while holding the upload's exclusive lock.
async fn locked<T: Send + 'static>(
    uploads: Arc<Dir>,
    lock: String,
    key: String,
    operation: impl FnOnce() -> Result<T, StorageError> + Send + 'static,
) -> Result<T, StorageError> {
    tokio::task::spawn_blocking(move || {
        let path = uploads.child_path(&lock);
        let file = uploads
            .open_or_create(&lock)
            .map_err(|error| fs_error(error, &key, &path))?;
        file.lock().map_err(io(&path))?;
        let result = operation();
        drop(file);
        result
    })
    .await
    .map_err(join_error)?
}

/// Reads the whole regular file `name` in `dir` without following a link,
/// or `None` when it does not exist.
fn read_file(dir: &Dir, name: &str, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
    let path = dir.child_path(name);
    let Some(mut file) = dir
        .open_read(name)
        .map_err(|error| fs_error(error, key, &path))?
    else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(io(&path))?;
    Ok(Some(bytes))
}

fn read_manifest(dir: &Dir, key: &str) -> Result<Option<Manifest>, StorageError> {
    match read_file(dir, MANIFEST, key)? {
        Some(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(json_error),
        None => Ok(None),
    }
}

fn read_stored_parts(dir: &Dir, key: &str) -> Result<Vec<StoredPart>, StorageError> {
    let mut parts = Vec::new();
    let entries = match dir.entries() {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(parts),
        Err(source) => return Err(StorageError::io(dir.path(), source)),
    };
    for entry in entries {
        let Some(name) = entry.name.to_str() else {
            continue;
        };
        if !(name.starts_with("part-") && name.ends_with(".json")) {
            continue;
        }
        // A receipt that is a link, or anything but a regular file, is
        // refused by the read below rather than skipped.
        let Some(bytes) = read_file(dir, name, key)? else {
            continue;
        };
        parts.push(serde_json::from_slice(&bytes).map_err(json_error)?);
    }
    parts.sort_by_key(|part: &StoredPart| part.receipt.part_number);
    Ok(parts)
}

/// Opens the regular file `name` in `dir` for reading without following a
/// link; a missing file is an error.
fn open_existing(dir: &Dir, name: &str, key: &str) -> Result<std::fs::File, StorageError> {
    let path = dir.child_path(name);
    dir.open_read(name)
        .map_err(|error| fs_error(error, key, &path))?
        .ok_or_else(|| not_found(&path))
}

/// Size and lowercase hex SHA-256 of an existing file.
fn file_digest(dir: &Dir, name: &str, key: &str) -> Result<(u64, String), StorageError> {
    let path = dir.child_path(name);
    let mut input = open_existing(dir, name, key)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut size = 0_u64;
    loop {
        let read = input.read(&mut buffer).map_err(io(&path))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((size, format!("{:x}", hasher.finalize())))
}

/// Stream a part into the new file `name` in `dir`, hashing it and refusing
/// it once it exceeds `max`.
async fn write_part(
    key: &str,
    dir: Arc<Dir>,
    name: String,
    body: StorageByteStream,
    max: u64,
) -> Result<(u64, String), StorageError> {
    let path = dir.child_path(&name);
    let file = tokio::task::spawn_blocking(move || dir.create_new(&name))
        .await
        .map_err(join_error)?
        .map_err(io(&path))?;
    let mut file = tokio::fs::File::from_std(file);
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
        file.write_all(&chunk).await.map_err(io(&path))?;
    }
    file.sync_all().await.map_err(io(&path))?;
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
        // Refuse a key the completed upload could not be written to.
        self.ensure_internal_dir();
        self.check_write_key(key)?;
        let rel = self.rel_for(key)?;
        check_leaf(&rel, key)?;
        let id = Uuid::new_v4().hyphenated().to_string();
        let manifest = Manifest {
            format_version: MANIFEST_FORMAT,
            key: key.to_string(),
            content_type: options.put.content_type,
            if_not_exists: options.if_not_exists,
            started_at_unix: OffsetDateTime::now_utc().unix_timestamp(),
        };
        let bytes = serde_json::to_vec(&manifest).map_err(json_error)?;
        let owned_key = key.to_string();
        let upload_id = id.clone();
        self.blocking(move |backend| {
            if let Some(target) = backend.locate_blocking(&rel, &owned_key)? {
                target.refuse_link_leaf(&owned_key)?;
            }
            let uploads = backend.prepare_dir_blocking(&UPLOADS, &owned_key)?;
            let path = uploads.child_path(&upload_id);
            if !uploads.create_dir(&upload_id).map_err(io(&path))? {
                return Err(StorageError::io(
                    &path,
                    std::io::ErrorKind::AlreadyExists.into(),
                ));
            }
            sync_dir(&uploads)?;
            let dir = uploads
                .child_dir(&upload_id)
                .map_err(|error| fs_error(error, &owned_key, &path))?
                .ok_or_else(|| not_found(&path))?;
            write_durably(&dir, MANIFEST, &bytes)
        })
        .await?;
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
        let id = self.upload_id(handle)?;
        check_part_number(&handle.key, part_number, self.multipart_limits())?;
        let unknown = || multipart_error(&handle.key, MultipartErrorKind::UnknownUpload);
        let key = handle.key.clone();
        let lookup_id = id.clone();
        let upload = self
            .blocking(move |backend| {
                let Some(upload) = backend.upload_blocking(&lookup_id, &key)? else {
                    return Ok(None);
                };
                let manifest = read_manifest(&upload.dir, &key)?;
                Ok(manifest
                    .filter(|manifest| manifest.key == key)
                    .map(|_| upload))
            })
            .await?
            .ok_or_else(unknown)?;

        // The data file has a unique name, so concurrent uploads of one part
        // number never write the same file; the receipt rename picks one.
        let data_file = format!("part-{part_number:05}.{}.data", Uuid::new_v4());
        let written = write_part(
            &handle.key,
            Arc::clone(&upload.dir),
            data_file.clone(),
            body,
            self.multipart_max_part_size(),
        )
        .await;
        let (size_bytes, sha256) = match written {
            Ok(written) => written,
            Err(error) => {
                let dir = Arc::clone(&upload.dir);
                let name = data_file.clone();
                let _ = tokio::task::spawn_blocking(move || dir.remove_file(&name)).await;
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
        let dir = Arc::clone(&upload.dir);
        locked(upload.uploads, lock_name(&id), key.clone(), move || {
            if read_manifest(&dir, &key)?.is_none() {
                let _ = dir.remove_file(&stored.data_file);
                return Err(multipart_error(&key, MultipartErrorKind::UnknownUpload));
            }
            let receipt_name = part_receipt_name(part_number);
            let previous = read_file(&dir, &receipt_name, &key)
                .ok()
                .flatten()
                .and_then(|bytes| serde_json::from_slice::<StoredPart>(&bytes).ok());
            sync_dir(&dir)?;
            let bytes = serde_json::to_vec(&stored).map_err(json_error)?;
            write_durably(&dir, &receipt_name, &bytes)?;
            if let Some(previous) = previous
                && previous.data_file != stored.data_file
            {
                let _ = dir.remove_file(&previous.data_file);
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
        let id = self.upload_id(handle)?;
        let key = handle.key.clone();
        self.blocking(move |backend| {
            let unknown = || multipart_error(&key, MultipartErrorKind::UnknownUpload);
            let upload = backend.upload_blocking(&id, &key)?.ok_or_else(unknown)?;
            if read_manifest(&upload.dir, &key)?.is_none_or(|manifest| manifest.key != key) {
                return Err(unknown());
            }
            Ok(read_stored_parts(&upload.dir, &key)?
                .into_iter()
                .map(|part| part.receipt)
                .collect())
        })
        .await
    }

    async fn complete_multipart(
        &self,
        handle: &MultipartUploadHandle,
        parts: &[PartReceipt],
    ) -> Result<MultipartCompleteOutcome, StorageError> {
        let id = self.upload_id(handle)?;
        // Completion publishes the object, so the key policy applies here as
        // it does to every other write: an upload started before canonical
        // keys were enabled cannot publish a key they refuse.
        self.check_write_key(&handle.key)?;
        let rel = self.rel_for(&handle.key)?;
        // Refuse a temporary-shaped key before taking the lock or creating any
        // directory.
        check_leaf(&rel, &handle.key)?;
        let key = handle.key.clone();
        let parts = parts.to_vec();
        let limits = self.multipart_limits();
        let handle_condition = handle.if_not_exists;
        let lookup_key = key.clone();
        let uploads = self
            .blocking(move |backend| backend.uploads_blocking(&lookup_key))
            .await?
            .ok_or_else(|| multipart_error(&handle.key, MultipartErrorKind::UnknownUpload))?;
        let backend = self.clone();
        let held = Arc::clone(&uploads);
        let upload_id = id.clone();
        let outcome = locked(
            Arc::clone(&uploads),
            lock_name(&id),
            key.clone(),
            move || {
                let unknown = || multipart_error(&key, MultipartErrorKind::UnknownUpload);
                let dir = held
                    .child_dir(&upload_id)
                    .map_err(|error| fs_error(error, &key, &held.child_path(&upload_id)))?
                    .ok_or_else(unknown)?;
                let manifest = read_manifest(&dir, &key)?
                    .filter(|manifest| manifest.key == key)
                    .ok_or_else(unknown)?;
                let stored = read_stored_parts(&dir, &key)?;
                let receipts = stored
                    .iter()
                    .map(|part| part.receipt.clone())
                    .collect::<Vec<_>>();
                verify_receipts(&key, &receipts, &parts, limits)?;

                let target = backend.prepare_blocking(&rel, &key)?;
                let target_path = target.path();
                let temp = temp_file_name();
                let temp_path = target.dir.child_path(&temp);
                let assembled = (|| {
                    let mut output = target.dir.create_new(&temp).map_err(io(&temp_path))?;
                    let mut hasher = Sha256::new();
                    let mut size_bytes = 0_u64;
                    let mut buffer = vec![0_u8; 1024 * 1024];
                    for part in &stored {
                        let path = dir.child_path(&part.data_file);
                        // Part data is opened like any other read: never through
                        // a link, and never a file another hard link shares.
                        let mut input = open_existing(&dir, &part.data_file, &key)?;
                        let mut part_size = 0_u64;
                        let mut part_hasher = Sha256::new();
                        loop {
                            let read = input.read(&mut buffer).map_err(io(&path))?;
                            if read == 0 {
                                break;
                            }
                            part_hasher.update(&buffer[..read]);
                            hasher.update(&buffer[..read]);
                            output.write_all(&buffer[..read]).map_err(io(&temp_path))?;
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
                    output.sync_all().map_err(io(&temp_path))?;
                    trace(DurabilityStep::FileSynced, &temp_path);
                    Ok(BlobWriteOutcome {
                        size_bytes,
                        sha256_hex: format!("{:x}", hasher.finalize()),
                    })
                })();
                let outcome = match assembled {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        let _ = target.dir.remove_file(&temp);
                        return Err(error);
                    }
                };

                let published = if manifest.if_not_exists || handle_condition {
                    match target.dir.rename_noreplace(&temp, &target.leaf) {
                        Ok(()) => Ok(()),
                        // A completion that crashed after publishing left these
                        // staged parts behind. Its retry finds its own object: that
                        // is success, recognised by an identical size and digest.
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                            if file_digest(&target.dir, &target.leaf, &key)?
                                == (outcome.size_bytes, outcome.sha256_hex.clone())
                            {
                                Ok(())
                            } else {
                                Err(StorageError::PreconditionFailed {
                                    key: key.clone(),
                                    condition: "object already exists".to_string(),
                                })
                            }
                        }
                        Err(source) => Err(crate::local::publish_error(&target_path, source)),
                    }
                } else {
                    target
                        .dir
                        .rename(&temp, &target.leaf)
                        .map_err(io(&target_path))
                };
                // Gone after a rename; left behind after a refused create.
                let _ = target.dir.remove_file(&temp);
                published?;
                trace_published(Some(&temp_path), &target_path);
                sync_dir(&target.dir)?;

                // The object is published; the staged parts are no longer needed.
                drop(dir);
                held.remove_tree(&upload_id)
                    .map_err(io(&held.child_path(&upload_id)))?;
                Ok(outcome)
            },
        )
        .await?;
        let lock = lock_name(&id);
        let _ = tokio::task::spawn_blocking(move || uploads.remove_file(&lock)).await;
        Ok(MultipartCompleteOutcome {
            size_bytes: outcome.size_bytes,
            sha256_hex: Some(outcome.sha256_hex),
        })
    }

    async fn abort_multipart(&self, handle: &MultipartUploadHandle) -> Result<(), StorageError> {
        let id = match self.upload_id(handle) {
            Ok(id) => id,
            // An identifier this backend never issued names no upload.
            Err(StorageError::Multipart {
                kind: MultipartErrorKind::UnknownUpload,
                ..
            }) => return Ok(()),
            Err(error) => return Err(error),
        };
        let key = handle.key.clone();
        let lookup = (id.clone(), key.clone());
        let found = self
            .blocking(move |backend| {
                let Some(uploads) = backend.uploads_blocking(&lookup.1)? else {
                    return Ok(None);
                };
                let present = uploads
                    .info(&lookup.0)
                    .map_err(io(&uploads.child_path(&lookup.0)))?
                    .is_some();
                if !present {
                    let _ = uploads.remove_file(&lock_name(&lookup.0));
                    return Ok(None);
                }
                Ok(Some(uploads))
            })
            .await?;
        let Some(uploads) = found else {
            return Ok(());
        };
        let held = Arc::clone(&uploads);
        let upload_id = id.clone();
        locked(Arc::clone(&uploads), lock_name(&id), key, move || {
            held.remove_tree(&upload_id)
                .map_err(io(&held.child_path(&upload_id)))
        })
        .await?;
        let lock = lock_name(&id);
        let _ = tokio::task::spawn_blocking(move || uploads.remove_file(&lock)).await;
        Ok(())
    }

    async fn list_incomplete_multipart(
        &self,
        prefix: &str,
        continuation: Option<String>,
        limit: usize,
    ) -> Result<IncompleteMultipartPage, StorageError> {
        crate::blob::validate_blob_prefix(prefix)?;
        self.check_root_name(prefix)?;
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
        let prefix = prefix.to_string();
        let mut found = self
            .blocking(move |backend| {
                let mut found = Vec::new();
                let Some(uploads) = backend.uploads_blocking(&prefix)? else {
                    return Ok(found);
                };
                let entries = uploads
                    .entries()
                    .map_err(|source| StorageError::io(uploads.path(), source))?;
                for entry in entries {
                    if entry.kind != Kind::Dir {
                        continue;
                    }
                    let Some(id) = entry
                        .name
                        .to_str()
                        .filter(|name| Uuid::parse_str(name).is_ok() && !name.ends_with(".lock"))
                        .map(str::to_string)
                    else {
                        continue;
                    };
                    let dir = match uploads.child_dir(&id) {
                        Ok(Some(dir)) => dir,
                        Ok(None) => continue,
                        // Replaced by a link: not an upload this backend made.
                        Err(err) if crate::local_fs::is_refusal(&err) => continue,
                        Err(source) => {
                            return Err(StorageError::io(uploads.child_path(&id), source));
                        }
                    };
                    // A directory without a manifest is a start that never
                    // finished; a manifest that is a link is not this
                    // backend's.
                    let manifest = match read_manifest(&dir, &id) {
                        Ok(Some(manifest)) => manifest,
                        Ok(None) | Err(StorageError::InvalidStorageKey { .. }) => continue,
                        Err(error) => return Err(error),
                    };
                    if manifest.key.starts_with(&prefix) && validate_blob_key(&manifest.key).is_ok()
                    {
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
            .await?;
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
