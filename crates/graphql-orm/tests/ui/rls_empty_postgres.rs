use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(backend = "postgres", table = "empty_rls_notes", plural = "EmptyRlsNotes")]
#[graphql_rls]
struct EmptyRlsNote {
    #[primary_key]
    pub id: String,

    pub title: String,
}

fn main() {}
