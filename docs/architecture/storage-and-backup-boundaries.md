---
title: Storage, backup, and restore boundaries
kind: architecture
status: active
owner: workspace-maintainers
last_reviewed: 2026-08-01
review_by: 2027-02-01
supersedes: []
---

# Storage, backup, and restore boundaries

`graphql-orm-storage` owns provider-neutral byte and stream transport.
`graphql-orm-backup` owns snapshot manifests, repository layout, locks,
verification, compaction, and restore orchestration. `graphql-orm` exposes
dependency-owned schema-module and restore contracts but does not absorb either
companion package. `graphql-orm-ai` may depend on all three to implement its
own module-specific backup and applied-restore behavior.

## Data path

1. A host supplies a storage provider behind `BlobStore` or another documented
   storage contract.
2. Backup adapts that provider through `BlobStoreBackupRepository`; it does not
   duplicate S3, SMB, Azure, or local filesystem transport.
3. A package that owns durable records describes its schema module, backup
   metadata, restore hooks, and readiness conditions.
4. Restore validates manifest/backend/schema compatibility before target
   checks or writes, applies bounded work, reconciles incomplete effects, and
   opens runtime activity only after readiness succeeds.

Repository locking requires the provider’s atomic create-if-absent primitive.
An existence check followed by a write is not equivalent. Stream and range
interfaces preserve bounded memory use; HTTP status selection, response
headers, and client delivery remain host responsibilities.

## Evidence and operational guidance

- [Storage architecture](../../crates/graphql-orm-storage/docs/architecture.md)
- [Backup architecture](../../crates/graphql-orm-backup/docs/architecture.md)
- [Restore semantics](../../crates/graphql-orm-backup/docs/restore-semantics.md)
- [Streaming/range boundary investigation](../investigations/2026/storage-streaming-range-boundary.md)
- [Restore-readiness decision](../decisions/ADR-0005-restore-readiness-and-uncertain-effects.md)
- [Streaming-layering decision](../decisions/ADR-0006-streaming-storage-and-backup-layering.md)
