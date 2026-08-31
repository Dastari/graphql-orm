---
title: "graphql-orm-backup"
kind: reference
status: active
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-08-31
review_by: 2027-02-01
supersedes: []
---

# graphql-orm-backup

Backup and restore orchestration for `graphql-orm` applications. It writes
versioned manifests, compressed table/change streams, and content-addressed
objects; it validates chains, verifies snapshots, restores in a defined order,
compacts chains, and prunes retention safely.

The host supplies database export/import and referenced-object adapters. This
crate does not define application authorization, scheduling, object metadata,
cloud credentials, application transactions, or a claim that a restore is safe
for an unverified target.

## Install

Packages are Git-only. Pin one reviewed full monorepo revision for backup,
storage, and any direct ORM dependency:

Upgrade deliberately: review migration guides and changelogs for every pinned
companion, move all of them to one reviewed revision, and exercise backup plus
restore before replacing a production pin.

```toml
[dependencies]
graphql-orm-backup = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.7.2" }
```

The default `local` feature provides `LocalBackupRepository`. To supply only a
custom repository, disable defaults. The optional `orm-sqlite` or
`orm-postgres` features select one ready-made ORM adapter lane; do not enable
both.

## Shortest valid integration

The repository is concrete; the database and object index are deliberately
host-provided traits. A full backup requires both:

```rust,no_run
use graphql_orm_backup::{create_full_backup, BackupObjectIndex, FullBackupRequest, GraphqlOrmBackupAdapter, LocalBackupRepository};
use uuid::Uuid;

# async fn example(database: &dyn GraphqlOrmBackupAdapter, objects: &dyn BackupObjectIndex) -> Result<(), graphql_orm_backup::BackupError> {
let repository = LocalBackupRepository::new("./backups");
let result = create_full_backup(&repository, database, objects, FullBackupRequest {
    snapshot_id: Uuid::new_v4(), created_at: 0,
    app_id: "host-application".into(), app_version: "0.1.0".into(),
}).await?;
println!("{}", result.manifest.snapshot_id);
# Ok(())
# }
```

Run `restore_snapshot` in `RestoreMode::DryRun` first, then apply only after
the host validates its target and recovery process. Both modes check manifest
backend/schema compatibility before adapter writes.

The canonical executable fixture is
[`tests/full_backup_creation.rs`](tests/full_backup_creation.rs); it supplies
test-only adapters and verifies repository layout, checksums, deduplication,
and manifest-last publication. Run it with:

```sh
cargo test -p graphql-orm-backup --test full_backup_creation
```

## Features and safety boundary

| Feature | Default | Effect |
| --- | --- | --- |
| `local` | Yes | Local repository over storage's `BlobStore`. |
| `smb` | No | Native SMB transport through storage; no backup-owned SMB code. |
| `orm` | No | Lower-level host-selected ORM integration. |
| `orm-sqlite` / `orm-postgres` | No | One explicit ready-made ORM adapter lane. |

All repository writes use an advisory lock backed by atomic conditional create.
Never replace it with exists-then-write. Credentials are owned by the storage
provider; backup manifests and diagnostics must not contain them. Client-side
encryption and content-defined chunking are out of scope.

## Configuration and operations

[Configuration and limits](docs/configuration.md) covers execution concurrency,
lock, verification, restore, and retention policy types. See [usage](docs/usage.md)
for host adapters, [restore semantics](docs/restore-semantics.md) for the
fail-closed path, and [snapshot format](docs/snapshot-format.md) for durable
layout.

## Further reading

- [Documentation index](docs/README.md)
- [Storage `BlobStore` integration](../graphql-orm-storage/docs/backup-integration.md)
- [SMB integration](docs/smb.md)
- [Migration guide](MIGRATION.md) and [changelog](CHANGELOG.md)
