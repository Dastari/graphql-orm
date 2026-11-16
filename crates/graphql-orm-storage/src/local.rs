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
    local_fs::{self, Dir, Kind},
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
/// The root tree must be managed by the trusted storage deployment, and
/// filesystem permissions should keep untrusted local actors from writing to
/// it.
///
/// The backend never follows a filesystem link below the root, and never
/// reads a file with more than one hard link. Every operation opens the
/// root and reaches its target one component at a time relative to the
/// directory handle it holds, without following links (`openat2` with
/// `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS` or `openat(O_NOFOLLOW)` walks on
/// Unix, held `FILE_FLAG_OPEN_REPARSE_POINT` handles on Windows), and leaf
/// operations act on the held parent directory, never on a full path. A
/// key whose path below the root names or passes through a symbolic link
/// (on Windows, also a junction, mount point or other name-surrogate
/// reparse point), or a read of a file another hard link shares, fails with
/// [`StorageError::InvalidStorageKey`] without touching the link's target,
/// including when the link replaces a directory while the operation runs.
/// Listings neither list nor enter links. The configured root itself may be
/// a link; the host chose it.
#[derive(Clone, Debug)]
pub struct LocalStorageBackend {
    root: PathBuf,
    multipart_max_part_size: u64,
    /// Whether new writes must use canonical keys; see
    /// [`Self::with_canonical_keys`].
    canonical_keys: bool,
    /// Directories below the root whose entries, and every ancestor's entry up
    /// to the root, this backend (or a clone) has synced.
    durable_dirs: Arc<Mutex<HashSet<PathBuf>>>,
}

impl LocalStorageBackend {
    /// Creates a local storage backend rooted at the given directory.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            multipart_max_part_size: crate::local_multipart::DEFAULT_MAX_PART_SIZE,
            canonical_keys: false,
            durable_dirs: Arc::default(),
        }
    }

    /// Refuses new writes whose key [`validate_canonical_blob_key`] refuses;
    /// off by default.
    ///
    /// Enable it for a root on a case-insensitive or normalising file system,
    /// such as NTFS, APFS or an OS-mounted SMB share, where two keys that
    /// differ only in case or Unicode normalisation would name the same file.
    /// Canonical keys are lowercase ASCII and portable, so no two distinct
    /// canonical keys can alias. It applies to the keys of new writes and
    /// every publication: [`BlobStore::put_blob`],
    /// [`BlobStore::put_blob_if_not_exists`], the destination of
    /// [`BlobStore::copy_blob`], multipart starts and completions, including
    /// completion of an upload started before the mode was enabled, and the
    /// `bucket/key` of streaming objects. A refused write fails with
    /// [`StorageError::InvalidStorageKey`] before any filesystem change.
    /// Reads, deletes and listings keep the ordinary validation, so objects
    /// written before the mode was enabled stay readable, listed and
    /// deletable.
    ///
    /// [`validate_canonical_blob_key`]: crate::validate_canonical_blob_key
    #[must_use]
    pub fn with_canonical_keys(mut self, enabled: bool) -> Self {
        self.canonical_keys = enabled;
        self
    }

    /// Applies [`Self::with_canonical_keys`] to the key of a new write.
    pub(crate) fn check_write_key(&self, key: &str) -> Result<(), StorageError> {
        if self.canonical_keys {
            crate::validate_canonical_blob_key(key)?;
        }
        Ok(())
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
    /// may be interrupted during writes. The sweep walks the tree relative to
    /// held directory handles and never follows a link: symbolic links
    /// (including directory links and cycles) and other non-file entries are
    /// skipped, and a root that is itself a symbolic link is not swept.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the storage root cannot be walked or a
    /// stale temp file cannot be removed.
    pub async fn sweep_temp_files(&self, older_than: Duration) -> Result<usize, StorageError> {
        let now = SystemTime::now();
        match tokio::fs::symlink_metadata(&self.root).await {
            Ok(metadata) if metadata.file_type().is_symlink() => return Ok(0),
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(source) => return Err(StorageError::io(&self.root, source)),
        }
        self.blocking(move |backend| {
            let Some(root) = backend.open_root()? else {
                return Ok(0);
            };
            let mut removed = 0;
            let mut stack = vec![root];
            while let Some(dir) = stack.pop() {
                let entries = match dir.entries() {
                    Ok(entries) => entries,
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(source) => return Err(StorageError::io(dir.path(), source)),
                };
                for entry in entries {
                    let Some(name) = entry.name.to_str() else {
                        continue;
                    };
                    match entry.kind {
                        Kind::File if is_temp_file_name(name) => {
                            let path = dir.child_path(name);
                            let info = match dir.info(name) {
                                Ok(Some(info)) => info,
                                Ok(None) => continue,
                                Err(source) => return Err(StorageError::io(&path, source)),
                            };
                            if info.kind == Kind::File
                                && is_older_than(info.modified, now, older_than)
                            {
                                match dir.remove_file(name) {
                                    Ok(()) => removed += 1,
                                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                                    Err(source) => return Err(StorageError::io(&path, source)),
                                }
                            }
                        }
                        Kind::Dir => match dir.child_dir(name) {
                            Ok(Some(child)) => stack.push(child),
                            Ok(None) => {}
                            // Replaced by a link since it was listed.
                            Err(err) if local_fs::is_refusal(&err) => {}
                            Err(source) => {
                                return Err(StorageError::io(dir.child_path(name), source));
                            }
                        },
                        _ => {}
                    }
                }
            }
            Ok(removed)
        })
        .await
    }

    /// The root-relative path of a caller's key.
    pub(crate) fn rel_for(&self, key: &str) -> Result<String, StorageError> {
        validate_blob_key(key)?;
        self.check_root_name(key)?;
        Ok(key.to_string())
    }

    fn object_rel_for(&self, bucket: &str, key: &str) -> Result<String, StorageError> {
        validate_object_bucket(bucket)?;
        validate_blob_key(key)?;
        self.check_root_name(bucket)?;
        refuse_colon(key)?;
        Ok(format!("{bucket}/{key}"))
    }

    fn object_metadata_rel_for(&self, bucket: &str, key: &str) -> Result<String, StorageError> {
        validate_object_bucket(bucket)?;
        validate_blob_key(key)?;
        self.check_root_name(bucket)?;
        refuse_colon(key)?;
        Ok(format!(
            "{INTERNAL_STORAGE_DIR}/{OBJECT_METADATA_DIR}/{bucket}/{key}.metadata.json"
        ))
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
        let _ = std::fs::create_dir_all(&self.root);
        if let Ok(Some(root)) = Dir::open_root(&self.root) {
            let _ = root.create_dir(INTERNAL_STORAGE_DIR);
        }
    }

    /// Whether `path` resolves to the internal directory, whatever its
    /// spelling: another case, a Unicode case fold or a Windows short name.
    /// A path that does not exist or cannot be opened is not an alias; the
    /// operation on it fails or creates an ordinary entry.
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

/// The directory components and the leaf name of a root-relative path.
pub(crate) fn split_rel(rel: &str) -> (Vec<&str>, &str) {
    match rel.rsplit_once('/') {
        Some((dirs, leaf)) => (dirs.split('/').collect(), leaf),
        None => (Vec::new(), rel),
    }
}

/// Refuses a key whose leaf is itself a temporary name, so that listing and
/// [`LocalStorageBackend::sweep_temp_files`] never mistake a caller's object
/// for an interrupted upload.
pub(crate) fn check_leaf(rel: &str, key: &str) -> Result<(), StorageError> {
    if is_temp_file_name(split_rel(rel).1) {
        return Err(StorageError::InvalidStorageKey {
            key: key.to_string(),
        });
    }
    Ok(())
}

/// Maps a filesystem error for `key`: a refused link or hard link becomes
/// [`StorageError::InvalidStorageKey`], anything else an I/O error at `path`.
pub(crate) fn fs_error(error: std::io::Error, key: &str, path: &Path) -> StorageError {
    if local_fs::is_refusal(&error) {
        StorageError::InvalidStorageKey {
            key: key.to_string(),
        }
    } else {
        StorageError::io(path, error)
    }
}

/// A key's parent directory, held open, and the key's leaf name in it.
pub(crate) struct Located {
    pub(crate) dir: Arc<Dir>,
    pub(crate) leaf: String,
}

impl Located {
    pub(crate) fn path(&self) -> PathBuf {
        self.dir.child_path(&self.leaf)
    }

    /// Refuses when the leaf is a link. Replacing or removing a link would
    /// not follow it, but a key that names a link is refused everywhere.
    pub(crate) fn refuse_link_leaf(&self, key: &str) -> Result<(), StorageError> {
        match self.dir.info(&self.leaf) {
            Ok(Some(info)) if info.kind == Kind::Link => Err(StorageError::InvalidStorageKey {
                key: key.to_string(),
            }),
            Ok(_) => Ok(()),
            Err(error) => Err(fs_error(error, key, &self.path())),
        }
    }

    /// Opens the leaf for reading without following a link, with its
    /// metadata, or `None` when it does not exist.
    pub(crate) fn open_read(
        &self,
        key: &str,
    ) -> Result<Option<(std::fs::File, std::fs::Metadata)>, StorageError> {
        let Some(file) = self
            .dir
            .open_read(&self.leaf)
            .map_err(|error| fs_error(error, key, &self.path()))?
        else {
            return Ok(None);
        };
        let metadata = file
            .metadata()
            .map_err(|source| StorageError::io(self.path(), source))?;
        Ok(Some((file, metadata)))
    }
}

impl LocalStorageBackend {
    /// Runs `operation` on the blocking pool with a clone of this backend.
    pub(crate) async fn blocking<T: Send + 'static>(
        &self,
        operation: impl FnOnce(Self) -> Result<T, StorageError> + Send + 'static,
    ) -> Result<T, StorageError> {
        let backend = self.clone();
        tokio::task::spawn_blocking(move || operation(backend))
            .await
            .map_err(crate::local_multipart::join_error)?
    }

    /// Opens the root, or `None` when it does not exist.
    pub(crate) fn open_root(&self) -> Result<Option<Dir>, StorageError> {
        Dir::open_root(&self.root).map_err(|source| StorageError::io(&self.root, source))
    }

    /// Opens the directory `components` below the root without creating
    /// anything, or `None` when it does not exist.
    pub(crate) fn open_dir_blocking(
        &self,
        components: &[&str],
        key: &str,
    ) -> Result<Option<Dir>, StorageError> {
        let Some(root) = self.open_root()? else {
            return Ok(None);
        };
        let path = components
            .iter()
            .fold(self.root.clone(), |path, name| path.join(name));
        root.descend(components)
            .map_err(|error| fs_error(error, key, &path))
    }

    /// Finds the directory holding `rel` without creating anything, or
    /// `None` when it does not exist.
    pub(crate) fn locate_blocking(
        &self,
        rel: &str,
        key: &str,
    ) -> Result<Option<Located>, StorageError> {
        let (dirs, leaf) = split_rel(rel);
        let located = self.open_dir_blocking(&dirs, key)?.map(|dir| Located {
            dir: Arc::new(dir),
            leaf: leaf.to_string(),
        });
        #[cfg(test)]
        race_hook::fire(&self.root);
        Ok(located)
    }

    /// Creates the directory holding `rel` durably (see
    /// [`Self::prepare_dir_blocking`]) and refuses a leaf that is a link.
    pub(crate) fn prepare_blocking(&self, rel: &str, key: &str) -> Result<Located, StorageError> {
        let (dirs, leaf) = split_rel(rel);
        let located = Located {
            dir: self.prepare_dir_blocking(&dirs, key)?,
            leaf: leaf.to_string(),
        };
        located.refuse_link_leaf(key)?;
        #[cfg(test)]
        race_hook::fire(&self.root);
        Ok(located)
    }

    /// [`Self::prepare_blocking`] on the blocking pool.
    pub(crate) async fn prepare(&self, rel: String, key: &str) -> Result<Located, StorageError> {
        let key = key.to_string();
        self.blocking(move |backend| backend.prepare_blocking(&rel, &key))
            .await
    }

    /// Opens a caller's key for reading, or `None` when nothing is stored
    /// there.
    async fn open_key(
        &self,
        rel: String,
        key: &str,
    ) -> Result<Option<(tokio::fs::File, std::fs::Metadata, PathBuf)>, StorageError> {
        let key = key.to_string();
        let opened = self
            .blocking(move |backend| {
                let Some(located) = backend.locate_blocking(&rel, &key)? else {
                    return Ok(None);
                };
                let path = located.path();
                Ok(located
                    .open_read(&key)?
                    .map(|(file, metadata)| (file, metadata, path)))
            })
            .await?;
        Ok(opened.map(|(file, metadata, path)| (tokio::fs::File::from_std(file), metadata, path)))
    }

    /// What is stored at a caller's key, inspected without following it, or
    /// `None` when nothing is. A link is refused, and so is a file with more
    /// than one hard link.
    async fn key_info(
        &self,
        rel: String,
        key: &str,
    ) -> Result<Option<local_fs::Info>, StorageError> {
        let key = key.to_string();
        self.blocking(move |backend| {
            let Some(located) = backend.locate_blocking(&rel, &key)? else {
                return Ok(None);
            };
            let refused = || StorageError::InvalidStorageKey { key: key.clone() };
            match located.dir.info(&located.leaf) {
                Ok(Some(info)) if info.kind == Kind::Link => Err(refused()),
                Ok(Some(info)) if info.kind == Kind::File && info.links > 1 => Err(refused()),
                Ok(info) => Ok(info),
                Err(error) => Err(fs_error(error, &key, &located.path())),
            }
        })
        .await
    }

    /// Removes the key at `rel` and makes the removal durable. A missing
    /// file is success, and its directory is still synced when it exists,
    /// so repeating a deletion whose sync failed completes it. A link is
    /// refused; a file with other hard links loses only this one.
    fn delete_blocking(&self, rel: &str, key: &str) -> Result<(), StorageError> {
        let Some(located) = self.locate_blocking(rel, key)? else {
            return Ok(());
        };
        located.refuse_link_leaf(key)?;
        let path = located.path();
        match located.dir.remove_file(&located.leaf) {
            Ok(()) => trace_published(None, &path),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(fs_error(source, key, &path)),
        }
        sync_dir(&located.dir)
    }
}

#[async_trait]
impl BlobStore for LocalStorageBackend {
    async fn delete_blob_once(&self, key: &str) -> Result<(), StorageError> {
        // A refused key changes nothing, so it is not a single-attempt
        // failure. One filesystem removal, with no retry or background path
        // replay.
        let rel = self.rel_for(key)?;
        let key = key.to_string();
        self.blocking(move |backend| {
            backend
                .delete_blocking(&rel, &key)
                .map_err(|error| match error {
                    refused @ StorageError::InvalidStorageKey { .. } => refused,
                    other => StorageError::single_attempt(other),
                })
        })
        .await
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
        self.check_write_key(key)?;
        let rel = self.rel_for(key)?;
        check_leaf(&rel, key)?;
        let located = self.prepare(rel, key).await?;
        let temp = TempFile::new(Arc::clone(&located.dir));
        let outcome = write_stream_to_temp(&temp, body).await?;
        let key = key.to_string();
        let temp_name = temp.name().to_string();
        let temp_path = temp.path().to_path_buf();
        let published = self
            .blocking(move |_| {
                located.refuse_link_leaf(&key)?;
                let target = located.path();
                located
                    .dir
                    .rename(&temp_name, &located.leaf)
                    .map_err(|source| StorageError::io(&target, source))?;
                trace_published(Some(&temp_path), &target);
                Ok(located)
            })
            .await;
        let located = match published {
            Ok(located) => {
                temp.published();
                located
            }
            Err(error) => return Err(error),
        };
        let dir = Arc::clone(&located.dir);
        self.blocking(move |_| sync_dir(&dir)).await?;
        Ok(outcome)
    }

    async fn put_blob_if_not_exists(
        &self,
        key: &str,
        body: StorageByteStream,
        _options: BlobPutOptions,
    ) -> Result<Option<BlobWriteOutcome>, StorageError> {
        self.ensure_internal_dir();
        self.check_write_key(key)?;
        let rel = self.rel_for(key)?;
        check_leaf(&rel, key)?;
        let located = self.prepare(rel, key).await?;
        let temp = TempFile::new(Arc::clone(&located.dir));
        let outcome = write_stream_to_temp(&temp, body).await?;
        let key = key.to_string();
        let temp_name = temp.name().to_string();
        let temp_path = temp.path().to_path_buf();
        let published = self
            .blocking(move |_| {
                located.refuse_link_leaf(&key)?;
                let target = located.path();
                match located.dir.rename_noreplace(&temp_name, &located.leaf) {
                    Ok(()) => {
                        trace_published(Some(&temp_path), &target);
                        Ok(true)
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
                    Err(source) => Err(StorageError::io(&target, source)),
                }
            })
            .await;
        let dir = Arc::clone(&temp.dir);
        match published {
            Ok(true) => {
                temp.published();
                // One directory sync covers the new entry and the temp
                // removal. The key now exists, so a retry would find it
                // taken: a failed sync reports a non-retryable,
                // outcome-unknown error.
                self.blocking(move |_| sync_dir(&dir).map_err(StorageError::single_attempt))
                    .await?;
                Ok(Some(outcome))
            }
            Ok(false) => {
                temp.remove().await;
                // Make the existing entry this call observed durable too.
                self.blocking(move |_| sync_dir(&dir)).await?;
                Ok(None)
            }
            Err(error) => {
                temp.remove().await;
                Err(error)
            }
        }
    }

    async fn get_blob(&self, key: &str) -> Result<BlobBody, StorageError> {
        let rel = self.rel_for(key)?;
        let Some((file, metadata, path)) = self.open_key(rel, key).await? else {
            return Err(StorageError::MissingBlob {
                key: key.to_string(),
            });
        };
        let metadata = Some(blob_metadata(key, &metadata));
        let stream = ReaderStream::new(file)
            .map(move |chunk| chunk.map_err(|source| StorageError::io(&path, source)));

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

        let rel = self.rel_for(key)?;
        let Some((mut file, metadata, path)) = self.open_key(rel, key).await? else {
            return Err(StorageError::MissingBlob {
                key: key.to_string(),
            });
        };
        file.seek(SeekFrom::Start(range.start))
            .await
            .map_err(|source| StorageError::io(&path, source))?;
        let length = range.end - range.start;
        let stream = ReaderStream::new(file.take(length))
            .map(move |chunk| chunk.map_err(|source| StorageError::io(&path, source)));

        Ok(BlobBody {
            key: key.to_string(),
            metadata: Some(blob_metadata(key, &metadata)),
            body: StorageByteStream::with_size_hint(Box::pin(stream), length),
        })
    }

    async fn blob_exists(&self, key: &str) -> Result<bool, StorageError> {
        let rel = self.rel_for(key)?;
        Ok(self
            .key_info(rel, key)
            .await?
            .is_some_and(|info| info.kind == Kind::File))
    }

    async fn head_blob(&self, key: &str) -> Result<Option<BlobMetadata>, StorageError> {
        let rel = self.rel_for(key)?;
        Ok(self.key_info(rel, key).await?.map(|info| BlobMetadata {
            key: key.to_string(),
            size_bytes: Some(info.len),
            sha256_hex: None,
            etag: None,
            last_modified: info.modified.map(OffsetDateTime::from),
        }))
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

        self.collect_blob_page(prefix, continuation, limit).await
    }

    async fn copy_blob(&self, from: &str, to: &str) -> Result<(), StorageError> {
        self.ensure_internal_dir();
        self.check_write_key(to)?;
        let from_rel = self.rel_for(from)?;
        let to_rel = self.rel_for(to)?;
        check_leaf(&to_rel, to)?;
        // The source is opened without following a link before anything is
        // created, so a refused or missing source leaves nothing behind.
        let Some((source, _, source_path)) = self.open_key(from_rel, from).await? else {
            return Err(StorageError::MissingBlob {
                key: from.to_string(),
            });
        };
        let mut source = source.into_std().await;
        let located = self.prepare(to_rel, to).await?;
        let temp = TempFile::new(Arc::clone(&located.dir));
        temp.run_blocking(move |dir, name| {
            let mut output = dir.create_new(name)?;
            std::io::copy(&mut source, &mut output)?;
            output.sync_all()
        })
        .await
        .map_err(|source| StorageError::io(&source_path, source))?;
        trace(DurabilityStep::FileSynced, temp.path());

        let key = to.to_string();
        let temp_name = temp.name().to_string();
        let temp_path = temp.path().to_path_buf();
        let located = self
            .blocking(move |_| {
                located.refuse_link_leaf(&key)?;
                let target = located.path();
                located
                    .dir
                    .rename(&temp_name, &located.leaf)
                    .map_err(|source| StorageError::io(&target, source))?;
                trace_published(Some(&temp_path), &target);
                Ok(located)
            })
            .await?;
        temp.published();
        self.blocking(move |_| sync_dir(&located.dir)).await
    }

    async fn delete_blob(&self, key: &str) -> Result<(), StorageError> {
        let rel = self.rel_for(key)?;
        let key = key.to_string();
        self.blocking(move |backend| backend.delete_blocking(&rel, &key))
            .await
    }
}

/// Provider metadata for the regular file at `key`.
fn blob_metadata(key: &str, metadata: &std::fs::Metadata) -> BlobMetadata {
    BlobMetadata {
        key: key.to_string(),
        size_bytes: Some(metadata.len()),
        sha256_hex: None,
        etag: None,
        last_modified: metadata.modified().ok().map(OffsetDateTime::from),
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
        let object_rel = self.object_rel_for(bucket, key)?;
        let metadata_rel = self.object_metadata_rel_for(bucket, key)?;
        self.check_write_key(&object_rel)?;
        check_leaf(&object_rel, key)?;
        let object = self.prepare(object_rel, key).await?;
        let metadata_location = self.prepare(metadata_rel, key).await?;
        let temp = TempFile::new(Arc::clone(&object.dir));
        let metadata_temp = TempFile::new(Arc::clone(&metadata_location.dir));
        let file = temp.create().await?;

        Ok(Box::new(LocalMultipartWriter {
            backend: self.clone(),
            bucket: bucket.to_string(),
            key: key.to_string(),
            content_type,
            metadata,
            object,
            temp,
            metadata_location,
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

        // The object file is opened, without following links, before its
        // metadata is read, so a key through a link is refused either way.
        let object_rel = self.object_rel_for(bucket, key)?;
        let Some((mut file, _, object_path)) = self.open_key(object_rel, key).await? else {
            return Err(StorageError::MissingBlob {
                key: format!("{bucket}/{key}"),
            });
        };
        let object = self.get_object_metadata(bucket, key).await?;
        if range.start > object.size_bytes {
            return Err(StorageError::PreconditionFailed {
                key: format!("{bucket}/{key}"),
                condition: "range start is beyond object size".to_string(),
            });
        }
        let end = range.end.min(object.size_bytes);
        let length = end.saturating_sub(range.start);
        file.seek(SeekFrom::Start(range.start))
            .await
            .map_err(|source| StorageError::io(&object_path, source))?;
        let stream = ReaderStream::new(file.take(length))
            .map(move |chunk| chunk.map_err(|source| StorageError::io(&object_path, source)));

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
        let metadata_rel = self.object_metadata_rel_for(bucket, key)?;
        let Some((mut file, _, metadata_path)) = self.open_key(metadata_rel, key).await? else {
            return Err(StorageError::MissingBlob {
                key: format!("{bucket}/{key}"),
            });
        };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .await
            .map_err(|source| StorageError::io(&metadata_path, source))?;
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
        let start_key = listing_start_key(prefix).to_string();
        let mut start: Vec<String> = vec![bucket.to_string()];
        start.extend(
            start_key
                .split('/')
                .filter(|name| !name.is_empty())
                .map(str::to_string),
        );
        let prefix_owned = prefix.to_string();
        let keys = self
            .blocking(move |backend| {
                let components: Vec<&str> = start.iter().map(String::as_str).collect();
                let Some(start_dir) = backend.open_dir_blocking(&components, &prefix_owned)? else {
                    return Ok(Vec::new());
                };
                let mut keys = Vec::new();
                let mut stack = vec![(start_dir, start_key)];
                while let Some((dir, dir_key)) = stack.pop() {
                    let entries = match dir.entries() {
                        Ok(entries) => entries,
                        Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(source) => return Err(StorageError::io(dir.path(), source)),
                    };
                    for entry in entries {
                        let Some(name) = entry.name.to_str() else {
                            continue;
                        };
                        if cfg!(windows) && name.contains(':') {
                            continue;
                        }
                        let key = if dir_key.is_empty() {
                            name.to_string()
                        } else {
                            format!("{dir_key}/{name}")
                        };
                        if validate_blob_key(&key).is_err() {
                            continue;
                        }
                        match entry.kind {
                            Kind::File => {
                                if !is_temp_file_name(name) && key.starts_with(&prefix_owned) {
                                    keys.push(key);
                                }
                            }
                            Kind::Dir if directory_may_match(&key, &prefix_owned) => {
                                match dir.child_dir(name) {
                                    Ok(Some(child)) => stack.push((child, key)),
                                    Ok(None) => {}
                                    Err(err) if local_fs::is_refusal(&err) => {}
                                    Err(source) => {
                                        return Err(StorageError::io(dir.child_path(name), source));
                                    }
                                }
                            }
                            // Links are neither listed nor entered.
                            _ => {}
                        }
                    }
                }
                Ok(keys)
            })
            .await?;

        let mut objects = Vec::new();
        for key in keys {
            if let Ok(info) = self.get_object_metadata(bucket, &key).await {
                objects.push(info);
            }
        }
        objects.sort_by(|left, right| left.key.cmp(&right.key));
        Ok(objects)
    }

    async fn delete_object(&self, bucket: &str, key: &str) -> Result<(), StorageError> {
        let object_rel = self.object_rel_for(bucket, key)?;
        let metadata_rel = self.object_metadata_rel_for(bucket, key)?;
        let key = key.to_string();
        self.blocking(move |backend| {
            // Both are checked before either is removed.
            if let Some(located) = backend.locate_blocking(&object_rel, &key)? {
                located.refuse_link_leaf(&key)?;
            }
            if let Some(located) = backend.locate_blocking(&metadata_rel, &key)? {
                located.refuse_link_leaf(&key)?;
            }
            backend.delete_blocking(&object_rel, &key)?;
            backend.delete_blocking(&metadata_rel, &key)
        })
        .await
    }
}

struct LocalMultipartWriter {
    backend: LocalStorageBackend,
    bucket: String,
    key: String,
    content_type: Option<String>,
    metadata: ObjectMetadata,
    object: Located,
    /// Removes the staged object if the writer is dropped unfinished.
    temp: TempFile,
    metadata_location: Located,
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

        // The metadata is made durable before the object can appear:
        // listings read the object tree and skip an object that has no
        // metadata. Its file was synced when written; it is renamed into
        // place and its directory synced before the object is renamed, and
        // the object's directory is synced last. A failure, crash or power
        // loss at any step can leave metadata for an object that is not
        // there, which reads report as missing and the next completion for
        // the key replaces, but never the object without its metadata. A
        // failure or cancellation before each rename removes the staged
        // files that remain.
        let metadata_location = Located {
            dir: Arc::clone(&self.metadata_location.dir),
            leaf: self.metadata_location.leaf.clone(),
        };
        let key = self.key.clone();
        let from = self.metadata_temp.name().to_string();
        let from_path = self.metadata_temp.path().to_path_buf();
        self.backend
            .blocking(move |_| publish_renamed(&metadata_location, &from, &from_path, &key))
            .await?;
        self.metadata_temp.mark_published();
        let metadata_dir = Arc::clone(&self.metadata_location.dir);
        self.backend
            .blocking(move |_| sync_dir(&metadata_dir))
            .await?;

        let object_location = Located {
            dir: Arc::clone(&self.object.dir),
            leaf: self.object.leaf.clone(),
        };
        let key = self.key.clone();
        let from = self.temp.name().to_string();
        let from_path = self.temp.path().to_path_buf();
        self.backend
            .blocking(move |_| publish_renamed(&object_location, &from, &from_path, &key))
            .await?;
        self.temp.mark_published();
        let object_dir = Arc::clone(&self.object.dir);
        self.backend
            .blocking(move |_| sync_dir(&object_dir))
            .await?;

        Ok(object)
    }

    async fn abort(self: Box<Self>) -> Result<(), StorageError> {
        let writer = *self;
        drop(writer.file);
        let temp = writer.temp;
        let metadata_temp = writer.metadata_temp;
        writer
            .backend
            .blocking(move |_| {
                for staged in [&temp, &metadata_temp] {
                    match staged.dir().remove_file(staged.name()) {
                        Ok(()) => {}
                        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                        Err(source) => return Err(StorageError::io(staged.path(), source)),
                    }
                }
                let (mut temp, mut metadata_temp) = (temp, metadata_temp);
                temp.mark_published();
                metadata_temp.mark_published();
                Ok(())
            })
            .await
    }
}

/// Renames the staged file `from` over `location`'s leaf, refusing a leaf
/// that is a link.
fn publish_renamed(
    location: &Located,
    from: &str,
    from_path: &Path,
    key: &str,
) -> Result<(), StorageError> {
    location.refuse_link_leaf(key)?;
    let target = location.path();
    location
        .dir
        .rename(from, &location.leaf)
        .map_err(|source| StorageError::io(&target, source))?;
    trace_published(Some(from_path), &target);
    Ok(())
}

impl LocalStorageBackend {
    /// One listing page; see [`ListingWalk`].
    async fn collect_blob_page(
        &self,
        prefix: &str,
        continuation: Option<String>,
        limit: usize,
    ) -> Result<BlobListPage, StorageError> {
        crate::blob::validate_blob_prefix(prefix)?;
        self.check_root_name(prefix)?;
        let prefix = prefix.to_string();
        let (page, work) = self
            .blocking(move |backend| {
                let start_key = listing_start_key(&prefix).to_string();
                let components: Vec<&str> = start_key
                    .split('/')
                    .filter(|name| !name.is_empty())
                    .collect();
                let Some(start) = backend.open_dir_blocking(&components, &prefix)? else {
                    return Ok((BlobListPage::default(), ListingWork::default()));
                };
                let internal = match backend.open_root()? {
                    Some(root) => match root.info(INTERNAL_STORAGE_DIR) {
                        Ok(Some(info)) if info.kind == Kind::Dir => Some(info.identity),
                        _ => None,
                    },
                    None => None,
                };
                ListingWalk {
                    internal,
                    prefix: &prefix,
                    resume: Resume::parse(continuation.as_deref()),
                    limit,
                    work: ListingWork::default(),
                }
                .run(start, start_key.clone())
            })
            .await?;
        #[cfg(test)]
        listing_log::record(&self.root, work);
        #[cfg(not(test))]
        let _ = work;
        Ok(page)
    }
}

/// Directories a listing page may read beyond twice its page size before it
/// returns the keys it has, with a continuation.
const LISTING_SPARE_DIRECTORY_READS: usize = 64;
/// Fewest entries a listing keeps from one directory read.
const LISTING_MIN_BATCH: usize = 32;

/// The work one listing page did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct ListingWork {
    /// Directories read, counting each read of the same directory.
    pub(crate) directory_reads: usize,
    /// Directory entries read across those reads.
    pub(crate) entries_read: usize,
    /// Most entries the walk held at once.
    pub(crate) peak_retained: usize,
}

/// Where a listing continues: after the continuation token of the previous
/// page.
///
/// A token is either the last key a page returned, or, when a page stopped
/// at its work bound after finishing a directory, that directory's key
/// followed by `/`. No key ends in `/`, so the two never collide.
#[derive(Clone, Copy, Debug)]
enum Resume<'a> {
    Start,
    /// Every key up to and including this one was listed.
    AfterKey(&'a str),
    /// Every key before this directory, `name/`, and every key below it was
    /// listed.
    AfterDirectory(&'a str),
}

impl<'a> Resume<'a> {
    fn parse(token: Option<&'a str>) -> Self {
        match token {
            None => Self::Start,
            Some(token) if token.ends_with('/') => Self::AfterDirectory(token),
            Some(token) => Self::AfterKey(token),
        }
    }

    /// Whether an earlier page covered `key`.
    fn passed_key(self, key: &str) -> bool {
        match self {
            Self::Start => false,
            Self::AfterKey(token) => key <= token,
            Self::AfterDirectory(token) => key < token || key.starts_with(token),
        }
    }

    /// Whether an earlier page covered every key below `directory`, a
    /// directory key followed by `/`. Every such key starts with it, so the
    /// subtree was covered when it sorts before the position and does not
    /// contain it, or is the finished directory or below it.
    fn passed_directory(self, directory: &str) -> bool {
        let before = |token: &str| directory < token && !token.starts_with(directory);
        match self {
            Self::Start => false,
            Self::AfterKey(token) => before(token),
            Self::AfterDirectory(token) => before(token) || directory.starts_with(token),
        }
    }
}

/// A directory entry a listing may still visit.
struct ListedChild {
    /// The entry's name, followed by `/` for a directory: the order in which
    /// a walk visits siblings so that keys come out in byte order. Every key
    /// below a directory `name` starts with `name/`, so `a-b` (whose bytes
    /// sort below `/`) comes before the keys below `a`.
    order: Vec<u8>,
    name: String,
    key: String,
    is_dir: bool,
}

impl PartialEq for ListedChild {
    fn eq(&self, other: &Self) -> bool {
        self.order == other.order
    }
}

impl Eq for ListedChild {}

impl PartialOrd for ListedChild {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ListedChild {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.order.cmp(&other.order)
    }
}

/// A directory on the listing's current path.
struct ListingFrame {
    dir: Dir,
    /// The directory's key; empty for the root.
    key: String,
    /// The smallest children not yet visited, largest first.
    batch: Vec<ListedChild>,
    /// Whether the directory was read at least once.
    scanned: bool,
    /// Whether `batch` holds every child left to visit.
    complete: bool,
    /// The order of the last child visited; a re-read keeps only later ones.
    floor: Option<Vec<u8>>,
}

impl ListingFrame {
    fn new(dir: Dir, key: String) -> Self {
        Self {
            dir,
            key,
            batch: Vec::new(),
            scanned: false,
            complete: false,
            floor: None,
        }
    }
}

/// One page of a local listing.
///
/// A depth-first walk from the prefix's directory that visits each
/// directory's children in key order, so keys come out in byte order. Every
/// directory is opened relative to its parent's held handle without
/// following links, and links are neither listed nor entered. It resumes at
/// the continuation without revisiting what earlier pages covered: a
/// subtree that sorts entirely before the position is skipped from its
/// parent's entry, without being read, and only the directories on the path
/// to the position are read again.
///
/// Memory is bounded by the page; work is bounded by the page and by the
/// size of the directories it reads:
///
/// - A directory read keeps only the smallest children still to visit, at
///   least the rest of the page plus one and at least [`LISTING_MIN_BATCH`],
///   and reads the directory again for later ones if the page needs them.
///   The walk holds at most that many entries per directory on its path,
///   however large a directory is.
/// - Each read still reads every entry of the directory, since the file
///   system returns entries in no particular order. A page's work therefore
///   grows with the size of the largest directory it reads: a directory with
///   `n` entries costs `n` entries per page that passes through it.
/// - Entry types come from the directory read; no entry is inspected
///   separately except on a file system that does not report types.
/// - After `2 * limit +` [`LISTING_SPARE_DIRECTORY_READS`] directory reads,
///   as when many directories hold no keys, the page ends with the keys it
///   found, possibly none, and a continuation. Every page makes progress.
struct ListingWalk<'a> {
    /// The identity of the internal directory, which is never entered under
    /// any name.
    internal: Option<(u64, u64)>,
    prefix: &'a str,
    resume: Resume<'a>,
    limit: usize,
    work: ListingWork,
}

impl ListingWalk<'_> {
    fn run(
        mut self,
        start: Dir,
        start_key: String,
    ) -> Result<(BlobListPage, ListingWork), StorageError> {
        let budget = self
            .limit
            .saturating_mul(2)
            .saturating_add(LISTING_SPARE_DIRECTORY_READS);
        let mut keys: Vec<String> = Vec::new();
        // How far this page got: the last key it listed, or a directory it
        // finished, followed by `/`.
        let mut reached: Option<String> = None;
        let mut next_continuation = None;
        let mut stack = vec![ListingFrame::new(start, start_key)];

        while let Some(frame) = stack.last_mut() {
            if let Some(child) = frame.batch.pop() {
                frame.floor = Some(child.order);
                if child.is_dir {
                    match frame.dir.child_dir(&child.name) {
                        Ok(Some(dir)) => stack.push(ListingFrame::new(dir, child.key)),
                        // Removed, or replaced by a link, since it was read.
                        Ok(None) => {}
                        Err(err) if local_fs::is_refusal(&err) => {}
                        Err(source) => {
                            return Err(StorageError::io(
                                frame.dir.child_path(&child.name),
                                source,
                            ));
                        }
                    }
                } else if keys.len() == self.limit {
                    // Another key exists after a full page.
                    next_continuation = keys.last().cloned();
                    break;
                } else {
                    reached = Some(child.key.clone());
                    keys.push(child.key);
                }
                continue;
            }
            if frame.scanned && frame.complete {
                let finished = stack.pop().expect("a frame");
                reached = Some(format!("{}/", finished.key));
                continue;
            }
            if self.work.directory_reads >= budget && reached.is_some() {
                next_continuation = reached;
                break;
            }
            let keep = (self.limit + 1)
                .saturating_sub(keys.len())
                .max(LISTING_MIN_BATCH);
            self.scan(frame, keep)?;
            let retained = stack.iter().map(|frame| frame.batch.len()).sum();
            self.work.peak_retained = self.work.peak_retained.max(retained);
        }

        Ok((
            BlobListPage {
                keys,
                next_continuation,
            },
            self.work,
        ))
    }

    /// Reads `frame`'s directory, keeping its `keep` smallest children still
    /// to visit.
    fn scan(&mut self, frame: &mut ListingFrame, keep: usize) -> Result<(), StorageError> {
        frame.scanned = true;
        frame.complete = true;
        self.work.directory_reads += 1;
        let mut smallest = std::collections::BinaryHeap::with_capacity(keep.saturating_add(1));
        let mut overflowed = false;
        let mut failure = None;
        let walk = &mut *self;
        let result = frame.dir.for_each_entry(|entry| {
            walk.work.entries_read += 1;
            match walk.child(frame, &entry) {
                Ok(Some(child)) => {
                    smallest.push(child);
                    if smallest.len() > keep {
                        smallest.pop();
                        overflowed = true;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    failure = Some(error);
                    return Err(std::io::Error::other("listing stopped"));
                }
            }
            Ok(())
        });
        if let Some(error) = failure {
            return Err(error);
        }
        match result {
            Ok(()) => {}
            // Removed since its parent was read: nothing below it is listed.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(StorageError::io(frame.dir.path(), source)),
        }
        frame.complete = !overflowed;
        let mut batch = smallest.into_sorted_vec();
        batch.reverse();
        frame.batch = batch;
        Ok(())
    }

    /// The child `entry` of `frame` when the listing may still visit it.
    ///
    /// Skips links, entries that are neither files nor directories, names
    /// that are not valid keys (with everything below them, since every key
    /// there would contain the same component), temporary upload files, the
    /// internal directory under any name, entries outside the prefix, and
    /// entries an earlier page or read covered.
    fn child(
        &self,
        frame: &ListingFrame,
        entry: &local_fs::Entry,
    ) -> Result<Option<ListedChild>, StorageError> {
        let Some(name) = entry.name.to_str() else {
            return Ok(None);
        };
        if cfg!(windows) && name.contains(':') {
            return Ok(None);
        }
        let is_dir = match entry.kind {
            Kind::Dir => true,
            Kind::File => false,
            Kind::Link | Kind::Other => return Ok(None),
        };
        let mut order = name.as_bytes().to_vec();
        if is_dir {
            order.push(b'/');
        }
        if frame.floor.as_ref().is_some_and(|floor| order <= *floor) {
            return Ok(None);
        }
        let key = if frame.key.is_empty() {
            name.to_string()
        } else {
            format!("{}/{name}", frame.key)
        };
        if validate_blob_key(&key).is_err() {
            return Ok(None);
        }
        if is_dir {
            if !directory_may_match(&key, self.prefix)
                || self.resume.passed_directory(&format!("{key}/"))
            {
                return Ok(None);
            }
            // A root entry that resolves to the internal directory under
            // another name is never entered.
            if frame.key.is_empty()
                && let Some(internal) = self.internal
                && frame
                    .dir
                    .info(name)
                    .ok()
                    .flatten()
                    .is_some_and(|info| info.identity == internal)
            {
                return Ok(None);
            }
        } else if is_temp_file_name(name)
            || !key.starts_with(self.prefix)
            || self.resume.passed_key(&key)
        {
            return Ok(None);
        }
        Ok(Some(ListedChild {
            order,
            name: name.to_string(),
            key,
            is_dir,
        }))
    }
}

/// The key of the directory a listing for `prefix` starts from: the prefix
/// up to its last `/`. Every key the prefix matches lies below it.
fn listing_start_key(prefix: &str) -> &str {
    match prefix.rfind('/') {
        Some(index) => &prefix[..index],
        None => "",
    }
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
    /// Opens the directory `components` below the root, creating what is
    /// missing, and makes its entry, and each ancestor's entry up to the
    /// root, durable before returning it.
    ///
    /// Every component is opened relative to its parent's held handle
    /// without following links, and created there when missing, so a link
    /// anywhere on the way is refused and nothing is ever created outside
    /// the root. The root is created by its path when it is missing.
    ///
    /// Each directory from the deepest upward has its entry synced in its
    /// parent until the walk reaches the root or a directory this backend
    /// remembers as durable. A directory that existed is synced too, because
    /// another writer may have created it and not synced it yet. A directory
    /// found missing is forgotten before it is created, so a concurrent call
    /// cannot trust it before this call has synced its entry. The root's own
    /// entry is synced only when this call created the root. Directories are
    /// remembered only after their whole chain synced, so a failed sync is
    /// retried by the next write.
    ///
    /// The memory assumes nothing outside this backend removes directories
    /// under the root while it runs: a remembered directory removed and
    /// recreated elsewhere, and still present when checked, remains trusted.
    pub(crate) fn prepare_dir_blocking(
        &self,
        components: &[&str],
        key: &str,
    ) -> Result<Arc<Dir>, StorageError> {
        let mut root_created = false;
        let root = match self.open_root()? {
            Some(root) => root,
            None => {
                std::fs::create_dir_all(&self.root)
                    .map_err(|source| StorageError::io(&self.root, source))?;
                root_created = true;
                self.open_root()?
                    .ok_or_else(|| StorageError::MissingParent {
                        path: self.root.clone(),
                    })?
            }
        };
        let mut chain: Vec<(Dir, bool)> = vec![(root, root_created)];
        for name in components {
            let parent = &chain.last().expect("the root").0;
            let path = parent.child_path(name);
            let opened = parent
                .child_dir(name)
                .map_err(|error| fs_error(error, key, &path))?;
            let (child, created) = match opened {
                Some(child) => (child, false),
                None => {
                    self.forget_durable(&path);
                    let created = parent
                        .create_dir(name)
                        .map_err(|source| StorageError::io(&path, source))?;
                    let child = parent
                        .child_dir(name)
                        .map_err(|error| fs_error(error, key, &path))?
                        .ok_or_else(|| {
                            StorageError::io(
                                &path,
                                std::io::Error::from(std::io::ErrorKind::NotADirectory),
                            )
                        })?;
                    (child, created)
                }
            };
            chain.push((child, created));
        }

        let mut synced = Vec::new();
        for index in (1..chain.len()).rev() {
            let (dir, created) = &chain[index];
            if !created && self.is_durable_dir(dir.path()) {
                break;
            }
            sync_dir(&chain[index - 1].0)?;
            synced.push(dir.path().to_path_buf());
        }
        if root_created {
            sync_dir_blocking(entry_dir(&self.root)?)?;
        }

        let mut durable = self
            .durable_dirs
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if durable.len().saturating_add(synced.len()) > MAX_DURABLE_DIRS {
            durable.clear();
        }
        durable.extend(synced);
        drop(durable);
        Ok(Arc::new(chain.pop().expect("the root").0))
    }

    fn is_durable_dir(&self, dir: &Path) -> bool {
        self.durable_dirs
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .contains(dir)
    }

    fn forget_durable(&self, dir: &Path) {
        self.durable_dirs
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .remove(dir);
    }
}

/// A temporary file a write owns until it publishes it, named
/// `.<uuid>.uploading` in the target's held directory. Dropping the guard
/// before then, as a failed write or a cancelled or dropped write future
/// does, removes the file through the same directory handle, so an
/// abandoned write leaves nothing behind.
pub(crate) struct TempFile {
    dir: Arc<Dir>,
    name: String,
    path: PathBuf,
    /// Set once the file is renamed into place or removed on purpose.
    released: bool,
    /// Set when the guard is dropped while it still owns the file; a blocking
    /// operation still running then removes what it created.
    abandoned: Arc<AtomicBool>,
}

impl TempFile {
    pub(crate) fn new(dir: Arc<Dir>) -> Self {
        let name = temp_file_name();
        let path = dir.child_path(&name);
        Self {
            dir,
            name,
            path,
            released: false,
            abandoned: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The file's path, for diagnostics and tracing only.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn dir(&self) -> &Arc<Dir> {
        &self.dir
    }

    /// Runs `operation`, which may create the file, on the blocking pool. A
    /// blocking task cannot be cancelled, so if this guard is dropped while
    /// it runs, the task removes the file once `operation` returns.
    pub(crate) async fn run_blocking<T, F>(&self, operation: F) -> std::io::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Dir, &str) -> std::io::Result<T> + Send + 'static,
    {
        let dir = Arc::clone(&self.dir);
        let name = self.name.clone();
        let abandoned = Arc::clone(&self.abandoned);
        tokio::task::spawn_blocking(move || {
            let result = operation(&dir, &name);
            // Pairs with the store in `Drop`: either the guard sees this
            // file and removes it, or this task sees the guard gone.
            if abandoned.load(Ordering::SeqCst) {
                let _ = dir.remove_file(&name);
            }
            result
        })
        .await
        .map_err(std::io::Error::other)?
    }

    /// Creates the file for writing.
    pub(crate) async fn create(&self) -> Result<tokio::fs::File, StorageError> {
        self.run_blocking(|dir, name| dir.create_new(name))
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
        let dir = Arc::clone(&self.dir);
        let name = self.name.clone();
        let _ = tokio::task::spawn_blocking(move || dir.remove_file(&name)).await;
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
        let _ = self.dir.remove_file(&self.name);
    }
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
#[cfg(test)]
fn is_uploading_temp_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(is_temp_file_name)
}

fn is_older_than(modified: Option<SystemTime>, now: SystemTime, older_than: Duration) -> bool {
    modified
        .and_then(|modified| now.duration_since(modified).ok())
        .is_some_and(|age| age >= older_than)
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

/// Makes a creation, rename, link or removal of an entry in the held
/// directory `dir` durable; see [`sync_dir_blocking`] for the platforms and
/// errors.
pub(crate) fn sync_dir(dir: &Dir) -> Result<(), StorageError> {
    #[cfg(unix)]
    {
        #[cfg(test)]
        if durability_log::take_injected_failure(dir.path()) {
            return Err(StorageError::io(
                dir.path(),
                std::io::Error::from_raw_os_error(5),
            ));
        }
        match dir.sync() {
            Ok(()) => trace(DurabilityStep::DirSynced, dir.path()),
            Err(err) if directory_sync_unsupported(&err) => {}
            Err(source) => return Err(StorageError::io(dir.path(), source)),
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Makes a creation, rename, link or removal of an entry in `dir` durable.
/// Used only for the directory holding the root, which is outside it.
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

/// The directory whose entry names `path`.
fn entry_dir(path: &Path) -> Result<&Path, StorageError> {
    match path.parent() {
        Some(parent) if parent.as_os_str().is_empty() => Ok(Path::new(".")),
        Some(parent) => Ok(parent),
        None => Err(StorageError::MissingParent { path: path.into() }),
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
        let object_path = temp.path().join(
            backend
                .object_rel_for(&object.bucket, &object.key)
                .expect("object path"),
        );
        let metadata_path = temp.path().join(
            backend
                .object_metadata_rel_for(&object.bucket, &object.key)
                .expect("metadata path"),
        );

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

    /// Whether `target` is certainly present after a crash at the end of
    /// `done`: it was published from a file synced beforehand, and its
    /// directory was synced afterwards.
    fn durably_present(done: &[(DurabilityStep, PathBuf)], target: &Path) -> bool {
        let Some(published) = position(done, DurabilityStep::Published, target) else {
            return false;
        };
        let parent = target.parent().expect("parent");
        let source_synced = match published.checked_sub(1).map(|index| &done[index]) {
            Some((DurabilityStep::PublishedFrom, source)) => {
                position(&done[..published], DurabilityStep::FileSynced, source).is_some()
            }
            _ => false,
        };
        source_synced
            && done[published + 1..]
                .iter()
                .any(|(step, path)| *step == DurabilityStep::DirSynced && path == parent)
    }

    /// A crash or power loss at any step of a streamed object's completion
    /// never leaves the object present without its metadata, which listings
    /// would silently omit (#540). A rename may or may not survive a crash
    /// until its directory is synced, so the object may be present as soon
    /// as it is renamed, and its metadata counts only once durable.
    #[tokio::test]
    async fn object_completion_never_exposes_an_object_without_durable_metadata() {
        let temp = TempDir::new().expect("temp dir");
        let backend = LocalStorageBackend::new(temp.path());
        let mut writer = backend
            .create_multipart_object(
                "recordings",
                "sessions/alpha/video.webm",
                None,
                ObjectMetadata::new(),
            )
            .await
            .expect("create writer");
        writer
            .write_chunk(Bytes::from_static(b"payload"))
            .await
            .expect("write");
        let _ = durability_log::take_under(temp.path());
        writer.complete().await.expect("complete");

        let steps = durability_log::take_under(temp.path());
        let object = temp.path().join("recordings/sessions/alpha/video.webm");
        let metadata = temp.path().join(
            ".graphql-orm-storage/object-metadata/recordings/sessions/alpha/video.webm.metadata.json",
        );
        for crash in 0..=steps.len() {
            let done = &steps[..crash];
            let object_may_be_present =
                position(done, DurabilityStep::Published, &object).is_some();
            assert!(
                !object_may_be_present || durably_present(done, &metadata),
                "a crash after step {crash} can keep the object without its metadata: {done:?}"
            );
        }
        assert!(durably_present(&steps, &object), "{steps:?}");
        assert!(durably_present(&steps, &metadata), "{steps:?}");
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

/// Test-only record of the work each local listing page does, by root.
#[cfg(test)]
pub(crate) mod listing_log {
    use std::{
        path::{Path, PathBuf},
        sync::Mutex,
    };

    pub(crate) use super::ListingWork as PageWork;

    static PAGES: Mutex<Vec<(PathBuf, PageWork)>> = Mutex::new(Vec::new());

    pub(crate) fn record(root: &Path, work: PageWork) {
        PAGES
            .lock()
            .expect("listing log")
            .push((root.to_path_buf(), work));
    }

    /// Removes and returns the pages recorded for `root`, in order.
    pub(crate) fn take(root: &Path) -> Vec<PageWork> {
        let mut pages = PAGES.lock().expect("listing log");
        let (mine, others) = pages
            .drain(..)
            .partition(|(recorded, _): &(PathBuf, PageWork)| recorded == root);
        *pages = others;
        mine.into_iter().map(|(_, work)| work).collect()
    }
}

/// Test-only hook that runs once a key's directory is resolved and before
/// the operation uses it, to replace directories while an operation runs.
#[cfg(test)]
pub(crate) mod race_hook {
    use std::{
        path::{Path, PathBuf},
        sync::Mutex,
    };

    type Hook = Box<dyn FnOnce() + Send>;

    static HOOKS: Mutex<Vec<(PathBuf, Hook)>> = Mutex::new(Vec::new());

    /// Runs `hook` the next time an operation under `root` resolves a key.
    pub(crate) fn set(root: &Path, hook: impl FnOnce() + Send + 'static) {
        HOOKS
            .lock()
            .expect("race hooks")
            .push((root.to_path_buf(), Box::new(hook)));
    }

    pub(crate) fn fire(root: &Path) {
        let hook = {
            let mut hooks = HOOKS.lock().expect("race hooks");
            hooks
                .iter()
                .position(|(hooked, _)| hooked == root)
                .map(|index| hooks.remove(index).1)
        };
        if let Some(hook) = hook {
            hook();
        }
    }
}

/// A directory replaced by a link to outside the root after an operation
/// resolved it never redirects the operation: it keeps using the directory
/// handle it holds.
#[cfg(all(test, unix))]
mod race_tests {
    use std::path::{Path, PathBuf};

    use bytes::Bytes;
    use tempfile::TempDir;

    use super::{LocalStorageBackend, race_hook};
    use crate::{BlobPutOptions, BlobStore, StorageByteStream, collect_storage_stream};

    struct Fixture {
        _owned: TempDir,
        root: PathBuf,
        outside: PathBuf,
        backend: LocalStorageBackend,
    }

    async fn fixture() -> Fixture {
        let owned = TempDir::new().expect("temp dir");
        let root = owned.path().join("root");
        let outside = owned.path().join("outside");
        std::fs::create_dir_all(&outside).expect("outside");
        std::fs::write(outside.join("x"), b"outside").expect("outside file");
        let backend = LocalStorageBackend::new(&root);
        backend
            .put_blob(
                "dir/x",
                StorageByteStream::from_bytes(Bytes::from_static(b"inside")),
                BlobPutOptions::default(),
            )
            .await
            .expect("seed");
        Fixture {
            _owned: owned,
            root,
            outside,
            backend,
        }
    }

    /// Once the operation has resolved `dir`, moves it to `moved` and puts a
    /// link to the outside directory in its place.
    fn swap_after_resolution(root: &Path, outside: &Path) {
        let (root, outside) = (root.to_path_buf(), outside.to_path_buf());
        race_hook::set(&root.clone(), move || {
            std::fs::rename(root.join("dir"), root.join("moved")).expect("move directory");
            std::os::unix::fs::symlink(&outside, root.join("dir")).expect("link");
        });
    }

    fn outside_untouched(fixture: &Fixture) {
        let names: Vec<_> = std::fs::read_dir(&fixture.outside)
            .expect("outside")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(names, ["x"], "nothing created outside the root");
        assert_eq!(
            std::fs::read(fixture.outside.join("x")).expect("outside file"),
            b"outside"
        );
    }

    #[tokio::test]
    async fn a_write_keeps_the_directory_it_resolved() {
        let fixture = fixture().await;
        swap_after_resolution(&fixture.root, &fixture.outside);
        fixture
            .backend
            .put_blob(
                "dir/new",
                StorageByteStream::from_bytes(Bytes::from_static(b"written")),
                BlobPutOptions::default(),
            )
            .await
            .expect("the write completes in the directory it resolved");
        outside_untouched(&fixture);
        assert_eq!(
            std::fs::read(fixture.root.join("moved/new")).expect("written"),
            b"written"
        );
    }

    #[tokio::test]
    async fn a_read_keeps_the_directory_it_resolved() {
        let fixture = fixture().await;
        swap_after_resolution(&fixture.root, &fixture.outside);
        let read = fixture.backend.get_blob("dir/x").await.expect("read");
        assert_eq!(
            collect_storage_stream(read.body).await.expect("body"),
            Bytes::from_static(b"inside"),
            "the read never reaches the outside file"
        );
        // A new operation resolves the link and is refused.
        let again = fixture.backend.get_blob("dir/x").await.map(|_| ());
        assert!(
            matches!(again, Err(crate::StorageError::InvalidStorageKey { .. })),
            "{again:?}"
        );
    }

    #[tokio::test]
    async fn a_delete_keeps_the_directory_it_resolved() {
        let fixture = fixture().await;
        swap_after_resolution(&fixture.root, &fixture.outside);
        fixture.backend.delete_blob("dir/x").await.expect("delete");
        outside_untouched(&fixture);
        assert!(!fixture.root.join("moved/x").exists());
    }
}

#[cfg(test)]
mod resume_tests {
    use super::Resume;

    #[test]
    fn a_key_token_resumes_after_that_key() {
        let resume = Resume::parse(Some("a/b"));
        for key in ["a", "a-b", "a/a", "a/b"] {
            assert!(resume.passed_key(key), "{key}");
        }
        for key in ["a/b-c", "a/b0", "a/c", "b"] {
            assert!(!resume.passed_key(key), "{key}");
        }
        // `a/` holds the position; `a-x/` sorts before it; `a/b/` and later
        // directories follow it.
        assert!(resume.passed_directory("a-x/"));
        for directory in ["a/", "a/b/", "a/c/", "b/"] {
            assert!(!resume.passed_directory(directory), "{directory}");
        }
    }

    #[test]
    fn a_directory_token_resumes_after_that_directory() {
        let resume = Resume::parse(Some("a/b/"));
        // Keys before the directory, and every key below it.
        for key in ["a/a", "a/b-c", "a/b.c", "a/b/x", "a/b/x/y"] {
            assert!(resume.passed_key(key), "{key}");
        }
        // `a/b` itself cannot name a file beside the directory `a/b`, and
        // `a/b0` sorts after every key below `a/b/`.
        for key in ["a/b0", "a/c", "b"] {
            assert!(!resume.passed_key(key), "{key}");
        }
        for directory in ["a/a/", "a/b-c/", "a/b/", "a/b/x/"] {
            assert!(resume.passed_directory(directory), "{directory}");
        }
        for directory in ["a/", "a/b0/", "a/c/", "b/"] {
            assert!(!resume.passed_directory(directory), "{directory}");
        }
        assert!(!Resume::parse(None).passed_key("a"));
        assert!(!Resume::parse(None).passed_directory("a/"));
    }
}

#[cfg(test)]
mod listing_work_tests {
    use bytes::Bytes;
    use tempfile::TempDir;

    use super::{LocalStorageBackend, listing_log};
    use crate::{BlobPutOptions, BlobStore, StorageByteStream};

    const LIMIT: usize = 10;
    /// Deepest directory nesting in the fixture, root included.
    const DEPTH: usize = 3;

    async fn put(backend: &LocalStorageBackend, key: &str) {
        backend
            .put_blob(
                key,
                StorageByteStream::from_bytes(Bytes::from_static(b"x")),
                BlobPutOptions::default(),
            )
            .await
            .expect("put");
    }

    /// Pages through the whole store, checking each page's work.
    async fn page_through(backend: &LocalStorageBackend, root: &std::path::Path) -> Vec<String> {
        listing_log::take(root);
        let mut keys = Vec::new();
        let mut continuation = None;
        let mut pages = 0;
        loop {
            let page = backend
                .list_blobs_page("", continuation, LIMIT)
                .await
                .expect("page");
            assert!(page.keys.len() <= LIMIT);
            keys.extend(page.keys);
            pages += 1;
            assert!(pages < 1_000, "the listing makes progress");
            continuation = page.next_continuation;
            let work = listing_log::take(root);
            assert_eq!(work.len(), 1, "one recorded walk per page");
            let work = work[0];
            // Reads are bounded by the page, not by the size of the tree:
            // the resume path, and at most twice the page size plus a fixed
            // allowance of directories that hold no keys.
            assert!(
                work.directory_reads <= 2 * LIMIT + 64 + DEPTH,
                "page {pages} read {} directories",
                work.directory_reads
            );
            // Memory is bounded by the page size and depth, not by the size
            // of any directory.
            assert!(
                work.peak_retained <= DEPTH * (LIMIT + 1).max(32),
                "page {pages} held {} entries",
                work.peak_retained
            );
            if continuation.is_none() {
                break;
            }
        }
        keys
    }

    #[tokio::test]
    async fn each_listing_page_does_bounded_work() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().to_path_buf();
        let backend = LocalStorageBackend::new(&root);
        let mut expected = Vec::new();
        // Many directories that hold no keys, before any key.
        for index in 0..200 {
            std::fs::create_dir_all(root.join(format!("a-empty/e{index:03}"))).expect("dir");
        }
        // A large flat directory.
        for index in 0..300 {
            let key = format!("b-flat/f{index:03}");
            put(&backend, &key).await;
            expected.push(key);
        }
        // A sharded tree.
        for shard in 0..20 {
            for index in 0..10 {
                let key = format!("c-tree/d{shard:02}/k{index:02}");
                put(&backend, &key).await;
                expected.push(key);
            }
        }
        expected.sort();

        let keys = page_through(&backend, &root).await;
        assert_eq!(keys, expected, "every key once, in byte order");
    }

    /// A resumed page reads only the directories on the path to its
    /// position, never the subtrees earlier pages covered.
    #[tokio::test]
    async fn resumed_pages_read_only_the_path_to_their_position() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().to_path_buf();
        let backend = LocalStorageBackend::new(&root);
        for shard in 0..50 {
            for index in 0..LIMIT {
                put(&backend, &format!("tree/d{shard:02}/k{index:02}")).await;
            }
        }
        listing_log::take(&root);
        let mut continuation = None;
        let mut pages = 0;
        loop {
            let page = backend
                .list_blobs_page("tree/", continuation, LIMIT)
                .await
                .expect("page");
            pages += 1;
            assert_eq!(page.keys.len(), LIMIT);
            let work = listing_log::take(&root)[0];
            // The prefix directory, the shard holding the position, the
            // shard the page lists, and the next one for the look-ahead.
            assert!(
                work.directory_reads <= 4,
                "page {pages} read {} directories",
                work.directory_reads
            );
            assert!(work.entries_read <= 50 + 3 * LIMIT, "{work:?}");
            continuation = page.next_continuation;
            if continuation.is_none() {
                break;
            }
        }
        assert_eq!(pages, 50);
    }
}
