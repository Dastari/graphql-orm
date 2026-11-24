use graphql_orm::prelude::*;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, async_graphql::SimpleObject)]
struct Target {
    id: i32,
}

impl graphql_orm::graphql::orm::GeneratedRelationReadability for Target {
    const COLUMNS: &'static [(&'static str, bool)] = &[("id", true), ("partition", false)];
}

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "sources", plural = "Sources")]
struct Source {
    #[primary_key]
    #[sortable]
    id: i32,
    target_id: i32,
    #[graphql_orm(private)]
    partition: i32,
    #[relation(
        target = "Target",
        from = ["target_id", "partition"],
        to = ["id", "partition"],
        server_only_key(from = "partition", to = "partition"),
        emit_fk = false
    )]
    target: Option<Target>,
}

fn main() {}
