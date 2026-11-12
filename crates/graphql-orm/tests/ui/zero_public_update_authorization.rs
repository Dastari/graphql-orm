use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "badges")]
#[graphql_orm(operation_authorization(
    categories = ["update_many"],
    all_scopes = ["badges.update"]
))]
struct Badge {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    code: String,
    #[graphql_orm(write = false)]
    issued_at: String,
}

fn main() {}
