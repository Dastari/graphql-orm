use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "operation_authorization_empty_categories")]
#[graphql_orm(operation_authorization(
    categories = [],
    any_scopes = [["records.read"]]
))]
struct OperationAuthorizationEmptyCategories {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    label: String,
}

fn main() {}
