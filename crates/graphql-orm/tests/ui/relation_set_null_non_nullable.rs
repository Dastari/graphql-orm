use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(table = "invalid_parents", plural = "InvalidParents", default_sort = "id ASC")]
struct InvalidParent {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,
}

#[derive(
    GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(table = "invalid_children", plural = "InvalidChildren", default_sort = "id ASC")]
struct InvalidChild {
    #[primary_key]
    pub id: String,

    pub parent_id: String,

    #[relation(target = "InvalidParent", from = "parent_id", to = "id", on_delete = "set_null")]
    pub parent: Option<InvalidParent>,
}

fn main() {}
