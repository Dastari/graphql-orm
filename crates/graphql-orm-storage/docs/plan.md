# graphql-orm-storage Implementation Plan

## Goal

Create a reusable object storage crate for applications that use `graphql-orm`. The crate owns byte storage concerns only. Applications remain responsible for authorization, domain ownership, GraphQL entities, upload routes, download routes, and workflow-specific behavior.

## What This Crate Provides

- Provider-neutral object metadata.
- Provider-neutral object storage trait.
- Storage service that generates object IDs, keys, sizes, hashes, and timestamps.
- Local filesystem backend.
- Feature placeholders for S3 and Azure Blob.
- Tests for key generation, checksum generation, local round trips, and path safety.

## What This Crate Must Not Provide

- Application auth or policy checks.
- Default GraphQL upload/download resolvers.
- Digitise collection, record, accession, media, or tenant assumptions.
- Database entities that force one application schema.
- File blobs in database rows.
- Backup repository providers such as Dropbox or SMB.

## Initial Implementation

1. Define `StorageBackend` and `StorageNamespace`.
2. Define `StoragePutRequest`, `StoredObject`, and `StorageObjectBody`.
3. Define `ObjectStorage`.
4. Define `StorageService`.
5. Implement SHA-256 checksums.
6. Implement safe sharded object key generation.
7. Implement `LocalStorageBackend`.
8. Add tests for the local backend and key safety.

## Integration Pattern For Applications

Applications should call `StorageService::put_object`, then persist the returned `StoredObject` fields into their own `graphql-orm` entity.

If database insertion fails after object storage succeeds, application code should delete the stored object or enqueue an orphan cleanup job. This crate deliberately does not know the application transaction boundary.

Applications should also own GraphQL resolvers and route handlers. A future optional GraphQL helper must be authorization-adapter driven and must not expose generic upload/download operations without host-provided access checks.

## Expected Output From A Storage Agent

- A compilable crate under `/home/toby/graphql-orm-storage`.
- Public API documented in `README.md`.
- Local backend tests passing with `cargo test`.
- Provider roadmap documented.
- Notes explaining what Digitise must change to consume this crate.

## Future Work

- Add streaming upload/download APIs so large files do not need to fit in memory.
- Add S3-compatible provider behind the `s3` feature.
- Add Azure Blob provider behind the `azure` feature.
- Add optional object existence and metadata APIs if backup verification needs them.
- Add optional server-side encryption hooks if applications need provider-managed keys.
