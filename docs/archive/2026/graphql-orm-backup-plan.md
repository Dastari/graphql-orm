---
title: "graphql-orm-backup Implementation Plan"
kind: reference
status: archived
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-08-01
review_by: none
supersedes: []
---

> **Archived (2026-08-01).** This completed implementation plan is retained as
> historical context. Current provider work is tracked in
> [the backup-provider backlog](../../../docs/plans/backlog/backup-providers/README.md).

# graphql-orm-backup Implementation Plan

## Goal

Create a reusable backup and restore crate for applications using `graphql-orm`. The crate must support full snapshots first, then incremental snapshots after `graphql-orm` exposes a reliable change journal.

## What This Crate Provides

- Snapshot manifest format.
- Manifest chain loading and validation.
- Backup repository trait.
- Local backup repository.
- Database backup adapter contract.
- Stored-object index adapter contract.
- Full backup planner.
- Full snapshot writer.
- Verification helpers.
- Zstd-compressed table payloads.
- Restore context and initial empty-target safety checks.

## What This Crate Must Not Provide

- Application authentication.
- Application authorization or row policy decisions.
- UI or scheduling.
- Host application-specific entity names or workflow assumptions.
- Primary object storage implementation details beyond reading objects through `BackupObjectIndex`.

## Historical Initial Implementation Order

1. Implement manifest types and checksum helpers.
2. Implement `BackupRepository`.
3. Implement `LocalBackupRepository`.
4. Implement `BackupObjectIndex`.
5. Implement `GraphqlOrmBackupAdapter` as an interim integration contract.
6. Implement full backup planning.
7. Implement full snapshot creation.
8. Implement manifest/object verification.
9. Implement restore context and empty-target guard.
10. Wait for finalized `graphql-orm` export/import/change-journal APIs.

The full export/import runtime API has since landed and
`graphql-orm-backup` 0.7.0 resolves the reviewed `graphql-orm` 0.16.0 workspace
package.
Incremental adapter execution still waits for a reliable integrated change
journal.

## Expected Output From A Backup Agent

- A compilable crate under `crates/graphql-orm-backup` in the
  `graphql-orm` workspace.
- Manifest format docs.
- Restore semantics docs.
- Local repository implementation.
- Full snapshot creation API.
- Tests for manifest, local repository, verification, and planning.
- Precise list of missing graphql-orm APIs.

## Future Work

- Stream database exports instead of holding rows in memory.
- Add zstd compression for future change files.
- Improve provider-native pagination for repositories with very large key sets.
- Add Azure Blob backup repository through the shared `BlobStore` adapter path.
- Add Dropbox backup repository.
- Implement incremental backup after graphql-orm change journal lands.

Implemented since the initial plan: `BlobStoreBackupRepository`, S3 reuse,
native SMB reuse, full restore, referenced-object restore, deletion, pruning,
locking, streamed referenced-object transfer, exact restore schema preflight,
and private schema-module conformance on SQLite and PostgreSQL.
