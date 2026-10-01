---
title: "Typed Read Projections"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-10-01
review_by: 2027-02-01
supersedes: []
---

# Typed Read Projections

Typed projections are repository-only DTOs generated from an exact subset of one managed entity.
They are intended for least-privilege reads where fetching an excluded column—even if it is later
discarded—would be unacceptable.

## Declaration

Declare one or more projections on a `GraphQLEntity` or private `RepositoryEntity`:

```rust
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(table = "ca_certificates", plural = "CaCertificates")]
#[graphql_orm(projection(
    name = "PublicCertificateInventory",
    fields = [
        id,
        role,
        serial,
        spki_digest,
        pem,
        parent_id,
        issued_at
    ],
    private = true
))]
struct CaCertificate {
    #[primary_key]
    #[filterable(type = "uuid")]
    id: uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    role: String,
    #[unique]
    #[filterable(type = "string")]
    serial: String,
    spki_digest: Vec<u8>,
    pem: String,
    parent_id: Option<uuid::Uuid>,
    #[sortable]
    issued_at: i64,

    #[graphql_orm(private, sensitive)]
    #[backup(redact)]
    private_key_enc: String,
}
```

`PublicCertificateInventory` receives the exact Rust field types and nullability from
`CaCertificate`. `private_key_enc` is absent from both the generated `SELECT` list and the DTO, so it
is never returned by the driver or deserialized into process memory. Selecting a sensitive field in
a different projection is an explicit declaration; its generated `Debug` implementation prints
`[redacted]` for fields marked `sensitive` or `#[backup(redact)]`.

Projection names and fields are checked during macro expansion. Empty projections, duplicates,
unknown/non-persisted fields, schema-only entities, unsupported backends, and `private = false` are
rejected. Because the DTO is generated from the entity rather than supplied by the application, a
different DTO field type cannot be declared.

## Repository reads

```rust
let by_id = PublicCertificateInventory::find_by_id(&database, &certificate_id).await?;
let by_serial = PublicCertificateInventory::find_by_serial(&database, &serial).await?;

let public = PublicCertificateInventory::query(&database)
    .filter(CaCertificateWhereInput {
        role: Some(StringFilter {
            eq: Some("intermediate".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    })
    .order_by(CaCertificateOrderByInput {
        issued_at: Some(OrderDirection::Desc),
        ..Default::default()
    })
    .limit(50)
    .fetch_all()
    .await?;
```

Generated primary-key and single-column `#[unique]` helpers return `Option<Projection>`. The typed
builder provides `filter`, `order_by`, `limit`, `fetch_all`, `fetch_first`, and
`fetch_optional_one`. Lists always apply the database's configured default and maximum page bounds
before execution. Ordering appends every primary-key column as a deterministic tiebreaker.

## Transaction-bound reads

```rust
database.transaction(TransactionMode::StateMachine, |transaction| {
    Box::pin(async move {
        let inserted = transaction.insert::<CaCertificate>(input).await
            .map_err(OrmPublicError::from)?;

        transaction
            .project::<PublicCertificateInventory>()
            .filter(CaCertificateWhereInput {
                id: Some(UuidFilter {
                    eq: Some(inserted.id),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .fetch_optional_one()
            .await
            .map_err(OrmPublicError::from)
    })
}).await?;
```

`Projection::find_by_id_in(transaction, ...)`, `find_by_key_in`, and generated unique-field `_in`
helpers are also available. These reads use the active ORM transaction and observe its earlier
writes. No pool, executor, row, SQL string, or backend database type appears in application code.

## Authorization and GraphQL

Projection reads call the entity's normal repository `read_policy` decision. Declared-policy and
explicit-policy modes therefore fail closed exactly as full-entity repository reads do.
`query_with_auth` uses backend-neutral `DbAuthContext`, and `transaction_with_auth` installs the
same transaction-local PostgreSQL settings used by generated RLS reads.

With a global `RowPolicy`, projections use its current `read_visibility` decision
for the entity and `Repository` surface on every call:

| Visibility | Projection/group-page behavior |
| --- | --- |
| `Unrestricted` | Explicitly permit all matching rows; retain entity/selected-field checks. |
| `Complete(predicate)` | Validate entity/backend and parameterize the typed predicate before ordering/limits/grouping. No residual callback. |
| `CallbackOnly` (default), `Prefilter(predicate)` | Reject before query I/O: evaluating a full entity would violate the excluded-column boundary. |

For example, a global provider can deliberately mark public inventory unrestricted,
or return `ReadVisibility::Complete(ReadPredicate::from_filter::<SqliteBackend, _>(
&PrivateIssuerWhereInput { tenant: Some(StringFilter { eq: Some(verified_tenant),
..Default::default() }), ..Default::default() })?)` for a verified tenant. Never
mark a partial predicate complete. Reapply current policy/ownership each request;
previous projection values and cursors do not grant access. Filters requiring
residual in-memory evaluation remain rejected. Repository field checks still run
without a full-record value; policies needing that value must fail closed.

Pool and pinned-transaction projections have the same behavior, including generated
primary/unique-key helpers. SQLite and PostgreSQL generated projections execute this
contract; PostgreSQL `DbAuthContext`/RLS handling remains unchanged. Generated
MSSQL projections are still unsupported; this change does not add that macro profile.
Complete group pages remain SQLite-only, independently of projection support.

Generated projections supply the associated entity's identity to validate SQL
visibility. The provided `ReadProjection::entity_type_id` defaults to `None` for
existing handwritten implementations, keeping them source compatible. Those
implementations can use unrestricted reads; complete visibility requires returning
`Some(TypeId::of::<Self::Entity>())`. An absent/mismatched identity fails closed.

Run the standalone dependency-minimal private repository example (no application
query SQL, GraphQL roots or direct async-graphql dependency):

```sh
cargo run --manifest-path crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/Cargo.toml --locked --no-default-features --features sqlite --example policy_projections
cargo test -p graphql-orm --locked --no-default-features --features sqlite --test projection_visibility
cargo test -p graphql-orm --locked --no-default-features --features postgres --test projection_visibility
```

The PostgreSQL command creates and removes its own labelled disposable container
using `tests/support/owned_postgres.rs`; it never consumes an application URL and
fails if infrastructure is unavailable. Regression views raise an error if the
excluded private-key expression is selected, proving the projection boundary.
The observer records actual bounded projection SELECTs without bind values.

Projections and their methods are never added to GraphQL schemas. `private = true` is the only
supported mode in this release; `private = false` is a compile error. Existing GraphQL field-policy
and naming behavior is therefore unchanged.

## Schema migration

A projection changes no table, index, trigger, RLS, or stable schema hash. Upgrading requires no DDL
migration. Add the declaration, update callers to the generated DTO, and retain normal managed
schema validation. Existing full-entity repository APIs remain source-compatible.
