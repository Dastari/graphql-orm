---
title: "graphql-orm-backup"
kind: reference
status: active
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-08-10
review_by: 2027-02-01
supersedes: []
---

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
- `BlobStoreRestoreObjectSink` for rehydrating a `graphql-orm-storage` blob store in place
- optional `orm` feature with ready-made `graphql-orm` runtime adapters
  (`OrmBackupAdapter`, `OrmBackupObjectIndex`) including replace-existing
  restore-target clearing
- manifest-chain loading and validation
- zstd-compressed JSON Lines table and change payloads
- content-addressed object blobs keyed by SHA-256
- local filesystem repository with path traversal protection
- `graphql-orm-storage::BlobStore` repository adapter for shared local/S3/SMB provider code
- feature-gated native SMB2/SMB3 plus separately named mounted-SMB legacy support
- streaming backup, verification, and restore for large referenced objects
- bounded concurrent object writes and checksum verification
- advisory repository writer lock for backup, compaction, and pruning operations
- synthetic-full compaction through `compact_chain`
- retention pruning through `prune`
- single-snapshot deletion with object garbage collection through `delete_snapshot`

## Install

```toml
[dependencies]
graphql-orm-backup = {
    git = "https://github.com/Dastari/graphql-orm.git",
    rev = "<reviewed-full-40-character-commit-sha>",
    version = "0.7.0"
}
```

GitHub with an exact reviewed revision is the supported distribution method.
Do not depend on a moving branch. Applications that also depend directly on
`graphql-orm` or `graphql-orm-storage` must use this monorepo URL and the same
revision as this crate so Cargo resolves one instance of each shared type.

The default `local` feature enables `LocalBackupRepository`.

```toml
[dependencies]
graphql-orm-backup = {
    git = "https://github.com/Dastari/graphql-orm.git",
    rev = "<reviewed-full-40-character-commit-sha>",
    version = "0.7.0",
    default-features = false
}
```

Use `default-features = false` when providing only custom repository implementations.

Enable `smb` for native SMB2/SMB3 through `SmbStorageBackend` and
`BlobStoreBackupRepository`. Backup 0.7.0 uses
`graphql-orm-storage` 0.6.0.

Enable `orm-sqlite` or `orm-postgres` for the ready-made `graphql-orm` runtime
adapters with an exact backend. Hosts that already select a backend through
their direct `graphql-orm` dependency may enable the lower-level `orm` feature
instead. Exactly one ORM backend must be active.

This workspace resolves `graphql-orm` 0.19.0, `graphql-orm-backup` 0.7.0, and
`graphql-orm-storage` 0.6.0 through workspace path dependencies and the root
`Cargo.lock`. External consumers select one reviewed full monorepo revision.
The ORM's optional `auth-agql` bridge pins `agql-auth` 0.14.0 at
`413fda3435f060604cd653c11e2cc18a668aace1`. This crate does not enable that
feature or depend directly on authorization; hosts own authorization and may
enable it through their direct `graphql-orm` dependency.

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
calling adapter import methods. Both dry-run and applying restore compare the
manifest backend/schema hash with the target before target checks or writes.

## Adapter Boundaries

- `GraphqlOrmBackupAdapter` handles schema metadata, full row export, incremental row export, and
  full/incremental row restore. Full restore receives the source manifest
  schema so adapters can fail closed on incompatibility.
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
- [Provider backlog](../../docs/plans/backlog/backup-providers/README.md)
- [Cloud provider direction](docs/cloud-provider-direction.md)
- [Native and mounted SMB](docs/smb.md)
- [Migration guide](MIGRATION.md)
- [Changelog](CHANGELOG.md)

## Status

Full backups, restore orchestration, incremental backups, manifest-chain validation, synthetic-full
compaction, local repository support, locking, pruning, and single-snapshot deletion are
implemented. The optional `orm` feature ships ready-made `graphql-orm` runtime adapters so hosts
only supply entity metadata and object-table column names.

`OrmBackupAdapter` accepts a flattened metadata vector from
`SchemaModuleCatalog::entities()`, including dependency-owned private schema
modules. It does not require generated GraphQL operation roots and does not
run module restore hooks, reconciliation, or readiness transitions.

Provider code is shared through `graphql-orm-storage::BlobStore`. `LocalBackupRepository` is a thin
wrapper over the storage crate's local blob backend, and `BlobStoreBackupRepository` can adapt any
storage blob provider, including S3-compatible and native SMB storage from
`graphql-orm-storage`.

Client-side encryption and content-defined chunking are intentionally out of scope for the current
crate.
