//! Recoverable current-policy refusals at the native classification boundary.
use super::*;

pub(crate) enum NativeToolClassification {
    Admitted(AiApplicationToolDisposition),
    Rejected(Box<AiPersistedApplicationToolCall>),
}

/// Minted only by the undispatched native classification path, never a grant.
pub(crate) struct NativePreflightRefusal(serde_json::Value);

impl NativePreflightRefusal {
    pub(crate) fn to_json(&self) -> serde_json::Value {
        self.0.clone()
    }
}

impl OrmAiApplicationToolCallService {
    pub(crate) async fn classify_native_tool_call(
        &self,
        lease: &AiRunLease,
        result: &AiProviderCallResult,
        context: &AiApplicationToolCallContext,
        route: AiToolResultEgressRoute,
    ) -> Result<NativeToolClassification, AiError> {
        // Classification performs no resolver dispatch. Keep error conversion
        // inside this owning boundary; execution/persistence errors elsewhere
        // cannot enter this path through a caller-supplied failure code.
        let error = match self.classify_tool_call(lease, result, context).await {
            Ok(disposition) => return Ok(NativeToolClassification::Admitted(disposition)),
            Err(error) => error,
        };
        let code = match error {
            AiError::ReauthorizationFailed => {
                crate::AiApplicationToolFailureCode::AuthenticationRequired
            }
            AiError::Forbidden => crate::AiApplicationToolFailureCode::PreflightAuthorizationDenied,
            _ => return Err(error),
        };
        let call = result
            .tool_calls()
            .get(context.tool_call_index)
            .ok_or(AiError::Conflict)?;
        let descriptor = self
            .runtime
            .tool_catalog()
            .descriptor(call.tool_id())
            .ok_or(AiError::Forbidden)?;
        if descriptor.fingerprint != call.tool_fingerprint()
            || descriptor.operation_domain != AiToolOperationDomain::Application
        {
            return Err(AiError::Forbidden);
        }
        let risk = risk_value(descriptor.risk);
        // A fresh fenced insert must succeed before publishing a failure.
        // Duplicate callback keys, stale fences, missing authority/protection,
        // and failed egress remain errors, never invented no-effect receipts.
        let persisted = self
            .persist_unexecuted_failure(lease, result, context.clone(), route, code, risk, true)
            .await?;
        Ok(NativeToolClassification::Rejected(Box::new(persisted)))
    }
}

pub(super) fn provenance(
    code: crate::AiApplicationToolFailureCode,
    fingerprint: &str,
    argument_hash: &str,
) -> serde_json::Value {
    json!({"formatVersion": 1, "kind": "native_preflight_refusal", "failureCode": code.as_str(), "toolFingerprint": fingerprint, "argumentHash": argument_hash})
}

pub(super) fn receipt(
    code: crate::AiApplicationToolFailureCode,
    fingerprint: &str,
    argument_hash: &str,
) -> NativePreflightRefusal {
    NativePreflightRefusal(provenance(code, fingerprint, argument_hash))
}

/// Recognizes only the crate-owned, undispatched callback receipt. Ordinary
/// executor errors, read-failure fallbacks and caller-authored JSON lack it.
pub(crate) fn native_preflight_failure_code(
    call: &AiToolCallRecord,
) -> Option<crate::AiApplicationToolFailureCode> {
    use crate::AiApplicationToolFailureCode as Code;
    let code = match call.authorization_code.as_deref()? {
        "preflight_authentication_required" => Code::AuthenticationRequired,
        "preflight_authorization_denied" => Code::PreflightAuthorizationDenied,
        _ => return None,
    };
    (call.state == "execution_failed"
        && call.approval_id.is_none()
        && call.authorization_policy_version.is_none()
        && call.authorization_state_digest.is_none()
        && call.application_audit_ref.is_none()
        && call.result_classification.as_deref() == Some("public")
        && call.disclosure_schema_fingerprint.as_deref()
            == Some(safe_failure_disclosure_fingerprint().as_str())
        && call.idempotency_key.as_deref()
            == Some(format!("ai-tool:{}", call.provider_call_key).as_str())
        && call.execution_provenance.as_ref()
            == Some(&provenance(
                code,
                &call.tool_fingerprint,
                &call.argument_hash,
            )))
    .then_some(code)
}
