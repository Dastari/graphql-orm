#![cfg(feature = "orm-sqlite")]

use async_trait::async_trait;
use bytes::Bytes;
use graphql_orm::graphql::orm::Entity;
use graphql_orm::prelude::GraphQLSchemaEntity;
use graphql_orm_backup::*;
use graphql_orm_storage::{BlobPutOptions, BlobStore, LocalStorageBackend, StorageByteStream};
use std::sync::Arc;
use uuid::Uuid;

#[derive(GraphQLSchemaEntity, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "snapshot_objects", plural = "SnapshotObjects")]
struct SnapshotObject {
    #[primary_key]
    id: Uuid,
    storage_key: String,
    digest: String,
    size: i64,
}

struct ExportedRows(Vec<BackupTableExport>);
#[async_trait]
impl GraphqlOrmBackupAdapter for ExportedRows {
    async fn schema_snapshot(&self) -> Result<GraphqlOrmBackupSchema, BackupError> {
        Ok(GraphqlOrmBackupSchema {
            backend: "sqlite".into(),
            migration_version: "1".into(),
            schema_hash: "test-schema".into(),
        })
    }
    async fn export_full(&self) -> Result<Vec<BackupTableExport>, BackupError> {
        Ok(self.0.clone())
    }
    async fn export_incremental(&self, _: Uuid) -> Result<Vec<BackupChangeExport>, BackupError> {
        unreachable!()
    }
    async fn restore_full(
        &self,
        _: GraphqlOrmBackupSchema,
        _: Vec<BackupTableExport>,
        _: RestoreContext,
    ) -> Result<(), BackupError> {
        unreachable!()
    }
    async fn restore_incremental(
        &self,
        _: Vec<BackupChangeExport>,
        _: RestoreContext,
    ) -> Result<(), BackupError> {
        unreachable!()
    }
}

#[tokio::test]
async fn object_set_comes_from_exported_rows_even_after_live_metadata_is_deleted() {
    let pool = graphql_orm::sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .unwrap();
    // Live metadata has already lost the object; the snapshot below still has it.
    graphql_orm::sqlx::query(
        "CREATE TABLE snapshot_objects (id TEXT, storage_key TEXT, digest TEXT, size INTEGER)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let source = tempfile::TempDir::new().unwrap();
    let store: Arc<dyn BlobStore> = Arc::new(LocalStorageBackend::new(source.path()));
    let body = Bytes::from_static(b"snapshot bytes");
    store
        .put_blob(
            "media/object",
            StorageByteStream::from_bytes(body.clone()),
            BlobPutOptions::default(),
        )
        .await
        .unwrap();
    let object_id = Uuid::new_v4();
    let table = BackupTableExport { table_name: "snapshot_objects".into(), rows: vec![BackupRow {
        table_name: "snapshot_objects".into(), primary_key: object_id.to_string(), row_hash: String::new(),
        values: serde_json::json!({"id": {"Uuid": object_id}, "storage_key": {"String": "media/object"}, "digest": {"String": bytes_sha256_hex(&body).to_ascii_uppercase()}, "size": {"Integer": body.len()}}).as_object().unwrap().clone(),
    }] };
    let database = ExportedRows(vec![table]);
    let index = OrmBackupObjectIndex::new(
        Arc::new(graphql_orm::db::Database::new(pool)),
        vec![SnapshotObject::metadata()],
        OrmObjectIndexColumns {
            table_name: "snapshot_objects".into(),
            object_id_column: "id".into(),
            storage_key_column: "storage_key".into(),
            sha256_hex_column: "digest".into(),
            size_bytes_column: "size".into(),
            mime_type_column: None,
        },
        store,
    );
    let dir = tempfile::TempDir::new().unwrap();
    let repository = LocalBackupRepository::new(dir.path()).with_manifest_authenticator(
        ManifestAuthenticator::new(ManifestKey::from_bytes(&[42; 32]).unwrap())
            .with_repository_identity(uuid::Uuid::from_u128(1)),
    );
    let result = create_full_backup(
        &repository,
        &database,
        &index,
        FullBackupRequest {
            snapshot_id: Uuid::new_v4(),
            created_at: 1,
            app_id: "snapshot-test".into(),
            app_version: "1".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        result.manifest.objects.len(),
        1,
        "a reference from exported metadata must not disappear"
    );
    assert_eq!(result.manifest.objects[0].object_id, object_id);
    verify_manifest_and_objects(&repository, &result.manifest)
        .await
        .unwrap();
    let target = tempfile::TempDir::new().unwrap();
    let target_store: Arc<dyn BlobStore> = Arc::new(LocalStorageBackend::new(target.path()));
    let sink = BlobStoreRestoreObjectSink::new(
        target_store.clone(),
        RestoreKeyScope::prefixes(["media"]).unwrap(),
    );
    restore_objects(&repository, &result.manifest, &sink)
        .await
        .unwrap();
    let restored = collect_backup_stream(
        target_store.get_blob("media/object").await.unwrap().body,
        BackupByteLimit::Unbounded,
    )
    .await
    .unwrap();
    assert_eq!(restored, body);
}
