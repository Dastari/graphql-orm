//! Small source addresses for bounded provider-history admission. These rows are correlation
//! hints only; the protected owning call/result must be reopened before every positive decision.
use super::*;

#[backend_selected_graphql_entity(
    table = "graphql_orm_ai_tool_result_sources",
    plural = "GraphqlOrmAiToolResultSources",
    default_sort = "created_at ASC",
    index(name = "idx_graphql_orm_ai_tool_sources_cursor", columns = ["session_id", "provider_session_binding_id", "provider_cursor_fingerprint", "provider_kind", "provider_profile_id", "provider_model", "created_at"], directions = ["asc", "asc", "asc", "asc", "asc", "asc", "asc"])
)]
#[cfg_attr(feature = "mssql", derive(GraphQLSchemaEntity))]
#[cfg_attr(
    any(feature = "sqlite", feature = "postgres"),
    derive(GraphQLEntity, GraphQLOperations)
)]
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct AiToolResultSourceRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,
    #[filterable(type = "uuid")]
    pub session_id: graphql_orm::uuid::Uuid,
    #[filterable(type = "uuid")]
    pub run_id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    pub provider_kind: String,
    #[filterable(type = "string")]
    pub provider_profile_id: String,
    #[filterable(type = "string")]
    pub provider_model: String,
    #[filterable(type = "uuid")]
    pub provider_session_binding_id: Option<graphql_orm::uuid::Uuid>,
    #[filterable(type = "string")]
    pub provider_cursor_fingerprint: Option<String>,
    pub source_reference: String,
    pub classification: String,
    pub image_count: i64,
    #[sortable]
    pub created_at: i64,
}
