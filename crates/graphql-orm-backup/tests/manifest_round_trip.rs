use async_trait::async_trait;
use bytes::Bytes;
use graphql_orm_backup::{
    BACKUP_FORMAT_VERSION, BackupChangeExport, BackupError, BackupKind, BackupObjectIndex,
    BackupObjectRef, BackupSnapshotManifest, BackupTableExport, BackupTombstone,
    DatabaseBackupManifest, FullBackupPlan, GraphqlOrmBackupAdapter, GraphqlOrmBackupSchema,
    ObjectBackupEntry, RestoreContext, TableBackupEntry, bytes_sha256_hex,
    ensure_empty_restore_target, manifest_checksum, object_content_key, plan_full_backup,
    set_manifest_checksum, verify_manifest_checksum,
};
use uuid::Uuid;

#[test]
fn manifest_serializes_round_trips_and_verifies_checksum() {
    let mut manifest = sample_manifest();
    set_manifest_checksum(&mut manifest).expect("set checksum");

    let encoded = serde_json::to_string(&manifest).expect("serialize manifest");
    let decoded: BackupSnapshotManifest =
        serde_json::from_str(&encoded).expect("deserialize manifest");

    assert_eq!(decoded, manifest);
    verify_manifest_checksum(&decoded).expect("checksum verifies");
}

#[test]
fn manifest_checksum_is_stable_and_excludes_checksum_field() {
    let mut manifest = sample_manifest();
    let first = manifest_checksum(&manifest).expect("checksum");
    manifest.checksum = "ignored-by-canonical-checksum".to_string();
    let second = manifest_checksum(&manifest).expect("checksum");

    assert_eq!(first, second);
}

#[test]
fn object_content_key_uses_sha256_shards() {
    let hash = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
    assert_eq!(
        object_content_key(hash),
        "objects/sha256/ab/cd/abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
    );
}

#[test]
fn empty_database_restore_refuses_non_empty_target() {
    let context = RestoreContext::empty_database();
    let err = ensure_empty_restore_target(false, &context)
        .expect_err("non-empty target should be rejected");

    assert!(matches!(err, BackupError::RestoreTargetNotEmpty));
}

#[tokio::test]
async fn full_backup_planner_includes_database_and_objects() {
    let database = MockDatabase;
    let objects = MockObjectIndex;

    let plan = plan_full_backup(&database, &objects)
        .await
        .expect("plan full backup");

    assert_eq!(
        plan,
        FullBackupPlan {
            schema: GraphqlOrmBackupSchema {
                backend: "sqlite".to_string(),
                migration_version: "20260514000000".to_string(),
                schema_hash: "schema-hash".to_string(),
            },
            tables: vec![BackupTableExport {
                table_name: "storage".to_string(),
                rows: Vec::new(),
            }],
            objects: vec![BackupObjectRef {
                object_id: object_id(),
                storage_key: "originals/aa/bb/object.txt".to_string(),
                sha256_hex: bytes_sha256_hex(b"object"),
                size_bytes: 6,
                mime_type: Some("text/plain".to_string()),
            }],
        }
    );
}

fn sample_manifest() -> BackupSnapshotManifest {
    let table_bytes = b"{\"id\":\"1\"}\n";
    let object_bytes = b"object";
    let table_hash = bytes_sha256_hex(table_bytes);
    let object_hash = bytes_sha256_hex(object_bytes);

    BackupSnapshotManifest {
        format_version: BACKUP_FORMAT_VERSION,
        snapshot_id: snapshot_id(),
        parent_snapshot_id: None,
        created_at: 1_775_174_400,
        app_id: "example-app".to_string(),
        app_version: "0.1.0".to_string(),
        graphql_orm_schema_version: "20260514000000".to_string(),
        graphql_orm_schema_hash: "schema-hash".to_string(),
        database_backend: "sqlite".to_string(),
        backup_kind: BackupKind::Full,
        database: DatabaseBackupManifest {
            export_format: "jsonl.zst".to_string(),
            row_count: 1,
            table_count: 1,
            tables: vec![TableBackupEntry {
                table_name: "storage".to_string(),
                row_count: 1,
                content_key: "snapshots/snapshot/database/tables/storage.jsonl.zst".to_string(),
                sha256_hex: table_hash,
            }],
        },
        objects: vec![ObjectBackupEntry {
            object_id: object_id(),
            storage_key: "originals/aa/bb/object.txt".to_string(),
            content_key: object_content_key(&object_hash),
            sha256_hex: object_hash,
            size_bytes: 6,
            mime_type: Some("text/plain".to_string()),
        }],
        tombstones: vec![BackupTombstone {
            table_name: Some("storage".to_string()),
            primary_key: Some("deleted-row".to_string()),
            object_id: None,
            deleted_at: 1_775_174_401,
        }],
        checksum: String::new(),
    }
}

fn snapshot_id() -> Uuid {
    Uuid::parse_str("bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb").expect("valid uuid")
}

fn object_id() -> Uuid {
    Uuid::parse_str("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa").expect("valid uuid")
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
            table_name: "storage".to_string(),
            rows: Vec::new(),
        }])
    }

    async fn export_incremental(
        &self,
        _parent_snapshot_id: Uuid,
    ) -> Result<Vec<BackupChangeExport>, BackupError> {
        Err(BackupError::UnsupportedOperation {
            operation: "mock incremental export".to_string(),
        })
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
        Err(BackupError::UnsupportedOperation {
            operation: "mock incremental restore".to_string(),
        })
    }
}

struct MockObjectIndex;

#[async_trait]
impl BackupObjectIndex for MockObjectIndex {
    async fn list_objects_for_full_backup(&self) -> Result<Vec<BackupObjectRef>, BackupError> {
        Ok(vec![BackupObjectRef {
            object_id: object_id(),
            storage_key: "originals/aa/bb/object.txt".to_string(),
            sha256_hex: bytes_sha256_hex(b"object"),
            size_bytes: 6,
            mime_type: Some("text/plain".to_string()),
        }])
    }

    async fn list_objects_for_incremental_backup(
        &self,
        _since_snapshot_id: Uuid,
    ) -> Result<Vec<BackupObjectRef>, BackupError> {
        Err(BackupError::UnsupportedOperation {
            operation: "mock incremental object list".to_string(),
        })
    }

    async fn load_object(&self, _object: &BackupObjectRef) -> Result<Bytes, BackupError> {
        Ok(Bytes::from_static(b"object"))
    }
}
