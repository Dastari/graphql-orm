use bytes::Bytes;
use graphql_orm_storage::{
    StorageByteStream, StorageError, collect_storage_stream, validate_blob_key,
    validate_object_bucket, validate_portable_blob_key,
};

#[test]
fn validate_blob_key_accepts_safe_relative_keys() {
    assert!(validate_blob_key("snapshots/a/manifest.json").is_ok());
    assert!(validate_blob_key("objects/sha256/aa/bb/hash").is_ok());
    assert!(validate_blob_key("originals/aa/bb/object.jpg").is_ok());
}

#[test]
fn validate_blob_key_rejects_unsafe_keys() {
    for key in [
        "",
        "/absolute/path",
        "../escape",
        "a/../escape",
        "a/./b",
        "a//b",
        "a\\b",
        "a\0b",
    ] {
        let err = validate_blob_key(key).expect_err("unsafe key should be rejected");
        assert!(matches!(err, StorageError::InvalidStorageKey { .. }));
    }
}

#[test]
fn validate_blob_key_reserves_the_internal_namespace() {
    for key in [
        ".graphql-orm-storage",
        ".graphql-orm-storage/multipart/upload",
        ".GRAPHQL-ORM-STORAGE/objects",
        ".graphql-orm-storage./x",
        ".graphql-orm-storage /x",
    ] {
        let err = validate_blob_key(key).expect_err("reserved key should be rejected");
        assert!(
            matches!(err, StorageError::InvalidStorageKey { .. }),
            "{key}"
        );
        assert!(validate_object_bucket(key.split('/').next().unwrap()).is_err());
    }
    // Only the first component is reserved; similar names stay available.
    for key in [
        "data/.graphql-orm-storage",
        "data/.graphql-orm-storage/x",
        ".graphql-orm-storage-old/x",
        "graphql-orm-storage/x",
    ] {
        assert!(validate_blob_key(key).is_ok(), "{key}");
    }
}

#[test]
fn byte_stream_from_bytes_preserves_size_hint() {
    let stream = StorageByteStream::from_bytes(Bytes::from_static(b"hello storage"));

    assert_eq!(stream.size_hint(), Some(13));
}

#[tokio::test]
async fn collect_storage_stream_returns_original_bytes() {
    let bytes = collect_storage_stream(StorageByteStream::from_bytes(Bytes::from_static(
        b"hello storage",
    )))
    .await
    .expect("collect stream");

    assert_eq!(bytes, Bytes::from_static(b"hello storage"));
}

#[tokio::test]
async fn collect_does_not_preallocate_a_declared_size() {
    use graphql_orm_storage::{StorageByteStream, collect_storage_stream};
    // A provider may report a size the remote side declared.
    let body = futures_util::stream::iter([Ok(bytes::Bytes::from_static(b"small"))]);
    let stream = StorageByteStream::with_size_hint(Box::pin(body), u64::MAX);
    let collected = collect_storage_stream(stream).await.expect("collect");
    assert_eq!(collected.as_ref(), b"small");
}

/// Portable validation also refuses names that SMB servers and Windows file
/// systems cannot store as written, or alias to another name (#482).
#[test]
fn validate_portable_blob_key_refuses_windows_unsafe_names() {
    for key in [
        "snapshots/a/manifest.json",
        "objects/sha256/aa/bb/hash",
        "media/report.final.pdf",
        "media/.hidden",
        "media/CONSOLE.txt",
        "media/COM10",
        "media/LPT",
        "media/aux-file",
        "media/con_",
        "media/naïve name.txt",
    ] {
        assert!(validate_portable_blob_key(key).is_ok(), "{key}");
    }
    for key in [
        // Everything the provider-neutral rules refuse.
        "",
        "../escape",
        "a//b",
        ".graphql-orm-storage/x",
        // Stream separator and characters Windows refuses.
        "a:b",
        "dir/file:stream",
        "a<b",
        "a>b",
        "a\"b",
        "a|b",
        "a?b",
        "a*b",
        "a\u{1}b",
        "dir\u{1f}/b",
        // Trailing dots and spaces, which Windows strips.
        "report.",
        "dir./file",
        "report ",
        "dir /file",
        "...",
        // Reserved device names, in any case and with any extension.
        "CON",
        "con",
        "dir/Prn.txt",
        "AUX.tar.gz",
        "nul",
        "COM1",
        "com9.log",
        "LPT0",
        "lpt5.bin",
        "COM\u{b9}",
        "LPT\u{b2}.txt",
        "CONIN$",
        "conout$.x",
        "dir/CON /x",
    ] {
        let error = validate_portable_blob_key(key).expect_err(key);
        assert!(
            matches!(error, StorageError::InvalidStorageKey { .. }),
            "{key}: {error:?}"
        );
        // The public message never reveals the rejected key.
        assert_eq!(error.to_string(), "invalid storage key");
    }
}
