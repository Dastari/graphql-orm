use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "operation_authorization_missing_generated_category")]
#[graphql_orm(operation_authorization(
    categories = ["upsert"],
    all_scopes = ["records.upsert"]
))]
struct OperationAuthorizationMissingGeneratedCategory {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    label: String,
}

fn main() {}
