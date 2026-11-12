use graphql_orm::prelude::*;

#[derive(GraphQLEntity, Clone, Debug)]
#[graphql_entity(backend = "postgres", table = "unknown_rls_notes", plural = "UnknownRlsNotes")]
#[graphql_rls(select(tenant = "tenant_id\" = \"tenant_id\" OR true OR \"tenant_id"))]
struct UnknownRlsNote {
    #[primary_key]
    pub id: String,

    pub tenant_id: String,
}

fn main() {}
