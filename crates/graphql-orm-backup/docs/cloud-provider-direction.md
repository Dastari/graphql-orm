# Cloud Provider Direction

`graphql-orm-backup` should not duplicate S3 or Azure Blob SDK integrations
while `graphql-orm-storage` is expected to grow shared cloud blob support.

## Shared Layer

The shared point is the lower-level `graphql-orm-storage::BlobStore`
abstraction, not the high-level primary-object storage APIs.

Backup repositories and primary object storage have different semantics:

- backup repositories use arbitrary manifest, table, change, and content-addressed object keys
- primary object storage uses generated object ids, namespaces, checksums, and app-persisted metadata
- backup repositories need list operations for prefixes
- primary object storage should not inherit backup manifest semantics

## Adapter

`graphql-orm-backup` now exposes `BlobStoreBackupRepository`:

```rust
pub struct BlobStoreBackupRepository {
    store: Arc<dyn graphql_orm_storage::BlobStore>,
    prefix: Option<String>,
}
```

Mapping:

- `BackupRepository::put_blob` calls `BlobStore::put_blob`
- `BackupRepository::put_blob_if_absent` calls `BlobStore::put_blob_if_not_exists`
- `BackupRepository::get_blob_stream` preserves native streaming; the buffered
  `get_blob` convenience method remains for small metadata
- `BackupRepository::blob_exists` calls `BlobStore::blob_exists`
- `BackupRepository::list_blobs` calls `BlobStore::list_blobs`
- `BackupRepository::delete_blob` calls `BlobStore::delete_blob`

The adapter must apply and strip its configured repository prefix consistently.

## Provider Ownership

- S3-compatible and Azure Blob provider SDK integration belongs in
  `graphql-orm-storage`; S3 already implements `BlobStore`, while Azure remains
  an explicit unsupported placeholder.
- Dropbox is backup-specific and belongs in this crate.
- Native SMB lives in `graphql-orm-storage`; mounted SMB remains an explicitly
  named legacy use of `LocalBackupRepository`.

## Current Rule

Do not add direct AWS or Azure SDK dependencies to this crate. Cloud provider SDK
integration belongs in `graphql-orm-storage` as `BlobStore` implementations.
