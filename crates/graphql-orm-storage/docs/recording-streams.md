# Recording And Large-Object Streams

`StreamingObjectStore` provides a bucket/key API for recording-style workloads
that need incremental writes, atomic completion, metadata, and HTTP range
playback.

The API is generic. It does not know about any RMM domain model, user model,
tenant model, or authorization system.

## API Shape

```rust
#[async_trait::async_trait]
pub trait StreamingObjectStore: Send + Sync {
    async fn put_object_stream(
        &self,
        bucket: &str,
        key: &str,
        content_type: Option<String>,
        metadata: ObjectMetadata,
        stream: StorageByteStream,
    ) -> Result<ObjectInfo, StorageError>;

    async fn create_multipart_object(
        &self,
        bucket: &str,
        key: &str,
        content_type: Option<String>,
        metadata: ObjectMetadata,
    ) -> Result<BoxedMultipartWriter, StorageError>;

    async fn get_object_range(
        &self,
        bucket: &str,
        key: &str,
        range: std::ops::Range<u64>,
    ) -> Result<ObjectRangeBody, StorageError>;

    async fn get_object_metadata(
        &self,
        bucket: &str,
        key: &str,
    ) -> Result<ObjectInfo, StorageError>;

    async fn list_objects(
        &self,
        bucket: &str,
        prefix: &str,
    ) -> Result<Vec<ObjectInfo>, StorageError>;

    async fn delete_object(&self, bucket: &str, key: &str) -> Result<(), StorageError>;
}
```

`MultipartWriter::complete` makes the object visible. Until completion succeeds,
local objects do not appear in `list_objects` and `get_object_metadata` returns
missing.

## Multipart Recording Write

```rust
use bytes::Bytes;
use graphql_orm_storage::{LocalStorageBackend, ObjectMetadata, StreamingObjectStore};

# async fn example() -> Result<(), graphql_orm_storage::StorageError> {
let backend = LocalStorageBackend::new("./data/storage");
let metadata = ObjectMetadata::from([
    ("tenant_id".to_string(), "tenant-1".to_string()),
    ("device_id".to_string(), "device-1".to_string()),
]);

let mut writer = backend
    .create_multipart_object(
        "recordings",
        "sessions/session-1/video.webm",
        Some("video/webm".to_string()),
        metadata,
    )
    .await?;

writer.write_chunk(Bytes::from_static(b"chunk-1")).await?;
writer.write_chunk(Bytes::from_static(b"chunk-2")).await?;

let object = writer.complete().await?;
assert_eq!(object.size_bytes, 14);
# Ok(())
# }
```

Call `abort` when the host application rejects or cancels a recording:

```rust
# use graphql_orm_storage::{LocalStorageBackend, ObjectMetadata, StreamingObjectStore};
# async fn example() -> Result<(), graphql_orm_storage::StorageError> {
# let backend = LocalStorageBackend::new("./data/storage");
let writer = backend
    .create_multipart_object("recordings", "sessions/cancelled/video.webm", None, ObjectMetadata::new())
    .await?;
writer.abort().await?;
# Ok(())
# }
```

## HTTP Range Playback

`get_object_range` accepts an exclusive Rust range and returns range metadata.
For an HTTP `Content-Range` header, subtract one from the exclusive end:

```rust
# use graphql_orm_storage::{LocalStorageBackend, StreamingObjectStore};
# async fn example() -> Result<(), graphql_orm_storage::StorageError> {
# let backend = LocalStorageBackend::new("./data/storage");
let body = backend
    .get_object_range("recordings", "sessions/session-1/video.webm", 0..1024)
    .await?;

let header = format!(
    "bytes {}-{}/{}",
    body.range.start,
    body.range.end.saturating_sub(1),
    body.range.total_size
);
# let _ = header;
# Ok(())
# }
```

The returned `body.body` is a `StorageByteStream`; route handlers can stream it
directly to the HTTP response.

## graphql-orm And agql-auth Integration

Host applications should keep authorization and persistence outside this crate:

1. Use `agql-auth` or application policy code to authorize the recording
   session, playback request, or retention deletion.
2. Persist `ObjectInfo` fields in an application-owned `graphql-orm` entity.
3. Store authorization-related values such as tenant, device, user, session, or
   policy scope in application rows and, when useful, in `ObjectMetadata`.
4. On playback, load the metadata row first, authorize it, then call
   `get_object_range`.
5. On retention deletion, authorize and select rows in the application, then
   call `delete_object`.

The storage crate does not depend on `graphql-orm` or `agql-auth`; it exposes
the storage primitives those layers can call after they have made policy
decisions.
