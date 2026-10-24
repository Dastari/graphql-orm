//! Read-only, non-exportable metadata projections over AI-owned tables.
//!
//! Compose imported entities through `schema_roots!` using the database returned
//! by [`crate::AiPublicEntityAccess::database`]. These projections do not own schema,
//! migrations, backup custody or content disclosure. Continue registering
//! [`crate::AiSchemaModule`] for owned persistence. Generated notifications and
//! child propagation use trusted after-commit ORM witnesses and current authorization.

use graphql_orm::prelude::*;
use graphql_orm::uuid::Uuid;

/// Read-only authorized session metadata; protected payloads remain private.
#[backend_selected_graphql_entity(
    table = "graphql_orm_ai_sessions",
    plural = "AiPublicSessions",
    schema_policy = "external_read_only",
    auth = "required",
    read_policy = "ai.public.read",
    write_policy = "ai.public.never_write",
    backup = false
)]
#[derive(
    GraphQLEntity,
    graphql_orm::async_graphql::SimpleObject,
    GraphQLOperations,
    GraphQLRelations,
    Clone,
    Debug,
    PartialEq,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql_orm(policy_binding = "ai.public.metadata")]
#[graphql(
    complex,
    guard = "crate::public_entity_access::PublicMetadataFieldGuard::new(self)"
)]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_fields = "PascalCase"))]
pub struct AiPublicSession {
    /// Authorized id metadata.
    #[primary_key]
    #[sortable]
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "uuid")]
    pub id: Uuid,
    /// Authorized scope kind metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub scope_kind: String,
    /// Authorized scope id metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub scope_id: String,
    /// Authorized tenant id metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub tenant_id: Option<String>,
    /// Authorized title metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub title: String,
    /// Authorized state metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub state: String,
    /// Authorized stream head metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "number")]
    pub stream_head: i64,
    /// Authorized last activity at metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "number")]
    pub last_activity_at: i64,
    /// Authorized archived at metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "number")]
    pub archived_at: Option<i64>,
    /// Authorized related messages; child changes propagate through the trusted ORM channel.
    #[graphql(skip)]
    #[serde(skip)]
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[relation(
        target = "AiPublicMessage",
        from = "id",
        to = "session_id",
        multiple,
        emit_fk = false,
        propagate_change = "none"
    )]
    pub messages: Vec<AiPublicMessage>,
    /// Authorized related runs; child changes propagate through the trusted ORM channel.
    #[graphql(skip)]
    #[serde(skip)]
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[relation(
        target = "AiPublicRun",
        from = "id",
        to = "session_id",
        multiple,
        emit_fk = false,
        propagate_change = "none"
    )]
    pub runs: Vec<AiPublicRun>,
    /// Authorized related attachments; child changes propagate through the trusted ORM channel.
    #[graphql(skip)]
    #[serde(skip)]
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[relation(
        target = "AiPublicAttachment",
        from = "id",
        to = "session_id",
        multiple,
        emit_fk = false,
        propagate_change = "none"
    )]
    pub attachments: Vec<AiPublicAttachment>,
}

/// Read-only authorized message metadata; protected payloads remain private.
#[backend_selected_graphql_entity(
    table = "graphql_orm_ai_messages",
    plural = "AiPublicMessages",
    schema_policy = "external_read_only",
    auth = "required",
    read_policy = "ai.public.read",
    write_policy = "ai.public.never_write",
    backup = false
)]
#[derive(
    GraphQLEntity,
    graphql_orm::async_graphql::SimpleObject,
    GraphQLOperations,
    GraphQLRelations,
    Clone,
    Debug,
    PartialEq,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql_orm(policy_binding = "ai.public.metadata")]
#[graphql(
    complex,
    guard = "crate::public_entity_access::PublicMetadataFieldGuard::new(self)"
)]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_fields = "PascalCase"))]
pub struct AiPublicMessage {
    /// Authorized id metadata.
    #[primary_key]
    #[sortable]
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "uuid")]
    pub id: Uuid,
    /// Authorized session id metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "uuid")]
    pub session_id: Uuid,
    /// Authorized sequence metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "number")]
    pub sequence: i64,
    /// Authorized message role metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub message_role: String,
    /// Authorized run id metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "uuid")]
    pub run_id: Option<Uuid>,
    /// Authorized block count metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "number")]
    pub block_count: i64,
    /// Authorized completion state metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub completion_state: String,
    /// Authorized created at metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "number")]
    pub created_at: i64,
    /// Authorized finalized at metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "number")]
    pub finalized_at: Option<i64>,
    /// Authorized content purged at metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "number")]
    pub content_purged_at: Option<i64>,
    /// Authorized related session; child changes propagate through the trusted ORM channel.
    #[graphql(skip)]
    #[serde(skip)]
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[relation(
        target = "AiPublicSession",
        from = "session_id",
        to = "id",
        emit_fk = false,
        propagate_change = "up"
    )]
    pub session: Option<AiPublicSession>,
    /// Authorized related run; child changes propagate through the trusted ORM channel.
    #[graphql(skip)]
    #[serde(skip)]
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[relation(
        target = "AiPublicRun",
        from = "run_id",
        to = "id",
        emit_fk = false,
        propagate_change = "up"
    )]
    pub run: Option<AiPublicRun>,
}

/// Read-only authorized run metadata; protected payloads remain private.
#[backend_selected_graphql_entity(
    table = "graphql_orm_ai_runs",
    plural = "AiPublicRuns",
    schema_policy = "external_read_only",
    auth = "required",
    read_policy = "ai.public.read",
    write_policy = "ai.public.never_write",
    backup = false
)]
#[derive(
    GraphQLEntity,
    graphql_orm::async_graphql::SimpleObject,
    GraphQLOperations,
    GraphQLRelations,
    Clone,
    Debug,
    PartialEq,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql_orm(policy_binding = "ai.public.metadata")]
#[graphql(
    complex,
    guard = "crate::public_entity_access::PublicMetadataFieldGuard::new(self)"
)]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_fields = "PascalCase"))]
pub struct AiPublicRun {
    /// Authorized id metadata.
    #[primary_key]
    #[sortable]
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "uuid")]
    pub id: Uuid,
    /// Authorized session id metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "uuid")]
    pub session_id: Uuid,
    /// Authorized input message id metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "uuid")]
    pub input_message_id: Uuid,
    /// Authorized state metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub state: String,
    /// Authorized error code metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub error_code: Option<String>,
    /// Authorized created at metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "number")]
    pub created_at: i64,
    /// Authorized row version metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "number")]
    pub row_version: i64,
    /// Authorized related session; child changes propagate through the trusted ORM channel.
    #[graphql(skip)]
    #[serde(skip)]
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[relation(
        target = "AiPublicSession",
        from = "session_id",
        to = "id",
        emit_fk = false,
        propagate_change = "up"
    )]
    pub session: Option<AiPublicSession>,
    /// Authorized related messages; child changes propagate through the trusted ORM channel.
    #[graphql(skip)]
    #[serde(skip)]
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[relation(
        target = "AiPublicMessage",
        from = "id",
        to = "run_id",
        multiple,
        emit_fk = false,
        propagate_change = "none"
    )]
    pub messages: Vec<AiPublicMessage>,
}

/// Read-only authorized attachment metadata; protected payloads remain private.
#[backend_selected_graphql_entity(
    table = "graphql_orm_ai_attachments",
    plural = "AiPublicAttachments",
    schema_policy = "external_read_only",
    auth = "required",
    read_policy = "ai.public.read",
    write_policy = "ai.public.never_write",
    backup = false
)]
#[derive(
    GraphQLEntity,
    graphql_orm::async_graphql::SimpleObject,
    GraphQLOperations,
    GraphQLRelations,
    Clone,
    Debug,
    PartialEq,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql_orm(policy_binding = "ai.public.metadata")]
#[graphql(
    complex,
    guard = "crate::public_entity_access::PublicMetadataFieldGuard::new(self)"
)]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_fields = "PascalCase"))]
pub struct AiPublicAttachment {
    /// Authorized id metadata.
    #[primary_key]
    #[sortable]
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "uuid")]
    pub id: Uuid,
    /// Authorized session id metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "uuid")]
    pub session_id: Uuid,
    /// Authorized message id metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "uuid")]
    pub message_id: Option<Uuid>,
    /// Authorized safe filename metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub safe_filename: String,
    /// Authorized declared mime metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub declared_mime: Option<String>,
    /// Authorized detected mime metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub detected_mime: Option<String>,
    /// Authorized byte count metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "number")]
    pub byte_count: Option<i64>,
    /// Authorized quarantine state metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub quarantine_state: String,
    /// Authorized scan state metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub scan_state: String,
    /// Authorized processing state metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub processing_state: String,
    /// Authorized rejection code metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub rejection_code: Option<String>,
    /// Authorized created at metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "number")]
    pub created_at: i64,
    /// Authorized finalized at metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "number")]
    pub finalized_at: Option<i64>,
    /// Authorized deleted at metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "number")]
    pub deleted_at: Option<i64>,
    /// Authorized related session; child changes propagate through the trusted ORM channel.
    #[graphql(skip)]
    #[serde(skip)]
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[relation(
        target = "AiPublicSession",
        from = "session_id",
        to = "id",
        emit_fk = false,
        propagate_change = "up"
    )]
    pub session: Option<AiPublicSession>,
    /// Authorized related message; child changes propagate through the trusted ORM channel.
    #[graphql(skip)]
    #[serde(skip)]
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[relation(
        target = "AiPublicMessage",
        from = "message_id",
        to = "id",
        emit_fk = false,
        propagate_change = "up"
    )]
    pub message: Option<AiPublicMessage>,
}

/// Read-only authorized toolpolicy metadata; protected payloads remain private.
#[backend_selected_graphql_entity(
    table = "graphql_orm_ai_tool_policies",
    plural = "AiPublicToolPolicies",
    schema_policy = "external_read_only",
    auth = "required",
    read_policy = "ai.public.read",
    write_policy = "ai.public.never_write",
    backup = false
)]
#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[graphql_orm(policy_binding = "ai.public.metadata")]
pub struct AiPublicToolPolicy {
    /// Authorized id metadata.
    #[primary_key]
    #[sortable]
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "uuid")]
    pub id: Uuid,
    /// Authorized scope kind metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub scope_kind: String,
    /// Authorized scope id metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub scope_id: String,
    /// Authorized tenant id metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub tenant_id: Option<String>,
    /// Authorized tool id metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub tool_id: String,
    /// Authorized tool fingerprint metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub tool_fingerprint: String,
    /// Authorized enabled metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "boolean")]
    pub enabled: bool,
    /// Authorized maximum maturity metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub maximum_maturity: String,
    /// Authorized approval rule metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "string")]
    pub approval_rule: String,
    /// Authorized maximum calls metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "number")]
    pub maximum_calls: i64,
    /// Authorized maximum output bytes metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "number")]
    pub maximum_output_bytes: i64,
    /// Authorized updated at metadata.
    #[graphql_orm(non_exportable, read_policy = "ai.public.field")]
    #[filterable(type = "number")]
    pub updated_at: i64,
}

// Relation resolvers select foreign-key tuples themselves; this adapter supplies
// each public projection's typed primary identity for the standard loader.
macro_rules! impl_public_metadata_batch_loader {
    ($($entity:ty),+ $(,)?) => {
        $(
            impl<B: OrmBackend> graphql_orm::graphql::loaders::BatchLoadEntity<B> for $entity
            where
                $entity: FromSqlRow<B>,
            {
                fn batch_column() -> &'static str {
                    "id"
                }

                fn batch_key_from_row(row: &B::Row) -> graphql_orm::Result<String> {
                    let entity = <Self as FromSqlRow<B>>::from_row(row)?;
                    Ok(entity.id.to_string())
                }

                fn batch_relation_key_from_row(
                    row: &B::Row,
                ) -> graphql_orm::Result<graphql_orm::graphql::loaders::RelationKey> {
                    let entity = <Self as FromSqlRow<B>>::from_row(row)?;
                    Ok(graphql_orm::graphql::loaders::RelationKey::from_sql_values(&[
                        SqlValue::Uuid(entity.id),
                    ]))
                }
            }
        )+
    };
}

impl_public_metadata_batch_loader!(
    AiPublicSession,
    AiPublicMessage,
    AiPublicRun,
    AiPublicAttachment,
    AiPublicToolPolicy,
);
