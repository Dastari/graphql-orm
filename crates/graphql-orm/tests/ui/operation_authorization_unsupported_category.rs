use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "operation_authorization_unsupported_category")]
#[graphql_orm(operation_authorization(
    categories = ["unknown"],
    any_scopes = [["records.read"]]
))]
struct OperationAuthorizationUnsupportedCategory {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    label: String,
}

fn main() {}
