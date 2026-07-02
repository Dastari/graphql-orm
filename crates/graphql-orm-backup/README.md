# graphql-orm-backup

`graphql-orm-backup` provides backup repository, snapshot manifest, verification, restore,
incremental backup, and compaction orchestration for applications that use `graphql-orm`.

The crate deliberately stays outside application policy and storage metadata decisions. Host
applications provide adapters for database export/import and stored-object lookup; this crate owns
backup layout, checksums, repository writes, restore ordering, and operational safety.

## Highlights

- full snapshot creation through `create_full_backup`
- incremental snapshot creation through `create_incremental_backup`
- restore orchestration through `restore_snapshot`
- object rehydration through caller-supplied `RestoreObjectSink`
- manifest-chain loading and validation
- zstd-compressed JSON Lines table and change payloads
- content-addressed object blobs keyed by SHA-256
- local filesystem repository with path traversal protection
- `graphql-orm-storage::BlobStore` repository adapter for shared local/S3 provider code
- mounted SMB support through local filesystem semantics and `LocalBackupRepository::open_existing`
- bounded concurrent object writes and checksum verification
- advisory repository writer lock for backup, compaction, and pruning operations
- synthetic-full compaction through `compact_chain`
- retention pruning through `prune`

## Install

```toml
[dependencies]
graphql-orm-backup = { git = "https://github.com/Dastari/graphql-orm-backup" }
```

The default `local` feature enables `LocalBackupRepository`.

```toml
[dependencies]
graphql-orm-backup = {
    git = "https://github.com/Dastari/graphql-orm-backup",
    default-features = false
}
```

Use `default-features = false` when providing only custom repository implementations.

## Snapshot Layout

Backups are manifest-based and content-addressed:

```text
snapshots/{snapshot_id}/manifest.json
snapshots/{snapshot_id}/database/tables/{table_name}.jsonl.zst
snapshots/{snapshot_id}/database/changes/{table_name}.jsonl.zst
objects/sha256/{first_two}/{next_two}/{sha256}
locks/repository.lock
```

Table and change payloads are zstd-compressed JSON Lines. Manifest table/change checksums cover the
stored compressed bytes. Object blobs are deduplicated by SHA-256 content key.

## Quick Full Backup Example

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

## Restore Example

```rust
use graphql_orm_backup::{
    BackupRepository, GraphqlOrmBackupAdapter, RestoreContext, restore_snapshot,
};
use uuid::Uuid;

async fn restore_database(
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

`RestoreMode::DryRun` validates manifests, checksums, decompression, and JSONL parsing without
calling adapter import methods.

## Adapter Boundaries

- `GraphqlOrmBackupAdapter` handles schema metadata, full row export, incremental row export, and
  full/incremental row restore.
- `BackupObjectIndex` lists and loads application object bytes referenced by snapshots.
- `BackupRepository` stores backup blobs and manifests.
- `RestoreObjectSink` receives object bytes when applications rehydrate their primary object store.

The crate does not own authentication, authorization, application transactions, scheduling, audit
events, object metadata persistence, or cloud credentials.

## Documentation

- [Documentation index](docs/README.md)
- [Usage guide](docs/usage.md)
- [Architecture](docs/architecture.md)
- [Snapshot format](docs/snapshot-format.md)
- [Restore semantics](docs/restore-semantics.md)
- [Provider roadmap](docs/provider-roadmap.md)
- [Cloud provider direction](docs/cloud-provider-direction.md)
- [SMB mounted repository guidance](docs/smb.md)
- [graphql-orm integration brief](docs/graphql-orm-agent-brief.md)

## Status

Full backups, restore orchestration, incremental backups, manifest-chain validation, synthetic-full
compaction, local repository support, locking, and pruning are implemented.

Provider code is shared through `graphql-orm-storage::BlobStore`. `LocalBackupRepository` is a thin
wrapper over the storage crate's local blob backend, and `BlobStoreBackupRepository` can adapt any
storage blob provider, including S3-compatible storage from `graphql-orm-storage`.

Client-side encryption and content-defined chunking are intentionally out of scope for the current
crate.
