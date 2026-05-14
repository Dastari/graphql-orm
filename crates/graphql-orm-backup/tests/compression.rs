use bytes::Bytes;
use graphql_orm_backup::{
    BACKUP_FORMAT_VERSION, BackupCompression, BackupKind, BackupRepository, BackupSnapshotManifest,
    DatabaseBackupManifest, TableBackupEntry, bytes_sha256_hex, compress_payload,
    decompress_payload, set_manifest_checksum, verify_object_checksums,
};
use uuid::Uuid;

mod support {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use async_trait::async_trait;
    use bytes::Bytes;
    use graphql_orm_backup::{BackupError, BackupRepository};

    #[derive(Clone, Default)]
    pub struct RecordingRepository {
        blobs: Arc<Mutex<HashMap<String, Bytes>>>,
    }

    #[async_trait]
    impl BackupRepository for RecordingRepository {
        async fn put_blob(&self, key: &str, body: Bytes) -> Result<(), BackupError> {
            self.blobs
                .lock()
                .expect("blobs lock")
                .insert(key.to_string(), body);
            Ok(())
        }

        async fn get_blob(&self, key: &str) -> Result<Bytes, BackupError> {
            self.blobs
                .lock()
                .expect("blobs lock")
                .get(key)
                .cloned()
                .ok_or_else(|| BackupError::MissingBlob {
                    key: key.to_string(),
                })
        }

        async fn blob_exists(&self, key: &str) -> Result<bool, BackupError> {
            Ok(self.blobs.lock().expect("blobs lock").contains_key(key))
        }

        async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, BackupError> {
            let mut keys = self
                .blobs
                .lock()
                .expect("blobs lock")
                .keys()
                .filter(|key| key.starts_with(prefix))
                .cloned()
                .collect::<Vec<_>>();
            keys.sort();
            Ok(keys)
        }

        async fn delete_blob(&self, key: &str) -> Result<(), BackupError> {
            self.blobs.lock().expect("blobs lock").remove(key);
            Ok(())
        }
    }
}

#[test]
fn compressed_payload_round_trips() {
    let payload = b"{\"table_name\":\"users\"}\n{\"table_name\":\"posts\"}\n";
    let compressed = compress_payload(payload).expect("compress payload");
    let decompressed = decompress_payload(&compressed).expect("decompress payload");

    assert_eq!(decompressed, payload);
}

#[test]
fn compressed_table_export_decompresses_to_json_lines() {
    let payload = b"{\"table_name\":\"users\",\"primary_key\":\"1\"}\n";
    let compressed = compress_payload(payload).expect("compress payload");
    let decompressed = decompress_payload(&compressed).expect("decompress payload");

    assert!(decompressed.ends_with(b"\n"));
    let first_line = decompressed
        .split(|byte| *byte == b'\n')
        .next()
        .expect("first line");
    let row: serde_json::Value = serde_json::from_slice(first_line).expect("json row");
    assert_eq!(row["table_name"], "users");
    assert_eq!(row["primary_key"], "1");
}

#[test]
fn manifest_checksum_changes_when_compressed_content_hash_changes() {
    let mut first = manifest_with_table_hash(bytes_sha256_hex(b"compressed-one"));
    let mut second = manifest_with_table_hash(bytes_sha256_hex(b"compressed-two"));
    set_manifest_checksum(&mut first).expect("first checksum");
    set_manifest_checksum(&mut second).expect("second checksum");

    assert_ne!(first.checksum, second.checksum);
}

#[tokio::test]
async fn table_checksum_validates_compressed_bytes() {
    let repository = support::RecordingRepository::default();
    let compressed = Bytes::from(compress_payload(b"{\"id\":\"1\"}\n").expect("compress payload"));
    let hash = bytes_sha256_hex(&compressed);
    let manifest = manifest_with_table_hash(hash);

    repository
        .put_blob(&manifest.database.tables[0].content_key, compressed)
        .await
        .expect("put table");

    verify_object_checksums(&repository, &manifest)
        .await
        .expect("compressed table checksum verifies");
}

#[test]
fn corrupted_compressed_payload_returns_error() {
    let err = decompress_payload(b"not a valid zstd frame").expect_err("corrupt payload rejected");

    assert!(matches!(
        err,
        graphql_orm_backup::BackupError::Compression { .. }
    ));
}

fn manifest_with_table_hash(table_hash: String) -> BackupSnapshotManifest {
    BackupSnapshotManifest {
        format_version: BACKUP_FORMAT_VERSION,
        snapshot_id: Uuid::parse_str("bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb").expect("valid uuid"),
        parent_snapshot_id: None,
        created_at: 1_775_174_400,
        app_id: "example-app".to_string(),
        app_version: "0.1.0".to_string(),
        graphql_orm_schema_version: "20260514000000".to_string(),
        graphql_orm_schema_hash: "schema-hash".to_string(),
        database_backend: "sqlite".to_string(),
        backup_kind: BackupKind::Full,
        database: DatabaseBackupManifest {
            export_format: "jsonl".to_string(),
            compression: BackupCompression::Zstd,
            row_count: 1,
            table_count: 1,
            tables: vec![TableBackupEntry {
                table_name: "users".to_string(),
                row_count: 1,
                content_key: "snapshots/snapshot/database/tables/users.jsonl.zst".to_string(),
                sha256_hex: table_hash,
            }],
        },
        objects: Vec::new(),
        tombstones: Vec::new(),
        checksum: String::new(),
    }
}
