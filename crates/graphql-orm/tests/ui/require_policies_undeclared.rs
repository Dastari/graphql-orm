use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "decided_notes", plural = "DecidedNotes", read_policy = "notes.read")]
struct DecidedNote {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
}

// Nobody decided who may use this entity.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "undecided_tags", plural = "UndecidedTags")]
struct UndecidedTag {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
}

schema_roots! {
    backend: "sqlite",
    require_policies: true,
    query_custom_ops: [],
    entities: [DecidedNote, UndecidedTag],
}

fn main() {}
