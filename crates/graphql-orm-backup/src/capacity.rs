//! Limits for compatibility buffering and decoded database payloads.
use crate::BackupError;
use bytes::Bytes;
use futures::StreamExt;
use graphql_orm_storage::StorageByteStream;

/// Maximum bytes accepted at a buffering boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackupByteLimit {
    /// Reject input exceeding this many bytes, before adding it to our buffer.
    Bytes(u64),
    /// Explicitly accept unbounded buffering; callers own the memory risk.
    Unbounded,
}
impl Default for BackupByteLimit {
    fn default() -> Self {
        Self::Bytes(64 * 1024 * 1024)
    }
}
impl BackupByteLimit {
    pub(crate) fn min(self, other: Self) -> Self {
        match (self, other) {
            (Self::Bytes(a), Self::Bytes(b)) => Self::Bytes(a.min(b)),
            (Self::Unbounded, limit) | (limit, Self::Unbounded) => limit,
        }
    }
    pub(crate) fn remaining(self, consumed: u64) -> Result<Self, BackupError> {
        self.check(consumed)?;
        Ok(match self {
            Self::Bytes(limit) => Self::Bytes(limit - consumed),
            Self::Unbounded => Self::Unbounded,
        })
    }

    pub(crate) fn check(self, bytes: u64) -> Result<(), BackupError> {
        if let Self::Bytes(limit_bytes) = self
            && bytes > limit_bytes
        {
            return Err(BackupError::CapacityExceeded { limit_bytes });
        }
        Ok(())
    }
}

/// Repository limits for each payload and database manifest stage.
/// These are not a process RSS or whole-snapshot allocation guarantee.
///
/// Backup, incremental backup and compaction enforce the same limits that
/// restore and verification enforce, using the repository handle's
/// [`crate::BackupRepository::capacity_limits`], so a snapshot that a handle
/// writes is one the same configuration can restore.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BackupCapacityLimits {
    /// Buffered repository bodies and compatibility stream fallbacks (default 64 MiB).
    /// Every stored table or change payload and every manifest must fit.
    pub buffered_bytes: BackupByteLimit,
    /// Decoded database payload bytes (default 256 MiB).
    pub decompressed_bytes: BackupByteLimit,
    /// Aggregate decoded bytes in one full/incremental database stage (default 512 MiB).
    pub stage_bytes: BackupByteLimit,
    /// Bytes of one stored object (default unbounded, because objects stream).
    pub object_bytes: BackupByteLimit,
}
impl Default for BackupCapacityLimits {
    fn default() -> Self {
        Self {
            buffered_bytes: BackupByteLimit::default(),
            decompressed_bytes: BackupByteLimit::Bytes(256 * 1024 * 1024),
            stage_bytes: BackupByteLimit::Bytes(512 * 1024 * 1024),
            object_bytes: BackupByteLimit::Unbounded,
        }
    }
}

/// Which [`BackupCapacityLimits`] field a backup payload would exceed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackupCapacityLimitKind {
    /// [`BackupCapacityLimits::buffered_bytes`]: a stored payload or manifest.
    StoredPayload,
    /// [`BackupCapacityLimits::decompressed_bytes`]: one decoded payload.
    DecodedPayload,
    /// [`BackupCapacityLimits::stage_bytes`]: all decoded payloads of a stage.
    Stage,
    /// [`BackupCapacityLimits::object_bytes`]: one object.
    Object,
}

impl std::fmt::Display for BackupCapacityLimitKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::StoredPayload => "stored payload",
            Self::DecodedPayload => "decoded payload",
            Self::Stage => "database stage",
            Self::Object => "object",
        })
    }
}

/// Collects a stream, rejecting oversized chunks before copying them.
/// Provider-owned allocations and the current input chunk already exist.
///
/// # Errors
/// Returns a stream error or [`BackupError::CapacityExceeded`].
pub async fn collect_backup_stream(
    body: StorageByteStream,
    limit: BackupByteLimit,
) -> Result<Bytes, BackupError> {
    if let Some(size) = body.size_hint() {
        limit.check(size)?;
    }
    let mut stream = body.into_inner();
    let mut buffer = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let size = u64::try_from(buffer.len())
            .unwrap_or(u64::MAX)
            .checked_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX))
            .ok_or(BackupError::CapacityExceeded {
                limit_bytes: u64::MAX,
            })?;
        limit.check(size)?;
        buffer.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(buffer))
}
