use graphql_orm::prelude::*;

#[derive(GraphQLEntity, Clone, Debug)]
#[graphql_entity(backend = "mssql", table = "mssql_rls_notes", plural = "MssqlRlsNotes")]
#[graphql_rls(select(scope = "notes.read"))]
struct MssqlRlsNote {
    #[primary_key]
    pub id: String,
}

fn main() {}
