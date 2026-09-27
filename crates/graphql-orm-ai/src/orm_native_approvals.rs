//! Non-executable preparation of consequential native callbacks.

use crate::orm_runs::PreparedToolLifecycleEvent;
use crate::{AiEgressManifest, AiRunLease, AiToolCallId, ModelInputBlock};

/// Closed runtime disposition of a native call that performed no application effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AiNativeToolControlKind {
    /// The exact action was prepared, and approval becomes available after the turn settles.
    ApprovalPending,
    /// An earlier action in this turn is awaiting approval, so this action was not admitted.
    ConsequentialCallsPaused,
}

impl AiNativeToolControlKind {
    pub(crate) fn model_value(self) -> serde_json::Value {
        serde_json::json!({
            "formatVersion": 1,
            "kind": "FrameworkToolControl",
            "status": match self {
                Self::ApprovalPending => "ApprovalPending",
                Self::ConsequentialCallsPaused => "ConsequentialCallsPaused",
            },
            "effectExecuted": false,
            "retryAllowed": false,
        })
    }
}

/// Protected and separately egress-authorized runtime control reply.
///
/// This proves only that a native callback received a durable no-effect
/// disposition. It is never a completed application result or an approval grant.
#[derive(Clone)]
pub struct AiNativeToolControlReceipt {
    pub(crate) tool_call_id: AiToolCallId,
    pub(crate) provider_call_id: String,
    pub(crate) kind: AiNativeToolControlKind,
    pub(crate) model_input: ModelInputBlock,
    pub(crate) egress_manifest: AiEgressManifest,
    pub(crate) lease: AiRunLease,
}

impl std::fmt::Debug for AiNativeToolControlReceipt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiNativeToolControlReceipt")
            .field("tool_call_id", &self.tool_call_id)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl AiNativeToolControlReceipt {
    /// Exact durable callback identity.
    pub const fn tool_call_id(&self) -> AiToolCallId {
        self.tool_call_id
    }

    /// Provider-owned call identity consumed by this one reply.
    pub fn provider_call_id(&self) -> &str {
        &self.provider_call_id
    }

    /// Closed no-effect disposition, distinct from a domain result.
    pub const fn kind(&self) -> AiNativeToolControlKind {
        self.kind
    }

    /// Separately authorized runtime reply for the exact native callback.
    pub fn model_input(&self) -> &ModelInputBlock {
        &self.model_input
    }

    /// Active run fence after receipt persistence; no human wait has started yet.
    pub fn lease(&self) -> &AiRunLease {
        &self.lease
    }

    pub(crate) fn egress_manifest(&self) -> &AiEgressManifest {
        &self.egress_manifest
    }
}

/// Durable preparation of an exact action while its native provider turn runs.
///
/// This is not an approval or an execution grant. The active run lease remains
/// held, and no resolver effect has occurred. Only the crate-owned finalization
/// path can create an ordinary approval after real provider usage settlement.
#[derive(Clone)]
pub struct AiPreparedNativeApproval {
    pub(crate) tool_call_id: AiToolCallId,
    pub(crate) provider_call_id: String,
    pub(crate) lease: AiRunLease,
}

impl std::fmt::Debug for AiPreparedNativeApproval {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiPreparedNativeApproval")
            .field("tool_call_id", &self.tool_call_id)
            .finish_non_exhaustive()
    }
}

impl AiPreparedNativeApproval {
    /// Exact durable tool call whose action has not executed.
    pub const fn tool_call_id(&self) -> AiToolCallId {
        self.tool_call_id
    }

    /// Native callback identity, already bound by the provider executor.
    pub fn provider_call_id(&self) -> &str {
        &self.provider_call_id
    }

    /// Renewed active run fence; preparation does not release the lease.
    pub fn lease(&self) -> &AiRunLease {
        &self.lease
    }
}

/// Protected preparation supplied only after current authorization and preview.
pub(crate) struct PreparedNativeApprovalCandidate {
    pub binding_hash: String,
    pub preview_hash: String,
    pub protected_preparation: serde_json::Value,
    pub prepared_event: PreparedToolLifecycleEvent,
}

// A bounded preview can grow when protected envelopes encode ciphertext. This
// custody ceiling is separate from the 2 MiB plaintext preview-details limit.
const MAXIMUM_PROTECTED_NATIVE_PREPARATION_BYTES: usize = 4 * 1024 * 1024;

pub(crate) struct PreparedNativeApprovalReceipt {
    pub protected_receipt: serde_json::Value,
    pub receipt_hash: String,
    pub egress_decision_id: uuid::Uuid,
    pub egress_manifest_hash: String,
}

pub(crate) struct PreparedNativeBlockedReceipt {
    pub candidate_id: AiToolCallId,
    pub receipt: PreparedNativeApprovalReceipt,
    pub disclosure_schema_fingerprint: String,
    pub event: PreparedToolLifecycleEvent,
}

impl PreparedNativeApprovalCandidate {
    pub(crate) fn valid(&self) -> bool {
        [&self.binding_hash, &self.preview_hash]
            .into_iter()
            .all(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
            && self.protected_preparation.is_object()
            && serde_json::to_vec(&self.protected_preparation)
                .is_ok_and(|bytes| bytes.len() <= MAXIMUM_PROTECTED_NATIVE_PREPARATION_BYTES)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredNativeControlReceipt {
    pub(crate) format_version: u8,
    pub(crate) provider_call_id: String,
    pub(crate) tool_id: String,
    pub(crate) egress_manifest: AiEgressManifest,
    pub(crate) egress_decision_id: uuid::Uuid,
}

impl crate::OrmAiConsequentialToolCallService {
    /// Persists and authorizes the closed no-effect reply for a prepared native call.
    ///
    /// This does not publish an approval or execute the action. It keeps the
    /// run active until the real provider turn and its usage have settled.
    /// Exact retries reopen the stored receipt and recheck current egress.
    ///
    /// # Errors
    ///
    /// Returns a safe error for stale ownership, changed callback/route bindings,
    /// unavailable protection, denied current access or egress, or failed durable
    /// receipt/audit persistence. A rejected receipt must not reach the provider.
    pub async fn prepare_native_approval_receipt(
        &self,
        lease: &AiRunLease,
        prepared: &AiPreparedNativeApproval,
        route: &crate::AiToolResultEgressRoute,
    ) -> Result<AiNativeToolControlReceipt, crate::AiError> {
        use crate::persistence::{AiSessionRecord, AiToolCallRecord};
        use crate::{AiDataSourceRef, AiError, AiScope, AiSourceTrust, DataClassification};
        if prepared.lease.run_id() != lease.run_id()
            || prepared.lease.session_id() != lease.session_id()
            || prepared.lease.attempt_id() != lease.attempt_id()
            || prepared.lease.lease_generation() != lease.lease_generation()
        {
            return Err(AiError::Conflict);
        }
        route.validate()?;
        let tools = self.application_tools();
        let run_service = tools.run_service();
        let candidate = run_service
            .load_prepared_native_approval(lease, prepared.tool_call_id)
            .await?;
        let call = AiToolCallRecord::find_by_id(run_service.database(), &candidate.id)
            .await
            .map_err(|_| AiError::PersistenceFailed)?
            .ok_or(AiError::Conflict)?;
        let session = AiSessionRecord::find_by_id(run_service.database(), &lease.session_id().0)
            .await
            .map_err(|_| AiError::PersistenceFailed)?
            .ok_or(AiError::Conflict)?;
        if call.provider_call_id != prepared.provider_call_id {
            return Err(AiError::Conflict);
        }
        let scope = AiScope {
            kind: session.scope_kind,
            id: session.scope_id,
            tenant_id: session.tenant_id,
        };
        let current = tools.current_access(lease, &scope).await?;
        let policy = tools
            .runtime()
            .content_protection_policy_resolver()
            .resolve(current.principal(), &scope)
            .await?;
        if !policy.ready || policy.scope != scope {
            return Err(AiError::RuntimeNotReady);
        }
        let kind = AiNativeToolControlKind::ApprovalPending;
        let output = kind.model_value();
        let callback_binding = serde_json::json!({
            "providerCallId": call.provider_call_id,
            "toolId": call.tool_id,
            "output": output,
        });
        let source = AiDataSourceRef {
            kind: "native_tool_control_receipt".to_owned(),
            reference: format!(
                "v1:{}:{}",
                candidate.id,
                native_receipt_hash(&callback_binding)?
            ),
            classification: DataClassification::Internal,
            trust: AiSourceTrust::TrustedRuntime,
        };
        let expected_manifest = route.subscription_wait_manifest(
            scope.clone(),
            lease.session_id(),
            lease.run_id(),
            call.provider_kind.as_ref().ok_or(AiError::Conflict)?,
            call.provider_model.as_ref().ok_or(AiError::Conflict)?,
            source,
            u64::try_from(
                serde_json::to_vec(&callback_binding)
                    .map_err(|_| AiError::PersistenceFailed)?
                    .len(),
            )
            .map_err(|_| AiError::PersistenceFailed)?,
        );
        let protection_context = || crate::ContentProtectionContext {
            entity: "graphql_orm_ai_native_approval_candidates".to_owned(),
            row_id: candidate.id.to_string(),
            field: "protected_control_receipt".to_owned(),
            scope: scope.clone(),
        };
        let existing = if let Some(protected) = &candidate.protected_control_receipt {
            let value = tools.open(&policy, protection_context(), protected).await?;
            if candidate.control_receipt_hash.as_deref()
                != Some(native_receipt_hash(&value)?.as_str())
            {
                return Err(AiError::Conflict);
            }
            let stored: StoredNativeControlReceipt =
                serde_json::from_value(value).map_err(|_| AiError::PersistenceFailed)?;
            if stored.format_version != 1
                || stored.provider_call_id != call.provider_call_id
                || stored.tool_id != call.tool_id
                || stored.egress_manifest != expected_manifest
                || Some(stored.egress_decision_id) != candidate.control_egress_decision_id
                || candidate.control_egress_manifest_hash.as_deref()
                    != Some(stored.egress_manifest.stable_hash().as_str())
            {
                return Err(AiError::Conflict);
            }
            Some(stored)
        } else {
            if candidate.control_receipt_hash.is_some()
                || candidate.control_egress_decision_id.is_some()
                || candidate.control_egress_manifest_hash.is_some()
            {
                return Err(AiError::Conflict);
            }
            None
        };
        let decision = tools
            .runtime()
            .authorize_egress(lease.principal_reference(), &expected_manifest)
            .await?;
        tools
            .egress_audit()
            .record(&expected_manifest, &decision)
            .await?;
        decision.authorize(&expected_manifest)?;
        let stored = existing.unwrap_or_else(|| StoredNativeControlReceipt {
            format_version: 1,
            provider_call_id: call.provider_call_id.clone(),
            tool_id: call.tool_id.clone(),
            egress_manifest: expected_manifest.clone(),
            egress_decision_id: decision.id.0,
        });
        let value = serde_json::to_value(&stored).map_err(|_| AiError::PersistenceFailed)?;
        let protected_receipt = if let Some(protected) = candidate.protected_control_receipt {
            protected
        } else {
            tools
                .protect(&policy, protection_context(), value.clone())
                .await?
        };
        let lease = run_service
            .persist_native_approval_receipt(
                lease,
                prepared.tool_call_id,
                PreparedNativeApprovalReceipt {
                    protected_receipt,
                    receipt_hash: native_receipt_hash(&value)?,
                    egress_decision_id: stored.egress_decision_id,
                    egress_manifest_hash: expected_manifest.stable_hash(),
                },
            )
            .await?;
        Ok(AiNativeToolControlReceipt {
            tool_call_id: prepared.tool_call_id,
            provider_call_id: call.provider_call_id.clone(),
            kind,
            model_input: ModelInputBlock::ToolResult {
                call_id: call.provider_call_id,
                tool_id: call.tool_id,
                output,
            },
            egress_manifest: expected_manifest,
            lease,
        })
    }
}

impl crate::OrmAiConsequentialToolCallService {
    /// Replies without executing a later mutation while this native turn awaits approval.
    ///
    /// The closed reply has its own durable call/step and egress evidence. It is
    /// not a domain result, a second approval proposal, or permission to retry.
    ///
    /// # Errors
    ///
    /// Returns a safe error for changed authority, a read-only call, a missing
    /// exact pending candidate, denied egress, or failed protection/persistence.
    pub async fn pause_native_consequential_call(
        &self,
        lease: &AiRunLease,
        provider_result: &crate::AiProviderCallResult,
        context: crate::AiApplicationToolCallContext,
        pending: &AiPreparedNativeApproval,
        route: &crate::AiToolResultEgressRoute,
    ) -> Result<AiNativeToolControlReceipt, crate::AiError> {
        use crate::{AiDataSourceRef, AiError, AiSourceTrust, DataClassification};
        if pending.lease.run_id() != lease.run_id()
            || pending.lease.attempt_id() != lease.attempt_id()
            || pending.lease.lease_generation() != lease.lease_generation()
        {
            return Err(AiError::Conflict);
        }
        route.validate()?;
        let tools = self.application_tools();
        let (call, policy) = tools
            .prepare_native_blocked_call(lease, provider_result, context)
            .await?;
        let scope = policy.scope.clone();
        let kind = AiNativeToolControlKind::ConsequentialCallsPaused;
        let output = kind.model_value();
        let model_input = ModelInputBlock::ToolResult {
            call_id: call.provider_call_id.clone(),
            tool_id: call.tool_id.clone(),
            output: output.clone(),
        };
        let binding = serde_json::to_value(&model_input).map_err(|_| AiError::PersistenceFailed)?;
        let manifest = route.subscription_wait_manifest(
            scope.clone(),
            lease.session_id(),
            lease.run_id(),
            &call.provider_kind,
            &call.provider_model,
            AiDataSourceRef {
                kind: "native_tool_control_receipt".to_owned(),
                reference: format!("v1:{}:{}", call.id, native_receipt_hash(&binding)?),
                classification: DataClassification::Internal,
                trust: AiSourceTrust::TrustedRuntime,
            },
            u64::try_from(
                serde_json::to_vec(&binding)
                    .map_err(|_| AiError::PersistenceFailed)?
                    .len(),
            )
            .map_err(|_| AiError::PersistenceFailed)?,
        );
        let decision = tools
            .runtime()
            .authorize_egress(lease.principal_reference(), &manifest)
            .await?;
        tools.egress_audit().record(&manifest, &decision).await?;
        decision.authorize(&manifest)?;
        let plaintext = serde_json::json!({
            "formatVersion": 1,
            "kind": "ConsequentialCallsPaused",
            "pendingToolCallId": pending.tool_call_id,
            "modelInput": model_input,
            "egressManifest": manifest,
            "egressDecisionId": decision.id,
        });
        let receipt_hash = native_receipt_hash(&plaintext)?;
        let protected_receipt = tools
            .protect(
                &policy,
                crate::ContentProtectionContext {
                    entity: "graphql_orm_ai_tool_calls".to_owned(),
                    row_id: call.id.to_string(),
                    field: "protected_result".to_owned(),
                    scope: scope.clone(),
                },
                plaintext,
            )
            .await?;
        let event_id = uuid::Uuid::new_v4();
        let inbox_event_id = uuid::Uuid::new_v4();
        let event = serde_json::json!({"toolCallId": call.id, "runId": lease.run_id(), "state": "control_blocked"});
        let protected_event = tools
            .protect(
                &policy,
                crate::ContentProtectionContext {
                    entity: "graphql_orm_ai_session_events".to_owned(),
                    row_id: event_id.to_string(),
                    field: "protected_payload".to_owned(),
                    scope: scope.clone(),
                },
                event.clone(),
            )
            .await?;
        let protected_inbox_event = tools
            .protect(
                &policy,
                crate::ContentProtectionContext {
                    entity: "graphql_orm_ai_inbox_events".to_owned(),
                    row_id: inbox_event_id.to_string(),
                    field: "protected_payload".to_owned(),
                    scope,
                },
                event,
            )
            .await?;
        let tool_call_id = AiToolCallId(call.id);
        let provider_call_id = call.provider_call_id.clone();
        let lease = tools
            .run_service()
            .persist_blocked_native_call(
                lease,
                call,
                PreparedNativeBlockedReceipt {
                    candidate_id: pending.tool_call_id,
                    receipt: PreparedNativeApprovalReceipt {
                        protected_receipt,
                        receipt_hash,
                        egress_decision_id: decision.id.0,
                        egress_manifest_hash: manifest.stable_hash(),
                    },
                    disclosure_schema_fingerprint: native_receipt_hash(&serde_json::json!({
                        "schema": "graphql-orm-ai/native-control/v1", "closedOutput": output,
                    }))?,
                    event: PreparedToolLifecycleEvent {
                        event_id,
                        inbox_event_id,
                        protected_event,
                        protected_inbox_event,
                    },
                },
            )
            .await?;
        Ok(AiNativeToolControlReceipt {
            tool_call_id,
            provider_call_id,
            kind,
            model_input,
            egress_manifest: manifest,
            lease,
        })
    }
}

impl crate::OrmAiConsequentialToolCallService {
    pub(crate) async fn native_approved_outcome_continuation(
        &self,
        lease: &AiRunLease,
        result: &crate::AiPersistedApplicationToolCall,
        continuation: crate::ModelContinuation,
        reasoning: crate::ModelReasoningEffort,
        route: &crate::AiToolResultEgressRoute,
    ) -> Result<(crate::AiAgentContinuation, uuid::Uuid), crate::AiError> {
        use crate::{AiDataSourceRef, AiError, AiSourceTrust, DataClassification};
        if result.lease().run_id() != lease.run_id()
            || result.lease().attempt_id() != lease.attempt_id()
            || result.lease().lease_generation() != lease.lease_generation()
        {
            return Err(AiError::Conflict);
        }
        let original = result.egress_manifest().ok_or(AiError::EgressDenied)?;
        if !route.matches_manifest(
            original,
            lease,
            &original.scope,
            &original.provider_kind,
            &original.model,
        ) {
            return Err(AiError::Conflict);
        }
        self.application_tools()
            .current_access(lease, &original.scope)
            .await?;
        let Some(ModelInputBlock::ToolResult {
            call_id,
            tool_id,
            output,
        }) = result.model_input()
        else {
            return Err(AiError::Conflict);
        };
        let value = serde_json::json!({
            "formatVersion":1, "kind":"FrameworkApprovedToolOutcome",
            "toolCallId":result.id().0,"providerCallId":call_id,"toolId":tool_id,
            "state":match result.state() { crate::AiApplicationToolCallState::Completed => "completed", crate::AiApplicationToolCallState::ExecutionFailed => "execution_failed", _ => return Err(AiError::EgressDenied) },"output":output,
        });
        let mut manifest = original.clone();
        manifest.sources.push(AiDataSourceRef {
            kind: "native_approved_outcome".to_owned(),
            reference: native_receipt_hash(&value)?,
            classification: DataClassification::Internal,
            trust: AiSourceTrust::TrustedRuntime,
        });
        manifest.estimated_bytes = u64::try_from(
            serde_json::to_vec(&ModelInputBlock::Json {
                value: value.clone(),
            })
            .map_err(|_| AiError::PersistenceFailed)?
            .len(),
        )
        .map_err(|_| AiError::PersistenceFailed)?;
        // Use a conservative byte-sized token estimate for the entire new
        // wrapper; the prior tool result estimate did not include this framing.
        manifest.estimated_tokens = manifest.estimated_bytes;
        let tools = self.application_tools();
        let decision = tools
            .runtime()
            .authorize_egress(lease.principal_reference(), &manifest)
            .await?;
        tools.egress_audit().record(&manifest, &decision).await?;
        decision.authorize(&manifest)?;
        Ok((
            crate::AiAgentContinuation::from_native_approved_outcome(
                continuation,
                reasoning,
                value,
                manifest,
            )?,
            decision.id.0,
        ))
    }
}

pub(crate) fn native_receipt_hash(value: &serde_json::Value) -> Result<String, crate::AiError> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(value).map_err(|_| crate::AiError::PersistenceFailed)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

#[cfg(test)]
mod preparation_limit_tests {
    use super::*;

    #[test]
    fn native_protected_preparation_has_separate_exact_envelope_ceiling() {
        let framing = serde_json::to_vec(&serde_json::json!({"ciphertext":""}))
            .unwrap()
            .len();
        let mut candidate = PreparedNativeApprovalCandidate {
            binding_hash: "a".repeat(64),
            preview_hash: "b".repeat(64),
            protected_preparation: serde_json::json!({"ciphertext":"x".repeat(MAXIMUM_PROTECTED_NATIVE_PREPARATION_BYTES-framing)}),
            prepared_event: PreparedToolLifecycleEvent {
                event_id: uuid::Uuid::new_v4(),
                inbox_event_id: uuid::Uuid::new_v4(),
                protected_event: serde_json::json!({}),
                protected_inbox_event: serde_json::json!({}),
            },
        };
        assert!(
            candidate.valid(),
            "bounded encrypted encoding may exceed plaintext preview capacity"
        );
        candidate.protected_preparation["ciphertext"] =
            serde_json::json!("x".repeat(MAXIMUM_PROTECTED_NATIVE_PREPARATION_BYTES - framing + 1));
        assert!(
            !candidate.valid(),
            "one excess protected byte must fail closed"
        );
    }
}
