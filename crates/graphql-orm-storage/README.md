# graphql-orm-storage

Provider-neutral object storage primitives for applications that use `graphql-orm`.

This crate stores bytes in an object backend and returns metadata that an application can persist in its own `graphql-orm` entity. It deliberately does not define application concepts such as collections, records, accessions, tenants, users, or media workflows.

## Current Status

- Local filesystem backend implemented.
- Stable object metadata and key generation implemented.
- S3 and Azure Blob expose explicit unsupported placeholder backends behind feature flags for later provider work.

## Design Rule

Do not store file bytes in the application database. Store bytes in an object backend and persist only metadata in the database.

The core crate does not provide default GraphQL resolvers. Upload, download, delete, and metadata mutation resolvers need host-application authorization and row-policy logic. Future GraphQL helpers should require the application to inject an explicit access-policy adapter.

## Cargo Features

- `local`: enabled by default; provides `LocalStorageBackend`.
- `s3`: provides `S3StorageBackend` and `S3StorageConfig` placeholders that return an unsupported-backend error until real S3-compatible storage is implemented.
- `azure`: provides `AzureBlobStorageBackend` and `AzureBlobStorageConfig` placeholders that return an unsupported-backend error until real Azure Blob Storage is implemented.

For detailed integration guidance, see [docs/usage.md](docs/usage.md).

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

// Persist this metadata in your application's graphql-orm entity.
let object_id = stored.object_id;
let storage_key = stored.storage_key;
let sha256_hex = stored.sha256_hex;
# Ok(())
# }
```

## Suggested graphql-orm Entity Shape

Applications should own their metadata entity so they can attach their own tenant, collection, user, or workflow fields.

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

## Object Keys

Default object keys use this format:

```text
{namespace}/{uuid[0..2]}/{uuid[2..4]}/{uuid}.{extension}
```

Example:

```text
originals/6c/57/6c57a6cc-09e6-4a7f-a320-2f2bde4cfd86.jpg
```

Only the file extension is copied from the original filename. The original filename is never used as an object path.

## Provider Roadmap

1. Local filesystem
2. S3-compatible object storage
3. Azure Blob Storage

Backup repositories such as Dropbox and SMB belong in `graphql-orm-backup`, not this crate.

## Verification

```bash
cargo fmt --check
cargo test
cargo test --all-features
cargo test --no-default-features
cargo check --features s3,azure --no-default-features
cargo clippy --all-features --all-targets -- -D warnings
cargo clippy --no-default-features --lib -- -D warnings
```
