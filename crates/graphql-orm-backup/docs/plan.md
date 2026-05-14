# graphql-orm-backup Implementation Plan

## Goal

Create a reusable backup and restore crate for applications using `graphql-orm`. The crate must support full snapshots first, then incremental snapshots after `graphql-orm` exposes a reliable change journal.

## What This Crate Provides

- Snapshot manifest format.
- Backup repository trait.
- Local backup repository.
- Database backup adapter contract.
- Stored-object index adapter contract.
- Full backup planner.
- Full snapshot writer.
- Verification helpers.
- Restore context and initial empty-target safety checks.

## What This Crate Must Not Provide

- Application authentication.
- Application authorization or row policy decisions.
- UI or scheduling.
- Digitise-specific entity names or workflow assumptions.
- Primary object storage implementation details beyond reading objects through `BackupObjectIndex`.

## Initial Implementation Order

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

## Expected Output From A Backup Agent

- A compilable crate under `/home/toby/graphql-orm-backup`.
- Manifest format docs.
- Restore semantics docs.
- Local repository implementation.
- Full snapshot creation API.
- Tests for manifest, local repository, verification, and planning.
- Precise list of missing graphql-orm APIs.

## Future Work

- Stream database exports instead of holding rows in memory.
- Add zstd compression for table and change files.
- Add S3 backup repository.
- Add Azure Blob backup repository.
- Add Dropbox backup repository.
- Add SMB mounted-path documentation and validation.
- Implement full restore after graphql-orm import lands.
- Implement incremental backup after graphql-orm change journal lands.
