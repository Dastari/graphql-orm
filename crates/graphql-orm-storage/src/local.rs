use std::path::{Path, PathBuf};

use async_trait::async_trait;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::{
    BlobBody, BlobMetadata, BlobStore, BlobWriteOutcome, ObjectStorage, StorageBackend,
    StorageByteStream, StorageError, StorageObjectBody, StoredObject, collect_storage_stream,
    validate_blob_key,
};

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

    fn path_for(&self, key: &str) -> Result<PathBuf, StorageError> {
        validate_blob_key(key)?;
        Ok(self.root.join(Path::new(key)))
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
    ) -> Result<BlobWriteOutcome, StorageError> {
        let path = self.path_for(key)?;
        let parent = path
            .parent()
            .ok_or_else(|| StorageError::MissingParent { path: path.clone() })?;
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| StorageError::io(parent, source))?;

        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| StorageError::InvalidStorageKey {
                key: key.to_string(),
            })?;
        let temp_path = path.with_file_name(format!("{file_name}.{}.uploading", Uuid::new_v4()));

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

    async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        crate::blob::validate_blob_prefix(prefix)?;

        let start = if prefix.is_empty() {
            self.root.clone()
        } else {
            self.root.join(prefix)
        };

        let mut result = Vec::new();
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
                if let Ok(relative) = path.strip_prefix(&self.root) {
                    result.push(relative.to_string_lossy().replace('\\', "/"));
                }
                continue;
            }

            let mut entries = tokio::fs::read_dir(&path)
                .await
                .map_err(|source| StorageError::io(&path, source))?;
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|source| StorageError::io(&path, source))?
            {
                stack.push(entry.path());
            }
        }

        result.sort();
        Ok(result)
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
        self.put_blob(&object.storage_key, StorageByteStream::from_bytes(bytes))
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

fn is_uploading_temp_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".uploading"))
}
