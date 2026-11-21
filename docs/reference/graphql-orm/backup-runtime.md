---
title: "Backup Runtime API"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-15
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

`restore_backup_rows` imports every table of the snapshot in one transaction, on PostgreSQL and
SQLite. Each table's row count and re-exported row hashes are checked inside that transaction, and
it commits once at the end. A table the database refuses, or a failed check, rolls back every table
imported before it: the target is as it was, and the restore can be retried without cleanup. The
transaction stays open for the whole restore, so size the restoring role's PostgreSQL
`statement_timeout` and `idle_in_transaction_session_timeout` for the largest snapshot.
`import_table_rows` restores one table in a transaction of its own.

The same transaction resets the runtime search index of every restored table: its documents are
deleted and its readiness set to `building`, so search is refused until
`backfill_runtime_search` rebuilds it (see
[runtime search](runtime-search.md#backup-and-restore)). Static search documents are not rebuilt.

SQL Server has no ORM restore. A host that restores it with its own statements records a
[restore-in-progress marker](mssql.md#restore-in-progress-marker) instead.

A single-key table whose dependency targets its own key restores parent-first. Rows that
parent-first order cannot place (rows on a self-reference cycle, and the rows below them) are
inserted after the others with placeholder self-reference values: `NULL` in a nullable column, the
row's own key in a non-null one (a row that references itself satisfies its foreign key at the end
of the statement). Each row is then updated to its stored values, in the restore's
transaction, before count and hash validation. `NULL` placeholders never collide on a `UNIQUE`
column. A non-null `UNIQUE` self-reference cycle cannot be staged that way, and an append-only
table refuses the update; the restore's transaction rolls back and the error carries
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

Enable the `change-journal` feature and prepare the journal:

```rust
let database = Database::new(pool).with_change_journal();
// As the schema owner. On SQLite this only creates the journal table.
database
    .ensure_change_journal_recorder(&graphql_orm_entity_metadata())
    .await?;
```

Applying the schema target with the feature enabled does the same on PostgreSQL.

Generated GraphQL/repository writes and `MutationContext` writes record their changes in the same
transaction as the data change. On SQLite the ORM writes each entry from the transactional
mutation event path. On PostgreSQL the database records them: an `AFTER INSERT OR UPDATE OR
DELETE` row trigger on each single-key entity table calls a `SECURITY DEFINER` recorder that takes
the key from the changed row and the action from the operation. ORM write transactions turn
recording on with a transaction-local setting; a journaled write fails and rolls back if its
table's recorder trigger is missing or altered. Dependent deletes, cascades and set-null updates
journal like any other change. An update that changes a primary key journals a `delete` of the old
key and a `create` of the new one. `changed_at` is whole seconds from the database clock on
PostgreSQL and from the application clock on SQLite. Composite-key entities are refused.

Restore imports do not use generated mutation paths and switch PostgreSQL recording off in their
own transactions, even when a role or database default turns it on, so restore does not pollute the
journal.

### Incremental export

Every entry has a `seq` column, its position in the journal, which the database assigns in
increasing order. `export_changes` returns entries in that order and each `BackupChange` carries its
position in `sequence`. Entry ids are random UUIDs and do not order entries.

```rust
let mut position = None; // persist this with the backup it belongs to
let changes = database
    .export_changes(ChangeWindow { after_sequence: position, until })
    .await?;
if let Some(last) = changes.last() {
    position = Some(last.sequence);
}
```

Consecutive exports that each continue from the last returned position return every entry exactly
once. An export returns a contiguous run of positions: `until` (Unix seconds) ends the run at the
first entry recorded later rather than leaving it out, so a later export still returns it. Positions
can have gaps (a rolled-back write uses one) and are never reused.

On PostgreSQL a position is assigned when an entry is written, which can be before its transaction
commits. So that no entry appears later behind a position already exported, `export_changes` waits
for the journaled write transactions that were open when it started, by watching the lock the
recorder holds on the journal, and then returns the entries up to the highest position it saw
committed. It waits for up to 30 seconds (`CHANGE_EXPORT_DEFAULT_WAIT`);
`export_changes_with_wait` takes another limit. When the limit passes it returns nothing and fails
with an error that `ChangeExportTimedOut::from_error` recognises; retry with the same window. A
write transaction left open for a long time therefore delays exports, and a prepared (two-phase)
transaction that journaled a write blocks them until it is resolved. Export from the primary: a
standby cannot see the primary's open transactions, so `export_changes` refuses to run on one.
SQLite has one writer at a time and never waits.

A journal created by a release before 0.157.0 has no `seq` column, and `export_changes` refuses it
instead of ordering by id. Run `ensure_change_journal_recorder` (or `ensure_change_journal_table`,
or on PostgreSQL apply the schema target) as the journal's owner: existing entries are numbered by
`changed_at` and then by the order they were stored in, and new entries continue after them. The
upgrade is all or nothing on both backends. On SQLite it rebuilds the journal in one transaction
under the temporary name `__graphql_orm_change_log_sequenced`; when an object with that name already
exists it changes nothing and fails with an error `ChangeJournalUpgradeConflict::from_error`
recognises, because that object is not the upgrade's to drop. On
PostgreSQL only the owner can add the column; for any other role the call leaves the journal
unchanged. Positions kept from an export made before the upgrade are entry ids and cannot be
converted: take a full backup, then start incremental exports from the position it returns.

The journal cannot record a `TRUNCATE`, or rows the database changes through a foreign key in a
table without a recorder (a composite-key table, for example). On PostgreSQL both are refused
rather than left unjournaled:

- Every journaled table carries a `BEFORE TRUNCATE` statement trigger,
  `graphql_orm_change_journal_truncate`. `TRUNCATE` fails with SQLSTATE `0A000` and a message that
  starts `graphql-orm change journal refuses TRUNCATE`, including when `TRUNCATE ... CASCADE`
  reaches the table from another one. Delete the rows instead. A transaction that sets
  `graphql_orm.change_journal` (`CHANGE_JOURNAL_SETTING`) to `off` declares that it writes without
  journaling, as a restore does, and may truncate; take a full backup afterwards.
  The guard reads the effective `current_setting`: a role-level or database-level
  default of `off` also permits TRUNCATE. Hosts must never configure that default
  on a role or database; scope trusted restore opt-outs to `SET LOCAL` in the
  restore transaction.
  `validate_target` reports a journaled table without the guard.
- A foreign key from a table without a recorder to a journaled table, whose delete action is
  `CASCADE` or `SET NULL`, is refused when the recorder is planned (`plan_schema_target`) or
  installed (`ensure_change_journal_recorder`), with `ChangeJournalCascadeGap`, and is an error
  in `validate_target`. The install also checks the catalog, so it refuses such a foreign key
  (including `ON UPDATE` actions) on a table no schema target declares. Give the table a
  single-column primary key, or make the foreign key restrict the delete.
- A cascade into a journaled table is recorded by that table's own recorder.

SQLite writes journal entries from the ORM's own statements and has no `TRUNCATE`; rows changed by
raw SQL or by a database-side cascade are not journaled there. SQL Server has no change journal.

On PostgreSQL only privileged roles can read or write the journal. It has RLS enabled and no
policies, so a role subject to RLS can neither read nor write entries; the recorder runs as the
journal owner, so plans revoke every table or column privilege beyond `SELECT` that other roles hold on the journal,
and `EXECUTE` on the recorder. Create the journal, the tables and the recorder as the schema owner,
grant the runtime role nothing on the journal, and grant `SELECT` to a dedicated `BYPASSRLS`
export role if one reads it; that grant survives re-applying the plan. `export_changes` runs with `SET LOCAL row_security = off`, so
it reads every entry as the table owner or a `BYPASSRLS` role and fails with a PostgreSQL error for
a role subject to RLS, rather than exporting an empty journal. See
[PostgreSQL RLS](postgres.md#change-journal-recorder).
