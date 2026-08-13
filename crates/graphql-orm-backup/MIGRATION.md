---
title: "Migration Guide"
kind: reference
status: active
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-08-01
review_by: 2027-02-01
supersedes: []
---

# Migration Guide

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
