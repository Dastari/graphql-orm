# Usage Guide

`graphql-orm-storage` is a byte-storage companion crate. It stores object bytes
in a provider backend and returns metadata that the host application can persist
in its own `graphql-orm` entity.

The crate deliberately avoids application policy. It does not decide who can
upload, read, delete, or list objects. It also does not define database tables,
GraphQL resolvers, upload routes, download routes, tenant behavior, collection
behavior, media workflows, or audit events.

Use `StorageService` for primary object metadata workflows. Use `BlobStore` for
low-level key-addressed blob operations that do not need generated object
metadata.

When using `LocalStorageBackend`, interrupted writes can leave temporary
`*.uploading` files. Schedule `LocalStorageBackend::sweep_temp_files` from the
host application if long-running processes may be interrupted.

## Dependency

Default local filesystem support:

```toml
[dependencies]
graphql-orm-storage = { git = "https://github.com/Dastari/graphql-orm-storage" }
```

Provider-specific builds:

```toml
[dependencies]
graphql-orm-storage = {
    git = "https://github.com/Dastari/graphql-orm-storage",
    default-features = false,
    features = ["s3", "azure"],
}
```

## Store An Object

```rust
use std::sync::Arc;

use graphql_orm_storage::{
    LocalStorageBackend, StorageNamespace, StoragePutRequest, StorageService,
};

# async fn example() -> Result<(), graphql_orm_storage::StorageError> {
let service = StorageService::new(Arc::new(LocalStorageBackend::new("./data/storage")));

let stored = service
    .put_object(StoragePutRequest {
        namespace: StorageNamespace::Originals,
        file_name: Some("artifact.jpg".to_string()),
        mime_type: Some("image/jpeg".to_string()),
        bytes: b"image bytes".to_vec(),
    })
    .await?;

// Persist these fields in the host application's graphql-orm entity.
let object_id = stored.object_id;
let backend = stored.backend.as_str();
let namespace = stored.namespace.as_str();
let storage_key = stored.storage_key;
let size_bytes = stored.size_bytes;
let sha256_hex = stored.sha256_hex;
let created_at = stored.created_at;
# Ok(())
# }
```

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
        body: StorageByteStream::from_bytes(b"streamed bytes".to_vec()),
    })
    .await?;

assert_eq!(stored.size_bytes, 14);
# Ok(())
# }
```

## Load Or Delete An Object

The host application loads its own metadata row first, performs authorization,
then passes the stored metadata to the storage service.

```rust
# use std::sync::Arc;
# use graphql_orm_storage::{
#     LocalStorageBackend, StorageNamespace, StoragePutRequest, StorageService,
# };
# async fn example() -> Result<(), graphql_orm_storage::StorageError> {
# let service = StorageService::new(Arc::new(LocalStorageBackend::new("./data/storage")));
# let stored = service.put_object(StoragePutRequest {
#     namespace: StorageNamespace::Originals,
#     file_name: Some("artifact.jpg".to_string()),
#     mime_type: Some("image/jpeg".to_string()),
#     bytes: b"image bytes".to_vec(),
# }).await?;
let body = service.get_object(&stored).await?;
assert_eq!(body.object.object_id, stored.object_id);

service.delete_object(&stored).await?;
# Ok(())
# }
```

## Suggested Metadata Flow

1. Validate the upload request in the host application.
2. Check application-specific authorization before accepting bytes.
3. Call `StorageService::put_object`.
4. Insert the returned metadata into the host application's `graphql-orm` table.
5. If database insertion fails after storage succeeds, delete the object or
   enqueue an orphan cleanup job.
6. On downloads, load the metadata row first, check row-level access, then call
   `StorageService::get_object`.
7. On deletes, check access first, delete bytes, then update or delete metadata
   according to the host application's workflow.

## Suggested `graphql-orm` Entity

Applications own this entity. Add tenant, collection, user, workflow, policy,
or audit fields in the application, not in this crate.

```rust
#[derive(GraphQLEntity, GraphQLRelations, GraphQLOperations, async_graphql::SimpleObject)]
#[graphql_entity(
    table = "storage",
    plural = "StorageItems",
    default_sort = "created_at DESC"
)]
pub struct Storage {
    #[primary_key]
    pub id: graphql_orm::uuid::Uuid,

    #[unique]
    pub object_id: graphql_orm::uuid::Uuid,

    pub namespace: String,
    pub backend: String,

    #[unique]
    pub storage_key: String,

    pub original_file_name: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: i64,
    pub sha256_hex: String,
    pub created_at: i64,
}
```

## Object Key Safety

The original filename is metadata only. The generated storage key copies only a
sanitized extension from the filename. Local storage rejects unsafe keys before
joining them with the root path:

- empty keys
- absolute paths
- `.` path components
- `..` path components
- empty path components
- backslashes
- NUL bytes
- platform prefix components

Generated keys use:

```text
{namespace}/{uuid[0..2]}/{uuid[2..4]}/{uuid}.{extension}
```

## Provider Features

| Feature | Status | Public API |
| --- | --- | --- |
| `local` | Implemented and enabled by default | `LocalStorageBackend` |
| `s3` | Implemented with `aws-sdk-s3` | `S3StorageBackend`, `S3StorageConfig` |
| `azure` | Placeholder only | `AzureBlobStorageBackend`, `AzureBlobStorageConfig` |

The S3 backend supports S3-compatible services such as MinIO and respects the
`path_style` configuration flag. S3 integration tests run only when
`S3_TEST_ENDPOINT` and `S3_TEST_BUCKET` are set.

The Azure placeholder backend is intentionally explicit. It exposes the planned
configuration shape but returns `StorageError::UnsupportedBackend` until a real
Azure provider implementation lands.

## GraphQL Boundary

The core crate does not provide default GraphQL upload, download, delete, or
metadata mutation resolvers.

Storage access rules are application-specific. A host application may need
global policy checks, row ownership checks, collection membership checks,
admin bypass behavior, route-level bearer-token validation, audit logging, or
workflow-specific side effects. A reusable storage crate cannot safely infer
those rules.

Future GraphQL helper work should provide building blocks only and require an
application-supplied authorization adapter.

## Verification

Before publishing changes, run:

```bash
cargo fmt --check
cargo test
cargo test --all-features
cargo test --no-default-features
cargo check --features s3,azure --no-default-features
cargo clippy --all-features --all-targets -- -D warnings
cargo clippy --no-default-features --lib -- -D warnings
```
