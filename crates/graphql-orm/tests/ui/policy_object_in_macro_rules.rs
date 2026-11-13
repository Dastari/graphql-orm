use graphql_orm::prelude::*;
use graphql_orm::async_graphql::SimpleObject;

// `graphql_policy_object` cannot be applied when `SimpleObject` reaches the
// derive list through a `macro_rules!` argument: the guard it installs refers
// to `self`, which macro hygiene then hides from the derive.
macro_rules! policy_entity {
    ($name:ident, [$($derives:tt)*]) => {
        #[graphql_orm::graphql_policy_object]
        #[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize, $($derives)*)]
        #[graphql(complex)]
        #[graphql_entity(backend = "sqlite", table = "policy_fields", plural = "PolicyFields")]
        struct $name {
            #[primary_key]
            #[filterable(type = "string")]
            #[sortable]
            id: String,
            #[graphql_orm(read_policy = "secret.read")]
            secret: String,
        }
        #[graphql_orm::async_graphql::ComplexObject]
        impl $name {}
    };
}

policy_entity!(PolicyField, [SimpleObject]);

fn main() {}
