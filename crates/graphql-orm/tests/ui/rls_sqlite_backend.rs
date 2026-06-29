use graphql_orm::prelude::*;

#[derive(GraphQLEntity, Clone, Debug)]
#[graphql_entity(backend = "sqlite", table = "sqlite_rls_notes", plural = "SqliteRlsNotes")]
#[graphql_rls(select(scope = "notes.read"))]
struct SqliteRlsNote {
    #[primary_key]
    pub id: String,
}

fn main() {}
