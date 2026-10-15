use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use graphql_orm_backup::{
    BackupChangeAction, BackupChangeExport, BackupCompression, BackupError, BackupKind,
    BackupObjectIndex, BackupObjectRef, BackupRepository, BackupRow, BackupTableExport,
    CompactChainRequest, FullBackupRequest, GraphqlOrmBackupAdapter, GraphqlOrmBackupSchema,
    IncrementalBackupRequest, LocalBackupRepository, RestoreContext, RestoreFailurePhase,
    bytes_sha256_hex, compact_chain, create_full_backup, create_incremental_backup,
    database_changes_key, load_manifest, restore_snapshot, write_manifest,
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
    fail_incremental_after_one: bool,
}

impl MockDatabase {
    fn with_full(full_tables: Vec<BackupTableExport>) -> Self {
        Self {
            full_tables,
            incremental_changes: Vec::new(),
            incremental_export_calls: Arc::default(),
            restored_full: Arc::default(),
            restored_incremental: Arc::default(),
            fail_incremental_after_one: false,
        }
    }

    fn with_incremental(incremental_changes: Vec<BackupChangeExport>) -> Self {
        Self {
            full_tables: Vec::new(),
            incremental_changes,
            incremental_export_calls: Arc::default(),
            restored_full: Arc::default(),
            restored_incremental: Arc::default(),
            fail_incremental_after_one: false,
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
        Ok(self.restored_full().is_empty() && self.restored_incremental().is_empty())
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
        _backup_schema: GraphqlOrmBackupSchema,
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
        if self.fail_incremental_after_one {
            self.restored_incremental
                .lock()
                .expect("restored incremental lock")
                .extend(changes.into_iter().take(1));
            return Err(BackupError::Database {
                message: "injected-private-adapter-detail".to_string(),
            });
        }
        self.restored_incremental
            .lock()
            .expect("restored incremental lock")
            .extend(changes);
        Ok(())
    }
}

async fn create_restore_fault_fixture(repository: &LocalBackupRepository) {
    let source = MockDatabase::with_full(vec![BackupTableExport {
        table_name: "users".to_string(),
        rows: vec![backup_row("users", "1", "Ada")],
    }]);
    create_full_backup(repository, &source, &MockObjectIndex, full_request())
        .await
        .expect("full fixture");
    let incremental_source = MockDatabase::with_incremental(sample_changes());
    create_incremental_backup(
        repository,
        &incremental_source,
        &MockObjectIndex,
        incremental_request(full_id()),
    )
    .await
    .expect("incremental fixture");
}

#[tokio::test]
async fn later_preparation_failure_reports_prior_stages_without_applying_failed_stage() {
    let temp = TempDir::new().expect("owned repository");
    let repository = LocalBackupRepository::new(temp.path());
    create_restore_fault_fixture(&repository).await;
    let mut manifest = load_manifest(&repository, incremental_id())
        .await
        .expect("manifest");
    manifest.database.compression = BackupCompression::None;
    let malformed = bytes::Bytes::from_static(b"{invalid-json}\n");
    let entry = &mut manifest.database.changes[0];
    entry.sha256_hex = bytes_sha256_hex(&malformed);
    repository
        .put_blob(&entry.content_key, malformed)
        .await
        .expect("malformed owned payload");
    write_manifest(&repository, &mut manifest)
        .await
        .expect("valid checksum for malformed payload");

    let target = MockDatabase::empty_restore_target();
    let error = restore_snapshot(
        &repository,
        &target,
        incremental_id(),
        RestoreContext::empty_database(),
    )
    .await
    .expect_err("later JSON preparation fails");
    let BackupError::RestoreStageFailed { progress, source } = error else {
        panic!("typed restore progress");
    };
    assert_eq!(progress.requested_snapshot_id, incremental_id());
    assert_eq!(progress.failed_manifest_id, Some(incremental_id()));
    assert_eq!(progress.phase, RestoreFailurePhase::Preparation);
    assert_eq!(progress.completed_manifest_ids, vec![full_id()]);
    assert!(progress.target_may_have_changed());
    assert!(matches!(*source, BackupError::Serialization(_)));
    assert_eq!(target.restored_full().len(), 1);
    assert!(target.restored_incremental().is_empty());

    let dry_target = MockDatabase::empty_restore_target();
    let error = restore_snapshot(
        &repository,
        &dry_target,
        incremental_id(),
        RestoreContext::dry_run(),
    )
    .await
    .expect_err("dry run also parses the failing stage");
    let BackupError::RestoreStageFailed { progress, .. } = error else {
        panic!("typed dry-run progress");
    };
    assert_eq!(progress.phase, RestoreFailurePhase::Preparation);
    assert!(progress.completed_manifest_ids.is_empty());
    assert!(!progress.target_may_have_changed());
    assert!(dry_target.restored_full().is_empty());
    assert!(dry_target.restored_incremental().is_empty());
}

#[tokio::test]
async fn partially_applied_stage_reports_uncertainty_and_requires_fresh_target_recovery() {
    let temp = TempDir::new().expect("owned repository");
    let repository = LocalBackupRepository::new(temp.path());
    create_restore_fault_fixture(&repository).await;
    let mut target = MockDatabase::empty_restore_target();
    target.fail_incremental_after_one = true;
    let error = restore_snapshot(
        &repository,
        &target,
        incremental_id(),
        RestoreContext::empty_database(),
    )
    .await
    .expect_err("adapter fails after one applied change");
    assert!(
        !error
            .to_string()
            .contains("injected-private-adapter-detail")
    );
    let BackupError::RestoreStageFailed { progress, source } = error else {
        panic!("typed restore progress");
    };
    assert_eq!(progress.phase, RestoreFailurePhase::Apply);
    assert_eq!(progress.failed_manifest_id, Some(incremental_id()));
    assert_eq!(progress.completed_manifest_ids, vec![full_id()]);
    assert!(progress.target_may_have_changed());
    assert!(
        matches!(*source, BackupError::Database { message } if message == "injected-private-adapter-detail")
    );
    assert_eq!(target.restored_full().len(), 1);
    assert_eq!(target.restored_incremental().len(), 1);

    let retry_error = restore_snapshot(
        &repository,
        &target,
        incremental_id(),
        RestoreContext::empty_database(),
    )
    .await
    .expect_err("cannot resume into the partially populated target");
    let BackupError::RestoreStageFailed { progress, source } = retry_error else {
        panic!("typed validation progress");
    };
    assert_eq!(progress.phase, RestoreFailurePhase::Validation);
    assert!(
        !progress.target_may_have_changed(),
        "this retry performed no writes"
    );
    assert!(matches!(*source, BackupError::RestoreTargetNotEmpty));
    assert_eq!(target.restored_incremental().len(), 1);

    let recovered = MockDatabase::empty_restore_target();
    restore_snapshot(
        &repository,
        &recovered,
        incremental_id(),
        RestoreContext::empty_database(),
    )
    .await
    .expect("restart in a fresh disposable target");
    assert_eq!(recovered.restored_full().len(), 1);
    assert_eq!(recovered.restored_incremental(), sample_changes());
}

#[tokio::test]
async fn database_stage_capacity_fails_before_apply_and_finite_override_restores() {
    use graphql_orm_backup::{BackupByteLimit, BackupCapacityLimits};
    let temp = TempDir::new().expect("owned repository");
    let repository = LocalBackupRepository::new(temp.path());
    create_restore_fault_fixture(&repository).await;
    let limited = repository.clone().with_capacity_limits(BackupCapacityLimits {
        stage_bytes: BackupByteLimit::Bytes(1), ..BackupCapacityLimits::default()
    });
    let target = MockDatabase::empty_restore_target();
    let error = restore_snapshot(&limited, &target, incremental_id(), RestoreContext::empty_database())
        .await.expect_err("decoded stage exceeds budget");
    let BackupError::RestoreStageFailed { progress, source } = error else {
        panic!("typed progress for capacity failure");
    };
    assert_eq!(progress.phase, RestoreFailurePhase::Preparation);
    assert!(!progress.target_may_have_changed());
    assert!(matches!(*source, BackupError::CapacityExceeded { limit_bytes: 1 }));
    assert!(target.restored_full().is_empty());
    let reviewed = repository.with_capacity_limits(BackupCapacityLimits {
        stage_bytes: BackupByteLimit::Bytes(1024 * 1024), ..BackupCapacityLimits::default()
    });
    restore_snapshot(&reviewed, &target, incremental_id(), RestoreContext::empty_database())
        .await.expect("reviewed finite stage budget");
    assert_eq!(target.restored_full().len(), 1);
    assert_eq!(target.restored_incremental(), sample_changes());
}
