use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use graphql_orm_backup::{
    BackupChangeExport, BackupError, BackupObjectIndex, BackupObjectRef, BackupRepository,
    BackupRow, BackupTableExport, FullBackupRequest, GraphqlOrmBackupAdapter,
    GraphqlOrmBackupSchema, LocalBackupRepository, RestoreContext, RestoreObjectSink,
    bytes_sha256_hex, create_full_backup, object_content_key, restore_objects, restore_snapshot,
};
use serde_json::{Map, Value};
use tempfile::TempDir;
use uuid::Uuid;

#[tokio::test]
async fn restore_snapshot_round_trips_full_backup_rows() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path());
    let rows = vec![
        backup_row("users", "1", &[("name", "Ada")]),
        backup_row("users", "2", &[("name", "Grace")]),
    ];
    let source = MockDatabase::with_tables(vec![BackupTableExport {
        table_name: "users".to_string(),
        rows: rows.clone(),
    }]);
    let objects = MockObjectIndex::default();

    create_full_backup(&repository, &source, &objects, backup_request())
        .await
        .expect("create full backup");

    let target = MockDatabase::empty_restore_target();
    let result = restore_snapshot(
        &repository,
        &target,
        snapshot_id(),
        RestoreContext::empty_database(),
    )
    .await
    .expect("restore snapshot");

    assert_eq!(result.manifest_chain_len, 1);
    assert_eq!(result.full_table_count, 1);
    assert_eq!(result.full_row_count, 2);
    assert_eq!(
        target.restored_full(),
        vec![BackupTableExport {
            table_name: "users".to_string(),
            rows,
        }]
    );
}

#[tokio::test]
async fn restore_snapshot_dry_run_validates_and_parses_without_applying() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path());
    let source = MockDatabase::with_tables(vec![BackupTableExport {
        table_name: "users".to_string(),
        rows: vec![backup_row("users", "1", &[("name", "Ada")])],
    }]);
    let objects = MockObjectIndex::default();

    create_full_backup(&repository, &source, &objects, backup_request())
        .await
        .expect("create full backup");

    let target = MockDatabase::non_empty_restore_target();
    let result = restore_snapshot(
        &repository,
        &target,
        snapshot_id(),
        RestoreContext::dry_run(),
    )
    .await
    .expect("dry-run restore");

    assert_eq!(result.full_row_count, 1);
    assert!(target.restored_full().is_empty());
}

#[tokio::test]
async fn restore_snapshot_refuses_non_empty_empty_database_target() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path());
    let source = MockDatabase::with_tables(vec![BackupTableExport {
        table_name: "users".to_string(),
        rows: Vec::new(),
    }]);
    let objects = MockObjectIndex::default();

    create_full_backup(&repository, &source, &objects, backup_request())
        .await
        .expect("create full backup");

    let target = MockDatabase::non_empty_restore_target();
    let err = restore_snapshot(
        &repository,
        &target,
        snapshot_id(),
        RestoreContext::empty_database(),
    )
    .await
    .expect_err("non-empty target rejected");

    assert!(matches!(err, BackupError::RestoreTargetNotEmpty));
    assert!(target.restored_full().is_empty());
}

#[tokio::test]
async fn restore_objects_loads_verified_object_bytes_into_sink() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path());
    let object_bytes = Bytes::from_static(b"object");
    let object_hash = bytes_sha256_hex(&object_bytes);
    let source = MockDatabase::with_tables(vec![BackupTableExport {
        table_name: "users".to_string(),
        rows: Vec::new(),
    }]);
    let objects = MockObjectIndex {
        objects: vec![BackupObjectRef {
            object_id: object_id(),
            storage_key: "objects/original.txt".to_string(),
            sha256_hex: object_hash.clone(),
            size_bytes: object_bytes.len() as u64,
            mime_type: Some("text/plain".to_string()),
        }],
        bytes: vec![object_bytes.clone()],
    };

    let result = create_full_backup(&repository, &source, &objects, backup_request())
        .await
        .expect("create full backup");
    assert!(
        repository
            .blob_exists(&object_content_key(&object_hash))
            .await
            .expect("exists")
    );

    let sink = RecordingObjectSink::default();
    restore_objects(&repository, &result.manifest, &sink)
        .await
        .expect("restore objects");

    assert_eq!(
        sink.restored(),
        vec![(objects.objects[0].clone(), object_bytes)]
    );
}

fn backup_request() -> FullBackupRequest {
    FullBackupRequest {
        snapshot_id: snapshot_id(),
        created_at: 1_775_174_400,
        app_id: "example-app".to_string(),
        app_version: "0.1.0".to_string(),
    }
}

fn backup_row(table_name: &str, primary_key: &str, values: &[(&str, &str)]) -> BackupRow {
    let mut row_values = Map::new();
    for (key, value) in values {
        row_values.insert((*key).to_string(), Value::String((*value).to_string()));
    }

    BackupRow {
        table_name: table_name.to_string(),
        primary_key: primary_key.to_string(),
        row_hash: bytes_sha256_hex(primary_key.as_bytes()),
        values: row_values,
    }
}

fn snapshot_id() -> Uuid {
    Uuid::parse_str("bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb").expect("valid uuid")
}

fn object_id() -> Uuid {
    Uuid::parse_str("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa").expect("valid uuid")
}

#[derive(Default)]
struct MockObjectIndex {
    objects: Vec<BackupObjectRef>,
    bytes: Vec<Bytes>,
}

#[async_trait]
impl BackupObjectIndex for MockObjectIndex {
    async fn list_objects_for_full_backup(&self) -> Result<Vec<BackupObjectRef>, BackupError> {
        Ok(self.objects.clone())
    }

    async fn list_objects_for_incremental_backup(
        &self,
        _since_snapshot_id: Uuid,
    ) -> Result<Vec<BackupObjectRef>, BackupError> {
        Ok(Vec::new())
    }

    async fn load_object(&self, object: &BackupObjectRef) -> Result<Bytes, BackupError> {
        let index = self
            .objects
            .iter()
            .position(|candidate| candidate.object_id == object.object_id)
            .expect("object exists");
        Ok(self.bytes[index].clone())
    }
}

#[derive(Clone)]
struct MockDatabase {
    tables: Vec<BackupTableExport>,
    target_is_empty: bool,
    restored_full: Arc<Mutex<Vec<BackupTableExport>>>,
    restored_incremental: Arc<Mutex<Vec<BackupChangeExport>>>,
}

impl MockDatabase {
    fn with_tables(tables: Vec<BackupTableExport>) -> Self {
        Self {
            tables,
            target_is_empty: true,
            restored_full: Arc::default(),
            restored_incremental: Arc::default(),
        }
    }

    fn empty_restore_target() -> Self {
        Self::with_tables(Vec::new())
    }

    fn non_empty_restore_target() -> Self {
        Self {
            target_is_empty: false,
            ..Self::with_tables(Vec::new())
        }
    }

    fn restored_full(&self) -> Vec<BackupTableExport> {
        self.restored_full
            .lock()
            .expect("restored full lock")
            .clone()
    }
}

#[async_trait]
impl GraphqlOrmBackupAdapter for MockDatabase {
    async fn schema_snapshot(&self) -> Result<GraphqlOrmBackupSchema, BackupError> {
        Ok(GraphqlOrmBackupSchema {
            backend: "sqlite".to_string(),
            migration_version: "20260514000000".to_string(),
            schema_hash: "schema-hash".to_string(),
        })
    }

    async fn restore_target_is_empty(&self) -> Result<bool, BackupError> {
        Ok(self.target_is_empty)
    }

    async fn export_full(&self) -> Result<Vec<BackupTableExport>, BackupError> {
        Ok(self.tables.clone())
    }

    async fn export_incremental(
        &self,
        _parent_snapshot_id: Uuid,
    ) -> Result<Vec<BackupChangeExport>, BackupError> {
        Ok(Vec::new())
    }

    async fn restore_full(
        &self,
        export: Vec<BackupTableExport>,
        _context: RestoreContext,
    ) -> Result<(), BackupError> {
        self.restored_full
            .lock()
            .expect("restored full lock")
            .extend(export);
        Ok(())
    }

    async fn restore_incremental(
        &self,
        changes: Vec<BackupChangeExport>,
        _context: RestoreContext,
    ) -> Result<(), BackupError> {
        self.restored_incremental
            .lock()
            .expect("restored incremental lock")
            .extend(changes);
        Ok(())
    }
}

#[derive(Default)]
struct RecordingObjectSink {
    restored: Arc<Mutex<Vec<(BackupObjectRef, Bytes)>>>,
}

impl RecordingObjectSink {
    fn restored(&self) -> Vec<(BackupObjectRef, Bytes)> {
        self.restored.lock().expect("restored lock").clone()
    }
}

#[async_trait]
impl RestoreObjectSink for RecordingObjectSink {
    async fn restore_object(
        &self,
        object: BackupObjectRef,
        bytes: Bytes,
    ) -> Result<(), BackupError> {
        self.restored
            .lock()
            .expect("restored lock")
            .push((object, bytes));
        Ok(())
    }
}
