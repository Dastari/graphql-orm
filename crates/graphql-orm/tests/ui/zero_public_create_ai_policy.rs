use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "snapshots", ai_mutations(create = "approval_required"))]
struct Snapshot {
    #[primary_key]
    #[sortable]
    id: graphql_orm::uuid::Uuid,
    #[graphql_orm(skip_input)]
    captured_at: String,
}

fn main() {}
