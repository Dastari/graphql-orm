# Getting Started

`graphql-orm` is used from application crates through the runtime crate:

```toml
[dependencies]
graphql-orm = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.7.1", default-features = false, features = ["sqlite"] }
```

Exact full-revision Git dependencies are the supported installation method; the crates are not
published to crates.io.

The proc macros are re-exported by `graphql-orm`, so application code should usually import the
prelude:

```rust
use graphql_orm::prelude::*;
```

## Define An Entity

```rust
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "users", plural = "Users", default_sort = "name ASC")]
pub struct User {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[filterable(type = "boolean")]
    pub active: bool,
}
```

`GraphQLEntity` generates:

- async-graphql object and input/filter/order types
- SQL row decoding
- entity and schema metadata
- typed filter and order rendering

`GraphQLOperations` generates:

- list queries with filtering, ordering, pagination, and counts
- single-by-key lookups
- mutation roots for write-capable backends and policies
- repository helpers for application code

## Build A Schema

```rust
schema_roots! {
    auth: "required",
    query_custom_ops: [],
    entities: [User],
}

async fn build_schema(database_url: &str) -> graphql_orm::Result<AppSchema> {
    let database =
        graphql_orm::db::Database::<graphql_orm::SqliteBackend>::connect_sqlite(database_url)
            .await?;

    Ok(schema_builder(database)
        .data(AuthSubject::new("current-user-id"))
        .finish())
}
```

The generated `schema_builder(database)` registers the database runtime and generated dataloaders.
Applications can attach additional async-graphql data after calling it.

`auth: "required"` makes generated resolvers require an `AuthSubject` before database access. Use
`auth: "none"` for public schemas, or `auth: "optional"` when policy hooks decide whether an
anonymous request may continue. The compatibility default is still fail-closed.

Applications no longer need to import SQLX for normal setup. `connect_sqlite`, `connect_postgres`,
and `connect_ado` create `Database` handles directly. `Database::new(pool)` and
`Database::builder(pool)` remain available when an application intentionally owns driver-specific
pool setup.

Generated GraphQL mutation exposure is controlled at the schema root, not by disabling repository
writes. `generated_mutations` defaults to `"all"`; set it to `"none"`, `"allowlist"`, or
`"denylist"` to control only which generated mutations are merged into the public mutation root.
Custom roots in `extra_mutation_types` remain available.

## Query Shape

```graphql
query {
  users(where: { active: { eq: true } }, orderBy: [{ name: ASC }], page: { limit: 20 }) {
    edges {
      node { id name active }
      cursor
    }
    pageInfo {
      totalCount
      hasNextPage
    }
  }
}
```

Generated defaults use camelCase GraphQL names. Use the `resolver-case-*`, `argument-case-*`, and
`field-case-*` feature groups when a whole build needs a different GraphQL naming contract.

## Next Steps

- Choose backend features in [Backend Features](backends.md).
- Map legacy column names and composite keys in [Entities And Relations](entities-and-relations.md).
- Use SQL Server in read-only mode with [MSSQL](mssql.md).
- Plan schema ownership and migrations in [Schema Management](schema-management.md).
