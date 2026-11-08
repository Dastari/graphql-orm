use graphql_orm::prelude::*;
#[derive(GraphQLEntity,GraphQLOperations,Clone,Debug,serde::Serialize,serde::Deserialize)]
#[graphql_entity(dependent_deletes,backend="mssql",schema_policy="external_writable",table="mssql_dependents",plural="MssqlDependents")]
struct MssqlDependent {
    #[primary_key] id:String,
}
fn main() {}
