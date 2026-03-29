use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(table = "schema_only_examples", plural = "SchemaOnlyExamples")]
struct SchemaOnlyExample {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,
}

fn main() {
    let _missing_where: SchemaOnlyExampleWhereInput = Default::default();
    let _missing_create: CreateSchemaOnlyExampleInput;
}
