use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "operation_authorization_mixed_scope_modes")]
#[graphql_orm(operation_authorization(
    categories = ["single_read"],
    all_scopes = ["records.read"],
    any_scopes = [["records.admin"]]
))]
struct OperationAuthorizationMixedScopeModes {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    label: String,
}

fn main() {}
