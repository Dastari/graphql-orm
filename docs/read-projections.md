# Typed Read Projections

Typed projections are repository-only DTOs generated from an exact subset of one managed entity.
They are intended for least-privilege reads where fetching an excluded column—even if it is later
discarded—would be unacceptable.

## Declaration

Declare one or more projections on a `GraphQLEntity`:

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

An application `RowPolicy` receives a full Rust entity. Evaluating it would require selecting every
field, defeating a projection's memory boundary, so projection reads fail closed whenever an
application row-policy provider is registered. Use PostgreSQL RLS or an explicit generated typed
filter for projection-compatible tenant/soft-delete enforcement. Filters requiring residual
in-memory entity evaluation are likewise rejected rather than silently loading excluded columns.

Projections and their methods are never added to GraphQL schemas. `private = true` is the only
supported mode in this release; `private = false` is a compile error. Existing GraphQL field-policy
and naming behavior is therefore unchanged.

## Schema migration

A projection changes no table, index, trigger, RLS, or stable schema hash. Upgrading requires no DDL
migration. Add the declaration, update callers to the generated DTO, and retain normal managed
schema validation. Existing full-entity repository APIs remain source-compatible.
