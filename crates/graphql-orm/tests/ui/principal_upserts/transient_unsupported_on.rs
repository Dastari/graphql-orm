use graphql_orm::prelude::*;

struct Machine;

impl PrincipalKeyBinding for Machine {
    type Key = String;

    fn bind(ctx: &async_graphql::Context<'_>) -> async_graphql::Result<Self::Key> {
        Ok(ctx.auth_subject()?.id)
    }
}

#[derive(async_graphql::InputObject)]
struct Proof {
    sequence: i64,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "snapshots",
    plural = "Snapshots",
    upsert = "machine_key",
    upsert_bound_key(field = "machine_key", binding = "Machine"),
    transient_input(type = "Proof", on = "update"),
    mutation_shapes = "upsert",
)]
struct Snapshot {
    #[primary_key]
    #[sortable]
    id: String,
    #[graphql_orm(private)]
    #[unique]
    machine_key: String,
    payload: String,
}

fn main() {
    let _ = std::marker::PhantomData::<(Machine, Proof)>;
}
