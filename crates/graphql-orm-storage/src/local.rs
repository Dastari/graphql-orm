use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;

use crate::{ObjectStorage, StorageBackend, StorageError, StorageObjectBody, StoredObject};

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

    fn path_for(&self, object: &StoredObject) -> Result<PathBuf, StorageError> {
        validate_storage_key(&object.storage_key)?;
        Ok(self.root.join(Path::new(&object.storage_key)))
    }
}

#[async_trait]
impl ObjectStorage for LocalStorageBackend {
    fn backend(&self) -> StorageBackend {
        StorageBackend::Local
    }

    async fn put_object(
        &self,
        object: StoredObject,
        bytes: Vec<u8>,
    ) -> Result<StoredObject, StorageError> {
        let path = self.path_for(&object)?;
        let parent = path
            .parent()
            .ok_or_else(|| StorageError::MissingParent { path: path.clone() })?;
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| StorageError::io(parent, source))?;

        let temp_path = path.with_extension("uploading");
        tokio::fs::write(&temp_path, bytes)
            .await
            .map_err(|source| StorageError::io(&temp_path, source))?;
        tokio::fs::rename(&temp_path, &path)
            .await
            .map_err(|source| StorageError::io(&path, source))?;

        Ok(object)
    }

    async fn get_object(&self, object: &StoredObject) -> Result<StorageObjectBody, StorageError> {
        let path = self.path_for(object)?;
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|source| StorageError::io(&path, source))?;
        Ok(StorageObjectBody {
            object: object.clone(),
            bytes,
        })
    }

    async fn delete_object(&self, object: &StoredObject) -> Result<(), StorageError> {
        let path = self.path_for(object)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(StorageError::io(&path, source)),
        }
    }
}

fn validate_storage_key(key: &str) -> Result<(), StorageError> {
    if key.is_empty()
        || key.contains('\\')
        || key.contains('\0')
        || key
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(StorageError::InvalidStorageKey {
            key: key.to_string(),
        });
    }

    let path = Path::new(key);
    if path.is_absolute() {
        return Err(StorageError::InvalidStorageKey {
            key: key.to_string(),
        });
    }

    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(StorageError::InvalidStorageKey {
                key: key.to_string(),
            });
        }
    }

    Ok(())
}
