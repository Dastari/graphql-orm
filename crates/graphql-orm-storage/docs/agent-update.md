# Agent Update

This update summarizes the `0.3.0` storage-provider boundary for agents working
on `graphql-orm-storage` or downstream crates.

## What Changed

- Added the streaming `BlobStore` trait as the low-level provider abstraction.
- Added `StorageByteStream`, `BlobBody`, `BlobMetadata`, and
  `BlobWriteOutcome`.
- Added `BlobPutOptions` for write metadata passthrough.
- Added `BlobListPage` for continuation-token listing.
- Added byte-range reads, conditional writes, and blob copy to `BlobStore`.
- Added retryable provider error taxonomy through `StorageError`.
- Added `validate_blob_key` for consistent provider key validation.
- Added `StorageService::put_object_stream` and
  `StorageService::get_object_stream`.
- Buffered object APIs still exist and delegate through the streaming layer.
- `LocalStorageBackend` now implements `BlobStore` and `ObjectStorage`.
- S3 and Azure Blob placeholders now implement `BlobStore` and still return
  `UnsupportedBackend`.

## Provider Guidance

New providers should implement `BlobStore` first. `ObjectStorage` should remain a
thin layer over blob operations unless a provider has object-specific behavior
that belongs in this crate.

Do not add default GraphQL upload, download, or delete resolvers. Applications
own GraphQL schema design, authorization, and persistence of `StoredObject`
metadata.

## Backup Guidance

`graphql-orm-backup` should adapt `BlobStore` directly. It should not use
`StorageService`, because backup repositories use arbitrary manifest, table, and
object keys rather than primary object namespaces and generated `StoredObject`
metadata.

Future adapter shape:

```rust
pub struct BlobStoreBackupRepository {
    store: std::sync::Arc<dyn graphql_orm_storage::BlobStore>,
    prefix: Option<String>,
}
```

## Still Pending

- Real S3 `BlobStore` provider using the `0.3.0` trait surface.
- Real Azure Blob `BlobStore` provider.
- Backup adapter implementation after downstream crate alignment.
- Any cloud SDK dependency decisions.
- Provider integration tests that require external services.
