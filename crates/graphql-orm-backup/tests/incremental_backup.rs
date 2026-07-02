use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use graphql_orm_backup::{
    BackupChangeAction, BackupChangeExport, BackupError, BackupKind, BackupObjectIndex,
    BackupObjectRef, BackupRepository, BackupRow, BackupTableExport, CompactChainRequest,
    FullBackupRequest, GraphqlOrmBackupAdapter, GraphqlOrmBackupSchema, IncrementalBackupRequest,
    LocalBackupRepository, RestoreContext, bytes_sha256_hex, compact_chain, create_full_backup,
    create_incremental_backup, database_changes_key, load_manifest, restore_snapshot,
};
use serde_json::{Map, Value};
use tempfile::TempDir;
use uuid::Uuid;

#[tokio::test]
async fn create_incremental_backup_writes_change_files_and_tombstones() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path());
    let changes = sample_changes();
    let database = MockDatabase::with_incremental(changes.clone());
    let objects = MockObjectIndex;

    let result = create_incremental_backup(
        &repository,
        &database,
        &objects,
        incremental_request(parent_id()),
    )
    .await
    .expect("create incremental backup");

    assert_eq!(result.manifest.backup_kind, BackupKind::Incremental);
    assert_eq!(result.manifest.parent_snapshot_id, Some(parent_id()));
    assert_eq!(result.manifest.database.changes.len(), 1);
    assert_eq!(result.manifest.database.row_count, changes.len() as u64);
    assert_eq!(result.manifest.tombstones.len(), 1);
    assert_eq!(
        result.manifest.tombstones[0].primary_key.as_deref(),
        Some("3")
    );
    assert!(
        repository
            .blob_exists(&database_changes_key(incremental_id(), "users"))
            .await
            .expect("change file exists")
    );
    assert_eq!(database.incremental_export_calls(), 1);
}

#[tokio::test]
async fn restore_snapshot_applies_incremental_chain_after_full_snapshot() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path());
    let objects = MockObjectIndex;
    let source = MockDatabase::with_full(vec![BackupTableExport {
        table_name: "users".to_string(),
        rows: vec![backup_row("users", "1", "Ada")],
    }]);
    create_full_backup(&repository, &source, &objects, full_request())
        .await
        .expect("create full backup");

    let incremental_source = MockDatabase::with_incremental(sample_changes());
    create_incremental_backup(
        &repository,
        &incremental_source,
        &objects,
        incremental_request(full_id()),
    )
    .await
    .expect("create incremental backup");

    let target = MockDatabase::empty_restore_target();
    let result = restore_snapshot(
        &repository,
        &target,
        incremental_id(),
        RestoreContext::empty_database(),
    )
    .await
    .expect("restore chain");

    assert_eq!(result.manifest_chain_len, 2);
    assert_eq!(result.incremental_change_count, 3);
    assert_eq!(
        target.restored_full()[0].rows[0],
        backup_row("users", "1", "Ada")
    );
    assert_eq!(target.restored_incremental(), sample_changes());
}

#[tokio::test]
async fn compact_chain_writes_synthetic_full_snapshot() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path());
    let objects = MockObjectIndex;
    let source = MockDatabase::with_full(vec![BackupTableExport {
        table_name: "users".to_string(),
        rows: vec![
            backup_row("users", "1", "Ada"),
            backup_row("users", "3", "Delete Me"),
        ],
    }]);
    create_full_backup(&repository, &source, &objects, full_request())
        .await
        .expect("create full backup");

    let incremental_source = MockDatabase::with_incremental(sample_changes());
    create_incremental_backup(
        &repository,
        &incremental_source,
        &objects,
        incremental_request(full_id()),
    )
    .await
    .expect("create incremental backup");

    let result = compact_chain(
        &repository,
        CompactChainRequest {
            snapshot_id: compacted_id(),
            source_snapshot_id: incremental_id(),
            created_at: 1_775_174_402,
            app_id: "example-app".to_string(),
            app_version: "0.1.0".to_string(),
        },
    )
    .await
    .expect("compact chain");

    assert_eq!(result.manifest.backup_kind, BackupKind::SyntheticFull);
    assert_eq!(result.manifest.parent_snapshot_id, None);
    assert_eq!(result.manifest.database.row_count, 2);

    let loaded = load_manifest(&repository, compacted_id())
        .await
        .expect("load compacted manifest");
    assert_eq!(loaded.backup_kind, BackupKind::SyntheticFull);

    let target = MockDatabase::empty_restore_target();
    restore_snapshot(
        &repository,
        &target,
        compacted_id(),
        RestoreContext::empty_database(),
    )
    .await
    .expect("restore compacted snapshot");

    let rows = &target.restored_full()[0].rows;
    assert_eq!(rows.len(), 2);
    assert!(rows.contains(&backup_row("users", "1", "Grace")));
    assert!(rows.contains(&backup_row("users", "2", "New")));
}

fn full_request() -> FullBackupRequest {
    FullBackupRequest {
        snapshot_id: full_id(),
        created_at: 1_775_174_400,
        app_id: "example-app".to_string(),
        app_version: "0.1.0".to_string(),
    }
}

fn incremental_request(parent_snapshot_id: Uuid) -> IncrementalBackupRequest {
    IncrementalBackupRequest {
        snapshot_id: incremental_id(),
        parent_snapshot_id,
        created_at: 1_775_174_401,
        app_id: "example-app".to_string(),
        app_version: "0.1.0".to_string(),
    }
}

fn sample_changes() -> Vec<BackupChangeExport> {
    vec![
        BackupChangeExport {
            table_name: "users".to_string(),
            primary_key: "1".to_string(),
            action: BackupChangeAction::Update,
            row: Some(backup_row("users", "1", "Grace")),
            changed_at: 1_775_174_401,
        },
        BackupChangeExport {
            table_name: "users".to_string(),
            primary_key: "2".to_string(),
            action: BackupChangeAction::Create,
            row: Some(backup_row("users", "2", "New")),
            changed_at: 1_775_174_401,
        },
        BackupChangeExport {
            table_name: "users".to_string(),
            primary_key: "3".to_string(),
            action: BackupChangeAction::Delete,
            row: None,
            changed_at: 1_775_174_401,
        },
    ]
}

fn backup_row(table_name: &str, primary_key: &str, name: &str) -> BackupRow {
    let mut values = Map::new();
    values.insert("name".to_string(), Value::String(name.to_string()));

    BackupRow {
        table_name: table_name.to_string(),
        primary_key: primary_key.to_string(),
        row_hash: bytes_sha256_hex(name.as_bytes()),
        values,
    }
}

fn full_id() -> Uuid {
    Uuid::parse_str("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa").expect("valid uuid")
}

fn parent_id() -> Uuid {
    Uuid::parse_str("99999999-9999-4999-9999-999999999999").expect("valid uuid")
}

fn incremental_id() -> Uuid {
    Uuid::parse_str("bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb").expect("valid uuid")
}

fn compacted_id() -> Uuid {
    Uuid::parse_str("cccccccc-cccc-4ccc-cccc-cccccccccccc").expect("valid uuid")
}

struct MockObjectIndex;

#[async_trait]
impl BackupObjectIndex for MockObjectIndex {
    async fn list_objects_for_full_backup(&self) -> Result<Vec<BackupObjectRef>, BackupError> {
        Ok(Vec::new())
    }

    async fn list_objects_for_incremental_backup(
        &self,
        _since_snapshot_id: Uuid,
    ) -> Result<Vec<BackupObjectRef>, BackupError> {
        Ok(Vec::new())
    }

    async fn load_object(&self, _object: &BackupObjectRef) -> Result<bytes::Bytes, BackupError> {
        Err(BackupError::MissingBlob {
            key: "mock object".to_string(),
        })
    }
}

#[derive(Clone)]
struct MockDatabase {
    full_tables: Vec<BackupTableExport>,
    incremental_changes: Vec<BackupChangeExport>,
    incremental_export_calls: Arc<Mutex<u64>>,
    restored_full: Arc<Mutex<Vec<BackupTableExport>>>,
    restored_incremental: Arc<Mutex<Vec<BackupChangeExport>>>,
}

impl MockDatabase {
    fn with_full(full_tables: Vec<BackupTableExport>) -> Self {
        Self {
            full_tables,
            incremental_changes: Vec::new(),
            incremental_export_calls: Arc::default(),
            restored_full: Arc::default(),
            restored_incremental: Arc::default(),
        }
    }

    fn with_incremental(incremental_changes: Vec<BackupChangeExport>) -> Self {
        Self {
            full_tables: Vec::new(),
            incremental_changes,
            incremental_export_calls: Arc::default(),
            restored_full: Arc::default(),
            restored_incremental: Arc::default(),
        }
    }

    fn empty_restore_target() -> Self {
        Self::with_full(Vec::new())
    }

    fn incremental_export_calls(&self) -> u64 {
        *self
            .incremental_export_calls
            .lock()
            .expect("incremental calls lock")
    }

    fn restored_full(&self) -> Vec<BackupTableExport> {
        self.restored_full
            .lock()
            .expect("restored full lock")
            .clone()
    }

    fn restored_incremental(&self) -> Vec<BackupChangeExport> {
        self.restored_incremental
            .lock()
            .expect("restored incremental lock")
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
        Ok(true)
    }

    async fn export_full(&self) -> Result<Vec<BackupTableExport>, BackupError> {
        Ok(self.full_tables.clone())
    }

    async fn export_incremental(
        &self,
        _parent_snapshot_id: Uuid,
    ) -> Result<Vec<BackupChangeExport>, BackupError> {
        *self
            .incremental_export_calls
            .lock()
            .expect("incremental calls lock") += 1;
        Ok(self.incremental_changes.clone())
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
