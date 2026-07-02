use std::collections::HashSet;

use uuid::Uuid;

use crate::{
    BackupError, BackupRepository, BackupSnapshotManifest, RepositoryLock, RepositoryLockOptions,
    load_manifest, load_manifest_chain,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeepPolicy {
    pub keep_last: usize,
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
pub struct PruneResult {
    pub retained_snapshots: usize,
    pub deleted_snapshots: usize,
    pub deleted_blobs: usize,
}

/// Prunes expired snapshots and unreferenced object blobs.
///
/// # Errors
///
/// Returns [`BackupError`] if repository listing, manifest loading, locking, or
/// blob deletion fails.
pub async fn prune(
    repository: &dyn BackupRepository,
    keep_policy: &KeepPolicy,
) -> Result<PruneResult, BackupError> {
    let lock = RepositoryLock::acquire(repository, &keep_policy.lock).await?;
    let result = prune_inner(repository, keep_policy).await;
    let release_result = lock.release(repository).await;
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
