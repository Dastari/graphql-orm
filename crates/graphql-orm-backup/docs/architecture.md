---
title: "graphql-orm-backup Architecture"
kind: architecture
status: active
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-11-11
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
key:

- Every manifest is sealed with HMAC-SHA256 under a `ManifestKey` that the host
  supplies at run time and that is never written to the repository. The MAC
  key is derived with HKDF-SHA256 under a fixed label, so the host may derive
  it from key material that has other uses.
- The key travels with the in-process repository handle
  (`BackupRepository::manifest_authenticator`), the same way capacity limits
  do. A handle without one fails closed: no manifest is read, sealed or
  written, and backups refuse to start.
- Every path that reads a manifest verifies its seal before using it: restore,
  object restore (which re-checks a caller-supplied manifest), verification,
  compaction, pruning and snapshot deletion. Payload digests are trusted only
  because a verified manifest names them.
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

Legacy manifests (format version 1, written before sealing existed) carry no
seal. They are refused unless the host selects a `LegacyManifestPolicy`; the
identity, content-key and restore-target checks still apply to them.

Residual risks the host owns: a repository writer can still delete snapshots,
withhold the newest ones, or copy sealed manifests and payloads between
repositories that share a key. Use one key per repository or environment,
keep independent copies, and choose the snapshot id to restore from host
records rather than from the repository listing alone. Sealed manifests are
not encrypted.

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
8. Build manifest, refusing table names, digests or object keys that restore
   would reject.
9. Set the manifest checksum and seal it with the host key.
10. Write manifest last.

## Incremental Backup Flow

`create_incremental_backup` implements the repository-side incremental snapshot flow. It depends on
the application adapter, or a future change-journal extension to
`OrmBackupAdapter`, returning reliable changes from
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

- `create_full_backup`, `create_incremental_backup`, `compact_chain`, and `prune` acquire the
  repository advisory lock.
- Object blob writes and checksum verification use bounded concurrency with configurable limits.
- `prune` retains manifest chains selected by `KeepPolicy` and removes expired snapshot blobs plus
  unreferenced content-addressed object blobs.
- `prune` and `delete_snapshot` fail closed on an incomplete snapshot listing: a stored but
  unlisted parent, or a different manifest set on re-listing, aborts before any snapshot or
  object is deleted, and the manifest set is checked again before the object sweep.
