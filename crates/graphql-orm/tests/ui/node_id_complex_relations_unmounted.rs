// With relations, GraphQLRelations emits the composed object, but only
// `#[graphql_complex_object]` mounts it; a plain `#[ComplexObject]` would leave
// the `NodeId` key without its field.
use graphql_orm::async_graphql::{ComplexObject, SimpleObject};
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(backend = "sqlite", table = "children", plural = "Children")]
struct Child {
    #[primary_key]
    #[sortable]
    id: String,
    parent_id: String,
}

impl BatchLoadEntity for Child {
    fn batch_column() -> &'static str {
        "parent_id"
    }
    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, graphql_orm::sqlx::Error> {
        use graphql_orm::sqlx::Row;
        row.try_get("parent_id")
    }
}

#[derive(
    GraphQLEntity,
    GraphQLOperations,
    GraphQLRelations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_orm(compose_complex_object)]
#[graphql_entity(backend = "sqlite", table = "parents", plural = "Parents", node_id)]
struct Parent {
    #[primary_key]
    #[sortable]
    id: String,
    name: String,
    #[graphql(skip)]
    #[serde(default)]
    #[relation(target = "Child", from = "id", to = "parent_id", multiple, emit_fk = false)]
    children: Vec<Child>,
}

#[ComplexObject]
impl Parent {
    async fn label(&self) -> String {
        self.id.clone()
    }
}

fn main() {}
