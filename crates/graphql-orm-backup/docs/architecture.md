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
4. Serialize table exports as JSON Lines.
5. Compress table exports with zstd, checksum the stored compressed bytes, and write them to the backup repository.
6. Load and checksum each object from `BackupObjectIndex`.
7. Write object blobs by content-addressed key if missing.
8. Build manifest.
9. Set manifest checksum.
10. Write manifest last.

## Incremental Backup Flow

`create_incremental_backup` implements the repository-side incremental snapshot flow. It depends on
the application or future `graphql-orm` runtime adapter returning reliable changes from
`GraphqlOrmBackupAdapter::export_incremental`.

1. Load parent snapshot marker.
2. Ask the adapter for changed rows and deletes since the parent.
3. Ask object index for newly referenced or changed objects.
4. Serialize changes as JSON Lines.
5. Compress change payloads with zstd, checksum the stored compressed bytes, and write them.
6. Write object blobs by content-addressed key if missing.
7. Emit delete tombstones for `BackupChangeAction::Delete`.
8. Write an incremental manifest with `parent_snapshot_id`.

## Restore Flow

`restore_snapshot` implements database restore orchestration.

1. Load selected manifest.
2. Load and validate parent manifest chain.
3. Verify manifest checksums.
4. Verify object, table, and change payload checksums.
5. Confirm target database is empty for `RestoreMode::EmptyDatabase`.
6. Download, decompress, and parse full table payloads.
7. Call `GraphqlOrmBackupAdapter::restore_full`.
8. Download, decompress, and parse incremental change payloads in chain order.
9. Call `GraphqlOrmBackupAdapter::restore_incremental` for each incremental manifest.

`RestoreMode::DryRun` performs steps 1-6 and incremental parsing without calling restore methods.
Stored object rehydration is explicit through `restore_objects` and a caller-supplied
`RestoreObjectSink`.

## Compaction Flow

`compact_chain` folds a full-plus-incremental chain into a `SyntheticFull` snapshot.

1. Load and verify the source manifest chain.
2. Parse the full table payloads into in-memory row maps keyed by primary key.
3. Apply incremental create/update/delete records in chain order.
4. Write the resulting tables as compressed JSON Lines under the new snapshot id.
5. Carry forward non-tombstoned object entries.
6. Write a new synthetic-full manifest.

## Operational Safety

- `create_full_backup`, `create_incremental_backup`, `compact_chain`, and `prune` acquire the
  repository advisory lock.
- Object blob writes and checksum verification use bounded concurrency with configurable limits.
- `prune` retains manifest chains selected by `KeepPolicy` and removes expired snapshot blobs plus
  unreferenced content-addressed object blobs.
