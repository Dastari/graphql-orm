use bytes::Bytes;
use uuid::Uuid;

use crate::{BackupByteLimit, BackupError, BackupRepository, collect_backup_stream};

pub const DEFAULT_LOCK_STALE_AFTER_SECONDS: i64 = 3_600;
/// Repository key of the advisory repository lock.
pub const REPOSITORY_LOCK_KEY: &str = "locks/repository.lock";
/// Largest lock record that is read; a larger blob is not an owner record.
const LOCK_RECORD_LIMIT: u64 = 64 * 1024;
const MAX_HOST_NAME_CHARS: usize = 255;

#[derive(Clone, Debug, Eq, PartialEq)]
/// Advisory repository lock settings.
pub struct RepositoryLockOptions {
    /// Legacy age setting, retained for source compatibility and ignored.
    /// Elapsed time never grants ownership; see [`break_repository_lock`].
    pub stale_after_seconds: i64,
    /// Host-supplied label recorded in the owner record, such as a job id.
    pub owner_label: Option<String>,
}

impl Default for RepositoryLockOptions {
    fn default() -> Self {
        Self {
            stale_after_seconds: DEFAULT_LOCK_STALE_AFTER_SECONDS,
            owner_label: None,
        }
    }
}

/// Owner record stored in the repository lock blob.
///
/// The record identifies a holder for operators. It is not authenticated: a
/// party that can write the repository can also write or remove locks.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryLockOwner {
    /// Random id of the run that holds the lock.
    pub run_id: Uuid,
    /// Best-effort host name of the holder.
    pub host: String,
    /// Process id of the holder on `host`.
    pub process_id: u32,
    /// Operation that acquired the lock, such as `full_backup` or `restore`.
    pub operation: String,
    /// Host-supplied [`RepositoryLockOptions::owner_label`].
    pub label: Option<String>,
    /// Acquisition time as UTC Unix seconds, from the holder's clock.
    pub acquired_at: i64,
}

impl std::fmt::Display for RepositoryLockOwner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "run {} ({}) on host {} pid {}, acquired at {}",
            self.run_id, self.operation, self.host, self.process_id, self.acquired_at
        )?;
        if let Some(label) = &self.label {
            write!(formatter, ", label {label}")?;
        }
        Ok(())
    }
}

/// State of the repository lock, from [`read_repository_lock`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RepositoryLockStatus {
    /// No lock blob exists.
    Unlocked,
    /// The lock holds an owner record.
    Owned(RepositoryLockOwner),
    /// A lock exists without a readable owner record, such as one written by an
    /// earlier release. [`break_repository_lock`] refuses it; it can only be
    /// removed offline.
    Unidentified,
}

/// Explicit operator request to remove a lock left by a crashed run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BreakRepositoryLockRequest {
    /// Run id from the owner record, of a run the operator confirmed has
    /// stopped.
    pub expected_run_id: Uuid,
    /// Refuse unless the lock was acquired at least this many seconds ago by
    /// this host's clock. Must not be negative.
    pub min_age_seconds: i64,
}

/// Reads the repository lock without changing it.
///
/// # Errors
///
/// Returns [`BackupError`] if the lock blob exists but cannot be read.
pub async fn read_repository_lock(
    repository: &dyn BackupRepository,
) -> Result<RepositoryLockStatus, BackupError> {
    let body = match repository.get_blob_stream(REPOSITORY_LOCK_KEY).await {
        Ok(body) => body,
        Err(BackupError::MissingBlob { .. }) => return Ok(RepositoryLockStatus::Unlocked),
        Err(error) => return Err(error),
    };
    let bytes = match collect_backup_stream(body, BackupByteLimit::Bytes(LOCK_RECORD_LIMIT)).await {
        Ok(bytes) => bytes,
        Err(BackupError::MissingBlob { .. }) => return Ok(RepositoryLockStatus::Unlocked),
        Err(BackupError::CapacityExceeded { .. }) => return Ok(RepositoryLockStatus::Unidentified),
        Err(error) => return Err(error),
    };
    Ok(
        serde_json::from_slice::<RepositoryLockOwner>(&bytes).map_or(
            RepositoryLockStatus::Unidentified,
            RepositoryLockStatus::Owned,
        ),
    )
}

/// Removes a lock left by a crashed or killed run, after checking its owner.
///
/// This is the documented recovery path for a holder that can no longer
/// release its lock. Before calling it, the operator must confirm from the
/// owner record ([`read_repository_lock`], or the owner reported by
/// [`BackupError::RepositoryLocked`]) that the run has stopped and that no
/// other backup, restore, verification, compaction or prune is running
/// against the repository.
///
/// It refuses unless the lock holds an owner record naming
/// `expected_run_id` that is at least `min_age_seconds` old, then removes the
/// lock with single-attempt deletion. The check and the deletion are separate
/// requests, so the owner check guards against breaking the wrong lock, not
/// against a writer that starts concurrently: that is why every writer must be
/// stopped first. Locks without an owner record are refused and need offline
/// removal of [`REPOSITORY_LOCK_KEY`]. Elapsed time never breaks a lock on its
/// own.
///
/// # Errors
///
/// Returns [`BackupError::BreakLockRefused`] if the repository is not locked,
/// the lock has no owner record, names another run or is too young, or
/// `min_age_seconds` is negative. Returns another [`BackupError`] if the lock
/// cannot be read or deleted; a failed deletion leaves its outcome unknown.
pub async fn break_repository_lock(
    repository: &dyn BackupRepository,
    request: &BreakRepositoryLockRequest,
) -> Result<RepositoryLockOwner, BackupError> {
    let refuse = |reason: String| BackupError::BreakLockRefused {
        lock_key: REPOSITORY_LOCK_KEY.to_string(),
        reason,
    };
    if request.min_age_seconds < 0 {
        return Err(refuse("min_age_seconds must not be negative".to_string()));
    }
    let owner = match read_repository_lock(repository).await? {
        RepositoryLockStatus::Unlocked => {
            return Err(refuse("the repository is not locked".to_string()));
        }
        RepositoryLockStatus::Unidentified => {
            return Err(refuse(
                "the lock has no owner record; stop every writer and remove it offline".to_string(),
            ));
        }
        RepositoryLockStatus::Owned(owner) => owner,
    };
    if owner.run_id != request.expected_run_id {
        return Err(refuse(format!(
            "the lock is held by {owner}, not run {}",
            request.expected_run_id
        )));
    }
    let age = unix_seconds().saturating_sub(owner.acquired_at);
    if age < request.min_age_seconds {
        return Err(refuse(format!(
            "the lock is {age} seconds old; at least {} required",
            request.min_age_seconds
        )));
    }
    repository.delete_blob_once(REPOSITORY_LOCK_KEY).await?;
    Ok(owner)
}

/// Acquired advisory repository lock.
///
/// Backups, compaction, pruning, snapshot deletion, restore and verification
/// all hold this one exclusive lock, so no operation can delete blobs that
/// another is reading or writing. The lock blob stores a
/// [`RepositoryLockOwner`] record.
///
/// This handle cannot be cloned: release consumes the sole ownership handle.
/// It borrows its acquisition repository, so it cannot release another repository.
/// Dropping or cancelling a holder leaves the lock in place with its owner
/// record, because release is asynchronous; recover with
/// [`break_repository_lock`]. No automatic expiry or fencing is provided.
///
/// ```compile_fail
/// # use graphql_orm_backup::RepositoryLock;
/// fn duplicate(lock: RepositoryLock<'_>) {
///     let _second = lock.clone();
/// }
/// ```
pub struct RepositoryLock<'a> {
    repository: &'a dyn BackupRepository,
    owner: RepositoryLockOwner,
}

impl std::fmt::Debug for RepositoryLock<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RepositoryLock")
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

impl<'a> RepositoryLock<'a> {
    /// Acquires the repository lock, recording operation `external`.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError::RepositoryLocked`] with the holder's owner record
    /// if any lock exists, or another [`BackupError`] if the repository cannot
    /// be written.
    pub async fn acquire(
        repository: &'a dyn BackupRepository,
        options: &RepositoryLockOptions,
    ) -> Result<Self, BackupError> {
        Self::acquire_for(repository, options, "external").await
    }

    pub(crate) async fn acquire_for(
        repository: &'a dyn BackupRepository,
        options: &RepositoryLockOptions,
        operation: &str,
    ) -> Result<Self, BackupError> {
        let owner = RepositoryLockOwner {
            run_id: Uuid::new_v4(),
            host: local_host_name(),
            process_id: std::process::id(),
            operation: operation.to_string(),
            label: options.owner_label.clone(),
            acquired_at: unix_seconds(),
        };
        let body = Bytes::from(serde_json::to_vec(&owner)?);
        if repository
            .put_blob_if_absent(REPOSITORY_LOCK_KEY, body)
            .await?
        {
            return Ok(Self { repository, owner });
        }

        // No provider-independent atomic compare/delete or fencing primitive is
        // available. Never remove an existing lock, regardless of its age or
        // readability: its holder may still be running. The owner is reported
        // only to help an operator; an unreadable record still means locked.
        let owner = match read_repository_lock(repository).await {
            Ok(RepositoryLockStatus::Owned(owner)) => Some(Box::new(owner)),
            _ => None,
        };
        Err(BackupError::RepositoryLocked {
            lock_key: REPOSITORY_LOCK_KEY.to_string(),
            owner,
        })
    }

    /// Owner record of this lock.
    #[must_use]
    pub fn owner(&self) -> &RepositoryLockOwner {
        &self.owner
    }

    /// Releases the repository lock through single-attempt deletion.
    ///
    /// Release first reads the lock and leaves it in place unless it still
    /// names this run, so a holder whose lock an operator broke cannot remove
    /// a later holder's lock.
    ///
    /// A failed or cancelled release leaves its outcome unknown. Do not retry
    /// deletion by key or reclaim the lock while writers or requests can remain
    /// active. The handle is consumed even when release fails.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError::RepositoryLockNotHeld`] if the lock no longer
    /// names this run, or another [`BackupError`] if the lock cannot be read
    /// or deleted.
    pub async fn release(self) -> Result<(), BackupError> {
        match read_repository_lock(self.repository).await? {
            RepositoryLockStatus::Owned(found) if found.run_id == self.owner.run_id => {}
            status => {
                return Err(BackupError::RepositoryLockNotHeld {
                    lock_key: REPOSITORY_LOCK_KEY.to_string(),
                    run_id: self.owner.run_id,
                    found: match status {
                        RepositoryLockStatus::Owned(found) => Some(Box::new(found)),
                        _ => None,
                    },
                });
            }
        }
        self.repository.delete_blob_once(REPOSITORY_LOCK_KEY).await
    }

    /// Releases the lock after `result`, keeping an operation's own error.
    ///
    /// If the operation succeeded but release failed, the result is lost but
    /// the error says the operation completed and names the run still holding
    /// the lock, and the snapshot it committed, if any.
    pub(crate) async fn finish<T>(
        self,
        result: Result<T, BackupError>,
        committed_snapshot_id: impl FnOnce(&T) -> Option<Uuid>,
    ) -> Result<T, BackupError> {
        let run_id = self.owner.run_id;
        let released = self.release().await;
        match (result, released) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) => Err(error),
            (Ok(value), Err(source)) => Err(BackupError::LockReleaseFailed {
                lock_key: REPOSITORY_LOCK_KEY.to_string(),
                run_id,
                committed_snapshot_id: committed_snapshot_id(&value),
                source: Box::new(source),
            }),
        }
    }
}

fn unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// Best-effort host name for owner records; never fails.
fn local_host_name() -> String {
    static HOST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HOST.get_or_init(|| {
        let from_env = ["HOSTNAME", "COMPUTERNAME"]
            .into_iter()
            .filter_map(|name| std::env::var(name).ok());
        let from_files = ["/proc/sys/kernel/hostname", "/etc/hostname"]
            .into_iter()
            .filter_map(|path| std::fs::read_to_string(path).ok());
        from_env
            .chain(from_files)
            .find_map(|value| clean_host_name(&value))
            .unwrap_or_else(|| "unknown".to_string())
    })
    .clone()
}

fn clean_host_name(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && !value.chars().any(char::is_control))
        .then(|| value.chars().take(MAX_HOST_NAME_CHARS).collect())
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
