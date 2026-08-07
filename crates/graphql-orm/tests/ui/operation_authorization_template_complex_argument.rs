use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "operation_authorization_template_complex_argument")]
#[graphql_orm(operation_authorization(
    categories = ["create"],
    all_scope_templates = ["records.{input}.write"]
))]
struct OperationAuthorizationTemplateComplexArgument {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    label: String,
}

fn main() {}
