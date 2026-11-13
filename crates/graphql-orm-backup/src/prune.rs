use std::collections::HashSet;

use uuid::Uuid;

use crate::{
    BackupError, BackupRepository, BackupSnapshotManifest, RepositoryLock, RepositoryLockOptions,
    backup::snapshot_blob_keys, load_manifest, load_manifest_chain,
    manifest::required_authenticator, snapshot_manifest_key,
};

#[derive(Clone, Debug, Eq, PartialEq)]
/// Retention policy for repository pruning.
pub struct KeepPolicy {
    /// Number of newest manifest chains to retain. Must be at least one:
    /// [`prune`] refuses `0`, which would delete every snapshot. Remove a
    /// repository's last snapshot explicitly with [`delete_snapshot`].
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
    /// Number of blobs deleted, including orphaned payloads.
    pub deleted_blobs: usize,
    /// Number of deleted snapshot payloads that no manifest owned, such as
    /// those left by a crashed backup.
    pub deleted_orphan_blobs: usize,
}

/// Prunes expired snapshots and unreferenced object blobs.
///
/// Retention and object reachability are derived from the snapshot listing,
/// so pruning fails closed when that listing is incomplete. It aborts with
/// [`BackupError::IncompleteRepositoryListing`] before deleting any snapshot or object
/// if a listed manifest's parent is stored but was not listed, or if
/// re-listing manifests gives a different set. It lists manifests once more
/// immediately before the object sweep and aborts, without deleting any
/// object, if the set is not exactly the retained snapshots. A listing that is
/// truncated the same way every time cannot be detected here; providers must
/// fail such listings themselves.
///
/// It also removes orphaned snapshot payloads: blobs below a
/// `snapshots/{snapshot_id}/` directory that has no manifest, such as those a
/// crashed backup left. Before removing a directory it checks that no
/// manifest is stored there, and aborts without deleting it if one is, since
/// the listing then omitted a snapshot. Keys outside canonical snapshot
/// directories are left alone. Because every writer holds the repository
/// lock, no running backup's payloads can be mistaken for orphans.
///
/// # Errors
///
/// Returns [`BackupError::InvalidRetentionPolicy`] for `keep_last: 0` without
/// touching the repository, or another [`BackupError`] if repository listing,
/// manifest loading, locking, or blob deletion fails, or if the snapshot
/// listing is incomplete.
pub async fn prune(
    repository: &dyn BackupRepository,
    keep_policy: &KeepPolicy,
) -> Result<PruneResult, BackupError> {
    if keep_policy.keep_last == 0 {
        return Err(BackupError::InvalidRetentionPolicy {
            reason: "keep_last must be at least 1; 0 would delete every snapshot".to_string(),
        });
    }
    // Refuse before the lock write: without a key no manifest can be trusted.
    required_authenticator(repository)?;
    let lock = RepositoryLock::acquire_for(repository, &keep_policy.lock, "prune").await?;
    let result = prune_inner(repository, keep_policy).await;
    lock.finish(result, |_| None).await
}

async fn prune_inner(
    repository: &dyn BackupRepository,
    keep_policy: &KeepPolicy,
) -> Result<PruneResult, BackupError> {
    let mut manifests = load_all_manifests(repository).await?;
    ensure_listed_parents(repository, &manifests).await?;
    // A listing that hid a newer child would expire its parent: confirm the
    // listing before deleting anything.
    ensure_stable_listing(repository, &manifest_ids(&manifests)).await?;
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

    let all_ids = manifest_ids(&manifests);
    let expired_ids = all_ids
        .difference(&retained_ids)
        .copied()
        .collect::<Vec<_>>();

    let mut deleted_blobs = 0_usize;
    for snapshot_id in &expired_ids {
        for key in snapshot_blob_keys(repository, *snapshot_id).await? {
            repository.delete_blob(&key).await?;
            deleted_blobs += 1;
        }
    }

    ensure_stable_listing(repository, &retained_ids).await?;
    let deleted_orphan_blobs = delete_orphaned_payloads(repository, &retained_ids).await?;
    deleted_blobs += deleted_orphan_blobs;
    for key in content_blob_keys(repository).await? {
        if !reachable_content_keys.contains(&key) {
            repository.delete_blob(&key).await?;
            deleted_blobs += 1;
        }
    }

    Ok(PruneResult {
        retained_snapshots: retained_ids.len(),
        deleted_snapshots: expired_ids.len(),
        deleted_blobs,
        deleted_orphan_blobs,
    })
}

/// Deletes blobs in snapshot directories that hold no manifest.
///
/// A directory whose manifest is stored but was not listed aborts the sweep
/// before anything in it is deleted.
async fn delete_orphaned_payloads(
    repository: &dyn BackupRepository,
    retained_ids: &HashSet<Uuid>,
) -> Result<usize, BackupError> {
    let mut checked = HashSet::new();
    let mut deleted = 0_usize;
    for key in repository.list_blobs("snapshots").await? {
        let Some(snapshot_id) = snapshot_id_from_payload_key(&key) else {
            continue;
        };
        if retained_ids.contains(&snapshot_id) {
            continue;
        }
        if checked.insert(snapshot_id)
            && repository
                .blob_exists(&snapshot_manifest_key(snapshot_id))
                .await?
        {
            return Err(incomplete_listing(format!(
                "snapshot {snapshot_id} has a stored manifest that was not listed"
            )));
        }
        repository.delete_blob(&key).await?;
        deleted += 1;
    }
    Ok(deleted)
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
/// dependent snapshots must be deleted or compacted first. Like [`prune`], it
/// fails closed when the snapshot listing is incomplete: a stored but unlisted
/// parent, or a different manifest set on re-listing, aborts before the
/// snapshot or any object is deleted, and the manifest set is checked again
/// before the object sweep.
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
    required_authenticator(repository)?;
    let lock = RepositoryLock::acquire_for(repository, lock_options, "delete_snapshot").await?;
    let result = delete_snapshot_inner(repository, snapshot_id).await;
    lock.finish(result, |_| None).await
}

async fn delete_snapshot_inner(
    repository: &dyn BackupRepository,
    snapshot_id: Uuid,
) -> Result<DeleteSnapshotResult, BackupError> {
    let manifests = load_all_manifests(repository).await?;
    ensure_listed_parents(repository, &manifests).await?;
    // A listing that hid a child would pass the dependency check below.
    ensure_stable_listing(repository, &manifest_ids(&manifests)).await?;
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
    for key in snapshot_blob_keys(repository, snapshot_id).await? {
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
    for key in content_blob_keys(repository).await? {
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

fn manifest_ids(manifests: &[BackupSnapshotManifest]) -> HashSet<Uuid> {
    manifests
        .iter()
        .map(|manifest| manifest.snapshot_id)
        .collect()
}

/// Re-lists manifests and refuses to continue unless the set is exactly
/// `expected`: the first listing before any deletion, or the retained
/// snapshots before the object sweep.
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
            "manifest listing changed between reads (newly listed: {unexpected:?}, \
             no longer listed: {missing:?})"
        )));
    }
    Ok(())
}

fn incomplete_listing(detail: String) -> BackupError {
    BackupError::IncompleteRepositoryListing {
        reason: format!(
            "snapshot listing is incomplete; refusing to delete snapshots or objects: {detail}"
        ),
    }
}

/// Every stored blob below the content directory, `objects/sha256/`. A
/// listing prefix is a string prefix, so siblings such as
/// `objects/sha256-legacy/` are filtered out before anything is deleted.
async fn content_blob_keys(repository: &dyn BackupRepository) -> Result<Vec<String>, BackupError> {
    let mut keys = repository.list_blobs("objects/sha256").await?;
    keys.retain(|key| key.starts_with("objects/sha256/"));
    Ok(keys)
}

fn snapshot_id_from_manifest_key(key: &str) -> Option<Uuid> {
    let rest = key.strip_prefix("snapshots/")?;
    let snapshot_id = rest.strip_suffix("/manifest.json")?;
    // Accept only the canonical lowercase hyphenated form that this crate
    // writes, so each listed key names exactly one manifest path.
    let parsed = Uuid::parse_str(snapshot_id).ok()?;
    (parsed.hyphenated().to_string() == snapshot_id).then_some(parsed)
}

/// Snapshot id of a key inside a canonical `snapshots/{snapshot_id}/` directory.
fn snapshot_id_from_payload_key(key: &str) -> Option<Uuid> {
    let rest = key.strip_prefix("snapshots/")?;
    let (snapshot_id, _) = rest.split_once('/')?;
    let parsed = Uuid::parse_str(snapshot_id).ok()?;
    (parsed.hyphenated().to_string() == snapshot_id).then_some(parsed)
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

#[cfg(test)]
mod tests {
    use super::snapshot_id_from_manifest_key;

    #[test]
    fn only_canonical_manifest_keys_name_a_snapshot() {
        let id = "bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb";
        assert!(snapshot_id_from_manifest_key(&format!("snapshots/{id}/manifest.json")).is_some());
        for key in [
            "snapshots/BBBBBBBB-BBBB-4BBB-BBBB-BBBBBBBBBBBB/manifest.json",
            "snapshots/bbbbbbbbbbbb4bbbbbbbbbbbbbbbbbbb/manifest.json",
            "snapshots/{bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb}/manifest.json",
            "snapshots/urn:uuid:bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb/manifest.json",
            "snapshots/x/bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb/manifest.json",
        ] {
            assert!(snapshot_id_from_manifest_key(key).is_none(), "{key}");
        }
    }
}
