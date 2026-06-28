# Getting Started

`graphql-orm` is used from application crates through the runtime crate:

```toml
[dependencies]
graphql-orm = { version = "0.2.11", default-features = false, features = ["sqlite"] }
```

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
    query_custom_ops: [],
    entities: [User],
}

async fn build_schema(pool: graphql_orm::sqlx::SqlitePool) -> AppSchema {
    let database = graphql_orm::db::Database::new(pool);

    schema_builder(database)
        .data("current-user-id".to_string())
        .finish()
}
```

The generated `schema_builder(database)` registers the database runtime and generated dataloaders.
Applications can attach additional async-graphql data after calling it.

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
