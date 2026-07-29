# Release Notes

## 0.6.0

- Hardened native SMB streaming writes against negotiated request limits and
  partial acknowledgements without buffering complete bodies.
- Added bounded, concurrency-safe parent-directory coordination and reconnect
  cache invalidation for large content-addressed backups.
- Added cleanup ownership for cancelled direct and temporary uploads, bounded
  idempotent reconnect behavior, expanded probe facts, and redaction-safe live
  diagnostics.
- Expanded the managed Samba matrix with a constrained `MaxWriteSize`, upload
  concurrency 1/2/4/8, cancellation cleanup, directory-work assertions, and
  downstream backup lifecycle coverage.

## 0.5.0

- Added feature-gated, pure-Rust native SMB2/SMB3 `BlobStore` support with
  redacted credentials, signing/encryption policy, atomic conditional CREATE,
  streamed temp/flush/rename writes, paged listing, reconnect, and probe APIs.
- Added structured remote-provider error classifications.
- Added Samba tests covering auth, workgroup credentials, signing, encryption,
  streaming round trips, interruption cleanup, atomic lock creation, and
  reconnect after server restart.
- Added migration guidance for exhaustive enum matches, direct SMB
  configuration, mounted-provider compatibility, and downstream release order.

This page records user-facing changes for recent `graphql-orm-storage`
releases.

## 0.4.0

Large-object streaming support for recording-style workloads.

- Bumped `graphql-orm-storage` to `0.4.0`.
- Added `StreamingObjectStore` for bucket/key objects.
- Added `ObjectInfo`, `ObjectMetadata`, `ObjectRangeBody`, and
  `ObjectContentRange`.
- Added `MultipartWriter` and `BoxedMultipartWriter` for incremental writes.
- Implemented local multipart writes with temp files and atomic visibility on
  completion.
- Added range reads for HTTP playback use cases.
- Added local object sidecar metadata persistence, listing, and deletion.
- Added tests for large streamed writes, range reads, abort cleanup, metadata,
  listing visibility, and retention deletion.

## 0.3.0

Provider API stabilization and S3 implementation.

- Bumped `graphql-orm-storage` to `0.3.0`.
- Added retry-aware provider errors through `StorageError::Provider` and
  `StorageError::is_retryable`.
- Added `StorageError::PreconditionFailed` for conditional and range/listing
  precondition failures.
- Added `BlobPutOptions` for provider write metadata such as content type.
- Added `BlobListPage` and `BlobStore::list_blobs_page` for continuation-token
  listing.
- Added `BlobStore::get_blob_range`, `put_blob_if_not_exists`, and `copy_blob`.
- Marked `StorageBackend` and `StorageNamespace` as `#[non_exhaustive]`.
- Added `LocalStorageBackend::sweep_temp_files` for stale `.uploading` cleanup.
- Reworked local paged listing to walk incrementally instead of collecting the
  whole tree for each page.
- Implemented S3-compatible storage behind the `s3` feature using
  `aws-sdk-s3`.
- Added opt-in S3 integration tests controlled by `S3_TEST_ENDPOINT` and
  `S3_TEST_BUCKET`.
- Kept Azure Blob as an explicit unsupported placeholder.

## 0.2.0

Streaming blob storage foundation.

- Added the streaming `BlobStore` trait.
- Added `StorageByteStream`, `BlobBody`, `BlobMetadata`, and
  `BlobWriteOutcome`.
- Rebuilt local storage on top of `BlobStore`.
- Added streaming object APIs while preserving buffered `StorageService`
  methods.
- Added backup integration guidance to adapt `BlobStore` directly.

## 0.1.0

Initial baseline.

- Added provider-neutral object metadata.
- Added local filesystem storage.
- Added key generation, checksum helpers, and path-safety validation.
- Added unsupported S3 and Azure Blob placeholder APIs.
