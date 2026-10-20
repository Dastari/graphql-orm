//! Local filesystem capacity observation.
//!
//! On Unix the storage root itself is opened and queried with `fstatvfs`, so
//! the observation describes the filesystem that actually holds the root.
//! Unprivileged callers receive the available block count, which excludes
//! reserved blocks. On Linux the effective user's quota, the quota of the
//! group that new files receive, and the project quota of the root directory
//! are read with `quotactl_fd` on the same descriptor. Every constraint that
//! cannot be read is reported as `CapacityQuota::NotObserved`.

use std::path::{Path, PathBuf};

use time::OffsetDateTime;

use crate::{
    CapacityBinding, CapacityObservation, CapacityPlacement, CapacityRequest, CapacityState,
    QuotaLimit, QuotaScope, StorageBackend, StorageError,
};

#[cfg(unix)]
use crate::CapacityQuota;
#[cfg(target_os = "linux")]
use linux::observe_quota;

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
    // Freshness is measured from before the filesystem is queried.
    let observed_at = OffsetDateTime::now_utc();
    let (placement, state) = tokio::task::spawn_blocking(move || sample(&root))
        .await
        .map_err(|error| StorageError::Provider {
            backend: StorageBackend::Local.as_str().to_string(),
            message: format!("capacity observation task failed: {error}"),
            retryable: true,
        })??;
    Ok(CapacityObservation::new(binding, placement, state, observed_at, &request))
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

/// Block counts reported by `statvfs`, in fragment units.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(unix), allow(dead_code))]
struct BlockCounts {
    fragment_bytes: u64,
    blocks: u64,
    free: u64,
    available: u64,
}

/// Filesystem totals in bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(unix), allow(dead_code))]
struct FilesystemBytes {
    total: u64,
    physical_free: u64,
    caller_free: u64,
}

/// Converts block counts with checked arithmetic and rejects inconsistent
/// reports instead of guessing.
#[cfg_attr(not(unix), allow(dead_code))]
fn filesystem_bytes(
    counts: BlockCounts,
    privileged: bool,
) -> Result<FilesystemBytes, &'static str> {
    if counts.fragment_bytes == 0 {
        return Err("filesystem reported a zero block size");
    }
    if counts.free > counts.blocks || counts.available > counts.free {
        return Err("filesystem reported inconsistent block counts");
    }
    let bytes = |blocks: u64| {
        blocks
            .checked_mul(counts.fragment_bytes)
            .ok_or("filesystem capacity overflows 64 bits")
    };
    let total = bytes(counts.blocks)?;
    let physical_free = bytes(counts.free)?;
    let caller_free = if privileged {
        physical_free
    } else {
        bytes(counts.available)?
    };
    Ok(FilesystemBytes {
        total,
        physical_free,
        caller_free,
    })
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

    let fragment_bytes = match widen(stats.f_frsize) {
        0 => widen(stats.f_bsize),
        fragment => fragment,
    };
    let counts = BlockCounts {
        fragment_bytes,
        blocks: widen(stats.f_blocks),
        free: widen(stats.f_bfree),
        available: widen(stats.f_bavail),
    };
    // SAFETY: geteuid has no preconditions and cannot fail.
    let privileged = unsafe { libc::geteuid() } == 0;
    let filesystem = filesystem_bytes(counts, privileged).map_err(local_provider_error)?;

    let quota = observe_quota(&directory, &metadata);
    let caller_available = match &quota {
        CapacityQuota::Limited(limits) => limits
            .iter()
            .map(QuotaLimit::remaining_bytes)
            .fold(filesystem.caller_free, u64::min),
        _ => filesystem.caller_free,
    };
    let placement = CapacityPlacement(format!(
        "local:dev={:x};fsid={:x}",
        metadata.dev(),
        widen(stats.f_fsid)
    ));
    let state = CapacityState::Known(KnownCapacity {
        caller_available_bytes: caller_available,
        total_bytes: Some(filesystem.total),
        physical_available_bytes: Some(filesystem.physical_free),
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
fn local_provider_error(message: &str) -> StorageError {
    StorageError::Provider {
        backend: StorageBackend::Local.as_str().to_string(),
        message: message.to_string(),
        retryable: false,
    }
}

#[cfg(unix)]
fn widen<T: TryInto<u64>>(value: T) -> u64 {
    value.try_into().unwrap_or(u64::MAX)
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

#[cfg(target_os = "linux")]
mod linux {
    use std::os::{fd::AsRawFd, unix::fs::MetadataExt};

    use super::{QuotaSample, quota_limit};
    use crate::{CapacityQuota, QuotaLimit, QuotaScope};

    const PROJECT_QUOTA: libc::c_int = 2;
    const EXT4_SUPER_MAGIC: u64 = 0xef53;
    const XFS_SUPER_MAGIC: u64 = 0x5846_5342;
    const FS_XFLAG_PROJINHERIT: u32 = 0x0000_0200;
    const SET_GROUP_ID: u32 = 0o2000;

    /// A failed quota query, with the raw errno when the kernel supplied one.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub(super) struct QuotaQueryError {
        errno: Option<i32>,
        reason: String,
    }

    impl QuotaQueryError {
        fn permission_denied(&self) -> bool {
            matches!(self.errno, Some(libc::EPERM | libc::EACCES))
        }
    }

    /// Project attributes of the storage root.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(super) struct ProjectAttributes {
        project_id: u32,
        inherit: bool,
    }

    /// Outcome of the project quota observation.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub(super) enum ProjectQuota {
        /// The root belongs to no project tree, or project quotas are not
        /// active or supported for its filesystem.
        NotApplicable,
        /// The root's project has a finite quota.
        Limited(QuotaLimit),
        /// The quota is unreadable for this caller, but the filesystem applies
        /// it to the `statvfs` result of a project-inheriting directory (ext4
        /// and XFS).
        ReflectedByFilesystem,
        /// The project quota could not be observed.
        NotObserved(String),
    }

    pub(super) fn observe_quota(
        directory: &std::fs::File,
        metadata: &std::fs::Metadata,
    ) -> CapacityQuota {
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
                Ok(Some(sample)) => {
                    limits.extend(quota_limit(scope, u64::from(principal), &sample));
                }
                Ok(None) => {}
                Err(error) => {
                    return CapacityQuota::NotObserved {
                        reason: error.reason,
                    };
                }
            }
        }

        let attributes = project_attributes(directory);
        let reflects = filesystem_reflects_project_quota(directory);
        let query = |id| query_quota(directory, PROJECT_QUOTA, id);
        let reflected = match project_quota(attributes, reflects, query) {
            ProjectQuota::NotApplicable => false,
            ProjectQuota::Limited(limit) => {
                limits.push(limit);
                false
            }
            ProjectQuota::ReflectedByFilesystem => true,
            ProjectQuota::NotObserved(reason) => return CapacityQuota::NotObserved { reason },
        };
        if !limits.is_empty() {
            CapacityQuota::Limited(limits)
        } else if reflected {
            CapacityQuota::ProviderAccounted
        } else {
            CapacityQuota::NotEnforced
        }
    }

    /// Decides the project quota outcome from the root's attributes and a
    /// project quota query.
    pub(super) fn project_quota(
        attributes: Result<ProjectAttributes, i32>,
        filesystem_reflects: bool,
        query: impl FnOnce(u32) -> Result<Option<QuotaSample>, QuotaQueryError>,
    ) -> ProjectQuota {
        let attributes = match attributes {
            Ok(attributes) => attributes,
            // The filesystem has no project identifiers to enforce.
            Err(libc::ENOTTY | libc::EOPNOTSUPP | libc::EINVAL) => {
                return ProjectQuota::NotApplicable;
            }
            Err(errno) => {
                let reason = format!("project attributes are unavailable (errno {errno})");
                return ProjectQuota::NotObserved(reason);
            }
        };
        // Project 0 without inheritance is the filesystem default, not a tree.
        if attributes.project_id == 0 && !attributes.inherit {
            return ProjectQuota::NotApplicable;
        }
        let project_id = u64::from(attributes.project_id);
        match query(attributes.project_id) {
            Ok(Some(sample)) => quota_limit(QuotaScope::Project, project_id, &sample)
                .map_or(ProjectQuota::NotApplicable, ProjectQuota::Limited),
            Ok(None) => ProjectQuota::NotApplicable,
            Err(error)
                if error.permission_denied() && attributes.inherit && filesystem_reflects =>
            {
                ProjectQuota::ReflectedByFilesystem
            }
            Err(error) => ProjectQuota::NotObserved(error.reason),
        }
    }

    /// Queries one quota; `Ok(None)` means quotas of that kind are not active
    /// or not supported by the filesystem, so none can be enforced.
    fn query_quota(
        directory: &std::fs::File,
        kind: libc::c_int,
        principal: u32,
    ) -> Result<Option<QuotaSample>, QuotaQueryError> {
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
            let errno = error.raw_os_error();
            let reason = match errno {
                Some(libc::ESRCH | libc::EINVAL) => return Ok(None),
                Some(libc::ENOSYS) => {
                    "quota interface is unavailable for this kernel or filesystem".to_string()
                }
                Some(libc::EPERM | libc::EACCES) => {
                    "caller is not permitted to read this quota".to_string()
                }
                _ => format!("quota query failed: {error}"),
            };
            return Err(QuotaQueryError { errno, reason });
        }
        // SAFETY: quotactl_fd succeeded and initialised the structure.
        let quota = unsafe { quota.assume_init() };
        let required = libc::QIF_BLIMITS | libc::QIF_SPACE;
        if quota.dqb_valid & required != required {
            return Err(QuotaQueryError {
                errno: None,
                reason: "quota response omitted block limits or usage".to_string(),
            });
        }
        Ok(Some(QuotaSample {
            hard_limit_blocks: quota.dqb_bhardlimit,
            soft_limit_blocks: quota.dqb_bsoftlimit,
            used_bytes: quota.dqb_curspace,
        }))
    }

    /// Reads the root's project identifier with `FS_IOC_FSGETXATTR`.
    ///
    /// The request number uses the generic ioctl encoding, so other
    /// architectures report the attributes as unavailable.
    #[cfg(any(
        target_arch = "x86",
        target_arch = "x86_64",
        target_arch = "arm",
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "loongarch64",
        target_arch = "s390x"
    ))]
    fn project_attributes(directory: &std::fs::File) -> Result<ProjectAttributes, i32> {
        /// Kernel `struct fsxattr`.
        #[repr(C)]
        #[derive(Default)]
        #[allow(dead_code)]
        struct FsXattr {
            xflags: u32,
            extsize: u32,
            nextents: u32,
            projid: u32,
            cowextsize: u32,
            pad: [u8; 8],
        }

        /// `_IOR('X', 31, struct fsxattr)`.
        const FS_IOC_FSGETXATTR: u32 = 0x801c_581f;

        let mut attributes = FsXattr::default();
        // SAFETY: the descriptor stays open for the call and `attributes` is a
        // valid out pointer for the `fsxattr` structure this request writes.
        let result = unsafe {
            libc::ioctl(
                directory.as_raw_fd(),
                FS_IOC_FSGETXATTR as libc::Ioctl,
                &raw mut attributes,
            )
        };
        if result != 0 {
            let errno = std::io::Error::last_os_error().raw_os_error();
            return Err(errno.unwrap_or(libc::EIO));
        }
        Ok(ProjectAttributes {
            project_id: attributes.projid,
            inherit: attributes.xflags & FS_XFLAG_PROJINHERIT != 0,
        })
    }

    #[cfg(not(any(
        target_arch = "x86",
        target_arch = "x86_64",
        target_arch = "arm",
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "loongarch64",
        target_arch = "s390x"
    )))]
    fn project_attributes(_directory: &std::fs::File) -> Result<ProjectAttributes, i32> {
        Err(libc::ENOSYS)
    }

    fn filesystem_reflects_project_quota(directory: &std::fs::File) -> bool {
        let mut stats = std::mem::MaybeUninit::<libc::statfs>::zeroed();
        // SAFETY: the descriptor stays open for the call and `stats` is a valid
        // out pointer for one `statfs` structure.
        let result = unsafe { libc::fstatfs(directory.as_raw_fd(), stats.as_mut_ptr()) };
        if result != 0 {
            return false;
        }
        // SAFETY: fstatfs succeeded and initialised the structure.
        let stats = unsafe { stats.assume_init() };
        matches!(super::widen(stats.f_type), EXT4_SUPER_MAGIC | XFS_SUPER_MAGIC)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn limited(hard: u64) -> Result<Option<QuotaSample>, QuotaQueryError> {
            Ok(Some(QuotaSample {
                hard_limit_blocks: hard,
                soft_limit_blocks: 0,
                used_bytes: 1024,
            }))
        }

        fn denied() -> Result<Option<QuotaSample>, QuotaQueryError> {
            Err(QuotaQueryError {
                errno: Some(libc::EPERM),
                reason: "denied".to_string(),
            })
        }

        fn project(project_id: u32, inherit: bool) -> Result<ProjectAttributes, i32> {
            Ok(ProjectAttributes {
                project_id,
                inherit,
            })
        }

        #[test]
        fn filesystems_without_project_ids_have_no_project_quota() {
            for errno in [libc::ENOTTY, libc::EOPNOTSUPP, libc::EINVAL] {
                assert_eq!(
                    project_quota(Err(errno), false, |_| panic!("no query expected")),
                    ProjectQuota::NotApplicable
                );
            }
            assert!(matches!(
                project_quota(Err(libc::EIO), false, |_| panic!("no query expected")),
                ProjectQuota::NotObserved(_)
            ));
        }

        #[test]
        fn default_project_without_inheritance_is_not_a_tree() {
            assert_eq!(
                project_quota(project(0, false), false, |_| panic!("no query expected")),
                ProjectQuota::NotApplicable
            );
        }

        #[test]
        fn readable_project_quota_is_a_limit() {
            let outcome = project_quota(project(42, true), true, |id| {
                assert_eq!(id, 42);
                limited(10)
            });
            let ProjectQuota::Limited(limit) = outcome else {
                panic!("expected a project limit, got {outcome:?}");
            };
            assert_eq!(limit.scope, QuotaScope::Project);
            assert_eq!(limit.principal, 42);
            assert_eq!(limit.limit_bytes, 10 * super::super::QUOTA_BLOCK_BYTES);
            assert_eq!(
                project_quota(project(42, true), true, |_| Ok(None)),
                ProjectQuota::NotApplicable
            );
        }

        #[test]
        fn unreadable_project_quota_is_accounted_only_where_statfs_applies_it() {
            assert_eq!(
                project_quota(project(42, true), true, |_| denied()),
                ProjectQuota::ReflectedByFilesystem
            );
            for (inherit, reflects) in [(true, false), (false, true), (false, false)] {
                assert!(matches!(
                    project_quota(project(42, inherit), reflects, |_| denied()),
                    ProjectQuota::NotObserved(_)
                ));
            }
        }
    }
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

    fn counts(blocks: u64, free: u64, available: u64) -> BlockCounts {
        BlockCounts {
            fragment_bytes: 4096,
            blocks,
            free,
            available,
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
    fn filesystem_bytes_keep_reserved_blocks_from_unprivileged_callers() {
        let unprivileged = filesystem_bytes(counts(100, 40, 30), false).expect("consistent");
        assert_eq!(
            unprivileged,
            FilesystemBytes {
                total: 100 * 4096,
                physical_free: 40 * 4096,
                caller_free: 30 * 4096,
            }
        );
        let privileged = filesystem_bytes(counts(100, 40, 30), true).expect("consistent");
        assert_eq!(privileged.caller_free, 40 * 4096);
        let full = filesystem_bytes(counts(100, 0, 0), false).expect("consistent");
        assert_eq!(full.caller_free, 0);
    }

    #[test]
    fn filesystem_bytes_reject_inconsistent_or_overflowing_reports() {
        assert!(filesystem_bytes(counts(10, 11, 0), false).is_err());
        assert!(filesystem_bytes(counts(10, 5, 6), false).is_err());
        assert!(filesystem_bytes(counts(u64::MAX, 0, 0), false).is_err());
        let mut zero = counts(1, 1, 1);
        zero.fragment_bytes = 0;
        assert!(filesystem_bytes(zero, false).is_err());
    }

    #[test]
    fn binding_covers_root_and_account() {
        assert_eq!(binding(Path::new("/a")), binding(Path::new("/a")));
        assert_ne!(binding(Path::new("/a")), binding(Path::new("/b")));
    }
}
