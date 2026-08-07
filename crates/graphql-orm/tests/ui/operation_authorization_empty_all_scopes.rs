use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "operation_authorization_empty_all_scopes")]
#[graphql_orm(operation_authorization(
    categories = ["list"],
    all_scopes = []
))]
struct OperationAuthorizationEmptyAllScopes {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    label: String,
}

fn main() {}
