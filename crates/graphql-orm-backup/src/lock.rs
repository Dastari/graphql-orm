use bytes::Bytes;

use crate::{BackupError, BackupRepository};

pub const DEFAULT_LOCK_STALE_AFTER_SECONDS: i64 = 3_600;
const REPOSITORY_LOCK_KEY: &str = "locks/repository.lock";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryLockOptions {
    pub stale_after_seconds: i64,
}

impl Default for RepositoryLockOptions {
    fn default() -> Self {
        Self {
            stale_after_seconds: DEFAULT_LOCK_STALE_AFTER_SECONDS,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryLock {
    key: String,
}

impl RepositoryLock {
    /// Acquires the repository writer lock.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError::RepositoryLocked`] if a non-stale lock exists, or
    /// another [`BackupError`] if the repository cannot be read or written.
    pub async fn acquire(
        repository: &dyn BackupRepository,
        options: &RepositoryLockOptions,
    ) -> Result<Self, BackupError> {
        let now = unix_seconds();
        let body = Bytes::from(now.to_string());
        if repository
            .put_blob_if_absent(REPOSITORY_LOCK_KEY, body.clone())
            .await?
        {
            return Ok(Self {
                key: REPOSITORY_LOCK_KEY.to_string(),
            });
        }

        let existing = repository.get_blob(REPOSITORY_LOCK_KEY).await?;
        let locked_at = std::str::from_utf8(&existing)
            .ok()
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(now);
        if now.saturating_sub(locked_at) <= options.stale_after_seconds {
            return Err(BackupError::RepositoryLocked {
                lock_key: REPOSITORY_LOCK_KEY.to_string(),
            });
        }

        repository.delete_blob(REPOSITORY_LOCK_KEY).await?;
        if repository
            .put_blob_if_absent(REPOSITORY_LOCK_KEY, body)
            .await?
        {
            Ok(Self {
                key: REPOSITORY_LOCK_KEY.to_string(),
            })
        } else {
            Err(BackupError::RepositoryLocked {
                lock_key: REPOSITORY_LOCK_KEY.to_string(),
            })
        }
    }

    /// Releases the repository writer lock.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the repository cannot delete the lock blob.
    pub async fn release(self, repository: &dyn BackupRepository) -> Result<(), BackupError> {
        repository.delete_blob(&self.key).await
    }
}

fn unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}
