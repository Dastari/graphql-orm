use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "boards", plural = "Boards", auth = "none")]
struct Board {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
}

// The relation data loader is generated-resolver plumbing (#445): it is not
// in the prelude, and hosts cannot construct it directly.
fn loader(db: graphql_orm::db::Database) -> RelationLoader<Board> {
    graphql_orm::graphql::loaders::RelationLoader::<Board>::new(db)
}

fn main() {}
