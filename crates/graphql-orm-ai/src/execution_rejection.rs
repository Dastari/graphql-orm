//! Exact binding for trusted no-effect application refusals.

use crate::{
    AiToolExecutionProvenance, ToolExecutionError, ToolGraphqlRequest, ToolPreExecutionRejection,
    ToolPreExecutionRejectionReason,
};

pub(crate) fn rejection_binding_hash(
    fingerprint: &str,
    provenance: Option<&AiToolExecutionProvenance>,
    request: &ToolGraphqlRequest,
) -> String {
    crate::tools::canonical_json_digest(&serde_json::json!({
        "contract": "graphql-orm-ai/pre-execution-rejection/v1",
        "toolFingerprint": fingerprint,
        "provenance": provenance,
        "request": request,
    }))
}

pub(crate) fn attest(
    fingerprint: &str,
    provenance: Option<&AiToolExecutionProvenance>,
    request: &ToolGraphqlRequest,
    reason: ToolPreExecutionRejectionReason,
) -> Result<ToolPreExecutionRejection, ToolExecutionError> {
    ToolPreExecutionRejection::attest_trusted(
        rejection_binding_hash(fingerprint, provenance, request),
        reason,
    )
}

pub(crate) fn before_executor_error(
    binding: &crate::AiRegisteredToolExecutionBinding,
    request: &ToolGraphqlRequest,
    error: ToolExecutionError,
) -> ToolExecutionError {
    use ToolPreExecutionRejectionReason as Reason;
    // This helper is called only before the executor is invoked. A context
    // factory constructs authority; it must never execute an application tool.
    if let ToolExecutionError::RejectedBeforeExecution(proof) = error {
        return validate_rejection(binding, request, proof);
    }
    // Only the durable lifecycle establishes that this exact invocation has
    // not previously crossed execution. Direct bridge callers may reuse an
    // invocation/idempotency key, so absence of dispatch in this call alone
    // cannot attest absence of earlier work.
    if binding.provenance().is_none() {
        return error;
    }
    let reason = match error {
        ToolExecutionError::Reauthorization => Reason::AuthenticationRequired,
        ToolExecutionError::Authorization => Reason::AuthorizationDenied,
        ToolExecutionError::InvalidTarget | ToolExecutionError::StaleContract => {
            Reason::CapabilityStale
        }
        _ => Reason::TemporarilyUnavailable,
    };
    match binding.reject_before_execution(request, reason) {
        Ok(proof) => ToolExecutionError::RejectedBeforeExecution(proof),
        Err(_) => ToolExecutionError::Execution,
    }
}

pub(crate) fn validate_rejection(
    binding: &crate::AiRegisteredToolExecutionBinding,
    request: &ToolGraphqlRequest,
    proof: ToolPreExecutionRejection,
) -> ToolExecutionError {
    if binding.matches_request(request)
        && proof.binding_hash()
            == rejection_binding_hash(binding.tool_fingerprint(), binding.provenance(), request)
    {
        ToolExecutionError::RejectedBeforeExecution(proof)
    } else {
        // Do not downgrade a foreign or malformed attestation to a safe refusal.
        ToolExecutionError::Execution
    }
}
