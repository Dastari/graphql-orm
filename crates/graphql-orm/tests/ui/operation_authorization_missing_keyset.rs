use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "operation_authorization_missing_keyset")]
#[graphql_orm(operation_authorization(
    categories = ["keyset_list"],
    all_scopes = ["records.page"]
))]
struct OperationAuthorizationMissingKeyset {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    label: String,
}

fn main() {}
