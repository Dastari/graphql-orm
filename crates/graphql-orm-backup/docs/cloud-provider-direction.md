# Cloud Provider Direction

`graphql-orm-backup` should not duplicate S3 or Azure Blob SDK integrations
while `graphql-orm-storage` is expected to grow shared cloud blob support.

## Shared Layer

The intended shared point is a future lower-level `graphql-orm-storage::BlobStore`
abstraction, not the current high-level primary-object storage APIs.

Backup repositories and primary object storage have different semantics:

- backup repositories use arbitrary manifest, table, change, and content-addressed object keys
- primary object storage uses generated object ids, namespaces, checksums, and app-persisted metadata
- backup repositories need list operations for prefixes
- primary object storage should not inherit backup manifest semantics

## Future Adapter

Once `graphql-orm-storage::BlobStore` exists, add an optional adapter in this
crate:

```rust
pub struct BlobStoreBackupRepository {
    store: Arc<dyn graphql_orm_storage::BlobStore>,
    prefix: Option<String>,
}
```

Mapping:

- `BackupRepository::put_blob` calls `BlobStore::put_blob`
- `BackupRepository::get_blob` collects the blob stream into `bytes::Bytes`
- `BackupRepository::blob_exists` calls `BlobStore::blob_exists`
- `BackupRepository::list_blobs` calls `BlobStore::list_blobs`
- `BackupRepository::delete_blob` calls `BlobStore::delete_blob`

The adapter must apply and strip its configured repository prefix consistently.

## Provider Ownership

- S3-compatible and Azure Blob provider SDK integration should live in
  `graphql-orm-storage` once `BlobStore` exists.
- Dropbox is backup-specific and belongs in this crate.
- SMB starts as mounted filesystem support through `LocalBackupRepository`.

## Current Rule

Do not add direct AWS or Azure SDK dependencies to this crate until the shared
`BlobStore` path has been implemented or explicitly rejected.
