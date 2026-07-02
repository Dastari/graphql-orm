# Release Notes

This page records user-facing changes for recent `graphql-orm-storage`
releases.

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
