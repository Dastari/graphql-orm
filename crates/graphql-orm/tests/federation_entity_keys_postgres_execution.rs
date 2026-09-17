#![cfg(all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))))]
//! `_entities` execution and batching against a disposable PostgreSQL.
//!
//! The SQLite lane proves the resolver's behaviour; this lane proves the same
//! batched predicate renders and executes with PostgreSQL placeholders and
//! identifier quoting. It owns its database: see the disposable-infrastructure
//! helper, which refuses to clean up anything it did not create.

#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use graphql_orm::prelude::*;
use owned_postgres::OwnedPostgres;

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    backend = "postgres",
    table = "federation_pg_zones",
    plural = "Zones",
    default_sort = "name ASC",
    auth = "none",
    federation_key,
    federation_key(fields = ["tenant", "code"]),
    unique_index(name = "federation_pg_zones_tenant_code", columns = ["tenant", "code"])
)]
struct Zone {
    #[primary_key]
    id: String,

    #[filterable(type = "string")]
    #[sortable]
    name: String,

    #[filterable(type = "string")]
    tenant: String,

    #[filterable(type = "string")]
    code: String,
}

schema_roots! {
    backend: "postgres",
    query_custom_ops: [],
    entities: [Zone],
}

#[tokio::test(flavor = "multi_thread")]
async fn postgres_entity_keys_resolve_and_batch() -> Result<(), Box<dyn std::error::Error>> {
    let mut postgres = OwnedPostgres::start("federation-entity-keys")?;
    let result = run(&postgres.url).await;
    postgres.cleanup()?;
    result
}

async fn run(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::PgPool::connect(url).await?;
    sqlx::query(
        "CREATE TABLE federation_pg_zones (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            tenant TEXT NOT NULL,
            code TEXT NOT NULL
        )",
    )
    .execute(&pool)
    .await?;
    for (id, name, tenant, code) in [
        ("zone-1", "north", "alpha", "N"),
        ("zone-2", "south", "alpha", "S"),
        ("zone-3", "east", "beta", "E"),
    ] {
        sqlx::query(
            "INSERT INTO federation_pg_zones (id, name, tenant, code) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(name)
        .bind(tenant)
        .bind(code)
        .execute(&pool)
        .await?;
    }

    let schema = schema_builder(graphql_orm::db::Database::new(pool)).finish();

    // A single-column key renders `id IN ($1, $2, ...)`.
    graphql_orm::graphql::orm::reset_query_count();
    let simple = schema
        .execute(
            "query { _entities(representations: [\
                 { __typename: \"Zone\", id: \"zone-1\" }, \
                 { __typename: \"Zone\", id: \"zone-3\" }, \
                 { __typename: \"Zone\", id: \"absent\" }]) \
             { ... on Zone { id name } } }",
        )
        .await;
    let simple_statements = graphql_orm::graphql::orm::query_count();
    assert!(simple.errors.is_empty(), "{:?}", simple.errors);
    assert_eq!(
        simple_statements, 1,
        "three representations issue one statement"
    );
    let json = simple.data.into_json()?;
    assert_eq!(json["_entities"][0]["name"], "north");
    assert_eq!(json["_entities"][1]["name"], "east");
    assert!(json["_entities"][2].is_null(), "a miss resolves to null");

    // A composite key renders an OR of AND groups over both columns.
    graphql_orm::graphql::orm::reset_query_count();
    let composite = schema
        .execute(
            "query { _entities(representations: [\
                 { __typename: \"Zone\", tenant: \"alpha\", code: \"S\" }, \
                 { __typename: \"Zone\", tenant: \"beta\", code: \"E\" }]) \
             { ... on Zone { id name } } }",
        )
        .await;
    let composite_statements = graphql_orm::graphql::orm::query_count();
    assert!(composite.errors.is_empty(), "{:?}", composite.errors);
    assert_eq!(
        composite_statements, 1,
        "two composite representations issue one statement"
    );
    let json = composite.data.into_json()?;
    assert_eq!(json["_entities"][0]["id"], "zone-2");
    assert_eq!(json["_entities"][1]["id"], "zone-3");

    Ok(())
}
