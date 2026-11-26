use graphql_orm::prelude::*;

struct Machine;

impl PrincipalKeyBinding for Machine {
    type Key = String;

    fn bind(ctx: &async_graphql::Context<'_>) -> async_graphql::Result<Self::Key> {
        Ok(ctx.auth_subject()?.id)
    }
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "snapshots",
    plural = "Snapshots",
    upsert = "machine_key",
    mutation_shapes = "upsert",
    nested_create_keys = ["parent_id"],
    upsert_bound_key(field = "machine_key", binding = "Machine"),
)]
struct Snapshot {
    #[primary_key]
    #[sortable]
    id: String,
    #[graphql_orm(private)]
    #[unique]
    machine_key: String,
    parent_id: String,
    payload: String,
}

fn main() {}
