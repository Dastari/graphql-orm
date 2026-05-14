# Usage Guide

`graphql-orm-backup` is an orchestration crate. It does not connect directly to
an application database or object store. Host applications provide small adapter
implementations, and the crate handles snapshot layout, checksums, repository
writes, verification, and restore safety scaffolding.

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

Restore and incremental methods are present in the trait so the public contract
can evolve in place, but full restore and true incremental backup are not yet
implemented by this crate.

## Object Index Responsibilities

`BackupObjectIndex` lets an application expose externally stored objects without
coupling this crate to a specific object-storage implementation.

For full backups, implement:

- `list_objects_for_full_backup`: return object ids, original storage keys,
  expected SHA-256 hashes, sizes, and optional MIME types.
- `load_object`: return the exact bytes for a listed object.

`create_full_backup` verifies the loaded bytes against the declared SHA-256
before the object is referenced in the manifest.

## Repository Layout

Full backups use this layout:

```text
snapshots/{snapshot_id}/manifest.json
snapshots/{snapshot_id}/database/tables/{table_name}.jsonl.zst
objects/sha256/{first_two}/{next_two}/{sha256}
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

## Restore Status

The crate currently provides restore safety scaffolding only:

- `RestoreContext::empty_database`
- `RestoreContext::dry_run`
- `ensure_empty_restore_target`

Full restore depends on stable `graphql-orm` row import, dependency ordering,
restore context, and policy/journal bypass APIs.

## Incremental Backup Status

Incremental backup is intentionally deferred. It depends on a reliable
`graphql-orm` change journal with row updates, deletes/tombstones, transaction
ordering, and object-change discovery semantics.
