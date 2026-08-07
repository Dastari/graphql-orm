use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "operation_authorization_duplicate_declaration")]
#[graphql_orm(operation_authorization(categories = ["single_read"], any_scopes = [["records.read"]]))]
#[graphql_orm(operation_authorization(categories = ["single_read"], any_scopes = [["records.admin"]]))]
struct OperationAuthorizationDuplicateDeclaration {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    label: String,
}

fn main() {}
