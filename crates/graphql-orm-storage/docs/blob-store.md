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

    async fn delete_blob(&self, key: &str) -> Result<(), StorageError>;
}
```

`list_blobs` is a convenience method that drains `list_blobs_page`.
Provider implementations should make `list_blobs_page` the native listing path.

`put_blob_if_not_exists` returns `Ok(None)` when the target key already exists.
It is the race-safe primitive for content-addressed deduplication.

`copy_blob` may use provider-side copy and does not return a SHA-256 checksum.
Callers can use `head_blob` after copying when they need backend metadata.

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

## Error Taxonomy

`StorageError::Provider` includes a `retryable` flag for future network
providers. Callers can use `StorageError::is_retryable()` to decide whether a
failed operation is worth retrying. Local filesystem IO errors are treated as
retryable; invalid keys, missing blobs, unsupported backends, and failed
preconditions are permanent.

## Relationship To ObjectStorage

`ObjectStorage` builds on `BlobStore`. Provider implementations should implement
`BlobStore` first, then expose object-storage behavior on top of it.

`StorageService` remains the high-level API for primary object metadata. It
generates object IDs, storage keys, sizes, hashes, and timestamps.

## Backup Integration

`BlobStore` is the intended future sharing point for `graphql-orm-backup`.
Backup repositories should adapt `BlobStore`; they should not use
`StorageService` or `StoredObject`, because backup keys and primary object
metadata have different semantics.
