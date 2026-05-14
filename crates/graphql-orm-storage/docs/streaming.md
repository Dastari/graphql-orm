# Streaming API

The crate supports both buffered and streaming object APIs.

Use buffered APIs for small files and simple application flows:

```rust
StorageService::put_object(request).await?;
StorageService::get_object(&stored).await?;
```

Use streaming APIs when the caller already has a stream or when large objects
should not be represented as a caller-owned `Vec<u8>`.

## Store A Streaming Object

```rust
use std::sync::Arc;

use graphql_orm_storage::{
    LocalStorageBackend, StorageByteStream, StorageNamespace,
    StoragePutStreamRequest, StorageService,
};

# async fn example() -> Result<(), graphql_orm_storage::StorageError> {
let service = StorageService::new(Arc::new(LocalStorageBackend::new("./data/storage")));

let stored = service
    .put_object_stream(StoragePutStreamRequest {
        namespace: StorageNamespace::Originals,
        file_name: Some("artifact.bin".to_string()),
        mime_type: Some("application/octet-stream".to_string()),
        body: StorageByteStream::from_bytes(b"bytes".to_vec()),
    })
    .await?;

assert_eq!(stored.size_bytes, 5);
# Ok(())
# }
```

`put_object_stream` computes the final byte count and SHA-256 checksum while the
backend writes the stream.

## Load A Streaming Object

```rust
# use std::sync::Arc;
# use graphql_orm_storage::{
#     LocalStorageBackend, StorageByteStream, StorageNamespace,
#     StoragePutStreamRequest, StorageService, collect_storage_stream,
# };
# async fn example() -> Result<(), graphql_orm_storage::StorageError> {
# let service = StorageService::new(Arc::new(LocalStorageBackend::new("./data/storage")));
# let stored = service.put_object_stream(StoragePutStreamRequest {
#     namespace: StorageNamespace::Originals,
#     file_name: Some("artifact.bin".to_string()),
#     mime_type: Some("application/octet-stream".to_string()),
#     body: StorageByteStream::from_bytes(b"bytes".to_vec()),
# }).await?;
let loaded = service.get_object_stream(&stored).await?;
let bytes = collect_storage_stream(loaded.body).await?;
# Ok(())
# }
```

Applications can use the stream directly instead of collecting it.

## Buffered Compatibility

The original buffered APIs remain available and delegate through the streaming
path:

- `StoragePutRequest`
- `StorageObjectBody`
- `StorageService::put_object`
- `StorageService::get_object`

They are convenient wrappers, not a separate provider implementation path.
