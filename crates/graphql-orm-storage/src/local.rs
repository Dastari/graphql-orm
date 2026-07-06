use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::{
    BlobBody, BlobListPage, BlobMetadata, BlobPutOptions, BlobStore, BlobWriteOutcome,
    BoxedMultipartWriter, MultipartWriter, ObjectContentRange, ObjectInfo, ObjectMetadata,
    ObjectRangeBody, ObjectStorage, StorageBackend, StorageByteStream, StorageError,
    StorageObjectBody, StoredObject, StreamingObjectStore, collect_storage_stream,
    validate_blob_key, validate_object_bucket,
};

const INTERNAL_STORAGE_DIR: &str = ".graphql-orm-storage";
const OBJECT_METADATA_DIR: &str = "object-metadata";

/// Local filesystem object storage backend.
#[derive(Clone, Debug)]
pub struct LocalStorageBackend {
    root: PathBuf,
}

impl LocalStorageBackend {
    /// Creates a local storage backend rooted at the given directory.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Removes stale temporary upload files under the local storage root.
    ///
    /// Files are considered temporary when their filename ends with
    /// `.uploading`. Callers should schedule this periodically if the process
    /// may be interrupted during writes.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the storage root cannot be walked or a
    /// stale temp file cannot be removed.
    pub async fn sweep_temp_files(&self, older_than: Duration) -> Result<usize, StorageError> {
        let now = SystemTime::now();
        let mut removed = 0;
        let mut stack = vec![self.root.clone()];

        while let Some(path) = stack.pop() {
            let metadata = match tokio::fs::metadata(&path).await {
                Ok(metadata) => metadata,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(source) => return Err(StorageError::io(&path, source)),
            };

            if metadata.is_file() {
                if is_uploading_temp_file(&path) && is_older_than(&metadata, now, older_than) {
                    tokio::fs::remove_file(&path)
                        .await
                        .map_err(|source| StorageError::io(&path, source))?;
                    removed += 1;
                }
                continue;
            }

            let entries = sorted_child_paths(&path).await?;
            stack.extend(entries);
        }

        Ok(removed)
    }

    fn path_for(&self, key: &str) -> Result<PathBuf, StorageError> {
        validate_blob_key(key)?;
        Ok(self.root.join(Path::new(key)))
    }

    fn object_path_for(&self, bucket: &str, key: &str) -> Result<PathBuf, StorageError> {
        validate_object_bucket(bucket)?;
        validate_blob_key(key)?;
        Ok(self.root.join(bucket).join(Path::new(key)))
    }

    fn object_metadata_path_for(&self, bucket: &str, key: &str) -> Result<PathBuf, StorageError> {
        validate_object_bucket(bucket)?;
        validate_blob_key(key)?;
        let path = self
            .root
            .join(INTERNAL_STORAGE_DIR)
            .join(OBJECT_METADATA_DIR)
            .join(bucket)
            .join(Path::new(key));
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| StorageError::InvalidStorageKey {
                key: key.to_string(),
            })?;
        Ok(path.with_file_name(format!("{file_name}.metadata.json")))
    }
}

#[async_trait]
impl BlobStore for LocalStorageBackend {
    fn backend(&self) -> StorageBackend {
        StorageBackend::Local
    }

    async fn put_blob(
        &self,
        key: &str,
        body: StorageByteStream,
        _options: BlobPutOptions,
    ) -> Result<BlobWriteOutcome, StorageError> {
        let path = self.path_for(key)?;
        create_parent_dir(&path).await?;
        let temp_path = temp_path_for(&path, key)?;

        let write_result = write_stream_to_temp(&temp_path, body).await;
        let outcome = match write_result {
            Ok(outcome) => outcome,
            Err(err) => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(err);
            }
        };

        if let Err(source) = tokio::fs::rename(&temp_path, &path).await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(StorageError::io(&path, source));
        }

        Ok(outcome)
    }

    async fn put_blob_if_not_exists(
        &self,
        key: &str,
        body: StorageByteStream,
        _options: BlobPutOptions,
    ) -> Result<Option<BlobWriteOutcome>, StorageError> {
        let path = self.path_for(key)?;
        create_parent_dir(&path).await?;
        let temp_path = temp_path_for(&path, key)?;

        let outcome = match write_stream_to_temp(&temp_path, body).await {
            Ok(outcome) => outcome,
            Err(err) => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(err);
            }
        };

        match tokio::fs::hard_link(&temp_path, &path).await {
            Ok(()) => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                Ok(Some(outcome))
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                Ok(None)
            }
            Err(source) => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                Err(StorageError::io(&path, source))
            }
        }
    }

    async fn get_blob(&self, key: &str) -> Result<BlobBody, StorageError> {
        let path = self.path_for(key)?;
        let file = match tokio::fs::File::open(&path).await {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(StorageError::MissingBlob {
                    key: key.to_string(),
                });
            }
            Err(source) => return Err(StorageError::io(&path, source)),
        };
        let metadata = self.head_blob(key).await?;
        let stream_path = path.clone();
        let stream = ReaderStream::new(file)
            .map(move |chunk| chunk.map_err(|source| StorageError::io(&stream_path, source)));

        Ok(BlobBody {
            key: key.to_string(),
            metadata,
            body: StorageByteStream::new(Box::pin(stream)),
        })
    }

    async fn get_blob_range(
        &self,
        key: &str,
        range: std::ops::Range<u64>,
    ) -> Result<BlobBody, StorageError> {
        if range.end < range.start {
            return Err(StorageError::PreconditionFailed {
                key: key.to_string(),
                condition: "range end is before range start".to_string(),
            });
        }

        let path = self.path_for(key)?;
        let mut file = match tokio::fs::File::open(&path).await {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(StorageError::MissingBlob {
                    key: key.to_string(),
                });
            }
            Err(source) => return Err(StorageError::io(&path, source)),
        };
        file.seek(SeekFrom::Start(range.start))
            .await
            .map_err(|source| StorageError::io(&path, source))?;
        let length = range.end - range.start;
        let stream_path = path.clone();
        let stream = ReaderStream::new(file.take(length))
            .map(move |chunk| chunk.map_err(|source| StorageError::io(&stream_path, source)));

        Ok(BlobBody {
            key: key.to_string(),
            metadata: self.head_blob(key).await?,
            body: StorageByteStream::with_size_hint(Box::pin(stream), length),
        })
    }

    async fn blob_exists(&self, key: &str) -> Result<bool, StorageError> {
        let path = self.path_for(key)?;
        match tokio::fs::metadata(&path).await {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(StorageError::io(&path, source)),
        }
    }

    async fn head_blob(&self, key: &str) -> Result<Option<BlobMetadata>, StorageError> {
        let path = self.path_for(key)?;
        match tokio::fs::metadata(&path).await {
            Ok(metadata) => Ok(Some(BlobMetadata {
                key: key.to_string(),
                size_bytes: Some(metadata.len()),
                sha256_hex: None,
                etag: None,
                last_modified: metadata.modified().ok().map(OffsetDateTime::from),
            })),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(StorageError::io(&path, source)),
        }
    }

    async fn list_blobs_page(
        &self,
        prefix: &str,
        continuation: Option<String>,
        limit: usize,
    ) -> Result<BlobListPage, StorageError> {
        if limit == 0 {
            return Err(StorageError::PreconditionFailed {
                key: prefix.to_string(),
                condition: "list limit must be greater than zero".to_string(),
            });
        }

        let mut page_keys = self
            .collect_blob_page(prefix, continuation.as_deref(), limit + 1)
            .await?;
        let next_continuation = if page_keys.len() > limit {
            page_keys.truncate(limit);
            page_keys.last().cloned()
        } else {
            None
        };

        Ok(BlobListPage {
            keys: page_keys,
            next_continuation,
        })
    }

    async fn copy_blob(&self, from: &str, to: &str) -> Result<(), StorageError> {
        let from_path = self.path_for(from)?;
        let to_path = self.path_for(to)?;
        create_parent_dir(&to_path).await?;
        let temp_path = temp_path_for(&to_path, to)?;

        match tokio::fs::copy(&from_path, &temp_path).await {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(StorageError::MissingBlob {
                    key: from.to_string(),
                });
            }
            Err(source) => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(StorageError::io(&from_path, source));
            }
        }

        if let Err(source) = tokio::fs::rename(&temp_path, &to_path).await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(StorageError::io(&to_path, source));
        }

        Ok(())
    }

    async fn delete_blob(&self, key: &str) -> Result<(), StorageError> {
        let path = self.path_for(key)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(StorageError::io(&path, source)),
        }
    }
}

#[async_trait]
impl ObjectStorage for LocalStorageBackend {
    async fn put_object(
        &self,
        object: StoredObject,
        bytes: Vec<u8>,
    ) -> Result<StoredObject, StorageError> {
        self.put_blob(
            &object.storage_key,
            StorageByteStream::from_bytes(bytes),
            BlobPutOptions::default(),
        )
        .await?;
        Ok(object)
    }

    async fn get_object(&self, object: &StoredObject) -> Result<StorageObjectBody, StorageError> {
        let body = self.get_blob(&object.storage_key).await?;
        let bytes = collect_storage_stream(body.body).await?;
        Ok(StorageObjectBody {
            object: object.clone(),
            bytes: bytes.to_vec(),
        })
    }

    async fn delete_object(&self, object: &StoredObject) -> Result<(), StorageError> {
        self.delete_blob(&object.storage_key).await
    }
}

#[async_trait]
impl StreamingObjectStore for LocalStorageBackend {
    async fn put_object_stream(
        &self,
        bucket: &str,
        key: &str,
        content_type: Option<String>,
        metadata: ObjectMetadata,
        stream: StorageByteStream,
    ) -> Result<ObjectInfo, StorageError> {
        let mut writer = self
            .create_multipart_object(bucket, key, content_type, metadata)
            .await?;
        let mut stream = stream.into_inner();

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    if let Err(err) = writer.write_chunk(bytes).await {
                        let _ = writer.abort().await;
                        return Err(err);
                    }
                }
                Err(err) => {
                    let _ = writer.abort().await;
                    return Err(err);
                }
            }
        }

        writer.complete().await
    }

    async fn create_multipart_object(
        &self,
        bucket: &str,
        key: &str,
        content_type: Option<String>,
        metadata: ObjectMetadata,
    ) -> Result<BoxedMultipartWriter, StorageError> {
        let object_path = self.object_path_for(bucket, key)?;
        let metadata_path = self.object_metadata_path_for(bucket, key)?;
        create_parent_dir(&object_path).await?;
        create_parent_dir(&metadata_path).await?;
        let temp_path = temp_path_for(&object_path, key)?;
        let metadata_temp_path = temp_path_for(&metadata_path, key)?;
        let file = tokio::fs::File::create(&temp_path)
            .await
            .map_err(|source| StorageError::io(&temp_path, source))?;

        Ok(Box::new(LocalMultipartWriter {
            bucket: bucket.to_string(),
            key: key.to_string(),
            content_type,
            metadata,
            object_path,
            temp_path,
            metadata_path,
            metadata_temp_path,
            file,
            hasher: Sha256::new(),
            size_bytes: 0,
        }))
    }

    async fn get_object_range(
        &self,
        bucket: &str,
        key: &str,
        range: std::ops::Range<u64>,
    ) -> Result<ObjectRangeBody, StorageError> {
        if range.end < range.start {
            return Err(StorageError::PreconditionFailed {
                key: format!("{bucket}/{key}"),
                condition: "range end is before range start".to_string(),
            });
        }

        let object = self.get_object_metadata(bucket, key).await?;
        if range.start > object.size_bytes {
            return Err(StorageError::PreconditionFailed {
                key: format!("{bucket}/{key}"),
                condition: "range start is beyond object size".to_string(),
            });
        }
        let end = range.end.min(object.size_bytes);
        let length = end.saturating_sub(range.start);
        let object_path = self.object_path_for(bucket, key)?;
        let mut file = match tokio::fs::File::open(&object_path).await {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(StorageError::MissingBlob {
                    key: format!("{bucket}/{key}"),
                });
            }
            Err(source) => return Err(StorageError::io(&object_path, source)),
        };
        file.seek(SeekFrom::Start(range.start))
            .await
            .map_err(|source| StorageError::io(&object_path, source))?;
        let stream_path = object_path.clone();
        let stream = ReaderStream::new(file.take(length))
            .map(move |chunk| chunk.map_err(|source| StorageError::io(&stream_path, source)));

        let total_size = object.size_bytes;
        Ok(ObjectRangeBody {
            object,
            range: ObjectContentRange {
                start: range.start,
                end,
                total_size,
            },
            content_length: length,
            body: StorageByteStream::with_size_hint(Box::pin(stream), length),
        })
    }

    async fn get_object_metadata(
        &self,
        bucket: &str,
        key: &str,
    ) -> Result<ObjectInfo, StorageError> {
        let metadata_path = self.object_metadata_path_for(bucket, key)?;
        let bytes = match tokio::fs::read(&metadata_path).await {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(StorageError::MissingBlob {
                    key: format!("{bucket}/{key}"),
                });
            }
            Err(source) => return Err(StorageError::io(&metadata_path, source)),
        };
        serde_json::from_slice(&bytes).map_err(|source| StorageError::Provider {
            backend: StorageBackend::Local.as_str().to_string(),
            message: format!("local object metadata is invalid: {source}"),
            retryable: false,
        })
    }

    async fn list_objects(
        &self,
        bucket: &str,
        prefix: &str,
    ) -> Result<Vec<ObjectInfo>, StorageError> {
        validate_object_bucket(bucket)?;
        crate::blob::validate_blob_prefix(prefix)?;
        let start = if prefix.is_empty() {
            self.root.join(bucket)
        } else {
            self.root.join(bucket).join(prefix)
        };
        let mut objects = Vec::new();
        let mut stack = vec![start];

        while let Some(path) = stack.pop() {
            let metadata = match tokio::fs::metadata(&path).await {
                Ok(metadata) => metadata,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(source) => return Err(StorageError::io(&path, source)),
            };

            if metadata.is_file() {
                if is_uploading_temp_file(&path) {
                    continue;
                }
                let Some(key) = self.key_for_bucket_path(bucket, &path) else {
                    continue;
                };
                if !key.starts_with(prefix) {
                    continue;
                }
                if let Ok(info) = self.get_object_metadata(bucket, &key).await {
                    objects.push(info);
                }
                continue;
            }

            let entries = sorted_child_paths(&path).await?;
            stack.extend(entries.into_iter().rev());
        }

        objects.sort_by(|left, right| left.key.cmp(&right.key));
        Ok(objects)
    }

    async fn delete_object(&self, bucket: &str, key: &str) -> Result<(), StorageError> {
        let object_path = self.object_path_for(bucket, key)?;
        let metadata_path = self.object_metadata_path_for(bucket, key)?;

        match tokio::fs::remove_file(&object_path).await {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(StorageError::io(&object_path, source)),
        }
        match tokio::fs::remove_file(&metadata_path).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(StorageError::io(&metadata_path, source)),
        }
    }
}

struct LocalMultipartWriter {
    bucket: String,
    key: String,
    content_type: Option<String>,
    metadata: ObjectMetadata,
    object_path: PathBuf,
    temp_path: PathBuf,
    metadata_path: PathBuf,
    metadata_temp_path: PathBuf,
    file: tokio::fs::File,
    hasher: Sha256,
    size_bytes: u64,
}

#[async_trait]
impl MultipartWriter for LocalMultipartWriter {
    async fn write_chunk(&mut self, bytes: Bytes) -> Result<(), StorageError> {
        self.size_bytes = self
            .size_bytes
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        self.hasher.update(&bytes);
        self.file
            .write_all(&bytes)
            .await
            .map_err(|source| StorageError::io(&self.temp_path, source))
    }

    async fn complete(mut self: Box<Self>) -> Result<ObjectInfo, StorageError> {
        self.file
            .flush()
            .await
            .map_err(|source| StorageError::io(&self.temp_path, source))?;

        let sha256_hex = format!("{:x}", self.hasher.finalize());
        let object = ObjectInfo {
            bucket: self.bucket.clone(),
            key: self.key.clone(),
            content_type: self.content_type.clone(),
            metadata: self.metadata.clone(),
            size_bytes: self.size_bytes,
            sha256_hex: sha256_hex.clone(),
            etag: Some(sha256_hex),
            last_modified: OffsetDateTime::now_utc(),
        };

        write_object_metadata(&self.metadata_temp_path, &object).await?;

        if let Err(source) = tokio::fs::rename(&self.temp_path, &self.object_path).await {
            let _ = tokio::fs::remove_file(&self.temp_path).await;
            let _ = tokio::fs::remove_file(&self.metadata_temp_path).await;
            return Err(StorageError::io(&self.object_path, source));
        }
        if let Err(source) = tokio::fs::rename(&self.metadata_temp_path, &self.metadata_path).await
        {
            let _ = tokio::fs::remove_file(&self.metadata_temp_path).await;
            return Err(StorageError::io(&self.metadata_path, source));
        }

        Ok(object)
    }

    async fn abort(self: Box<Self>) -> Result<(), StorageError> {
        drop(self.file);
        let temp_result = tokio::fs::remove_file(&self.temp_path).await;
        let metadata_result = tokio::fs::remove_file(&self.metadata_temp_path).await;

        match temp_result {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(StorageError::io(&self.temp_path, source)),
        }
        match metadata_result {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(StorageError::io(&self.metadata_temp_path, source)),
        }
    }
}

impl LocalStorageBackend {
    async fn collect_blob_page(
        &self,
        prefix: &str,
        continuation: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>, StorageError> {
        crate::blob::validate_blob_prefix(prefix)?;

        let start = if prefix.is_empty() {
            self.root.clone()
        } else {
            self.root.join(prefix)
        };

        let mut page = Vec::with_capacity(limit);
        let mut stack = vec![start];

        while let Some(path) = stack.pop() {
            if page.len() == limit {
                break;
            }
            if is_internal_storage_path(&self.root, &path) {
                continue;
            }

            let metadata = match tokio::fs::metadata(&path).await {
                Ok(metadata) => metadata,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(source) => return Err(StorageError::io(&path, source)),
            };

            if metadata.is_file() {
                if is_uploading_temp_file(&path) {
                    continue;
                }
                if let Some(key) = self.key_for_path(&path) {
                    if continuation.is_some_and(|token| key.as_str() <= token) {
                        continue;
                    }
                    page.push(key);
                }
                continue;
            }

            let entries = sorted_child_paths(&path).await?;
            stack.extend(entries.into_iter().rev());
        }

        Ok(page)
    }

    fn key_for_path(&self, path: &Path) -> Option<String> {
        path.strip_prefix(&self.root)
            .ok()
            .map(|relative| relative.to_string_lossy().replace('\\', "/"))
    }

    fn key_for_bucket_path(&self, bucket: &str, path: &Path) -> Option<String> {
        path.strip_prefix(self.root.join(bucket))
            .ok()
            .map(|relative| relative.to_string_lossy().replace('\\', "/"))
    }
}

async fn create_parent_dir(path: &Path) -> Result<(), StorageError> {
    let parent = path
        .parent()
        .ok_or_else(|| StorageError::MissingParent { path: path.into() })?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|source| StorageError::io(parent, source))
}

fn temp_path_for(path: &Path, key: &str) -> Result<PathBuf, StorageError> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| StorageError::InvalidStorageKey {
            key: key.to_string(),
        })?;
    Ok(path.with_file_name(format!("{file_name}.{}.uploading", Uuid::new_v4())))
}

async fn write_stream_to_temp(
    temp_path: &Path,
    body: StorageByteStream,
) -> Result<BlobWriteOutcome, StorageError> {
    let mut file = tokio::fs::File::create(temp_path)
        .await
        .map_err(|source| StorageError::io(temp_path, source))?;
    let mut stream = body.into_inner();
    let mut hasher = Sha256::new();
    let mut size_bytes = 0_u64;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        size_bytes = size_bytes.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|source| StorageError::io(temp_path, source))?;
    }

    file.flush()
        .await
        .map_err(|source| StorageError::io(temp_path, source))?;

    Ok(BlobWriteOutcome {
        size_bytes,
        sha256_hex: format!("{:x}", hasher.finalize()),
    })
}

async fn write_object_metadata(path: &Path, object: &ObjectInfo) -> Result<(), StorageError> {
    let bytes = serde_json::to_vec(object).map_err(|source| StorageError::Provider {
        backend: StorageBackend::Local.as_str().to_string(),
        message: format!("local object metadata could not be serialized: {source}"),
        retryable: false,
    })?;
    tokio::fs::write(path, bytes)
        .await
        .map_err(|source| StorageError::io(path, source))
}

fn is_uploading_temp_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".uploading"))
}

fn is_internal_storage_path(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root)
        .ok()
        .and_then(|relative| relative.components().next())
        .is_some_and(|component| component.as_os_str() == INTERNAL_STORAGE_DIR)
}

fn is_older_than(metadata: &std::fs::Metadata, now: SystemTime, older_than: Duration) -> bool {
    metadata
        .modified()
        .ok()
        .and_then(|modified| now.duration_since(modified).ok())
        .is_some_and(|age| age >= older_than)
}

async fn sorted_child_paths(path: &Path) -> Result<Vec<PathBuf>, StorageError> {
    let mut entries = tokio::fs::read_dir(path)
        .await
        .map_err(|source| StorageError::io(path, source))?;
    let mut paths = Vec::new();

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|source| StorageError::io(path, source))?
    {
        paths.push(entry.path());
    }

    paths.sort();
    Ok(paths)
}
