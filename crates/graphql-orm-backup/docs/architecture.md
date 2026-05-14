# graphql-orm-backup Architecture

## Boundary

`graphql-orm-backup` orchestrates backup and restore. It delegates database details to `graphql-orm` and delegates application object loading to a `BackupObjectIndex` adapter.

## Collaborating Components

- `graphql-orm`: entity metadata, schema hash, row export/import, change journal, restore context.
- `graphql-orm-storage`: stored object metadata and object storage primitives.
- Application: auth, scheduling, admin APIs, audit, and provider configuration.
- `graphql-orm-backup`: manifests, repositories, verification, backup planning, restore orchestration.

## Full Backup Flow

`create_full_backup` implements the current full snapshot flow:

1. Read schema snapshot from `GraphqlOrmBackupAdapter`.
2. Export all backup-enabled tables through `GraphqlOrmBackupAdapter`.
3. List all referenced stored objects through `BackupObjectIndex`.
4. Write table exports to the backup repository as uncompressed JSON Lines.
5. Load and checksum each object from `BackupObjectIndex`.
6. Write object blobs by content-addressed key if missing.
7. Build manifest.
8. Set manifest checksum.
9. Write manifest last.

## Incremental Backup Flow

Incremental backup is blocked on graphql-orm change journal support.

Expected flow:

1. Load parent snapshot marker.
2. Ask graphql-orm for changed rows and tombstones since the parent.
3. Ask object index for newly referenced or changed objects.
4. Write change files and objects.
5. Write incremental manifest with `parent_snapshot_id`.

## Restore Flow

1. Load selected manifest.
2. Load and verify parent manifest chain.
3. Verify manifest checksums.
4. Verify object and table checksums.
5. Confirm target database/object store is empty.
6. Run migrations to compatible schema.
7. Import full snapshot rows in dependency order.
8. Apply incremental snapshots in order.
9. Restore objects.
10. Verify restored row counts and checksums.

Only the safety scaffolding exists until graphql-orm import/export APIs are finalized.
