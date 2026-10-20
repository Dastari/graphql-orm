//! Local filesystem capacity observation.
//!
//! On Unix the storage root itself is opened and queried with `fstatvfs`, so
//! the observation describes the filesystem that actually holds the root. The
//! unprivileged available block count excludes reserved blocks, and
//! filesystems that implement directory-tree project quotas in `statfs` (for
//! example ext4 and XFS) already report them there. On Linux the effective
//! user's quota and the quota of the group that new files receive are read
//! with `quotactl_fd` on the same descriptor.

use std::path::{Path, PathBuf};

use crate::{
    CapacityBinding, CapacityObservation, CapacityPlacement, CapacityRequest, CapacityState,
    QuotaLimit, QuotaScope, StorageBackend, StorageError,
};

#[cfg(unix)]
use crate::CapacityQuota;

/// Linux reports quota block limits in 1 KiB units (`QIF_DQBLKSIZE`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const QUOTA_BLOCK_BYTES: u64 = 1024;

pub(crate) fn binding(root: &Path) -> CapacityBinding {
    let root = root.to_string_lossy();
    let account = account_identity();
    CapacityBinding::new(
        StorageBackend::Local,
        &[("root", root.as_ref()), ("account", account.as_str())],
    )
}

pub(crate) async fn observe(
    root: PathBuf,
    binding: CapacityBinding,
    request: CapacityRequest,
) -> Result<CapacityObservation, StorageError> {
    let (placement, state) = tokio::task::spawn_blocking(move || sample(&root))
        .await
        .map_err(|error| StorageError::Provider {
            backend: StorageBackend::Local.as_str().to_string(),
            message: format!("capacity observation task failed: {error}"),
            retryable: true,
        })??;
    Ok(CapacityObservation::new(binding, placement, state, &request))
}

#[cfg(unix)]
fn account_identity() -> String {
    // SAFETY: geteuid and getegid have no preconditions and cannot fail.
    let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
    format!("uid={uid};gid={gid}")
}

#[cfg(not(unix))]
fn account_identity() -> String {
    "process".to_string()
}

#[cfg(unix)]
fn sample(root: &Path) -> Result<(Option<CapacityPlacement>, CapacityState), StorageError> {
    use std::os::{fd::AsRawFd, unix::fs::MetadataExt};

    use crate::KnownCapacity;

    let directory = std::fs::File::open(root).map_err(|source| StorageError::io(root, source))?;
    let metadata = directory
        .metadata()
        .map_err(|source| StorageError::io(root, source))?;
    ensure_directory(root, &metadata)?;

    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: the descriptor stays open for the call and `stats` is a valid
    // out pointer for one `statvfs` structure.
    let result = unsafe { libc::fstatvfs(directory.as_raw_fd(), stats.as_mut_ptr()) };
    if result != 0 {
        return Err(StorageError::io(root, std::io::Error::last_os_error()));
    }
    // SAFETY: fstatvfs succeeded and initialised the structure.
    let stats = unsafe { stats.assume_init() };

    let fragment = match widen(stats.f_frsize) {
        0 => widen(stats.f_bsize),
        fragment => fragment,
    };
    let total = widen(stats.f_blocks).saturating_mul(fragment);
    let physical_free = widen(stats.f_bfree).saturating_mul(fragment);
    let unprivileged_free = widen(stats.f_bavail).saturating_mul(fragment);
    // SAFETY: geteuid has no preconditions and cannot fail.
    let privileged = unsafe { libc::geteuid() } == 0;
    let filesystem_available = if privileged {
        physical_free
    } else {
        unprivileged_free.min(physical_free)
    };

    let quota = observe_quota(&directory, &metadata);
    let caller_available = match &quota {
        CapacityQuota::Limited(limits) => limits
            .iter()
            .map(QuotaLimit::remaining_bytes)
            .fold(filesystem_available, u64::min),
        _ => filesystem_available,
    };
    let placement = CapacityPlacement(format!(
        "local:dev={:x};fsid={:x}",
        metadata.dev(),
        widen(stats.f_fsid)
    ));
    let state = CapacityState::Known(KnownCapacity {
        caller_available_bytes: caller_available,
        total_bytes: Some(total),
        physical_available_bytes: Some(physical_free),
        quota,
    });
    Ok((Some(placement), state))
}

#[cfg(not(unix))]
fn sample(root: &Path) -> Result<(Option<CapacityPlacement>, CapacityState), StorageError> {
    use crate::{CapacityUnknownReason, UnknownCapacity};

    let metadata = std::fs::metadata(root).map_err(|source| StorageError::io(root, source))?;
    ensure_directory(root, &metadata)?;
    let state = CapacityState::Unknown(UnknownCapacity {
        reason: CapacityUnknownReason::PlatformUnsupported,
    });
    Ok((None, state))
}

fn ensure_directory(root: &Path, metadata: &std::fs::Metadata) -> Result<(), StorageError> {
    if metadata.is_dir() {
        return Ok(());
    }
    Err(StorageError::io(
        root,
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "local storage root is not a directory",
        ),
    ))
}

#[cfg(unix)]
fn widen<T: TryInto<u64>>(value: T) -> u64 {
    value.try_into().unwrap_or(u64::MAX)
}

#[cfg(target_os = "linux")]
fn observe_quota(directory: &std::fs::File, metadata: &std::fs::Metadata) -> CapacityQuota {
    use std::os::unix::fs::MetadataExt;

    const SET_GROUP_ID: u32 = 0o2000;

    // SAFETY: geteuid and getegid have no preconditions and cannot fail.
    let (uid, egid) = unsafe { (libc::geteuid(), libc::getegid()) };
    // New files take the directory's group on set-group-ID directories.
    let gid = if metadata.mode() & SET_GROUP_ID == 0 {
        egid
    } else {
        metadata.gid()
    };

    let mut limits = Vec::new();
    for (scope, kind, principal) in [
        (QuotaScope::User, libc::USRQUOTA, uid),
        (QuotaScope::Group, libc::GRPQUOTA, gid),
    ] {
        match query_quota(directory, kind, principal) {
            Ok(None) => {}
            Ok(Some(sample)) => {
                if let Some(limit) = quota_limit(scope, u64::from(principal), &sample) {
                    limits.push(limit);
                }
            }
            Err(reason) => return CapacityQuota::NotObserved { reason },
        }
    }
    if limits.is_empty() {
        CapacityQuota::NotEnforced
    } else {
        CapacityQuota::Limited(limits)
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn observe_quota(_directory: &std::fs::File, _metadata: &std::fs::Metadata) -> CapacityQuota {
    CapacityQuota::NotObserved {
        reason: "quota observation is not implemented for this platform".to_string(),
    }
}

/// Raw block quota values in the units the kernel reports.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct QuotaSample {
    hard_limit_blocks: u64,
    soft_limit_blocks: u64,
    used_bytes: u64,
}

/// Queries one quota; `Ok(None)` means quotas of that kind are not enabled.
#[cfg(target_os = "linux")]
fn query_quota(
    directory: &std::fs::File,
    kind: libc::c_int,
    principal: u32,
) -> Result<Option<QuotaSample>, String> {
    use std::os::fd::AsRawFd;

    let mut quota = std::mem::MaybeUninit::<libc::dqblk>::zeroed();
    let command = libc::QCMD(libc::Q_GETQUOTA, kind);
    // SAFETY: the descriptor stays open for the call and `quota` is a valid
    // out pointer for the `dqblk` structure Q_GETQUOTA writes.
    let result = unsafe {
        libc::syscall(
            libc::SYS_quotactl_fd,
            directory.as_raw_fd(),
            command,
            principal,
            quota.as_mut_ptr(),
        )
    };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::ESRCH) => Ok(None),
            Some(libc::ENOSYS) => {
                Err("quota interface is unavailable for this kernel or filesystem".to_string())
            }
            Some(libc::EOPNOTSUPP | libc::ENOTTY | libc::EINVAL) => {
                Err("filesystem does not support quota queries".to_string())
            }
            Some(libc::EPERM | libc::EACCES) => {
                Err("caller is not permitted to read its quota".to_string())
            }
            _ => Err(format!("quota query failed: {error}")),
        };
    }
    // SAFETY: quotactl_fd succeeded and initialised the structure.
    let quota = unsafe { quota.assume_init() };
    let required = libc::QIF_BLIMITS | libc::QIF_SPACE;
    if quota.dqb_valid & required != required {
        return Err("quota response omitted block limits or usage".to_string());
    }
    Ok(Some(QuotaSample {
        hard_limit_blocks: quota.dqb_bhardlimit,
        soft_limit_blocks: quota.dqb_bsoftlimit,
        used_bytes: quota.dqb_curspace,
    }))
}

/// Converts a quota sample into a finite limit; `None` means unlimited.
///
/// Exceeding a soft limit starts a grace period after which allocation fails,
/// so admission uses the tighter of the soft and hard limits.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn quota_limit(scope: QuotaScope, principal: u64, sample: &QuotaSample) -> Option<QuotaLimit> {
    let bytes = |blocks: u64| (blocks > 0).then_some(blocks.saturating_mul(QUOTA_BLOCK_BYTES));
    let hard = bytes(sample.hard_limit_blocks);
    let soft = bytes(sample.soft_limit_blocks);
    let limit = match (soft, hard) {
        (Some(soft), Some(hard)) => soft.min(hard),
        (Some(limit), None) | (None, Some(limit)) => limit,
        (None, None) => return None,
    };
    Some(QuotaLimit {
        scope,
        principal,
        used_bytes: sample.used_bytes,
        limit_bytes: limit,
        soft_limit_bytes: soft,
        hard_limit_bytes: hard,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(hard: u64, soft: u64, used: u64) -> QuotaSample {
        QuotaSample {
            hard_limit_blocks: hard,
            soft_limit_blocks: soft,
            used_bytes: used,
        }
    }

    #[test]
    fn zero_limits_are_unlimited() {
        assert_eq!(quota_limit(QuotaScope::User, 7, &sample(0, 0, 4096)), None);
    }

    #[test]
    fn tighter_of_soft_and_hard_limits_is_enforced() {
        let limit = quota_limit(QuotaScope::Group, 9, &sample(200, 100, 1024)).expect("limited");
        assert_eq!(limit.limit_bytes, 100 * QUOTA_BLOCK_BYTES);
        assert_eq!(limit.hard_limit_bytes, Some(200 * QUOTA_BLOCK_BYTES));
        assert_eq!(limit.soft_limit_bytes, Some(100 * QUOTA_BLOCK_BYTES));
        assert_eq!(limit.remaining_bytes(), 99 * QUOTA_BLOCK_BYTES);

        let hard_only = quota_limit(QuotaScope::User, 1, &sample(10, 0, 0)).expect("limited");
        assert_eq!(hard_only.limit_bytes, 10 * QUOTA_BLOCK_BYTES);
        assert_eq!(hard_only.soft_limit_bytes, None);
    }

    #[test]
    fn exhausted_quota_leaves_nothing() {
        let limit = quota_limit(QuotaScope::User, 1, &sample(1, 0, 4096)).expect("limited");
        assert_eq!(limit.remaining_bytes(), 0);
    }

    #[test]
    fn binding_covers_root_and_account() {
        assert_eq!(binding(Path::new("/a")), binding(Path::new("/a")));
        assert_ne!(binding(Path::new("/a")), binding(Path::new("/b")));
    }
}
