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

## Storage and migration compatibility

The surface choice is not persisted metadata. Equivalent `RepositoryEntity`
and `GraphQLEntity` declarations produce the same `SchemaModel`, migration
plan, validation result, structural hash, schema-module identity, constraints,
indexes, RLS metadata, and backup descriptors. Changing only the code-generation
surface requires no DDL or data migration.

SQLite and PostgreSQL support the normal applicable read/write contract. MSSQL
supports the existing static read-only contract; repository-only declarations
with write, upsert, append-only, retention, or mutation-hook options fail at
macro expansion rather than silently dropping behavior.
