use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "records", mutations = false)]
#[graphql_orm(operation_authorization(categories = ["create"], all_scopes = ["records.access"]))]
struct Record {
    #[primary_key]
    #[sortable]
    id: String,
}

fn main() {}
