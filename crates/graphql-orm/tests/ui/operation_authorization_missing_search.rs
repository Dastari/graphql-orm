use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "operation_authorization_missing_search")]
#[graphql_orm(operation_authorization(
    categories = ["search"],
    any_scopes = [["records.search"]]
))]
struct OperationAuthorizationMissingSearch {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    label: String,
}

fn main() {}
