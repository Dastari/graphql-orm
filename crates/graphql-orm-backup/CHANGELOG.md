---
title: "Changelog"
kind: reference
status: active
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-08-01
review_by: 2027-02-01
supersedes: []
---

# Changelog

## 0.7.1 - 2026-08-13

- Published the maintained backup configuration catalogue, provider-neutral
  examples, and current monorepo navigation. The manifest wire, repository
  layout, public API, and restore semantics are unchanged.
- Aligned current workspace guidance with `graphql-orm` 0.22.0 and
  `graphql-orm-storage` 0.6.1. No schema or data migration is required.

## 0.7.0

- Consolidated into the `graphql-orm` workspace and aligned the public storage
  type universe with `graphql-orm-storage` 0.6.0 through one workspace path.
- Lock acquisition now treats a winner that is temporarily unreadable after an
  atomic conditional-create collision as locked. It never deletes or replaces
  a lock whose age cannot be proved.
- Retained the backup API, manifest format, repository key layout, and
  fail-closed restore preflight from 0.6.0.

## 0.6.0

- Aligned the optional ORM integration with `graphql-orm` 0.16.0 at reviewed
  commit `dd68a001f47f04178bf3389dd47ee952faa6ecf0`.
- Confirmed that revision's optional `auth-agql` bridge pins `agql-auth`
  0.12.0 at `3f3b0c5365adfbe436514a681d977b600991b797`. Authorization remains
  host-owned, so this crate does not enable or directly depend on
  `agql-auth`.
- Renamed the SQLite and PostgreSQL conformance targets to reflect the
  `graphql-orm` 0.16 type universe.
- The public backup API, manifest format, repository key layout, restore
  preflight, and storage revision are unchanged from 0.5.0.

## 0.5.0

- Aligned the optional `graphql-orm` integration with the reviewed
  `graphql-orm` 0.15.0 commit
  `6beef53633befd90a4d4810887a3e4640dc4ad91`; the reviewed
  `graphql-orm-storage` 0.5.0 revision is unchanged.
- Added `orm-sqlite` and `orm-postgres` convenience features for selecting the
  adapter's exact backend while preserving `orm` for host feature unification.
- Added fail-before-write restore preflight: applying and dry-run restores now
  compare the manifest backend/schema hash with the target adapter before
  target-emptiness checks or imports.
- `GraphqlOrmBackupAdapter::restore_full` now receives the source manifest's
  `GraphqlOrmBackupSchema`, allowing adapters to repeat schema validation at
  the import boundary.
- Adapter-level column policy overrides now reject unknown columns and any
  `Include -> Redact -> Exclude` policy weakening. Effective overrides
  participate in the backup schema hash.
- Applying through `OrmBackupAdapter` now rejects restore contexts that do not
  request both administrative policy and change-journal suppression. Database
  roles and database-native policies remain host responsibilities.
- Added SQLite and disposable-PostgreSQL conformance coverage using two
  dependency-owned private schema modules without generated GraphQL roots.
  The fixture covers a nullable UUID reference, nullable JSON, redaction,
  exclusion, adapter policy overrides, consistent export, manifest
  verification, dependency-ordered empty-target restore, and exact object
  restore through a separate sink.
- The manifest format and repository key layout are unchanged. Backups created
  with adapter-level policy overrides under 0.4.x require the compatibility
  handling described in `MIGRATION.md`.

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
