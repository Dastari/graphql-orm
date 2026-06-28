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

Rendered SQL is available as `plan.statements`, but callers should treat it as an artifact of the plan rather than the migration source of truth.

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
    require_clean_schema: true,
    dry_run: false,
    expected_current_schema_hash: None,
    record_history: true,
}
```

Set `dry_run: true` to verify an application path without running statements. Destructive plans are rejected unless `allow_destructive` is explicitly enabled.

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
3. Introspect the live schema before each stage.
4. Validate the baseline when `require_clean_schema` is true.
5. Build a structured plan.
6. Reject disallowed risks.
7. Render backend SQL.
8. Apply statements explicitly.
9. Record migration history.

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

MSSQL is read-only in this phase. It can be used for queries and read-only validation where supported, but it does not implement migration application or write capability traits.
