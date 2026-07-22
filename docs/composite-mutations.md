# Typed Composite-Key and Bounded Mutations

`graphql-orm` 0.6 adds an opt-in repository-only write surface for entities whose identity is a
natural composite primary key. The API uses generated Rust values and filter inputs throughout;
applications do not supply SQL, column names, pools, executors, or backend transaction types.

## Declaration

```rust
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize)]
#[graphql_entity(
    table = "oauth_states",
    plural = "OauthStates",
    repository_mutations = true,
    default_sort = "provider_name ASC, state_hash ASC",
    upsert = "provider_name,state_hash",
    unique_composite = "provider_name,state_hash",
    write_policy = "oauth_states.write"
)]
struct OauthState {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    provider_name: String,

    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "bytes")]
    #[sortable]
    state_hash: Vec<u8>,

    #[filterable(type = "number")]
    consumed_at: Option<i64>,
}
```

The opt-in requires at least two explicitly declared, persisted, non-null, host-supplied primary
key fields. Key order is declaration order. Nullable, generated, skipped, relation, read-only, and
MSSQL key configurations fail during macro expansion. Existing composite read-only entities do not
change unless `repository_mutations = true` is added.

The macro generates `OauthStateKey`, `CreateOauthStateInput`, and `UpdateOauthStateInput`. These
types and their methods are repository-only. No GraphQL create/update/delete/upsert field is added.

## Complete-key operations

```rust
let key = OauthStateKey {
    provider_name: "oidc".to_string(),
    state_hash: digest_bytes,
};

let row = OauthState::find_by_key(&database, &key).await?;
let updated = OauthState::update_by_key(&database, &key, update).await?;
let deleted = OauthState::delete_by_key(&database, &key).await?;
```

The same operations are available as `MutationContext::find_by_key`, `update_by_key`, and
`delete_by_key`. Complete-key updates/deletes validate that at most one row was affected and fail
closed otherwise. Identifiers are dialect-quoted and values are always bound parameters.

## Insert-if-absent and upsert

`insert_if_absent` uses the configured `upsert` conflict target, or the complete primary key when
no separate target is configured:

```rust
match OauthState::insert_if_absent(&database, input).await? {
    InsertIfAbsentOutcome::Inserted(row) => { /* this call inserted it */ }
    InsertIfAbsentOutcome::AlreadyPresent(row) => { /* target already existed */ }
}
```

The database performs `INSERT ... ON CONFLICT ... DO NOTHING`; there is no read-then-insert race.
An existing row must be visible and writable under row policy/RLS before it is returned. Generated
`upsert` and `MutationContext::upsert` use the same private Rust create input even when conflict
fields are absent from public GraphQL inputs.

Input transforms run before conflict evaluation. A before-create mutation hook can run on the
losing side of a concurrent insert race, but no after hook, change event, search update, or deferred
post-commit action is queued for `AlreadyPresent`.

## Atomic predicate update

`update_if` combines the complete key and a non-empty generated `WhereInput` in one `UPDATE`
statement. This supports one-time nullable-state transitions without host SQL:

```rust
let outcome = OauthState::update_if(
    &database,
    &key,
    OauthStateWhereInput {
        consumed_at: Some(IntFilter {
            is_null: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    },
    UpdateOauthStateInput {
        consumed_at: Some(Some(now)),
    },
).await?;

match outcome {
    PredicateUpdateOutcome::NotFound => {}
    PredicateUpdateOutcome::PredicateConflict => {}
    PredicateUpdateOutcome::Updated(row) => {}
}
```

Predicates requiring residual in-memory evaluation are rejected. Zero affected rows are classified
with a key lookup after the atomic statement; more than one affected row is an internal invariant
failure.

## Bounded bulk replacement

Single-key and opted-in composite-key entities generate bounded variants alongside the legacy
bulk helpers:

```rust
let maximum = MutationLimit::new(32)?;
let outcome = UserRole::delete_where_bounded(&database, filter, maximum).await?;
```

`BoundedMutationOutcome::LimitExceeded` performs no mutation. `Applied { affected }` reports the
exact number changed. Empty filters and zero limits are rejected. Inside a state-machine
transaction, bounded deletes followed by inserts provide atomic grant-set replacement.

The generated implementation performs a deterministic complete-primary-key
selection of exactly `MutationLimit + 1` rows. This narrow mutation sentinel is
not resolved through `PageInput` or public pagination configuration, so limits
of 100 and above remain exact. It does not add an uncapped repository or
GraphQL read API: ordinary page, connection, repository, and runtime-query
limits remain unchanged. A predicate requiring residual/in-memory evaluation
is rejected before selection, hooks, events, notifications, or writes. After
selection, the mutation must affect the same cardinality or the complete
transaction fails closed.

## Authorization and transactions

All new writes run through entity and row authorization, write transforms, mutation hooks, search
maintenance, queued change events, and ORM commit handling. `DeclaredPoliciesRequired` remains
fail-closed. Opted-in composite repository mutations additionally require a registered
`EntityPolicy` provider even under the legacy authorization mode, so the new surface is never
default-allow. PostgreSQL `transaction_with_auth` installs the normal transaction-local RLS settings.
Callback error, cancellation, panic, constraint failure, and commit failure use the existing ORM
transaction rollback behavior; queued events/actions are released only after a successful commit.

SQLite and PostgreSQL are supported write backends. MSSQL remains intentionally read-only and
rejects `repository_mutations = true` at compile time.
