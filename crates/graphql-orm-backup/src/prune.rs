use std::collections::HashSet;

use uuid::Uuid;

use crate::{
    BackupError, BackupRepository, BackupSnapshotManifest, RepositoryLock, RepositoryLockOptions,
    load_manifest, load_manifest_chain, snapshot_manifest_key,
};

#[derive(Clone, Debug, Eq, PartialEq)]
/// Retention policy for repository pruning.
pub struct KeepPolicy {
    /// Number of newest manifest chains to retain.
    pub keep_last: usize,
    /// Advisory repository lock settings.
    pub lock: RepositoryLockOptions,
}

impl Default for KeepPolicy {
    fn default() -> Self {
        Self {
            keep_last: 1,
            lock: RepositoryLockOptions::default(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Summary returned by `prune`.
pub struct PruneResult {
    /// Number of retained snapshot manifests.
    pub retained_snapshots: usize,
    /// Number of expired snapshot manifests.
    pub deleted_snapshots: usize,
    /// Number of blobs deleted.
    pub deleted_blobs: usize,
}

/// Prunes expired snapshots and unreferenced object blobs.
///
/// Object reachability is derived from the snapshot listing, so pruning fails
/// closed when that listing is incomplete: a listed manifest's parent that is
/// stored but not listed, or a manifest set that differs when re-listed
/// immediately before the object sweep, aborts with
/// [`BackupError::InvalidManifestChain`] before any object is deleted.
///
/// # Errors
///
/// Returns [`BackupError`] if repository listing, manifest loading, locking, or
/// blob deletion fails, or if the snapshot listing is incomplete.
pub async fn prune(
    repository: &dyn BackupRepository,
    keep_policy: &KeepPolicy,
) -> Result<PruneResult, BackupError> {
    let lock = RepositoryLock::acquire(repository, &keep_policy.lock).await?;
    let result = prune_inner(repository, keep_policy).await;
    let release_result = lock.release().await;
    match (result, release_result) {
        (Ok(result), Ok(())) => Ok(result),
        (Err(err), _) => Err(err),
        (Ok(_), Err(err)) => Err(err),
    }
}

async fn prune_inner(
    repository: &dyn BackupRepository,
    keep_policy: &KeepPolicy,
) -> Result<PruneResult, BackupError> {
    let mut manifests = load_all_manifests(repository).await?;
    ensure_listed_parents(repository, &manifests).await?;
    manifests.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.snapshot_id.cmp(&left.snapshot_id))
    });

    let selected = manifests
        .iter()
        .take(keep_policy.keep_last)
        .map(|manifest| manifest.snapshot_id)
        .collect::<Vec<_>>();

    let mut retained_ids = HashSet::new();
    let mut reachable_content_keys = HashSet::new();
    for snapshot_id in selected {
        let chain = load_manifest_chain(repository, snapshot_id).await?;
        for manifest in chain {
            retained_ids.insert(manifest.snapshot_id);
            collect_reachable_keys(&manifest, &mut reachable_content_keys);
        }
    }

    let all_ids = manifests
        .iter()
        .map(|manifest| manifest.snapshot_id)
        .collect::<HashSet<_>>();
    let expired_ids = all_ids
        .difference(&retained_ids)
        .copied()
        .collect::<Vec<_>>();

    let mut deleted_blobs = 0_usize;
    for snapshot_id in &expired_ids {
        for key in repository
            .list_blobs(&format!("snapshots/{snapshot_id}"))
            .await?
        {
            repository.delete_blob(&key).await?;
            deleted_blobs += 1;
        }
    }

    ensure_stable_listing(repository, &retained_ids).await?;
    for key in repository.list_blobs("objects/sha256").await? {
        if !reachable_content_keys.contains(&key) {
            repository.delete_blob(&key).await?;
            deleted_blobs += 1;
        }
    }

    Ok(PruneResult {
        retained_snapshots: retained_ids.len(),
        deleted_snapshots: expired_ids.len(),
        deleted_blobs,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Summary returned by `delete_snapshot`.
pub struct DeleteSnapshotResult {
    /// Number of blobs deleted, including unreferenced object blobs.
    pub deleted_blobs: usize,
    /// Number of snapshot manifests remaining after deletion.
    pub retained_snapshots: usize,
}

/// Deletes one snapshot and any object blobs no other snapshot references.
///
/// Snapshots that are the parent of another manifest cannot be deleted;
/// dependent snapshots must be deleted or compacted first. Like [`prune`], the
/// object sweep fails closed when the snapshot listing is incomplete.
///
/// # Errors
///
/// Returns [`BackupError`] if the snapshot is missing, another manifest depends
/// on it, the snapshot listing is incomplete, or repository listing, locking,
/// or blob deletion fails.
pub async fn delete_snapshot(
    repository: &dyn BackupRepository,
    snapshot_id: Uuid,
    lock_options: &RepositoryLockOptions,
) -> Result<DeleteSnapshotResult, BackupError> {
    let lock = RepositoryLock::acquire(repository, lock_options).await?;
    let result = delete_snapshot_inner(repository, snapshot_id).await;
    let release_result = lock.release().await;
    match (result, release_result) {
        (Ok(result), Ok(())) => Ok(result),
        (Err(err), _) => Err(err),
        (Ok(_), Err(err)) => Err(err),
    }
}

async fn delete_snapshot_inner(
    repository: &dyn BackupRepository,
    snapshot_id: Uuid,
) -> Result<DeleteSnapshotResult, BackupError> {
    let manifests = load_all_manifests(repository).await?;
    ensure_listed_parents(repository, &manifests).await?;
    if !manifests
        .iter()
        .any(|manifest| manifest.snapshot_id == snapshot_id)
    {
        return Err(BackupError::MissingBlob {
            key: snapshot_manifest_key(snapshot_id),
        });
    }
    if let Some(child) = manifests
        .iter()
        .find(|manifest| manifest.parent_snapshot_id == Some(snapshot_id))
    {
        return Err(BackupError::InvalidManifestChain {
            reason: format!(
                "snapshot {snapshot_id} is the parent of snapshot {}; delete or compact dependent snapshots first",
                child.snapshot_id
            ),
        });
    }

    let mut deleted_blobs = 0_usize;
    for key in repository
        .list_blobs(&format!("snapshots/{snapshot_id}"))
        .await?
    {
        repository.delete_blob(&key).await?;
        deleted_blobs += 1;
    }

    let mut reachable_content_keys = HashSet::new();
    for manifest in manifests
        .iter()
        .filter(|manifest| manifest.snapshot_id != snapshot_id)
    {
        collect_reachable_keys(manifest, &mut reachable_content_keys);
    }
    let remaining_ids = manifests
        .iter()
        .map(|manifest| manifest.snapshot_id)
        .filter(|id| *id != snapshot_id)
        .collect::<HashSet<_>>();
    ensure_stable_listing(repository, &remaining_ids).await?;
    for key in repository.list_blobs("objects/sha256").await? {
        if !reachable_content_keys.contains(&key) {
            repository.delete_blob(&key).await?;
            deleted_blobs += 1;
        }
    }

    Ok(DeleteSnapshotResult {
        deleted_blobs,
        retained_snapshots: manifests.len() - 1,
    })
}

async fn load_all_manifests(
    repository: &dyn BackupRepository,
) -> Result<Vec<BackupSnapshotManifest>, BackupError> {
    let mut manifests = Vec::new();
    for key in repository.list_blobs("snapshots").await? {
        let Some(snapshot_id) = snapshot_id_from_manifest_key(&key) else {
            continue;
        };
        manifests.push(load_manifest(repository, snapshot_id).await?);
    }
    Ok(manifests)
}

async fn list_manifest_ids(
    repository: &dyn BackupRepository,
) -> Result<HashSet<Uuid>, BackupError> {
    Ok(repository
        .list_blobs("snapshots")
        .await?
        .iter()
        .filter_map(|key| snapshot_id_from_manifest_key(key))
        .collect())
}

/// Refuses a listing that omits a stored parent of a listed manifest.
///
/// A parent missing from storage is a pre-existing broken chain, which manifest
/// chain loading reports where it matters; a parent that is stored but was not
/// listed proves the listing is incomplete.
async fn ensure_listed_parents(
    repository: &dyn BackupRepository,
    manifests: &[BackupSnapshotManifest],
) -> Result<(), BackupError> {
    let listed = manifests
        .iter()
        .map(|manifest| manifest.snapshot_id)
        .collect::<HashSet<_>>();
    for manifest in manifests {
        let Some(parent) = manifest.parent_snapshot_id else {
            continue;
        };
        if !listed.contains(&parent)
            && repository
                .blob_exists(&snapshot_manifest_key(parent))
                .await?
        {
            return Err(incomplete_listing(format!(
                "parent snapshot {parent} of snapshot {} is stored but was not listed",
                manifest.snapshot_id
            )));
        }
    }
    Ok(())
}

/// Re-lists manifests immediately before the object sweep and refuses to
/// delete objects unless the set matches the snapshots the sweep keeps.
async fn ensure_stable_listing(
    repository: &dyn BackupRepository,
    expected: &HashSet<Uuid>,
) -> Result<(), BackupError> {
    let listed = list_manifest_ids(repository).await?;
    if listed != *expected {
        let mut unexpected = listed.difference(expected).copied().collect::<Vec<_>>();
        let mut missing = expected.difference(&listed).copied().collect::<Vec<_>>();
        unexpected.sort();
        missing.sort();
        return Err(incomplete_listing(format!(
            "manifest listing changed before the object sweep (newly listed: {unexpected:?}, \
             no longer listed: {missing:?})"
        )));
    }
    Ok(())
}

fn incomplete_listing(detail: String) -> BackupError {
    BackupError::InvalidManifestChain {
        reason: format!(
            "snapshot listing is incomplete; refusing to delete unreferenced objects: {detail}"
        ),
    }
}

fn snapshot_id_from_manifest_key(key: &str) -> Option<Uuid> {
    let rest = key.strip_prefix("snapshots/")?;
    let snapshot_id = rest.strip_suffix("/manifest.json")?;
    if snapshot_id.contains('/') {
        return None;
    }
    Uuid::parse_str(snapshot_id).ok()
}

fn collect_reachable_keys(
    manifest: &BackupSnapshotManifest,
    reachable_content_keys: &mut HashSet<String>,
) {
    for table in &manifest.database.tables {
        reachable_content_keys.insert(table.content_key.clone());
    }
    for change in &manifest.database.changes {
        reachable_content_keys.insert(change.content_key.clone());
    }
    for object in &manifest.objects {
        reachable_content_keys.insert(object.content_key.clone());
    }
}
