---
title: "Migration Guide"
kind: reference
status: active
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-11-12
review_by: 2027-02-01
supersedes: []
---

# Migration Guide

## 0.16.0 to 0.17.0: snapshot consistency and repository identity

1. Implement `BackupObjectIndex::objects_from_full_export` for custom indexes.
   Derive the object set solely from its supplied rows. An objectless or
   externally frozen index may return its fixed set explicitly; querying a
   live metadata table is not valid. `export_full` must export all tables in
   one consistent snapshot or fail. The ORM index requires a valid recorded
   checksum for every exported object; backfill checksums before backup and
   include those columns in backup policy. Implement the analogous
   `objects_from_incremental_export(parent_snapshot_id, changes)` method for
   incremental indexes. It must derive references and recorded digests solely
   from that exact exported change set; its default also fails closed. Export
   any required object metadata rows within the same database snapshot.
2. Handle `BackupError::IncompleteRepositoryListing { reason }` in exhaustive
   matches. Existing listing safety checks now return this variant rather than
   `InvalidManifestChain`; do not match error strings.
3. New writes use format 3; older readers cannot read them. `ManifestMac`
   gains `repository_id`, so update struct literals. New seals require the
   repository UUID; `None` represents an older unbound seal only. Call
   `initialize_repository_identity` on an
   authenticated repository, record the returned UUID outside storage, then
   configure `ManifestAuthenticator::with_repository_identity` with that pin.
   This pin is mandatory for format 3 sealing, verification and restore;
   unpinned handles fail closed before a new backup exports or writes payloads.
   Initialization uses atomic conditional creation and never replaces a record.
   Initial enrollment without a pin must create a new record. An existing
   record is refused until the host supplies its independently retained UUID;
   the helper never adopts an identity from storage. It also refuses a record
   replaced between atomic creation and its readback. Alternatively generate and
   retain a UUID in host configuration first; a new backup run
   stores that configured UUID atomically after acquiring the lock.
   Preserve the record when moving the entire repository. A different repository
   must keep its own record and unique host pin; never reuse an identity across
   independent repositories. Copied format-3 manifests from a different repository identity will be refused.
4. Existing backups are not rewritten. Format 2 cannot be bound to a
   repository after the fact: its seal authenticates content with the key, but
   does not bind it to this repository. `AcceptSealedLegacy` accepts valid
   format-2 seals and refuses format 1. Use it only when the key was never
   shared across repositories. Otherwise use `TrustPinned` with the checksums
   you recorded independently; a copied format-2 manifest sealed with a shared
   key is indistinguishable from this repository's own snapshots.

   `TrustPinned` accepts format 1 and format 2 only when each snapshot's
   checksum matches its host-held record. That independent checksum authenticates
   format-1 content even though it has no seal. Format 2 must also pass its
   original seal verification. A pin supplied by repository contents is not a
   host-held record.

   Choose legacy snapshot ids from host records. Do not compact or prune legacy
   snapshots on the strength of `AcceptSealedLegacy` unless those ids are known
   from host records: copied snapshots can affect retention's `keep_last`.
   As an additional safeguard, compaction refuses every format-2 input unless
   individually authenticated by `TrustPinned`, including every format-2 parent
   in a mixed chain. Switch to that policy with all required checksum pins before
   compacting. This permits explicit migration without silently promoting unbound
   content to repository-bound trust.

   A host without historical checksum records can still restore with its
   exclusive old key. If that key was never shared and the snapshot ids are
   known from host records, verify those sealed snapshots first, retain their
   checksums outside the repository, then configure `TrustPinned` to compact.
   Never enroll pins this way when the key was shared or the ids are unknown;
   the repository cannot establish historical provenance in that case.

   **Warning:** `TrustUnauthenticated` is the only policy that accepts format 1
   without independent host-held authentication. A repository writer can forge
   that content and recompute its plain checksum. Prefer `TrustPinned` when
   historical checksum records exist. Do not enable unauthenticated trust to
   migrate sealed format-2 backups.

   The default `Refuse` policy rejects both older formats. Operation requirements
   for existing legacy repositories are:

   | Operation | `Refuse` | `AcceptSealedLegacy` | `TrustPinned` |
   | --- | --- | --- | --- |
   | Restore, object restore, load or verify format 1 | Refused | Refused | Requires that snapshot's independent checksum pin |
   | Restore, object restore, load or verify format 2 | Refused | Requires the old sealing key; still unbound | Requires the old sealing key and that snapshot's checksum pin |
   | Incremental from a chain containing format 2 | Refused | Works with the old sealing key; select parents from host records | Requires pins for every legacy chain member |
   | Compact a chain containing format 2 | Refused | Refused | Requires pins for every legacy chain member |
   | Prune or delete while any listed manifest is legacy | Refused, even if selecting format 3 | Format 2 only; use only with ids known from host records | Requires pins for every listed legacy manifest |
   | New full backup or an independent all-format-3 chain | Works with a host UUID pin | Works with a host UUID pin | Works with a host UUID pin |

   Legacy reads require no identity record or UUID pin. Every format-3 member of
   a mixed chain or repository still requires the host UUID pin and matching
   durable record, regardless of legacy policy. New incremental and compaction
   writes require a host UUID pin and validate/create its durable record under
   the lock before export or payload writes.

   Compaction creates a repository-bound format-3 snapshot; it does not rewrite
   or remove old manifests, or retroactively prove their repository of origin.
   Keep an appropriate legacy policy while old manifests remain, or remove them
   under that policy before switching back to `Refuse`.
5. PostgreSQL roles subject to RLS now fail export, target emptiness checks,
   and search rebuild. Use an administrative role exempt from RLS: a superuser,
   a role with `BYPASSRLS`, or the table owner when RLS is not forced. A permissive
   policy is still RLS and does not qualify. There is no policy-filtered export
   option. ORM and macros move together to
   placeholder 0.136.0 for this change; refresh consumer lockfiles together.

See the [snapshot guarantees](docs/architecture.md#full-backup-flow) and
[format reference](docs/snapshot-format.md#manifest-authentication).

## 0.15.0 to 0.16.0: storage 0.11.0

Update the backup requirement to 0.16.0 and storage to 0.11.0 together, then
refresh the consumer lockfile. Repository layout, manifest format and the
public API are unchanged.

- Follow the storage [0.11.0 migration](../graphql-orm-storage/MIGRATION.md):
  remove `require_signing = false` from SMB configurations. The SMB share must
  support hard links, which Samba and NTFS shares do.
- An SMB content write whose publishing link has an unknown outcome now fails
  with a non-retryable error; treat it like any other failed backup and
  verify the repository before relying on it.
- When the outcome is unknown while acquiring the repository lock, the lock
  may hold this run's record, and nothing releases it: the operation fails
  without a lock to release. The next operation then fails with
  `BackupError::RepositoryLocked` naming this run. Confirm that the run and
  every other writer have stopped, inspect the lock, and remove it with
  `break_repository_lock` for that run. This could already happen before this
  release, when a `FILE_CREATE` response was lost.

## 0.14.0 to 0.15.0: storage 0.10.0

Update the backup requirement to 0.15.0 and storage to 0.10.0 together, then
refresh the consumer lockfile. Repository layout, manifest format and the
public API are unchanged.

- Follow the storage [0.10.0 migration](../graphql-orm-storage/MIGRATION.md):
  SMB repositories on servers that cannot negotiate SMB 3.1.1 must set
  `min_dialect` explicitly, and storage keys under `.graphql-orm-storage` are
  reserved. Keep a repository prefix outside that namespace.
- A custom `BackupRepository` should treat `list_blobs` prefixes as string
  prefixes, as storage does. Built-in callers filter results to the exact
  directory before deleting anything.

## 0.13.0 to 0.14.0: lock records, locked reads and write-side limits

Storage stays on its current version. Repository layout and manifest format
are unchanged; existing snapshots restore as before.

1. **Construct the new fields.** `RepositoryLockOptions` gains `owner_label`,
   `BackupExecutionOptions` gains `cancellation`, `VerificationOptions` gains
   `lock`, `lock_mode` and `cancellation`, `BackupCapacityLimits` gains
   `object_bytes` and `PruneResult` gains `deleted_orphan_blobs`. Struct
   literals need `..Default::default()` or the new fields. `BackupError::RepositoryLocked` gains an `owner` field, so
   patterns must use `{ lock_key, .. }`.
2. **Handle the new errors.** Match `LockNotReleased`, `SnapshotExists`,
   `InvalidRetentionPolicy`, `BackupCapacityExceeded`, `Cancelled`,
   `LockReleaseFailed`, `RepositoryLockNotHeld` and `BreakLockRefused`.
   Compaction onto a used id now reports `SnapshotExists` instead of
   `InvalidManifestChain`. A `LockReleaseFailed` after a backup means the
   snapshot named in `committed_snapshot_id` is complete but the repository
   is still locked.
3. **Use a fresh id per snapshot.** Backups no longer overwrite an id that has
   a manifest or payloads. Do not retry with the same id after an error whose
   outcome is unknown; use a new id and let `prune` remove the leftovers.
4. **Keep at least one snapshot.** `KeepPolicy { keep_last: 0 }` is refused.
   Delete a repository's last snapshot explicitly with `delete_snapshot`.
5. **Serialize restores and verification with writers.** Restore and
   verification now take the exclusive repository lock and fail with
   `RepositoryLocked` while a backup, prune or another restore holds it.
   Schedule them so they do not overlap, or retry after the holder finishes.
   Custom `BackupRepository` wrappers used for restore or verification must
   now forward `put_blob_if_absent` and `delete_blob_once`, because those
   operations acquire and release the lock.
6. **Read-only repositories need the explicit opt-in.** Read credentials are
   no longer enough by default, because taking the lock writes
   `locks/repository.lock`. For read-only credentials, replicas or mounts,
   call `restore_snapshot_with_options` and `restore_objects_with_options`
   with `RestoreOptions { lock_mode: RepositoryLockMode::AssumeNoConcurrentWriters, .. }`,
   and set `VerificationOptions::lock_mode` the same way. Only do this when no
   prune, `delete_snapshot` or `compact_chain` can run against the repository
   meanwhile: one that does can delete blobs mid-read and leave a partly
   applied restore target.
7. **Cancel instead of dropping.** Wrap host-side timeouts around a
   `BackupCancellation` (`BackupExecutionOptions`, `RestoreOptions` or
   `VerificationOptions`) and call `cancel()`. The operation then stops at a
   safe point and releases the lock, or returns `LockNotReleased` naming the
   run whose lock it could not release. Dropping a backup, restore or
   verification future leaves the lock in place until it is broken.
8. **Do not rewrite manifests.** `write_manifest` now refuses an existing
   snapshot id with `SnapshotExists`. Write a new snapshot instead; tooling
   that deliberately replaces a manifest must delete it first, with every
   other operation stopped.
9. **Keep owner labels short.** `RepositoryLockOptions::owner_label` is
   limited to 1024 bytes without control characters.
10. **Check limits before upgrading large repositories.** Backups now refuse
   payloads that restore with the same handle would refuse. If a backup
   fails with `BackupCapacityExceeded`, raise the reported limit on the
   repository handle used for both backup and restore after reviewing the
   memory budget.
11. **Recover stuck locks explicitly.** A crashed or dropped run leaves
   `locks/repository.lock` with an owner record. Read it with
   `read_repository_lock`, confirm that run and every other writer have
   stopped, then call `break_repository_lock` with its `run_id` and a minimum
   age. Locks written by 0.13.0 or earlier have no owner record and still need
   offline removal after quiescing every writer. Prefer
   `BackupCancellation` over dropping a backup future: it releases the lock.

## 0.12.1 to 0.13.0: authenticated manifests and constrained restore targets

This is a breaking change. Read the
[trust model](docs/architecture.md#trust-model) first. Storage stays on its
current version.

1. **Provision a manifest key.** Generate at least 32 random bytes and keep
   them in the host's secret store, separate from the backup repository and
   from anyone who can write it. Use one key per repository or environment.
   Losing the key makes the repository's manifests unverifiable.
2. **Attach it to every repository handle** that backs up, restores,
   verifies, compacts or prunes:

   ```rust,ignore
   let authenticator = ManifestAuthenticator::new(ManifestKey::from_bytes(&key_material)?);
   let repository = LocalBackupRepository::new(root).with_manifest_authenticator(authenticator);
   // or BlobStoreBackupRepository::new(store).with_manifest_authenticator(authenticator)
   ```

   Custom `BackupRepository` implementations, including wrappers, must
   implement `manifest_authenticator`. The default returns `None`, which fails
   closed with `ManifestAuthenticationRequired`.
3. **Constrain object restore.** `BlobStoreRestoreObjectSink::new(store, scope)`
   now needs a `RestoreKeyScope`. Use `RestoreKeyScope::prefixes([...])` for
   the live-store namespaces your objects use, or
   `RestoreKeyScope::entire_store()` only for a store that holds nothing but
   restored objects. The sink no longer overwrites: restore into an empty
   staging namespace, and treat `RestoreObjectConflict` as a target that
   already holds different data. The live store must support atomic
   conditional create; the bundled local, S3 and SMB backends do.
   Custom sinks may override `RestoreObjectSink::check_restore_target`.
4. **Migrate existing snapshots.** Snapshots written by 0.12.x and earlier are
   format 1 and are refused with `LegacyManifestRefused`. While you still trust
   the repository:
   - Record each snapshot you need with its manifest `checksum` (the
     `checksum` field of `snapshots/{id}/manifest.json`), ideally from a copy
     taken before upgrading, and configure
     `LegacyManifestPolicy::TrustPinned` with that map. Prefer this to
     `TrustUnauthenticated`, which accepts any well-formed legacy manifest,
     including one a repository writer adds later.
   - Take a new full backup, or `compact_chain` each chain you need into a
     sealed synthetic full.
   - Prune or delete the legacy snapshots under the same policy.
   - Remove the legacy policy. `prune` and `delete_snapshot` refuse to run
     while any listed manifest fails authentication.
5. **Update error handling** for the new `BackupError` variants and the
   `BackupSnapshotManifest::authentication` field. Code that builds manifests
   by hand must use `BACKUP_FORMAT_VERSION` (now 2), canonical content keys
   from `database_table_key`, `database_changes_key` and `object_content_key`,
   and lowercase SHA-256 digests, then seal with `write_manifest` or
   `authenticate_manifest`.
6. **Check custom sinks and compaction ids.** A custom `RestoreObjectSink`
   must read each object stream to its end and must discard a write whose
   stream ends in an error. `restore_objects` now reports a digest mismatch
   that way, and a sink that stops reading early fails with
   `ChecksumMismatch`. `compact_chain` refuses a `snapshot_id` that already
   exists; pass a new id.
7. **Check backup inputs.** Backups now refuse table names that are not a
   single safe key segment, non-hex digests, and object storage keys that fail
   `graphql-orm-storage` key validation, before writing anything.

Stored payloads and the repository layout are unchanged. Earlier releases
ignore the new `authentication` field, so they can still read 0.13.0
manifests without verifying them; do not keep older releases pointed at a
repository you rely on.

## 0.12.0 to 0.12.1: fail-closed pruning (GHSA-439h-2cx8-xwgx)

Update backup to 0.12.1 and storage to 0.8.1 from one reviewed workspace
revision, then refresh the consumer lockfile. No API, manifest or data
migration is required. `prune` and `delete_snapshot` may now return
`InvalidManifestChain` with "snapshot listing is incomplete". When the check
before deletion fails, nothing has been deleted. When the check before the
object sweep fails, expired snapshots may have been deleted but no object has.
Find out why the listing was incomplete before retrying, and do not work around
it. A dedicated error variant is planned for the next minor release.

## 0.11.1 to 0.12.0: optional ORM public type identity

Upgrade optional ORM adapters and any direct ORM consumer together to ORM
0.80.0/catalog 0.6.0 from one reviewed workspace revision. Concrete public
`Database`, entity metadata and `ColumnBackupPolicy` types must share one source
universe. Storage stays on its existing version. No backup/restore logic,
schema, snapshot manifest or repository-format change; no data migration is
required. This identity alignment does not authorize a backup or restore run.

## 0.11.0 to 0.11.1: package release identity

Update the backup requirement to 0.11.1 and refresh the consumer lockfile when
adopting the reviewed workspace revision. This patch gives the snapshot-format
documentation added after the immutable 0.11.0 tag its own package identity.
There are no new breaking changes, API/SDL changes, data migrations or runtime
changes in this patch. ORM/macros remain at 0.78.0; their existing migration
requirements still apply when adopting the complete workspace revision.

## 0.10.0 to 0.11.0: composite-key tables

Adopt together with `graphql-orm` 0.71.0 from one reviewed revision. Snapshot
format and stored data are unchanged.

- `compact_chain` now fails with `BackupError::InvalidManifestChain` for a
  chain whose incremental changes touch a composite-key table. The 0.71.0
  change journal refuses composite-key events, so chains produced by the ORM do
  not contain such changes. A host that writes its own change exports for a
  composite-key table must take full snapshots of it instead.
- Schemas with a static composite-key entity get a new schema hash in 0.71.0,
  so their earlier backups compare as a different schema. Restore them with the
  previous release, then take a new full backup.
- Restores refuse orphan references and occupied `EmptyDatabase` targets
  before writing. Treat these as validation failures, not partial restores.

## 0.9.0 to 0.10.0: explicit buffer and decompression limits

This is a breaking pre-1.0 capacity-policy change. Previously accepted large
buffered payloads can now fail with `BackupError::CapacityExceeded`; snapshot
format and stored data are unchanged. Update exhaustive error matches.

Defaults are 64 MiB for each buffered repository body or compatibility stream
fallback, 256 MiB for each decoded database payload, and 512 MiB of decoded
payloads across one full/incremental manifest stage. Limits are inclusive.
Stored payloads are limited before decoding; compressed expansion is checked
incrementally before appending output. Before reading frame input, a finite
output cap also sets zstd's maximum history-window size: round down to the next
power of two, with a 1 KiB minimum and a portable 1 GiB maximum. This may reject
otherwise small output encoded with a larger window; the error is `Compression`.
An unbounded override retains the zstd library's default window policy. Decoder
metadata and other overhead still require a separate resource budget.

Each payload's output limit is further clamped to the remaining stage budget,
and aggregate decoded limits are checked before parsing each next payload. They do not limit the whole chain or row count.

For legitimate larger backups, set reviewed finite `BackupCapacityLimits` via
`LocalBackupRepository::with_capacity_limits` or
`BlobStoreBackupRepository::with_capacity_limits`; custom repositories override
`BackupRepository::capacity_limits`. Indexes and sinks override
`buffered_byte_limit` for their buffered compatibility defaults. The built-in
optional `OrmBackupObjectIndex` also defaults its direct buffered
`load_object` to 64 MiB; configure it with `with_buffered_byte_limit`. Missing or
invalid checksum/size metadata is backfilled by incremental hashing and checked
byte counting, without collecting the object or reserving a provider size hint.
Its native `load_object_stream` bypasses that buffered cap so large object streams
remain supported. Recorded sizes cannot constrain buffered allocation; actual
buffered byte checks apply regardless of recorded size. New and deduplicated
backup entries record the byte count observed alongside SHA-256 verification;
compaction normalizes output sizes from verified stored content as well.
Historical manifest `size_bytes` remains advisory for compatibility: valid
content is not rejected solely for a stale size. Object restore performs a
streaming checksum/count preflight and supplies the observed verified size to
the sink, then streams again with the existing checksum guard. This adds a read
pass without whole-object buffering or a transaction guarantee. Existing
historical manifests are not rewritten. Changes between preflight and sink
streaming remain subject to final checksum verification; use staging namespaces
and the existing partial-object recovery rules.
Direct zstd callers use `decompress_payload_with_limit`. `BackupByteLimit::Unbounded` is an
explicit opt-in accepting the associated allocation risk; it is not recommended
for untrusted input. Do not retry capacity errors without changing the relevant
limit or input. Restore errors retain the Q003 stage/progress boundary.

Native streaming implementations bypass compatibility collection. Local repository
stream methods now delegate directly to the storage stream API. This is not a
bounded-RSS guarantee: a custom buffered provider or index may allocate before
returning; a stream's current chunk already exists; parsed JSON, manifests,
adapter exports, accumulated restored/compacted rows and application state may
use additional memory. Streaming source metadata is only an early rejection
hint; actual collected bytes are checked independently. Audit native provider
and adapter behavior and apply process resource limits for hostile input.

## 0.8.0 to 0.9.0: restore failure progress

This is a breaking pre-1.0 error-contract change. `restore_snapshot` returns
`BackupError::RestoreStageFailed { progress, source }` for failures, including
validation and schema/empty-target failures. Update exhaustive `BackupError`
matches and inspect `source` for the original error category, such as
`RestoreSchemaMismatch` or `RestoreTargetNotEmpty`. Direct adapter calls and
`ensure_empty_restore_target` retain their own error boundaries.

`RestoreFailureProgress` identifies the requested snapshot, the failing manifest
when known, the phase and the manifests whose adapter apply calls reported
success. `Validation` runs before apply calls; `Preparation` does not apply the
failing stage, but earlier completed stages can remain. `Apply` has an unknown
outcome and may be partial even when no earlier stage completed. The primary
display contains only phase/progress; the original cause remains available for
trusted diagnostics through the error source chain.

`target_may_have_changed()` describes changes by this attempt, not data that
already existed in the target. Dry run never records completed apply stages or
calls adapter apply methods. Backend/schema validation still occurs before
checking target emptiness or applying rows.

Progress is not permission to resume from the next manifest or retry in place.
Orchestration does not roll back earlier stages or provide a transaction across
the chain. Restore into a disposable empty staging database; if an apply fails,
or later preparation fails after earlier stages applied, quarantine/discard that
staging target through its owner and restart from a fresh empty target after
resolving the failure. An empty-target retry rejects a partially populated target. Cancellation or
dropping the future cannot return progress or roll back an active adapter call;
treat cancellation during apply as an unknown outcome requiring the same recovery.

Object restore is a separate operation and consistency boundary. Earlier objects
may remain after failure, and custom sinks may partially write a failing object;
checksum failure can be reported after writes. Use an isolated staging object
namespace and publish the database/object set only after every required restore
and verification step succeeds. There is no cross-store rollback guarantee.

The snapshot format and stored data need no migration. Keep workspace packages
on one reviewed revision and regenerate `Cargo.lock`.

## 0.7.2 to 0.8.0: fail-closed writer locks

This is a breaking pre-1.0 release. Upgrade all writers sharing a repository
at once; older clients can still delete active locks through stale takeover.
Do not run mixed old/new clients against the same repository.

`RepositoryLock` now borrows its acquisition repository and cannot be cloned
or compared. Replace `lock.release(&repository).await` with
`lock.release().await`. Public signatures naming the handle require its
lifetime, for example `RepositoryLock<'_>`.

Automatic stale takeover is disabled. `RepositoryLockOptions::stale_after_seconds`
and its default constant remain available for source compatibility but have no
effect: old, malformed, and unreadable existing locks all block acquisition.
A slow writer retains exclusion indefinitely. A crashed, cancelled, or timed-out
operation may leave a lock; there is no automatic Drop release or lease expiry.
For deliberate offline recovery, stop and quiesce **all** repository writers,
including older processes, pending tasks and in-flight provider requests, and
ensure no lock handles remain.
Only then remove `locks/repository.lock` using provider administration, and
restart upgraded writers. An ambiguous remote failure requires establishing that
its request has finished before recovery. Never clear a lock based on elapsed
time alone.
Deleting or replacing lock blobs during live operation invalidates exclusion;
this design does not provide fencing against administrative storage mutation.

Custom `BackupRepository` implementations must override `put_blob_if_absent`
with atomic create-if-absent across all writers. The inherited default now
returns `BackupError::UnsupportedOperation` without checking or writing storage;
its streaming default delegates to that fail-closed method. An existence check
followed by a write does not satisfy this contract. Local, SMB and S3 adapters
retain their native atomic conditional creation.

Storage advances independently to 0.7.0 so custom `BlobStore` providers also
fail closed when inheriting conditional creation. Override that method atomically
when using `BlobStoreBackupRepository`; backup maps its unsupported-operation
error to `BackupError::UnsupportedOperation`. Update exhaustive storage error
matches for the new variant.

Lock release now requires explicit `BackupRepository::delete_blob_once` and,
for storage adapters, `BlobStore::delete_blob_once`. Both default to unsupported
without I/O. Custom providers must implement a single-attempt deletion that never
replays by key after a mutation may have reached the server, including after
cancellation or a lost response. Do not delegate to an ordinary delete method
that may retry. Acquisition can succeed while release is unsupported; implement
and verify both capabilities before enabling a custom repository for writers.
Release consumes the handle on success or failure. Failed/cancelled release has
an unknown outcome: never retry by key or reclaim while any request can remain.
Native local release issues one filesystem removal. Native S3 disables SDK
operation retries for lock release. Native SMB never reopens the path after a
possibly applied disposition/close. Ordinary blob deletion retains its existing
provider behavior.
SMB conditional-create failures and cancellation close only the original handle;
there is no shared-key fallback cleanup after that handle closes. Ambiguous
cleanup/disarm may leave an orphan lock until offline recovery.

The snapshot format, repository layout and restore preflight are unchanged. No snapshot/data migration is required. Regenerate `Cargo.lock`
after updating the reviewed monorepo revision.


### Single-attempt error classification

`StorageError::SingleAttemptFailed` preserves the underlying failure in its
source chain and always reports `is_retryable() == false`. Update exhaustive
storage error matches for this added variant as well as `UnsupportedOperation`.
Native single-attempt deletion and the backup adapter never advertise retrying
by key, even when the underlying ordinary I/O/provider failure was retryable.
Non-retryable here does **not** mean the operation had no effect: a request can
have succeeded remotely while its response was lost. Establish quiescence
before offline recovery; never infer that the lock is safe to reclaim from this
flag. Ordinary deletion retains its prior error taxonomy.

## 0.7.1 to 0.7.2

Version 0.7.2 refreshes maintained workspace guidance and establishes a new
immutable package tag for the changed package source tree. The public API,
snapshot manifest, repository-key layout, restore preflight, persisted data,
and feature defaults are unchanged. No schema or data migration is required.
Git consumers should move every selected monorepo package to the same reviewed
full revision and regenerate `Cargo.lock`.

## 0.7.0 to 0.7.1

Version 0.7.1 publishes the maintained documentation and examples and aligns
the workspace with `graphql-orm` 0.22.0 and `graphql-orm-storage` 0.6.1. The
public API, snapshot manifest, repository-key layout, restore preflight, and
persisted data are unchanged. No data migration is required; Git consumers
should update every selected monorepo package to the same reviewed full
revision and regenerate `Cargo.lock`.

## 0.6.0 to 0.7.0: repository consolidation

The source repository is now
`https://github.com/Dastari/graphql-orm.git`. Git consumers should use that URL
for backup, ORM, storage, and AI packages and pin every selected package to one
reviewed full monorepo revision.

The consolidated workspace advances this crate to 0.7.0 and aligns it with
`graphql-orm-storage` 0.6.0 while retaining `graphql-orm` 0.16.0. This is a
pre-1.0 dependency type-universe boundary because public backup contracts use
storage types. It does not otherwise change the backup API, snapshot format,
repository key layout, restore preflight, or persisted data. Regenerate
`Cargo.lock`, verify that the old backup and storage repository URLs are
absent, and rerun the selected repository and ORM conformance checks.

Lock acquisition also closes a provider visibility race: after an atomic
conditional-create collision, a lock that cannot yet be read is treated as
active rather than returning the provider read error or attempting stale-lock
removal. Callers may retry the complete operation later; no data migration is
required.

## 0.5.x to 0.6.0

Version 0.6.0 moves the optional ORM adapter to `graphql-orm` 0.16.0 at
reviewed commit `dd68a001f47f04178bf3389dd47ee952faa6ecf0`. Applications that
use `graphql-orm` directly must use the same canonical Git URL and full
revision so public ORM types resolve from one source.

The reviewed ORM revision owns an optional `auth-agql` bridge that pins
`agql-auth` 0.12.0 at
`3f3b0c5365adfbe436514a681d977b600991b797`. This backup crate does not enable
that feature or depend directly on `agql-auth`: backup and restore
authorization remains a host responsibility. Hosts that enable
`graphql-orm/auth-agql` should retain the ORM-owned exact auth revision.

```toml
graphql-orm-backup = {
    git = "https://github.com/Dastari/graphql-orm.git",
    rev = "<reviewed-full-40-character-commit-sha>",
    version = "0.6.0",
    default-features = false,
    features = ["local", "orm-sqlite"]
}
```

Use `orm-postgres` instead for PostgreSQL. This dependency/type-universe
alignment does not change the public backup API, manifest format, repository
key layout, storage revision, or the fail-closed restore preflight introduced
in 0.5.0.

## 0.4.x to 0.5.0

Version 0.5.0 moves the optional ORM adapter to `graphql-orm` 0.15.0 at the
reviewed commit
`6beef53633befd90a4d4810887a3e4640dc4ad91`. Applications must use the same
canonical Git URL and full revision. The reviewed `graphql-orm-storage` 0.5.0
commit remains `f1a1f06483d5fd3a0b8fd17f013b3ad4dd9849c5`.

This is a breaking pre-1.0 release for `orm` consumers because public adapter
signatures and the re-exported `ColumnBackupPolicy` now belong to the 0.15.0
type universe.

### Backend features

Hosts may continue enabling `orm` and selecting exactly one backend through
their direct `graphql-orm` dependency. The following convenience features are
also available:

```toml
graphql-orm-backup = {
    git = "https://github.com/Dastari/graphql-orm-backup.git",
    rev = "<reviewed-full-40-character-commit-sha>",
    version = "0.5.0",
    default-features = false,
    features = ["local", "orm-sqlite"]
}
```

Use `orm-postgres` instead of `orm-sqlite` for PostgreSQL. Never enable both in
one binary.

### Adapter restore signature

Custom `GraphqlOrmBackupAdapter` implementations must accept the source
manifest schema in `restore_full`:

```rust,ignore
async fn restore_full(
    &self,
    backup_schema: GraphqlOrmBackupSchema,
    export: Vec<BackupTableExport>,
    context: RestoreContext,
) -> Result<(), BackupError>;
```

`restore_snapshot` validates `backup_schema` against `schema_snapshot()` before
checking target emptiness or importing rows. Custom adapters should repeat any
backend-specific descriptor validation at their import boundary.

Dry run now performs the same backend/schema-hash preflight. It still never
calls adapter import methods.

### Column policy overrides

`OrmBackupAdapter::with_column_backup_policy` is now fail-closed when
descriptors are resolved:

- an unknown table or column is an error;
- `Include` may become `Redact` or `Exclude`;
- `Redact` may become `Exclude`; and
- no policy may be weakened.

Effective overrides are included in the backup schema hash. The same reviewed
override configuration must therefore be present during backup and restore.

In 0.4.x, adapter overrides were applied after hashing. A 0.4.x snapshot made
with overrides will not pass 0.5.0's exact compatibility check. Restore such a
snapshot with the pinned 0.4.x dependency into an owned staging target, verify
it, and create a new 0.5.0 full snapshot. Do not rewrite a signed manifest hash
by hand.

Sensitive opaque object references that must be preserved should be declared
`Include` in reviewed entity metadata. A restore operator cannot use the
adapter override API to silently include a field declared `Redact` or
`Exclude`.

### Administrative restore context

Applying an `OrmBackupAdapter` restore requires both
`disable_policies = true` and `disable_change_journal = true`. ORM restore uses
the administrative row-import path rather than generated GraphQL/repository
mutations. These flags do not grant authorization, change the database role,
or bypass database-native row-level security. The host must authorize the
operation, select an appropriate restore role, quiesce the application, and
keep runtimes closed until its own reconciliation and readiness checks pass.

The manifest format and repository key layout remain version 1 and are
otherwise unchanged.

## 0.3.x to 0.4.0

The snapshot format and repository key layout are unchanged. Existing local,
S3-backed, and custom repositories can read and write the same snapshots.

### Streaming trait methods

`BackupRepository`, `BackupObjectIndex`, and `RestoreObjectSink` add streaming
methods. They have buffered default implementations, so existing trait
implementations remain source-compatible and do not need immediate changes.

Providers that handle large stored objects should override:

- `BackupRepository::put_blob_stream`
- `BackupRepository::put_blob_stream_if_absent`
- `BackupRepository::get_blob_stream`
- `BackupObjectIndex::load_object_stream`
- `RestoreObjectSink::restore_object_stream`

`BlobStoreBackupRepository` and `BlobStoreRestoreObjectSink` already provide
native streaming overrides. Small manifests and compressed database table
payloads retain their buffered convenience APIs.

### Native SMB repositories

Enable native SMB without the local provider:

```toml
graphql-orm-backup = {
    version = "0.4.0",
    default-features = false,
    features = ["smb"]
}
```

Construct `graphql-orm-storage::SmbStorageBackend`, erase it to
`Arc<dyn BlobStore>`, and pass it to `BlobStoreBackupRepository`. Do not pass a
mount path or UNC string as native SMB configuration. Existing mounted-share
deployments can continue through `LocalBackupRepository`, preferably under an
explicit legacy provider name.

Credentials remain host-owned runtime inputs. No manifest or repository data
migration is required.

### Dependency identity and release order

Storage 0.5.0 must be released or pinned before backup 0.4.0. Applications that
also use `graphql-orm-storage` directly must resolve the same canonical source
and reviewed revision as this crate; otherwise Rust treats the duplicated
`BlobStore` traits as different types.

### Host authorization

No `agql-auth` migration is required. Hosts continue to authorize
configuration, validation, backup, restore, delete, and prune operations and to
provide an internal trusted path for scheduled backups.

## graphql-orm-backup 0.18.0

Test infrastructure only: no API or data migration required. Owned PostgreSQL tests pull digest-pinned images.
