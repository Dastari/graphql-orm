use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "unknown_mutation_entity_notes", plural = "Notes")]
struct Note {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub title: String,
}

schema_roots! {
    backend: "sqlite",
    generated_mutations: "allowlist",
    generated_mutation_allowlist: [MissingNote],
    entities: [Note],
}

fn main() {}
