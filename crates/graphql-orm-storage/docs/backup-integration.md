---
title: "Backup Integration"
kind: reference
status: active
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-08-01
review_by: 2027-02-01
supersedes: []
---

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

## Adapter Shape

`graphql-orm-backup` 0.7.0 exposes this adapter:

```rust
pub struct BlobStoreBackupRepository {
    store: Arc<dyn graphql_orm_storage::BlobStore>,
    prefix: Option<String>,
}
```

Mapping:

- `BackupRepository::put_blob` calls `BlobStore::put_blob` with backup-owned
  write options
- `BackupRepository::get_blob_stream` preserves `BlobStore` streaming, while
  the buffered `get_blob` helper remains suitable for small metadata
- `BackupRepository::blob_exists` calls `BlobStore::blob_exists`
- `BackupRepository::put_blob_if_absent` calls
  `BlobStore::put_blob_if_not_exists`, the atomic primitive used for repository
  locking
- `BackupRepository::list_blobs` calls `BlobStore::list_blobs_page` or
  `BlobStore::list_blobs`
- `BackupRepository::delete_blob` calls `BlobStore::delete_blob`

The adapter should apply and strip its configured repository prefix
consistently.

## Provider Ownership

S3-compatible storage lives in this crate as a `BlobStore` implementation.
Future Azure Blob SDK integration should also live in this crate.
`graphql-orm-backup` should adapt these providers instead of duplicating cloud
SDK code.

Dropbox remains backup-specific and is not a primary object storage provider for
this crate.
