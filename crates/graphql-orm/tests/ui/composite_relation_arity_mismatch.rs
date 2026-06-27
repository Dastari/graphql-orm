use graphql_orm::prelude::*;

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "parents", plural = "Parents", default_sort = "id ASC")]
struct Parent {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,

    #[filterable(type = "string")]
    pub name: String,
}

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "children", plural = "Children", default_sort = "id ASC")]
struct Child {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,

    #[filterable(type = "string")]
    pub parent_id: String,

    #[filterable(type = "string")]
    pub parent_tenant_id: String,

    #[relation(
        target = "Parent",
        from = ["parent_id", "parent_tenant_id"],
        to = ["id"]
    )]
    pub parent: Option<Parent>,
}

fn main() {}
