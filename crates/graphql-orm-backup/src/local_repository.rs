use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use bytes::Bytes;

use crate::{BackupError, BackupRepository};

#[derive(Clone, Debug)]
pub struct LocalBackupRepository {
    root: PathBuf,
}

impl LocalBackupRepository {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path_for(&self, key: &str) -> Result<PathBuf, BackupError> {
        validate_repository_key(key)?;
        Ok(self.root.join(Path::new(key)))
    }
}

#[async_trait]
impl BackupRepository for LocalBackupRepository {
    async fn put_blob(&self, key: &str, body: Bytes) -> Result<(), BackupError> {
        let path = self.path_for(key)?;
        let parent = path
            .parent()
            .ok_or_else(|| BackupError::InvalidRepositoryKey {
                key: key.to_string(),
            })?;
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| BackupError::io(parent, source))?;

        let temp_path = path.with_extension("uploading");
        tokio::fs::write(&temp_path, body)
            .await
            .map_err(|source| BackupError::io(&temp_path, source))?;
        tokio::fs::rename(&temp_path, &path)
            .await
            .map_err(|source| BackupError::io(&path, source))?;
        Ok(())
    }

    async fn get_blob(&self, key: &str) -> Result<Bytes, BackupError> {
        let path = self.path_for(key)?;
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(Bytes::from(bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Err(BackupError::MissingBlob {
                    key: key.to_string(),
                })
            }
            Err(source) => Err(BackupError::io(&path, source)),
        }
    }

    async fn blob_exists(&self, key: &str) -> Result<bool, BackupError> {
        let path = self.path_for(key)?;
        match tokio::fs::metadata(&path).await {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(BackupError::io(&path, source)),
        }
    }

    async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, BackupError> {
        if !prefix.is_empty() {
            validate_repository_key(prefix)?;
        }

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
                Err(source) => return Err(BackupError::io(&path, source)),
            };

            if metadata.is_file() {
                if let Ok(relative) = path.strip_prefix(&self.root) {
                    result.push(relative.to_string_lossy().replace('\\', "/"));
                }
                continue;
            }

            let mut entries = tokio::fs::read_dir(&path)
                .await
                .map_err(|source| BackupError::io(&path, source))?;
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|source| BackupError::io(&path, source))?
            {
                stack.push(entry.path());
            }
        }

        result.sort();
        Ok(result)
    }

    async fn delete_blob(&self, key: &str) -> Result<(), BackupError> {
        let path = self.path_for(key)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(BackupError::io(&path, source)),
        }
    }
}

fn validate_repository_key(key: &str) -> Result<(), BackupError> {
    if key.is_empty() {
        return Err(BackupError::InvalidRepositoryKey {
            key: key.to_string(),
        });
    }

    let path = Path::new(key);
    if path.is_absolute() {
        return Err(BackupError::InvalidRepositoryKey {
            key: key.to_string(),
        });
    }

    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(BackupError::InvalidRepositoryKey {
                key: key.to_string(),
            });
        }
    }

    Ok(())
}
