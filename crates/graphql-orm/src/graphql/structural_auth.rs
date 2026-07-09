//! Backend-independent structural tenant/owner authorization predicates.
//!
//! PostgreSQL RLS remains useful defense in depth. These helpers generate
//! parameterized SQL predicates that apply on every supported backend before
//! pagination so tenant safety does not disappear on SQLite, MSSQL, or
//! repository paths.

use crate::graphql::orm::{FilterExpression, SqlValue};

/// Entity-level structural authorization requirement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StructuralAuthorization {
    /// No structural tenant/owner requirement.
    #[default]
    None,
    /// Require a principal tenant (and optional owner) before query execution.
    Required,
    /// Apply tenant/owner predicates when context values are present.
    Optional,
}

/// Metadata describing structural authorization columns for an entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct StructuralAuthMetadata {
    /// Column storing the tenant identifier.
    pub tenant_column: Option<&'static str>,
    /// Column storing the owner/user identifier.
    pub owner_column: Option<&'static str>,
    /// Whether structural authorization is required.
    pub authorization: StructuralAuthorization,
}

impl StructuralAuthMetadata {
    /// Create metadata with explicit columns and requirement.
    pub const fn new(
        tenant_column: Option<&'static str>,
        owner_column: Option<&'static str>,
        authorization: StructuralAuthorization,
    ) -> Self {
        Self {
            tenant_column,
            owner_column,
            authorization,
        }
    }

    /// Return true when this entity declares any structural auth column.
    pub const fn is_configured(self) -> bool {
        self.tenant_column.is_some() || self.owner_column.is_some()
    }
}

/// Values used to build structural predicates for a request.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct StructuralAuthValues {
    /// Tenant identifier from the request principal.
    pub tenant_id: Option<String>,
    /// Owner/user identifier from the request principal.
    pub owner_id: Option<String>,
}

impl StructuralAuthValues {
    /// Build values from optional tenant and owner identifiers.
    pub fn new(tenant_id: Option<String>, owner_id: Option<String>) -> Self {
        Self {
            tenant_id,
            owner_id,
        }
    }

    /// Build values from a database auth context.
    pub fn from_db_auth(context: &crate::graphql::orm::DbAuthContext) -> Self {
        Self {
            tenant_id: context.tenant_id.clone(),
            owner_id: context.user_id.clone().or_else(|| context.subject.clone()),
        }
    }

    /// Build values from an auth subject.
    pub fn from_subject(subject: &crate::graphql::auth::AuthSubject) -> Self {
        Self {
            tenant_id: subject.tenant_id.clone(),
            owner_id: subject.user_id.clone().or_else(|| Some(subject.id.clone())),
        }
    }
}

/// Outcome of resolving structural authorization for a query.
#[derive(Clone, Debug, PartialEq)]
pub enum StructuralAuthResolution {
    /// No structural predicate is required.
    None,
    /// A parameterized predicate to AND with the caller's filter.
    Filter(FilterExpression),
    /// Strict mode denied because required context was missing.
    DeniedMissingContext,
}

/// Build a parameterized equality predicate for one column.
pub fn equality_predicate(column: &str, value: impl Into<String>) -> FilterExpression {
    FilterExpression::trusted_fragment(
        format!("{column} = ?"),
        vec![SqlValue::String(value.into())],
    )
}

/// Resolve structural authorization for the given metadata and request values.
pub fn resolve_structural_auth(
    metadata: StructuralAuthMetadata,
    values: &StructuralAuthValues,
) -> StructuralAuthResolution {
    if !metadata.is_configured() {
        return StructuralAuthResolution::None;
    }

    let mut predicates = Vec::new();

    if let Some(column) = metadata.tenant_column {
        match values.tenant_id.as_deref() {
            Some(tenant_id) if !tenant_id.is_empty() => {
                predicates.push(equality_predicate(column, tenant_id));
            }
            _ if metadata.authorization == StructuralAuthorization::Required => {
                return StructuralAuthResolution::DeniedMissingContext;
            }
            _ => {}
        }
    }

    if let Some(column) = metadata.owner_column {
        match values.owner_id.as_deref() {
            Some(owner_id) if !owner_id.is_empty() => {
                predicates.push(equality_predicate(column, owner_id));
            }
            _ if metadata.authorization == StructuralAuthorization::Required
                && metadata.tenant_column.is_none() =>
            {
                return StructuralAuthResolution::DeniedMissingContext;
            }
            _ => {}
        }
    }

    match predicates.len() {
        0 => StructuralAuthResolution::None,
        1 => StructuralAuthResolution::Filter(predicates.remove(0)),
        _ => StructuralAuthResolution::Filter(FilterExpression::And(predicates)),
    }
}

/// Merge an existing filter with a structural authorization predicate.
pub fn merge_filters(
    existing: Option<FilterExpression>,
    structural: Option<FilterExpression>,
) -> Option<FilterExpression> {
    match (existing, structural) {
        (None, None) => None,
        (Some(existing), None) => Some(existing),
        (None, Some(structural)) => Some(structural),
        (Some(existing), Some(structural)) => {
            Some(FilterExpression::And(vec![existing, structural]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_tenant_denies_without_context() {
        let metadata =
            StructuralAuthMetadata::new(Some("tenant_id"), None, StructuralAuthorization::Required);
        let resolution = resolve_structural_auth(metadata, &StructuralAuthValues::default());
        assert!(matches!(
            resolution,
            StructuralAuthResolution::DeniedMissingContext
        ));
    }

    #[test]
    fn required_tenant_builds_parameterized_filter() {
        let metadata =
            StructuralAuthMetadata::new(Some("tenant_id"), None, StructuralAuthorization::Required);
        let values = StructuralAuthValues::new(Some("t1".into()), None);
        match resolve_structural_auth(metadata, &values) {
            StructuralAuthResolution::Filter(FilterExpression::TrustedFragment {
                clause,
                values,
            }) => {
                assert_eq!(clause, "tenant_id = ?");
                assert_eq!(values, vec![SqlValue::String("t1".into())]);
            }
            other => panic!("unexpected resolution: {other:?}"),
        }
    }
}
