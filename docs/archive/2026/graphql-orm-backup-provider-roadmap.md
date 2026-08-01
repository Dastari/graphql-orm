---
title: "Provider Roadmap"
kind: reference
status: archived
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-08-01
review_by: none
supersedes: []
---

> **Superseded (2026-08-01).** This historical roadmap is retained for
> context. Current provider work is tracked in
> [the backup-provider backlog](../../../docs/plans/backlog/backup-providers/README.md).

# Provider Roadmap

## Phase 1: Local Filesystem

Implemented first as the deterministic baseline.

Acceptance criteria:

- put/get/list/delete blob
- nested key support
- path traversal rejection
- delete missing blob succeeds

## Phase 2: S3

Do not implement direct AWS SDK integration in this crate.

`graphql-orm-backup` adapts `graphql-orm-storage::BlobStore` through
`BlobStoreBackupRepository`, so S3-compatible provider code should live in
`graphql-orm-storage`.

Expected configuration:

- endpoint URL
- region
- bucket
- prefix
- credentials
- path-style toggle

## Phase 3: Azure Blob

Do not implement direct Azure SDK integration in this crate. Azure Blob should
follow the same `graphql-orm-storage::BlobStore` adapter path as S3 once the
storage crate provides a real Azure Blob implementation.

Expected configuration:

- account/container or connection string
- container
- prefix
- credentials

## Phase 4: SMB

Native SMB is implemented by `graphql-orm-storage::SmbStorageBackend` and
adapted through `BlobStoreBackupRepository`. Mounted filesystem support remains
an explicitly named legacy deployment option.

## Phase 5: Dropbox

Dropbox should be a backup repository provider only. It should not become a primary object storage backend unless a future product requirement justifies that.
