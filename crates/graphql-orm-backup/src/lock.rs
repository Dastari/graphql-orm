use bytes::Bytes;

use crate::{BackupError, BackupRepository};

pub const DEFAULT_LOCK_STALE_AFTER_SECONDS: i64 = 3_600;
const REPOSITORY_LOCK_KEY: &str = "locks/repository.lock";

#[derive(Clone, Debug, Eq, PartialEq)]
/// Advisory repository lock settings.
pub struct RepositoryLockOptions {
    /// Legacy age setting, retained for source compatibility and ignored.
    /// Existing locks require offline recovery; elapsed time never grants ownership.
    pub stale_after_seconds: i64,
}

impl Default for RepositoryLockOptions {
    fn default() -> Self {
        Self {
            stale_after_seconds: DEFAULT_LOCK_STALE_AFTER_SECONDS,
        }
    }
}

/// Acquired advisory repository lock.
///
/// This handle cannot be cloned: release consumes the sole ownership handle.
/// It borrows its acquisition repository, so it cannot release another repository.
/// Dropping or cancelling a holder leaves the lock in place. Recovery requires
/// stopping all writers and ensuring no handles remain before removing the blob.
/// No automatic expiry or fencing is provided.
///
/// ```compile_fail
/// # use graphql_orm_backup::RepositoryLock;
/// fn duplicate(lock: RepositoryLock<'_>) {
///     let _second = lock.clone();
/// }
/// ```
pub struct RepositoryLock<'a> {
    repository: &'a dyn BackupRepository,
}

impl std::fmt::Debug for RepositoryLock<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RepositoryLock")
            .finish_non_exhaustive()
    }
}

impl<'a> RepositoryLock<'a> {
    /// Acquires the repository writer lock.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError::RepositoryLocked`] if any lock exists, or
    /// another [`BackupError`] if the repository cannot be read or written.
    pub async fn acquire(
        repository: &'a dyn BackupRepository,
        _options: &RepositoryLockOptions,
    ) -> Result<Self, BackupError> {
        let now = unix_seconds();
        let body = Bytes::from(now.to_string());
        if repository
            .put_blob_if_absent(REPOSITORY_LOCK_KEY, body)
            .await?
        {
            return Ok(Self { repository });
        }

        // No provider-independent atomic compare/delete or fencing primitive is
        // available. Never remove an existing lock, regardless of its age or
        // readability: its writer may still be running.
        Err(BackupError::RepositoryLocked {
            lock_key: REPOSITORY_LOCK_KEY.to_string(),
        })
    }

    /// Releases the repository writer lock through single-attempt deletion.
    ///
    /// A failed or cancelled release leaves its outcome unknown. Do not retry
    /// deletion by key or reclaim the lock while writers or requests can remain
    /// active. The handle is consumed even when release fails.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the repository cannot delete the lock blob.
    pub async fn release(self) -> Result<(), BackupError> {
        self.repository.delete_blob_once(REPOSITORY_LOCK_KEY).await
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
