---
title: "Backup Runtime API"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-10-27
review_by: 2027-02-01
supersedes: []
---

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
remain distinguishable. Exact Decimal values carry their validated precision and scale in both the
column descriptor and value. SQLite scaled integers and PostgreSQL native numerics therefore
round-trip through one canonical representation; restore rejects changed definitions, rounding,
range loss, and value-kind mismatches.

Runtime `DateTime` columns (`TEXT` on SQLite, `TIMESTAMPTZ` on PostgreSQL) use
`BackupValueKind::DateTime`. Their values are `BackupValue::String` holding the canonical
`RuntimeDateTime` text, UTC with exactly six fractional digits (`2026-10-27T01:02:03.000004Z`), on
every backend:

- export reads SQLite text through the record engine's parse and canonicalizes it, and reads
  PostgreSQL `TIMESTAMPTZ` typed, independent of the session `TimeZone`;
- a value without a portable form fails the export: unreadable SQLite text, PostgreSQL
  `±infinity`, or a UTC instant outside years 0001–9999 (including when offset normalization or
  microsecond rounding crosses that boundary), or leap-second text (`SS = 60`, checked before
  rounding can carry it into the next minute);
- restore accepts only canonical text within that range (other text or another value kind is
  refused before any write, including in `DryRun`) and binds PostgreSQL values through
  `CAST(? AS TIMESTAMPTZ)`;
- restore writes the stored values; no default, clock or automatic timestamp applies.

PostgreSQL normalizes a leap-second literal when storing it. Export preserves that normalized
instant; it cannot recover the original literal. Snapshot input containing `SS = 60` is refused.

Static derived date fields keep `BackupValueKind::String`, so their descriptors and schema hashes
are unchanged. `BackupValueKind` is `#[non_exhaustive]`.

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

Before importing any table, restore checks schema compatibility, table names, every table's row
shapes and value kinds (including canonical portable DateTime text), in-snapshot references, and
in `EmptyDatabase` mode the emptiness of all target tables. `DryRun` performs the input checks
without importing rows. A late invalid row shape or value therefore leaves every target table untouched.

Imports use one transaction per table, with row-count checks and post-commit re-export/hash
verification. A later database failure or verification failure can leave earlier tables committed;
restore does not promise a transaction across all tables.

A single-key table whose dependency targets its own key restores parent-first. Rows that
parent-first order cannot place (rows on a self-reference cycle, and the rows below them) are
inserted after the others with placeholder self-reference values: `NULL` in a nullable column, the
row's own key in a non-null one (a row that references itself satisfies its foreign key at the end
of the statement). Each row is then updated to its stored values, in the table's import
transaction, before count and hash validation. `NULL` placeholders never collide on a `UNIQUE`
column. A non-null `UNIQUE` self-reference cycle cannot be staged that way, and an append-only
table refuses the update; the table's transaction rolls back and the error carries
`BackupSelfReferenceCycle` (read it with `BackupSelfReferenceCycle::from_error`). Constraint
violations and explicit trigger refusals (SQLite `SQLITE_CONSTRAINT_TRIGGER`; PostgreSQL
`raise_exception`, `triggered_action_exception`, or `triggered_data_change_violation`) receive this
classification. The error's standard `source()` chain retains the original SQLx/database cause.
Other failures, including PostgreSQL statement cancellation (`57014`) and deadlock (`40P01`),
propagate unchanged and also roll back the current table.

Before restoring into a database named by configuration, compare `Database::database_identity()`
(PostgreSQL `current_database()` and its OID; the SQLite main file path) with the operator's
expectation.

## Change Journal

Enable the `change-journal` feature and prepare the internal table:

```rust
let database = Database::new(pool).with_change_journal();
database.ensure_change_journal_table().await?;
```

Generated GraphQL/repository writes and `MutationContext` writes pass through the existing
transactional mutation event path, so journal rows are written in the same transaction as the data
change. Restore imports do not use generated mutation paths, so restore does not pollute the journal.
