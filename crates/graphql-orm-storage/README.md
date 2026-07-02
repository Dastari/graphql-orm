# graphql-orm-storage

`graphql-orm-storage` provides provider-neutral object storage primitives for
applications that use `graphql-orm`.

It stores bytes in object backends and returns metadata that host applications
can persist in their own `graphql-orm` entities. It does not define application
tables, authorization, upload routes, download routes, GraphQL resolvers, or
domain workflows.

## Highlights

- streaming `BlobStore` trait for low-level key-addressed blob storage
- high-level `StorageService` that generates object IDs, sharded keys, byte
  counts, SHA-256 checksums, and timestamps
- buffered and streaming object APIs
- local filesystem backend enabled by default
- S3-compatible backend behind the `s3` feature, including MinIO-compatible
  path-style configuration
- Azure Blob placeholder behind the `azure` feature that returns explicit
  `UnsupportedBackend` errors until implemented
- strict key validation for path safety across providers
- byte-range reads, conditional writes, provider-side copy hooks, and paged
  listing for cloud-provider compatibility
- retry-aware provider error taxonomy through `StorageError::is_retryable`

## Install

Default local filesystem support:

```toml
[dependencies]
graphql-orm-storage = { git = "https://github.com/Dastari/graphql-orm-storage" }
```

S3-compatible storage without the default local backend:

```toml
[dependencies]
graphql-orm-storage = {
    git = "https://github.com/Dastari/graphql-orm-storage",
    default-features = false,
    features = ["s3"],
}
```

Available provider features:

- `local` - enabled by default; provides `LocalStorageBackend`
- `s3` - provides `S3StorageBackend` and `S3StorageConfig`
- `azure` - provides unsupported placeholder types for future Azure Blob work

## Quick Local Example

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

// Persist this metadata in the host application's graphql-orm entity.
let object_id = stored.object_id;
let storage_key = stored.storage_key;
let sha256_hex = stored.sha256_hex;
# Ok(())
# }
```

## S3-Compatible Example

```rust
use std::sync::Arc;

use graphql_orm_storage::{
    S3StorageBackend, S3StorageConfig, StorageNamespace, StoragePutRequest,
    StorageService,
};

# async fn example() -> Result<(), graphql_orm_storage::StorageError> {
let backend = S3StorageBackend::new(S3StorageConfig {
    endpoint_url: "http://127.0.0.1:9000".to_string(),
    region: "us-east-1".to_string(),
    bucket: "objects".to_string(),
    key_prefix: Some("app-storage".to_string()),
    access_key_id: "minioadmin".to_string(),
    secret_access_key: "minioadmin".to_string(),
    path_style: true,
});

let service = StorageService::new(Arc::new(backend));
let stored = service
    .put_object(StoragePutRequest {
        namespace: StorageNamespace::Originals,
        file_name: Some("artifact.bin".to_string()),
        mime_type: Some("application/octet-stream".to_string()),
        bytes: b"bytes".to_vec(),
    })
    .await?;
# let _ = stored;
# Ok(())
# }
```

## Storage Metadata

Applications own their metadata entity so they can attach application-specific
fields and policies.

```rust
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "storage_objects", plural = "StorageObjects")]
pub struct StorageObjectRow {
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

The crate deliberately stores file bytes in object storage, not in database
rows.

## Documentation

- [Documentation index](docs/README.md)
- [Usage guide](docs/usage.md)
- [BlobStore API](docs/blob-store.md)
- [Streaming APIs](docs/streaming.md)
- [Architecture and crate boundaries](docs/architecture.md)
- [Provider roadmap](docs/provider-roadmap.md)
- [Backup integration guidance](docs/backup-integration.md)
- [Development and test commands](docs/development.md)
- [Release notes](docs/release-notes.md)

## Status

Current crate version: `0.3.0`.

Local filesystem and S3-compatible storage are implemented. Azure Blob remains
an explicit placeholder. Provider integration tests that require external
services are opt-in.
