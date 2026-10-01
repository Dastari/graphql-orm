---
title: Owned runtime migration targets
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-10-01
review_by: 2027-02-01
supersedes: []
---

# Owned runtime migration targets

ORM/macros 0.34.0 adds migration targets from the existing validated runtime
schema on SQLite and PostgreSQL. These APIs are separate from schema activation,
catalog persistence, public revision selection, authorization policy and transport,
which remain host responsibilities. Dynamic GraphQL and runtime record mutations
are later capabilities. No feature beyond the selected database backend is required.

## Public host flow

```rust,ignore
use graphql_orm::graphql::orm::*;

let schema = std::sync::Arc::new(catalog_definition.validate()?);
let target = schema.physical_schema::<SqliteBackend>(RuntimeMigrationLimits::default())?
    .with_static_entities(&[HostJournal::metadata()])?;
let ownership = ManagedTableSet::new([
    "notes".to_owned(), "host_journal".to_owned(),
])?;
let manager = database.schema();
let plan = manager.plan_owned_migration(
    "schema-42", "reviewed notes target", &target, &ownership, PlanOptions::strict(),
).await?;
for step in plan.steps() {
    inspect(step.table_name(), step.risk(), step.reason());
}
inspect_sql(plan.statements());
// Separate explicit host decision; ordinary destructive/additive guards still apply.
let report = manager.apply_owned_migration(&plan, ApplyOptions {
    additive_only: true,
    expected_current_schema_hash: Some(plan.source_schema_hash().to_owned()),
    ..Default::default()
}).await?;
assert!(!report.dry_run);
```

The compiled [disposable host example](../../../crates/graphql-orm/examples/runtime_owned_migration.rs)
uses an application-generated UUID key, previews by default and applies only when
invoked with `--apply`:

```sh
cargo run -p graphql-orm --no-default-features --features sqlite \
  --example runtime_owned_migration
cargo run -p graphql-orm --no-default-features --features sqlite \
  --example runtime_owned_migration -- --apply
```

Both commands use an in-memory database owned by that process. PostgreSQL host
code selects `PostgresBackend` and a PostgreSQL `Database`; regression tests use
labelled disposable containers, never an ambient application connection.

## One canonical physical contract

`OwnedSchemaModel`, `OwnedTableModel`, and `IndexModel` own the same canonical
physical definitions as `SchemaModel`, `TableModel`, and `IndexDef`. Private
borrowed views adapt storage into one validation/hash/diff/classification/render
engine. Runtime conversion and owned live introspection never leak strings or
index arrays. Static compatibility adapters retain their existing static index
storage convention.

The legacy public `MigrationStep` remains an actual enum. Variant and glob imports,
qualified constructors, exhaustive matches, inferred values, existing struct
literals and function pointers remain valid. The external
[migration compatibility fixture](../../../crates/graphql-orm/tests/fixtures/migration-api-compatibility/src/lib.rs)
compiles these call sites without depending on implementation internals. Legacy
physical and plan hash formats are retained. Owned plan integrity additionally
binds version, description, complete live physical source hash, target hash and
explicit ownership. Hashes are drift detectors, not signatures or authorization.

Public collection/field names are diagnostic/query identities. Physical table and
column names determine DDL. Changing public names alone never plans physical
renames or record deletion. Equivalent representable static/owned targets use the same SQL/risk
semantics and replan to an empty step/statement list after application in either
origin direction. SQLite introspection respects primary-key member ordinals,
including a composite key whose order differs from column declaration order.

## Ownership, composition and dependencies

`ManagedTableSet` is trusted host state. Its portable table identifiers are unique,
case-sensitive and exclude ORM-reserved infrastructure. Every target table must
belong to it. Keep deliberately removed tables in the set until their drop is
reviewed; dropping them is destructive. Removing a name from the set relinquishes
ownership and cannot authorize a drop. `PlanOptions::strict` includes all owned
live tables. `managed_tables_only` retains the legacy option's narrower behavior:
it ignores owned live tables absent from the target. Neither option includes
unowned live tables as drop candidates.

`with_static_entities` composes host system metadata, resolves relation target
labels against the complete physical target and rejects duplicate names or invalid
keys/FKs. Hosts can bind static relation metadata to stable physical table names
when the runtime's public type name may change. Ordinary static policy metadata
stays on the host entity declaration; this method does not create or relax policies.

Live introspection reads the full database schema used by the canonical backend
contract. Unowned incoming FKs are checked before permitting changes to referenced
owned definitions. Initial preservation checks are conservative: changes to a
referenced definition with an unowned incoming FK are rejected; an unchanged
definition can gain indexes. Compose host-owned system tables explicitly to plan
changes to the owned dependency graph together. No `CASCADE` DDL bypass is added.

Owned runtime validation supplements the legacy FK model with live capability
checks. `UnsupportedForeignKey` identifies the physical source table and SQLite
source column or PostgreSQL constraint in `subject`; `collection` is absent for
catalog sources that need not belong to the runtime schema. A modifying
`ON DELETE SET DEFAULT` is never classified as `Restrict` for a runtime dependency
certificate. Initial owned support accepts explicit delete `RESTRICT`, `CASCADE`
and `SET NULL`, default update `NO ACTION`, and non-deferrable constraints.
Delete `NO ACTION` is rejected because its constraint-check timing differs from
`RESTRICT`; nondefault update actions and any deferrable FK are also rejected.
The static physical model and static migration behavior remain unchanged.

Checks include relevant incoming sources from unowned/system/ORM tables. Unrelated
unsupported FKs outside the owned dependency graph do not block planning or change
those tables. SQLite reads action metadata from its FK PRAGMA and scans unquoted,
non-comment DDL words for deferral; if a relevant source table contains a deferrable
FK, it conservatively rejects that table. PostgreSQL reads `pg_constraint`, including
cross-schema incoming sources, and rejects cross-schema bindings, non-simple match
rules, unvalidated constraints and partial-column delete actions that the canonical
model cannot represent. SQLite physical names with case variants of owned names are
also rejected rather than losing dependency identity.

These checks run during read-only validation/planning and mutation-environment
certification, and again on the pinned apply transaction before target DDL/history
success. The legacy source hash omits reserved infrastructure tables and omitted FK
attributes, so that hash alone is insufficient for these checks. Unsupported FK
adapters or extended canonical FK semantics remain separate follow-ups; hosts may
surface this diagnostic in capability/admin previews.

After applying and replanning to no-op, hosts can obtain a
`RuntimeMutationEnvironment` through `runtime_mutation_environment(Arc<ValidatedRuntimeSchema>,
&target, &ownership)`. It verifies that the schema matches the live composed
physical target and records incoming ordered FK bindings/actions from all physical
sources, including system and ORM infrastructure tables absent from the runtime
IR. This immutable environment is a physical dependency certificate for later
runtime writes, not another schema IR or an authorization grant. Hosts must fence
external DDL and revalidate after drift. The host's complete public/policy revision
remains distinct from structural ORM fingerprints; validate policy-only revision
changes before future mutations.

## Read-only planning and guarded application

`validate_owned_schema` and `plan_owned_migration` only read catalog state. They do
not create migration history, clean SQLite rewrite tables, apply SQL or reconcile
RLS. Capability, target bounds, ownership, policy and unsupported preservation
checks produce structured diagnostics where applicable.

`apply_owned_migration` accepts an immutable plan under `SchemaPolicy::Managed`.
It enforces backend/integrity/ownership binding, `expected_current_schema_hash`,
`additive_only`, and explicit `allow_destructive`. `dry_run` performs no I/O.
Owned plans always require their pinned baseline: `require_clean_schema = false`
does not weaken this binding. A changed live physical source requires a new plan.

Application rechecks the baseline on the pinned transaction before DDL. SQLite
uses `BEGIN IMMEDIATE`; controlled rebuilds check FKs before commit and restore
connection state. An operation guard closes a connection whose FK enforcement was not restored,
so cancellation cannot return that state to the pool. Successful restoration
disarms the guard and retains the connection, including a one-connection in-memory
database. PostgreSQL uses a cooperative
migration advisory lock plus locks on existing owned tables. Hosts must coordinate
DDL issued outside ORM migration execution. DDL and the successful history row
commit together. A recorded version is an already-applied success only after a
fresh no-op replan; nonempty reuse is rejected. Database commit errors remain
errors; do not blindly retry an ambiguous outcome.

Owned apply never invokes RLS reconciliation with an empty target. PostgreSQL RLS
flags, policies, table ownership/grants, host helper functions and unrelated tables
remain intact. Removal/type changes on existing RLS-protected definitions are
rejected when preservation cannot be established. SQLite controlled rebuilds
reject host triggers, view preservation cases and reserved temporary-name collisions
instead of running the legacy global stale-table cleanup. Managed expression,
non-default-collation and unsupported partial indexes are rejected instead of
losing semantics through catalog interpretation. PostgreSQL index INCLUDE members,
non-default operator classes/null ordering/storage options and unsupported methods
are also rejected. SQLite plans that would drop an existing shared retention
context are rejected to preserve infrastructure serving other tables.

## Scalar/default and capability limits

Mapping preserves boolean, signed 64-bit integer, finite float, string, UUID, JSON,
bytes and UTC datetime logical storage, ordered keys, unique/index definitions,
relations/delete actions, append-only and retention semantics. An application-
generated UUID has no database identity/default expression. Generated non-UUID
fields need a supported default; otherwise `UnsupportedGeneration` identifies the
collection/member. Literal defaults must retain their validated type semantics.
Integer `CurrentTimestamp` uses backend epoch seconds. SQLite datetime defaults
emit UTC RFC 3339 with six fractional digits (`strftime` has millisecond resolution,
so the final three are zero); PostgreSQL uses its native timestamp default. SQL
NULL and JSON null semantics remain those of the runtime record API.

Static-to-runtime conversion rejects legacy epoch-second defaults on fields
marked DateTime with a scoped `RuntimeSchemaDiagnosticCode::UnsupportedDefault`.
The diagnostic identifies the collection and field and explains the incompatible
storage semantic. In particular, SQLite `#[date_field] created_at: String` keeps
its existing static `unixepoch()` default; it is never reinterpreted as RFC3339.
Supported Integer timestamp fields keep epoch-second defaults. For representable
static datetime declarations, use an explicitly compatible default or suppress
the legacy implicit default with `#[graphql_orm(default = false)]`. Suppression
changes that declaration's target and is not an automatic existing-data migration.
A legacy datetime adapter remains a separate follow-up.

`RuntimeMigrationLimits` bounds collections, per-table fields/indexes/relations,
key arity, default bytes, total target bytes and rendered plan steps/statements/bytes.
Custom limits remain attached to the owned target through composition and planning.
Default bounds are 512 tables, 256 fields, 128 indexes/relations, 32 key members,
64 KiB per default, 8 MiB per target/plan, 16,384 steps and 32,768 statements.

MSSQL and unknown backend capabilities reject runtime migration conversion,
planning and apply before connection acquisition. Existing static APIs and MSSQL
read/write/schema-policy contracts remain unchanged. Unsupported runtime IR
semantics continue to be rejected by its existing validator/conversion; do not
substitute arbitrary SQL fragments. The dependency environment does not implement
record writes, cascade mutations, catalog activation or durable event replay.
