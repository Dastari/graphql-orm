---
title: "Changelog"
kind: reference
status: active
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-11-12
review_by: 2027-02-01
supersedes: []
---

# Changelog

## 0.18.0 (unreleased)

Requires `graphql-orm-storage` 0.12.0. See [MIGRATION.md](MIGRATION.md).

- On S3, a repository lock or content object whose conditional create has an
  unknown outcome now fails with a non-retryable error instead of being
  retried inside the SDK, where a retry after a lost success reported the
  writer's own lock as held (storage
  [#657](https://github.com/Dastari/graphql-orm/issues/657)). The usage
  guide tells hosts to run `S3StorageBackend::probe_conditional_create`
  before using an S3 bucket as a repository (storage
  [#478](https://github.com/Dastari/graphql-orm/issues/478)).
- Restore tests cover a sink that returns success before reading an object
  to its end, which fails as unverified, and a change payload swapped after
  verification, which is never applied or compacted. The architecture and
  restore-semantics guides no longer describe SMB as creating restored
  objects at their final key with delete-on-close: since storage 0.11.0 a
  refused object is never readable at its key.
  ([#612](https://github.com/Dastari/graphql-orm/issues/612))
- The `lifecycle_safety` integration test requires the `local` feature, so
  every target builds in the native SMB configuration
  (`--no-default-features --features smb`).
  ([#662](https://github.com/Dastari/graphql-orm/issues/662))

## 0.17.0 (unreleased)

- `AcceptSealedLegacy` authenticates format-2 content without repository binding;
  use it only with keys never shared across repositories and legacy snapshot ids
  known from host records. Compaction requires an individual `TrustPinned`
  checksum for every format-2 input before creating a format-3 synthetic full.
- `TrustPinned` accepts format 1 and format 2 against independent host-held
  checksums; format 2 also requires its original seal. `TrustUnauthenticated`
  alone accepts format 1 without host-held authentication.
- Backup, incremental and compaction validate the repository identity under the
  lock before exporting or writing payloads. Incremental indexes derive object
  references from the exported changes and fail closed without that contract.

- Full backup derives object references from the exported table rows rather
  than a second database snapshot (#474). Custom indexes must implement
  `objects_from_full_export`; its default fails closed. The ORM index requires
  recorded checksums in those rows; it never backfills them from live bytes.
  Existing streaming source checksum checks remain in place.
- PostgreSQL export, restore emptiness checks and search rebuild refuse reads
  subject to row security instead of silently omitting rows (#526).
- `IncompleteRepositoryListing { reason }` distinguishes inconsistent listings
  from invalid manifest chains in pruning and deletion (#552).
- New format 3 seals bind manifests to an atomically created `repository-id`
  record (#610). Restore and verification compare that identity; hosts must pin
  it outside storage. Format 2 restores require explicit legacy trust.

## 0.16.0 (unreleased)

Requires `graphql-orm-storage` 0.11.0. See [MIGRATION.md](MIGRATION.md).

- On native SMB, content objects and locks written with
  `put_blob_if_not_exists` appear only once complete. Two backups that store
  the same content object at once no longer race on a partly written object:
  the second backup found the key taken while the first was still writing
  it, and could fail if it then read the object, or if the first write failed
  and removed it. ([#475](https://github.com/Dastari/graphql-orm/issues/475))
- The SMB guide describes the lock primitive as a non-replacing hard link of
  a complete temporary file instead of `FILE_CREATE`.

## 0.15.0 (unreleased)

Requires `graphql-orm-storage` 0.10.0. See [MIGRATION.md](MIGRATION.md).

- Storage listing prefixes are plain string prefixes on every provider. A
  `BlobStoreBackupRepository` with a prefix now lists only keys below
  `<prefix>/`; listing the repository `repo` previously also returned keys of
  a sibling prefix such as `repo-other/` unchanged, on S3 and SMB already and
  now on local storage too.
- `prune` and `delete_snapshot` delete only keys inside the snapshot's
  `snapshots/{id}/` directory and the content directory `objects/sha256/`. A
  sibling such as `snapshots/{id}-copy/` or `objects/sha256-legacy/`, which a
  string prefix also matches, is left alone instead of being deleted.

## 0.14.0 (unreleased)

Breaking: lock records, locked restore and verification, write-side capacity
limits and stricter snapshot lifecycle checks. See [MIGRATION.md](MIGRATION.md).

- Snapshot lifecycle (#481): `prune` refuses `KeepPolicy { keep_last: 0 }`
  with the new `InvalidRetentionPolicy` error before touching the repository;
  previously it deleted every snapshot. Full and incremental backups now refuse
  a snapshot id that already has a manifest or any payload with the new
  `SnapshotExists` error, under the lock and before any write; previously a
  reused id overwrote the existing snapshot's table payloads. Compaction
  reports the same error instead of `InvalidManifestChain`. Incremental
  backups load and authenticate the parent's whole chain before exporting or
  writing, and refuse a missing parent or one recorded for another
  application, backend or schema hash.
- Orphaned payloads (#481): a backup, incremental backup or compaction that
  fails or is cancelled before its manifest write removes the snapshot
  payloads it wrote. `prune` removes blobs in `snapshots/{id}/` directories
  that have no manifest, reported as `PruneResult::deleted_orphan_blobs`, and
  aborts if such a directory's manifest is stored but was not listed.
- Restore and verification are locked (#480): `restore_snapshot`,
  `restore_objects`, `verify_manifest_and_objects` and
  `verify_object_checksums` (and their `_with_options` forms) hold the
  repository lock while they read, so a concurrent prune, snapshot deletion or
  compaction can no longer delete the blobs they read. They fail with
  `RepositoryLocked` while another operation holds the lock; restore reports
  it as a validation-stage failure. The lock is exclusive, so these operations
  no longer run concurrently with each other or with backups.
- Read credentials are no longer enough by default: taking the lock writes
  `locks/repository.lock`. Read-only repositories opt out explicitly with
  `RepositoryLockMode::AssumeNoConcurrentWriters` in the new `RestoreOptions`
  (`restore_snapshot_with_options`, `restore_objects_with_options`) or in
  `VerificationOptions::lock_mode`. The caller then guarantees that no prune,
  deletion or compaction runs concurrently.
- Restore and verification accept a `BackupCancellation`
  (`RestoreOptions::cancellation`, `VerificationOptions::cancellation`); a
  cancelled run stops at its next read or apply call and releases the lock.
  Dropping a restore or verification future leaves the lock in place.
- Lock owner records and recovery (#477): the lock blob now holds a
  `RepositoryLockOwner` record (random run id, best-effort host, process id,
  operation, optional `RepositoryLockOptions::owner_label`, acquisition time).
  `RepositoryLocked` carries the holder's record. `read_repository_lock`
  inspects the lock and `break_repository_lock` removes a lock left by a
  crashed run only when it names the operator-confirmed run id and is at least
  `min_age_seconds` old; locks without an owner record still need offline
  removal. Nothing expires automatically. `RepositoryLock::release` leaves a
  lock that no longer names its run in place and returns
  `RepositoryLockNotHeld`; this narrows, but cannot close, the window after an
  operator breaks a live run's lock, so break a lock only after stopping every
  writer. `owner_label` is limited to `MAX_LOCK_OWNER_LABEL_BYTES` (1024)
  bytes without control characters and refused with
  `InvalidRepositoryLockOptions` before any write. Owner records ignore
  unknown fields, so later releases can add fields.
- Cancellation (#477): `BackupExecutionOptions::cancellation` takes a
  `BackupCancellation`. Cancelling it stops a backup, incremental backup or
  compaction at its next safe point, lets object uploads already in flight
  finish without starting new ones, removes its payloads, releases the lock
  and returns `Cancelled`. Dropping the future still leaves the lock, now with
  its owner record. An object upload failure likewise lets the other
  in-flight uploads finish before the error is returned.
- `write_manifest` never overwrites: it creates the manifest with an atomic
  conditional write and fails with `SnapshotExists` for an existing id.
  `write_manifest`, `load_manifest` and `load_manifest_chain` take no lock.
- A completed operation whose lock release fails now returns
  `LockReleaseFailed`, naming the run still holding the lock and, for backups
  and compaction, the committed snapshot, instead of the bare release error.
  A failed or cancelled operation whose release also fails returns the new
  `LockNotReleased`, which keeps both errors and names the run whose lock to
  break; previously the release failure was dropped.
- Write-side capacity limits (#476): backups, incremental backups and
  compaction check every payload against the repository handle's
  `BackupCapacityLimits` before writing, and fail with the new
  `BackupCapacityExceeded` error naming the payload and the
  `BackupCapacityLimitKind`. Decoded per-payload and per-stage sizes and
  declared object sizes are checked before anything is written; compressed
  payloads stop at `buffered_bytes` while they are encoded instead of being
  buffered without bound; manifests larger than `buffered_bytes` are refused.
  Previously a backup could succeed and then fail to restore with the same
  limits.
- New `BackupCapacityLimits::object_bytes` per-object limit, unbounded by
  default. Backups enforce it on declared and streamed sizes, aborting the
  conditional create; object restore, verification and compaction refuse
  stored objects over it. Object restore also caps its second, sink-facing
  read at the limit.

## 0.13.0 (unreleased)

Breaking: manifests are authenticated with a host-held key and restore targets
are constrained. See [MIGRATION.md](MIGRATION.md) and the
[trust model](docs/architecture.md#trust-model).

- Security: restore, object restore, verification, compaction, pruning and
  snapshot deletion trusted any manifest whose plain SHA-256 checksum matched.
  Anyone able to write the backup repository could therefore recompute the
  checksum and choose every restored row, and every live-store key that
  `BlobStoreRestoreObjectSink` overwrote. Manifests are now sealed with
  HMAC-SHA256 under an HKDF-derived key from a host-held `ManifestKey`, and
  every manifest read verifies the seal before using the manifest.
- Added `ManifestKey`, `ManifestAuthenticator` (with key rotation through
  `with_previous_key`), `LegacyManifestPolicy`, `ManifestMac`,
  `authenticate_manifest`, `verify_manifest_authenticity` and
  `validate_manifest_entries`. Repository handles carry the authenticator:
  `LocalBackupRepository::with_manifest_authenticator`,
  `BlobStoreBackupRepository::with_manifest_authenticator`, and the new
  `BackupRepository::manifest_authenticator` method, whose default returns
  `None`. Without an authenticator, every manifest read and write fails with
  `ManifestAuthenticationRequired`, and backups refuse before writing.
- Manifest format version 2 adds the `authentication` seal. Readers reject
  unknown fields and unsupported versions, and bind each manifest to its path:
  `load_manifest` refuses a manifest whose snapshot id differs from the
  requested one. Format 1 (legacy) manifests are refused with
  `LegacyManifestRefused` unless the host selects `TrustPinned` or
  `TrustUnauthenticated`.
- Content keys must match their entries: object keys must be the
  `objects/sha256/...` address of their own digest, and table and change keys
  must be their own snapshot's key for a single-segment table name. Digests
  must be lowercase SHA-256 hex. Backups refuse invalid table names, digests
  and object storage keys before writing.
- `restore_objects` authenticates the manifest it is given, then validates
  every live-store key with `graphql-orm-storage` key rules and the new
  `RestoreObjectSink::check_restore_target` before restoring any object.
- `BlobStoreRestoreObjectSink::new` takes a required `RestoreKeyScope`. The
  sink writes only inside it, creates objects with an atomic conditional write,
  leaves identical existing content in place and fails with
  `RestoreObjectConflict` instead of overwriting different content.
- Restore and compaction check each table and change payload's digest on the
  exact bytes they decode, not only during the earlier verification read, so a
  repository that serves different bytes on a later read cannot inject rows or
  have them sealed into a synthetic full.
- `restore_objects` streams each object through a digest check that fails at
  the end of the stream on a mismatch, so the sink's conditional create aborts
  before committing. Buffering sinks no longer receive mismatched bytes, and a
  sink that returns success without reading the whole object fails with
  `ChecksumMismatch`.
- `prune` and `delete_snapshot` refuse a handle without an authenticator
  before acquiring the repository lock, so they write nothing.
- `compact_chain` refuses a `snapshot_id` that already has a manifest, with
  `InvalidManifestChain`, instead of overwriting that snapshot.
- Snapshot listings recognise only canonical lowercase hyphenated snapshot ids
  in manifest keys.
- New `BackupError` variants: `ManifestAuthenticationRequired`,
  `InvalidManifestKey`, `ManifestAuthenticationFailed`,
  `LegacyManifestRefused`, `UnsupportedManifestFormat`,
  `ManifestSnapshotMismatch`, `InvalidManifestEntry`, `RestoreTargetRejected`
  and `RestoreObjectConflict`.
- `verify_manifest_checksum` and `verify_object_checksums` are unchanged in
  purpose but now documented as integrity checks, not authentication.
  `verify_object_checksums` also validates entry namespaces.

## 0.12.1 (unreleased)

- Security ([GHSA-439h-2cx8-xwgx](https://github.com/Dastari/graphql-orm/security/advisories/GHSA-439h-2cx8-xwgx)): `prune` and `delete_snapshot` derive retention,
  dependency checks and object reachability from the snapshot listing, so a
  truncated listing made them delete live snapshots or their objects. Both now
  fail closed with `InvalidManifestChain`:
  - before deleting any snapshot or object, when a listed manifest's parent is
    stored but was not listed, or when re-listing manifests gives a different
    set (for example, a child that a listing hid would otherwise let its parent
    be deleted or expired); and
  - before the object sweep, when manifests re-listed after snapshot deletion
    are not exactly the retained snapshots. Expired snapshots may already be
    deleted at that point; no object is.

  A listing truncated identically on every read cannot be detected here.
  Storage 0.8.1 fixes the SMB cause: forged unsigned status responses and
  listings that skipped an unreadable directory.
- No API, manifest, repository-format or data migration change.

## 0.12.0 (unreleased)

- Align the optional adapter's concrete ORM public types with ORM 0.80.0 and
  canonical operation catalog 0.6.0. No backup/restore, storage, persistence or
  manifest-format changes; no data migration required.

## 0.11.1 (unreleased)

- Repair the package release identity after the snapshot-format reference gained
  ORM DateTime row-value documentation following the immutable 0.11.0 tag.
- Align the package manifest, workspace requirement, lockfile, installation
  example and generated inventory. No new breaking changes: Rust API, SDL,
  snapshot format, persistence, runtime behavior and feature defaults are unchanged.

## 0.11.0 (unreleased)

Companion ORM: `graphql-orm` **0.71.0**.

- Full snapshots back up and restore composite-key tables, including the
  ORM-owned link tables of many-to-many runtime relations. A composite row's
  identity is the canonical JSON array of its key parts, for example
  `["a","1"]`. Single-column identities are unchanged.
- Breaking (behaviour): `compact_chain` refuses a chain whose incremental
  changes touch a composite-key table, with `InvalidManifestChain`. The change
  journal cannot name composite rows, so compacting such changes would
  resurrect or duplicate rows. Manifests carry no key descriptors, so a table
  counts as composite when every row identity in the base snapshot is such an
  array. Back these tables up with full snapshots.
- Breaking (behaviour, through `graphql-orm` 0.71.0): restores with an orphan
  reference, or an `EmptyDatabase` restore into an occupied table, are refused
  before any table is written.

## 0.10.0 - 2026-10-15

- Breaking: compatibility stream buffering and buffered repository reads default
  to 64 MiB per body; database decoding defaults to 256 MiB per payload and
  512 MiB aggregate decoded bytes per manifest stage. Large legitimate inputs
  require reviewed finite overrides or an explicit unbounded opt-in.
- Added typed capacity failures, bounded incremental zstd decoding and native
  local stream delegation. These byte limits do not guarantee bounded RSS for
  buffered providers, parsed data or accumulated application state.
- Optional ORM object-index metadata backfill now hashes/counts native streams
  without whole-blob collection. Its direct buffered loader has a configurable
  finite cap; native object streaming remains supported beyond that cap.
- New/deduplicated and compacted object entries use observed verified byte sizes.
  Historical sizes remain advisory; restore streams a checksum/count preflight
  and normalizes sink metadata without rewriting existing manifests.

## 0.9.0 - 2026-10-15

- Added typed snapshot-restore failure progress for validation, preparation and
  apply phases, preserving the original error as a source with a safe primary
  display. Earlier reported-complete stages and a possibly partial failed apply
  are explicit; progress is not an automatic resume checkpoint.
- Breaking: `restore_snapshot` failures now return `RestoreStageFailed` around
  the original cause. Update error matches and exhaustive `BackupError` matches.
- Documented disposable staging-target recovery and the separate database/object
  restore boundaries. Whole-chain atomicity or rollback is not promised.
- Added later-stage parse and partial-adapter fault regressions, including dry
  run, rejection of retry into a dirty target and recovery into a fresh target.

## 0.8.0 - 2026-10-14

- Fixed S-005 writer exclusion: existing locks never expire automatically,
  preventing stale takeover from removing a running writer's lock.
- Breaking: lock handles borrow their repository, are no longer cloneable or
  comparable, and release through `release()` with no repository argument.
- Lock release requires explicit single-attempt deletion; custom defaults fail
  closed, S3 disables SDK operation retries and SMB avoids reopening a lock path
  after a possibly applied mutation. Ambiguous failures require offline recovery.
- Uses storage 0.7.0 so custom `BlobStore` conditional creation also fails closed.
- Custom repository conditional creation now fails closed unless an atomic
  implementation is supplied. Built-in local, SMB and S3 creation is retained.
- Documented coordinated upgrade and offline lock recovery in the migration
  guide; snapshot formats and restore preflight are unchanged.

## 0.7.2 - 2026-08-31

- Refreshed the package's maintained workspace handoff to the current ORM,
  storage, and authorization revisions. Backup APIs, snapshot manifests,
  repository layout, restore behavior, and feature defaults are unchanged.
- This patch establishes a new immutable package tag because maintained files
  under the package source tree changed after 0.7.1. No schema or data
  migration is required.

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
