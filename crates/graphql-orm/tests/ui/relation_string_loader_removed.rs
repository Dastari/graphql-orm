use graphql_orm::async_graphql::dataloader::Loader;
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "boards", plural = "Boards", auth = "none")]
struct Board {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
}

impl BatchLoadEntity for Board {
    fn batch_column() -> &'static str {
        "id"
    }
    fn batch_key_from_row(_: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
        Ok(String::new())
    }
}

// The policy-free `Loader<String>` form of the relation data loader is gone
// (#445): it read rows without entity, row or field policy.
async fn load(db: graphql_orm::db::Database) {
    let loader = graphql_orm::graphql::loaders::RelationLoader::<Board>::__generated(db);
    let _ = loader.load(&["board-1".to_string()]).await;
}

fn main() {}
