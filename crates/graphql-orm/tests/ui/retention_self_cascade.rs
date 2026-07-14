use graphql_orm::prelude::*;

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "retained_tree",
    plural = "RetainedTrees",
    append_only = true,
    retention_purge = "retained_tree.purge"
)]
struct RetainedTree {
    #[primary_key]
    id: String,
    parent_id: Option<String>,
    #[relation(
        target = "RetainedTree",
        from = "parent_id",
        to = "id",
        emit_fk = true,
        on_delete = "cascade"
    )]
    parent: Option<Box<RetainedTree>>,
}

fn main() {}
