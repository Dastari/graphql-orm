use async_trait::async_trait;
use bytes::Bytes;
use graphql_orm_backup::{
    BackupChangeExport, BackupError, BackupObjectIndex, BackupObjectRef, BackupRepository,
    BackupTableExport, FullBackupRequest, GraphqlOrmBackupAdapter, GraphqlOrmBackupSchema,
    KeepPolicy, LocalBackupRepository, RepositoryLockOptions, RestoreContext, bytes_sha256_hex,
    create_full_backup, delete_snapshot, object_content_key, prune, snapshot_manifest_key,
};
use tempfile::TempDir;
use uuid::Uuid;

#[tokio::test]
async fn create_full_backup_refuses_active_repository_lock() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path());
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
    let repository = LocalBackupRepository::new(temp.path());
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
    let repository = LocalBackupRepository::new(temp.path());
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
