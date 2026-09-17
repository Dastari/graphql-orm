#![cfg(feature = "sqlite")]
//! One statement per entity type per `_entities` fetch.
//!
//! `query_count` is process-global, so this proof owns its own test binary and
//! declares a single test. Statements are counted rather than assumed: the
//! generated entity resolvers enqueue every representation on a shared
//! `DataLoader`, which resolves them with one `IN (...)` select.

use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    backend = "sqlite",
    table = "zones",
    plural = "Zones",
    default_sort = "name ASC",
    federation_key
)]
pub struct Zone {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    backend = "sqlite",
    table = "assets",
    plural = "Assets",
    default_sort = "label ASC",
    federation_key
)]
pub struct Asset {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub label: String,
}

schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [Zone, Asset],
}

async fn seeded_pool() -> sqlx::SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("SQLite test pool");
    sqlx::query("CREATE TABLE zones (id TEXT PRIMARY KEY, name TEXT NOT NULL)")
        .execute(&pool)
        .await
        .expect("create zones");
    sqlx::query("CREATE TABLE assets (id TEXT PRIMARY KEY, label TEXT NOT NULL)")
        .execute(&pool)
        .await
        .expect("create assets");
    for index in 0..8 {
        sqlx::query("INSERT INTO zones (id, name) VALUES (?1, ?2)")
            .bind(format!("zone-{index}"))
            .bind(format!("zone {index}"))
            .execute(&pool)
            .await
            .expect("seed zone");
        sqlx::query("INSERT INTO assets (id, label) VALUES (?1, ?2)")
            .bind(format!("asset-{index}"))
            .bind(format!("asset {index}"))
            .execute(&pool)
            .await
            .expect("seed asset");
    }
    pool
}

#[tokio::test]
async fn one_statement_per_entity_type_per_entities_fetch() {
    let schema = schema_builder(graphql_orm::db::Database::new(seeded_pool().await))
        .data(AuthSubject::new("user-1"))
        .finish();

    let mut representations = Vec::new();
    for index in 0..8 {
        representations.push(format!("{{ __typename: \"Zone\", id: \"zone-{index}\" }}"));
        representations.push(format!(
            "{{ __typename: \"Asset\", id: \"asset-{index}\" }}"
        ));
    }
    // One representation that matches no row, to prove a miss does not become
    // its own statement.
    representations.push("{ __typename: \"Zone\", id: \"absent\" }".to_string());
    let query = format!(
        "query {{ _entities(representations: [{}]) {{ \
             ... on Zone {{ id name }} ... on Asset {{ id label }} }} }}",
        representations.join(", ")
    );

    graphql_orm::graphql::orm::reset_query_count();
    let response = schema.execute(query).await;
    let statements = graphql_orm::graphql::orm::query_count();
    assert!(response.errors.is_empty(), "{:?}", response.errors);

    assert_eq!(
        statements, 2,
        "17 representations across two entity types must issue exactly two statements"
    );

    let json = response.data.into_json().expect("json");
    let entities = json["_entities"].as_array().expect("entity list");
    assert_eq!(entities.len(), 17);
    assert_eq!(entities[0]["name"], "zone 0");
    assert_eq!(entities[1]["label"], "asset 0");
    assert!(
        entities[16].is_null(),
        "the unmatched representation is null"
    );
}
