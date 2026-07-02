# Usage Guide

`graphql-orm-backup` is an orchestration crate. It does not connect directly to
an application database or object store. Host applications provide small adapter
implementations, and the crate handles snapshot layout, checksums, repository
writes, restore orchestration, compaction, pruning, and verification.

## Core Concepts

- `BackupRepository`: destination for backup blobs and manifests.
- `LocalBackupRepository`: filesystem implementation of `BackupRepository`.
- `GraphqlOrmBackupAdapter`: interim database export/import contract until the
  final `graphql-orm` runtime backup API lands.
- `BackupObjectIndex`: application adapter that lists and loads stored objects
  referenced by a snapshot.
- `BackupSnapshotManifest`: durable record of a snapshot's database files,
  object files, checksums, schema hash, and application metadata.
- `RestoreContext`: explicit restore mode and safety flags.
- `RestoreObjectSink`: application hook for rehydrating object bytes.
- `KeepPolicy`: retention rule used by `prune`.

## Creating A Full Backup

Full snapshot creation is implemented through `create_full_backup`.

```rust
use graphql_orm_backup::{
    BackupObjectIndex, FullBackupRequest, GraphqlOrmBackupAdapter,
    LocalBackupRepository, create_full_backup,
};
use uuid::Uuid;

async fn run_backup(
    database: &dyn GraphqlOrmBackupAdapter,
    objects: &dyn BackupObjectIndex,
) -> Result<(), graphql_orm_backup::BackupError> {
    let repository = LocalBackupRepository::new("./backups");

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

`GraphqlOrmBackupAdapter` is intentionally narrow. The host application or a
future `graphql-orm` runtime adapter owns database-specific export/import
details.

For full backups, implement:

- `schema_snapshot`: return backend name, migration version, and stable schema
  hash.
- `export_full`: return table exports in the order they should be written.

For restore and incremental backups, implement:

- `restore_target_is_empty`: report whether `RestoreMode::EmptyDatabase` is safe.
- `export_incremental`: return create/update/delete changes since a parent snapshot.
- `restore_full`: import full table exports.
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
    database: &dyn GraphqlOrmBackupAdapter,
    objects: &dyn BackupObjectIndex,
    parent_snapshot_id: Uuid,
) -> Result<(), graphql_orm_backup::BackupError> {
    let repository = LocalBackupRepository::new("./backups");

    create_incremental_backup(
        &repository,
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

`restore_snapshot` loads and validates the manifest chain, verifies payload
checksums, parses compressed JSON Lines, and calls the database adapter.

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
use graphql_orm_backup::{
    BackupRepository, BackupSnapshotManifest, RestoreObjectSink, restore_objects,
};

async fn restore_object_store(
    repository: &dyn BackupRepository,
    manifest: &BackupSnapshotManifest,
    sink: &dyn RestoreObjectSink,
) -> Result<(), graphql_orm_backup::BackupError> {
    restore_objects(repository, manifest, sink).await
}
```

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
expired snapshot blobs plus unreferenced content-addressed object blobs.

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

- manifest checksum
- object blob checksums
- table export blob checksums
- incremental change blob checksums

`VerificationOptions` lets callers tune bounded checksum verification
concurrency.
