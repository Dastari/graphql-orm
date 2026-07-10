use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "events", plural = "Events", append_only = true)]
struct Event {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    kind: String,
}

async fn update(database: &Database<SqliteBackend>) {
    let _ = Event::delete_all(database).await;
}

fn main() {}
