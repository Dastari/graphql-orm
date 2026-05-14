use bytes::Bytes;
use graphql_orm_backup::{
    BACKUP_FORMAT_VERSION, BackupCompression, BackupError, BackupKind, BackupRepository,
    BackupSnapshotManifest, DatabaseBackupManifest, LocalBackupRepository, ObjectBackupEntry,
    TableBackupEntry, bytes_sha256_hex, object_content_key, set_manifest_checksum,
    verify_object_checksums,
};
use tempfile::TempDir;
use uuid::Uuid;

#[tokio::test]
async fn local_repository_put_get_list_delete_round_trip() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path());

    repository
        .put_blob("snapshots/a/manifest.json", Bytes::from_static(b"manifest"))
        .await
        .expect("put blob");
    repository
        .put_blob("objects/sha256/aa/bb/aabb", Bytes::from_static(b"object"))
        .await
        .expect("put blob");

    assert!(
        repository
            .blob_exists("snapshots/a/manifest.json")
            .await
            .expect("exists check")
    );
    assert_eq!(
        repository
            .get_blob("snapshots/a/manifest.json")
            .await
            .expect("get blob"),
        Bytes::from_static(b"manifest")
    );

    let listed = repository
        .list_blobs("snapshots")
        .await
        .expect("list blobs");
    assert_eq!(listed, vec!["snapshots/a/manifest.json"]);

    repository
        .delete_blob("snapshots/a/manifest.json")
        .await
        .expect("delete blob");
    assert!(
        !repository
            .blob_exists("snapshots/a/manifest.json")
            .await
            .expect("exists check")
    );
}

#[tokio::test]
async fn local_repository_rejects_path_traversal_keys() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path());

    let err = repository
        .put_blob("../escape", Bytes::from_static(b"bad"))
        .await
        .expect_err("path traversal rejected");

    assert!(matches!(err, BackupError::InvalidRepositoryKey { .. }));
}

#[tokio::test]
async fn local_repository_rejects_absolute_keys() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path());

    let err = repository
        .put_blob("/tmp/escape", Bytes::from_static(b"bad"))
        .await
        .expect_err("absolute key rejected");

    assert!(matches!(err, BackupError::InvalidRepositoryKey { .. }));
}

#[tokio::test]
async fn local_repository_rejects_dot_components() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path());

    let err = repository
        .put_blob("snapshots/./manifest.json", Bytes::from_static(b"bad"))
        .await
        .expect_err("dot component rejected");

    assert!(matches!(err, BackupError::InvalidRepositoryKey { .. }));
}

#[tokio::test]
async fn local_repository_rejects_parent_components() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path());

    let err = repository
        .put_blob("snapshots/../manifest.json", Bytes::from_static(b"bad"))
        .await
        .expect_err("parent component rejected");

    assert!(matches!(err, BackupError::InvalidRepositoryKey { .. }));
}

#[tokio::test]
async fn local_repository_rejects_empty_segments() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path());

    let err = repository
        .put_blob("snapshots//manifest.json", Bytes::from_static(b"bad"))
        .await
        .expect_err("empty segment rejected");

    assert!(matches!(err, BackupError::InvalidRepositoryKey { .. }));
}

#[tokio::test]
async fn local_repository_rejects_backslashes() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path());

    let err = repository
        .put_blob("snapshots\\manifest.json", Bytes::from_static(b"bad"))
        .await
        .expect_err("backslash rejected");

    assert!(matches!(err, BackupError::InvalidRepositoryKey { .. }));
}

#[tokio::test]
async fn local_repository_rejects_nul_bytes() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path());

    let err = repository
        .put_blob("snapshots/manifest\0.json", Bytes::from_static(b"bad"))
        .await
        .expect_err("nul rejected");

    assert!(matches!(err, BackupError::InvalidRepositoryKey { .. }));
}

#[tokio::test]
async fn local_repository_empty_prefix_lists_all_blobs() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path());

    repository
        .put_blob("snapshots/a/manifest.json", Bytes::from_static(b"manifest"))
        .await
        .expect("put manifest");
    repository
        .put_blob("objects/sha256/aa/bb/aabb", Bytes::from_static(b"object"))
        .await
        .expect("put object");

    let listed = repository.list_blobs("").await.expect("list all blobs");
    assert_eq!(
        listed,
        vec![
            "objects/sha256/aa/bb/aabb".to_string(),
            "snapshots/a/manifest.json".to_string()
        ]
    );
}

#[tokio::test]
async fn local_repository_open_existing_accepts_existing_directory() {
    let temp = TempDir::new().expect("temp dir");

    LocalBackupRepository::open_existing(temp.path())
        .await
        .expect("open existing directory");
}

#[tokio::test]
async fn local_repository_open_existing_rejects_missing_path() {
    let temp = TempDir::new().expect("temp dir");
    let missing = temp.path().join("missing");

    let err = LocalBackupRepository::open_existing(missing)
        .await
        .expect_err("missing path rejected");

    assert!(matches!(err, BackupError::Io { .. }));
}

#[tokio::test]
async fn local_repository_open_existing_rejects_file_path() {
    let temp = TempDir::new().expect("temp dir");
    let file = temp.path().join("file");
    tokio::fs::write(&file, b"not a directory")
        .await
        .expect("write file");

    let err = LocalBackupRepository::open_existing(file)
        .await
        .expect_err("file path rejected");

    assert!(matches!(err, BackupError::InvalidRepositoryRoot { .. }));
}

#[tokio::test]
async fn verification_fails_when_object_blob_is_missing() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path());
    let manifest = sample_manifest_with_object_hash(bytes_sha256_hex(b"object"));

    let err = verify_object_checksums(&repository, &manifest)
        .await
        .expect_err("missing object should fail verification");

    assert!(matches!(err, BackupError::MissingBlob { .. }));
}

#[tokio::test]
async fn verification_fails_when_object_checksum_mismatches() {
    let temp = TempDir::new().expect("temp dir");
    let repository = LocalBackupRepository::new(temp.path());
    let manifest = sample_manifest_with_object_hash(bytes_sha256_hex(b"expected"));

    repository
        .put_blob(
            &manifest.objects[0].content_key,
            Bytes::from_static(b"different"),
        )
        .await
        .expect("put object");

    let err = verify_object_checksums(&repository, &manifest)
        .await
        .expect_err("checksum mismatch should fail verification");

    assert!(matches!(err, BackupError::ChecksumMismatch { .. }));
}

fn sample_manifest_with_object_hash(object_hash: String) -> BackupSnapshotManifest {
    let object_blob_key = object_content_key(&object_hash);
    let table_bytes = b"";
    let table_hash = bytes_sha256_hex(table_bytes);
    let mut manifest = BackupSnapshotManifest {
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
            row_count: 0,
            table_count: 1,
            tables: vec![TableBackupEntry {
                table_name: "storage".to_string(),
                row_count: 0,
                content_key: object_content_key(&table_hash),
                sha256_hex: table_hash,
            }],
        },
        objects: vec![ObjectBackupEntry {
            object_id: Uuid::parse_str("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa").expect("valid uuid"),
            storage_key: "originals/aa/bb/object.txt".to_string(),
            content_key: object_blob_key,
            sha256_hex: object_hash,
            size_bytes: 6,
            mime_type: Some("text/plain".to_string()),
        }],
        tombstones: Vec::new(),
        checksum: String::new(),
    };
    set_manifest_checksum(&mut manifest).expect("set checksum");
    manifest
}
