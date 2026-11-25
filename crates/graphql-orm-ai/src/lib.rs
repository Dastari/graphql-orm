//! Project-agnostic AI agent runtime for `graphql-orm` applications.
//!
//! The crate is intentionally built around default-deny capabilities:
//! resolver metadata is discovery rather than authorization, application work
//! executes through the host's authenticated GraphQL context, and external
//! data egress requires a separate explicit decision.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

#[cfg(not(any(feature = "sqlite", feature = "postgres", feature = "mssql")))]
compile_error!("enable one graphql-orm-ai persistence backend");

#[cfg(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql")
))]
compile_error!(
    "the initial graphql-orm-ai schema module requires exactly one backend; explicit multi-backend schema modules are planned"
);

/// ORM backend selected by this crate's persistence feature.
#[cfg(feature = "sqlite")]
pub type AiOrmBackend = graphql_orm::prelude::SqliteBackend;
/// ORM backend selected by this crate's persistence feature.
#[cfg(feature = "postgres")]
pub type AiOrmBackend = graphql_orm::prelude::PostgresBackend;
/// ORM backend selected by this crate's persistence feature.
#[cfg(feature = "mssql")]
pub type AiOrmBackend = graphql_orm::prelude::MssqlBackend;

#[cfg(all(test, any(feature = "postgres", feature = "mssql")))]
#[path = "../tests/support/durable_database.rs"]
mod durable_database;

mod access;
mod agent_loop;
mod approvals;
mod attachments;
mod sha256;
use sha256::valid_sha256;
mod budget;
mod capability_delivery;
mod configuration;
mod content_protection;
mod control_plane;
mod custody_field_policy;
mod dependent_effects;
mod domain;
mod egress;
mod entity_references;
mod execution;
mod execution_rejection;
mod inbox;
mod live_delta;
#[cfg(feature = "local-harness")]
mod local_harness;
mod message_preview;
mod orm_approvals;
mod orm_attachments;
#[cfg(feature = "provider-openai-core")]
mod orm_background;
mod orm_budget;
mod orm_budget_provisioning;
mod orm_checkpoints;
mod orm_configuration;
mod orm_context_compaction;
mod orm_coordinator;
mod orm_egress;
mod orm_inbox;
mod orm_live_delta;
mod orm_native_approvals;
mod orm_pricing;
mod orm_proposals;
mod orm_provider_output;
mod orm_provider_session;
mod orm_read_bounds;
mod orm_restore;
mod orm_rules;
mod orm_run_cancellation;
mod orm_run_disposition;
mod orm_runs;
mod orm_runtime_recovery;
mod orm_session_activity;
mod orm_session_execution;
mod orm_session_retention;
mod orm_session_title_work;
mod orm_sessions;
mod orm_skills;
mod orm_subscription_waits;
mod orm_subscriptions;
mod orm_supervised;
mod orm_supervised_coordinator;
mod orm_support;
mod orm_tool_result_preview;
mod orm_tools;
mod orm_ui_intents;
mod orm_usage;
#[cfg(feature = "provider-openai-core")]
mod orm_webhooks;
mod persisted_state;
mod persistence;
mod pricing;
mod principal_projection;
mod proposals;
mod provider;
mod provider_activity;
mod provider_calls;
mod provider_projection;
mod provider_run;
mod provider_session;
mod providers;
mod remote_execution;
mod restore;
mod rules;
mod run_authorization;
mod run_cancellation;
mod run_disposition;
mod run_state;
mod runtime;
mod schema_upgrade;
mod secrets;
mod session_execution;
mod session_retention;
mod session_title_work;
mod sessions;
mod skills;
mod subscription_waits;
mod subscriptions;
mod telemetry;
mod tool_media;
mod tool_result_preview;
mod tools;
mod ui_intents;
mod usage;

pub use access::*;
pub use agent_loop::*;
pub use approvals::*;
pub use attachments::*;
pub use budget::*;
pub use capability_delivery::*;
pub use configuration::*;
pub use content_protection::*;
pub use control_plane::ai_control_plane_root_fields;
pub use dependent_effects::{
    AiDependentEffect, AiDependentEffectKind, AiDependentEffectPolicy, AiDependentEffectRefusal,
};
pub use domain::*;
pub use egress::*;
pub use entity_references::{
    AI_ENTITY_REFERENCE_BLOCK_KIND, AI_ENTITY_REFERENCE_ENVELOPE_RESERVED_BYTES,
    AI_ENTITY_REFERENCE_MODEL_BLOCK_TYPE, AI_ENTITY_REFERENCE_PROVENANCE,
    AI_MAXIMUM_ENTITY_REFERENCE_DELIVERY_AGE, AiEntityReferenceInput, AiEntityReferenceLimits,
    AiEntityReferenceNotice, AiEntityReferenceNoticeCode, AiEntityReferenceRegistry,
    AiEntityReferenceResolver, AiEntityReferenceUse, AiMessageEntityReferenceInput,
    AiResolvedEntityReference, ModelDispatchReferences, ModelDispatchRequest,
    is_entity_reference_block_kind, withhold_request_entity_references,
};
pub use execution::*;
pub use graphql_orm_ai_tool_profiles::*;
// Disambiguate the public helper from the crate-private ORM wrapper.
pub use graphql_orm_ai_tool_profiles::canonical_json_hash;
pub use inbox::*;
pub use live_delta::*;
#[cfg(feature = "local-harness")]
pub use local_harness::*;
pub use orm_approvals::*;
pub use orm_attachments::*;
#[cfg(feature = "provider-openai-core")]
pub use orm_background::*;
pub use orm_budget::*;
pub use orm_budget_provisioning::{
    AI_DEPLOYMENT_ACTOR_KIND, AI_DEPLOYMENT_BUDGET_POLICY_ACTION, AiDeploymentActor,
    AiDeploymentBudgetPolicy, AiDeploymentBudgetPolicyOutcome, OrmAiBudgetPolicyProvisioner,
};
pub use orm_checkpoints::*;
pub use orm_configuration::*;
pub use orm_context_compaction::*;
pub use orm_coordinator::*;
pub use orm_egress::*;
pub use orm_inbox::*;
pub use orm_live_delta::*;
pub use orm_native_approvals::*;
pub use orm_pricing::*;
pub use orm_proposals::*;
pub use orm_provider_output::*;
pub use orm_provider_session::*;
pub use orm_restore::*;
pub use orm_rules::*;
pub use orm_run_cancellation::*;
pub use orm_run_disposition::*;
pub use orm_runs::*;
pub use orm_runtime_recovery::*;
pub use orm_session_execution::*;
pub use orm_session_retention::*;
pub use orm_session_title_work::*;
pub use orm_sessions::*;
pub use orm_skills::*;
pub use orm_subscription_waits::*;
pub use orm_subscriptions::*;
pub use orm_supervised::*;
pub use orm_supervised_coordinator::*;
pub use orm_tool_result_preview::*;
pub use orm_tools::*;
pub use orm_ui_intents::*;
pub use orm_usage::*;
#[cfg(feature = "provider-openai-core")]
pub use orm_webhooks::*;
pub use persistence::{
    AI_SCHEMA_MODULE_ID, AI_SCHEMA_MODULE_VERSION, AI_TABLE_NAMESPACE, AiSchemaModule,
};
pub use pricing::*;
pub use principal_projection::{
    AiPrincipalNamespace, AiPrincipalProjection, AiPrincipalProjector,
    MAX_AI_PRINCIPAL_NAMESPACE_BYTES,
};
pub use proposals::*;
pub use provider::*;
pub use provider_activity::*;
pub use provider_calls::*;
pub use provider_projection::*;
pub use provider_run::*;
pub use provider_session::*;
pub use providers::*;
pub use remote_execution::*;
pub use restore::*;
pub use rules::*;
pub use run_authorization::*;
pub use run_cancellation::*;
pub use run_disposition::*;
pub use run_state::*;
pub use runtime::*;
pub use secrets::*;
pub use session_execution::*;
pub use session_retention::*;
pub use session_title_work::*;
pub use sessions::*;
pub use skills::*;
pub use subscription_waits::*;
pub use subscriptions::*;
pub use telemetry::*;
pub use tool_media::*;
pub use tool_result_preview::*;
pub use tools::*;
pub use ui_intents::*;
pub use usage::*;

/// Common imports for host integrations.
pub mod prelude {
    pub use crate::ai_scope_key;
    pub use crate::{
        AiAccessPolicy, AiAgentRuleResolution, AiApprovalAccessPolicy, AiApprovalBinding,
        AiAttachmentAcceptancePolicy, AiAttachmentCleanupReport, AiAttachmentCleanupService,
        AiAttachmentScanner, AiAttachmentService, AiAttachmentUploadService,
        AiBrowserResultPreviewPolicy, AiBudgetReservation, AiBudgetService,
        AiContentProtectionPolicy, AiDataSourceRef, AiDisclosureSchema, AiEgressDecision,
        AiEgressDecisionAudit, AiEgressManifest, AiEgressPolicy, AiError, AiInboxPruningService,
        AiInboxService, AiLiveDeltaCoalescerLimits, AiOperationalTelemetry,
        AiOperationalTelemetryEvent, AiOperationalTelemetrySink, AiPricingCatalogService,
        AiPricingQuoteService, AiProposalAccessPolicy, AiProposalCatalog, AiProposalTypeDescriptor,
        AiProvider, AiProviderAttachmentRequest, AiProviderAttachmentResolver,
        AiProviderFileDeletionRequest, AiProviderFileDeletionService,
        AiRemoteAuthenticatedGraphqlAdapter, AiRemoteGraphqlAuthority,
        AiRemoteGraphqlAuthorityIssuer, AiRemoteGraphqlCapabilityBinding,
        AiRemoteGraphqlCapabilityKind, AiRemoteGraphqlDelegationRequest,
        AiRemoteGraphqlExecutionLimits, AiRemoteGraphqlTransport, AiResolvedProviderAttachment,
        AiRuleAccessPolicy, AiRuleDeploymentLimits, AiRuleHierarchyResolver, AiRulePolicyService,
        AiRuleRunUsage, AiRunCancellationHub, AiRunCancellationService, AiRuntime,
        AiRuntimeBuilder, AiScope, AiSecretStore, AiSessionRetentionService,
        AiSessionTitleWorkService, AiSkillAccessPolicy, AiSkillCatalogService,
        AiToolAuthorizationPolicy, AiToolCallResultPreviewService, AiToolCatalog, AiToolDescriptor,
        AiToolResultPreviewAuthorizer, AiUiIntentCatalog, AiUiIntentTypeDescriptor,
        AiUsageAccessPolicy, AiUsageService, DataClassification, SecretRef, ToolMaturity,
    };
    pub use crate::{
        AiAdoptedAutomaticMutationBatch, AiAdoptedClassifiedMutationBatch,
        AiAdoptedReadOnlyToolBatch, AiAdoptedSupervisedProviderTurn, AiAdoptedSupervisedToolBatch,
        AiAgentCheckpointAdopter, AiAgentCheckpointWriter, AiAgentProviderOutputWriter,
        AiAgentProviderTurnExecutor, AiAgentRuleResolver, AiAgentRunControl,
        AiAgentSupervisedApprovalStager, AiAgentSupervisedCheckpointControl,
        AiAgentSupervisedResumeExecutor, AiApplicationToolCallLimits, AiApprovalServiceLimits,
        AiApprovalWaitPolicyContext, AiApprovalWaitPolicyDecision,
        AiApprovalWaitReconciliationLimits, AiApprovalWaitReconciliationPolicy,
        AiApprovalWaitReconciliationReport, AiApprovedRunClaim, AiAttachmentCleanupLimits,
        AiAttachmentServiceLimits, AiBudgetServiceLimits, AiCanonicalActionPreviewBuilder,
        AiConsequentialToolCallOutcome, AiContextCompactionLimits, AiContextSourceMessage,
        AiCoordinatorCheckpointLimits, AiCurrentRuleResolverLimits, AiInboxPruningLimits,
        AiLiveDeltaPersistenceContext, AiLiveDeltaPersistenceLimits, AiLiveDeltaSink,
        AiLoadedContextCheckpoint, AiPersistedContextCheckpoint, AiPreparedContextCompaction,
        AiProposalServiceLimits, AiProtectedSupervisedToolBatch,
        AiProviderAttachmentResolutionLimits, AiProviderCallExecutor, AiProviderCallLimits,
        AiProviderOutputLimits, AiProviderUsageAccounting, AiReadOnlyAgentCoordinator,
        AiReadOnlyAgentCoordinatorLimits, AiReadOnlyAgentTurnPlan, AiReadOnlyAgentTurnPlanner,
        AiRequestedConsequentialToolCall, AiRestoreAttachmentMetadataAuditLimits,
        AiRestoreCollectorLimits, AiRestorePolicyAuditLimits, AiRunCancellationLimits,
        AiRunServiceLimits, AiSessionRetentionLimits, AiSessionTitleWorkLimits,
        AiSupervisedAgentCoordinator, AiSupervisedAgentCoordinatorLimits,
        AiSupervisedAgentRunOutcome, AiSupervisedAgentTurnPlan, AiSupervisedAgentTurnPlanner,
        AiSupervisedApprovalWait, AiSupervisedResumeOutcome, AiUiIntentDeliveryLimits,
        AiUiIntentDeliveryService, DeploymentAiCurrentRuleResolver,
        OrmAiApplicationToolCallService, OrmAiApprovalService,
        OrmAiApprovalWaitReconciliationService, OrmAiAttachmentService, OrmAiBudgetService,
        OrmAiConsequentialToolCallService, OrmAiContextCompactionService,
        OrmAiCoordinatorCheckpointService, OrmAiCurrentRuleResolver, OrmAiEgressDecisionAudit,
        OrmAiInboxPruningService, OrmAiInboxService, OrmAiLiveDeltaService, OrmAiPricingService,
        OrmAiProposalService, OrmAiProviderOutputService, OrmAiRestoreFactCollector,
        OrmAiRulePolicyService, OrmAiRunCancellationService, OrmAiRunService,
        OrmAiSessionRetentionService, OrmAiSessionTitleWorkService, OrmAiSkillCatalogService,
        OrmAiSupervisedResumeService, OrmAiToolCallResultPreviewService,
        OrmAiUiIntentDeliveryService, OrmAiUsageService,
    };
    #[cfg(feature = "provider-openai-core")]
    pub use crate::{
        AiOpenAiBackgroundReconciliationClaim, AiOpenAiBackgroundReconciliationLimits,
        AiOpenAiBackgroundReconciliationWindows, AiOpenAiBackgroundRetrievalAttempt,
        AiOpenAiBackgroundRetrievalFailure, AiOpenAiBackgroundRetrievalLimits,
        AiOpenAiBackgroundRetrievalObservation, AiOpenAiBackgroundRetrievalRoute,
        AiOpenAiBackgroundSubmission, AiOpenAiBackgroundTerminalOutcome,
        AiProviderWebhookReceiptOutcome, AiTrustedWorkerEntityFallback,
        OrmAiOpenAiBackgroundReconciliationService, OrmAiOpenAiBackgroundRetrievalService,
        OrmAiOpenAiBackgroundSubmissionService, OrmAiOpenAiBackgroundTerminalService,
        OrmAiProviderWebhookReceiptService, ProviderBackgroundStatus, ProviderBackgroundUsage,
    };
    #[cfg(feature = "provider-openai-core")]
    pub use crate::{
        OpenAiVerifiedWebhookEvent, OpenAiWebhookEventKind, OpenAiWebhookHeaders,
        OpenAiWebhookVerifier, OpenAiWebhookVerifierLimits,
    };
    pub use agql_auth::{CurrentPrincipalResolver, PrincipalReference, ResolvedPrincipal};
}
