use graphql_orm::prelude::*;

// Without mutation_shapes the entity generates update_many, which writes rows
// without a per-row ExpectedVersion or assurance selection.
#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "reviews", update_expected_version = "required")]
struct Review {
    #[primary_key]
    #[sortable]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    state: String,
    #[graphql_orm(version = "managed", default = "0")]
    row_version: i64,
}

fn main() {}
