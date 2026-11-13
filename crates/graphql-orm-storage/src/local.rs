use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tokio_util::io::ReaderStream;

use crate::{
    BlobBody, BlobListPage, BlobMetadata, BlobPutOptions, BlobStore, BlobWriteOutcome,
    BoxedMultipartWriter, MultipartWriter, ObjectContentRange, ObjectInfo, ObjectMetadata,
    ObjectRangeBody, ObjectStorage, StorageBackend, StorageByteStream, StorageError,
    StorageObjectBody, StoredObject, StreamingObjectStore, collect_storage_stream,
    key::{is_temp_file_name, temp_file_name},
    validate_blob_key, validate_object_bucket,
};

/// Internal staging and metadata directory at the root. Caller keys cannot
/// address it: [`validate_blob_key`] reserves its name, and
/// [`LocalStorageBackend`] refuses any other name that resolves to it.
pub(crate) const INTERNAL_STORAGE_DIR: &str = crate::blob::INTERNAL_NAMESPACE;
const OBJECT_METADATA_DIR: &str = "object-metadata";
/// Most directories a backend remembers as durable before it forgets them all.
const MAX_DURABLE_DIRS: usize = 4_096;

/// Local filesystem object storage backend.
///
/// The root tree must be exclusively managed by the trusted storage deployment.
/// Filesystem permissions must protect its entries and ancestor path components
/// against replacement by untrusted local actors.
/// Validated keys do not confine access against symlinks or concurrent local path
/// replacement. Ordinary reads and writes follow existing filesystem links.
#[derive(Clone, Debug)]
pub struct LocalStorageBackend {
    root: PathBuf,
    multipart_max_part_size: u64,
    /// Directories below the root whose entries, and every ancestor's entry up
    /// to the root, this backend (or a clone) has synced.
    durable_dirs: Arc<Mutex<HashSet<PathBuf>>>,
}

impl LocalStorageBackend {
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Creates a local storage backend rooted at the given directory.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            multipart_max_part_size: crate::local_multipart::DEFAULT_MAX_PART_SIZE,
            durable_dirs: Arc::default(),
        }
    }

    /// Sets the largest accepted multipart part, at most 5 GiB (the default).
    #[must_use]
    pub fn with_multipart_max_part_size(mut self, bytes: u64) -> Self {
        self.multipart_max_part_size =
            bytes.clamp(1, crate::local_multipart::DEFAULT_MAX_PART_SIZE);
        self
    }

    pub(crate) fn multipart_max_part_size(&self) -> u64 {
        self.multipart_max_part_size
    }

    /// Removes stale temporary upload files under the local storage root.
    ///
    /// Only files named exactly `.<uuid>.uploading`, the form this backend
    /// writes, are considered temporary; a file whose name merely ends with
    /// `.uploading` is a stored object and is never removed. Temporary files
    /// left by versions before 0.9.1, named `<name>.<uuid>.uploading`, cannot
    /// be told apart from stored objects and are not removed either; see the
    /// migration guide. Callers should schedule this periodically if the process
    /// may be interrupted during writes. Symbolic links (including directory
    /// links and cycles) are skipped. This does not protect against a concurrent
    /// local actor replacing a path after inspection; the trusted-root contract
    /// on [`LocalStorageBackend`] still applies.
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
            let metadata = match tokio::fs::symlink_metadata(&path).await {
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

            if metadata.is_dir() {
                let entries = sorted_child_paths(&path).await?;
                stack.extend(entries);
            }
        }

        Ok(removed)
    }

    pub(crate) fn path_for(&self, key: &str) -> Result<PathBuf, StorageError> {
        validate_blob_key(key)?;
        self.check_root_name(key)?;
        Ok(self.root.join(Path::new(key)))
    }

    fn object_path_for(&self, bucket: &str, key: &str) -> Result<PathBuf, StorageError> {
        validate_object_bucket(bucket)?;
        validate_blob_key(key)?;
        self.check_root_name(bucket)?;
        refuse_colon(key)?;
        Ok(self.root.join(bucket).join(Path::new(key)))
    }

    fn object_metadata_path_for(&self, bucket: &str, key: &str) -> Result<PathBuf, StorageError> {
        validate_object_bucket(bucket)?;
        validate_blob_key(key)?;
        self.check_root_name(bucket)?;
        refuse_colon(key)?;
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

impl LocalStorageBackend {
    fn internal_dir(&self) -> PathBuf {
        self.root.join(INTERNAL_STORAGE_DIR)
    }

    /// Creates the internal directory if it is missing. Writes call this before
    /// they check names, so a name that a case-insensitive or aliasing
    /// filesystem resolves to the internal directory can never claim it first:
    /// it resolves to the existing directory and [`Self::check_root_name`]
    /// refuses it. Best effort; a write that cannot create it fails on its own.
    pub(crate) fn ensure_internal_dir(&self) {
        let _ = std::fs::create_dir_all(self.internal_dir());
    }

    /// Whether `path` resolves to the internal directory, whatever its
    /// spelling: another case, a Unicode case fold, a Windows short name or a
    /// link. A path that does not exist or cannot be opened is not an alias;
    /// the operation on it fails or creates an ordinary entry.
    fn aliases_internal_dir(&self, path: &Path) -> bool {
        same_file::is_same_file(path, self.internal_dir()).unwrap_or(false)
    }

    /// Local checks for a caller's root-relative key, listing prefix or bucket,
    /// after the shared lexical validation: on Windows no component contains
    /// `:`, which names an alternate data stream there, and the first
    /// component does not resolve to the internal directory.
    pub(crate) fn check_root_name(&self, name: &str) -> Result<(), StorageError> {
        refuse_colon(name)?;
        let first = name.split('/').next().unwrap_or_default();
        if !first.is_empty() && self.aliases_internal_dir(&self.root.join(first)) {
            return Err(StorageError::InvalidStorageKey {
                key: name.to_string(),
            });
        }
        Ok(())
    }
}

/// On Windows, refuses a local key or prefix containing `:`, which separates
/// an alternate data stream (`name::$INDEX_ALLOCATION` aliases a directory).
/// Elsewhere `:` is an ordinary filename character.
fn refuse_colon(name: &str) -> Result<(), StorageError> {
    if cfg!(windows) && name.contains(':') {
        return Err(StorageError::InvalidStorageKey {
            key: name.to_string(),
        });
    }
    Ok(())
}

#[async_trait]
impl BlobStore for LocalStorageBackend {
    async fn delete_blob_once(&self, key: &str) -> Result<(), StorageError> {
        // One filesystem removal, with no retry or background path replay.
        self.delete_blob(key)
            .await
            .map_err(StorageError::single_attempt)
    }

    fn backend(&self) -> StorageBackend {
        StorageBackend::Local
    }

    fn as_multipart(&self) -> Option<&dyn crate::MultipartBlobStore> {
        Some(self)
    }

    async fn put_blob(
        &self,
        key: &str,
        body: StorageByteStream,
        _options: BlobPutOptions,
    ) -> Result<BlobWriteOutcome, StorageError> {
        self.ensure_internal_dir();
        let path = self.path_for(key)?;
        let temp = TempFile::new(temp_path_for(&path, key)?);
        self.create_parent_dir(&path).await?;

        let outcome = write_stream_to_temp(&temp, body).await?;
        if let Err(source) = tokio::fs::rename(temp.path(), &path).await {
            return Err(StorageError::io(&path, source));
        }
        trace_published(Some(temp.path()), &path);
        temp.published();
        sync_entry(&path).await?;

        Ok(outcome)
    }

    async fn put_blob_if_not_exists(
        &self,
        key: &str,
        body: StorageByteStream,
        _options: BlobPutOptions,
    ) -> Result<Option<BlobWriteOutcome>, StorageError> {
        self.ensure_internal_dir();
        let path = self.path_for(key)?;
        let temp = TempFile::new(temp_path_for(&path, key)?);
        self.create_parent_dir(&path).await?;

        let outcome = write_stream_to_temp(&temp, body).await?;
        match tokio::fs::hard_link(temp.path(), &path).await {
            Ok(()) => {
                trace_published(Some(temp.path()), &path);
                temp.remove().await;
                // One directory sync covers the new link and the temp removal.
                // The key now exists, so a retry would find it taken: a failed
                // sync reports a non-retryable, outcome-unknown error.
                sync_entry(&path)
                    .await
                    .map_err(StorageError::single_attempt)?;
                Ok(Some(outcome))
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                temp.remove().await;
                // Make the existing entry this call observed durable too.
                sync_entry(&path).await?;
                Ok(None)
            }
            Err(source) => {
                temp.remove().await;
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
        self.ensure_internal_dir();
        let from_path = self.path_for(from)?;
        let to_path = self.path_for(to)?;
        let temp = TempFile::new(temp_path_for(&to_path, to)?);
        self.create_parent_dir(&to_path).await?;

        let source = from_path.clone();
        match temp
            .run_blocking(move |temp_path| std::fs::copy(&source, temp_path))
            .await
        {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(StorageError::MissingBlob {
                    key: from.to_string(),
                });
            }
            Err(source) => return Err(StorageError::io(&from_path, source)),
        }
        sync_written_file(temp.path()).await?;

        if let Err(source) = tokio::fs::rename(temp.path(), &to_path).await {
            return Err(StorageError::io(&to_path, source));
        }
        trace_published(Some(temp.path()), &to_path);
        temp.published();
        sync_entry(&to_path).await
    }

    async fn delete_blob(&self, key: &str) -> Result<(), StorageError> {
        let path = self.path_for(key)?;
        remove_durably(&path).await
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
        self.ensure_internal_dir();
        let object_path = self.object_path_for(bucket, key)?;
        let metadata_path = self.object_metadata_path_for(bucket, key)?;
        let temp = TempFile::new(temp_path_for(&object_path, key)?);
        let metadata_temp = TempFile::new(temp_path_for(&metadata_path, key)?);
        self.create_parent_dir(&object_path).await?;
        self.create_parent_dir(&metadata_path).await?;
        let file = temp.create().await?;

        Ok(Box::new(LocalMultipartWriter {
            bucket: bucket.to_string(),
            key: key.to_string(),
            content_type,
            metadata,
            object_path,
            temp,
            metadata_path,
            metadata_temp,
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
        self.check_root_name(bucket)?;
        refuse_colon(prefix)?;
        let base = self.root.join(bucket);
        let mut objects = Vec::new();
        let mut stack = vec![listing_start(&base, prefix)];

        while let Some(path) = stack.pop() {
            let Some(key) = listed_key(&base, &path) else {
                continue;
            };
            let metadata = match tokio::fs::metadata(&path).await {
                Ok(metadata) => metadata,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(source) => return Err(StorageError::io(&path, source)),
            };

            if metadata.is_file() {
                if is_uploading_temp_file(&path) || !key.starts_with(prefix) {
                    continue;
                }
                if let Ok(info) = self.get_object_metadata(bucket, &key).await {
                    objects.push(info);
                }
                continue;
            }
            if !directory_may_match(&key, prefix) {
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

        remove_durably(&object_path).await?;
        remove_durably(&metadata_path).await
    }
}

struct LocalMultipartWriter {
    bucket: String,
    key: String,
    content_type: Option<String>,
    metadata: ObjectMetadata,
    object_path: PathBuf,
    /// Removes the staged object if the writer is dropped unfinished.
    temp: TempFile,
    metadata_path: PathBuf,
    /// Removes the staged metadata if the writer is dropped unfinished.
    metadata_temp: TempFile,
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
            .map_err(|source| StorageError::io(self.temp.path(), source))
    }

    async fn complete(mut self: Box<Self>) -> Result<ObjectInfo, StorageError> {
        self.file
            .flush()
            .await
            .map_err(|source| StorageError::io(self.temp.path(), source))?;
        sync_file(&self.file, self.temp.path()).await?;

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

        write_object_metadata(&self.metadata_temp, &object).await?;

        // A failure or cancellation before each rename removes the staged
        // files that remain.
        if let Err(source) = tokio::fs::rename(self.temp.path(), &self.object_path).await {
            return Err(StorageError::io(&self.object_path, source));
        }
        self.temp.mark_published();
        trace_published(Some(self.temp.path()), &self.object_path);
        if let Err(source) = tokio::fs::rename(self.metadata_temp.path(), &self.metadata_path).await
        {
            return Err(StorageError::io(&self.metadata_path, source));
        }
        self.metadata_temp.mark_published();
        trace_published(Some(self.metadata_temp.path()), &self.metadata_path);
        sync_entry(&self.object_path).await?;
        sync_entry(&self.metadata_path).await?;

        Ok(object)
    }

    async fn abort(self: Box<Self>) -> Result<(), StorageError> {
        let writer = *self;
        drop(writer.file);
        let temp_result = tokio::fs::remove_file(writer.temp.path()).await;
        let metadata_result = tokio::fs::remove_file(writer.metadata_temp.path()).await;

        match temp_result {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(StorageError::io(writer.temp.path(), source)),
        }
        match metadata_result {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(StorageError::io(writer.metadata_temp.path(), source)),
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
        self.check_root_name(prefix)?;
        let internal = same_file::Handle::from_path(self.internal_dir()).ok();

        let mut page = Vec::with_capacity(limit);
        let mut stack = vec![listing_start(&self.root, prefix)];

        while let Some(path) = stack.pop() {
            if page.len() == limit {
                break;
            }
            // A root entry that resolves to the internal directory under
            // another name is never entered.
            if internal.is_some()
                && path.parent() == Some(self.root.as_path())
                && same_file::Handle::from_path(&path).ok() == internal
            {
                continue;
            }
            // Skips the reserved internal namespace and any name that is not
            // a valid key, with everything below it.
            let Some(key) = listed_key(&self.root, &path) else {
                continue;
            };

            let metadata = match tokio::fs::metadata(&path).await {
                Ok(metadata) => metadata,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(source) => return Err(StorageError::io(&path, source)),
            };

            if metadata.is_file() {
                if is_uploading_temp_file(&path)
                    || !key.starts_with(prefix)
                    || continuation.is_some_and(|token| key.as_str() <= token)
                {
                    continue;
                }
                page.push(key);
                continue;
            }
            if !directory_may_match(&key, prefix) {
                continue;
            }

            // Every key below this directory starts with `<directory>/`. When
            // that prefix sorts before the token and does not contain it, the
            // whole subtree precedes the continuation.
            if let Some(token) = continuation {
                let directory = format!("{key}/");
                if directory.len() > 1
                    && directory.as_str() < token
                    && !token.starts_with(&directory)
                {
                    continue;
                }
            }

            let entries = sorted_child_paths(&path).await?;
            stack.extend(entries.into_iter().rev());
        }

        Ok(page)
    }
}

/// The directory a listing for `prefix` starts from: the prefix up to its
/// last `/`, below `base`. Every key the prefix matches lies below it.
fn listing_start(base: &Path, prefix: &str) -> PathBuf {
    match prefix.rfind('/') {
        Some(index) => base.join(&prefix[..index]),
        None => base.to_path_buf(),
    }
}

/// The key `path` has below `base`, or `None` when no valid key names it: a
/// name that is not valid UTF-8, contains `:` on Windows, or that
/// [`validate_blob_key`] refuses, such as the reserved internal namespace. Every key below such a directory
/// would contain the same component, so its whole subtree is skipped. `base`
/// itself has the empty key.
fn listed_key(base: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(base).ok()?;
    let mut key = String::new();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return None;
        };
        if !key.is_empty() {
            key.push('/');
        }
        let name = name.to_str()?;
        if cfg!(windows) && name.contains(':') {
            return None;
        }
        key.push_str(name);
    }
    (key.is_empty() || validate_blob_key(&key).is_ok()).then_some(key)
}

/// Whether keys below the directory `key` may start with `prefix`.
fn directory_may_match(key: &str, prefix: &str) -> bool {
    if key.is_empty() {
        return true;
    }
    let directory = format!("{key}/");
    directory.starts_with(prefix) || prefix.starts_with(&directory)
}

impl LocalStorageBackend {
    /// Creates the directory that will hold `path` and makes it durable; see
    /// [`Self::create_dir_durably`].
    async fn create_parent_dir(&self, path: &Path) -> Result<(), StorageError> {
        let parent = path
            .parent()
            .ok_or_else(|| StorageError::MissingParent { path: path.into() })?
            .to_path_buf();
        let backend = self.clone();
        tokio::task::spawn_blocking(move || backend.create_dir_durably(&parent))
            .await
            .map_err(crate::local_multipart::join_error)?
    }

    /// Creates `dir` and makes its entry, and each ancestor's entry up to the
    /// root, durable before returning.
    ///
    /// Each directory from `dir` upward has its entry synced in its parent
    /// until the walk reaches the root or a directory this backend remembers
    /// as durable. A directory that existed is synced too, because another
    /// writer may have created it and not synced it yet. A directory found
    /// missing is forgotten before it is created, so a concurrent call cannot
    /// trust it before this call has synced its entry. The root's own entry is
    /// synced only when this call created the root. Directories are remembered
    /// only after their whole chain synced, so a failed sync is retried by the
    /// next write.
    ///
    /// The memory assumes nothing outside this backend removes directories
    /// under the root while it runs, as the trusted-root contract requires: a
    /// remembered directory removed and recreated elsewhere, and still present
    /// when checked, remains trusted.
    pub(crate) fn create_dir_durably(&self, dir: &Path) -> Result<(), StorageError> {
        let mut missing = Vec::new();
        let mut ancestor = dir;
        loop {
            match std::fs::metadata(ancestor) {
                Ok(_) => break,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    missing.push(ancestor.to_path_buf());
                    match ancestor.parent() {
                        Some(next) if ancestor != self.root && !next.as_os_str().is_empty() => {
                            ancestor = next;
                        }
                        _ => break,
                    }
                }
                Err(source) => return Err(StorageError::io(ancestor, source)),
            }
        }
        if missing.is_empty() && (dir == self.root || self.is_durable_dir(dir)) {
            return Ok(());
        }
        if !missing.is_empty() {
            // Forget the missing directories before recreating them, so that a
            // concurrent call that finds them present syncs their entries too.
            let mut durable = self
                .durable_dirs
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            for directory in &missing {
                durable.remove(directory);
            }
        }

        std::fs::create_dir_all(dir).map_err(|source| StorageError::io(dir, source))?;

        let mut synced = Vec::new();
        let mut current = dir;
        loop {
            let created = missing.iter().any(|path| path == current);
            if current == self.root || !current.starts_with(&self.root) {
                if created {
                    sync_dir_blocking(entry_dir(current)?)?;
                }
                break;
            }
            if !created && self.is_durable_dir(current) {
                break;
            }
            sync_dir_blocking(entry_dir(current)?)?;
            synced.push(current.to_path_buf());
            match current.parent() {
                Some(parent) => current = parent,
                None => break,
            }
        }

        let mut durable = self
            .durable_dirs
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if durable.len().saturating_add(synced.len()) > MAX_DURABLE_DIRS {
            durable.clear();
        }
        durable.extend(synced);
        Ok(())
    }

    fn is_durable_dir(&self, dir: &Path) -> bool {
        self.durable_dirs
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .contains(dir)
    }
}

/// Derives the unique temporary path a write fills before renaming it over
/// `path`: `.<uuid>.uploading` beside the target.
///
/// A target whose name is itself a temporary name is refused, so that listing
/// and [`LocalStorageBackend::sweep_temp_files`] never mistake a caller's
/// object for an interrupted upload.
/// A temporary file a write owns until it publishes it. Dropping the guard
/// before then, as a failed write or a cancelled or dropped write future does,
/// removes the file, so an abandoned write leaves nothing behind.
pub(crate) struct TempFile {
    path: PathBuf,
    /// Set once the file is renamed into place or removed on purpose.
    released: bool,
    /// Set when the guard is dropped while it still owns the file; a blocking
    /// operation still running then removes what it created.
    abandoned: Arc<AtomicBool>,
}

impl TempFile {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            released: false,
            abandoned: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Runs `operation`, which may create the file, on the blocking pool. A
    /// blocking task cannot be cancelled, so if this guard is dropped while
    /// it runs, the task removes the file once `operation` returns.
    pub(crate) async fn run_blocking<T, F>(&self, operation: F) -> std::io::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Path) -> std::io::Result<T> + Send + 'static,
    {
        let path = self.path.clone();
        let abandoned = Arc::clone(&self.abandoned);
        tokio::task::spawn_blocking(move || {
            let result = operation(&path);
            // Pairs with the store in `Drop`: either the guard sees this
            // file and removes it, or this task sees the guard gone.
            if abandoned.load(Ordering::SeqCst) {
                let _ = std::fs::remove_file(&path);
            }
            result
        })
        .await
        .map_err(std::io::Error::other)?
    }

    /// Creates the file for writing.
    pub(crate) async fn create(&self) -> Result<tokio::fs::File, StorageError> {
        self.run_blocking(|path| std::fs::File::create(path))
            .await
            .map(tokio::fs::File::from_std)
            .map_err(|source| StorageError::io(&self.path, source))
    }

    /// Records that the file was renamed into place, so dropping the guard
    /// leaves the published path alone.
    pub(crate) fn mark_published(&mut self) {
        self.released = true;
    }

    /// Consumes the guard after the file was renamed into place.
    pub(crate) fn published(mut self) {
        self.mark_published();
    }

    /// Removes the file, ignoring a failure. If this is cancelled, dropping
    /// the guard still removes it.
    pub(crate) async fn remove(mut self) {
        let _ = tokio::fs::remove_file(&self.path).await;
        self.released = true;
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.abandoned.store(true, Ordering::SeqCst);
        // Blocking on purpose: `Drop` cannot await, and a spawned removal
        // may never run if the runtime is shutting down. It is one unlink of a
        // unique name; a write still in flight keeps its descriptor and
        // finishes harmlessly.
        let _ = std::fs::remove_file(&self.path);
    }
}

pub(crate) fn temp_path_for(path: &Path, key: &str) -> Result<PathBuf, StorageError> {
    let invalid = || StorageError::InvalidStorageKey {
        key: key.to_string(),
    };
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(invalid)?;
    if is_temp_file_name(file_name) {
        return Err(invalid());
    }
    Ok(path.with_file_name(temp_file_name()))
}

async fn write_stream_to_temp(
    temp: &TempFile,
    body: StorageByteStream,
) -> Result<BlobWriteOutcome, StorageError> {
    let temp_path = temp.path();
    let mut file = temp.create().await?;
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
    sync_file(&file, temp_path).await?;

    Ok(BlobWriteOutcome {
        size_bytes,
        sha256_hex: format!("{:x}", hasher.finalize()),
    })
}

async fn write_object_metadata(temp: &TempFile, object: &ObjectInfo) -> Result<(), StorageError> {
    let path = temp.path();
    let bytes = serde_json::to_vec(object).map_err(|source| StorageError::Provider {
        backend: StorageBackend::Local.as_str().to_string(),
        message: format!("local object metadata could not be serialized: {source}"),
        retryable: false,
    })?;
    let mut file = temp.create().await?;
    file.write_all(&bytes)
        .await
        .map_err(|source| StorageError::io(path, source))?;
    file.flush()
        .await
        .map_err(|source| StorageError::io(path, source))?;
    sync_file(&file, path).await
}

/// Whether `path` names a temporary upload file this backend writes. Only the
/// exact `.<uuid>.uploading` form qualifies; any other name, including one
/// that merely ends in `.uploading`, is a caller's object.
fn is_uploading_temp_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(is_temp_file_name)
}

fn is_older_than(metadata: &std::fs::Metadata, now: SystemTime, older_than: Duration) -> bool {
    metadata
        .modified()
        .ok()
        .and_then(|modified| now.duration_since(modified).ok())
        .is_some_and(|age| age >= older_than)
}

/// Lists the entries of the directory `path` in blob-key order.
///
/// Every key below a child directory `name` starts with `name/`, so a
/// directory sorts as `name/` and a file as `name`, compared bytewise. A
/// depth-first walk that visits children in this order yields keys in exactly
/// the lexicographic order that listing continuations assume, including for
/// siblings such as `a-b` or `a.b` whose bytes sort below `/`. Directory links
/// sort as the directories they resolve to, as the walks follow them.
async fn sorted_child_paths(path: &Path) -> Result<Vec<PathBuf>, StorageError> {
    let mut entries = tokio::fs::read_dir(path)
        .await
        .map_err(|source| StorageError::io(path, source))?;
    let mut children = Vec::new();

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|source| StorageError::io(path, source))?
    {
        let child = entry.path();
        // An entry removed since the directory was read is no longer listed.
        let file_type = match entry.file_type().await {
            Ok(file_type) => file_type,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => return Err(StorageError::io(&child, source)),
        };
        let is_dir = if file_type.is_symlink() {
            tokio::fs::metadata(&child)
                .await
                .is_ok_and(|metadata| metadata.is_dir())
        } else {
            file_type.is_dir()
        };
        let mut order = entry.file_name().into_encoded_bytes();
        if is_dir {
            order.push(b'/');
        }
        children.push((order, child));
    }

    children.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    Ok(children.into_iter().map(|(_, child)| child).collect())
}

/// Flushes a written file's data and metadata to stable storage before it is
/// published under its final name.
async fn sync_file(file: &tokio::fs::File, path: &Path) -> Result<(), StorageError> {
    file.sync_all()
        .await
        .map_err(|source| StorageError::io(path, source))?;
    trace(DurabilityStep::FileSynced, path);
    Ok(())
}

/// Makes a creation, rename, link or removal of an entry in `dir` durable.
async fn sync_dir(dir: &Path) -> Result<(), StorageError> {
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || sync_dir_blocking(&dir))
        .await
        .map_err(crate::local_multipart::join_error)?
}

/// Makes a creation, rename, link or removal of an entry in `dir` durable.
///
/// Directory entries are synced on Unix only. Other platforms do not expose a
/// directory sync through the standard library and rely on the file system to
/// persist the entry. A file system that cannot sync directories, and reports
/// `EINVAL` or `EBADF` from the directory fsync, is accepted as is, the way
/// PostgreSQL treats it; any other error, including failing to open the
/// directory, fails the operation.
pub(crate) fn sync_dir_blocking(dir: &Path) -> Result<(), StorageError> {
    #[cfg(unix)]
    {
        #[cfg(test)]
        if durability_log::take_injected_failure(dir) {
            return Err(StorageError::io(dir, std::io::Error::from_raw_os_error(5)));
        }
        let handle = std::fs::File::open(dir).map_err(|source| StorageError::io(dir, source))?;
        match handle.sync_all() {
            Ok(()) => trace(DurabilityStep::DirSynced, dir),
            Err(err) if directory_sync_unsupported(&err) => {}
            Err(source) => return Err(StorageError::io(dir, source)),
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Whether `EBADF` is 9 and `EINVAL` is 22 on this target. Only the targets
/// listed are confirmed; on any other Unix target no directory sync error is
/// tolerated.
#[cfg(unix)]
const KNOWN_DIRECTORY_SYNC_ERRNOS: bool = cfg!(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "illumos",
    target_os = "solaris",
    target_os = "aix"
));

/// Whether a directory fsync failed because the file system does not support
/// syncing directories, reported as `EBADF` (9) or `EINVAL` (22). This holds
/// only on the targets in [`KNOWN_DIRECTORY_SYNC_ERRNOS`]; elsewhere it is
/// always false, so a directory sync failure fails the operation.
#[cfg(unix)]
fn directory_sync_unsupported(err: &std::io::Error) -> bool {
    const EBADF: i32 = 9;
    const EINVAL: i32 = 22;
    KNOWN_DIRECTORY_SYNC_ERRNOS && matches!(err.raw_os_error(), Some(EBADF | EINVAL))
}

/// Opens a file that was written by another call, such as a copy, and syncs it.
async fn sync_written_file(path: &Path) -> Result<(), StorageError> {
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .await
        .map_err(|source| StorageError::io(path, source))?;
    sync_file(&file, path).await
}

/// Removes `path` and makes the removal durable. A missing file is success,
/// and its directory is still synced when it exists, so repeating a deletion
/// whose sync failed completes it.
async fn remove_durably(path: &Path) -> Result<(), StorageError> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => trace_published(None, path),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(StorageError::io(path, source)),
    }
    sync_entry_if_present(path).await
}

/// The directory whose entry names `path`.
fn entry_dir(path: &Path) -> Result<&Path, StorageError> {
    match path.parent() {
        Some(parent) if parent.as_os_str().is_empty() => Ok(Path::new(".")),
        Some(parent) => Ok(parent),
        None => Err(StorageError::MissingParent { path: path.into() }),
    }
}

/// Makes the entry for `path` durable after it was renamed, linked or removed.
async fn sync_entry(path: &Path) -> Result<(), StorageError> {
    sync_dir(entry_dir(path)?).await
}

/// Like [`sync_entry`], but succeeds when the directory itself is gone, so a
/// repeated deletion of a missing key still makes an earlier removal durable.
async fn sync_entry_if_present(path: &Path) -> Result<(), StorageError> {
    match sync_entry(path).await {
        Err(StorageError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(())
        }
        result => result,
    }
}

/// A durability step of a local write, recorded by tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DurabilityStep {
    FileSynced,
    DirSynced,
    /// The file that the next `Published` step renamed or linked into place.
    PublishedFrom,
    Published,
}

/// Records that `path` was renamed or linked from `from`, or removed when
/// `from` is `None`, into its final state.
pub(crate) fn trace_published(from: Option<&Path>, path: &Path) {
    if let Some(from) = from {
        trace(DurabilityStep::PublishedFrom, from);
    }
    trace(DurabilityStep::Published, path);
}

pub(crate) fn trace(step: DurabilityStep, path: &Path) {
    #[cfg(test)]
    durability_log::record(step, path);
    #[cfg(not(test))]
    let _ = (step, path);
}

/// Test-only record of the sync and publication order of local writes.
#[cfg(test)]
mod durability_log {
    use std::{
        path::{Path, PathBuf},
        sync::Mutex,
    };

    use super::DurabilityStep;

    static EVENTS: Mutex<Vec<(DurabilityStep, PathBuf)>> = Mutex::new(Vec::new());
    static FAILING_DIR_SYNCS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    /// Makes the next sync of the directory `dir` fail with `EIO`.
    pub(super) fn fail_next_dir_sync(dir: &Path) {
        FAILING_DIR_SYNCS
            .lock()
            .expect("injected failures")
            .push(dir.to_path_buf());
    }

    pub(super) fn take_injected_failure(dir: &Path) -> bool {
        let mut failing = FAILING_DIR_SYNCS.lock().expect("injected failures");
        match failing.iter().position(|path| path == dir) {
            Some(index) => {
                failing.remove(index);
                true
            }
            None => false,
        }
    }

    pub(super) fn record(step: DurabilityStep, path: &Path) {
        EVENTS
            .lock()
            .expect("durability log")
            .push((step, path.to_path_buf()));
    }

    /// Removes and returns the steps recorded at or below `root`, in order.
    pub(super) fn take_under(root: &Path) -> Vec<(DurabilityStep, PathBuf)> {
        let mut events = EVENTS.lock().expect("durability log");
        let (mine, others) = events
            .drain(..)
            .partition(|(_, path): &(DurabilityStep, PathBuf)| path.starts_with(root));
        *events = others;
        mine
    }
}

#[cfg(all(test, unix))]
mod durability_tests {
    use std::path::{Path, PathBuf};

    use bytes::Bytes;
    use tempfile::TempDir;

    use super::{
        DurabilityStep, KNOWN_DIRECTORY_SYNC_ERRNOS, LocalStorageBackend,
        directory_sync_unsupported, durability_log, is_uploading_temp_file,
    };
    use crate::{
        BlobPutOptions, BlobStore, MultipartBlobStore, MultipartStartOptions, ObjectMetadata,
        StorageByteStream, StorageError, StreamingObjectStore,
    };

    type Steps = Vec<(DurabilityStep, PathBuf)>;

    fn body() -> StorageByteStream {
        StorageByteStream::from_bytes(Bytes::from_static(b"payload"))
    }

    fn position(
        steps: &[(DurabilityStep, PathBuf)],
        step: DurabilityStep,
        path: &Path,
    ) -> Option<usize> {
        steps
            .iter()
            .position(|(recorded, recorded_path)| *recorded == step && recorded_path == path)
    }

    /// `target` was published from a temporary file beside it, that same
    /// file was synced first, and the directory entry was synced afterwards.
    fn assert_durably_published(steps: &Steps, target: &Path) {
        let published = position(steps, DurabilityStep::Published, target)
            .unwrap_or_else(|| panic!("{target:?} was never published: {steps:?}"));
        let parent = target.parent().expect("parent");
        let source = match published.checked_sub(1).map(|index| &steps[index]) {
            Some((DurabilityStep::PublishedFrom, source)) => source,
            _ => panic!("{target:?} was not published from a file: {steps:?}"),
        };
        assert!(
            source.parent() == Some(parent) && is_uploading_temp_file(source),
            "{target:?} was published from {source:?}, not a temporary file beside it"
        );
        assert!(
            position(&steps[..published - 1], DurabilityStep::FileSynced, source).is_some(),
            "{target:?} was published before {source:?} was synced: {steps:?}"
        );
        assert!(
            steps[published + 1..]
                .iter()
                .any(|(step, path)| *step == DurabilityStep::DirSynced && path == parent),
            "{target:?}'s directory was not synced after publication: {steps:?}"
        );
    }

    /// Every directory created for `target` below `root` has a durable entry
    /// before `target` is published.
    fn assert_new_directories_durable(steps: &Steps, root: &Path, target: &Path) {
        let published = position(steps, DurabilityStep::Published, target).expect("published");
        let mut directory = target.parent().expect("parent");
        while directory != root {
            let parent = directory.parent().expect("parent");
            assert!(
                position(&steps[..published], DurabilityStep::DirSynced, parent).is_some(),
                "entry for new directory {directory:?} was not synced: {steps:?}"
            );
            directory = parent;
        }
    }

    #[tokio::test]
    async fn put_blob_syncs_data_before_rename_and_directories_after() {
        let temp = TempDir::new().expect("temp dir");
        let backend = LocalStorageBackend::new(temp.path());
        let target = temp.path().join("snapshots/a/manifest.json");

        backend
            .put_blob(
                "snapshots/a/manifest.json",
                body(),
                BlobPutOptions::default(),
            )
            .await
            .expect("put blob");

        let steps = durability_log::take_under(temp.path());
        assert_durably_published(&steps, &target);
        assert_new_directories_durable(&steps, temp.path(), &target);
    }

    #[tokio::test]
    async fn put_blob_if_not_exists_syncs_data_before_link_and_directory_after() {
        let temp = TempDir::new().expect("temp dir");
        let backend = LocalStorageBackend::new(temp.path());
        let target = temp.path().join("locks/repository");

        let created = backend
            .put_blob_if_not_exists("locks/repository", body(), BlobPutOptions::default())
            .await
            .expect("conditional put");
        assert!(created.is_some());

        let steps = durability_log::take_under(temp.path());
        assert_durably_published(&steps, &target);
        assert_new_directories_durable(&steps, temp.path(), &target);
    }

    #[tokio::test]
    async fn copy_blob_syncs_the_copy_before_rename_and_directory_after() {
        let temp = TempDir::new().expect("temp dir");
        let backend = LocalStorageBackend::new(temp.path());
        backend
            .put_blob("staging/object", body(), BlobPutOptions::default())
            .await
            .expect("put blob");
        let _ = durability_log::take_under(temp.path());
        let target = temp.path().join("objects/object");

        backend
            .copy_blob("staging/object", "objects/object")
            .await
            .expect("copy blob");

        let steps = durability_log::take_under(temp.path());
        assert_durably_published(&steps, &target);
        assert_new_directories_durable(&steps, temp.path(), &target);
    }

    #[tokio::test]
    async fn delete_blob_syncs_the_directory_after_removal() {
        let temp = TempDir::new().expect("temp dir");
        let backend = LocalStorageBackend::new(temp.path());
        backend
            .put_blob("objects/object", body(), BlobPutOptions::default())
            .await
            .expect("put blob");
        let _ = durability_log::take_under(temp.path());
        let target = temp.path().join("objects/object");

        backend.delete_blob("objects/object").await.expect("delete");
        let steps = durability_log::take_under(temp.path());
        let removed = position(&steps, DurabilityStep::Published, &target).expect("removed");
        assert!(
            position(
                &steps[removed + 1..],
                DurabilityStep::DirSynced,
                target.parent().expect("parent")
            )
            .is_some(),
            "directory not synced after removal: {steps:?}"
        );

        // Deleting the now-missing key, or one in a missing directory, succeeds.
        backend
            .delete_blob("objects/object")
            .await
            .expect("delete missing");
        backend
            .delete_blob_once("missing/object")
            .await
            .expect("delete in missing directory");
    }

    #[tokio::test]
    async fn streamed_objects_and_their_metadata_are_durable() {
        let temp = TempDir::new().expect("temp dir");
        let backend = LocalStorageBackend::new(temp.path());
        let object = backend
            .put_object_stream(
                "bucket",
                "recordings/one",
                None,
                ObjectMetadata::default(),
                body(),
            )
            .await
            .expect("put object stream");
        let object_path = backend
            .object_path_for(&object.bucket, &object.key)
            .expect("object path");
        let metadata_path = backend
            .object_metadata_path_for(&object.bucket, &object.key)
            .expect("metadata path");

        let steps = durability_log::take_under(temp.path());
        assert_durably_published(&steps, &object_path);
        assert_durably_published(&steps, &metadata_path);
        assert_new_directories_durable(&steps, temp.path(), &object_path);
        assert_new_directories_durable(&steps, temp.path(), &metadata_path);

        backend
            .delete_object("bucket", "recordings/one")
            .await
            .expect("delete object");
        let steps = durability_log::take_under(temp.path());
        for path in [&object_path, &metadata_path] {
            let removed = position(&steps, DurabilityStep::Published, path).expect("removed");
            assert!(
                steps[removed + 1..].iter().any(|(step, synced)| {
                    *step == DurabilityStep::DirSynced && Some(synced.as_path()) == path.parent()
                }),
                "directory of {path:?} not synced after removal: {steps:?}"
            );
        }
    }

    #[tokio::test]
    async fn multipart_completion_makes_new_directories_durable() {
        let temp = TempDir::new().expect("temp dir");
        let backend = LocalStorageBackend::new(temp.path());
        let handle = backend
            .start_multipart("archives/2026/object.bin", MultipartStartOptions::default())
            .await
            .expect("start");
        let steps = durability_log::take_under(temp.path());
        // Staging creates `.graphql-orm-storage/multipart`: both entries are synced.
        let internal = temp.path().join(super::INTERNAL_STORAGE_DIR);
        for directory in [temp.path(), internal.as_path()] {
            assert!(
                position(&steps, DurabilityStep::DirSynced, directory).is_some(),
                "staging directory entry in {directory:?} not synced: {steps:?}"
            );
        }

        let receipt = backend
            .upload_part(&handle, 1, body())
            .await
            .expect("upload part");
        let _ = durability_log::take_under(temp.path());
        backend
            .complete_multipart(&handle, &[receipt])
            .await
            .expect("complete");

        let target = temp.path().join("archives/2026/object.bin");
        let steps = durability_log::take_under(temp.path());
        assert_durably_published(&steps, &target);
        assert_new_directories_durable(&steps, temp.path(), &target);
    }

    #[tokio::test]
    async fn a_directory_created_by_another_writer_is_made_durable() {
        let temp = TempDir::new().expect("temp dir");
        // Another writer created `shared` and has not synced the root yet.
        std::fs::create_dir(temp.path().join("shared")).expect("create directory");
        let backend = LocalStorageBackend::new(temp.path());

        backend
            .put_blob("shared/one", body(), BlobPutOptions::default())
            .await
            .expect("put blob");
        let steps = durability_log::take_under(temp.path());
        let target = temp.path().join("shared/one");
        let published = position(&steps, DurabilityStep::Published, &target).expect("published");
        assert!(
            position(&steps[..published], DurabilityStep::DirSynced, temp.path()).is_some(),
            "entry for the existing directory was not synced: {steps:?}"
        );

        // A known-durable directory is not synced again, by this backend or a
        // clone of it.
        backend
            .clone()
            .put_blob("shared/two", body(), BlobPutOptions::default())
            .await
            .expect("put blob");
        let steps = durability_log::take_under(temp.path());
        assert!(
            position(&steps, DurabilityStep::DirSynced, temp.path()).is_none(),
            "known-durable directory synced again: {steps:?}"
        );

        // A separate backend does not know the directory is durable.
        LocalStorageBackend::new(temp.path())
            .put_blob("shared/three", body(), BlobPutOptions::default())
            .await
            .expect("put blob");
        let steps = durability_log::take_under(temp.path());
        assert!(position(&steps, DurabilityStep::DirSynced, temp.path()).is_some());

        // A remembered directory that was removed and recreated is synced again.
        std::fs::remove_dir_all(temp.path().join("shared")).expect("remove directory");
        backend
            .put_blob("shared/four", body(), BlobPutOptions::default())
            .await
            .expect("put blob");
        let steps = durability_log::take_under(temp.path());
        assert!(
            position(&steps, DurabilityStep::DirSynced, temp.path()).is_some(),
            "recreated directory was not synced: {steps:?}"
        );
    }

    #[tokio::test]
    async fn conditional_put_syncs_an_existing_entry_and_never_invites_a_retry() {
        let temp = TempDir::new().expect("temp dir");
        let backend = LocalStorageBackend::new(temp.path());
        backend
            .put_blob_if_not_exists("locks/one", body(), BlobPutOptions::default())
            .await
            .expect("create");
        let _ = durability_log::take_under(temp.path());

        let existing = backend
            .put_blob_if_not_exists("locks/one", body(), BlobPutOptions::default())
            .await
            .expect("existing");
        assert!(existing.is_none());
        let steps = durability_log::take_under(temp.path());
        assert!(
            position(
                &steps,
                DurabilityStep::DirSynced,
                &temp.path().join("locks")
            )
            .is_some(),
            "observed entry was not synced: {steps:?}"
        );

        // A failed sync after the link reports a non-retryable error whose
        // outcome is unknown: the key exists, so a retry would find it taken.
        durability_log::fail_next_dir_sync(&temp.path().join("locks"));
        let error = backend
            .put_blob_if_not_exists("locks/two", body(), BlobPutOptions::default())
            .await
            .expect_err("sync failure");
        assert!(matches!(error, StorageError::SingleAttemptFailed { .. }));
        assert!(!error.is_retryable());
        assert!(
            backend
                .put_blob_if_not_exists("locks/two", body(), BlobPutOptions::default())
                .await
                .expect("retry")
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_remembered_directory_is_forgotten_before_it_is_recreated() {
        let temp = TempDir::new().expect("temp dir");
        let backend = LocalStorageBackend::new(temp.path());
        backend
            .put_blob("a/b/one", body(), BlobPutOptions::default())
            .await
            .expect("put blob");
        std::fs::remove_dir_all(temp.path().join("a/b")).expect("remove directory");
        let _ = durability_log::take_under(temp.path());

        // Writer X finds the remembered `a/b` missing and recreates it, but
        // has not made its entry durable yet when writer Y arrives. The
        // injected failure stops X at that point.
        durability_log::fail_next_dir_sync(&temp.path().join("a"));
        backend
            .put_blob("a/b/two", body(), BlobPutOptions::default())
            .await
            .expect_err("writer X stops before syncing the entry");
        assert!(temp.path().join("a/b").is_dir());
        let _ = durability_log::take_under(temp.path());

        // Writer Y finds `a/b` present and must not trust it.
        backend
            .put_blob("a/b/three", body(), BlobPutOptions::default())
            .await
            .expect("writer Y");
        let steps = durability_log::take_under(temp.path());
        let target = temp.path().join("a/b/three");
        let published = position(&steps, DurabilityStep::Published, &target).expect("published");
        assert!(
            position(
                &steps[..published],
                DurabilityStep::DirSynced,
                &temp.path().join("a")
            )
            .is_some(),
            "writer Y trusted the recreated directory: {steps:?}"
        );
    }

    #[tokio::test]
    async fn a_failed_directory_sync_is_retried_by_the_next_write() {
        let temp = TempDir::new().expect("temp dir");
        let backend = LocalStorageBackend::new(temp.path());
        durability_log::fail_next_dir_sync(temp.path());
        let error = backend
            .put_blob("fresh/one", body(), BlobPutOptions::default())
            .await
            .expect_err("sync failure");
        assert!(matches!(error, StorageError::Io { .. }));
        let _ = durability_log::take_under(temp.path());

        // `fresh` now exists, but its entry was never synced.
        backend
            .put_blob("fresh/one", body(), BlobPutOptions::default())
            .await
            .expect("retry");
        let steps = durability_log::take_under(temp.path());
        assert!(
            position(&steps, DurabilityStep::DirSynced, temp.path()).is_some(),
            "entry left unsynced by the failed write was not synced: {steps:?}"
        );
    }

    #[test]
    fn only_unsupported_directory_sync_errors_are_tolerated() {
        // Tolerated only where the errno values are confirmed.
        for code in [9, 22] {
            assert_eq!(
                directory_sync_unsupported(&std::io::Error::from_raw_os_error(code)),
                KNOWN_DIRECTORY_SYNC_ERRNOS
            );
        }
        if KNOWN_DIRECTORY_SYNC_ERRNOS {
            assert_eq!(
                std::io::Error::from_raw_os_error(22).kind(),
                std::io::ErrorKind::InvalidInput,
                "EINVAL has the expected value"
            );
        }
        for code in [1, 5, 13, 28, 30] {
            assert!(!directory_sync_unsupported(
                &std::io::Error::from_raw_os_error(code)
            ));
        }
    }
}
