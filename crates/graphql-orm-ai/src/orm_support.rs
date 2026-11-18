//! Shared conversions for the durable AI services.

#![cfg(any(feature = "sqlite", feature = "postgres"))]

use agql_auth::AuthPrincipal;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::graphql::orm::TransactionError;
use graphql_orm::graphql::pagination::{
    KeysetConnectionInput, KeysetWindowDirection, ValidatedKeysetConnection,
};
use uuid::Uuid;

use crate::persistence::{AiPricingPolicyRecord, AiSessionRecord};
use crate::{AiError, AiScope, ContentProtectionContext};

pub(crate) fn map_orm(error: OrmPublicError) -> AiError {
    if crate::orm_runtime_recovery::is_epoch_not_live(&error) {
        return AiError::RuntimeNotReady;
    }
    match error.code {
        OrmErrorCode::InvalidInput
        | OrmErrorCode::CursorInvalid
        | OrmErrorCode::PageLimitExceeded => AiError::InvalidInput(error.message),
        OrmErrorCode::Unauthenticated | OrmErrorCode::Forbidden => AiError::Forbidden,
        OrmErrorCode::NotFound => AiError::NotFound,
        OrmErrorCode::Conflict | OrmErrorCode::ConstraintViolation => AiError::Conflict,
        OrmErrorCode::ServiceUnavailable
        | OrmErrorCode::InternalError
        | OrmErrorCode::AuthorizationMisconfigured => persistence_failure(error.code),
    }
}

fn persistence_failure(code: OrmErrorCode) -> AiError {
    tracing::debug!(orm_error_code = code.as_str(), "AI ORM operation failed");
    AiError::PersistenceFailed
}

pub(crate) fn map_transaction(error: TransactionError) -> AiError {
    map_orm(error.public_error().clone())
}

pub(crate) fn principal_identity(principal: &AuthPrincipal) -> (String, &str) {
    (principal_kind(principal), principal.subject())
}

pub(crate) fn principal_kind(principal: &AuthPrincipal) -> String {
    match principal {
        AuthPrincipal::User(_) => "user".to_owned(),
        AuthPrincipal::ApiToken(token) => {
            format!("api_token:{}", token.principal_kind.as_str())
        }
    }
}

pub(crate) fn record_scope(session: &AiSessionRecord) -> AiScope {
    stored_scope(
        &session.scope_kind,
        &session.scope_id,
        session.tenant_id.as_deref(),
    )
}

pub(crate) fn content_context(
    entity: &str,
    row_id: Uuid,
    field: &str,
    scope: &AiScope,
) -> ContentProtectionContext {
    ContentProtectionContext {
        entity: entity.to_owned(),
        row_id: row_id.to_string(),
        field: field.to_owned(),
        scope: scope.clone(),
    }
}

pub(crate) fn page_input(
    page: &ValidatedKeysetConnection,
    include_total_count: bool,
) -> KeysetConnectionInput {
    match page.direction {
        KeysetWindowDirection::Forward => KeysetConnectionInput {
            after: page.cursor.clone(),
            first: Some(page.limit),
            include_total_count,
            ..Default::default()
        },
        KeysetWindowDirection::Backward => KeysetConnectionInput {
            before: page.cursor.clone(),
            last: Some(page.limit),
            include_total_count,
            ..Default::default()
        },
    }
}

// Compaction has historically mapped constraint and closed-epoch refusals
// to a persistence failure. Preserve that contract independently of the
// write services that return Conflict and RuntimeNotReady.
pub(crate) fn map_context_compaction(error: OrmPublicError) -> AiError {
    match error.code {
        OrmErrorCode::InvalidInput
        | OrmErrorCode::CursorInvalid
        | OrmErrorCode::PageLimitExceeded => AiError::InvalidInput(error.message),
        OrmErrorCode::Unauthenticated | OrmErrorCode::Forbidden => AiError::Forbidden,
        OrmErrorCode::NotFound => AiError::NotFound,
        OrmErrorCode::Conflict => AiError::Conflict,
        _ => persistence_failure(error.code),
    }
}

// Usage reads expose only the authorization, existence, and conflict codes
// listed below.
pub(crate) fn map_usage(error: impl Into<OrmPublicError>) -> AiError {
    let error = error.into();
    match error.code {
        OrmErrorCode::Forbidden => AiError::Forbidden,
        OrmErrorCode::NotFound => AiError::NotFound,
        OrmErrorCode::Conflict => AiError::Conflict,
        _ => persistence_failure(error.code),
    }
}

pub(crate) fn pricing_scope(record: &AiPricingPolicyRecord) -> AiScope {
    stored_scope(
        &record.scope_kind,
        &record.scope_id,
        record.tenant_id.as_deref(),
    )
}

fn stored_scope(kind: &str, id: &str, tenant_id: Option<&str>) -> AiScope {
    AiScope {
        kind: kind.to_owned(),
        id: id.to_owned(),
        tenant_id: tenant_id.map(str::to_owned),
    }
}
