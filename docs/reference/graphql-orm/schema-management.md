---
title: "Schema Management"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-15
review_by: 2027-02-01
supersedes: []
---

# Schema Management

`graphql-orm` treats backend support and schema ownership as separate decisions.

- Backend features decide which database runtimes are compiled.
- `SchemaPolicy` decides who owns the schema and which operations are allowed.
- Schema changes are explicit. `Database::new`, `Database::builder`, and GraphQL schema construction do not apply migrations.

## Schema Policies

`SchemaPolicy` is configured on the runtime `Database`.

```rust
use graphql_orm::prelude::*;

let database = Database::builder(pool)
    .schema_policy(SchemaPolicy::Managed)
    .build();
```

Available policies:

- `ExternalReadOnly`: the live database is the source of truth. Queries and read-only validation are allowed. Entity writes and schema mutation are rejected.
- `ExternalWritable`: the live database is the source of truth. Entity writes are allowed when the backend and entity support writes. Schema application is rejected.
- `ValidateOnly`: validation is allowed. Planning and application are rejected.
- `PlanOnly`: validation and planning are allowed. Application is rejected.
- `Managed`: Rust entity metadata is the source of truth. Validation, planning, and explicit migration application are allowed when the backend implements migration support.

Compatibility defaults are preserved:

- SQLite and Postgres default to `Managed`.
- MSSQL defaults to `ExternalReadOnly`.

New code should prefer the builder so the ownership decision is visible at the call site.

For PostgreSQL RLS metadata, the same policy controls whether `graphql-orm` manages database RLS objects:

- `Managed`: plan and apply helper functions, `ENABLE ROW LEVEL SECURITY`, optional `FORCE ROW LEVEL SECURITY`, and named policies.
- `ValidateOnly`: validate table schema and RLS state where PostgreSQL can report it, including enabled/forced flags and expected policy definitions.
- `PlanOnly`: produce table and RLS SQL previews, but reject application.
- `ExternalReadOnly` and `ExternalWritable`: preserve external-schema behavior and do not plan, apply, or validate RLS policies.

## Validation

Validation compares two structured `SchemaModel` values and returns diagnostics. It never mutates the database.

```rust
let report = database
    .schema()
    .validate_against_entities(&[User::metadata()])
    .await?;

if report.has_errors() {
    for diagnostic in report.diagnostics {
        eprintln!("{:?}: {}", diagnostic.kind, diagnostic.message);
    }
}
```

Diagnostics include missing tables, missing columns, type differences, nullability differences, primary-key mismatches, constraint mismatches, and unsupported backend capabilities.

Search structures are part of the structured schema model when an entity has searchable fields.
Changing searchable fields, JSON search paths, weights, language, tokenizer, or strategy changes the
schema hash and plans a search-index create/drop/recreate step.

## Planning

Migration plans are structured first and rendered to backend SQL second.

```rust
let plan = database
    .schema()
    .plan_migration_to_entities(
        "2026-06-27-add-users",
        "add users table",
        &[User::metadata()],
    )
    .await?;

for step in &plan.steps {
    println!("{:?}: {}", step.risk, step.reason);
}
```

Each `PlannedMigrationStep` carries a `MigrationRisk`:

- `Additive`
- `Compatible`
- `Risky`
- `Destructive`

A table's risk is the highest risk among its steps. `ApplyOptions::default()` applies everything up
to `Risky` and refuses `Destructive` steps until `allow_destructive` is set.

| Step | Risk |
| --- | --- |
| `CreateTable`, `CreateIndex`, `CreateSearchIndex`, `EnableExtension`, `SetAppendOnly { enabled: true, retention_purge: false }`, `SetAppendOnly { enabled: true, retention_purge: true }` on a table without append-only enforcement, `AddColumn` (nullable or defaulted) | `Additive` |
| `AlterColumn` without a type change (other than tightening nullability) | `Compatible` |
| `AddColumn` (required, no default), `AlterColumn` that tightens nullability, `AlterColumn` with a lossless type widening, `AddForeignKey`, `DropSearchIndex`, `AlterSearchIndex`, `SetCompositeUniques` or `SetCheckConstraints` that only adds constraints | `Risky` |
| `DropTable`, `DropColumn`, `AlterColumn` with any other type change, `DropIndex`, `DropForeignKey`, `SetAppendOnly { enabled: false }`, `SetAppendOnly { enabled: true, retention_purge: true }` on an append-only table or without a known prior state, `SetCompositeUniques` or `SetCheckConstraints` that removes or changes a constraint | `Destructive` |

- **Type changes.** PostgreSQL renders `ALTER COLUMN ... TYPE` without a `USING` clause, so a
  narrowing conversion rounds or truncates stored values. Only a closed set of widenings keeps
  every stored value and stays `Risky`: a larger integer; an integer into an exact numeric with
  enough integer digits, or into a float that represents it exactly (`SMALLINT` into `REAL`, up to
  `INTEGER` into `DOUBLE PRECISION`); `REAL` into `DOUBLE PRECISION`; a numeric with at least the
  same integer digits and scale; and a longer or unlimited `VARCHAR`, `NVARCHAR` or `TEXT`. The
  classifier recognises the PostgreSQL, SQLite and SQL Server spellings of those types and reads
  them as the plan's backend does: on SQL Server, `VARCHAR`, `CHARACTER VARYING` and `TEXT` keep
  only the collation's code page, so `NVARCHAR` into any of them is not a widening, and a
  length-less `VARCHAR`, `NVARCHAR`, `CHAR` or `NCHAR` is one character wide; bare
  `DECIMAL`/`NUMERIC` is `(18,0)` on SQL Server and SQLite; SQLite `INTEGER` is 64-bit. Every other
  change, including one between types it does not know, is `Destructive`.
- **Append-only steps.** Turning enforcement on without retention purge, or turning purge off, is
  `Additive`. Enabling purge-admitting enforcement is `Additive` only when the source schema shows
  the table without append-only enforcement; on a table that already rejects every delete, or
  without a known prior state (`classify_migration_step`), it is `Destructive`. Planners and
  `PlannedSchemaStage::steps()` classify against their source schema.
- **Integrity-removing steps.** Dropping an index (the step does not say whether it is unique), a
  foreign key, append-only enforcement, or an existing composite-unique or check constraint
  removes a guarantee that later writes depended on, so it needs the same explicit opt-in as a
  data-losing step. A changed index or foreign key is planned as a drop and a create, and so is
  `Destructive` as well.
- PostgreSQL and SQL Server plans drop a removed table's own foreign keys first, so that table's
  first step is a `DropForeignKey` and the `DropTable` follows; both are `Destructive`.

Rendered SQL is available as `plan.statements`, but callers should treat it as an artifact of the plan rather than the migration source of truth.

Plans ignore live tables the target does not declare. `PlanOptions::default()` is
`PlanOptions::managed_tables_only()`: the ORM keeps no record of which tables it created, so an
undeclared table may be the host's own or another application's, and no `DROP TABLE` is planned
for it.

To remove a table whose entity was deleted, opt in explicitly:

```rust
// Every live table the target does not declare gets a destructive DropTable step.
let plan = database
    .schema()
    .plan_migration_to_entities_with_options(
        "2026-07-06-remove-cache",
        "remove cache tables",
        &graphql_orm_entity_metadata(),
        PlanOptions::strict(),
    )
    .await?;
```

`PlanOptions::strict()` states that the ORM owns every table the introspected schema reports. In a
shared database, plan the drop from a current model that names only the tables to remove, with
`plan_migration_with_options(version, description, &current, &target, PlanOptions::strict())`.
Applying a drop always needs `allow_destructive`.

### Compatible existing-schema adoption

Adoption uses the ordinary planner and migration-history path; there is no
generic bypass option. If live introspection proves that managed primary and
unique keys, ordered compound foreign keys, delete actions, named index
columns/directions, and supported check expressions satisfy the target entity
contract, the resulting plan has no steps or statements. Applying that empty
plan records the migration version without rebuilding a table, dropping an
index, or rewriting a row.

Check comparison is deliberately conservative. It recognizes a closed grammar
of simple identifiers, literals, comparisons, parentheses, and commas, and
normalizes only physical names, whitespace, keyword case, redundant balanced
outer parentheses, and quoting of ordinary lowercase portable identifiers.
Case-sensitive quoted identifiers remain distinct. The comparator does not
claim algebraic or database-affinity equivalence. A partial, reordered,
differently targeted, weakened, or unrecognized constraint remains drift.
Repair or explicitly migrate it; do not record the version by hand.

For SQLite, enable `PRAGMA foreign_keys = ON` on every managed connection and
run `PRAGMA foreign_key_check` after migration. Migration application remains
transactional: a failed table rebuild does not record history and restores the
pre-migration schema and rows.

A SQLite rebuild that removes a composite UNIQUE key is refused while any
foreign key references that key, because the reference could no longer be
enforced. One plan may remove both: when the child table's rebuild, which
removes the foreign key, is scheduled before the parent's, the plan applies and
keeps every row. The check before any change reads the child's foreign keys as
its rebuild will leave them, and the parent's rebuild reads the catalog again
inside the migration transaction, with nothing excused, immediately before it
replaces the table. The refusal stands when the parent rebuild comes first, and
when a table the plan does not rebuild (for example an unmanaged host table)
still references the key.

Managed migrations create full-text search storage, such as Postgres shadow tables/GIN indexes and
SQLite FTS5 virtual tables. They do not backfill existing rows automatically. Run the generated
rebuild API after applying a migration that adds or changes search:

```rust
Article::rebuild_search_index(&database).await?;
```

This keeps migration application predictable for large tables. New ORM writes refresh local
searchable fields and configured JSON search paths automatically after the structures exist.

`plan_migration_to_entities` and `apply_migration` remain table-schema APIs. To include RLS metadata emitted by `#[graphql_rls]`, use the schema target generated by `schema_roots!`:

```rust
let target = graphql_orm_schema_target();

let report = database.schema().validate_target(&target).await?;
let plan = database
    .schema()
    .plan_schema_target("2026-06-29-rls", "enable RLS", &target)
    .await?;
database
    .schema()
    .apply_schema_target(&plan, ApplyOptions::default())
    .await?;
```

Managed PostgreSQL RLS SQL is appended after table migration SQL. Policy names are deterministic:
`graphql_orm_<table_name>_select`, `graphql_orm_<table_name>_insert`,
`graphql_orm_<table_name>_update`, and `graphql_orm_<table_name>_delete`, with deterministic
sanitization for long or schema-qualified table names.

## Applying

Migration application is explicit and requires a backend that implements `MigrationBackend`.

```rust
database
    .schema()
    .apply_migration(&plan, ApplyOptions::default())
    .await?;
```

`ApplyOptions::default()` is conservative:

```rust
ApplyOptions {
    allow_destructive: false,
    additive_only: false,
    require_clean_schema: true,
    dry_run: false,
    expected_current_schema_hash: None,
    record_history: true,
}
```

Set `dry_run: true` to verify an application path without running statements. Destructive plans are rejected unless `allow_destructive` is explicitly enabled.

Set `additive_only: true` for service startup paths that may create missing ORM-owned tables or
indexes but must not alter, rebuild, or drop existing structures. Combine it with
`PlanOptions::managed_tables_only()` when the database contains unrelated tables managed by other
systems.

## ABI Schema Upgrades

The ABI migration model is built from ordered schema stages. Given a current database version and a target version, the runtime can plan and apply each forward stage.

```rust
let abi = SchemaAbi::new(vec![
    SchemaStage::from_entities("1", "initial schema", &[User::metadata()]),
    SchemaStage::from_entities("2", "add audit fields", &[User::metadata(), Audit::metadata()]),
])?;

database
    .schema()
    .apply_upgrade(&abi, "2", ApplyOptions::default())
    .await?;
```

Upgrade behavior:

1. Read the current migration version.
2. Resolve the path from that version to the target stage.
3. Validate the baseline when `require_clean_schema` is true.
4. Introspect the live schema once. Plan the first pending stage from it, and
   each later stage from the previous stage's target.
5. Plan, classify and risk-check every pending stage, and compare
   `expected_current_schema_hash` with the live baseline, before any stage SQL
   runs. A refused later stage leaves earlier stages unapplied.
6. Render backend SQL.
7. Apply each stage transactionally.
8. Record migration history.

## Schema Stages

`SchemaStageRunner` applies ordered `SchemaStage`s directly on a `Database`,
tracking each applied stage version in migration history. It shares the step
classification and risk gate of `apply_migration`.

```rust
use graphql_orm::graphql::orm::{ApplyOptions, SchemaStage, SchemaStageRunner};

let stages = vec![
    SchemaStage::from_entities("1", "initial schema", &[User::metadata()]),
    SchemaStage::from_entities("2", "add audit fields", &[User::metadata(), Audit::metadata()]),
];

for stage in database.plan_schema_stages(&stages).await? {
    println!("{} {:?}", stage.version, stage.highest_risk());
}

// Refuses destructive steps, like `ApplyOptions::default()`.
database.apply_schema_stages(&stages).await?;

// Explicit options, for example after reviewing a plan that drops a column.
database
    .apply_schema_stages_with_options(
        &stages,
        ApplyOptions {
            allow_destructive: true,
            ..ApplyOptions::default()
        },
    )
    .await?;
```

- Every pending stage is planned, classified and risk-checked before any stage
  SQL runs. A refusal names the stage version and step, and leaves all pending
  stages unapplied.
- The first pending stage is planned from the live schema, limited to the
  tables that some stage declares. A table or column of those that is absent
  from the stage target, such as a newer release's schema after a rollback,
  appears as a destructive step and is refused by default. A live table that
  no stage declares is not the stages' to drop and is left alone.
- `additive_only` refuses any step that is not additive.
- `dry_run` returns one report per pending stage without running stage SQL;
  planning may still prepare the migration ledger table.
- `expected_current_schema_hash` must equal the stable hash of the introspected
  live schema.
- `record_history: false` is refused, because stage progress is tracked in
  history. `require_clean_schema` applies to ABI upgrades only.
- `PlannedSchemaStage::steps()` and `highest_risk()` expose the classified steps
  for review. Each stage classifies against `PlannedSchemaStage::source_schema`:
  the introspected live schema of the stage-declared tables for the first
  pending stage, and the previous stage's target for each later one.

## Append-only retention capability changes

`append_only` remains strict unless an entity explicitly declares a dedicated
`retention_purge` policy key. That opt-in is structural schema state: it changes
the SQLite DELETE trigger and reserved context-table contract, or the
PostgreSQL trigger function and managed RLS policy. It is included in stable
table, module, and backup fingerprints.

Enabling, disabling, or repairing the capability produces an explicit
`SetAppendOnly` step. Enabling purge on a table that is already append-only
relaxes its enforcement and needs `allow_destructive`. Apply it only under a fresh host/module migration version;
if a previously recorded version still has such work, application fails closed.
No row data is rewritten, but operators must validate foreign-key behavior and
existing data before enabling physical purge. See
[Bounded append-only retention maintenance](../../operations/runbooks/retention-maintenance.md).

## Migration History

The history table is `__graphql_orm_migrations`.

Existing columns are preserved:

- `version`
- `description`
- `applied_at`

Newer migrations may also record:

- `backend`
- `graphql_orm_version`
- `source_schema_hash`
- `target_schema_hash`
- `plan_hash`
- `policy`

Existing rows without the newer metadata remain valid.

## Backend Support

SQLite and Postgres implement validation, planning, and migration application.

For PostgreSQL spatial fields, managed plans include `CREATE EXTENSION IF NOT EXISTS postgis` before
table creation. Spatial indexes are rendered as normal migration statements, not concurrent index
builds.

PostgreSQL reports each built-in type under one name (`character varying(50)`,
`integer`, `numeric(10,2)`, `timestamp with time zone`). A model may declare
any alias of it (`varchar(50)`, `int`, `decimal(10,2)`, `timestamptz`): the
planner and the foreign-key member check treat an alias and the reported name
as the same type, so a freshly applied schema plans no change. Only a closed
set of built-in names is recognised. Length, precision and scale still
distinguish types, and domains, extension types and quoted type names are
compared by spelling.

PostgreSQL introspection reads the tables of the connection's current schema.
A table an entity declares with a schema (`table = "audit.logs"`) is read as
well, wherever it lives, when the target being planned or validated names it,
and is reported under its declared path. This applies to
`plan_migration_to_entities`, `plan_schema_target`, `validate_against_entities`,
`validate_target`, schema stages and ABI upgrades. Two limits follow from
reading only what the target names:

- tables of other schemas that the target does not name are never read, so no
  plan, strict or not, drops them;
- a schema-qualified table that is removed from the target is no longer read
  either, so the plan does not drop it. Drop it with explicit SQL.

`introspect_postgres_schema` on its own still reads the current schema only.

A PostgreSQL plan drops and re-adds a foreign key when its source column and
the column it references both change type, because the first `ALTER COLUMN
TYPE` would otherwise fail on mismatched members; re-adding the key rechecks
every referencing row. The key stays attached when both members only widen
within one base type: a longer `varchar` or `varbit`, or a `numeric` with more
precision at the same scale. PostgreSQL rewrites neither table for those
changes. MySQL plans follow the same foreign-key ordering as PostgreSQL and SQL
Server plans but have no executable lane.

PostgreSQL uniqueness introspection is structural. `pg_constraint.conkey`
members are grouped in key ordinal order: a one-column UNIQUE constraint marks
that column unique, while a multi-column constraint populates the table's
composite-unique model without making each member independently unique.
Indexes owned through `pg_constraint.conindid` (including PRIMARY KEY and
UNIQUE backing indexes) are excluded from ordinary secondary indexes.
Explicit unique indexes and conditional/partial unique indexes remain ordinary
indexes. This distinction prevents unchanged constraint-owned indexes from
being planned as `DropIndex` during an additive complete-target upgrade.

Compound foreign-key introspection is structural as well. SQLite groups
`PRAGMA foreign_key_list` rows by constraint `id` and orders members by `seq`;
PostgreSQL pairs `pg_constraint.conkey` and `confkey` members by ordinal.
Ordinary index direction comes from SQLite `index_xinfo` and PostgreSQL
`indoption`, so an unchanged compound relation or descending key replans to an
empty migration.

For SQLite spatial fields, managed plans create `TEXT` columns that store GeoJSON. SQLite plans do
not enable PostGIS and do not create spatial indexes. Introspection sees those columns as `TEXT`; the
planner treats the declared spatial metadata as compatible with that storage so repeated validation
does not rebuild the table just because the target entity marks the field as spatial.

MSSQL schema management remains read-only: SQL Server does not implement migration application,
managed schema mutation, or backup capability traits. This is independent of entity DML;
an explicitly writable Tiberius pool with `SchemaPolicy::ExternalWritable` may mutate rows in an
externally managed schema while schema planning/application remains unavailable.
