// Combined async-graphql and serde options compose in any order with the ORM
// derives: no duplicate object implementations and no orphan attributes.
use graphql_orm::async_graphql::SimpleObject;
use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
)]
#[graphql_entity(backend = "sqlite", table = "shelves", plural = "Shelves")]
#[graphql(rename_fields = "PascalCase", complex)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Shelf {
    #[primary_key]
    #[sortable]
    pub shelf_id: String,
    #[serde(rename(serialize = "shelfTitle", deserialize = "shelfTitle"))]
    pub title: String,
    #[graphql(skip)]
    #[relation(target = "Volume", to = "shelf_id", multiple)]
    pub volumes: Vec<Volume>,
}

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
)]
#[graphql_entity(backend = "sqlite", table = "volumes", plural = "Volumes")]
#[graphql(complex, rename_fields = "PascalCase")]
struct Volume {
    #[primary_key]
    #[sortable]
    pub volume_id: String,
    #[filterable(type = "string")]
    #[serde(skip_serializing_if = "String::is_empty", rename = "shelfKey")]
    pub shelf_id: String,
    #[graphql(skip)]
    #[relation(target = "Shelf", from = "shelf_id", to = "shelf_id")]
    pub shelf: Option<Shelf>,
}

impl BatchLoadEntity for Shelf {
    fn batch_column() -> &'static str {
        "shelf_id"
    }
    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, graphql_orm::sqlx::Error> {
        use graphql_orm::sqlx::Row;
        row.try_get("shelf_id")
    }
}

impl BatchLoadEntity for Volume {
    fn batch_column() -> &'static str {
        "shelf_id"
    }
    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, graphql_orm::sqlx::Error> {
        use graphql_orm::sqlx::Row;
        row.try_get("shelf_id")
    }
}

#[derive(Default)]
struct InventoryQueries;

#[graphql_orm_custom_operations(kind = "query", authorization = true)]
#[graphql_orm::async_graphql::Object]
impl InventoryQueries {
    /// Counts volumes on one shelf.
    async fn count_volumes(
        &self,
        #[graphql(name = "shelfKey")] shelf: String,
        #[graphql(name = "pageLimit", desc = "Most volumes to count.")] limit: i32,
        unnamed: i32,
    ) -> i32 {
        let _ = shelf;
        limit + unnamed
    }
}

schema_roots! {
    backend: "sqlite",
    entities: [Shelf, Volume],
    described_query_types: [InventoryQueries],
}

fn main() {}
