use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "postgres",
    table = "postgres_public_hidden_notes",
    plural = "PostgresPublicHiddenNotes"
)]
struct PostgresPublicHiddenNote {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub title: String,
}

schema_roots! {
    backend: "postgres",
    schema_policy: "managed",
    generated_mutations: "none",
    entities: [PostgresPublicHiddenNote],
}

fn main() {}
