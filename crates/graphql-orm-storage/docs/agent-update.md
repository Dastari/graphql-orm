# Agent Update

This update summarizes the `0.6.0` storage-provider boundary for agents working
on `graphql-orm-storage` or downstream crates.

## SMB hardening in 0.6.0

- Native SMB consumes arbitrary `StorageByteStream` chunks directly. Callers no
  longer need to split them to 1 MiB.
- Each SMB WRITE is bounded by the negotiated maximum and the backend's
  conservative request rule.
- Shared parent directories are cached and concurrent creation is coalesced.
- Reconnects invalidate directory knowledge and replace a failed client
  generation without replaying arbitrary upload bodies or ambiguous
  conditional creates.
- Use `SmbStorageBackend::diagnostics` for long-running transfer counters and
  `SmbStorageBackend::probe` for negotiated security and request-size facts.

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
- `LocalStorageBackend` implements `StreamingObjectStore` for local large-object
  streaming and recording-style workloads.
- S3 now implements `BlobStore` and `ObjectStorage` behind the `s3` feature.
- Azure Blob remains a placeholder that implements `BlobStore` and still returns
  `UnsupportedBackend`.
- Native SMB2/SMB3 implements `BlobStore` behind the `smb` feature.

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

The downstream adapter is implemented as:

```rust
pub struct BlobStoreBackupRepository {
    store: std::sync::Arc<dyn graphql_orm_storage::BlobStore>,
    prefix: Option<String>,
}
```

## Still Pending

- Real Azure Blob `BlobStore` provider.
- Manual native SMB compatibility runs against Windows Server and common NAS
  implementations; the automated Samba suite is opt-in.
