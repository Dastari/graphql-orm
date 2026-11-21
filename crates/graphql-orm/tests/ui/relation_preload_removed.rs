use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "boards", plural = "Boards", auth = "none")]
struct Board {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
}

// The policy-free relation preload API is gone (#445): relations are read
// through generated resolvers or repository builders, which apply policy.
async fn preload(pool: &graphql_orm::DbPool, boards: &mut [Board]) -> graphql_orm::Result<()> {
    Board::bulk_load_relations(boards, pool, &[]).await
}

fn main() {}
