---
title: "BlobStore"
kind: reference
status: active
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-10-31
review_by: 2027-02-01
supersedes: []
---

# BlobStore

`BlobStore` is the low-level, key-addressed storage abstraction in
`graphql-orm-storage`.

Use it when code needs to put, get, list, inspect, or delete arbitrary safe blob
keys without creating primary object metadata.

## Boundary

`BlobStore` does not know about:

- GraphQL
- application authorization
- tenants
- collections
- database rows
- object namespaces as domain policy
- upload or download routes

It only stores bytes at provider-neutral keys.

## API Shape

```rust
#[async_trait::async_trait]
pub trait BlobStore: Send + Sync {
    fn backend(&self) -> StorageBackend;

    async fn put_blob(
        &self,
        key: &str,
        body: StorageByteStream,
        options: BlobPutOptions,
    ) -> Result<BlobWriteOutcome, StorageError>;

    async fn put_blob_if_not_exists(
        &self,
        key: &str,
        body: StorageByteStream,
        options: BlobPutOptions,
    ) -> Result<Option<BlobWriteOutcome>, StorageError>;

    async fn get_blob(&self, key: &str) -> Result<BlobBody, StorageError>;

    async fn get_blob_range(&self, key: &str, range: Range<u64>) -> Result<BlobBody, StorageError>;

    async fn blob_exists(&self, key: &str) -> Result<bool, StorageError>;

    async fn head_blob(&self, key: &str) -> Result<Option<BlobMetadata>, StorageError>;

    async fn list_blobs_page(
        &self,
        prefix: &str,
        continuation: Option<String>,
        limit: usize,
    ) -> Result<BlobListPage, StorageError>;

    async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, StorageError>;

    async fn copy_blob(&self, from: &str, to: &str) -> Result<(), StorageError>;

    async fn delete_blob_once(&self, key: &str) -> Result<(), StorageError>;

    async fn delete_blob(&self, key: &str) -> Result<(), StorageError>;
}
```

`list_blobs` is a convenience method that drains `list_blobs_page`.
Provider implementations should make `list_blobs_page` the native listing path.

`put_blob_if_not_exists` returns `Ok(None)` when the target key already exists.
Implementations must make this operation atomic across all writers. The default
returns non-retryable `StorageError::UnsupportedOperation` without provider I/O;
custom providers must override it using a native atomic primitive. A separate
existence check followed by a write is unsafe for deduplication and backup locks.

`delete_blob_once` is the release capability for lock keys that another writer
can immediately reuse. Its default returns `UnsupportedOperation` without I/O.
Implementations must never replay deletion by key after mutation may have
reached storage. Native S3 disables SDK operation retries and native SMB avoids
path reopening after disposition/close; local uses one filesystem removal.
Ordinary `delete_blob` can retain provider retry behavior. A lost response or
cancellation leaves outcome unknown: do not retry by key, and establish that all
writers, handles and in-flight requests are quiesced before offline recovery.
Operation errors from this capability must report non-retryable. Native providers
use `StorageError::SingleAttemptFailed` to preserve the cause in the error source
chain; the backup adapter also suppresses erroneous custom-provider retry hints.
A non-retryable flag does not mean that storage was unchanged.

`copy_blob` may use provider-side copy and does not return a SHA-256 checksum.
Callers can use `head_blob` after copying when they need backend metadata.

The local provider writes through temporary `.uploading` files. Host
applications can call `LocalStorageBackend::sweep_temp_files` periodically to
remove stale temp files.

## Key Safety

Blob keys are `/`-separated relative keys. `validate_blob_key` rejects:

- empty keys
- absolute paths
- empty path segments
- `.`
- `..`
- backslashes
- NUL bytes
- platform prefix components

`list_blobs("")` is allowed and lists all blobs.

## Local filesystem trust boundary

Use a root managed exclusively by the trusted storage deployment. Restrict
permissions on the root and its descendants so untrusted local users/applications
cannot create or replace storage entries. Protect ancestor path components against
replacement too. This applies equally to
local backups and OS-mounted shares using this backend.

Key validation is lexical. Ordinary local reads/writes follow existing links;
the configured root is not a confinement boundary against a hostile local
writer. Keep links that could resolve outside the intended tree out of the
storage hierarchy. The temporary-file sweep skips symbolic links, including
linked directories, broken links and cycles; it also skips non-file/non-directory
entries. A root that is itself a symbolic link is skipped by the sweep.
Deployments intentionally using links must arrange cleanup of their real target
roots explicitly.

The sweep inspects paths before operating on them. A concurrent replacement can
still race that inspection. Race-resistant confinement would require directory
handle relative resolution and no-follow operations at every path component,
with platform-specific handling of reparse points and all read/write/delete
paths. That is not implemented here. Canonicalizing a path before opening it
would still leave a check/use race. Do not use this backend as a sandbox for
untrusted local filesystem writers; isolate them through host permissions or a
separate storage provider.

## Error Taxonomy

`StorageError::Provider` includes a `retryable` flag for future network
providers. Callers can use `StorageError::is_retryable()` to decide whether a
failed operation is worth retrying. Local filesystem IO errors are treated as
retryable; invalid keys, missing blobs, unsupported backends, and failed
preconditions are permanent.

`StorageError::Multipart { key, kind }` reports a refused multipart operation
(unknown upload, backend mismatch, part number, part size or receipt mismatch).
It is permanent. See [multipart uploads](multipart.md).

## Multipart uploads

`BlobStore::as_multipart()` returns the provider's `MultipartBlobStore`, or none.
Local and S3 storage implement it; SMB and Azure do not. See
[resumable multipart uploads](multipart.md).

## Relationship To ObjectStorage

`ObjectStorage` builds on `BlobStore`. Provider implementations should implement
`BlobStore` first, then expose object-storage behavior on top of it.

`StorageService` remains the high-level API for primary object metadata. It
generates object IDs, storage keys, sizes, hashes, and timestamps.

## Backup Integration

`BlobStore` is the sharing point used by `graphql-orm-backup` through
`BlobStoreBackupRepository`. Backup repositories should adapt `BlobStore`; they should not use
`StorageService` or `StoredObject`, because backup keys and primary object
metadata have different semantics.
