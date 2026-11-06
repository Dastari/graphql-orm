// An entity that declares `schema_only` generates no GraphQL object, so the
// relation derive still supplies the object's relation fields.
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

impl BatchLoadEntity for ProbeChild {
    fn batch_column() -> &'static str {
        "id"
    }
    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, graphql_orm::sqlx::Error> {
        use graphql_orm::sqlx::Row;
        row.try_get("id")
    }
}

#[derive(GraphQLEntity, GraphQLRelations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "sqlite",
    table = "probe_parents",
    plural = "ProbeParents",
    schema_only = true
)]
struct ProbeParent {
    #[primary_key]
    pub id: String,
    #[graphql(skip)]
    #[serde(default)]
    #[relation(target = "ProbeChild", from = "id", to = "parent_id", multiple, emit_fk = false)]
    pub children: Vec<ProbeChild>,
}

fn main() {}
