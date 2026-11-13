use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "collision_items", plural = "Items")]
struct Item {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    label: String,
}

#[derive(Default)]
struct ApplicationQueries;

#[graphql_orm_custom_operations(kind = "query")]
#[graphql_orm::async_graphql::Object]
impl ApplicationQueries {
    /// Returns an unrelated status.
    async fn status(&self) -> String {
        String::new()
    }

    /// A skipped resolver contributes no root field.
    #[graphql(skip)]
    async fn items(&self) -> String {
        String::new()
    }
}

#[derive(Default)]
struct LegacyMutations;

#[graphql_orm_root_fields]
#[graphql_orm::async_graphql::Object]
impl LegacyMutations {
    // A mutation may share a name with a generated query: roots are separate.
    async fn items(&self) -> bool {
        true
    }
}

schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [Item],
    extra_mutation_types: [LegacyMutations],
    described_query_types: [ApplicationQueries],
}

fn main() {}
