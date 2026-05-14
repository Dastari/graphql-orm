# Backup Integration

`graphql-orm-backup` should reuse storage provider code through `BlobStore`, not
through `StorageService`.

## Why Not StorageService?

`StorageService` is for primary object workflows:

- generated object IDs
- logical namespaces
- generated storage keys
- size and SHA-256 metadata
- app-persisted `StoredObject` fields

Backup repositories have different semantics:

- arbitrary manifest keys
- table payload keys
- change payload keys
- content-addressed object keys
- prefix listing

Those repository keys should not be forced through primary object metadata.

## Future Adapter Shape

Once `graphql-orm-backup` depends on a version of this crate with `BlobStore`, it
can add an adapter like:

```rust
pub struct BlobStoreBackupRepository {
    store: Arc<dyn graphql_orm_storage::BlobStore>,
    prefix: Option<String>,
}
```

Mapping:

- `BackupRepository::put_blob` calls `BlobStore::put_blob`
- `BackupRepository::get_blob` collects or streams `BlobStore::get_blob`
- `BackupRepository::blob_exists` calls `BlobStore::blob_exists`
- `BackupRepository::list_blobs` calls `BlobStore::list_blobs`
- `BackupRepository::delete_blob` calls `BlobStore::delete_blob`

The adapter should apply and strip its configured repository prefix
consistently.

## Provider Ownership

S3-compatible and Azure Blob SDK integrations should live in this crate as
`BlobStore` implementations. `graphql-orm-backup` should adapt them instead of
duplicating cloud SDK code.

Dropbox remains backup-specific and is not a primary object storage provider for
this crate.
