use graphql_orm::prelude::*;

// "open" states that no policy is declared; a policy key contradicts it.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "mixed_notes", plural = "MixedNotes", policy = "open", write_policy = "notes.write")]
struct MixedNote {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
}

fn main() {}
