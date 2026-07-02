# Project Plan

`graphql-orm-storage` is the reusable byte-storage companion crate for
`graphql-orm` applications. The crate owns object and blob storage concerns
only. Host applications remain responsible for authorization, GraphQL schema
design, metadata entities, routing, audit behavior, and workflow-specific
policy.

## Implemented In 0.3.0

- Provider-neutral object metadata.
- Provider-neutral `StorageBackend` and `StorageNamespace` enums.
- Streaming `BlobStore` trait with:
  - streaming writes and reads
  - byte-range reads
  - conditional writes for content-addressed deduplication
  - provider-side copy hook
  - paged listing
  - existence and metadata checks
- Buffered and streaming `StorageService` object APIs.
- Local filesystem backend.
- S3-compatible backend behind the `s3` feature.
- Azure Blob placeholder behind the `azure` feature.
- SHA-256 checksum helpers.
- Safe sharded storage-key generation.
- Strict key validation for local and cloud providers.
- Public documentation and rustdocs for the crate boundary.

## Crate Boundaries

This crate must not provide:

- application auth or policy checks
- default GraphQL upload/download/delete resolvers
- application-specific collection, record, media, tenant, or policy assumptions
- database entities that force one application schema
- file bytes stored in database rows
- backup repository providers such as Dropbox or SMB

Applications should persist returned `StoredObject` metadata in their own
`graphql-orm` entities.

## Application Integration Pattern

1. Validate upload requests in the host application.
2. Apply application-specific authorization before accepting bytes.
3. Call `StorageService::put_object` or `StorageService::put_object_stream`.
4. Persist the returned `StoredObject` fields in the application database.
5. If database insertion fails after storage succeeds, delete the stored object
   or enqueue orphan cleanup in the host application.
6. On downloads, load metadata first, authorize it, then call
   `StorageService::get_object` or `StorageService::get_object_stream`.

## Backup Integration Pattern

`graphql-orm-backup` should adapt `BlobStore` directly instead of using
`StorageService`. Backup repositories use manifest, table, change-payload, and
content-addressed object keys; those keys should not be forced through primary
object metadata or generated storage namespaces.

## Future Work

- Implement Azure Blob as a real `BlobStore` provider.
- Add a `graphql-orm-backup` adapter that wraps `BlobStore` as a backup
  repository.
- Add optional provider-managed encryption configuration if primary object
  storage applications need it.
