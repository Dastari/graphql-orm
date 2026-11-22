use graphql_orm::prelude::*;

#[derive(GraphQLEntity)]
#[graphql_entity(table = "parents", propagate_change_action = "updated")]
struct EntityOption {
    #[primary_key]
    id: String,
}

#[derive(GraphQLEntity)]
#[graphql_entity(table = "children")]
struct ScalarOption {
    #[primary_key]
    #[graphql_orm(propagate_change_action = "updated")]
    id: String,
}

#[derive(GraphQLEntity)]
#[graphql_entity(table = "children")]
struct DisabledPropagation {
    #[primary_key]
    id: String,
    #[relation(target = "EntityOption", propagate_change_action = "updated", propagate_change = "none")]
    parent: Option<EntityOption>,
}

#[derive(GraphQLEntity)]
#[graphql_entity(table = "children")]
struct UnknownAction {
    #[primary_key]
    id: String,
    #[relation(target = "EntityOption", propagate_change = "up", propagate_change_action = "created")]
    parent: Option<EntityOption>,
}

fn main() {}
