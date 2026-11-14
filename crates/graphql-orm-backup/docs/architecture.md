---
title: "graphql-orm-backup Architecture"
kind: architecture
status: active
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-11-12
review_by: 2027-02-01
supersedes: []
---

# graphql-orm-backup Architecture

## Boundary

`graphql-orm-backup` orchestrates backup and restore. It delegates database details to `graphql-orm` and delegates application object loading to a `BackupObjectIndex` adapter.

## Trust Model

The backup repository is storage, not a trust anchor. Anyone with write access
to it can replace payloads and recompute plain SHA-256 digests, so digests and
the manifest `checksum` detect corruption only. Trust comes from a host-held
key and an independently retained repository identity:

- Every manifest is sealed with HMAC-SHA256 under a `ManifestKey` that the host
  supplies at run time and that is never written to the repository. The MAC
  key is derived with HKDF-SHA256 under a fixed label, so the host may derive
  it from key material that has other uses.
- The key travels with the in-process repository handle
  (`BackupRepository::manifest_authenticator`), the same way capacity limits
  do. A handle without one fails closed: no manifest is read, sealed or
  written, and backups refuse to start.
- Every path that reads a manifest verifies its seal or enforces the configured
  legacy trust policy described below before use: restore,
  object restore (which re-checks a caller-supplied manifest), verification,
  compaction, pruning and snapshot deletion. Payload digests are trusted only
  because a verified manifest names them.
- Format 3 seals carry a repository UUID that must match both the host pin and
  the durable identity record. Storage cannot enroll its own UUID as a host
  trust anchor; initial creation is checked against the locally proposed value.
- A repository may serve different bytes on each read, so a digest is checked
  against the bytes that are actually used, not only during verification.
  Restore and compaction hash each table and change payload as it is buffered
  for decoding and refuse a mismatch before decoding it. Object restore streams
  through a digest check that fails at the end of the stream on a mismatch, so
  a conditional create aborts before committing: local storage writes to a
  temporary file, S3 drains the body before its committing request, and SMB
  writes a hidden temporary file that it links at the key only after the
  whole body is flushed. No bundled store makes a refused object readable at
  its key. Before `graphql-orm-storage` 0.11.0, SMB created the object at its
  final key with delete-on-close set, so a concurrent reader of the live
  store could read the complete object until the delete took effect
  ([#475](https://github.com/Dastari/graphql-orm/issues/475)); this crate
  requires 0.11.0 or later.
- A manifest must name the snapshot id of the path it was read from, use a
  supported format version and contain no unknown fields, so a sealed
  manifest cannot be replayed under another snapshot id or extended with
  unsealed data.
- Content keys are derived, not trusted: an object's key must be the content
  address of its declared digest under `objects/sha256/`, and table and change
  payloads must live at their own snapshot's key for their table name.
- Object restore writes live-store keys only after they pass
  `graphql-orm-storage` key validation and the sink's own check.
  `BlobStoreRestoreObjectSink` writes only inside an explicit
  `RestoreKeyScope` and never overwrites existing content.

Format 1 has no seal. `TrustPinned` authenticates its content against an
independently retained checksum; `TrustUnauthenticated` is the only policy that
accepts it without host-held authentication. Snapshot-path, content-key and
restore-target checks still apply.

Format 2 has a host-keyed seal but no repository binding. `AcceptSealedLegacy`
authenticates content with the key, but does not bind it to this repository.
Use it only when that key was never shared across repositories; otherwise use
`TrustPinned` with independently recorded checksums. A format-2 manifest copied
between repositories sharing a key can load, verify and restore under
`AcceptSealedLegacy`. Code cannot detect that provenance, and format 2 cannot
be bound after the fact. Its original MAC is always checked.

Do not compact or prune legacy snapshots based on `AcceptSealedLegacy` unless
their ids are known from host records. A copied snapshot can count toward
retention's `keep_last`. Compaction additionally requires `TrustPinned` for
every format-2 input before producing a format-3 synthetic full, including all
format-2 parents. The resulting seal binds the new artifact, not the historical
provenance of its inputs. Accepted legacy reads need no repository identity
record. See the [migration policy table](../MIGRATION.md).

Residual risks the host owns: a repository writer can still delete snapshots,
withhold the newest ones, or replace an entire repository including its identity
record. New seals bind a manifest to its repository UUID. Hosts must pin that UUID outside
storage with `with_repository_identity` to detect replacement of the record
itself; the identity record is not a trust anchor. Unpinned format 3 operations
fail closed. Keep independent copies, and choose the snapshot id to restore from host
records rather than from the repository listing alone. Sealed manifests are
not encrypted. Previously valid pruned snapshots can be replayed; there is no
host-held revocation or live-snapshot index in this implementation.

## Collaborating Components

- `graphql-orm`: entity metadata, schema hash, row export/import, change journal, restore context.
- `graphql-orm-storage`: stored object metadata and object storage primitives.
- Application: auth, scheduling, admin APIs, audit, and provider configuration.
- `graphql-orm-backup`: manifests, repositories, verification, backup planning, restore orchestration.

## Full Backup Flow

`create_full_backup` implements the current full snapshot flow. Full, incremental
and compaction writers first acquire the repository lock and validate/create the
host-pinned identity before claiming a snapshot, exporting rows or writing payloads.
Manifest publication checks the durable identity again.

1. Read schema snapshot from `GraphqlOrmBackupAdapter`.
2. Export all backup-enabled tables through `GraphqlOrmBackupAdapter`.
3. Derive object references from those exported rows through `BackupObjectIndex::objects_from_full_export`.
4. Serialize table exports as JSON Lines.
5. Compress table exports with zstd, checksum the stored compressed bytes, and write them to the backup repository.
6. Load and checksum each object from `BackupObjectIndex`.
7. Write object blobs by content-addressed key if missing.
8. Build manifest, refusing table names, digests or object keys that restore
   would reject.
9. Set the manifest checksum and seal it with the host key.
10. Write manifest last.

The built-in adapter exports all tables in a PostgreSQL `REPEATABLE READ
READ ONLY` transaction or a SQLite read transaction. SQLite establishes its
snapshot on the first read; concurrent writes subsequently see the usual
journal/WAL locking rules. Schema metadata is host-supplied, so schema migration
must be quiesced during backup. SQL Server has no built-in backup adapter in
this crate: a custom adapter must enable `ALLOW_SNAPSHOT_ISOLATION` and use a
single `SNAPSHOT` transaction covering all table queries (or quiesce writers),
or fail. Read-committed export, including statement-level
`READ_COMMITTED_SNAPSHOT`, does not provide this cross-table guarantee. See
[Microsoft's isolation-level reference](https://learn.microsoft.com/en-us/sql/t-sql/statements/set-transaction-isolation-level-transact-sql?view=sql-server-ver17).

The object set comes from those exact exported metadata rows. Each row must
carry its expected checksum. Copying validates the source stream against that
checksum, even if content-addressed bytes already exist in the repository.
A missing source or a checksum mismatch in the bytes read at copy time fails
before manifest publication. This is a logical snapshot of the rows and their referenced bytes, not an atomic
transaction across the database and blob provider. It relies on hosts recording
checksums with metadata and retaining the referenced bytes; a mutation to
unreferenced live objects is irrelevant.

PostgreSQL reads use `SET LOCAL row_security = off`: the server errors when
RLS would filter a table, taking account of ownership, `FORCE ROW LEVEL
SECURITY`, superuser and `BYPASSRLS`. This setting does not bypass RLS. Restore
emptiness checks and search rebuild follow the same fail-closed rule. No
policy-filtered export mode is provided. This follows
[PostgreSQL's backup guidance](https://www.postgresql.org/docs/17/ddl-rowsecurity.html).

## Incremental Backup Flow

`create_incremental_backup` implements the repository-side incremental snapshot flow. It depends on
the application adapter, or a future change-journal extension to
`OrmBackupAdapter`, returning reliable changes from
`GraphqlOrmBackupAdapter::export_incremental`. The export must include sufficient
object metadata for every new or changed reference, captured within the same
snapshot (including unchanged metadata rows if needed to describe a new
reference). Custom adapters must provide a reliable parent checkpoint and one
consistent change set, or fail. The built-in ORM adapter still has no incremental
journal integration and returns `UnsupportedOperation`. Copy-time object checksums
apply exactly as for full backups.

1. Validate/create the pinned repository identity under the lock, then authenticate the parent chain.
2. Ask the adapter for changed rows and deletes since the parent in one database snapshot.
3. Derive object references and recorded digests from those exact exported changes through `objects_from_incremental_export`. Its default fails closed; a second live metadata query is not permitted.
4. Serialize changes as JSON Lines.
5. Compress change payloads with zstd, checksum the stored compressed bytes, and write them.
6. Write object blobs by content-addressed key if missing.
7. Emit delete tombstones for `BackupChangeAction::Delete`.
8. Write an incremental manifest with `parent_snapshot_id`.

## Restore Flow

`restore_snapshot` implements database restore orchestration.

1. Load selected manifest, binding it to its path.
2. Load and validate parent manifest chain.
3. Verify each manifest's seal, checksum and entry namespaces.
4. Verify object, table, and change payload checksums.
5. Compare the root manifest's database backend and schema hash with the
   target adapter.
6. Confirm target database is empty for `RestoreMode::EmptyDatabase`.
7. Download, decompress, and parse full table payloads.
8. Call `GraphqlOrmBackupAdapter::restore_full` with the source manifest
   schema.
9. Download, decompress, and parse incremental change payloads in chain order.
10. Call `GraphqlOrmBackupAdapter::restore_incremental` for each incremental manifest.

`RestoreMode::DryRun` performs steps 1-5 and payload parsing without calling restore methods.
Stored object rehydration is explicit through `restore_objects` and a caller-supplied
`RestoreObjectSink`. It authenticates the manifest again, then checks every
object's live-store key before restoring any object.

## Compaction Flow

`compact_chain` folds a full-plus-incremental chain into a `SyntheticFull` snapshot.

1. Load and authenticate the source manifest chain.
2. Parse the full table payloads into in-memory row maps keyed by primary key.
3. Apply incremental create/update/delete records in chain order.
4. Write the resulting tables as compressed JSON Lines under the new snapshot id.
5. Carry forward non-tombstoned object entries.
6. Write a new sealed synthetic-full manifest.

## Operational Safety

- `create_full_backup`, `create_incremental_backup`, `compact_chain`, `prune`,
  `delete_snapshot`, `restore_snapshot`, `restore_objects` and verification acquire one exclusive
  repository lock, so no operation deletes blobs another is reading. The lock blob holds a
  `RepositoryLockOwner` record; recovery from a crashed holder is the explicit, owner- and
  age-checked `break_repository_lock`, never automatic expiry.
- Snapshot writers refuse a used snapshot id and, for incrementals, an invalid or incompatible
  parent chain before any write, and enforce the restore-side `BackupCapacityLimits` while
  writing. A failed or cancelled run removes its payloads unless its manifest write was
  attempted.
- Object blob writes and checksum verification use bounded concurrency with configurable limits.
- `prune` retains manifest chains selected by `KeepPolicy` (at least one) and removes expired
  snapshot blobs, orphaned snapshot payloads without a manifest, and unreferenced
  content-addressed object blobs.
- `prune` and `delete_snapshot` fail closed on an incomplete snapshot listing: a stored but
  unlisted parent, or a different manifest set on re-listing, aborts before any snapshot or
  object is deleted, and the manifest set is checked again before the object sweep.
