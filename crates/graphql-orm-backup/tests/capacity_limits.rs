use bytes::Bytes;
use graphql_orm_backup::{
    BackupByteLimit, BackupError, collect_backup_stream, compress_payload,
    decompress_payload_with_limit,
};
use graphql_orm_storage::StorageByteStream;

fn stream(chunks: &[&'static [u8]]) -> StorageByteStream {
    let values: Vec<_> = chunks.iter().map(|chunk| Ok(Bytes::from_static(chunk))).collect();
    StorageByteStream::new(Box::pin(futures::stream::iter(values)))
}

#[tokio::test]
async fn collector_checks_actual_chunks_and_aggregate_without_trusting_size_hint() {
    for body in [stream(&[b"oversized"]), stream(&[b"abc", b"def"])] {
        assert!(matches!(collect_backup_stream(body, BackupByteLimit::Bytes(5)).await,
            Err(BackupError::CapacityExceeded { limit_bytes: 5 })));
    }
    let lying = StorageByteStream::with_size_hint(
        Box::pin(futures::stream::once(async { Ok(Bytes::from_static(b"too large")) })), 1);
    assert!(matches!(collect_backup_stream(lying, BackupByteLimit::Bytes(2)).await,
        Err(BackupError::CapacityExceeded { .. })));
    assert_eq!(collect_backup_stream(stream(&[b"abc", b"de"]), BackupByteLimit::Bytes(5))
        .await.expect("exact boundary"), b"abcde"[..]);
    assert_eq!(collect_backup_stream(stream(&[b"abc", b"def"]), BackupByteLimit::Unbounded)
        .await.expect("explicit opt in"), b"abcdef"[..]);
}

#[test]
fn decompression_rejects_expansion_and_allows_reviewed_larger_limits() {
    let plain = vec![b'x'; 32 * 1024];
    let compressed = compress_payload(&plain).expect("compress owned fixture");
    assert!(compressed.len() < 1024);
    assert!(matches!(decompress_payload_with_limit(&compressed, BackupByteLimit::Bytes(1024)),
        Err(BackupError::CapacityExceeded { limit_bytes: 1024 })));
    assert_eq!(decompress_payload_with_limit(&compressed, BackupByteLimit::Bytes(plain.len() as u64))
        .expect("reviewed finite limit"), plain);
}

#[cfg(feature = "local")]
#[tokio::test]
async fn native_repository_streams_while_buffered_read_obeys_configured_limit() {
    use graphql_orm_backup::{BackupCapacityLimits, BackupRepository, LocalBackupRepository};
    let root = tempfile::TempDir::new().expect("owned repository");
    let repository = LocalBackupRepository::new(root.path()).with_capacity_limits(BackupCapacityLimits {
        buffered_bytes: BackupByteLimit::Bytes(3), ..BackupCapacityLimits::default()
    });
    repository.put_blob_stream("payload", stream(&[b"abc", b"def"]))
        .await.expect("native write bypasses compatibility buffering");
    assert!(matches!(repository.get_blob("payload").await,
        Err(BackupError::CapacityExceeded { limit_bytes: 3 })));
    assert_eq!(collect_backup_stream(repository.get_blob_stream("payload").await.expect("native read"),
        BackupByteLimit::Bytes(6)).await.expect("explicit streaming consumer limit"), b"abcdef"[..]);
}

#[tokio::test]
async fn buffered_sink_rejects_oversized_body_before_calling_legacy_sink() {
    use async_trait::async_trait;
    use graphql_orm_backup::{BackupObjectRef, RestoreObjectSink};
    struct NeverWrite;
    #[async_trait]
    impl RestoreObjectSink for NeverWrite {
        fn buffered_byte_limit(&self) -> BackupByteLimit { BackupByteLimit::Bytes(2) }
        async fn restore_object(&self, _: BackupObjectRef, _: Bytes) -> Result<(), BackupError> {
            panic!("over-limit fallback must not invoke buffered sink")
        }
    }
    let object = BackupObjectRef {
        object_id: uuid::Uuid::nil(), storage_key: "owned-object".to_string(),
        sha256_hex: String::new(), size_bytes: 4, mime_type: None,
    };
    assert!(matches!(NeverWrite.restore_object_stream(object, stream(&[b"abcd"])).await,
        Err(BackupError::CapacityExceeded { limit_bytes: 2 })));
}
