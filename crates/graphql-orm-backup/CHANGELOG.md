# Changelog

## 0.4.0

- Enabled native SMB repositories through the storage crate's `smb` feature
  and `BlobStoreBackupRepository`; transport code remains in storage.
- Added backward-compatible streaming methods to `BackupRepository`,
  `BackupObjectIndex`, and `RestoreObjectSink`.
- Full backup, verification, and object restore now stream referenced objects
  and compute checksums incrementally.
- Added Samba lifecycle coverage for full create/load/list, verify, database
  and object restore, prune, delete, and simultaneous repository locking.
- The snapshot format and repository key layout are unchanged.
- Added migration guidance for streaming trait defaults, native-versus-mounted
  SMB configuration, dependency-source identity, release order, and host
  authorization boundaries.
- Pinned `graphql-orm-storage` 0.5.0 to the reviewed native-SMB release commit.

## 0.3.1

- Pinned `graphql-orm` 0.6.1 and `graphql-orm-storage` 0.4.0 to reviewed full
  Git commit revisions so downstream builds do not advance when either
  repository's default branch changes.
- Standardized dependency URLs on the canonical `.git` form so applications
  can use the same source identities and avoid duplicate crate instances.
- This release changes no public Rust API, backup format, snapshot layout, or
  restore behavior.

## 0.3.0

- Added the optional `orm` feature with a generic [`OrmBackupAdapter`] that
  bridges the `graphql-orm` `GraphqlOrmBackupRuntime` implementation on
  `Database` to this crate's `GraphqlOrmBackupAdapter` contract: schema
  snapshots, consistent full export, empty-target detection, and full restore.
  Incremental export/restore report `UnsupportedOperation` until a
  change-journal integration lands.
- Added `OrmBackupAdapter::with_column_backup_policy` so hosts can exclude or
  redact columns whose database types the export cannot round-trip yet (for
  example PostGIS geometry) without editing entity metadata, which would
  change migration-planning inputs. `ColumnBackupPolicy` is re-exported behind
  the `orm` feature.
- Added `OrmBackupAdapter::clear_restore_target` so hosts can replace an
  existing database before an empty-database restore. PostgreSQL clears with
  one `TRUNCATE ... CASCADE`; SQLite suspends `PRAGMA foreign_keys` on a
  dedicated connection around a child-first delete transaction because
  `RESTRICT` foreign keys are enforced immediately on both backends.
- Added `OrmBackupObjectIndex` (also behind `orm`), a `BackupObjectIndex` over
  one backup-enabled object metadata table plus the application's primary
  `BlobStore`. Hosts supply table and column names; rows without a valid
  recorded SHA-256 are hashed from the loaded blob bytes at listing time.
- Added `BlobStoreRestoreObjectSink`, a `RestoreObjectSink` that writes object
  bytes back to a `graphql-orm-storage` `BlobStore` at each object's original
  storage key.
- Added `delete_snapshot` and `DeleteSnapshotResult`: deletes one snapshot
  under the repository writer lock, refuses when another manifest depends on
  it, and removes object blobs no remaining snapshot references.
- Added `BackupError::Database` for database adapter failures with an
  operation-context message.
- Manifest and object verification plus backup object writes now iterate owned
  key/checksum pairs so the returned futures stay fully `Send`-generalizable
  inside async GraphQL resolvers.

## 0.2.0

- Replaced the crate-local filesystem repository internals with
  `graphql-orm-storage` blob stores; added `BlobStoreBackupRepository` so any
  storage blob provider can back a repository.
- Expanded crate documentation.

## 0.1.0

- Initial release: full backups, compressed snapshots and manifest chains,
  restore orchestration, incremental backups and synthetic-full compaction,
  repository locking, and retention pruning.
