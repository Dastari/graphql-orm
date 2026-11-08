use graphql_orm::prelude::*;
#[derive(GraphQLEntity,GraphQLOperations,Clone,Debug,serde::Serialize,serde::Deserialize)]
#[graphql_entity(dependent_deletes,table="composite_dependents",plural="CompositeDependents")]
struct CompositeDependent {
    #[primary_key] first:String,
    #[primary_key] second:String,
}
fn main() {}
