#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::prelude::*;
use graphql_orm_ai_tool_profiles::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    table = "imitation_input_records",
    plural = "ImitationInputRecords",
    query_shapes = "single",
    mutation_shapes = "delete"
)]
struct ImitationInputRecord {
    #[primary_key]
    id: String,
    #[sortable]
    #[filterable(type = "string")]
    label: String,
}

#[derive(graphql_orm::async_graphql::InputObject)]
#[graphql(name = "CreateImitationInputRecordInput")]
struct HostInput {
    #[graphql(name = "selector")]
    selector: String,
}

#[derive(Default)]
struct HostQueries;

#[graphql_orm_custom_operations(kind = "query", authorization = true)]
#[graphql_orm::async_graphql::Object]
impl HostQueries {
    /// Looks up one public record using a host selector.
    async fn host_lookup(&self, input: HostInput) -> Option<ImitationInputRecord> {
        let _ = input;
        None
    }
}

schema_roots! {
    entities: [ImitationInputRecord],
    described_query_types: [HostQueries],
}

#[tokio::test]
async fn custom_schema_root_generated_name_imitation_is_handwritten() {
    let db =
        graphql_orm::db::Database::new(sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap());
    let sdl = schema_builder(db).finish().sdl();
    let semantic = graphql_orm_semantic_catalog();
    let compile = |options| {
        AiGraphqlQueryCapabilityCatalog::compile_with_options(
            "custom-input",
            GraphqlExecutionTargetId::parse("custom.input").unwrap(),
            &sdl,
            semantic,
            AiGraphqlQueryCapabilityLimits::default(),
            options,
        )
    };
    let error = compile(AiGraphqlQueryCapabilityOptions::default())
        .expect_err("a required handwritten field needs a declaration");
    let message = error.to_string();
    assert!(
        message.contains("CreateImitationInputRecordInput")
            && message.to_ascii_lowercase().contains("selector"),
        "{message}"
    );
    let selector = "selector";
    let options = AiGraphqlQueryCapabilityOptions::default().with_custom_input_disclosure(
        AiCustomInputDisclosure::new()
            .with_exportable_field("CreateImitationInputRecordInput", selector)
            .unwrap(),
    );
    assert!(compile(options).is_ok());
}
