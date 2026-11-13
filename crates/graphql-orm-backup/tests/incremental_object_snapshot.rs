#![cfg(feature = "local")]

use async_trait::async_trait;
use bytes::Bytes;
use graphql_orm_backup::*;
use std::sync::Mutex;
use uuid::Uuid;

struct Database(Vec<BackupChangeExport>, Mutex<Vec<BackupChangeExport>>);
#[async_trait]
impl GraphqlOrmBackupAdapter for Database {
    async fn schema_snapshot(&self) -> Result<GraphqlOrmBackupSchema, BackupError> {
        Ok(GraphqlOrmBackupSchema {
            backend: "sqlite".into(),
            migration_version: "1".into(),
            schema_hash: "snapshot".into(),
        })
    }
    async fn export_full(&self) -> Result<Vec<BackupTableExport>, BackupError> {
        Ok(vec![])
    }
    async fn export_incremental(&self, _: Uuid) -> Result<Vec<BackupChangeExport>, BackupError> {
        Ok(self.0.clone())
    }
    async fn restore_target_is_empty(&self) -> Result<bool, BackupError> {
        Ok(true)
    }
    async fn restore_full(
        &self,
        _: GraphqlOrmBackupSchema,
        _: Vec<BackupTableExport>,
        _: RestoreContext,
    ) -> Result<(), BackupError> {
        Ok(())
    }
    async fn restore_incremental(
        &self,
        changes: Vec<BackupChangeExport>,
        _: RestoreContext,
    ) -> Result<(), BackupError> {
        self.1.lock().unwrap().extend(changes);
        Ok(())
    }
}
struct Objects;
#[async_trait]
impl BackupObjectIndex for Objects {
    async fn objects_from_full_export(
        &self,
        _: &[BackupTableExport],
    ) -> Result<Vec<BackupObjectRef>, BackupError> {
        Ok(vec![])
    }
    async fn list_objects_for_full_backup(&self) -> Result<Vec<BackupObjectRef>, BackupError> {
        Ok(vec![])
    }
    async fn list_objects_for_incremental_backup(
        &self,
        _: Uuid,
    ) -> Result<Vec<BackupObjectRef>, BackupError> {
        // A separate live listing has already lost this object.
        Ok(vec![])
    }
    async fn objects_from_incremental_export(
        &self,
        _: Uuid,
        changes: &[BackupChangeExport],
    ) -> Result<Vec<BackupObjectRef>, BackupError> {
        assert_eq!(changes.len(), 1);
        let row = changes[0].row.as_ref().unwrap();
        Ok(vec![BackupObjectRef {
            object_id: row.primary_key.parse().unwrap(),
            storage_key: row.values["key"].as_str().unwrap().into(),
            sha256_hex: row.values["digest"].as_str().unwrap().into(),
            size_bytes: row.values["size"].as_u64().unwrap(),
            mime_type: None,
        }])
    }
    async fn load_object(&self, _: &BackupObjectRef) -> Result<Bytes, BackupError> {
        Ok(Bytes::from_static(b"snapshot object"))
    }
}

#[tokio::test]
async fn incremental_objects_come_from_the_export_and_restore_with_its_rows() {
    let id = Uuid::new_v4();
    let body = b"snapshot object";
    let changes = vec![BackupChangeExport { table_name: "objects".into(), primary_key: id.to_string(),
        action: BackupChangeAction::Create, changed_at: 2, row: Some(BackupRow {
            table_name: "objects".into(), primary_key: id.to_string(), row_hash: String::new(),
            values: serde_json::json!({"key": "media/new", "digest": bytes_sha256_hex(body), "size": body.len()}).as_object().unwrap().clone(),
        }) }];
    let database = Database(changes.clone(), Mutex::new(vec![]));
    let dir = tempfile::TempDir::new().unwrap();
    let repo = LocalBackupRepository::new(dir.path()).with_manifest_authenticator(
        ManifestAuthenticator::new(ManifestKey::from_bytes(&[42; 32]).unwrap())
            .with_repository_identity(Uuid::new_v4()),
    );
    let parent = create_full_backup(
        &repo,
        &database,
        &Objects,
        FullBackupRequest {
            snapshot_id: Uuid::new_v4(),
            created_at: 1,
            app_id: "snapshot-test".into(),
            app_version: "1".into(),
        },
    )
    .await
    .unwrap();
    let child = create_incremental_backup(
        &repo,
        &database,
        &Objects,
        IncrementalBackupRequest {
            snapshot_id: Uuid::new_v4(),
            parent_snapshot_id: parent.manifest.snapshot_id,
            created_at: 2,
            app_id: "snapshot-test".into(),
            app_version: "1".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        child.manifest.objects.len(),
        1,
        "the exported reference must not disappear with live metadata"
    );
    assert_eq!(child.manifest.objects[0].object_id, id);
    verify_manifest_and_objects(&repo, &child.manifest)
        .await
        .unwrap();
    restore_snapshot(
        &repo,
        &database,
        child.manifest.snapshot_id,
        RestoreContext::empty_database(),
    )
    .await
    .unwrap();
    assert_eq!(*database.1.lock().unwrap(), changes);
}
