//! ORM-backed current-owner application-tool result preview service.

#![cfg(any(feature = "sqlite", feature = "postgres"))]

use std::sync::Arc;

use agql_auth::AuthPrincipal;
use async_trait::async_trait;
use graphql_orm::db::Database;
use graphql_orm::graphql::errors::OrmPublicError;
use graphql_orm::graphql::orm::DefaultWriteBackend;

use crate::orm_sessions::{map_orm, map_protection, principal_identity, record_scope};
use crate::persistence::{AiRunRecord, AiSessionRecord, AiToolCallRecord};
use crate::{
    AiError, AiRunId, AiSessionAction, AiSessionId, AiToolCallId, AiToolCallResultPreviewInput,
    AiToolCallResultPreviewService, AiToolCallResultPreviewView, AiToolId,
    AiToolResultPreviewAuthorizer, ContentProtectionContext, DataClassification,
    GraphqlInvocationContext, ProtectedContentEnvelope, ToolGraphqlRequest,
};

const MAXIMUM_BROWSER_ARGUMENT_BYTES: usize = 64 * 1024;

/// Generated-ORM result-preview service for application hosts.
pub struct OrmAiToolCallResultPreviewService {
    database: Database<DefaultWriteBackend>,
    runtime: Arc<crate::AiRuntime>,
    authorizer: Arc<dyn AiToolResultPreviewAuthorizer>,
}

impl OrmAiToolCallResultPreviewService {
    /// Creates a default-deny preview service around one closed runtime and
    /// mandatory host row/field authorization seam.
    pub fn new(
        database: Database<DefaultWriteBackend>,
        runtime: Arc<crate::AiRuntime>,
        authorizer: Arc<dyn AiToolResultPreviewAuthorizer>,
    ) -> Self {
        Self {
            database,
            runtime,
            authorizer,
        }
    }

    async fn open(
        &self,
        policy: &crate::AiContentProtectionPolicy,
        context: ContentProtectionContext,
        value: &serde_json::Value,
    ) -> Result<serde_json::Value, AiError> {
        let envelope: ProtectedContentEnvelope =
            serde_json::from_value(value.clone()).map_err(|_| AiError::PersistenceFailed)?;
        self.runtime
            .content_protector()
            .open(policy, &context, &envelope)
            .await
            .map_err(map_protection)
    }

    async fn project_arguments(
        &self,
        principal: &agql_auth::ResolvedPrincipal,
        scope: &crate::AiScope,
        descriptor: &crate::AiToolDescriptor,
        arguments: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, AiError> {
        let Some(projected) = self
            .authorizer
            .authorize_and_project_arguments(principal, scope, descriptor, arguments)
            .await?
        else {
            return Ok(None);
        };
        if serde_json::to_vec(&projected)
            .map_err(|_| AiError::PersistenceFailed)?
            .len()
            > MAXIMUM_BROWSER_ARGUMENT_BYTES
        {
            return Err(AiError::Forbidden);
        }
        Ok(Some(projected))
    }
}

#[async_trait]
impl AiToolCallResultPreviewService for OrmAiToolCallResultPreviewService {
    async fn result_preview(
        &self,
        principal: &AuthPrincipal,
        input: AiToolCallResultPreviewInput,
    ) -> Result<Option<AiToolCallResultPreviewView>, AiError> {
        if input.session_id.is_nil() || input.tool_call_id.is_nil() {
            return Err(AiError::InvalidInput(
                "invalid tool result preview identity".to_owned(),
            ));
        }
        let requested_reference = principal.reference();
        let current = self
            .runtime
            .resolve_current_principal(&requested_reference)
            .await?;
        if current.reference() != &requested_reference {
            return Err(AiError::ReauthorizationFailed);
        }
        let session = AiSessionRecord::find_by_id(&self.database, &input.session_id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::NotFound)?;
        let (principal_kind, subject) = principal_identity(current.principal());
        if session.deleted_at.is_some()
            || session.owner_principal_kind != principal_kind
            || session.owner_subject != subject
        {
            return Err(AiError::NotFound);
        }
        let scope = record_scope(&session);
        if !self
            .runtime
            .access_policy()
            .can_access_session(
                current.principal(),
                AiSessionId(session.id),
                AiSessionAction::Read,
            )
            .await
            .is_allowed()
            || !self
                .runtime
                .access_policy()
                .can_access_scope(current.principal(), &scope, AiSessionAction::Read)
                .await
                .is_allowed()
        {
            return Err(AiError::Forbidden);
        }
        let call = AiToolCallRecord::find_by_id(&self.database, &input.tool_call_id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::NotFound)?;
        let run = AiRunRecord::find_by_id(&self.database, &call.run_id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .filter(|run| run.session_id == session.id)
            .ok_or(AiError::NotFound)?;
        let successful =
            call.state == "completed" && call.authorization_code.as_deref() == Some("allowed");
        let safe_failure = call
            .authorization_code
            .as_deref()
            .and_then(application_tool_failure_code)
            .filter(|_| call.state == "execution_failed");
        if call.run_id != run.id
            || (!successful && safe_failure.is_none())
            || call.completed_at.is_none()
            || call.payload_purged_at.is_some()
            || call.protected_result.is_none()
            || call.protected_arguments.is_none()
        {
            return Ok(None);
        }
        let tool_id = AiToolId::parse(call.tool_id.clone())?;
        let descriptor = self
            .runtime
            .tool_catalog()
            .descriptor(&tool_id)
            .filter(|descriptor| descriptor.fingerprint == call.tool_fingerprint)
            .ok_or(AiError::Forbidden)?;
        let preview_policy = match descriptor.browser_result_preview {
            Some(policy) => policy,
            None => return Ok(None),
        };
        let policy = self
            .runtime
            .content_protection_policy_resolver()
            .resolve(current.principal(), &scope)
            .await?;
        if !policy.ready || policy.scope != scope {
            return Err(AiError::RuntimeNotReady);
        }
        let arguments = self
            .open(
                &policy,
                protection_context(call.id, "protected_arguments", &scope),
                call.protected_arguments
                    .as_ref()
                    .ok_or(AiError::PersistenceFailed)?,
            )
            .await?;
        let contract = descriptor
            .graphql_contract
            .clone()
            .ok_or(AiError::Forbidden)?;
        let request = ToolGraphqlRequest {
            document: descriptor.document.clone(),
            operation_name: contract.operation_name.clone(),
            contract,
            variables: arguments,
            invocation: GraphqlInvocationContext {
                run_id: AiRunId(run.id),
                tool_call_id: AiToolCallId(call.id),
                scope: scope.clone(),
                correlation_id: call
                    .correlation_id
                    .clone()
                    .ok_or(AiError::PersistenceFailed)?,
                causation_id: call
                    .causation_id
                    .clone()
                    .ok_or(AiError::PersistenceFailed)?,
                delegation_reference: call.delegation_reference.clone(),
                idempotency_key: call.idempotency_key.clone(),
            },
        };
        let preauthorization = self
            .runtime
            .preauthorize_tool(&requested_reference, &tool_id, &request)
            .await?;
        if preauthorization.principal().reference() != &requested_reference {
            return Err(AiError::ReauthorizationFailed);
        }
        let arguments = self
            .project_arguments(
                preauthorization.principal(),
                &scope,
                descriptor,
                &request.variables,
            )
            .await?;
        if let Some(code) = safe_failure {
            let classification = parse_classification(
                call.result_classification
                    .as_deref()
                    .ok_or(AiError::PersistenceFailed)?,
            )?;
            if classification != DataClassification::Public {
                return Err(AiError::PersistenceFailed);
            }
            let stored = self
                .open(
                    &policy,
                    protection_context(call.id, "protected_result", &scope),
                    call.protected_result
                        .as_ref()
                        .ok_or(AiError::PersistenceFailed)?,
                )
                .await?;
            let preview = extract_safe_failure(&stored, code)?;
            return Ok(Some(AiToolCallResultPreviewView {
                session_id: session.id,
                run_id: run.id,
                tool_call_id: call.id,
                tool_id: call.tool_id,
                classification: classification_name(classification).to_owned(),
                arguments: arguments.map(async_graphql::Json),
                preview: async_graphql::Json(preview),
            }));
        }
        let disclosure = self
            .runtime
            .tool_catalog()
            .disclosure_schema(&tool_id)
            .filter(|schema| {
                call.disclosure_schema_fingerprint.as_deref() == Some(schema.fingerprint.as_str())
            })
            .ok_or(AiError::Forbidden)?;
        let classification = parse_classification(
            call.result_classification
                .as_deref()
                .ok_or(AiError::PersistenceFailed)?,
        )?;
        if classification > preview_policy.maximum_classification {
            return Ok(None);
        }
        let stored = self
            .open(
                &policy,
                protection_context(call.id, "protected_result", &scope),
                call.protected_result
                    .as_ref()
                    .ok_or(AiError::PersistenceFailed)?,
            )
            .await?;
        let result = extract_exact_result(&stored)?;
        let Some(preview) = self
            .authorizer
            .authorize_and_project(
                preauthorization.principal(),
                &scope,
                descriptor,
                &request,
                result,
            )
            .await?
        else {
            return Ok(None);
        };
        let evaluation = disclosure
            .evaluate(&preview)
            .map_err(|_| AiError::Forbidden)?;
        if evaluation.maximum_classification > preview_policy.maximum_classification
            || serde_json::to_vec(&preview)
                .map_err(|_| AiError::PersistenceFailed)?
                .len()
                > usize::try_from(preview_policy.maximum_bytes).map_err(|_| {
                    AiError::InvalidConfiguration("invalid preview bound".to_owned())
                })?
        {
            return Err(AiError::Forbidden);
        }
        let (depth, records) = json_shape(&preview, 0)?;
        if depth > usize::from(preview_policy.maximum_depth)
            || records > u64::from(preview_policy.maximum_records)
        {
            return Err(AiError::Forbidden);
        }
        Ok(Some(AiToolCallResultPreviewView {
            session_id: session.id,
            run_id: run.id,
            tool_call_id: call.id,
            tool_id: descriptor.id.as_str().to_owned(),
            classification: classification_name(classification).to_owned(),
            arguments: arguments.map(async_graphql::Json),
            preview: async_graphql::Json(preview),
        }))
    }
}

fn protection_context(
    row_id: uuid::Uuid,
    field: &str,
    scope: &crate::AiScope,
) -> ContentProtectionContext {
    ContentProtectionContext {
        entity: "graphql_orm_ai_tool_calls".to_owned(),
        row_id: row_id.to_string(),
        field: field.to_owned(),
        scope: scope.clone(),
    }
}

fn extract_exact_result(value: &serde_json::Value) -> Result<&serde_json::Value, AiError> {
    let object = value.as_object().ok_or(AiError::PersistenceFailed)?;
    if object.len() != 2
        || !object.contains_key("data")
        || !object
            .get("errorCodes")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|codes| {
                codes.len() <= 32
                    && codes.iter().all(|code| {
                        code.as_str().is_some_and(|code| {
                            !code.is_empty()
                                && code.len() <= 100
                                && code.bytes().all(|byte| {
                                    byte.is_ascii_uppercase()
                                        || byte.is_ascii_digit()
                                        || byte == b'_'
                                })
                        })
                    })
            })
    {
        return Err(AiError::PersistenceFailed);
    }
    object.get("data").ok_or(AiError::PersistenceFailed)
}

fn application_tool_failure_code(value: &str) -> Option<crate::AiApplicationToolFailureCode> {
    use crate::AiApplicationToolFailureCode as Code;

    let code = match value {
        "invalid_arguments" => Code::InvalidArguments,
        "selection_too_large" => Code::SelectionTooLarge,
        "relationship_depth_exceeded" => Code::RelationshipDepthExceeded,
        "result_budget_exceeded" => Code::ResultBudgetExceeded,
        "capability_stale" => Code::CapabilityStale,
        "authorization_denied" => Code::AuthorizationDenied,
        "temporarily_unavailable" => Code::TemporarilyUnavailable,
        "tool_unavailable" => Code::ToolUnavailable,
        "resolver_validation_failed" => Code::ResolverValidationFailed,
        "not_found" => Code::NotFound,
        _ => return None,
    };
    Some(exhaustive_browser_failure_code(code))
}

const fn exhaustive_browser_failure_code(
    code: crate::AiApplicationToolFailureCode,
) -> crate::AiApplicationToolFailureCode {
    use crate::AiApplicationToolFailureCode as Code;

    match code {
        Code::InvalidArguments
        | Code::SelectionTooLarge
        | Code::RelationshipDepthExceeded
        | Code::ResultBudgetExceeded
        | Code::CapabilityStale
        | Code::AuthorizationDenied
        | Code::TemporarilyUnavailable
        | Code::ToolUnavailable
        | Code::ResolverValidationFailed
        | Code::NotFound => code,
    }
}

fn extract_safe_failure(
    value: &serde_json::Value,
    code: crate::AiApplicationToolFailureCode,
) -> Result<serde_json::Value, AiError> {
    let object = value.as_object().ok_or(AiError::PersistenceFailed)?;
    if object.len() != 4
        || object.get("version").and_then(serde_json::Value::as_u64)
            != Some(u64::from(crate::AI_APPLICATION_TOOL_FAILURE_VERSION))
        || object.get("ok").and_then(serde_json::Value::as_bool) != Some(false)
        || object.get("code").and_then(serde_json::Value::as_str) != Some(code.as_str())
        || object.get("retryable").and_then(serde_json::Value::as_bool) != Some(code.retryable())
    {
        return Err(AiError::PersistenceFailed);
    }
    Ok(value.clone())
}

fn json_shape(value: &serde_json::Value, depth: usize) -> Result<(usize, u64), AiError> {
    if depth > 64 {
        return Err(AiError::Forbidden);
    }
    match value {
        serde_json::Value::Array(values) => {
            let mut maximum_depth = depth;
            let mut records = u64::try_from(values.len()).map_err(|_| AiError::Forbidden)?;
            for value in values {
                let (child_depth, child_records) = json_shape(value, depth + 1)?;
                maximum_depth = maximum_depth.max(child_depth);
                records = records
                    .checked_add(child_records)
                    .ok_or(AiError::Forbidden)?;
            }
            Ok((maximum_depth, records))
        }
        serde_json::Value::Object(values) => {
            let mut maximum_depth = depth;
            let mut records = 0u64;
            for value in values.values() {
                let (child_depth, child_records) = json_shape(value, depth + 1)?;
                maximum_depth = maximum_depth.max(child_depth);
                records = records
                    .checked_add(child_records)
                    .ok_or(AiError::Forbidden)?;
            }
            Ok((maximum_depth, records))
        }
        _ => Ok((depth, 0)),
    }
}

fn parse_classification(value: &str) -> Result<DataClassification, AiError> {
    match value {
        "public" => Ok(DataClassification::Public),
        "internal" => Ok(DataClassification::Internal),
        "confidential" => Ok(DataClassification::Confidential),
        "restricted" => Ok(DataClassification::Restricted),
        "secret" => Ok(DataClassification::Secret),
        _ => Err(AiError::PersistenceFailed),
    }
}

const fn classification_name(value: DataClassification) -> &'static str {
    match value {
        DataClassification::Public => "public",
        DataClassification::Internal => "internal",
        DataClassification::Confidential => "confidential",
        DataClassification::Restricted => "restricted",
        DataClassification::Secret => "secret",
    }
}

#[cfg(test)]
mod tests {
    use super::{application_tool_failure_code, extract_safe_failure};
    use crate::{AiApplicationToolFailureCode, AiApplicationToolFailureEnvelope, AiError};

    #[test]
    fn safe_failure_preview_accepts_only_the_exact_content_free_envelope() {
        let code = AiApplicationToolFailureCode::AuthorizationDenied;
        let envelope = AiApplicationToolFailureEnvelope::new(code).to_json();
        assert_eq!(application_tool_failure_code(code.as_str()), Some(code));
        assert_eq!(
            extract_safe_failure(&envelope, code).expect("exact safe envelope should validate"),
            envelope
        );

        let mut mismatched =
            AiApplicationToolFailureEnvelope::new(AiApplicationToolFailureCode::ToolUnavailable)
                .to_json();
        assert!(matches!(
            extract_safe_failure(&mismatched, code),
            Err(AiError::PersistenceFailed)
        ));
        mismatched["code"] = serde_json::Value::String(code.as_str().to_owned());
        mismatched["detail"] = serde_json::Value::String("must not cross".to_owned());
        assert!(matches!(
            extract_safe_failure(&mismatched, code),
            Err(AiError::PersistenceFailed)
        ));
    }
}
