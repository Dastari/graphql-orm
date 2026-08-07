use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "operation_authorization_unknown_option")]
#[graphql_orm(operation_authorization(
    categories = ["single_read"],
    any_scopes = [["records.read"]],
    required_roles = ["records.admin"]
))]
struct OperationAuthorizationUnknownOption {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    label: String,
}

fn main() {}
