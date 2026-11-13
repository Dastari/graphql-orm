use std::{
    env,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use bytes::Bytes;
use graphql_orm_backup::{
    BackupChangeExport, BackupError, BackupObjectIndex, BackupObjectRef, BackupRepository,
    BackupRow, BackupTableExport, BlobStoreBackupRepository, BlobStoreRestoreObjectSink,
    FullBackupRequest, GraphqlOrmBackupAdapter, GraphqlOrmBackupSchema, KeepPolicy, RepositoryLock,
    RepositoryLockOptions, RestoreContext, bytes_sha256_hex, create_full_backup, delete_snapshot,
    load_manifest, prune, restore_objects, restore_snapshot, verify_manifest_and_objects,
};
use graphql_orm_storage::{
    BlobStore, SmbDialect, SmbStorageBackend, SmbStorageConfig, StorageByteStream,
    collect_storage_stream,
};
use secrecy::SecretString;
use serde_json::{Map, Value};
use uuid::Uuid;

fn config(prefix: String) -> SmbStorageConfig {
    let mut config = SmbStorageConfig::new(
        env::var("SMB_TEST_SERVER").unwrap_or_else(|_| "127.0.0.1".to_string()),
        env::var("SMB_TEST_SHARE").unwrap_or_else(|_| "backups".to_string()),
        env::var("SMB_TEST_USERNAME").unwrap_or_else(|_| "backup".to_string()),
        SecretString::from(
            env::var("SMB_TEST_PASSWORD").unwrap_or_else(|_| "BackupTest-42!".to_string()),
        ),
    );
    config.port = env::var("SMB_TEST_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1445);
    config.domain = env::var("SMB_TEST_DOMAIN").ok();
    config.root_prefix = Some(prefix);
    config.min_dialect = SmbDialect::Smb2_1;
    config.connect_timeout = Duration::from_secs(5);
    config.operation_timeout = Duration::from_secs(30);
    config
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires SMB_TEST_* or the documented Samba container"]
async fn full_smb_snapshot_lifecycle_and_locking() {
    let test_root = format!("graphql-orm-backup-tests/{}", Uuid::new_v4());
    let repository_store: Arc<dyn BlobStore> = Arc::new(
        SmbStorageBackend::connect(config(format!("{test_root}/repository")))
            .await
            .expect("repository SMB connect"),
    );
    let repository = BlobStoreBackupRepository::new(repository_store)
        .with_manifest_authenticator(test_manifest_authenticator());
    let restore_store: Arc<dyn BlobStore> = Arc::new(
        SmbStorageBackend::connect(config(format!("{test_root}/restored-objects")))
            .await
            .expect("restore SMB connect"),
    );

    let object_bytes = Bytes::from(vec![0x6d; 2 * 1024 * 1024]);
    let object_hash = bytes_sha256_hex(&object_bytes);
    let object = BackupObjectRef {
        object_id: Uuid::new_v4(),
        storage_key: "originals/large-object.bin".to_string(),
        sha256_hex: object_hash,
        size_bytes: object_bytes.len() as u64,
        mime_type: Some("application/octet-stream".to_string()),
    };
    let objects = TestObjects {
        object,
        bytes: object_bytes,
    };
    let database = TestDatabase::default();
    let first_id = Uuid::new_v4();
    let first = create_full_backup(&repository, &database, &objects, request(first_id, 1))
        .await
        .expect("create first snapshot");
    let loaded = load_manifest(&repository, first_id)
        .await
        .expect("load first manifest");
    assert_eq!(loaded, first.manifest);
    verify_manifest_and_objects(&repository, &loaded)
        .await
        .expect("verify first snapshot");

    restore_snapshot(
        &repository,
        &database,
        first_id,
        RestoreContext::empty_database(),
    )
    .await
    .expect("restore database payload");
    assert_eq!(database.restored.lock().expect("restored lock").len(), 1);
    restore_objects(
        &repository,
        &loaded,
        &BlobStoreRestoreObjectSink::new(
            Arc::clone(&restore_store),
            graphql_orm_backup::RestoreKeyScope::prefixes(["originals"]).expect("restore scope"),
        ),
    )
    .await
    .expect("restore stored object");
    let restored = restore_store
        .get_blob("originals/large-object.bin")
        .await
        .expect("load restored object");
    assert_eq!(
        collect_storage_stream(restored.body)
            .await
            .expect("collect restored object"),
        objects.bytes
    );

    // A second read that serves other bytes fails at the end of the stream, so
    // SMB's conditional create discards the file instead of committing it.
    let swap_store: Arc<dyn BlobStore> = Arc::new(
        SmbStorageBackend::connect(config(format!("{test_root}/swap-restore")))
            .await
            .expect("swap restore SMB connect"),
    );
    let swapping = SwappingRepository {
        inner: repository.clone(),
        key: loaded.objects[0].content_key.clone(),
        forged: Bytes::from(vec![0x66; objects.bytes.len()]),
        reads: Arc::default(),
    };
    let error = restore_objects(
        &swapping,
        &loaded,
        &BlobStoreRestoreObjectSink::new(
            Arc::clone(&swap_store),
            graphql_orm_backup::RestoreKeyScope::prefixes(["originals"]).expect("restore scope"),
        ),
    )
    .await
    .expect_err("swapped object refused");
    assert!(matches!(error, BackupError::ChecksumMismatch { .. }));
    assert!(
        !swap_store
            .blob_exists("originals/large-object.bin")
            .await
            .expect("swap target exists check"),
        "SMB discarded the aborted conditional create"
    );
    assert!(
        swap_store
            .list_blobs("")
            .await
            .expect("list swap store")
            .is_empty(),
        "no temporary file remains"
    );

    let second_id = Uuid::new_v4();
    create_full_backup(&repository, &database, &objects, request(second_id, 2))
        .await
        .expect("create second snapshot");
    let pruned = prune(
        &repository,
        &KeepPolicy {
            keep_last: 1,
            lock: RepositoryLockOptions::default(),
        },
    )
    .await
    .expect("prune old snapshot");
    assert_eq!(pruned.deleted_snapshots, 1);
    assert!(
        !repository
            .blob_exists(&format!("snapshots/{first_id}/manifest.json"))
            .await
            .expect("first manifest exists")
    );

    let left_options = RepositoryLockOptions::default();
    let right_options = RepositoryLockOptions::default();
    let (left, right) = tokio::join!(
        RepositoryLock::acquire(&repository, &left_options),
        RepositoryLock::acquire(&repository, &right_options)
    );
    let acquired = match (left, right) {
        (Ok(lock), Err(BackupError::RepositoryLocked { .. }))
        | (Err(BackupError::RepositoryLocked { .. }), Ok(lock)) => lock,
        other => panic!("exactly one repository lock must succeed: {other:?}"),
    };
    acquired.release().await.expect("release lock");

    let deleted = delete_snapshot(&repository, second_id, &RepositoryLockOptions::default())
        .await
        .expect("delete second snapshot");
    assert_eq!(deleted.retained_snapshots, 0);
}

/// Serves the genuine object on the first read and forged bytes afterwards.
struct SwappingRepository {
    inner: BlobStoreBackupRepository,
    key: String,
    forged: Bytes,
    reads: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl BackupRepository for SwappingRepository {
    fn manifest_authenticator(&self) -> Option<&graphql_orm_backup::ManifestAuthenticator> {
        self.inner.manifest_authenticator()
    }
    async fn get_blob_stream(&self, key: &str) -> Result<StorageByteStream, BackupError> {
        if key == self.key && self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
            return Ok(StorageByteStream::from_bytes(self.forged.clone()));
        }
        self.inner.get_blob_stream(key).await
    }
    async fn get_blob(&self, key: &str) -> Result<Bytes, BackupError> {
        self.inner.get_blob(key).await
    }
    async fn put_blob(&self, key: &str, body: Bytes) -> Result<(), BackupError> {
        self.inner.put_blob(key, body).await
    }
    // Object restore holds the repository lock.
    async fn put_blob_if_absent(&self, key: &str, body: Bytes) -> Result<bool, BackupError> {
        self.inner.put_blob_if_absent(key, body).await
    }
    async fn delete_blob_once(&self, key: &str) -> Result<(), BackupError> {
        self.inner.delete_blob_once(key).await
    }
    async fn blob_exists(&self, key: &str) -> Result<bool, BackupError> {
        self.inner.blob_exists(key).await
    }
    async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, BackupError> {
        self.inner.list_blobs(prefix).await
    }
    async fn delete_blob(&self, key: &str) -> Result<(), BackupError> {
        self.inner.delete_blob(key).await
    }
}

fn request(snapshot_id: Uuid, created_at: i64) -> FullBackupRequest {
    FullBackupRequest {
        snapshot_id,
        created_at,
        app_id: "smb-integration-test".to_string(),
        app_version: "1.0.0".to_string(),
    }
}

#[derive(Default)]
struct TestDatabase {
    restored: Mutex<Vec<BackupTableExport>>,
}

#[async_trait]
impl GraphqlOrmBackupAdapter for TestDatabase {
    async fn schema_snapshot(&self) -> Result<GraphqlOrmBackupSchema, BackupError> {
        Ok(GraphqlOrmBackupSchema {
            backend: "sqlite".to_string(),
            migration_version: "20260713000000".to_string(),
            schema_hash: "smb-test-schema".to_string(),
        })
    }

    async fn export_full(&self) -> Result<Vec<BackupTableExport>, BackupError> {
        let mut fields = Map::new();
        fields.insert("name".to_string(), Value::String("SMB test".to_string()));
        Ok(vec![BackupTableExport {
            table_name: "items".to_string(),
            rows: vec![BackupRow {
                table_name: "items".to_string(),
                primary_key: "1".to_string(),
                row_hash: bytes_sha256_hex(b"1"),
                values: fields,
            }],
        }])
    }

    async fn export_incremental(
        &self,
        _parent_snapshot_id: Uuid,
    ) -> Result<Vec<BackupChangeExport>, BackupError> {
        Err(BackupError::UnsupportedOperation {
            operation: "incremental test export".to_string(),
        })
    }

    async fn restore_target_is_empty(&self) -> Result<bool, BackupError> {
        Ok(true)
    }

    async fn restore_full(
        &self,
        _backup_schema: GraphqlOrmBackupSchema,
        export: Vec<BackupTableExport>,
        _context: RestoreContext,
    ) -> Result<(), BackupError> {
        self.restored.lock().expect("restored lock").extend(export);
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

struct TestObjects {
    object: BackupObjectRef,
    bytes: Bytes,
}

#[async_trait]
impl BackupObjectIndex for TestObjects {
    async fn objects_from_full_export(
        &self,
        _: &[graphql_orm_backup::BackupTableExport],
    ) -> Result<Vec<BackupObjectRef>, BackupError> {
        self.list_objects_for_full_backup().await
    }

    async fn list_objects_for_full_backup(&self) -> Result<Vec<BackupObjectRef>, BackupError> {
        Ok(vec![self.object.clone()])
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

    async fn load_object_stream(
        &self,
        _object: &BackupObjectRef,
    ) -> Result<StorageByteStream, BackupError> {
        let chunks = self
            .bytes
            .chunks(128 * 1024)
            .map(Bytes::copy_from_slice)
            .map(Ok)
            .collect::<Vec<_>>();
        Ok(StorageByteStream::new(Box::pin(futures::stream::iter(
            chunks,
        ))))
    }
}

fn test_manifest_authenticator() -> graphql_orm_backup::ManifestAuthenticator {
    graphql_orm_backup::ManifestAuthenticator::new(
        graphql_orm_backup::ManifestKey::from_bytes(&[7; 32]).expect("test manifest key"),
    )
    .with_repository_identity(uuid::Uuid::from_u128(1))
}
