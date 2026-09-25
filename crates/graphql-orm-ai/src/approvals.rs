//! Exact, expiring, one-shot approval bindings for consequential tool calls.

use std::sync::Arc;

use agql_auth::{AuthPrincipal, PrincipalReference};
use async_graphql::{Context, Enum, ErrorExtensions, InputObject, Object, SimpleObject};
use async_trait::async_trait;
use graphql_orm::graphql::pagination::{
    KeysetConnectionInput, PageInfo, ValidatedKeysetConnection,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{AiApprovalId, AiError, AiScope, AiSessionId, AiToolCallId, GraphqlOperationContract};

/// Opaque application resource and optimistic-concurrency precondition.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AiApprovalResourceBinding {
    /// Host-defined resource type.
    pub resource_type: String,
    /// Opaque resource identifier.
    pub resource_id: String,
    /// Expected row version, ETag, or host-generated precondition digest.
    pub expected_version: String,
}

/// Server-generated canonical action preview shown to an approver.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiCanonicalActionPreview {
    /// Stable host-defined action kind.
    pub action_kind: String,
    /// Server-authored concise title.
    pub title: String,
    /// Typed target/precondition bindings included in the action.
    pub targets: Vec<AiApprovalResourceBinding>,
    /// Server-generated bounded structured diff/impact facts.
    pub details: serde_json::Value,
}

impl AiCanonicalActionPreview {
    /// Returns a stable hash suitable for approval binding.
    pub fn stable_hash(&self) -> String {
        let mut canonical = self.clone();
        canonical.targets.sort();
        let encoded = serde_json::to_vec(&canonical)
            .expect("AiCanonicalActionPreview consists only of serializable values");
        hex::encode(Sha256::digest(encoded))
    }
}

/// Complete action envelope to which one approval is bound.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiApprovalBinding {
    /// Tool call awaiting approval.
    pub tool_call_id: AiToolCallId,
    /// Session owning the action.
    pub session_id: AiSessionId,
    /// Scope and optional tenant boundary.
    pub scope: AiScope,
    /// Exact reviewed static descriptor or generated capability fingerprint.
    pub tool_fingerprint: String,
    /// Canonical validated variables/arguments hash.
    pub argument_hash: String,
    /// Exact local/remote GraphQL target and operation contract.
    pub operation: GraphqlOperationContract,
    /// Fingerprint of the safe durable principal reference.
    pub principal_reference_fingerprint: String,
    /// Original/delegated actor subject when applicable.
    pub delegated_actor_subject: Option<String>,
    /// Safe delegation/grant reference, never a token.
    pub delegation_reference: Option<String>,
    /// Current tool/scope/application policy version.
    pub policy_version: String,
    /// Host-generated safe authorization-state/precondition digest.
    pub authorization_state_digest: String,
    /// Exact target resources and optimistic-concurrency preconditions.
    pub resources: Vec<AiApprovalResourceBinding>,
    /// Hash of the server-generated canonical action preview.
    pub preview_hash: String,
}

impl AiApprovalBinding {
    /// Computes a stable hash over the complete approval envelope.
    pub fn stable_hash(&self) -> String {
        let mut canonical = self.clone();
        canonical.resources.sort();
        let encoded = serde_json::to_vec(&canonical)
            .expect("AiApprovalBinding consists only of serializable values");
        hex::encode(Sha256::digest(encoded))
    }

    /// Validates that required policy and operation bindings are present.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] when a required binding is
    /// empty, target resources are duplicated, or the preview hash is stale.
    pub fn validate(&self, preview: &AiCanonicalActionPreview) -> Result<(), AiError> {
        validate_preview_resources(preview, &self.resources, &self.preview_hash)?;
        if self.tool_fingerprint.trim().is_empty()
            || self.argument_hash.trim().is_empty()
            || self.policy_version.trim().is_empty()
            || self.authorization_state_digest.trim().is_empty()
            || !self.operation.generated_operation_shape_is_valid()
        {
            return Err(AiError::InvalidConfiguration(
                "approval binding is incomplete or stale".to_owned(),
            ));
        }
        Ok(())
    }

    /// Fingerprints a safe principal reference without preserving roles,
    /// scopes, or any credential material.
    pub fn principal_fingerprint(reference: &PrincipalReference) -> String {
        let encoded = serde_json::to_vec(reference)
            .expect("PrincipalReference consists only of serializable values");
        hex::encode(Sha256::digest(encoded))
    }
}

/// Maximum serialized JSON bytes in canonical approval preview details (2 MiB).
/// This is a representation limit, including JSON escaping, not an execution limit.
pub const AI_APPROVAL_PREVIEW_DETAILS_MAX_BYTES: usize = 2 * 1024 * 1024;

pub(crate) fn validate_preview_resources(
    preview: &AiCanonicalActionPreview,
    resources: &[AiApprovalResourceBinding],
    expected_preview_hash: &str,
) -> Result<(), AiError> {
    let preview_bytes = serde_json::to_vec(&preview.details)
        .map_err(|_| AiError::InvalidConfiguration("approval preview is invalid".to_owned()))?;
    if preview.action_kind.trim().is_empty()
        || preview.action_kind.len() > 200
        || preview.title.trim().is_empty()
        || preview.title.len() > 1_024
        || preview.targets.len() > 100
        || resources.len() > 100
        || preview_bytes.len() > AI_APPROVAL_PREVIEW_DETAILS_MAX_BYTES
        || expected_preview_hash != preview.stable_hash()
    {
        return Err(AiError::InvalidConfiguration(
            "approval preview is incomplete or stale".to_owned(),
        ));
    }
    let mut resources = resources.to_vec();
    resources.sort();
    if resources.iter().any(|resource| {
        resource.resource_type.trim().is_empty()
            || resource.resource_type.len() > 200
            || resource.resource_id.trim().is_empty()
            || resource.resource_id.len() > 1_024
            || resource.expected_version.trim().is_empty()
            || resource.expected_version.len() > 1_024
    }) || resources.windows(2).any(|window| window[0] == window[1])
    {
        return Err(AiError::InvalidConfiguration(
            "approval resource binding is invalid".to_owned(),
        ));
    }
    let mut targets = preview.targets.clone();
    targets.sort();
    if resources != targets {
        return Err(AiError::InvalidConfiguration(
            "approval preview targets do not match action resources".to_owned(),
        ));
    }
    Ok(())
}

/// Persisted lifecycle state for a one-shot approval.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiApprovalState {
    /// Awaiting an authorized human decision.
    Pending,
    /// Approved for one exact future consumption.
    Approved,
    /// Claimed by one fenced worker for immediate fresh validation and
    /// one-shot consumption; the action has not executed.
    ResumeClaimed,
    /// Explicitly denied.
    Denied,
    /// Binding or time window is no longer current.
    Expired,
    /// Previously approved authority was revoked.
    Revoked,
    /// The exact approved action was consumed once.
    Consumed,
}

/// Exact approved decision before transactional one-shot consumption.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiApprovalGrant {
    /// Approval identifier.
    pub id: AiApprovalId,
    /// Complete action-envelope hash.
    pub binding_hash: String,
    /// Human approver subject.
    pub approver_subject: String,
    /// Current approval state.
    pub state: AiApprovalState,
    /// Decision timestamp.
    pub approved_at: OffsetDateTime,
    /// Exclusive expiry timestamp.
    pub expires_at: OffsetDateTime,
}

impl AiApprovalGrant {
    /// Validates this grant against a freshly rebuilt action envelope.
    ///
    /// This check does not consume the approval and does not replace fresh
    /// resolver authorization. Persistence must atomically transition the
    /// matching approved row to `Consumed` before executing a side effect.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::Forbidden`] for a stale, expired, mismatched,
    /// or non-consumable grant state.
    pub fn authorize(
        &self,
        current_binding: &AiApprovalBinding,
        now: OffsetDateTime,
    ) -> Result<AuthorizedAiApproval, AiError> {
        if !matches!(
            self.state,
            AiApprovalState::Approved | AiApprovalState::ResumeClaimed
        ) || now < self.approved_at
            || now >= self.expires_at
            || self.binding_hash != current_binding.stable_hash()
        {
            return Err(AiError::Forbidden);
        }
        Ok(AuthorizedAiApproval {
            approval_id: self.id,
            binding_hash: self.binding_hash.clone(),
        })
    }
}

/// Opaque proof that an unexpired grant matched a freshly rebuilt action envelope.
#[derive(Clone, Debug)]
pub struct AuthorizedAiApproval {
    approval_id: AiApprovalId,
    binding_hash: String,
}

/// Opaque proof that the exact grant was atomically consumed once.
///
/// This proves only one-shot approval consumption. It does not prove current
/// resolver authorization, unchanged resource versions, or successful domain
/// mutation execution.
#[derive(Clone, Debug)]
pub struct ConsumedAiApproval {
    approval_id: AiApprovalId,
    binding_hash: String,
    consumed_at: OffsetDateTime,
}

impl ConsumedAiApproval {
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) fn new(authorized: AuthorizedAiApproval, consumed_at: OffsetDateTime) -> Self {
        Self {
            approval_id: authorized.approval_id,
            binding_hash: authorized.binding_hash,
            consumed_at,
        }
    }

    /// Consumed approval identifier.
    pub const fn approval_id(&self) -> AiApprovalId {
        self.approval_id
    }

    /// Exact action-envelope hash that was consumed.
    pub fn binding_hash(&self) -> &str {
        &self.binding_hash
    }

    /// Atomic consumption timestamp.
    pub const fn consumed_at(&self) -> OffsetDateTime {
        self.consumed_at
    }
}

/// Approval lifecycle action evaluated by the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiApprovalAction {
    /// Persist a pending approval for a current consequential tool call.
    Request,
    /// Read pending or historical approval state and canonical preview.
    Read,
    /// Approve or deny a pending request.
    Decide,
    /// Revoke a previously approved request before consumption.
    Revoke,
    /// Consume the exact grant immediately before fresh resolver execution.
    Consume,
}

/// Verified approval access evidence supplied only by the ORM approval service.
///
/// Retained evidence validates protected resources and preview against immutable
/// approval metadata. It is not a reconstructed complete `AiApprovalBinding`,
/// an execution grant, or a substitute for current host resource authorization.
/// Private construction prevents callers supplying arbitrary evidence to the service.
#[derive(Clone)]
pub struct AiApprovalAccessEvidence {
    pub(crate) approval_id: AiApprovalId,
    pub(crate) tool_call_id: AiToolCallId,
    pub(crate) binding_hash: String,
    pub(crate) tool_fingerprint: String,
    pub(crate) argument_hash: String,
    pub(crate) principal_reference_fingerprint: String,
    pub(crate) execution_target_id: String,
    pub(crate) target_schema_fingerprint: String,
    pub(crate) operation_name: String,
    pub(crate) operation_document_hash: String,
    pub(crate) result_projection_fingerprint: String,
    pub(crate) disclosure_schema_fingerprint: String,
    pub(crate) policy_version: String,
    pub(crate) authorization_state_digest: String,
    pub(crate) resources: Vec<AiApprovalResourceBinding>,
    pub(crate) preview: AiCanonicalActionPreview,
}

impl AiApprovalAccessEvidence {
    /// Approval identity.
    pub fn approval_id(&self) -> AiApprovalId {
        self.approval_id
    }
    /// Bound tool call identity.
    pub fn tool_call_id(&self) -> AiToolCallId {
        self.tool_call_id
    }
    /// Original complete action envelope digest; not a reconstructed envelope.
    pub fn binding_hash(&self) -> &str {
        &self.binding_hash
    }
    /// Exact registered tool descriptor fingerprint.
    pub fn tool_fingerprint(&self) -> &str {
        &self.tool_fingerprint
    }
    /// Exact canonical variables digest.
    pub fn argument_hash(&self) -> &str {
        &self.argument_hash
    }
    /// Original principal reference fingerprint.
    pub fn principal_reference_fingerprint(&self) -> &str {
        &self.principal_reference_fingerprint
    }
    /// Registered GraphQL execution target.
    pub fn execution_target_id(&self) -> &str {
        &self.execution_target_id
    }
    /// Exact target schema fingerprint.
    pub fn target_schema_fingerprint(&self) -> &str {
        &self.target_schema_fingerprint
    }
    /// Registered operation name.
    pub fn operation_name(&self) -> &str {
        &self.operation_name
    }
    /// Exact registered operation document digest.
    pub fn operation_document_hash(&self) -> &str {
        &self.operation_document_hash
    }
    /// Bound result projection fingerprint.
    pub fn result_projection_fingerprint(&self) -> &str {
        &self.result_projection_fingerprint
    }
    /// Bound disclosure schema fingerprint.
    pub fn disclosure_schema_fingerprint(&self) -> &str {
        &self.disclosure_schema_fingerprint
    }
    /// Policy version recorded for this action.
    pub fn policy_version(&self) -> &str {
        &self.policy_version
    }
    /// Authorization state digest recorded for this action.
    pub fn authorization_state_digest(&self) -> &str {
        &self.authorization_state_digest
    }
    /// Verified exact resource and precondition bindings.
    pub fn resources(&self) -> &[AiApprovalResourceBinding] {
        &self.resources
    }
    /// Verified bounded server-authored preview; may contain sensitive action details.
    pub fn preview(&self) -> &AiCanonicalActionPreview {
        &self.preview
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) fn from_binding(
        id: AiApprovalId,
        binding: &AiApprovalBinding,
        preview: &AiCanonicalActionPreview,
    ) -> Self {
        Self {
            approval_id: id,
            tool_call_id: binding.tool_call_id,
            binding_hash: binding.stable_hash(),
            tool_fingerprint: binding.tool_fingerprint.clone(),
            argument_hash: binding.argument_hash.clone(),
            principal_reference_fingerprint: binding.principal_reference_fingerprint.clone(),
            execution_target_id: binding.operation.target_id.as_str().to_owned(),
            target_schema_fingerprint: binding.operation.schema_fingerprint.clone(),
            operation_name: binding.operation.operation_name.clone(),
            operation_document_hash: binding.operation.document_hash.clone(),
            result_projection_fingerprint: binding.operation.result_projection_fingerprint.clone(),
            disclosure_schema_fingerprint: binding.operation.disclosure_schema_fingerprint.clone(),
            policy_version: binding.policy_version.clone(),
            authorization_state_digest: binding.authorization_state_digest.clone(),
            resources: binding.resources.clone(),
            preview: preview.clone(),
        }
    }
}

/// Host-owned approval authorization policy.
#[async_trait]
pub trait AiApprovalAccessPolicy: Send + Sync {
    /// Decides one exact approval action for the current principal and scope.
    async fn can_access_approval(
        &self,
        principal: &AuthPrincipal,
        scope: &AiScope,
        session_id: AiSessionId,
        action: AiApprovalAction,
    ) -> bool;

    /// Applies current host authorization to verified exact approval evidence.
    ///
    /// The service invokes the coarse gate before opening protected contents,
    /// then invokes this hook before disclosure, decision, or consumption. Hosts
    /// may require exact resource authority here. The compatible default forwards
    /// to the coarse policy; neither hook replaces fresh execution authorization.
    async fn can_access_bound_approval(
        &self,
        principal: &AuthPrincipal,
        scope: &AiScope,
        session_id: AiSessionId,
        action: AiApprovalAction,
        _evidence: &AiApprovalAccessEvidence,
    ) -> bool {
        self.can_access_approval(principal, scope, session_id, action)
            .await
    }
}

/// Human approval decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Enum)]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_items = "PascalCase"))]
pub enum AiApprovalDecision {
    /// Approve one exact future consumption.
    Approve,
    /// Deny the pending action.
    Deny,
}

/// Authorized/decrypted approval view.
#[derive(Clone, Debug, SimpleObject)]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_fields = "PascalCase"))]
pub struct AiApprovalView {
    /// Approval identifier.
    pub id: Uuid,
    /// Bound tool-call identifier.
    pub tool_call_id: Uuid,
    /// Owning session.
    pub session_id: Uuid,
    /// Server-generated canonical action preview.
    pub canonical_preview: async_graphql::Json<serde_json::Value>,
    /// Pending/approved/`resume_claimed`/denied/expired/revoked/consumed state.
    pub state: String,
    /// Whether approval required recent MFA.
    pub recent_mfa_required: bool,
    /// Human approver subject after a decision.
    pub approver_subject: Option<String>,
    /// Creation time in Unix seconds.
    pub created_at: i64,
    /// Exclusive expiry in Unix seconds.
    pub expires_at: i64,
    /// Decision time in Unix seconds.
    pub decided_at: Option<i64>,
    /// One-shot consumption time in Unix seconds.
    pub consumed_at: Option<i64>,
    /// Current CAS version.
    pub row_version: i64,
}

/// Approval connection edge.
#[derive(Clone, Debug, SimpleObject)]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_fields = "PascalCase"))]
pub struct AiApprovalEdge {
    /// Approval node.
    pub node: AiApprovalView,
    /// Opaque keyset cursor.
    pub cursor: String,
}

/// Bounded approval connection.
#[derive(Clone, Debug, SimpleObject)]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_fields = "PascalCase"))]
pub struct AiApprovalConnection {
    /// Bounded edges.
    pub edges: Vec<AiApprovalEdge>,
    /// Relay page metadata.
    pub page_info: PageInfo,
}

/// CAS-bound approval decision input.
#[derive(Clone, Debug, InputObject)]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_fields = "PascalCase"))]
pub struct DecideAiApprovalInput {
    /// Approval identifier.
    pub id: Uuid,
    /// Approve or deny.
    pub decision: AiApprovalDecision,
    /// Exact row version rendered with the canonical preview.
    pub expected_version: i64,
}

/// CAS-bound approval revocation input.
#[derive(Clone, Debug, InputObject)]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_fields = "PascalCase"))]
pub struct RevokeAiApprovalInput {
    /// Approval identifier.
    pub id: Uuid,
    /// Exact row version observed by the revoker.
    pub expected_version: i64,
}

/// Authenticated, scope-aware approval lifecycle exposed to GraphQL.
#[async_trait]
pub trait AiApprovalService: Send + Sync {
    /// Lists a bounded approval window visible in one session.
    async fn approvals(
        &self,
        principal: &AuthPrincipal,
        session_id: AiSessionId,
        page: ValidatedKeysetConnection,
    ) -> Result<AiApprovalConnection, AiError>;

    /// Loads one visible approval.
    async fn approval(
        &self,
        principal: &AuthPrincipal,
        approval_id: AiApprovalId,
    ) -> Result<Option<AiApprovalView>, AiError>;

    /// Applies one CAS-bound human approval decision.
    async fn decide_approval(
        &self,
        principal: &AuthPrincipal,
        input: DecideAiApprovalInput,
    ) -> Result<AiApprovalView, AiError>;

    /// Revokes an approved but unconsumed grant.
    async fn revoke_approval(
        &self,
        principal: &AuthPrincipal,
        input: RevokeAiApprovalInput,
    ) -> Result<AiApprovalView, AiError>;
}

/// Composable approval query root.
#[derive(Clone, Copy, Debug, Default)]
pub struct AiApprovalQueryRoot;

#[cfg_attr(
    feature = "graphql-case-pascal",
    Object(rename_fields = "PascalCase", rename_args = "PascalCase")
)]
#[cfg_attr(not(feature = "graphql-case-pascal"), Object)]
impl AiApprovalQueryRoot {
    /// Returns a bounded approval window for one visible session.
    async fn ai_approvals(
        &self,
        context: &Context<'_>,
        session_id: Uuid,
        #[graphql(default)] page: KeysetConnectionInput,
    ) -> async_graphql::Result<AiApprovalConnection> {
        let principal = agql_auth::principal_from_ctx(context)?;
        let page = page.validate(50, 200).map_err(|error| (&error).extend())?;
        approval_service(context)?
            .approvals(&principal, AiSessionId(session_id), page)
            .await
            .map_err(extend)
    }

    /// Returns one visible approval and its canonical preview.
    async fn ai_approval(
        &self,
        context: &Context<'_>,
        id: Uuid,
    ) -> async_graphql::Result<Option<AiApprovalView>> {
        let principal = agql_auth::principal_from_ctx(context)?;
        approval_service(context)?
            .approval(&principal, AiApprovalId(id))
            .await
            .map_err(extend)
    }
}

/// Composable approval mutation root.
#[derive(Clone, Copy, Debug, Default)]
pub struct AiApprovalMutationRoot;

#[cfg_attr(
    feature = "graphql-case-pascal",
    Object(rename_fields = "PascalCase", rename_args = "PascalCase")
)]
#[cfg_attr(not(feature = "graphql-case-pascal"), Object)]
impl AiApprovalMutationRoot {
    /// Approves or denies one exact pending action.
    async fn decide_ai_approval(
        &self,
        context: &Context<'_>,
        input: DecideAiApprovalInput,
    ) -> async_graphql::Result<AiApprovalView> {
        let principal = agql_auth::principal_from_ctx(context)?;
        approval_service(context)?
            .decide_approval(&principal, input)
            .await
            .map_err(extend)
    }

    /// Revokes an approved, unconsumed grant.
    async fn revoke_ai_approval(
        &self,
        context: &Context<'_>,
        input: RevokeAiApprovalInput,
    ) -> async_graphql::Result<AiApprovalView> {
        let principal = agql_auth::principal_from_ctx(context)?;
        approval_service(context)?
            .revoke_approval(&principal, input)
            .await
            .map_err(extend)
    }
}

fn approval_service(context: &Context<'_>) -> async_graphql::Result<Arc<dyn AiApprovalService>> {
    context
        .data::<Arc<dyn AiApprovalService>>()
        .cloned()
        .map_err(|_| {
            AiError::InvalidConfiguration("AI approval service is not installed".to_owned())
                .extend()
        })
}

fn extend(error: AiError) -> async_graphql::Error {
    error.extend()
}

impl AuthorizedAiApproval {
    /// Returns the approval identifier for atomic consumption and audit linkage.
    pub const fn approval_id(&self) -> AiApprovalId {
        self.approval_id
    }

    /// Returns the exact action-envelope hash.
    pub fn binding_hash(&self) -> &str {
        &self.binding_hash
    }
}

#[cfg(test)]
mod preview_bound_tests {
    use super::*;

    fn preview(details: serde_json::Value) -> AiCanonicalActionPreview {
        AiCanonicalActionPreview {
            action_kind: "bounded.action".to_owned(),
            title: "Exact action".to_owned(),
            targets: vec![],
            details,
        }
    }

    #[test]
    fn canonical_preview_allows_escape_heavy_quarter_megabyte_source() {
        let preview = preview(serde_json::json!({"source": "\0".repeat(256 * 1024)}));
        let bytes = serde_json::to_vec(&preview.details).unwrap();
        assert!(bytes.len() > 6 * 256 * 1024);
        assert!(validate_preview_resources(&preview, &[], &preview.stable_hash()).is_ok());
    }

    #[test]
    fn canonical_preview_details_has_exact_two_megabyte_json_ceiling() {
        let mut preview = preview(serde_json::Value::String(
            "x".repeat(AI_APPROVAL_PREVIEW_DETAILS_MAX_BYTES - 2),
        ));
        assert_eq!(
            serde_json::to_vec(&preview.details).unwrap().len(),
            AI_APPROVAL_PREVIEW_DETAILS_MAX_BYTES
        );
        assert!(validate_preview_resources(&preview, &[], &preview.stable_hash()).is_ok());
        preview.details =
            serde_json::Value::String("x".repeat(AI_APPROVAL_PREVIEW_DETAILS_MAX_BYTES - 1));
        assert!(validate_preview_resources(&preview, &[], &preview.stable_hash()).is_err());
    }
}
