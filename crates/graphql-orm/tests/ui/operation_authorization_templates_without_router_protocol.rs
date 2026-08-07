use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
)]
#[graphql_entity(table = "operation_authorization_templates_without_router_protocol")]
#[graphql_orm(operation_authorization(
    categories = ["single_read"],
    all_scope_templates = ["records.{id}.read"]
))]
struct OperationAuthorizationTemplatesWithoutRouterProtocol {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    label: String,
}

fn main() {}
