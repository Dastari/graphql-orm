use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "operation_authorization_invalid_scope")]
#[graphql_orm(operation_authorization(
    categories = ["single_read"],
    any_scopes = [["records {read}"]]
))]
struct OperationAuthorizationInvalidScope {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    label: String,
}

fn main() {}
