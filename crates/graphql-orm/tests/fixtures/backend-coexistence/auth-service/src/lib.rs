use graphql_orm::prelude::*;

pub fn project_direct_host_principal(principal: &agql_auth::AuthPrincipal) -> AuthSubject {
    graphql_orm::graphql::auth_agql::auth_subject_from_principal(principal)
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "auth_users",
    plural = "AuthUsers",
    default_sort = "id ASC"
)]
pub struct AuthUser {
    #[primary_key]
    #[filterable(type = "string")]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub email: String,

    #[sortable]
    pub created_at: i64,
}

schema_roots! {
    backend: "sqlite",
    schema_policy: "managed",
    query_custom_ops: [],
    entities: [AuthUser],
}

pub fn build_schema(
    pool: graphql_orm::sqlx::SqlitePool,
) -> graphql_orm::async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot> {
    schema_builder(graphql_orm::db::Database::<graphql_orm::SqliteBackend>::new(pool)).finish()
}
