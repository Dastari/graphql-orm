use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(backend = "postgres", table = "postgres_rls_notes", plural = "PostgresRlsNotes")]
#[graphql_rls(
    force = true,
    select(scope = "notes.read", tenant = "tenant_id"),
    update(predicate = "graphql_orm.has_scope('notes.write')")
)]
struct PostgresRlsNote {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    pub tenant_id: String,

    pub title: String,
}

fn main() {}
