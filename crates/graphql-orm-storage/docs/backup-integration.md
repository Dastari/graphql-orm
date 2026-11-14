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

`graphql-orm-backup` 0.8.0 exposes this adapter:

```rust
pub struct BlobStoreBackupRepository {
    store: Arc<dyn graphql_orm_storage::BlobStore>,
    prefix: Option<String>,
}
```

Mapping:

- Lock release uses `BackupRepository::delete_blob_once` through
  `BlobStore::delete_blob_once`; both require explicit provider opt-in and never
  replay deletion by key after the server may have applied it. Ordinary deletion
  remains a separate operation. Failed/cancelled release has an unknown outcome.

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
Its `put_blob_if_not_exists` relies on the provider enforcing
`If-None-Match: *`; some S3-compatible providers accept the header and
ignore it, and repository locks on them would not be exclusive. Each
`S3StorageBackend` instance therefore runs
`S3StorageBackend::probe_conditional_create` before its first conditional
create, which includes the first repository lock acquisition, and caches only
a pass. A failed probe makes that create, and so lock acquisition, fail with a
non-retryable `RemoteProvider` error of kind `ConditionalCreateUnverified`.
The probe creates, re-creates and deletes one random
`.graphql-orm-probe-<uuid>` object below the key prefix, so the credentials
need write, read and delete permission there.

The probe sends only a tiny `PutObject` and a one-part completion, so it
cannot certify enforcement that depends on object size, part count or a proxy
in front of the provider. If it is cancelled, or its cleanup fails, its
`.graphql-orm-probe-<uuid>` object, and possibly an incomplete multipart
upload for that key, remains below the key prefix and is safe to delete.
Where the probe's writes are not permitted, a host that verified enforcement
another way can construct the backend with
`with_conditional_writes_verified_by_host()`. That is an assertion of
responsibility: if it is wrong, two holders can acquire one lock.
Future Azure Blob SDK integration should also live in this crate.
`graphql-orm-backup` should adapt these providers instead of duplicating cloud
SDK code.

Dropbox remains backup-specific and is not a primary object storage provider for
this crate.
