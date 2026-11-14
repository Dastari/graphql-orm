use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "opt_out_parents", plural = "OptOutParents", default_sort = "id ASC")]
struct OptOutParent {
    #[primary_key]
    id: String,
    #[sortable]
    #[filterable(type = "string")]
    name: String,
}
#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(dependent_deletes, table = "cascade_children", plural = "CascadeChildren", default_sort = "id ASC")]
struct CascadeChild {
    #[primary_key]
    id: String,
    #[sortable]
    #[filterable(type = "string")]
    parent_id: String,
    #[relation(target = "OptOutParent", from = "parent_id", to = "id", on_delete = "cascade")]
    parent: Option<OptOutParent>,
}
#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(dependent_deletes, table = "null_children", plural = "NullChildren", default_sort = "id ASC")]
struct NullChild {
    #[primary_key]
    id: String,
    #[sortable]
    #[filterable(type = "string")]
    parent_id: Option<String>,
    #[relation(target = "OptOutParent", from = "parent_id", to = "id", on_delete = "set_null")]
    parent: Option<OptOutParent>,
}
fn main() {}
