---
title: "Usage Guide"
kind: reference
status: active
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-11-12
review_by: 2027-02-01
supersedes: []
---

# Usage Guide

`graphql-orm-backup` is an orchestration crate. It does not connect directly to
an application database or object store. Host applications provide small adapter
implementations, and the crate handles snapshot layout, checksums, repository
writes, restore orchestration, compaction, pruning, and verification.

## Core Concepts

- `BackupRepository`: destination for backup blobs and manifests.
- `BlobStoreBackupRepository`: adapter from `graphql-orm-storage::BlobStore` to
  `BackupRepository`.
- `LocalBackupRepository`: filesystem implementation of `BackupRepository`.
- `GraphqlOrmBackupAdapter`: crate-level database export/import contract;
  `OrmBackupAdapter` bridges it to the reviewed `graphql-orm` runtime.
- `BackupObjectIndex`: application adapter that lists and loads stored objects
  referenced by a snapshot.
- `BackupSnapshotManifest`: durable record of a snapshot's database files,
  object files, checksums, schema hash, and application metadata.
- `ManifestKey` and `ManifestAuthenticator`: host-held key that seals and
  verifies manifests. Every repository handle needs one; see the
  [trust model](architecture.md#trust-model).
- `RestoreContext`: explicit restore mode and safety flags.
- `RestoreObjectSink`: application hook for rehydrating object bytes.
- `KeepPolicy`: retention rule used by `prune`.

## Creating A Full Backup

Full snapshot creation is implemented through `create_full_backup`.

```rust
use graphql_orm_backup::{
    BackupObjectIndex, FullBackupRequest, GraphqlOrmBackupAdapter,
    LocalBackupRepository, ManifestAuthenticator, ManifestKey, create_full_backup,
};
use uuid::Uuid;

async fn run_backup(
    database: &dyn GraphqlOrmBackupAdapter,
    objects: &dyn BackupObjectIndex,
    // At least 32 random bytes from the host's secret store.
    manifest_key_material: &[u8],
) -> Result<(), graphql_orm_backup::BackupError> {
    let key = ManifestKey::from_bytes(manifest_key_material)?;
    let repository = LocalBackupRepository::new("./backups")
        .with_manifest_authenticator(ManifestAuthenticator::new(key));

    let result = create_full_backup(
        &repository,
        database,
        objects,
        FullBackupRequest {
            snapshot_id: Uuid::new_v4(),
            created_at: 1_775_174_400,
            app_id: "example-app".to_string(),
            app_version: "0.1.0".to_string(),
        },
    )
    .await?;

    println!("created snapshot {}", result.manifest.snapshot_id);
    Ok(())
}
```

The function performs these steps:

1. Reads schema metadata from `GraphqlOrmBackupAdapter`.
2. Exports all full-backup table rows through `GraphqlOrmBackupAdapter`.
3. Lists referenced objects through `BackupObjectIndex`.
4. Serializes table exports as JSON Lines.
5. Compresses table payloads with zstd and writes them to the repository.
6. Loads and verifies object bytes.
7. Writes missing object blobs by content-addressed key.
8. Builds and checksums the manifest.
9. Writes the manifest last.

## Database Adapter Responsibilities

`GraphqlOrmBackupAdapter` is intentionally narrow. A host adapter or the
included `OrmBackupAdapter` owns database-specific export/import details.

For full backups, implement:

- `schema_snapshot`: return backend name, migration version, and stable schema
  hash.
- `export_full`: return table exports in the order they should be written.

For restore and incremental backups, implement:

- `restore_target_is_empty`: report whether `RestoreMode::EmptyDatabase` is safe.
- `export_incremental`: return create/update/delete changes since a parent snapshot.
- `restore_full`: validate the supplied source manifest schema and import full
  table exports.
- `restore_incremental`: apply incremental changes in manifest-chain order.

## Object Index Responsibilities

`BackupObjectIndex` lets an application expose externally stored objects without
coupling this crate to a specific object-storage implementation.

For full backups, implement:

- `list_objects_for_full_backup`: return object ids, original storage keys,
  expected SHA-256 hashes, sizes, and optional MIME types.
- `load_object`: return the exact bytes for a listed object.

`create_full_backup` verifies the loaded bytes against the declared SHA-256
before the object is referenced in the manifest.

## graphql-orm Runtime Adapters (`orm` feature)

The optional `orm` feature ships adapters over the `GraphqlOrmBackupRuntime`
implementation on `graphql_orm::db::Database`, so hosts only supply entity
metadata and object-table column names:

```rust
use std::sync::Arc;

use graphql_orm::db::Database;
use graphql_orm::graphql::orm::EntityMetadata;
use graphql_orm_backup::{OrmBackupAdapter, OrmBackupObjectIndex, OrmObjectIndexColumns};
use graphql_orm_storage::BlobStore;

fn adapters(
    database: Arc<Database>,
    entities: Vec<&'static EntityMetadata>,
    store: Arc<dyn BlobStore>,
) -> (OrmBackupAdapter, OrmBackupObjectIndex) {
    let adapter = OrmBackupAdapter::new(database.clone(), entities.clone());
    let objects = OrmBackupObjectIndex::new(
        database,
        entities,
        OrmObjectIndexColumns {
            table_name: "stored_objects".to_string(),
            object_id_column: "object_id".to_string(),
            storage_key_column: "storage_key".to_string(),
            sha256_hex_column: "sha256_hex".to_string(),
            size_bytes_column: "size_bytes".to_string(),
            mime_type_column: Some("mime_type".to_string()),
        },
        store,
    );
    (adapter, objects)
}
```

The entity list must match the list used for migrations so exports and restores
cover every application-owned table.

Dependency-owned private schema modules can be passed without GraphQL roots:

```rust,ignore
let catalog = SchemaModuleCatalog::compose(&[&first_module, &second_module])?;
let adapter = OrmBackupAdapter::new(database, catalog.entities().to_vec());
```

The backup crate consumes the composed entity metadata only. Module restore
hooks, reconciliation, readiness, and application/provider effects remain
host-owned.

`OrmBackupAdapter::current_schema_snapshot` exposes the current schema hash for
manifest compatibility checks, and `OrmBackupAdapter::clear_restore_target`
empties every backup-enabled table so a replace-existing restore can run
through the standard empty-database path. Incremental export and restore return
`UnsupportedOperation` until a change-journal integration lands.

Adapter-level column policy overrides may only strengthen
`Include -> Redact -> Exclude`, and effective overrides participate in the
schema hash. Applying ORM restore requires the administrative context produced
by `RestoreContext::empty_database`; it does not grant host authorization or
bypass database-native policy.

`BlobStoreRestoreObjectSink` (available without the `orm` feature) writes
restored object bytes back to a `graphql-orm-storage` `BlobStore` at each
object's original storage key, only inside its `RestoreKeyScope` and never over
existing content. See [restore semantics](restore-semantics.md#object-restore).

## Incremental Backup

`create_incremental_backup` writes compressed change files and an incremental
manifest linked to a parent snapshot.

```rust
use graphql_orm_backup::{
    BackupObjectIndex, GraphqlOrmBackupAdapter, IncrementalBackupRequest,
    LocalBackupRepository, create_incremental_backup,
};
use uuid::Uuid;

async fn run_incremental(
    repository: &LocalBackupRepository,
    database: &dyn GraphqlOrmBackupAdapter,
    objects: &dyn BackupObjectIndex,
    parent_snapshot_id: Uuid,
) -> Result<(), graphql_orm_backup::BackupError> {

    create_incremental_backup(
        repository,
        database,
        objects,
        IncrementalBackupRequest {
            snapshot_id: Uuid::new_v4(),
            parent_snapshot_id,
            created_at: 1_775_174_401,
            app_id: "example-app".to_string(),
            app_version: "0.1.0".to_string(),
        },
    )
    .await?;

    Ok(())
}
```

Delete changes emit manifest tombstones. The quality of an incremental backup
depends on the adapter’s change journal or equivalent application-side change
tracking.

## Restore

`restore_snapshot` loads, authenticates and validates the manifest chain,
verifies payload checksums, parses compressed JSON Lines, and calls the
database adapter. The repository handle must carry the host's
`ManifestAuthenticator`.

```rust
use graphql_orm_backup::{
    BackupRepository, GraphqlOrmBackupAdapter, RestoreContext, restore_snapshot,
};
use uuid::Uuid;

async fn restore(
    repository: &dyn BackupRepository,
    database: &dyn GraphqlOrmBackupAdapter,
    snapshot_id: Uuid,
) -> Result<(), graphql_orm_backup::BackupError> {
    restore_snapshot(
        repository,
        database,
        snapshot_id,
        RestoreContext::empty_database(),
    )
    .await?;

    Ok(())
}
```

Use `RestoreContext::dry_run()` to validate and parse a snapshot chain without
calling adapter restore methods.

Object rehydration is separate:

```rust
use std::sync::Arc;

use graphql_orm_backup::{
    BackupRepository, BlobStoreRestoreObjectSink, RestoreKeyScope, load_manifest,
    restore_objects,
};
use graphql_orm_storage::BlobStore;
use uuid::Uuid;

async fn restore_object_store(
    repository: &dyn BackupRepository,
    live_store: Arc<dyn BlobStore>,
    snapshot_id: Uuid,
) -> Result<(), graphql_orm_backup::BackupError> {
    let manifest = load_manifest(repository, snapshot_id).await?;
    let sink = BlobStoreRestoreObjectSink::new(
        live_store,
        RestoreKeyScope::prefixes(["media"])?,
    );
    restore_objects(repository, &manifest, &sink).await
}
```

## Buffer and decoding limits

Compatibility buffering defaults to 64 MiB per body. Database restore accepts at
most 256 MiB of decoded bytes per payload and 512 MiB per manifest stage by
default. Objects are unbounded by default; set `object_bytes` to cap each one.

Backups, incremental backups and compaction enforce the same limits from the
same repository handle before writing: decoded payload and stage sizes and
declared object sizes before any write, the compressed size while encoding,
streamed object sizes while writing, and the manifest size before sealing it.
They fail with `BackupError::BackupCapacityExceeded`, which names the payload
and the `BackupCapacityLimitKind`, so a backup never produces a snapshot that
restore with the same configuration refuses. Configure reviewed larger finite limits with `BackupCapacityLimits` on
the repository; custom indexes/sinks expose `buffered_byte_limit` for their
compatibility defaults. `BackupByteLimit::Unbounded` explicitly accepts memory
risk. Capacity errors are permanent for the same input and configuration.

Native streams avoid compatibility collection. These byte limits do not bound
RSS or total snapshot size: provider-owned buffers/current chunks, parsed JSON,
accumulated rows and adapter/application allocations still require a separate
resource budget. See the migration guide before restoring large existing data.

## Restore failures and recovery

`restore_snapshot` reports `BackupError::RestoreStageFailed` with typed
`RestoreFailureProgress` and the original error source:

- `Validation`: no adapter apply call was attempted.
- `Preparation`: the failing payload was not applied; previously completed
  manifest stages can remain in the target.
- `Apply`: the failing adapter call may have partially changed the target.

`completed_manifest_ids` records calls that the adapter reported as successful.
It is not a resume checkpoint. `target_may_have_changed()` includes prior stages
and an uncertain failed apply; it describes this attempt, not pre-existing data.
Dry run performs validation/parsing without recording applied stages.

Use an empty disposable staging database. After a partial/unknown outcome,
retain the failure progress, stop using that staging target and discard or
reconcile it through the target owner. Restart from a fresh empty target after
fixing the failure. Do not automatically continue with the next manifest or
retry into a partially populated database. Per-stage adapter transactions do not
create a whole-chain transaction. Cancellation or dropping the restore future
does not return progress or roll back an active adapter call. Treat cancellation
during apply as an unknown outcome and use the same staging-target recovery.

Object restore is a separate boundary: earlier objects and a custom sink's
partially written failing object can remain, including after checksum failure.
Use an isolated staging object namespace and publish only after all required
restore, verification and application consistency checks succeed. The library
does not roll back database/object stores together.

## Compaction And Retention

`compact_chain` folds a full-plus-incremental chain into a synthetic full
snapshot:

```rust
use graphql_orm_backup::{CompactChainRequest, BackupRepository, compact_chain};
use uuid::Uuid;

async fn compact(
    repository: &dyn BackupRepository,
    source_snapshot_id: Uuid,
) -> Result<(), graphql_orm_backup::BackupError> {
    compact_chain(
        repository,
        CompactChainRequest {
            snapshot_id: Uuid::new_v4(),
            source_snapshot_id,
            created_at: 1_775_174_402,
            app_id: "example-app".to_string(),
            app_version: "0.1.0".to_string(),
        },
    )
    .await?;

    Ok(())
}
```

`prune` retains the newest manifest chains selected by `KeepPolicy` and deletes
expired snapshot blobs, orphaned snapshot payloads that no manifest owns, and
unreferenced content-addressed object blobs. `keep_last` must be at least one;
`keep_last: 0` fails with `InvalidRetentionPolicy` before touching the
repository. Delete a last snapshot explicitly with `delete_snapshot`.

Snapshot ids are never reused: a full or incremental backup, or compaction,
fails with `SnapshotExists` before writing if the id has a manifest or any
payload. Incremental backups validate the parent chain first. A run that fails
or is cancelled before writing its manifest removes the payloads it wrote; if
the manifest write itself failed, its outcome is unknown and the payloads stay
for `prune` to remove when no manifest landed.

Retention, dependency checks and reachability come from the snapshot listing,
so `prune` and `delete_snapshot` fail closed when that listing is incomplete.
They return `BackupError::InvalidManifestChain` before deleting any snapshot or
object if a listed manifest's parent is stored but was not listed, or if
re-listing manifests gives a different set. Just before the object sweep they
list manifests again and stop, without deleting any object, unless the set is
exactly the retained snapshots. A listing truncated identically on every read
cannot be detected; the storage provider must fail such listings.

`delete_snapshot` removes one snapshot under the repository writer lock. It
refuses to delete a snapshot that is the parent of another manifest and
garbage-collects object blobs no remaining snapshot references:

```rust
use graphql_orm_backup::{BackupRepository, RepositoryLockOptions, delete_snapshot};
use uuid::Uuid;

async fn delete(
    repository: &dyn BackupRepository,
    snapshot_id: Uuid,
) -> Result<(), graphql_orm_backup::BackupError> {
    let result =
        delete_snapshot(repository, snapshot_id, &RepositoryLockOptions::default()).await?;
    println!("deleted {} blobs", result.deleted_blobs);
    Ok(())
}
```

## Locking and recovery

The orchestration functions take one exclusive repository lock,
`locks/repository.lock`: backups, compaction, `prune`, `delete_snapshot`,
`restore_snapshot`, `restore_objects` and verification. A concurrent one
fails with `BackupError::RepositoryLocked`, which carries the holder's
`RepositoryLockOwner` record: run id, host, process id, operation, optional
label and acquisition time. Retry after the holder ends.

The low-level `load_manifest`, `load_manifest_chain` and `write_manifest`
take no lock; callers coordinate them. `write_manifest` never overwrites: an
existing snapshot id fails with `SnapshotExists`.

Taking the lock needs write access. For read-only credentials, replicas or
mounts, pass `RepositoryLockMode::AssumeNoConcurrentWriters` in
`RestoreOptions` (`restore_snapshot_with_options`,
`restore_objects_with_options`) or `VerificationOptions`. This asserts that no
prune, snapshot deletion or compaction runs meanwhile; nothing checks it. If
one does, it can delete blobs mid-read: digests still stop substituted data,
but the operation fails part-way, after a database restore may have applied
earlier stages or an object restore may have written earlier objects.

Cancel with a `BackupCancellation`: in `BackupExecutionOptions` for backups,
incremental backups and compaction, in `RestoreOptions` for restores, and in
`VerificationOptions` for verification. A writer stops at its next safe point,
lets object uploads already in flight finish, removes its payloads, releases
the lock and returns `Cancelled`. Restore and verification stop before their
next read or apply call and release the lock. Use cancellation for host-side
timeouts: dropping the future cannot release the lock. A crashed or dropped run leaves
the lock and its owner record. Nothing expires automatically, because a slow
holder could still be writing. To recover:

1. Read the lock with `read_repository_lock`, or take the owner from the
   `RepositoryLocked` error.
2. Confirm that the run has stopped (its process on that host is gone) and
   that no other backup, restore, verification, compaction or prune is running
   against the repository.
3. Call `break_repository_lock` with the owner's `run_id` and a minimum age.
   It refuses a lock that names another run, is younger than the minimum age,
   or has no owner record.

```rust
use graphql_orm_backup::{
    BackupRepository, BreakRepositoryLockRequest, RepositoryLockStatus,
    break_repository_lock, read_repository_lock,
};

async fn recover(
    repository: &dyn BackupRepository,
) -> Result<(), graphql_orm_backup::BackupError> {
    if let RepositoryLockStatus::Owned(owner) = read_repository_lock(repository).await? {
        // Only after confirming that `owner` and every other writer have stopped.
        break_repository_lock(
            repository,
            &BreakRepositoryLockRequest {
                expected_run_id: owner.run_id,
                min_age_seconds: 3_600,
            },
        )
        .await?;
    }
    Ok(())
}
```

A lock without an owner record, such as one written by 0.13.0 or earlier, is
reported as `RepositoryLockStatus::Unidentified` and needs offline removal
after quiescing every writer, handle and in-flight request. If an operation
completed but its lock release failed, it returns `LockReleaseFailed` naming
the run and, for backups and compaction, the committed snapshot; recover the
lock the same way.

## Repository Layout

Full backups use this layout:

```text
snapshots/{snapshot_id}/manifest.json
snapshots/{snapshot_id}/database/tables/{table_name}.jsonl.zst
snapshots/{snapshot_id}/database/changes/{table_name}.jsonl.zst
objects/sha256/{first_two}/{next_two}/{sha256}
locks/repository.lock
```

Table payloads are zstd-compressed JSON Lines. Manifest table checksums cover
the stored compressed bytes.

## BlobStore Adapter

Use `BlobStoreBackupRepository` when an application already has a
`graphql-orm-storage::BlobStore` provider:

```rust
use std::sync::Arc;

use graphql_orm_backup::{BlobStoreBackupRepository, ManifestAuthenticator};
use graphql_orm_storage::BlobStore;

fn backup_repository(
    store: Arc<dyn BlobStore>,
    authenticator: ManifestAuthenticator,
) -> BlobStoreBackupRepository {
    BlobStoreBackupRepository::new(store).with_manifest_authenticator(authenticator)
}
```

`LocalBackupRepository` is built on top of the storage crate's local
`BlobStore`. S3-compatible backup repositories should also use this adapter with
the storage crate's S3 backend rather than adding AWS SDK code here.

## Verification

Use `verify_manifest_and_objects` to validate a completed snapshot manifest
against repository contents:

```rust
use graphql_orm_backup::{BackupRepository, BackupSnapshotManifest};

async fn verify(
    repository: &dyn BackupRepository,
    manifest: &BackupSnapshotManifest,
) -> Result<(), graphql_orm_backup::BackupError> {
    graphql_orm_backup::verify_manifest_and_objects(repository, manifest).await
}
```

Verification checks:

- manifest seal, checksum, format version and entry namespaces
- object blob checksums
- table export blob checksums
- incremental change blob checksums

`VerificationOptions` lets callers tune bounded checksum verification
concurrency.
