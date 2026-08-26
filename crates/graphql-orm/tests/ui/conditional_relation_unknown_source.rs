use graphql_orm::prelude::*;

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "targets", plural = "Targets", default_sort = "id ASC")]
struct Target {
    #[primary_key]
    #[filterable(type = "number")]
    #[sortable]
    pub id: i32,

    #[filterable(type = "string")]
    pub name: String,
}

#[derive(
    GraphQLEntity, GraphQLRelations, serde::Serialize, serde::Deserialize, Clone, Debug,
)]
#[graphql_entity(table = "records", plural = "Records", default_sort = "id ASC")]
struct Record {
    #[primary_key]
    #[filterable(type = "number")]
    #[sortable]
    pub id: i32,

    #[filterable(type = "number")]
    pub target_id: i32,

    #[filterable(type = "string")]
    pub name: String,

    #[relation(
        target = "Target",
        from = "target_id",
        to = "id",
        source_condition(field = "missing_kind", equals = 1),
        emit_fk = false
    )]
    pub target: Option<Target>,
}

fn main() {}
