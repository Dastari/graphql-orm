use async_trait::async_trait;
use bytes::Bytes;
use graphql_orm_backup::{
    BackupCapacityLimits, BackupChangeExport, BackupError, BackupObjectIndex, BackupObjectRef,
    BackupRepository, BackupTableExport, FullBackupRequest, GraphqlOrmBackupAdapter,
    GraphqlOrmBackupSchema, IncrementalBackupRequest, KeepPolicy, LocalBackupRepository,
    RepositoryLockOptions, RestoreContext, bytes_sha256_hex, create_full_backup,
    create_incremental_backup, delete_snapshot, object_content_key, prune, snapshot_manifest_key,
};
use graphql_orm_storage::StorageByteStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;
use uuid::Uuid;

#[tokio::test]
async fn create_full_backup_refuses_active_repository_lock() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path())
        .with_manifest_authenticator(test_manifest_authenticator());
    repository
        .put_blob(
            "locks/repository.lock",
            Bytes::from(unix_seconds().to_string()),
        )
        .await
        .expect("write lock");

    let database = MockDatabase;
    let objects = MockObjectIndex::default();
    let err = create_full_backup(
        &repository,
        &database,
        &objects,
        backup_request(first_id(), 1),
    )
    .await
    .expect_err("active lock rejected");

    assert!(matches!(err, BackupError::RepositoryLocked { .. }));
}

#[tokio::test]
async fn prune_deletes_expired_snapshots_and_unreferenced_objects() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path())
        .with_manifest_authenticator(test_manifest_authenticator());
    let database = MockDatabase;

    let first_object = Bytes::from_static(b"first object");
    let first_hash = bytes_sha256_hex(&first_object);
    let first_objects = MockObjectIndex::new(first_hash.clone(), first_object);
    create_full_backup(
        &repository,
        &database,
        &first_objects,
        backup_request(first_id(), 1),
    )
    .await
    .expect("first backup");

    let second_object = Bytes::from_static(b"second object");
    let second_hash = bytes_sha256_hex(&second_object);
    let second_objects = MockObjectIndex::new(second_hash.clone(), second_object);
    create_full_backup(
        &repository,
        &database,
        &second_objects,
        backup_request(second_id(), 2),
    )
    .await
    .expect("second backup");

    let result = prune(
        &repository,
        &KeepPolicy {
            keep_last: 1,
            ..KeepPolicy::default()
        },
    )
    .await
    .expect("prune repository");

    assert_eq!(result.retained_snapshots, 1);
    assert_eq!(result.deleted_snapshots, 1);
    assert!(
        !repository
            .blob_exists(&snapshot_manifest_key(first_id()))
            .await
            .expect("first manifest exists check")
    );
    assert!(
        repository
            .blob_exists(&snapshot_manifest_key(second_id()))
            .await
            .expect("second manifest exists check")
    );
    assert!(
        !repository
            .blob_exists(&object_content_key(&first_hash))
            .await
            .expect("first object exists check")
    );
    assert!(
        repository
            .blob_exists(&object_content_key(&second_hash))
            .await
            .expect("second object exists check")
    );
}

#[tokio::test]
async fn delete_snapshot_removes_snapshot_and_unreferenced_objects() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path())
        .with_manifest_authenticator(test_manifest_authenticator());
    let database = MockDatabase;

    let first_object = Bytes::from_static(b"first object");
    let first_hash = bytes_sha256_hex(&first_object);
    let first_objects = MockObjectIndex::new(first_hash.clone(), first_object);
    create_full_backup(
        &repository,
        &database,
        &first_objects,
        backup_request(first_id(), 1),
    )
    .await
    .expect("first backup");

    let second_object = Bytes::from_static(b"second object");
    let second_hash = bytes_sha256_hex(&second_object);
    let second_objects = MockObjectIndex::new(second_hash.clone(), second_object);
    create_full_backup(
        &repository,
        &database,
        &second_objects,
        backup_request(second_id(), 2),
    )
    .await
    .expect("second backup");

    let result = delete_snapshot(&repository, first_id(), &RepositoryLockOptions::default())
        .await
        .expect("delete first snapshot");
    assert_eq!(result.retained_snapshots, 1);
    assert!(result.deleted_blobs >= 2);

    assert!(
        !repository
            .blob_exists(&snapshot_manifest_key(first_id()))
            .await
            .expect("first manifest exists check")
    );
    assert!(
        repository
            .blob_exists(&snapshot_manifest_key(second_id()))
            .await
            .expect("second manifest exists check")
    );
    assert!(
        !repository
            .blob_exists(&object_content_key(&first_hash))
            .await
            .expect("first object exists check")
    );
    assert!(
        repository
            .blob_exists(&object_content_key(&second_hash))
            .await
            .expect("second object exists check")
    );

    let missing = delete_snapshot(&repository, first_id(), &RepositoryLockOptions::default())
        .await
        .expect_err("deleting a missing snapshot fails");
    assert!(matches!(missing, BackupError::MissingBlob { .. }));
}

fn backup_request(snapshot_id: Uuid, created_at: i64) -> FullBackupRequest {
    FullBackupRequest {
        snapshot_id,
        created_at,
        app_id: "example-app".to_string(),
        app_version: "0.1.0".to_string(),
    }
}

fn first_id() -> Uuid {
    Uuid::parse_str("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa").expect("valid uuid")
}

fn second_id() -> Uuid {
    Uuid::parse_str("bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb").expect("valid uuid")
}

fn object_id() -> Uuid {
    Uuid::parse_str("cccccccc-cccc-4ccc-cccc-cccccccccccc").expect("valid uuid")
}

fn unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

struct MockDatabase;

#[async_trait]
impl GraphqlOrmBackupAdapter for MockDatabase {
    async fn schema_snapshot(&self) -> Result<GraphqlOrmBackupSchema, BackupError> {
        Ok(GraphqlOrmBackupSchema {
            backend: "sqlite".to_string(),
            migration_version: "20260514000000".to_string(),
            schema_hash: "schema-hash".to_string(),
        })
    }

    async fn export_full(&self) -> Result<Vec<BackupTableExport>, BackupError> {
        Ok(vec![BackupTableExport {
            table_name: "users".to_string(),
            rows: Vec::new(),
        }])
    }

    async fn export_incremental(
        &self,
        _parent_snapshot_id: Uuid,
    ) -> Result<Vec<BackupChangeExport>, BackupError> {
        Ok(Vec::new())
    }

    async fn restore_full(
        &self,
        _backup_schema: GraphqlOrmBackupSchema,
        _export: Vec<BackupTableExport>,
        _context: RestoreContext,
    ) -> Result<(), BackupError> {
        Ok(())
    }

    async fn restore_incremental(
        &self,
        _changes: Vec<BackupChangeExport>,
        _context: RestoreContext,
    ) -> Result<(), BackupError> {
        Ok(())
    }
}

#[derive(Default)]
struct MockObjectIndex {
    object: Option<BackupObjectRef>,
    bytes: Bytes,
}

impl MockObjectIndex {
    fn new(hash: String, bytes: Bytes) -> Self {
        Self {
            object: Some(BackupObjectRef {
                object_id: object_id(),
                storage_key: format!("objects/{hash}.txt"),
                sha256_hex: hash,
                size_bytes: bytes.len() as u64,
                mime_type: Some("text/plain".to_string()),
            }),
            bytes,
        }
    }
}

#[async_trait]
impl BackupObjectIndex for MockObjectIndex {
    async fn list_objects_for_full_backup(&self) -> Result<Vec<BackupObjectRef>, BackupError> {
        Ok(self.object.iter().cloned().collect())
    }

    async fn list_objects_for_incremental_backup(
        &self,
        _since_snapshot_id: Uuid,
    ) -> Result<Vec<BackupObjectRef>, BackupError> {
        Ok(Vec::new())
    }

    async fn load_object(&self, _object: &BackupObjectRef) -> Result<Bytes, BackupError> {
        Ok(self.bytes.clone())
    }
}

#[tokio::test]
async fn expired_holder_cannot_be_taken_over_or_release_a_new_owner() {
    use graphql_orm_backup::RepositoryLock;
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path())
        .with_manifest_authenticator(test_manifest_authenticator());
    let options = RepositoryLockOptions {
        stale_after_seconds: -1,
    };
    let first = RepositoryLock::acquire(&repository, &options)
        .await
        .expect("first");
    // Deterministically simulate a writer running beyond its former TTL.
    repository
        .put_blob("locks/repository.lock", Bytes::from_static(b"0"))
        .await
        .expect("age lock");
    let (second, third) = tokio::join!(
        RepositoryLock::acquire(&repository, &options),
        RepositoryLock::acquire(&repository, &options),
    );
    assert!(matches!(second, Err(BackupError::RepositoryLocked { .. })));
    assert!(matches!(third, Err(BackupError::RepositoryLocked { .. })));
    assert_eq!(
        repository
            .get_blob("locks/repository.lock")
            .await
            .expect("lock intact"),
        "0"
    );
    first.release().await.expect("owner releases");
    let next = RepositoryLock::acquire(&repository, &options)
        .await
        .expect("next owner");
    assert!(matches!(
        RepositoryLock::acquire(&repository, &options).await,
        Err(BackupError::RepositoryLocked { .. })
    ));
    next.release().await.expect("release next");
}

#[tokio::test]
async fn dropped_holder_leaves_lock_for_offline_recovery() {
    use graphql_orm_backup::RepositoryLock;
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path())
        .with_manifest_authenticator(test_manifest_authenticator());
    let options = RepositoryLockOptions::default();
    {
        let _holder = RepositoryLock::acquire(&repository, &options)
            .await
            .expect("holder");
    }
    assert!(matches!(
        RepositoryLock::acquire(&repository, &options).await,
        Err(BackupError::RepositoryLocked { .. })
    ));
}

/// Omits one manifest from snapshot listings, as a truncated provider listing
/// would (GHSA-439h-2cx8-xwgx). `hidden_listings` bounds how many listings omit
/// it; `usize::MAX` hides it from every listing.
struct TruncatedListingRepository {
    inner: LocalBackupRepository,
    hidden_key: String,
    hidden_listings: AtomicUsize,
}

impl TruncatedListingRepository {
    fn new(inner: LocalBackupRepository, hidden_key: String, hidden_listings: usize) -> Self {
        Self {
            inner,
            hidden_key,
            hidden_listings: AtomicUsize::new(hidden_listings),
        }
    }
}

#[async_trait]
impl BackupRepository for TruncatedListingRepository {
    fn capacity_limits(&self) -> BackupCapacityLimits {
        self.inner.capacity_limits()
    }
    fn manifest_authenticator(&self) -> Option<&graphql_orm_backup::ManifestAuthenticator> {
        self.inner.manifest_authenticator()
    }
    async fn delete_blob_once(&self, key: &str) -> Result<(), BackupError> {
        self.inner.delete_blob_once(key).await
    }
    async fn put_blob(&self, key: &str, body: Bytes) -> Result<(), BackupError> {
        self.inner.put_blob(key, body).await
    }
    async fn put_blob_if_absent(&self, key: &str, body: Bytes) -> Result<bool, BackupError> {
        self.inner.put_blob_if_absent(key, body).await
    }
    async fn get_blob(&self, key: &str) -> Result<Bytes, BackupError> {
        self.inner.get_blob(key).await
    }
    async fn get_blob_stream(&self, key: &str) -> Result<StorageByteStream, BackupError> {
        self.inner.get_blob_stream(key).await
    }
    async fn blob_exists(&self, key: &str) -> Result<bool, BackupError> {
        self.inner.blob_exists(key).await
    }
    async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, BackupError> {
        let mut keys = self.inner.list_blobs(prefix).await?;
        if prefix == "snapshots"
            && self
                .hidden_listings
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| match left {
                    0 => None,
                    usize::MAX => Some(usize::MAX),
                    left => Some(left - 1),
                })
                .is_ok()
        {
            keys.retain(|key| key != &self.hidden_key);
        }
        Ok(keys)
    }
    async fn delete_blob(&self, key: &str) -> Result<(), BackupError> {
        self.inner.delete_blob(key).await
    }
}

async fn two_full_snapshots(repository: &LocalBackupRepository) -> (String, String) {
    let first_object = Bytes::from_static(b"first object");
    let first_hash = bytes_sha256_hex(&first_object);
    create_full_backup(
        repository,
        &MockDatabase,
        &MockObjectIndex::new(first_hash.clone(), first_object),
        backup_request(first_id(), 1),
    )
    .await
    .expect("first backup");
    let second_object = Bytes::from_static(b"second object");
    let second_hash = bytes_sha256_hex(&second_object);
    create_full_backup(
        repository,
        &MockDatabase,
        &MockObjectIndex::new(second_hash.clone(), second_object),
        backup_request(second_id(), 2),
    )
    .await
    .expect("second backup");
    (first_hash, second_hash)
}

#[tokio::test]
async fn prune_refuses_to_sweep_objects_after_a_truncated_snapshot_listing() {
    let temp = TempDir::new().expect("temp dir");
    let local = LocalBackupRepository::new(temp.path())
        .with_manifest_authenticator(test_manifest_authenticator());
    let (first_hash, second_hash) = two_full_snapshots(&local).await;
    // The newest manifest is missing from the listing prune plans with.
    let repository = TruncatedListingRepository::new(local, snapshot_manifest_key(second_id()), 1);

    let error = prune(
        &repository,
        &KeepPolicy {
            keep_last: 1,
            ..KeepPolicy::default()
        },
    )
    .await
    .expect_err("an incomplete listing must not drive the object sweep");

    assert!(
        matches!(&error, BackupError::InvalidManifestChain { reason } if reason.contains("listing is incomplete")),
        "unexpected error: {error:?}"
    );
    for key in [
        snapshot_manifest_key(first_id()),
        snapshot_manifest_key(second_id()),
        object_content_key(&first_hash),
        object_content_key(&second_hash),
    ] {
        assert!(
            repository.blob_exists(&key).await.expect("exists check"),
            "{key} must survive"
        );
    }
}

#[tokio::test]
async fn delete_snapshot_refuses_a_listing_that_omits_a_stored_parent() {
    let temp = TempDir::new().expect("temp dir");
    let local = LocalBackupRepository::new(temp.path())
        .with_manifest_authenticator(test_manifest_authenticator());
    let object = Bytes::from_static(b"parent object");
    let hash = bytes_sha256_hex(&object);
    create_full_backup(
        &local,
        &MockDatabase,
        &MockObjectIndex::new(hash.clone(), object),
        backup_request(first_id(), 1),
    )
    .await
    .expect("parent backup");
    create_incremental_backup(
        &local,
        &MockDatabase,
        &MockObjectIndex::default(),
        IncrementalBackupRequest {
            snapshot_id: second_id(),
            parent_snapshot_id: first_id(),
            created_at: 2,
            app_id: "example-app".to_string(),
            app_version: "0.1.0".to_string(),
        },
    )
    .await
    .expect("child backup");
    let repository =
        TruncatedListingRepository::new(local, snapshot_manifest_key(first_id()), usize::MAX);

    let error = delete_snapshot(&repository, second_id(), &RepositoryLockOptions::default())
        .await
        .expect_err("a listing that omits a stored parent is incomplete");

    assert!(
        matches!(&error, BackupError::InvalidManifestChain { reason } if reason.contains("stored but was not listed")),
        "unexpected error: {error:?}"
    );
    assert!(
        repository
            .blob_exists(&object_content_key(&hash))
            .await
            .expect("exists check")
    );
    assert!(
        repository
            .blob_exists(&snapshot_manifest_key(second_id()))
            .await
            .expect("exists check")
    );
}

fn third_id() -> Uuid {
    Uuid::parse_str("dddddddd-dddd-4ddd-dddd-dddddddddddd").expect("valid uuid")
}

async fn incremental_child(repository: &LocalBackupRepository, child: Uuid, parent: Uuid, at: i64) {
    create_incremental_backup(
        repository,
        &MockDatabase,
        &MockObjectIndex::default(),
        IncrementalBackupRequest {
            snapshot_id: child,
            parent_snapshot_id: parent,
            created_at: at,
            app_id: "example-app".to_string(),
            app_version: "0.1.0".to_string(),
        },
    )
    .await
    .expect("child backup");
}

/// Every key of the three fixture snapshot trees. Per-snapshot prefixes are
/// never truncated by the wrapper.
async fn snapshot_keys(repository: &dyn BackupRepository) -> Vec<String> {
    let mut keys = Vec::new();
    for snapshot_id in [first_id(), second_id(), third_id()] {
        keys.extend(
            repository
                .list_blobs(&format!("snapshots/{snapshot_id}"))
                .await
                .expect("list snapshot tree"),
        );
    }
    keys
}

#[tokio::test]
async fn delete_snapshot_refuses_a_parent_whose_child_was_transiently_unlisted() {
    let temp = TempDir::new().expect("temp dir");
    let local = LocalBackupRepository::new(temp.path())
        .with_manifest_authenticator(test_manifest_authenticator());
    let object = Bytes::from_static(b"parent object");
    let hash = bytes_sha256_hex(&object);
    create_full_backup(
        &local,
        &MockDatabase,
        &MockObjectIndex::new(hash.clone(), object),
        backup_request(first_id(), 1),
    )
    .await
    .expect("parent backup");
    incremental_child(&local, second_id(), first_id(), 2).await;
    // The child is missing from the listing the dependency check uses.
    let before = snapshot_keys(&local).await;
    let repository = TruncatedListingRepository::new(local, snapshot_manifest_key(second_id()), 1);

    let error = delete_snapshot(&repository, first_id(), &RepositoryLockOptions::default())
        .await
        .expect_err("a hidden child must not let its parent be deleted");

    assert!(
        matches!(&error, BackupError::InvalidManifestChain { reason } if reason.contains("listing is incomplete")),
        "unexpected error: {error:?}"
    );
    assert_eq!(
        snapshot_keys(&repository).await,
        before,
        "no snapshot data may be deleted"
    );
    assert!(
        repository
            .blob_exists(&object_content_key(&hash))
            .await
            .expect("exists check")
    );
}

#[tokio::test]
async fn prune_refuses_to_expire_the_parent_of_a_transiently_unlisted_newest_child() {
    let temp = TempDir::new().expect("temp dir");
    let local = LocalBackupRepository::new(temp.path())
        .with_manifest_authenticator(test_manifest_authenticator());
    let object = Bytes::from_static(b"chain object");
    let hash = bytes_sha256_hex(&object);
    create_full_backup(
        &local,
        &MockDatabase,
        &MockObjectIndex::new(hash.clone(), object),
        backup_request(first_id(), 1),
    )
    .await
    .expect("full A");
    create_full_backup(
        &local,
        &MockDatabase,
        &MockObjectIndex::default(),
        backup_request(third_id(), 2),
    )
    .await
    .expect("full C");
    // B is the newest snapshot and an incremental child of A.
    incremental_child(&local, second_id(), first_id(), 3).await;
    let before = snapshot_keys(&local).await;
    let repository = TruncatedListingRepository::new(local, snapshot_manifest_key(second_id()), 1);

    let error = prune(
        &repository,
        &KeepPolicy {
            keep_last: 1,
            ..KeepPolicy::default()
        },
    )
    .await
    .expect_err("a hidden newest child must not expire its parent");

    assert!(
        matches!(&error, BackupError::InvalidManifestChain { reason } if reason.contains("listing is incomplete")),
        "unexpected error: {error:?}"
    );
    assert_eq!(
        snapshot_keys(&repository).await,
        before,
        "no snapshot data may be deleted"
    );
    assert!(
        repository
            .blob_exists(&object_content_key(&hash))
            .await
            .expect("exists check")
    );
}

fn test_manifest_authenticator() -> graphql_orm_backup::ManifestAuthenticator {
    graphql_orm_backup::ManifestAuthenticator::new(
        graphql_orm_backup::ManifestKey::from_bytes(&[7; 32]).expect("test manifest key"),
    )
}
