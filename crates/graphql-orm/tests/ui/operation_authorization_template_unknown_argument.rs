use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "operation_authorization_template_unknown_argument")]
#[graphql_orm(operation_authorization(
    categories = ["single_read"],
    all_scope_templates = ["records.{recordId}.read"]
))]
struct OperationAuthorizationTemplateUnknownArgument {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    label: String,
}

fn main() {}
