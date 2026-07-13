use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use bytes::Bytes;
use futures::{StreamExt, stream};
use graphql_orm_backup::{
    BackupChangeExport, BackupCompression, BackupError, BackupObjectIndex, BackupObjectRef,
    BackupRepository, BackupRow, BackupTableExport, DATABASE_EXPORT_FORMAT, FullBackupRequest,
    GraphqlOrmBackupAdapter, GraphqlOrmBackupSchema, RestoreContext, bytes_sha256_hex,
    create_full_backup, database_table_key, decompress_payload, object_content_key,
    snapshot_manifest_key, verify_manifest_and_objects, verify_manifest_checksum,
};
use graphql_orm_storage::StorageByteStream;
use serde_json::{Map, Value};
use uuid::Uuid;

#[tokio::test]
async fn create_full_backup_writes_tables_objects_and_manifest_last() {
    let repository = RecordingRepository::default();
    let object_bytes = Bytes::from_static(b"object");
    let object_hash = bytes_sha256_hex(&object_bytes);
    let database = MockDatabase::new(vec![BackupTableExport {
        table_name: "users".to_string(),
        rows: vec![backup_row("users", "1", &[("name", "Ada")])],
    }]);
    let objects = MockObjectIndex::new(vec![object_ref(&object_hash)], vec![object_bytes]);
    let request = backup_request();

    let result = create_full_backup(&repository, &database, &objects, request)
        .await
        .expect("create full backup");

    verify_manifest_checksum(&result.manifest).expect("manifest checksum verifies");
    verify_manifest_and_objects(&repository, &result.manifest)
        .await
        .expect("payload checksums verify");

    let table_key = database_table_key(snapshot_id(), "users");
    assert!(repository.blob_exists(&table_key).await.expect("exists"));
    assert!(
        repository
            .blob_exists(&object_content_key(&object_hash))
            .await
            .expect("exists")
    );
    assert!(
        repository
            .blob_exists(&snapshot_manifest_key(snapshot_id()))
            .await
            .expect("exists")
    );

    let table_bytes = repository.get_blob(&table_key).await.expect("table blob");
    let table_bytes = decompress_payload(&table_bytes).expect("decompress table blob");
    assert!(table_bytes.ends_with(b"\n"));
    let first_line = table_bytes
        .split(|byte| *byte == b'\n')
        .next()
        .expect("first jsonl row");
    let row: Value = serde_json::from_slice(first_line).expect("json row");
    assert_eq!(row["table_name"], "users");
    assert_eq!(row["primary_key"], "1");

    assert_eq!(
        repository.write_order().last(),
        Some(&snapshot_manifest_key(snapshot_id()))
    );
}

#[tokio::test]
async fn create_full_backup_streams_large_objects_in_bounded_chunks() {
    const CHUNK_SIZE: usize = 128 * 1024;
    const CHUNK_COUNT: usize = 128;
    let chunk = vec![0x5a; CHUNK_SIZE];
    let mut hasher = sha2::Sha256::new();
    use sha2::Digest;
    for _ in 0..CHUNK_COUNT {
        hasher.update(&chunk);
    }
    let hash = format!("{:x}", hasher.finalize());
    let repository = StreamingProbeRepository::default();
    let database = MockDatabase::new(vec![BackupTableExport {
        table_name: "empty".to_string(),
        rows: Vec::new(),
    }]);
    let objects = StreamingObjectIndex {
        object: BackupObjectRef {
            object_id: object_id(),
            storage_key: "objects/large.bin".to_string(),
            sha256_hex: hash,
            size_bytes: u64::try_from(CHUNK_SIZE * CHUNK_COUNT).expect("bounded size"),
            mime_type: Some("application/octet-stream".to_string()),
        },
        chunk_size: CHUNK_SIZE,
        chunk_count: CHUNK_COUNT,
    };

    create_full_backup(&repository, &database, &objects, backup_request())
        .await
        .expect("streaming full backup");

    assert_eq!(repository.max_chunk.load(Ordering::SeqCst), CHUNK_SIZE);
    assert_eq!(repository.chunk_count.load(Ordering::SeqCst), CHUNK_COUNT);
}

#[tokio::test]
async fn create_full_backup_deduplicates_existing_object_blob() {
    let repository = RecordingRepository::default();
    let object_bytes = Bytes::from_static(b"object");
    let object_hash = bytes_sha256_hex(&object_bytes);
    let object_key = object_content_key(&object_hash);
    repository
        .put_blob(&object_key, object_bytes.clone())
        .await
        .expect("prewrite object");
    repository.clear_write_order();

    let database = MockDatabase::new(vec![BackupTableExport {
        table_name: "users".to_string(),
        rows: Vec::new(),
    }]);
    let objects = MockObjectIndex::new(vec![object_ref(&object_hash)], vec![object_bytes]);

    let result = create_full_backup(&repository, &database, &objects, backup_request())
        .await
        .expect("create full backup");

    assert_eq!(result.manifest.objects[0].content_key, object_key);
    assert!(!repository.write_order().contains(&object_key));
    assert_eq!(
        repository.get_count(&object_key),
        0,
        "existing content-addressed object blobs must not be re-read"
    );
}

#[tokio::test]
async fn create_full_backup_rejects_object_checksum_mismatch() {
    let repository = RecordingRepository::default();
    let expected_hash = bytes_sha256_hex(b"expected");
    let database = MockDatabase::new(vec![BackupTableExport {
        table_name: "users".to_string(),
        rows: Vec::new(),
    }]);
    let objects = MockObjectIndex::new(
        vec![object_ref(&expected_hash)],
        vec![Bytes::from_static(b"different")],
    );

    let err = create_full_backup(&repository, &database, &objects, backup_request())
        .await
        .expect_err("checksum mismatch");

    assert!(matches!(err, BackupError::ChecksumMismatch { .. }));
    assert!(
        !repository
            .blob_exists(&snapshot_manifest_key(snapshot_id()))
            .await
            .expect("exists")
    );
}

#[tokio::test]
async fn create_full_backup_sets_database_counts() {
    let repository = RecordingRepository::default();
    let database = MockDatabase::new(vec![
        BackupTableExport {
            table_name: "users".to_string(),
            rows: vec![backup_row("users", "1", &[("name", "Ada")])],
        },
        BackupTableExport {
            table_name: "posts".to_string(),
            rows: vec![
                backup_row("posts", "10", &[("title", "First")]),
                backup_row("posts", "11", &[("title", "Second")]),
            ],
        },
    ]);
    let objects = MockObjectIndex::new(Vec::new(), Vec::new());

    let result = create_full_backup(&repository, &database, &objects, backup_request())
        .await
        .expect("create full backup");

    assert_eq!(
        result.manifest.database.export_format,
        DATABASE_EXPORT_FORMAT
    );
    assert_eq!(
        result.manifest.database.compression,
        BackupCompression::Zstd
    );
    assert_eq!(result.manifest.database.table_count, 2);
    assert_eq!(result.manifest.database.row_count, 3);
    assert_eq!(result.manifest.database.tables[0].table_name, "users");
    assert_eq!(result.manifest.database.tables[0].row_count, 1);
    assert_eq!(result.manifest.database.tables[1].table_name, "posts");
    assert_eq!(result.manifest.database.tables[1].row_count, 2);
}

#[tokio::test]
async fn create_full_backup_writes_manifest_after_payloads() {
    let repository = RecordingRepository::default();
    let database = MockDatabase::new(vec![BackupTableExport {
        table_name: "users".to_string(),
        rows: Vec::new(),
    }]);
    let objects = MockObjectIndex::new(Vec::new(), Vec::new());

    create_full_backup(&repository, &database, &objects, backup_request())
        .await
        .expect("create full backup");

    let writes = repository.write_order();
    assert_eq!(writes.len(), 3);
    assert_eq!(writes[0], "locks/repository.lock");
    assert_eq!(writes[1], database_table_key(snapshot_id(), "users"));
    assert_eq!(writes[2], snapshot_manifest_key(snapshot_id()));
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

fn object_ref(hash: &str) -> BackupObjectRef {
    BackupObjectRef {
        object_id: object_id(),
        storage_key: "objects/original.txt".to_string(),
        sha256_hex: hash.to_string(),
        size_bytes: 6,
        mime_type: Some("text/plain".to_string()),
    }
}

fn snapshot_id() -> Uuid {
    Uuid::parse_str("bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb").expect("valid uuid")
}

fn object_id() -> Uuid {
    Uuid::parse_str("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa").expect("valid uuid")
}

#[derive(Clone, Default)]
struct RecordingRepository {
    blobs: Arc<Mutex<HashMap<String, Bytes>>>,
    writes: Arc<Mutex<Vec<String>>>,
    gets: Arc<Mutex<HashMap<String, u64>>>,
}

impl RecordingRepository {
    fn write_order(&self) -> Vec<String> {
        self.writes.lock().expect("writes lock").clone()
    }

    fn clear_write_order(&self) {
        self.writes.lock().expect("writes lock").clear();
    }

    fn get_count(&self, key: &str) -> u64 {
        self.gets
            .lock()
            .expect("gets lock")
            .get(key)
            .copied()
            .unwrap_or_default()
    }
}

#[async_trait]
impl BackupRepository for RecordingRepository {
    async fn put_blob(&self, key: &str, body: Bytes) -> Result<(), BackupError> {
        self.blobs
            .lock()
            .expect("blobs lock")
            .insert(key.to_string(), body);
        self.writes
            .lock()
            .expect("writes lock")
            .push(key.to_string());
        Ok(())
    }

    async fn get_blob(&self, key: &str) -> Result<Bytes, BackupError> {
        *self
            .gets
            .lock()
            .expect("gets lock")
            .entry(key.to_string())
            .or_default() += 1;
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

#[derive(Default)]
struct StreamingProbeRepository {
    inner: RecordingRepository,
    max_chunk: AtomicUsize,
    chunk_count: AtomicUsize,
}

#[async_trait]
impl BackupRepository for StreamingProbeRepository {
    async fn put_blob(&self, key: &str, body: Bytes) -> Result<(), BackupError> {
        self.inner.put_blob(key, body).await
    }

    async fn put_blob_stream_if_absent(
        &self,
        _key: &str,
        body: StorageByteStream,
    ) -> Result<bool, BackupError> {
        let mut stream = body.into_inner();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            self.max_chunk.fetch_max(chunk.len(), Ordering::SeqCst);
            self.chunk_count.fetch_add(1, Ordering::SeqCst);
        }
        Ok(true)
    }

    async fn get_blob(&self, key: &str) -> Result<Bytes, BackupError> {
        self.inner.get_blob(key).await
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

struct MockDatabase {
    tables: Vec<BackupTableExport>,
}

impl MockDatabase {
    fn new(tables: Vec<BackupTableExport>) -> Self {
        Self { tables }
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

    async fn export_full(&self) -> Result<Vec<BackupTableExport>, BackupError> {
        Ok(self.tables.clone())
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
        Err(BackupError::UnsupportedOperation {
            operation: "mock full restore".to_string(),
        })
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

struct MockObjectIndex {
    objects: Vec<BackupObjectRef>,
    bytes: Vec<Bytes>,
}

impl MockObjectIndex {
    fn new(objects: Vec<BackupObjectRef>, bytes: Vec<Bytes>) -> Self {
        Self { objects, bytes }
    }
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
        Err(BackupError::UnsupportedOperation {
            operation: "mock incremental object list".to_string(),
        })
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

struct StreamingObjectIndex {
    object: BackupObjectRef,
    chunk_size: usize,
    chunk_count: usize,
}

#[async_trait]
impl BackupObjectIndex for StreamingObjectIndex {
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
        Err(BackupError::UnsupportedOperation {
            operation: "buffered load must not be used".to_string(),
        })
    }

    async fn load_object_stream(
        &self,
        _object: &BackupObjectRef,
    ) -> Result<StorageByteStream, BackupError> {
        let chunk_size = self.chunk_size;
        let chunks = (0..self.chunk_count).map(move |_| Ok(Bytes::from(vec![0x5a; chunk_size])));
        Ok(StorageByteStream::new(Box::pin(stream::iter(chunks))))
    }
}
