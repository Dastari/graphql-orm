// GraphQLEntity already generates the GraphQL object, so a bare GraphQLRelations
// derive cannot add relation fields to it. The macro names the supported form.
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "sqlite",
    table = "probe_children",
    plural = "ProbeChildren",
    default_sort = "id ASC"
)]
struct ProbeChild {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,
    #[filterable(type = "string")]
    pub parent_id: String,
}

#[derive(
    GraphQLEntity,
    GraphQLOperations,
    GraphQLRelations,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
)]
#[graphql_entity(
    backend = "sqlite",
    schema_policy = "managed",
    table = "probe_parents",
    plural = "ProbeParents",
    node_id,
    default_sort = "id ASC"
)]
struct ProbeParent {
    pub label: String,
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,
    #[graphql_orm(version = "managed", default = "0")]
    pub row_version: i64,
    #[graphql(skip)]
    #[serde(default)]
    #[relation(target = "ProbeChild", from = "id", to = "parent_id", multiple, emit_fk = false)]
    pub children: Vec<ProbeChild>,
}

fn main() {}
