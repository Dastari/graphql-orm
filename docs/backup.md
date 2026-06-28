# Backup Runtime API

`graphql-orm` owns backend-agnostic database backup primitives only:

- entity backup metadata
- dependency-derived restore ordering
- logical row export/import
- restore context flags
- schema snapshots and schema hashes
- optional change journal support

Repository providers, backup manifests, object deduplication, and object storage backends belong in
`graphql-orm-backup` and `graphql-orm-storage`, not in `graphql-orm`.

## Metadata

Generated entities expose backup metadata through `Entity::metadata()`. Applications can pass the
same entity list used for schema stages to:

```rust
let entities = database.list_backup_entities(&graphql_orm_entity_metadata());
let snapshot = database.schema_snapshot("20260514_01", &graphql_orm_entity_metadata());
```

`schema_roots!` generates these helpers:

- `graphql_orm_entity_metadata()`
- `graphql_orm_backup_entities()`
- `graphql_orm_schema_snapshot(migration_version)`

## Logical Rows

Rows are exported as `BackupRow` with typed `BackupValue` values. The row format is independent of
SQLite/PostgreSQL physical storage, so UUIDs, JSON, bytes, numbers, booleans, strings, and nulls
remain distinguishable.

Sensitive columns use `#[backup(redact)]` or `#[backup(exclude)]`.

## Full Export And Restore

Use a consistent snapshot for export:

```rust
let mut snapshot = database.begin_consistent_snapshot().await?;
let rows = database.export_table_rows(&mut snapshot, &entity).await?;
```

Restore initially supports `RestoreMode::EmptyDatabase` and `RestoreMode::DryRun`.
`RestoreMode::ReplaceExisting` is intentionally rejected until replace semantics are designed.

```rust
database
    .restore_backup_rows(
        &backup_schema_snapshot,
        &current_schema_snapshot,
        &rows_by_table,
        &RestoreContext::empty_database(),
    )
    .await?;
```

Restore validates schema compatibility, table names, row shapes, row counts, and row hashes.

## Change Journal

Enable the `change-journal` feature and prepare the internal table:

```rust
let database = Database::new(pool).with_change_journal();
database.ensure_change_journal_table().await?;
```

Generated GraphQL/repository writes and `MutationContext` writes pass through the existing
transactional mutation event path, so journal rows are written in the same transaction as the data
change. Restore imports do not use generated mutation paths, so restore does not pollute the journal.
