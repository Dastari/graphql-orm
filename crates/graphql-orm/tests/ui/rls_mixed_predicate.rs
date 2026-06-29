use graphql_orm::prelude::*;

#[derive(GraphQLEntity, Clone, Debug)]
#[graphql_entity(backend = "postgres", table = "mixed_rls_notes", plural = "MixedRlsNotes")]
#[graphql_rls(select(
    predicate = "graphql_orm.has_scope('notes.read')",
    scope = "notes.read"
))]
struct MixedRlsNote {
    #[primary_key]
    pub id: String,
}

fn main() {}
