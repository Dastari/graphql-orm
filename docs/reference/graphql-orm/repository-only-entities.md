---
title: "Repository-only entities"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-10-02
review_by: 2027-02-01
supersedes: []
---

# Repository-only entities

`RepositoryEntity` is the explicit persisted-but-not-GraphQL entity mode. It
uses the same managed-schema metadata, generated filters and ordering, typed
projections, repository operations, portable transactions, authorization,
hooks, search maintenance, events, constraints, backup metadata, and backend
implementations as a normal derived entity. It deliberately implements no
async-graphql object or input types and generates no resolver or schema-root
types.

Choose among the three declaration surfaces as follows:

| Mode | Use when | Generated database API | Generated GraphQL API |
| --- | --- | --- | --- |
| `GraphQLSchemaEntity` | A crate owns schema metadata only | Metadata/row primitives only | None |
| `RepositoryEntity` | Trusted Rust code needs typed persistence without GraphQL exposure | Typed reads, inputs, projections, and applicable mutations | None |
| `GraphQLEntity` + `GraphQLOperations` | The entity participates in generated GraphQL | Typed repository and generated mutations | Objects, inputs, resolvers, roots |

## Declaration and generated types

```rust
use graphql_orm::prelude::*;

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "sqlite",
    table = "credentials",
    plural = "Credentials",
    default_sort = "username ASC"
)]
#[graphql_orm(projection(
    name = "CredentialLookup",
    fields = [id, username, status],
    private = true
))]
struct Credential {
    #[primary_key]
    id: String,

    #[unique]
    #[filterable(type = "string")]
    #[sortable]
    username: String,

    #[filterable(type = "string")]
    status: String,

    #[graphql_orm(private, sensitive, write_policy = "credential.secret.write")]
    secret_hash: Vec<u8>,

    #[graphql_orm(version, default = "0")]
    version: i64,
}
```

This emits the canonical `Credential`, `CredentialWhereInput`,
`CredentialOrderByInput`, `CreateCredentialInput`, `UpdateCredentialInput`,
`CredentialLookup`, schema/row implementations, and repository methods. These
are ordinary Rust types. None implements async-graphql `Object`, `OutputType`,
`InputObject`, or `InputType`; no query, mutation, subscription, connection,
payload, or schema-root type is emitted. `schema_roots!`, `GraphQLOperations`,
and `GraphQLRelations` reject a repository-only declaration at compile time.

Unlike a public GraphQL write input, the repository create/update types include
writable persisted `private` and `sensitive` fields. They preserve the existing
generated omitted-versus-null representation, database defaults, transforms,
and database-managed version behavior. Their generated `Debug`, as well as
projection `Debug`, prints `[redacted]` for sensitive fields.

Sensitive mutation-hook snapshots contain redacted JSON and cannot be
downcast back to the original entity. Generated repository change events omit
their entity payload when any field is sensitive, retaining action/key/source
metadata without copying the protected value into the event bus. Hooks can
still deliberately inspect and transform the typed create/update input in the
normal before-write hook phase.

## Host-managed Integer timestamps

The legacy convention omits Rust fields named `created_at` and `updated_at`
from typed create/update inputs. Updates also assign epoch seconds whenever a
persisted physical column is named `updated_at`, including Rust aliases. Changing
`default = "0"` or `default = false` alone does not disable that write behavior.

Repository entities can opt each conventional timestamp field into ordinary
host-supplied writes:

```rust,ignore
#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(backend = "sqlite", table = "clocked_records", plural = "ClockedRecords")]
struct ClockedRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: graphql_orm::uuid::Uuid,
    label: String,
    #[graphql_orm(timestamp = "host", default = "0")]
    created_at: i64,
    #[graphql_orm(timestamp = "host", db_column = "updated_at", default = "0")]
    modified_ms: i64,
}
```

`timestamp = "host"` supports persisted `i64` and `Option<i64>` fields where
either the Rust name or physical column is `created_at` or `updated_at`.
It disables both typed-input exclusion and automatic seconds assignment for
that field. Supplied signed integers are bound unchanged: the ORM neither
selects a clock nor interprets/rescales their units. Mixed host-managed and
legacy fields retain their independent behavior.

Non-null writable create fields are required even with a database default.
Updates use `Option<i64>`: `None` preserves the existing value. Nullable fields
use `Option<i64>` on create and `Option<Option<i64>>` on update, distinguishing
omission (`None`), SQL NULL (`Some(None)`), and a value (`Some(Some(value))`).
Normal restrictions, constraints, policies, input transforms, hooks, and
redacted change metadata still apply. Host management grants no authority.

Host-managed `created_at` is ordinarily writable, including during an upsert
conflict update. It is **not immutable**; use existing restrictions and field
policies for immutability. Upsert accepts the existing complete create input,
while insert-if-absent leaves a conflicting existing row unchanged. No new
upsert, CAS, conflict, or bulk-write semantics are introduced. Direct and
transaction-bound insert, ID/key update, conditional/CAS update, bounded update,
upsert, and insert-if-absent use the same field decision where those operations
are supported by the declaration/backend.

The annotation changes no column/default/schema metadata. Explicit defaults and
legacy implicit epoch-second defaults remain intact, and adding the annotation
alone replans to no-op on SQLite/PostgreSQL. Typed creates provide the host value
explicitly; other writers that omit the column can still invoke its unchanged
database default. Removing a default remains a separate schema change.
`default = false` also works on an opted-in alias of a conventional physical
column. Static-to-runtime conversion retains `Integer` and its default; this
annotation is not a runtime write mode or a DateTime adapter. It neither implements
nor changes the separate owned migration contract. That contract's approved
rejection of legacy epoch-second DateTime storage, supported Integer epoch-second
defaults, and canonical runtime DateTime semantics remain independent requirements.

Whole-transaction cancellation and propagated hook/journal failures retain the
existing rollback and commit-before-event guarantees. **Catching an inner
mutation timeout/error and returning `Ok` does not guarantee rollback**: the
static transaction context has no unfinished-operation poisoning invariant.
Cancel the whole transaction or propagate the error. A lost commit response
remains ambiguous; do not blindly retry. This capability does not implement the
stronger runtime mutation contract.

The annotation is rejected on GraphQL/schema-only derives, unrelated names,
non-Integer representations (including `date_field`), relations, skipped database
fields, primary keys, version fields, and explicitly generated fields. Ordinary
`write = false` restrictions remain supported. Existing GraphQL SDL/automatic
behavior and unannotated repository declarations remain unchanged. MSSQL keeps
its existing external schema/write gates; the annotation does not authorize
schema management or enable otherwise unavailable operations.

The standalone [host timestamp example](../../../crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/examples/host_timestamps.rs)
uses no direct async-graphql dependency. SQLite runs it with:

```bash
CARGO_BUILD_JOBS=2 cargo run \
  --manifest-path crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/Cargo.toml \
  --locked --no-default-features --features sqlite --example host_timestamps
```

PostgreSQL regression/example execution creates test-owned Docker containers,
checks their ownership before cleanup, and never reads application connection
settings. Run backend builds sequentially:

```bash
CARGO_BUILD_JOBS=2 cargo test -p graphql-orm --locked \
  --no-default-features --features sqlite --test host_timestamps
CARGO_BUILD_JOBS=2 cargo test -p graphql-orm --locked \
  --no-default-features --features postgres --test host_timestamps \
  -- --ignored --test-threads=1
CARGO_BUILD_JOBS=2 cargo test \
  --manifest-path crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/Cargo.toml \
  --locked --no-default-features --features postgres --test host_timestamps \
  -- --ignored --test-threads=1
```

## Reads, writes, and transactions

Start bounded list reads with the generated Database-bound builder:

```rust,ignore
let credential = Credential::query(&database)
    .filter(CredentialWhereInput {
        username: Some(StringFilter {
            eq: Some("alice".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    })
    .default_order()
    .fetch_optional_one()
    .await?;
```

`fetch_all` and generated `find_all`/`find_many` apply the database default and
maximum page limits before execution. `fetch_optional_one` uses at most one
look-ahead row and fails if the predicate is not unique. Primary-key, complete
composite-key, unique-field, projection, bounded bulk mutation, insert,
insert-if-absent, upsert, CAS, append-only, retention, and keyset helpers are
generated under the same opt-in/backend rules as the existing operation
generator. Search-enabled declarations expose `search_db`, returning a bounded
`RepositorySearchQuery`; it returns ordinary `SearchHit<Entity>` values and
applies repository entity, row, and field policies without generating a
GraphQL search connection or resolver.

Repository-only credential and role entities compose directly inside one
portable transaction:

```rust,ignore
database.transaction(TransactionMode::StateMachine, |tx| {
    Box::pin(async move {
        let credential = tx
            .find_by_id::<Credential>(&credential_id)
            .await
            .map_err(OrmPublicError::from)?;
        let roles = tx
            .query::<UserRole>()
            .filter(UserRoleWhereInput {
                user_id: Some(StringFilter {
                    eq: Some(user_id.clone()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .default_order()
            .fetch_all()
            .await
            .map_err(OrmPublicError::from)?;
        Ok((credential, roles))
    })
}).await?;
```

SQLite `StateMachine` still acquires `BEGIN IMMEDIATE` before callback reads;
PostgreSQL still selects `SERIALIZABLE` before application statements and
installs transaction-local `DbAuthContext`/RLS settings through
`transaction_with_auth`.

## Authorization

Entity and row policy decisions use `EntityAccessSurface::Repository` and obey
the configured `AuthorizationMode`. A missing GraphQL request context is not
authority. `FieldPolicy` has separate `can_read_repository_field` and
`can_write_repository_field` callbacks; their source-compatible defaults deny
fields with declared repository policy keys. In
`DeclaredPoliciesRequired`, a declared key without a registered provider is a
safe `AUTHORIZATION_MISCONFIGURED` failure. In explicit-policy mode the normal
entity policy remains mandatory.

Projection reads authorize only their declared selected fields. Full-entity
reads authorize every persisted field. PostgreSQL RLS and auth-aware execution
remain active and are independent defense in depth.

Keyset pages evaluate the row policy for every returned entity and reject the
page if any edge is not visible. An opt-in total count is rejected while an
application `RowPolicy` is registered because counting rows outside the page
cannot be proven policy-safe in memory; use a database-visible typed tenant
predicate or PostgreSQL RLS for policy-safe counts.

## Federation

`federation_key` is rejected on `#[repository_entity(...)]` and on
`GraphQLSchemaEntity`. A resolvable Federation `@key` is produced by an
`#[graphql(entity)]` resolver on the generated queries object, and neither
surface has a GraphQL object to resolve or a queries object to host the
resolver. A repository-only entity that must also be a Federation entity has to
become a `GraphQLEntity` + `GraphQLOperations` entity; there is no way to
expose one through `_entities` without exposing its object type. See
[federation](federation.md).

## Storage and migration compatibility

The surface choice is not persisted metadata. Equivalent `RepositoryEntity`
and `GraphQLEntity` declarations produce the same `SchemaModel`, migration
plan, validation result, structural hash, schema-module identity, constraints,
indexes, RLS metadata, and backup descriptors. Changing only the code-generation
surface requires no DDL or data migration.

SQLite and PostgreSQL support the normal applicable read/write contract. MSSQL
supports static reads by default and the applicable repository DML contract
only for `schema_policy = "external_writable"` entities on an explicitly
writable Tiberius pool. Managed migrations, backup/restore, retention tables,
and managed search structures remain unsupported and fail at macro expansion
rather than silently dropping behavior.
