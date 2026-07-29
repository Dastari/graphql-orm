use bytes::Bytes;

use crate::{BackupError, BackupRepository};

pub const DEFAULT_LOCK_STALE_AFTER_SECONDS: i64 = 3_600;
const REPOSITORY_LOCK_KEY: &str = "locks/repository.lock";

#[derive(Clone, Debug, Eq, PartialEq)]
/// Advisory repository lock settings.
pub struct RepositoryLockOptions {
    /// Age in seconds after which a lock blob is considered stale.
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
/// Acquired advisory repository lock.
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

        // A provider may report the failed conditional create before the
        // winning writer has finished publishing a readable lock object. SMB,
        // for example, can briefly return STATUS_DELETE_PENDING while the
        // winner still owns its delete-on-close safety handle. Once atomic
        // creation has reported a collision, an unreadable winner must remain
        // locked: its age cannot be proved and deleting it would violate
        // mutual exclusion.
        let existing = match repository.get_blob(REPOSITORY_LOCK_KEY).await {
            Ok(existing) => existing,
            Err(_) => {
                return Err(BackupError::RepositoryLocked {
                    lock_key: REPOSITORY_LOCK_KEY.to_string(),
                });
            }
        };
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

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;

    struct UnreadableConditionalWinner;

    #[async_trait]
    impl BackupRepository for UnreadableConditionalWinner {
        async fn put_blob(&self, key: &str, _body: Bytes) -> Result<(), BackupError> {
            Err(unexpected(key))
        }

        async fn put_blob_if_absent(&self, key: &str, _body: Bytes) -> Result<bool, BackupError> {
            assert_eq!(key, REPOSITORY_LOCK_KEY);
            Ok(false)
        }

        async fn get_blob(&self, key: &str) -> Result<Bytes, BackupError> {
            assert_eq!(key, REPOSITORY_LOCK_KEY);
            Err(BackupError::Database {
                message: "winning lock is not readable yet".to_string(),
            })
        }

        async fn blob_exists(&self, key: &str) -> Result<bool, BackupError> {
            Err(unexpected(key))
        }

        async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, BackupError> {
            Err(unexpected(prefix))
        }

        async fn delete_blob(&self, key: &str) -> Result<(), BackupError> {
            Err(unexpected(key))
        }
    }

    fn unexpected(value: &str) -> BackupError {
        BackupError::Database {
            message: format!("unexpected repository operation for {value}"),
        }
    }

    #[tokio::test]
    async fn unreadable_conditional_winner_remains_locked() {
        let error = RepositoryLock::acquire(
            &UnreadableConditionalWinner,
            &RepositoryLockOptions::default(),
        )
        .await
        .expect_err("an unreadable conditional winner must remain locked");

        assert!(matches!(error, BackupError::RepositoryLocked { .. }));
    }
}
