# graphql-orm-backup

Backup and restore orchestration primitives for applications that use `graphql-orm`.

This crate coordinates database export/import adapters, stored-object indexes, backup repositories, snapshot manifests, verification, and restore planning. It does not own application auth, UI, scheduling, or domain-specific workflow behavior.

## Current Status

- Snapshot manifest types implemented.
- Manifest checksum support implemented.
- Backup repository trait implemented.
- Local filesystem backup repository implemented.
- Object index and database adapter contracts implemented.
- Full backup planner skeleton implemented.
- Full snapshot creation implemented through `create_full_backup`.
- Verification helpers implemented.
- Restore safety context implemented for empty-target restores.

`graphql-orm` still needs to provide stable logical export/import and change-journal APIs before complete database backup/restore can be implemented.

## Documentation

- [Architecture](docs/architecture.md)
- [Usage guide](docs/usage.md)
- [Snapshot format](docs/snapshot-format.md)
- [Restore semantics](docs/restore-semantics.md)
- [Provider roadmap](docs/provider-roadmap.md)
- [graphql-orm integration brief](docs/graphql-orm-agent-brief.md)

## Design Rule

Backups are manifest-based and content-addressed.

```text
snapshots/{snapshot_id}/manifest.json
snapshots/{snapshot_id}/database/tables/{table_name}.jsonl.zst
snapshots/{snapshot_id}/database/changes/{table_name}.jsonl.zst
objects/sha256/{first_two}/{next_two}/{sha256}
```

The manifest is written last. Its checksum excludes its own `checksum` field.
Table payloads are currently written as uncompressed JSON Lines. The `.zst`
filename suffix is reserved for the stable future compressed layout.

## Backup Repository Example

```rust
use bytes::Bytes;
use graphql_orm_backup::{BackupRepository, LocalBackupRepository};

# async fn example() -> Result<(), graphql_orm_backup::BackupError> {
let repository = LocalBackupRepository::new("./backup");
repository
    .put_blob("snapshots/example/manifest.json", Bytes::from_static(b"{}"))
    .await?;
# Ok(())
# }
```

## Full Backup Creation

`create_full_backup` coordinates a database adapter, stored-object index, and
backup repository. It writes table payloads and content-addressed object blobs,
then writes the snapshot manifest last.

```rust
use graphql_orm_backup::{
    FullBackupRequest, LocalBackupRepository, create_full_backup,
};
use uuid::Uuid;

# async fn example(
#     database: &dyn graphql_orm_backup::GraphqlOrmBackupAdapter,
#     objects: &dyn graphql_orm_backup::BackupObjectIndex,
# ) -> Result<(), graphql_orm_backup::BackupError> {
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
# Ok(())
# }
```

## Restore Policy

The first supported restore mode is restore into an empty database and empty object store. In-place replacement is future work.

## Provider Roadmap

1. Local filesystem
2. S3
3. Azure Blob
4. SMB through mounted filesystem path
5. Dropbox
