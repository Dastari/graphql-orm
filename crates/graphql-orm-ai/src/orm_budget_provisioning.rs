//! Trusted host provisioning of deployment-owned budget policies.
//!
//! `OrmAiBudgetService::reserve` denies without an applicable policy, and the
//! only interactive writer requires a recent-MFA principal. A host provisions
//! its deployment default (for example from an operator configuration file)
//! through this separate, Rust-only type. It is never exposed through GraphQL
//! and never reachable from [`crate::OrmAiConfigurationService`].

use crate::orm_configuration::{
    AuditFact, budget_policy_view, insert_audit, map_transaction,
    normalize_and_validate_budget_policy_input, scope_key, validate_scope,
};
use crate::persistence::CreateAiBudgetPolicyRecordInput;
use crate::persistence::{AiBudgetPolicyRecord, AiBudgetPolicyRecordWhereInput};
use crate::{
    AiBudgetIntervalInput, AiBudgetPolicyManagementLimits, AiBudgetPolicyView, AiError, AiScope,
    AiScopeInput, UpsertAiBudgetPolicyInput,
};
use graphql_orm::db::Database;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::graphql::filters::StringFilter;
use graphql_orm::graphql::orm::{DefaultWriteBackend, TransactionError, TransactionMode};
use sha2::{Digest, Sha256};
use std::fmt;
use uuid::Uuid;

/// Audit action recorded when a deployment policy is created.
pub const AI_DEPLOYMENT_BUDGET_POLICY_ACTION: &str = "ai.budget_policy.provision";
/// Audit actor kind of a deployment provisioner.
pub const AI_DEPLOYMENT_ACTOR_KIND: &str = "deployment";
pub(crate) const DEPLOYMENT_BUDGET_POLICY_REASON: &str = "deployment_budget_policy_created";
const MAXIMUM_DEPLOYMENT_ACTOR_BYTES: usize = 128;
const MAXIMUM_ENSURE_ATTEMPTS: usize = 3;

/// Nonsecret identity of a trusted host provisioning source, such as
/// `config-file` or `development-default`.
///
/// 1 to 128 bytes of `[a-z0-9._:-]`, starting with a letter or digit. It is
/// recorded as the audit actor subject and as the policy's provenance.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AiDeploymentActor(String);

impl AiDeploymentActor {
    /// Validates a deployment actor identity.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] for an empty, oversized or
    /// non-conforming value. The value is not echoed.
    pub fn new(id: impl Into<String>) -> Result<Self, AiError> {
        let id = id.into();
        if valid_deployment_actor(&id) {
            Ok(Self(id))
        } else {
            Err(AiError::InvalidConfiguration(
                "invalid AI deployment actor".to_owned(),
            ))
        }
    }

    /// Returns the actor identity.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AiDeploymentActor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("AiDeploymentActor").field(&self.0).finish()
    }
}

pub(crate) fn valid_deployment_actor(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAXIMUM_DEPLOYMENT_ACTOR_BYTES
        && id
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b':' | b'-')
        })
}

/// Desired deployment-owned budget policy for one exact scope.
///
/// Identity is the actor, scope, optional principal filter and interval. The
/// ceilings and `enabled` are the desired values; they are validated exactly
/// like an interactive upsert.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiDeploymentBudgetPolicy {
    /// Exact owning scope.
    pub scope: AiScope,
    /// Optional exact principal kind; supplied together with the subject.
    pub principal_kind: Option<String>,
    /// Optional exact principal subject.
    pub principal_subject: Option<String>,
    /// Reset interval.
    pub interval: AiBudgetIntervalInput,
    /// Optional total input-token ceiling.
    pub maximum_input_tokens: Option<i64>,
    /// Optional output-token ceiling.
    pub maximum_output_tokens: Option<i64>,
    /// Optional tool-unit ceiling.
    pub maximum_tool_units: Option<i64>,
    /// Optional image-unit ceiling.
    pub maximum_image_units: Option<i64>,
    /// Optional deployment-defined cost ceiling in microunits.
    pub maximum_cost_microunits: Option<i64>,
    /// Optional provider run/call ceiling.
    pub maximum_runs: Option<i64>,
    /// Whether the policy participates in new reservations when created.
    pub enabled: bool,
}

/// Result of [`OrmAiBudgetPolicyProvisioner::ensure_budget_policy`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum AiDeploymentBudgetPolicyOutcome {
    /// The policy did not exist and was created with one audit fact.
    Created(AiBudgetPolicyView),
    /// The policy already exists with exactly the requested values.
    Unchanged(AiBudgetPolicyView),
    /// The policy exists with other ceilings or `enabled`, for example after
    /// an MFA-authorized interactive edit. It was left untouched.
    Diverged(AiBudgetPolicyView),
}

impl AiDeploymentBudgetPolicyOutcome {
    /// Returns the current policy.
    pub fn policy(&self) -> &AiBudgetPolicyView {
        match self {
            Self::Created(policy) | Self::Unchanged(policy) | Self::Diverged(policy) => policy,
        }
    }
}

/// Trusted, idempotent provisioner of deployment-owned budget policies.
///
/// Construct it only in trusted host code. It holds no principal and checks no
/// MFA: a deployment policy is host configuration, not an interactive change.
/// It never updates, widens or re-enables an existing policy, and it can only
/// address policies it derived itself; interactive policies keep random IDs.
/// Interactive MFA-authorized edits of a provisioned policy are authoritative.
#[derive(Clone)]
pub struct OrmAiBudgetPolicyProvisioner {
    database: Database<DefaultWriteBackend>,
    actor: AiDeploymentActor,
    limits: AiBudgetPolicyManagementLimits,
}

impl OrmAiBudgetPolicyProvisioner {
    /// Creates a provisioner. Use the same `limits` as the host's interactive
    /// management and [`crate::AiRestorePolicyAuditLimits`].
    pub fn new(
        database: Database<DefaultWriteBackend>,
        actor: AiDeploymentActor,
        limits: AiBudgetPolicyManagementLimits,
    ) -> Self {
        Self {
            database,
            actor,
            limits,
        }
    }

    /// Ensures the deployment policy exists.
    ///
    /// Creates it with one redacted `ai.budget_policy.provision` audit fact
    /// (actor kind `deployment`, subject = actor) in the same transaction, or
    /// reports it unchanged or diverged without writing.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for an invalid scope, principal filter
    /// or ceiling (absent, negative or above the management maximum) and when
    /// the scope already holds the maximum number of policies.
    /// [`AiError::PersistenceFailed`] reports a stored row whose identity does
    /// not match its derived ID. [`AiError::Conflict`] is returned only if
    /// concurrent creation could not be resolved within a bounded retry.
    pub async fn ensure_budget_policy(
        &self,
        policy: AiDeploymentBudgetPolicy,
    ) -> Result<AiDeploymentBudgetPolicyOutcome, AiError> {
        validate_scope(&policy.scope)?;
        let mut input = UpsertAiBudgetPolicyInput {
            id: None,
            scope: AiScopeInput {
                kind: policy.scope.kind.clone(),
                id: policy.scope.id.clone(),
                tenant_id: policy.scope.tenant_id.clone(),
            },
            principal_kind: policy.principal_kind,
            principal_subject: policy.principal_subject,
            interval: policy.interval,
            maximum_input_tokens: policy.maximum_input_tokens,
            maximum_output_tokens: policy.maximum_output_tokens,
            maximum_tool_units: policy.maximum_tool_units,
            maximum_image_units: policy.maximum_image_units,
            maximum_cost_microunits: policy.maximum_cost_microunits,
            maximum_runs: policy.maximum_runs,
            enabled: policy.enabled,
            expected_version: None,
        };
        normalize_and_validate_budget_policy_input(&mut input, self.limits.maximum_ceiling())?;
        let scope = policy.scope;
        let exact_scope_key = scope_key(&scope);
        let id = deployment_budget_policy_id(
            self.actor.as_str(),
            &exact_scope_key,
            input.principal_kind.as_deref(),
            input.principal_subject.as_deref(),
            input.interval.as_str(),
        );
        let maximum_policies = i64::try_from(self.limits.maximum_policies_per_scope())
            .map_err(|_| AiError::InvalidConfiguration("invalid policy limit".to_owned()))?;
        for attempt in 1..=MAXIMUM_ENSURE_ATTEMPTS {
            let actor = self.actor.as_str().to_owned();
            let input = input.clone();
            let scope = scope.clone();
            let exact_scope_key = exact_scope_key.clone();
            let result = self
                .database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        if let Some(current) = tx
                            .find_by_id::<AiBudgetPolicyRecord>(&id)
                            .await
                            .map_err(OrmPublicError::from)?
                        {
                            if current.provisioned_by.as_deref() != Some(actor.as_str())
                                || current.scope_key != exact_scope_key
                                || current.scope_kind != scope.kind
                                || current.scope_id != scope.id
                                || current.tenant_id != scope.tenant_id
                                || current.principal_kind != input.principal_kind
                                || current.principal_subject != input.principal_subject
                                || current.interval_kind != input.interval.as_str()
                            {
                                return Err(OrmPublicError::new(OrmErrorCode::InternalError));
                            }
                            return Ok(if same_values(&current, &input) {
                                Outcome::Unchanged(current)
                            } else {
                                Outcome::Diverged(current)
                            });
                        }
                        let count = tx
                            .query::<AiBudgetPolicyRecord>()
                            .filter(AiBudgetPolicyRecordWhereInput {
                                scope_key: Some(StringFilter {
                                    eq: Some(exact_scope_key.clone()),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .count()
                            .await
                            .map_err(OrmPublicError::from)?;
                        if count >= maximum_policies {
                            return Err(OrmPublicError::new(OrmErrorCode::InvalidInput));
                        }
                        let record = tx
                            .insert::<AiBudgetPolicyRecord>(CreateAiBudgetPolicyRecordInput {
                                id,
                                provisioned_by: Some(actor.clone()),
                                scope_key: exact_scope_key,
                                scope_kind: scope.kind,
                                scope_id: scope.id,
                                tenant_id: scope.tenant_id,
                                principal_kind: input.principal_kind,
                                principal_subject: input.principal_subject,
                                interval_kind: input.interval.as_str().to_owned(),
                                maximum_input_tokens: input.maximum_input_tokens,
                                maximum_output_tokens: input.maximum_output_tokens,
                                maximum_tool_units: input.maximum_tool_units,
                                maximum_image_units: input.maximum_image_units,
                                maximum_cost_microunits: input.maximum_cost_microunits,
                                maximum_runs: input.maximum_runs,
                                enabled: input.enabled,
                            })
                            .await
                            .map_err(OrmPublicError::from)?;
                        insert_audit(
                            tx,
                            AuditFact {
                                actor_principal_kind: AI_DEPLOYMENT_ACTOR_KIND,
                                actor_subject: &actor,
                                action: AI_DEPLOYMENT_BUDGET_POLICY_ACTION,
                                resource_kind: "budget_policy",
                                resource_reference: &record.id.to_string(),
                                outcome: "allowed",
                                reason_code: DEPLOYMENT_BUDGET_POLICY_REASON,
                                policy_version: Some(record.row_version.to_string()),
                            },
                        )
                        .await?;
                        Ok(Outcome::Created(record))
                    })
                })
                .await;
            match result {
                Ok(Outcome::Created(record)) => {
                    return Ok(AiDeploymentBudgetPolicyOutcome::Created(
                        budget_policy_view(&record),
                    ));
                }
                Ok(Outcome::Unchanged(record)) => {
                    return Ok(AiDeploymentBudgetPolicyOutcome::Unchanged(
                        budget_policy_view(&record),
                    ));
                }
                Ok(Outcome::Diverged(record)) => {
                    return Ok(AiDeploymentBudgetPolicyOutcome::Diverged(
                        budget_policy_view(&record),
                    ));
                }
                // A concurrent ensure inserted the same derived ID first; the
                // next attempt reads it.
                Err(error) if attempt < MAXIMUM_ENSURE_ATTEMPTS && is_concurrent_insert(&error) => {
                    tokio::task::yield_now().await;
                }
                Err(error) => return Err(map_transaction(error)),
            }
        }
        Err(AiError::Conflict)
    }
}

enum Outcome {
    Created(AiBudgetPolicyRecord),
    Unchanged(AiBudgetPolicyRecord),
    Diverged(AiBudgetPolicyRecord),
}

fn is_concurrent_insert(error: &TransactionError) -> bool {
    matches!(error, TransactionError::Retryable(_))
        || matches!(
            error.public_error().code,
            OrmErrorCode::Conflict | OrmErrorCode::ConstraintViolation
        )
}

fn same_values(current: &AiBudgetPolicyRecord, input: &UpsertAiBudgetPolicyInput) -> bool {
    current.maximum_input_tokens == input.maximum_input_tokens
        && current.maximum_output_tokens == input.maximum_output_tokens
        && current.maximum_tool_units == input.maximum_tool_units
        && current.maximum_image_units == input.maximum_image_units
        && current.maximum_cost_microunits == input.maximum_cost_microunits
        && current.maximum_runs == input.maximum_runs
        && current.enabled == input.enabled
}

/// Derived v8 ID of a deployment policy. Interactive policies use random v4
/// IDs, so provisioning can never address one.
pub(crate) fn deployment_budget_policy_id(
    actor: &str,
    scope_key: &str,
    principal_kind: Option<&str>,
    principal_subject: Option<&str>,
    interval_kind: &str,
) -> Uuid {
    let mut hash = Sha256::new();
    hash.update(b"graphql-orm-ai/deployment-budget-policy/v1\0");
    for part in [
        actor,
        scope_key,
        principal_kind.unwrap_or(""),
        principal_subject.unwrap_or(""),
        interval_kind,
    ] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    let digest = hash.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Builder::from_custom_bytes(bytes).into_uuid()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_identity_is_bounded_and_not_echoed() {
        for invalid in ["", "-leading", "Upper", "has space", "secret=value"] {
            let error = AiDeploymentActor::new(invalid).unwrap_err();
            assert!(invalid.is_empty() || !error.to_string().contains(invalid));
        }
        assert!(AiDeploymentActor::new("x".repeat(129)).is_err());
        assert!(AiDeploymentActor::new("config-file:ai.default_v1").is_ok());
    }

    #[test]
    fn derived_ids_are_stable_v8_and_identity_bound() {
        let id = deployment_budget_policy_id("config", "k", None, None, "day");
        assert_eq!(
            id,
            deployment_budget_policy_id("config", "k", None, None, "day")
        );
        assert_eq!(id.get_version_num(), 8);
        for other in [
            deployment_budget_policy_id("other", "k", None, None, "day"),
            deployment_budget_policy_id("config", "k2", None, None, "day"),
            deployment_budget_policy_id("config", "k", Some("user"), Some("u"), "day"),
            deployment_budget_policy_id("config", "k", None, None, "hour"),
            // Length-prefixed parts cannot be shifted across boundaries.
            deployment_budget_policy_id("conf", "igk", None, None, "day"),
        ] {
            assert_ne!(id, other);
        }
        assert_ne!(Uuid::new_v4().get_version_num(), 8);
    }
}
