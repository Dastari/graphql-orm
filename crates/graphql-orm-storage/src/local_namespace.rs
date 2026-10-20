//! Local filesystem namespace markers and capacity-domain facts.
//!
//! Durability: a marker is published by writing and `fsync`ing a reserved
//! temporary file, hard-linking it to the marker name (which fails if the
//! marker exists), `fsync`ing the parent directory, then removing the
//! temporary file. A marker is reported `Initialised` only after a complete,
//! valid body is read back. Interrupted publications are visible as
//! `PublicationInProgress` until reconciled by retrying the same identity.
//!
//! Limitations: the trusted-root contract of [`crate::LocalStorageBackend`]
//! still applies. Path checks and fences cannot exclude a concurrent local
//! actor replacing paths between a check and a use.

use std::path::{Path, PathBuf};

use time::OffsetDateTime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

use crate::{
    CapacityDomainObservation, DEFAULT_MARKER_READ_TIMEOUT, DomainFact, DomainState, FactScope,
    FoundNamespace, KeyEquivalence, MAX_NAMESPACE_MARKER_BYTES, NAMESPACE_MARKER,
    NAMESPACE_MARKER_PUBLISHING, NamespaceFence, NamespaceId, NamespaceInitialisation,
    NamespaceObservation, NamespaceState, NamespaceTarget, ObservationEpoch, ScanCompleteness,
    ScanResult, StorageBackend, StorageError, TopologyScan, decode_marker, encode_marker,
    namespace::{ScanDeadline, fence_invalidated, require_key_in_target},
};
use crate::{AncestorScan, NestedScan};

/// Local roots may be case-insensitive, so reserved names are case-folded.
#[cfg(not(windows))]
pub(crate) const LOCAL_KEY_EQUIVALENCE: KeyEquivalence = KeyEquivalence::CaseFolding;

/// On Windows, local paths also resolve alternate data streams and 8.3 names.
#[cfg(windows)]
pub(crate) const LOCAL_KEY_EQUIVALENCE: KeyEquivalence = KeyEquivalence::Windows;

const INTERNAL_STORAGE_DIR: &str = ".graphql-orm-storage";

enum MarkerRead {
    Absent,
    Bytes(Vec<u8>),
    NotAFile,
    Failed(String),
}

pub(crate) fn namespace_dir(
    root: &Path,
    target: &NamespaceTarget,
) -> Result<PathBuf, StorageError> {
    Ok(match target.relative_prefix(LOCAL_KEY_EQUIVALENCE)? {
        Some(prefix) => root.join(prefix),
        None => root.to_path_buf(),
    })
}

async fn read_bounded(path: &Path) -> MarkerRead {
    let metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return MarkerRead::Absent,
        Err(error) => {
            return MarkerRead::Failed(format!("marker metadata unreadable: {}", error.kind()));
        }
    };
    if !metadata.is_file() {
        return MarkerRead::NotAFile;
    }
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return MarkerRead::Absent,
        Err(error) => return MarkerRead::Failed(format!("marker unreadable: {}", error.kind())),
    };
    let mut bytes = Vec::with_capacity(MAX_NAMESPACE_MARKER_BYTES + 1);
    let limit = u64::try_from(MAX_NAMESPACE_MARKER_BYTES + 1).unwrap_or(u64::MAX);
    match file.take(limit).read_to_end(&mut bytes).await {
        Ok(_) => MarkerRead::Bytes(bytes),
        Err(error) => MarkerRead::Failed(format!("marker read failed: {}", error.kind())),
    }
}

async fn marker_state(dir: &Path) -> NamespaceState {
    let read = tokio::time::timeout(
        DEFAULT_MARKER_READ_TIMEOUT,
        read_bounded(&dir.join(NAMESPACE_MARKER)),
    )
    .await;
    match read {
        Err(_) => NamespaceState::Unknown {
            reason: "marker read timed out".to_string(),
        },
        Ok(MarkerRead::Bytes(bytes)) => decode_marker(&bytes),
        Ok(MarkerRead::NotAFile) => NamespaceState::Conflict {
            reason: "marker is not a regular file".to_string(),
        },
        Ok(MarkerRead::Failed(reason)) => NamespaceState::Unknown { reason },
        Ok(MarkerRead::Absent) => {
            match tokio::fs::symlink_metadata(dir.join(NAMESPACE_MARKER_PUBLISHING)).await {
                Ok(_) => NamespaceState::PublicationInProgress,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    NamespaceState::Uninitialised
                }
                Err(error) => NamespaceState::Unknown {
                    reason: format!("publication state unreadable: {}", error.kind()),
                },
            }
        }
    }
}

#[cfg(unix)]
fn boot_scope() -> FactScope {
    match std::fs::read_to_string("/proc/sys/kernel/random/boot_id") {
        Ok(boot_id) if !boot_id.trim().is_empty() => FactScope::HostBoot {
            boot_id: boot_id.trim().to_string(),
        },
        _ => FactScope::Unspecified,
    }
}

#[cfg(unix)]
async fn device_inode(path: &Path) -> Result<(u64, u64), std::io::Error> {
    use std::os::unix::fs::MetadataExt;

    let metadata = tokio::fs::metadata(path).await?;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(unix)]
async fn root_witness(root: &Path) -> Option<String> {
    device_inode(root)
        .await
        .ok()
        .map(|(dev, ino)| format!("local:dev={dev:x};ino={ino:x}"))
}

#[cfg(not(unix))]
async fn root_witness(_root: &Path) -> Option<String> {
    None
}

#[cfg(unix)]
async fn location_fact(dir: &Path) -> Option<DomainFact> {
    let (dev, ino) = device_inode(dir).await.ok()?;
    Some(DomainFact::RootLocation {
        provider: StorageBackend::Local,
        tuple: vec![
            ("dev".to_string(), format!("{dev:x}")),
            ("ino".to_string(), format!("{ino:x}")),
        ],
        scope: boot_scope(),
    })
}

#[cfg(not(unix))]
async fn location_fact(_dir: &Path) -> Option<DomainFact> {
    None
}

pub(crate) async fn epoch(root: &Path, instance: Uuid) -> ObservationEpoch {
    ObservationEpoch {
        backend_instance: instance,
        connection_generation: 0,
        root_witness: root_witness(root).await,
    }
}

async fn require_directory(dir: &Path) -> Result<(), StorageError> {
    let metadata = tokio::fs::metadata(dir)
        .await
        .map_err(|source| StorageError::io(dir, source))?;
    if metadata.is_dir() {
        Ok(())
    } else {
        Err(StorageError::io(
            dir,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "namespace root is not a directory",
            ),
        ))
    }
}

fn truncated(limit: &str) -> ScanCompleteness {
    ScanCompleteness::Truncated {
        limit: limit.to_string(),
    }
}

#[cfg(unix)]
async fn scan_ancestors(dir: &Path, scan: AncestorScan) -> ScanResult {
    let deadline = ScanDeadline::after(scan.max_duration);
    let Some(Ok((device, _))) = deadline.bound(device_inode(dir)).await else {
        return ScanResult {
            completeness: ScanCompleteness::Denied {
                reason: "namespace root identity unreadable or deadline reached".to_string(),
            },
            found: Vec::new(),
        };
    };
    let mut found = Vec::new();
    let mut completeness = ScanCompleteness::Complete;
    let mut current = dir.to_path_buf();
    let mut level = 0_usize;
    while let Some(parent) = current.parent().map(Path::to_path_buf) {
        let Some(identity) = deadline.bound(device_inode(&parent)).await else {
            completeness = truncated("max_duration");
            break;
        };
        match identity {
            // The filesystem holding the root is the authority boundary.
            Ok((parent_device, _)) if parent_device != device => break,
            Ok(_) => {}
            Err(error) => {
                completeness = ScanCompleteness::Denied {
                    reason: format!("ancestor unreadable: {}", error.kind()),
                };
                break;
            }
        }
        if level == scan.max_depth {
            // A same-filesystem parent exists beyond the requested depth.
            completeness = truncated("max_depth");
            break;
        }
        level += 1;
        let Some(state) = deadline.bound(marker_state(&parent)).await else {
            completeness = truncated("max_duration");
            break;
        };
        if let NamespaceState::Unknown { reason } = &state {
            completeness = ScanCompleteness::Denied { reason: reason.clone() };
        }
        if state != NamespaceState::Uninitialised {
            found.push(FoundNamespace {
                relative_path: vec![".."; level].join("/"),
                state,
            });
        }
        current = parent;
    }
    ScanResult { completeness, found }
}

#[cfg(not(unix))]
async fn scan_ancestors(_dir: &Path, _scan: AncestorScan) -> ScanResult {
    ScanResult::unsupported()
}

/// Bounded nested scan: every await is bounded by the remaining original
/// deadline, and pages, entries, depth, bytes and results are all budgeted.
/// Reaching any limit reports `Truncated`, never a completed absence.
async fn scan_nested(dir: &Path, scan: NestedScan) -> ScanResult {
    let deadline = ScanDeadline::after(scan.max_duration);
    let mut found = Vec::new();
    let mut completeness = ScanCompleteness::Complete;
    let mut stack = vec![(dir.to_path_buf(), 0_usize)];
    let (mut pages, mut keys, mut bytes) = (0_usize, 0_usize, 0_usize);
    'scan: while let Some((path, depth)) = stack.pop() {
        pages += 1;
        if pages > scan.max_pages {
            completeness = truncated("max_pages");
            break;
        }
        let mut entries = match deadline.bound(tokio::fs::read_dir(&path)).await {
            None => {
                completeness = truncated("max_duration");
                break;
            }
            Some(Ok(entries)) => entries,
            Some(Err(error)) => {
                completeness = ScanCompleteness::Denied {
                    reason: format!("directory unreadable: {}", error.kind()),
                };
                continue;
            }
        };
        loop {
            let entry = match deadline.bound(entries.next_entry()).await {
                None => {
                    completeness = truncated("max_duration");
                    break 'scan;
                }
                Some(Ok(Some(entry))) => entry,
                Some(Ok(None)) => break,
                Some(Err(error)) => {
                    completeness = ScanCompleteness::Denied {
                        reason: format!("directory listing failed: {}", error.kind()),
                    };
                    break;
                }
            };
            keys += 1;
            if keys > scan.max_keys {
                completeness = truncated("max_keys");
                break 'scan;
            }
            let name = entry.file_name();
            if name == INTERNAL_STORAGE_DIR {
                continue;
            }
            let Some(file_type) = deadline.bound(entry.file_type()).await else {
                completeness = truncated("max_duration");
                break 'scan;
            };
            let Ok(file_type) = file_type else {
                completeness = ScanCompleteness::Denied {
                    reason: "entry type unreadable".to_string(),
                };
                continue;
            };
            if file_type.is_symlink() {
                // Ordinary operations follow links; the scan does not, so it is incomplete.
                completeness = truncated("symbolic link not followed");
                continue;
            }
            if !file_type.is_dir() {
                continue;
            }
            let child = entry.path();
            let relative = child
                .strip_prefix(dir)
                .unwrap_or(&child)
                .to_string_lossy()
                .replace('\\', "/");
            if depth.saturating_add(1) > scan.max_depth {
                completeness = truncated("max_depth");
                continue;
            }
            bytes = bytes.saturating_add(relative.len());
            if bytes > scan.max_bytes {
                completeness = truncated("max_bytes");
                break 'scan;
            }
            let Some(state) = deadline.bound(marker_state(&child)).await else {
                completeness = truncated("max_duration");
                break 'scan;
            };
            bytes = bytes.saturating_add(MAX_NAMESPACE_MARKER_BYTES);
            if let NamespaceState::Unknown { reason } = &state {
                completeness = ScanCompleteness::Denied { reason: reason.clone() };
            }
            if state != NamespaceState::Uninitialised {
                if found.len() == scan.max_results {
                    completeness = truncated("max_results");
                    break 'scan;
                }
                found.push(FoundNamespace {
                    relative_path: relative,
                    state,
                });
            }
            stack.push((child, depth + 1));
        }
    }
    found.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    ScanResult { completeness, found }
}

pub(crate) async fn observe(
    root: &Path,
    instance: Uuid,
    target: &NamespaceTarget,
    scan: &TopologyScan,
) -> Result<NamespaceObservation, StorageError> {
    scan.validate()?;
    let observed_at = OffsetDateTime::now_utc();
    let dir = namespace_dir(root, target)?;
    require_directory(&dir).await?;
    let state = marker_state(&dir).await;
    let ancestors = match scan.ancestors {
        Some(ancestors) => scan_ancestors(&dir, ancestors).await,
        None => ScanResult::not_requested(),
    };
    let nested = match scan.nested {
        Some(nested) => scan_nested(&dir, nested).await,
        None => ScanResult::not_requested(),
    };
    Ok(NamespaceObservation {
        target: target.clone(),
        state,
        root_location: location_fact(&dir).await,
        ancestors,
        nested,
        // The witness is the TARGET root, so a replaced sub-prefix directory
        // invalidates fences even when it carries a copied marker.
        epoch: epoch(&dir, instance).await,
        observed_at,
    })
}

#[cfg(unix)]
async fn sync_directory(dir: &Path) -> Result<(), StorageError> {
    let handle = tokio::fs::File::open(dir)
        .await
        .map_err(|source| StorageError::io(dir, source))?;
    handle
        .sync_all()
        .await
        .map_err(|source| StorageError::io(dir, source))
}

#[cfg(not(unix))]
async fn sync_directory(_dir: &Path) -> Result<(), StorageError> {
    // Directory fsync is unavailable here; the durability claim is weaker and documented.
    Ok(())
}

fn settled(existing: NamespaceState, proposed: NamespaceId) -> Option<NamespaceInitialisation> {
    match existing {
        NamespaceState::Initialised { id } if id == proposed => {
            Some(NamespaceInitialisation::AlreadyInitialisedSame(id))
        }
        NamespaceState::Initialised { id } => {
            Some(NamespaceInitialisation::AlreadyInitialisedOther(id))
        }
        NamespaceState::Conflict { reason } => Some(NamespaceInitialisation::Conflict { reason }),
        _ => None,
    }
}

pub(crate) async fn initialise(
    root: &Path,
    target: &NamespaceTarget,
    proposed: NamespaceId,
) -> Result<NamespaceInitialisation, StorageError> {
    let dir = namespace_dir(root, target)?;
    require_directory(&dir).await?;
    let marker = dir.join(NAMESPACE_MARKER);
    let publishing = dir.join(NAMESPACE_MARKER_PUBLISHING);
    let existing = marker_state(&dir).await;
    if let NamespaceState::Unknown { reason } = &existing {
        return Err(StorageError::PreconditionFailed {
            key: NAMESPACE_MARKER.to_string(),
            condition: format!("marker state unknown: {reason}"),
        });
    }
    if let Some(result) = settled(existing, proposed) {
        return Ok(result);
    }

    let body = encode_marker(proposed);
    match tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&publishing)
        .await
    {
        Ok(mut file) => {
            file.write_all(&body)
                .await
                .map_err(|source| StorageError::io(&publishing, source))?;
            file.sync_all()
                .await
                .map_err(|source| StorageError::io(&publishing, source))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // Reconcile only a complete publication of the SAME identity.
            match read_bounded(&publishing).await {
                MarkerRead::Bytes(bytes)
                    if decode_marker(&bytes) == (NamespaceState::Initialised { id: proposed }) => {}
                _ => {
                    return Ok(NamespaceInitialisation::Conflict {
                        reason: "another or incomplete publication is in progress".to_string(),
                    });
                }
            }
        }
        Err(source) => return Err(StorageError::io(&publishing, source)),
    }

    match tokio::fs::hard_link(&publishing, &marker).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = tokio::fs::remove_file(&publishing).await;
            let state = marker_state(&dir).await;
            return Ok(
                settled(state, proposed).unwrap_or(NamespaceInitialisation::Conflict {
                    reason: "marker appeared during publication but is unreadable".to_string(),
                }),
            );
        }
        Err(source) => return Err(StorageError::io(&marker, source)),
    }
    // Durable once the directory entry is synced; a failure here leaves the
    // outcome unknown and is reconciled by retrying the same identity.
    sync_directory(&dir).await?;
    let _ = tokio::fs::remove_file(&publishing).await;
    let _ = sync_directory(&dir).await;

    match marker_state(&dir).await {
        NamespaceState::Initialised { id } if id == proposed => {
            Ok(NamespaceInitialisation::Created(id))
        }
        other => Ok(
            settled(other, proposed).unwrap_or(NamespaceInitialisation::Conflict {
                reason: "published marker did not read back".to_string(),
            }),
        ),
    }
}

#[cfg(unix)]
async fn domain_facts(root: &Path) -> Result<(Vec<DomainFact>, DomainState), StorageError> {
    let (dev, _) = device_inode(root)
        .await
        .map_err(|source| StorageError::io(root, source))?;
    let mut facts = Vec::new();
    if let Some(location) = location_fact(root).await {
        facts.push(location);
    }
    facts.push(DomainFact::PhysicalStore {
        provider: StorageBackend::Local,
        tuple: vec![("dev".to_string(), format!("{dev:x}"))],
        scope: boot_scope(),
    });
    let missing = vec![
        "quota constraints".to_string(),
        "filesystem identity beyond the boot".to_string(),
    ];
    Ok((facts, DomainState::Partial { missing }))
}

#[cfg(not(unix))]
async fn domain_facts(_root: &Path) -> Result<(Vec<DomainFact>, DomainState), StorageError> {
    Ok((Vec::new(), DomainState::Unsupported))
}

pub(crate) async fn capacity_domain(
    root: &Path,
    instance: Uuid,
) -> Result<CapacityDomainObservation, StorageError> {
    let observed_at = OffsetDateTime::now_utc();
    require_directory(root).await?;
    let (facts, state) = domain_facts(root).await?;
    Ok(CapacityDomainObservation {
        facts,
        state,
        epoch: epoch(root, instance).await,
        observed_at,
    })
}

pub(crate) async fn check_fence(
    root: &Path,
    instance: Uuid,
    fence: &NamespaceFence,
    key: &str,
) -> Result<(), StorageError> {
    require_key_in_target(key, &fence.target)?;
    if fence.epoch.backend_instance != instance {
        return Err(fence_invalidated(key, "different backend instance"));
    }
    let dir = namespace_dir(root, &fence.target)?;
    let current = root_witness(&dir).await;
    if current.is_none() || current != fence.epoch.root_witness {
        return Err(fence_invalidated(key, "target root was replaced or is unreadable"));
    }
    match marker_state(&dir).await {
        NamespaceState::Initialised { id } if id == fence.namespace => Ok(()),
        _ => Err(fence_invalidated(key, "namespace marker changed")),
    }
}
