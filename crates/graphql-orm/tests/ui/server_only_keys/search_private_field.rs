use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "targets", plural = "Targets")]
struct Target {
    #[primary_key]
    #[sortable]
    id: i32,
    #[graphql(skip)]
    #[graphql_orm(private)]
    partition: i32,
}

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "searchers", plural = "Searchers")]
#[graphql_orm(search(index = true))]
struct Searcher {
    #[primary_key]
    #[sortable]
    id: i32,
    target_id: i32,
    #[relation(target = "Target", from = "target_id", to = "id", emit_fk = false)]
    #[graphql_orm(search_relation(fields = ["partition"], weight = "C"))]
    target: Option<Target>,
}

fn main() {}
