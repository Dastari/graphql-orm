//! ORM-backed GraphQL-managed AI configuration service.

#![cfg(any(feature = "sqlite", feature = "postgres"))]

use std::sync::Arc;

use agql_auth::{AuthPrincipal, Clock, RecentMfaPolicy};
use async_trait::async_trait;
use graphql_orm::db::Database;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::graphql::filters::{StringFilter, UuidFilter};
use graphql_orm::graphql::orm::{
    ConditionalUpdateOutcome, DefaultWriteBackend, OrderDirection, TransactionError,
    TransactionMode,
};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use serde_json::json;
use url::Url;
use uuid::Uuid;

use crate::orm_budget::BudgetPeriod;
use crate::persistence::*;
use crate::{
    AiBudgetAmounts, AiBudgetPolicyCapacityView, AiBudgetPolicyView,
    AiBudgetReservationCapacityView, AiBudgetScopeCapacityView, AiConfigurationAccessPolicy,
    AiConfigurationAction, AiConfigurationService, AiContentProtectionMode,
    AiContentProtectionPolicy, AiContentProtectionPolicyResolver, AiContentProtectionPolicyView,
    AiError, AiOpenAiCompatibleProfileInput, AiOpenAiCompatibleProfileView,
    AiProviderEndpointPolicy, AiProviderKindInput, AiProviderProfileView, AiRetentionPolicyView,
    AiRunState, AiScope, AiSecretStore, ReclaimAiBudgetReservationInput,
    RemoveAiProviderCredentialInput, SecretRef, SetAiContentProtectionPolicyInput,
    SetAiRetentionPolicyInput, UpsertAiBudgetPolicyInput, UpsertAiProviderProfileInput,
};

/// Deployment hard bounds for GraphQL-managed budget policies.
///
/// GraphQL values may only narrow these ceilings. This type does not grant
/// configuration authority and does not replace the transactional provider
/// reservation limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiBudgetPolicyManagementLimits {
    maximum_ceiling: AiBudgetAmounts,
    maximum_policies_per_scope: usize,
}

impl AiBudgetPolicyManagementLimits {
    /// Creates validated deployment management bounds.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] unless the per-scope policy
    /// count is in `1..=100`.
    pub fn new(
        maximum_ceiling: AiBudgetAmounts,
        maximum_policies_per_scope: usize,
    ) -> Result<Self, AiError> {
        if !(1..=100).contains(&maximum_policies_per_scope) {
            return Err(AiError::InvalidConfiguration(
                "invalid budget-policy management limits".to_owned(),
            ));
        }
        Ok(Self {
            maximum_ceiling,
            maximum_policies_per_scope,
        })
    }

    /// Returns the greatest ceiling GraphQL may configure in each dimension.
    pub const fn maximum_ceiling(self) -> AiBudgetAmounts {
        self.maximum_ceiling
    }

    /// Returns the maximum number of policies for one exact scope.
    pub const fn maximum_policies_per_scope(self) -> usize {
        self.maximum_policies_per_scope
    }
}

/// Deployment hard bounds for privileged budget-reservation reclamation.
///
/// This type proves only that the deployment reviewed and enabled the surface
/// and chose how long an expired reservation must remain unresolved. It grants
/// no authority: the host still authorizes
/// [`AiConfigurationAction::ManageBudgetReclamation`] for the exact scope and
/// the caller still needs current recent MFA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiBudgetReclamationLimits {
    minimum_expired_age: time::Duration,
    maximum_reservation_scan: usize,
}

impl AiBudgetReclamationLimits {
    /// Creates validated deployment reclamation bounds.
    ///
    /// `minimum_expired_age` is how long a reservation's `expires_at` must
    /// already have passed before it may be resolved. It exists so an
    /// in-flight provider turn whose worker is merely slow can never be
    /// charged out from underneath itself. `maximum_reservation_scan` is a
    /// deployment ceiling; the active database pagination maximum may narrow
    /// the reported window further, in which case a full page is
    /// conservatively marked truncated.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] unless the age is positive
    /// and the bounded read window is in `1..=1000`.
    pub fn new(
        minimum_expired_age: time::Duration,
        maximum_reservation_scan: usize,
    ) -> Result<Self, AiError> {
        if !minimum_expired_age.is_positive() || !(1..=1000).contains(&maximum_reservation_scan) {
            return Err(AiError::InvalidConfiguration(
                "invalid budget reclamation limits".to_owned(),
            ));
        }
        Ok(Self {
            minimum_expired_age,
            maximum_reservation_scan,
        })
    }

    /// Returns how long an expiry must already have passed.
    pub const fn minimum_expired_age(self) -> time::Duration {
        self.minimum_expired_age
    }

    /// Returns the bounded reservation read window.
    pub const fn maximum_reservation_scan(self) -> usize {
        self.maximum_reservation_scan
    }
}

/// Bounded read window used when the deployment has not enabled reclamation.
const DEFAULT_BUDGET_RESERVATION_SCAN: usize = 200;

/// Durable run states that can no longer reconcile their own reservation.
const TERMINAL_RUN_STATES: [&str; 4] = [
    AiRunState::Completed.as_str(),
    AiRunState::Failed.as_str(),
    AiRunState::Cancelled.as_str(),
    AiRunState::RecoveryRequired.as_str(),
];

/// Reservation states that still hold capacity against a policy ceiling.
const UNRESOLVED_RESERVATION_STATES: [&str; 2] = ["reserved", "uncertain"];

/// Durable configuration backend using generated ORM APIs and a compensating
/// secret-reference saga. Secret plaintext never enters an ORM input.
#[derive(Clone)]
pub struct OrmAiConfigurationService {
    database: Database<DefaultWriteBackend>,
    access_policy: Arc<dyn AiConfigurationAccessPolicy>,
    endpoint_policy: Arc<dyn AiProviderEndpointPolicy>,
    recent_mfa_policy: RecentMfaPolicy,
    clock: Arc<dyn Clock>,
    secret_store: Arc<dyn AiSecretStore>,
    budget_policy_limits: Option<AiBudgetPolicyManagementLimits>,
    budget_reclamation_limits: Option<AiBudgetReclamationLimits>,
}

impl OrmAiConfigurationService {
    /// Creates a fail-closed configuration service.
    pub fn new(
        database: Database<DefaultWriteBackend>,
        access_policy: Arc<dyn AiConfigurationAccessPolicy>,
        endpoint_policy: Arc<dyn AiProviderEndpointPolicy>,
        recent_mfa_policy: RecentMfaPolicy,
        clock: Arc<dyn Clock>,
        secret_store: Arc<dyn AiSecretStore>,
    ) -> Self {
        Self {
            database,
            access_policy,
            endpoint_policy,
            recent_mfa_policy,
            clock,
            secret_store,
            budget_policy_limits: None,
            budget_reclamation_limits: None,
        }
    }

    /// Enables GraphQL budget-policy management under deployment hard bounds.
    ///
    /// Without this explicit opt-in, budget-policy reads remain independently
    /// authorized but mutations fail closed as invalid configuration.
    pub fn with_budget_policy_management(mut self, limits: AiBudgetPolicyManagementLimits) -> Self {
        self.budget_policy_limits = Some(limits);
        self
    }

    /// Enables privileged reclamation of stranded budget reservations under
    /// deployment hard bounds.
    ///
    /// Without this explicit opt-in, capacity reporting still works and every
    /// reservation reports `reclaimable: false`, while the mutation fails
    /// closed as invalid configuration. Enabling it does not authorize anyone:
    /// the host still decides
    /// [`AiConfigurationAction::ManageBudgetReclamation`] per exact scope.
    pub fn with_budget_reservation_reclamation(
        mut self,
        limits: AiBudgetReclamationLimits,
    ) -> Self {
        self.budget_reclamation_limits = Some(limits);
        self
    }

    /// Returns the underlying ORM database handle for host schema wiring.
    pub fn database(&self) -> &Database<DefaultWriteBackend> {
        &self.database
    }

    async fn require_access(
        &self,
        principal: &AuthPrincipal,
        scope: &AiScope,
        action: AiConfigurationAction,
    ) -> Result<(), AiError> {
        validate_scope(scope)?;
        if self
            .access_policy
            .can_configure(principal, scope, action)
            .await
        {
            Ok(())
        } else {
            Err(AiError::Forbidden)
        }
    }

    fn require_recent_mfa(&self, principal: &AuthPrincipal) -> Result<(), AiError> {
        let user = principal.as_user().ok_or(AiError::RecentMfaRequired)?;
        self.recent_mfa_policy
            .evaluate(user, self.clock.as_ref())
            .map_err(|_| AiError::RecentMfaRequired)
    }

    async fn profile(&self, id: Uuid) -> Result<AiProviderProfileRecord, AiError> {
        AiProviderProfileRecord::find_by_id(&self.database, &id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::NotFound)
    }

    async fn complete_cleanup(&self, cleanup_id: Option<Uuid>, reference: &SecretRef) {
        let Some(cleanup_id) = cleanup_id else {
            return;
        };
        if self.secret_store.delete(reference).await.is_ok() {
            let _ = AiSecretCleanupRecord::update_by_id(
                &self.database,
                &cleanup_id,
                UpdateAiSecretCleanupRecordInput {
                    state: Some("complete".to_owned()),
                    completed_at: Some(Some(unix_seconds())),
                    ..Default::default()
                },
            )
            .await;
        }
    }
}

#[async_trait]
impl AiConfigurationService for OrmAiConfigurationService {
    async fn provider_profiles(
        &self,
        principal: &AuthPrincipal,
        scope: AiScope,
    ) -> Result<Vec<AiProviderProfileView>, AiError> {
        self.require_access(
            principal,
            &scope,
            AiConfigurationAction::ReadProviderProfiles,
        )
        .await?;
        let scope_key = scope_key(&scope);
        let rows = self
            .database
            .transaction(TransactionMode::Default, move |tx| {
                Box::pin(async move {
                    tx.query::<AiProviderProfileRecord>()
                        .filter(AiProviderProfileRecordWhereInput {
                            scope_key: Some(StringFilter {
                                eq: Some(scope_key),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .default_order()
                        .limit(101)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .await
            .map_err(map_transaction)?;
        if rows.len() > 100 {
            return Err(AiError::InvalidConfiguration(
                "provider profile scope exceeds the bounded limit".to_owned(),
            ));
        }
        rows.iter().map(provider_view).collect()
    }

    async fn content_protection_policy(
        &self,
        principal: &AuthPrincipal,
        scope: AiScope,
    ) -> Result<Option<AiContentProtectionPolicyView>, AiError> {
        self.require_access(
            principal,
            &scope,
            AiConfigurationAction::ReadContentProtection,
        )
        .await?;
        Ok(load_content_policy(&self.database, &scope)
            .await?
            .as_ref()
            .map(content_policy_view))
    }

    async fn retention_policy(
        &self,
        principal: &AuthPrincipal,
        scope: AiScope,
    ) -> Result<Option<AiRetentionPolicyView>, AiError> {
        self.require_access(principal, &scope, AiConfigurationAction::ReadRetention)
            .await?;
        Ok(load_retention_policy(&self.database, &scope)
            .await?
            .as_ref()
            .map(retention_policy_view))
    }

    async fn budget_policies(
        &self,
        principal: &AuthPrincipal,
        scope: AiScope,
    ) -> Result<Vec<AiBudgetPolicyView>, AiError> {
        self.require_access(principal, &scope, AiConfigurationAction::ReadBudgetPolicies)
            .await?;
        let exact_scope_key = scope_key(&scope);
        let rows = self
            .database
            .transaction(TransactionMode::Default, move |tx| {
                Box::pin(async move {
                    tx.query::<AiBudgetPolicyRecord>()
                        .filter(AiBudgetPolicyRecordWhereInput {
                            scope_key: Some(StringFilter {
                                eq: Some(exact_scope_key),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .default_order()
                        .limit(101)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .await
            .map_err(map_transaction)?;
        if rows.len() > 100 {
            return Err(AiError::InvalidConfiguration(
                "budget-policy scope exceeds the bounded read limit".to_owned(),
            ));
        }
        if rows.iter().any(|record| {
            record.scope_key != exact_scope_key_for_record(record)
                || record.scope_kind != scope.kind
                || record.scope_id != scope.id
                || record.tenant_id != scope.tenant_id
        }) {
            return Err(AiError::PersistenceFailed);
        }
        Ok(rows.iter().map(budget_policy_view).collect())
    }

    async fn upsert_provider_profile(
        &self,
        principal: &AuthPrincipal,
        input: UpsertAiProviderProfileInput,
    ) -> Result<AiProviderProfileView, AiError> {
        self.require_recent_mfa(principal)?;
        let scope: AiScope = input.scope.into();
        self.require_access(
            principal,
            &scope,
            AiConfigurationAction::ManageProviderProfiles,
        )
        .await?;
        let display_name = input.display_name.trim().to_owned();
        if display_name.is_empty() || display_name.len() > 200 {
            return Err(AiError::InvalidInput(
                "invalid provider display name".to_owned(),
            ));
        }
        let base_url = normalize_endpoint(
            input.provider_kind,
            input.base_url,
            self.endpoint_policy.as_ref(),
        )?;
        let data_policy = provider_data_policy(input.provider_kind, input.openai_compatible)?;
        let actor_kind = principal_kind(principal);
        let actor_subject = principal.subject().to_owned();
        let scope_hash = scope_key(&scope);
        let provider_kind = input.provider_kind.as_str().to_owned();
        let expected_version = input.expected_version;
        let profile_id = input.id;
        let profile = self
            .database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let profile = match (profile_id, expected_version) {
                        (None, None) => tx
                            .insert::<AiProviderProfileRecord>(CreateAiProviderProfileRecordInput {
                                scope_key: scope_hash,
                                scope_kind: scope.kind,
                                scope_id: scope.id,
                                tenant_id: scope.tenant_id,
                                provider_kind,
                                display_name,
                                base_url,
                                credential_reference: None,
                                enabled: input.enabled,
                                data_policy,
                                limits: json!({}),
                            })
                            .await
                            .map_err(OrmPublicError::from)?,
                        (Some(id), Some(expected_version)) => {
                            let current = tx
                                .find_by_id::<AiProviderProfileRecord>(&id)
                                .await
                                .map_err(OrmPublicError::from)?
                                .ok_or_else(OrmPublicError::not_found)?;
                            if current.scope_key != scope_hash {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                            if provider_kind == AiProviderKindInput::LocalHarness.as_str()
                                && current.credential_reference.is_some()
                            {
                                return Err(OrmPublicError::new(OrmErrorCode::InvalidInput));
                            }
                            match tx
                                .compare_and_swap::<AiProviderProfileRecord>(
                                    &id,
                                    expected_version,
                                    AiProviderProfileRecordWhereInput::default(),
                                    UpdateAiProviderProfileRecordInput {
                                        provider_kind: Some(provider_kind),
                                        display_name: Some(display_name),
                                        base_url: Some(base_url),
                                        data_policy: Some(data_policy),
                                        enabled: Some(input.enabled),
                                        ..Default::default()
                                    },
                                )
                                .await
                                .map_err(OrmPublicError::from)?
                            {
                                ConditionalUpdateOutcome::Updated(profile) => profile,
                                ConditionalUpdateOutcome::NotFound => {
                                    return Err(OrmPublicError::not_found());
                                }
                                ConditionalUpdateOutcome::Conflict => {
                                    return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                                }
                            }
                        }
                        _ => return Err(OrmPublicError::new(OrmErrorCode::InvalidInput)),
                    };
                    insert_audit(
                        tx,
                        AuditFact {
                            actor_principal_kind: &actor_kind,
                            actor_subject: &actor_subject,
                            action: "ai.provider_profile.upsert",
                            resource_kind: "provider_profile",
                            resource_reference: &profile.id.to_string(),
                            outcome: "allowed",
                            reason_code: "configuration_updated",
                            policy_version: None,
                        },
                    )
                    .await?;
                    Ok(profile)
                })
            })
            .await
            .map_err(map_transaction)?;
        provider_view(&profile)
    }

    async fn set_provider_credential(
        &self,
        principal: &AuthPrincipal,
        profile_id: Uuid,
        credential: SecretString,
        expected_version: i64,
    ) -> Result<AiProviderProfileView, AiError> {
        self.require_recent_mfa(principal)?;
        let existing = self.profile(profile_id).await?;
        let scope = profile_scope(&existing);
        self.require_access(
            principal,
            &scope,
            AiConfigurationAction::ManageProviderCredentials,
        )
        .await?;
        if existing.provider_kind == AiProviderKindInput::LocalHarness.as_str() {
            return Err(AiError::InvalidInput(
                "local-harness profiles do not accept provider credentials".to_owned(),
            ));
        }
        if existing.row_version != expected_version {
            return Err(AiError::Conflict);
        }
        let new_reference = self
            .secret_store
            .put(None, credential)
            .await
            .map_err(|_| AiError::PersistenceFailed)?;
        let previous_reference = existing
            .credential_reference
            .as_deref()
            .map(|reference| SecretRef::parse(reference.to_owned()))
            .transpose()
            .map_err(|_| AiError::InvalidConfiguration("invalid secret reference".to_owned()))?;
        let cleanup_id = previous_reference.as_ref().map(|_| Uuid::new_v4());
        let actor_kind = principal_kind(principal);
        let actor_subject = principal.subject().to_owned();
        let new_reference_value = new_reference.as_str().to_owned();
        let previous_for_tx = previous_reference.clone();
        let result = self
            .database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let current = tx
                        .find_by_id::<AiProviderProfileRecord>(&profile_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    if current.row_version != expected_version {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    let profile = match tx
                        .compare_and_swap::<AiProviderProfileRecord>(
                            &profile_id,
                            expected_version,
                            AiProviderProfileRecordWhereInput::default(),
                            UpdateAiProviderProfileRecordInput {
                                credential_reference: Some(Some(new_reference_value)),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?
                    {
                        ConditionalUpdateOutcome::Updated(profile) => profile,
                        ConditionalUpdateOutcome::NotFound => {
                            return Err(OrmPublicError::not_found());
                        }
                        ConditionalUpdateOutcome::Conflict => {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                    };
                    if let (Some(cleanup_id), Some(previous)) =
                        (cleanup_id, previous_for_tx.as_ref())
                    {
                        tx.insert::<AiSecretCleanupRecord>(CreateAiSecretCleanupRecordInput {
                            id: cleanup_id,
                            secret_reference: previous.as_str().to_owned(),
                            reason_code: "credential_rotated".to_owned(),
                            state: "pending".to_owned(),
                            retry_count: 0,
                            next_attempt_at: Some(unix_seconds()),
                            completed_at: None,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                    }
                    insert_audit(
                        tx,
                        AuditFact {
                            actor_principal_kind: &actor_kind,
                            actor_subject: &actor_subject,
                            action: "ai.provider_credential.set",
                            resource_kind: "provider_profile",
                            resource_reference: &profile_id.to_string(),
                            outcome: "allowed",
                            reason_code: "credential_rotated",
                            policy_version: None,
                        },
                    )
                    .await?;
                    Ok(profile)
                })
            })
            .await;
        let profile = match result {
            Ok(profile) => profile,
            Err(error) => {
                let _ = self.secret_store.delete(&new_reference).await;
                return Err(map_transaction(error));
            }
        };
        if let Some(previous) = previous_reference.as_ref() {
            self.complete_cleanup(cleanup_id, previous).await;
        }
        provider_view(&profile)
    }

    async fn remove_provider_credential(
        &self,
        principal: &AuthPrincipal,
        input: RemoveAiProviderCredentialInput,
    ) -> Result<AiProviderProfileView, AiError> {
        self.require_recent_mfa(principal)?;
        let existing = self.profile(input.profile_id).await?;
        let scope = profile_scope(&existing);
        self.require_access(
            principal,
            &scope,
            AiConfigurationAction::ManageProviderCredentials,
        )
        .await?;
        if existing.row_version != input.expected_version {
            return Err(AiError::Conflict);
        }
        let previous_reference = existing
            .credential_reference
            .as_deref()
            .map(|reference| SecretRef::parse(reference.to_owned()))
            .transpose()
            .map_err(|_| AiError::InvalidConfiguration("invalid secret reference".to_owned()))?;
        let cleanup_id = previous_reference.as_ref().map(|_| Uuid::new_v4());
        let previous_for_tx = previous_reference.clone();
        let actor_kind = principal_kind(principal);
        let actor_subject = principal.subject().to_owned();
        let profile_id = input.profile_id;
        let expected_version = input.expected_version;
        let profile = self
            .database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let profile = match tx
                        .compare_and_swap::<AiProviderProfileRecord>(
                            &profile_id,
                            expected_version,
                            AiProviderProfileRecordWhereInput::default(),
                            UpdateAiProviderProfileRecordInput {
                                credential_reference: Some(None),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?
                    {
                        ConditionalUpdateOutcome::Updated(profile) => profile,
                        ConditionalUpdateOutcome::NotFound => {
                            return Err(OrmPublicError::not_found());
                        }
                        ConditionalUpdateOutcome::Conflict => {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                    };
                    if let (Some(cleanup_id), Some(previous)) =
                        (cleanup_id, previous_for_tx.as_ref())
                    {
                        tx.insert::<AiSecretCleanupRecord>(CreateAiSecretCleanupRecordInput {
                            id: cleanup_id,
                            secret_reference: previous.as_str().to_owned(),
                            reason_code: "credential_removed".to_owned(),
                            state: "pending".to_owned(),
                            retry_count: 0,
                            next_attempt_at: Some(unix_seconds()),
                            completed_at: None,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                    }
                    insert_audit(
                        tx,
                        AuditFact {
                            actor_principal_kind: &actor_kind,
                            actor_subject: &actor_subject,
                            action: "ai.provider_credential.remove",
                            resource_kind: "provider_profile",
                            resource_reference: &profile_id.to_string(),
                            outcome: "allowed",
                            reason_code: "credential_removed",
                            policy_version: None,
                        },
                    )
                    .await?;
                    Ok(profile)
                })
            })
            .await
            .map_err(map_transaction)?;
        if let Some(previous) = previous_reference.as_ref() {
            self.complete_cleanup(cleanup_id, previous).await;
        }
        provider_view(&profile)
    }

    async fn set_content_protection_policy(
        &self,
        principal: &AuthPrincipal,
        input: SetAiContentProtectionPolicyInput,
    ) -> Result<AiContentProtectionPolicyView, AiError> {
        self.require_recent_mfa(principal)?;
        let scope: AiScope = input.scope.into();
        self.require_access(
            principal,
            &scope,
            AiConfigurationAction::ManageContentProtection,
        )
        .await?;
        let mode: AiContentProtectionMode = input.mode.into();
        match (mode, input.key_policy_reference.as_deref()) {
            (AiContentProtectionMode::DatabaseManaged, Some(_))
            | (AiContentProtectionMode::ApplicationEncrypted, None) => {
                return Err(AiError::InvalidInput(
                    "content-protection key policy does not match mode".to_owned(),
                ));
            }
            _ => {}
        }
        let scope_hash = scope_key(&scope);
        let actor_kind = principal_kind(principal);
        let actor_subject = principal.subject().to_owned();
        let expected_version = input.expected_version;
        let protection_mode = protection_mode_value(mode).to_owned();
        let ready = mode == AiContentProtectionMode::DatabaseManaged;
        let migration_state = if ready { "ready" } else { "pending" }.to_owned();
        let key_policy_reference = input.key_policy_reference;
        let now = unix_seconds();
        let record = self
            .database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let existing = tx
                        .query::<AiContentProtectionPolicyRecord>()
                        .filter(AiContentProtectionPolicyRecordWhereInput {
                            scope_key: Some(StringFilter {
                                eq: Some(scope_hash.clone()),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .limit(2)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)?;
                    if existing.len() > 1 {
                        return Err(OrmPublicError::new(
                            OrmErrorCode::AuthorizationMisconfigured,
                        ));
                    }
                    let record = match (existing.into_iter().next(), expected_version) {
                        (None, None) => tx
                            .insert::<AiContentProtectionPolicyRecord>(
                                CreateAiContentProtectionPolicyRecordInput {
                                    scope_key: scope_hash,
                                    scope_kind: scope.kind,
                                    scope_id: scope.id,
                                    tenant_id: scope.tenant_id,
                                    protection_mode,
                                    key_policy_reference,
                                    key_version: None,
                                    migration_state,
                                    ready,
                                    effective_at: now,
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?,
                        (Some(current), Some(expected_version)) => match tx
                            .compare_and_swap::<AiContentProtectionPolicyRecord>(
                                &current.id,
                                expected_version,
                                AiContentProtectionPolicyRecordWhereInput::default(),
                                UpdateAiContentProtectionPolicyRecordInput {
                                    protection_mode: Some(protection_mode),
                                    key_policy_reference: Some(key_policy_reference),
                                    key_version: Some(None),
                                    migration_state: Some(migration_state),
                                    ready: Some(ready),
                                    effective_at: Some(now),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?
                        {
                            ConditionalUpdateOutcome::Updated(record) => record,
                            ConditionalUpdateOutcome::NotFound => {
                                return Err(OrmPublicError::not_found());
                            }
                            ConditionalUpdateOutcome::Conflict => {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                        },
                        _ => return Err(OrmPublicError::new(OrmErrorCode::Conflict)),
                    };
                    insert_audit(
                        tx,
                        AuditFact {
                            actor_principal_kind: &actor_kind,
                            actor_subject: &actor_subject,
                            action: "ai.content_protection.set",
                            resource_kind: "content_protection_policy",
                            resource_reference: &record.id.to_string(),
                            outcome: "allowed",
                            reason_code: "content_protection_updated",
                            policy_version: None,
                        },
                    )
                    .await?;
                    Ok(record)
                })
            })
            .await
            .map_err(map_transaction)?;
        Ok(content_policy_view(&record))
    }

    async fn set_retention_policy(
        &self,
        principal: &AuthPrincipal,
        input: SetAiRetentionPolicyInput,
    ) -> Result<AiRetentionPolicyView, AiError> {
        self.require_recent_mfa(principal)?;
        validate_retention_input(&input)?;
        let scope: AiScope = input.scope.into();
        self.require_access(principal, &scope, AiConfigurationAction::ManageRetention)
            .await?;
        let scope_hash = scope_key(&scope);
        let actor_kind = principal_kind(principal);
        let actor_subject = principal.subject().to_owned();
        let expected_version = input.expected_version;
        let record = self
            .database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let existing = tx
                        .query::<AiRetentionPolicyRecord>()
                        .filter(AiRetentionPolicyRecordWhereInput {
                            scope_key: Some(StringFilter {
                                eq: Some(scope_hash.clone()),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .limit(2)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)?;
                    if existing.len() > 1 {
                        return Err(OrmPublicError::new(
                            OrmErrorCode::AuthorizationMisconfigured,
                        ));
                    }
                    let record = match (existing.into_iter().next(), expected_version) {
                        (None, None) => tx
                            .insert::<AiRetentionPolicyRecord>(CreateAiRetentionPolicyRecordInput {
                                scope_key: Some(scope_hash),
                                scope_kind: scope.kind,
                                scope_id: scope.id,
                                tenant_id: scope.tenant_id,
                                message_retention_seconds: input.message_retention_seconds,
                                delta_retention_seconds: input.delta_retention_seconds,
                                raw_payload_retention_seconds: input.raw_payload_retention_seconds,
                                audit_retention_seconds: input.audit_retention_seconds,
                                deleted_content_purge_seconds: input.deleted_content_purge_seconds,
                                provider_file_delete_required: input.provider_file_delete_required,
                                inbox_event_retention_seconds: Some(
                                    input.inbox_event_retention_seconds,
                                ),
                                inbox_minimum_events: Some(input.inbox_minimum_events),
                            })
                            .await
                            .map_err(OrmPublicError::from)?,
                        (Some(current), Some(expected_version)) => match tx
                            .compare_and_swap::<AiRetentionPolicyRecord>(
                                &current.id,
                                expected_version,
                                AiRetentionPolicyRecordWhereInput {
                                    scope_key: Some(StringFilter {
                                        eq: Some(scope_hash),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                },
                                UpdateAiRetentionPolicyRecordInput {
                                    message_retention_seconds: Some(
                                        input.message_retention_seconds,
                                    ),
                                    delta_retention_seconds: Some(input.delta_retention_seconds),
                                    raw_payload_retention_seconds: Some(
                                        input.raw_payload_retention_seconds,
                                    ),
                                    audit_retention_seconds: Some(input.audit_retention_seconds),
                                    deleted_content_purge_seconds: Some(
                                        input.deleted_content_purge_seconds,
                                    ),
                                    provider_file_delete_required: Some(
                                        input.provider_file_delete_required,
                                    ),
                                    inbox_event_retention_seconds: Some(Some(
                                        input.inbox_event_retention_seconds,
                                    )),
                                    inbox_minimum_events: Some(Some(input.inbox_minimum_events)),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?
                        {
                            ConditionalUpdateOutcome::Updated(record) => record,
                            ConditionalUpdateOutcome::NotFound => {
                                return Err(OrmPublicError::not_found());
                            }
                            ConditionalUpdateOutcome::Conflict => {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                        },
                        _ => return Err(OrmPublicError::new(OrmErrorCode::Conflict)),
                    };
                    insert_audit(
                        tx,
                        AuditFact {
                            actor_principal_kind: &actor_kind,
                            actor_subject: &actor_subject,
                            action: "ai.retention_policy.set",
                            resource_kind: "retention_policy",
                            resource_reference: &record.id.to_string(),
                            outcome: "allowed",
                            reason_code: "retention_policy_updated",
                            policy_version: Some(record.row_version.to_string()),
                        },
                    )
                    .await?;
                    Ok(record)
                })
            })
            .await
            .map_err(map_transaction)?;
        Ok(retention_policy_view(&record))
    }

    async fn upsert_budget_policy(
        &self,
        principal: &AuthPrincipal,
        mut input: UpsertAiBudgetPolicyInput,
    ) -> Result<AiBudgetPolicyView, AiError> {
        self.require_recent_mfa(principal)?;
        let scope: AiScope = input.scope.clone().into();
        self.require_access(
            principal,
            &scope,
            AiConfigurationAction::ManageBudgetPolicies,
        )
        .await?;
        let limits = self.budget_policy_limits.ok_or_else(|| {
            AiError::InvalidConfiguration("budget-policy management is not enabled".to_owned())
        })?;
        normalize_and_validate_budget_policy_input(&mut input, limits.maximum_ceiling())?;
        let exact_scope_key = scope_key(&scope);
        let actor_kind = principal_kind(principal);
        let actor_subject = principal.subject().to_owned();
        let id = input.id;
        let expected_version = input.expected_version;
        let maximum_policies = i64::try_from(limits.maximum_policies_per_scope())
            .map_err(|_| AiError::InvalidConfiguration("invalid policy limit".to_owned()))?;
        let record = self
            .database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let record = match (id, expected_version) {
                        (None, None) => {
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
                            tx.insert::<AiBudgetPolicyRecord>(CreateAiBudgetPolicyRecordInput {
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
                            .map_err(OrmPublicError::from)?
                        }
                        (Some(id), Some(expected_version)) if expected_version >= 0 => {
                            let current = tx
                                .find_by_id::<AiBudgetPolicyRecord>(&id)
                                .await
                                .map_err(OrmPublicError::from)?
                                .ok_or_else(OrmPublicError::not_found)?;
                            if current.scope_key != exact_scope_key
                                || current.scope_kind != scope.kind
                                || current.scope_id != scope.id
                                || current.tenant_id != scope.tenant_id
                                || current.principal_kind != input.principal_kind
                                || current.principal_subject != input.principal_subject
                                || current.interval_kind != input.interval.as_str()
                            {
                                return Err(OrmPublicError::not_found());
                            }
                            match tx
                                .compare_and_swap::<AiBudgetPolicyRecord>(
                                    &id,
                                    expected_version,
                                    AiBudgetPolicyRecordWhereInput {
                                        scope_key: Some(StringFilter {
                                            eq: Some(exact_scope_key),
                                            ..Default::default()
                                        }),
                                        ..Default::default()
                                    },
                                    UpdateAiBudgetPolicyRecordInput {
                                        maximum_input_tokens: Some(input.maximum_input_tokens),
                                        maximum_output_tokens: Some(input.maximum_output_tokens),
                                        maximum_tool_units: Some(input.maximum_tool_units),
                                        maximum_image_units: Some(input.maximum_image_units),
                                        maximum_cost_microunits: Some(
                                            input.maximum_cost_microunits,
                                        ),
                                        maximum_runs: Some(input.maximum_runs),
                                        enabled: Some(input.enabled),
                                        ..Default::default()
                                    },
                                )
                                .await
                                .map_err(OrmPublicError::from)?
                            {
                                ConditionalUpdateOutcome::Updated(record) => record,
                                ConditionalUpdateOutcome::NotFound => {
                                    return Err(OrmPublicError::not_found());
                                }
                                ConditionalUpdateOutcome::Conflict => {
                                    return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                                }
                            }
                        }
                        _ => return Err(OrmPublicError::new(OrmErrorCode::Conflict)),
                    };
                    insert_audit(
                        tx,
                        AuditFact {
                            actor_principal_kind: &actor_kind,
                            actor_subject: &actor_subject,
                            action: "ai.budget_policy.upsert",
                            resource_kind: "budget_policy",
                            resource_reference: &record.id.to_string(),
                            outcome: "allowed",
                            reason_code: "budget_policy_updated",
                            policy_version: Some(record.row_version.to_string()),
                        },
                    )
                    .await?;
                    Ok(record)
                })
            })
            .await
            .map_err(map_transaction)?;
        Ok(budget_policy_view(&record))
    }

    async fn budget_scope_capacity(
        &self,
        principal: &AuthPrincipal,
        scope: AiScope,
    ) -> Result<AiBudgetScopeCapacityView, AiError> {
        self.require_access(principal, &scope, AiConfigurationAction::ReadBudgetPolicies)
            .await?;
        let now = self.clock.now();
        let reclamation = self.budget_reclamation_limits;
        let requested_window = reclamation.map_or(DEFAULT_BUDGET_RESERVATION_SCAN, |limits| {
            limits.maximum_reservation_scan()
        });
        // Typed ORM queries always honor the database's pagination ceiling.
        // Narrow the administrative window to that ceiling instead of asking
        // for a larger page that the ORM would silently clamp. When the
        // ceiling leaves no room for a look-ahead record, a full page is
        // conservatively reported as truncated.
        let database_maximum = self
            .database
            .pagination_config()
            .max_limit
            .and_then(|maximum| usize::try_from(maximum.max(0)).ok());
        let window =
            database_maximum.map_or(requested_window, |maximum| requested_window.min(maximum));
        let has_lookahead = database_maximum.is_none_or(|maximum| window < maximum);
        let requested_scan = if has_lookahead {
            window.saturating_add(1)
        } else {
            window
        };
        let scan_limit = i64::try_from(requested_scan)
            .map_err(|_| AiError::InvalidConfiguration("invalid scan window".to_owned()))?;
        let exact_scope_key = scope_key(&scope);
        let query_scope = scope.clone();
        let (policies, counters, reservations, run_reclamation_evidence) = self
            .database
            .transaction(TransactionMode::Default, move |tx| {
                Box::pin(async move {
                    let policies = tx
                        .query::<AiBudgetPolicyRecord>()
                        .filter(AiBudgetPolicyRecordWhereInput {
                            scope_key: Some(StringFilter {
                                eq: Some(exact_scope_key.clone()),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .default_order()
                        .limit(101)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)?;
                    if policies.len() > 100 {
                        return Err(OrmPublicError::new(OrmErrorCode::PageLimitExceeded));
                    }
                    if policies.iter().any(|record| {
                        record.scope_key != exact_scope_key
                            || record.scope_kind != query_scope.kind
                            || record.scope_id != query_scope.id
                            || record.tenant_id != query_scope.tenant_id
                    }) {
                        return Err(OrmPublicError::new(OrmErrorCode::InternalError));
                    }

                    let mut counters = Vec::with_capacity(policies.len());
                    for policy in &policies {
                        let period =
                            crate::orm_budget::budget_period(&policy.interval_kind, now)
                                .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        let counter = tx
                            .query::<AiBudgetCounterRecord>()
                            .filter(AiBudgetCounterRecordWhereInput {
                                budget_policy_id: Some(UuidFilter {
                                    eq: Some(policy.id),
                                    ..Default::default()
                                }),
                                period_key: Some(StringFilter {
                                    eq: Some(period.key.clone()),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .limit(1)
                            .fetch_one()
                            .await
                            .map_err(OrmPublicError::from)?;
                        counters.push((period, counter));
                    }

                    let tenant_id = Some(match &query_scope.tenant_id {
                        Some(tenant_id) => StringFilter {
                            eq: Some(tenant_id.clone()),
                            ..Default::default()
                        },
                        None => StringFilter {
                            is_null: Some(true),
                            ..Default::default()
                        },
                    });
                    let reservations = tx
                        .query::<AiBudgetReservationRecord>()
                        .filter(AiBudgetReservationRecordWhereInput {
                            scope_kind: Some(StringFilter {
                                eq: Some(query_scope.kind.clone()),
                                ..Default::default()
                            }),
                            scope_id: Some(StringFilter {
                                eq: Some(query_scope.id.clone()),
                                ..Default::default()
                            }),
                            tenant_id,
                            state: Some(StringFilter {
                                in_list: Some(
                                    UNRESOLVED_RESERVATION_STATES
                                        .iter()
                                        .map(|state| (*state).to_owned())
                                        .collect(),
                                ),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .order_by(AiBudgetReservationRecordOrderByInput {
                            created_at: Some(OrderDirection::Asc),
                        })
                        .limit(scan_limit)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)?;
                    if reservations
                        .iter()
                        .any(|record| record.tenant_id != query_scope.tenant_id)
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::InternalError));
                    }

                    let mut run_reclamation_evidence = Vec::with_capacity(reservations.len());
                    for reservation in &reservations {
                        let run = tx
                            .find_by_id::<AiRunRecord>(&reservation.run_id)
                            .await
                            .map_err(OrmPublicError::from)?;
                        run_reclamation_evidence.push(run.map_or((false, false), |run| {
                            (
                                run.session_id == reservation.session_id
                                    && TERMINAL_RUN_STATES.contains(&run.state.as_str()),
                                run.session_id == reservation.session_id
                                    && run.lease_owner.is_none()
                                    && run.lease_expires_at.is_none(),
                            )
                        }));
                    }
                    Ok((policies, counters, reservations, run_reclamation_evidence))
                })
            })
            .await
            .map_err(map_transaction)?;

        let truncated = if has_lookahead {
            reservations.len() > window
        } else {
            reservations.len() >= window
        };
        let policy_views = policies
            .iter()
            .zip(counters.iter())
            .map(|(policy, (period, counter))| {
                budget_policy_capacity_view(policy, period, counter.as_ref())
            })
            .collect::<Vec<_>>();
        let mut uncertain_reservation_count = 0_i64;
        let mut reserved_reservation_count = 0_i64;
        let mut expired_reservation_count = 0_i64;
        let mut reclaimable_reservation_count = 0_i64;
        let mut reservation_views = Vec::with_capacity(reservations.len().min(window));
        for (record, (run_terminal, run_lease_free)) in reservations
            .iter()
            .take(window)
            .zip(run_reclamation_evidence)
        {
            let view = budget_reservation_capacity_view(
                record,
                run_terminal,
                run_lease_free,
                now,
                reclamation,
            );
            if record.state == "uncertain" {
                uncertain_reservation_count = uncertain_reservation_count.saturating_add(1);
            } else {
                reserved_reservation_count = reserved_reservation_count.saturating_add(1);
            }
            if view.expired {
                expired_reservation_count = expired_reservation_count.saturating_add(1);
            }
            if view.reclaimable {
                reclaimable_reservation_count = reclaimable_reservation_count.saturating_add(1);
            }
            reservation_views.push(view);
        }
        Ok(AiBudgetScopeCapacityView {
            policies: policy_views,
            uncertain_reservation_count,
            reserved_reservation_count,
            expired_reservation_count,
            reclaimable_reservation_count,
            reservations: reservation_views,
            truncated,
        })
    }

    async fn reclaim_budget_reservation(
        &self,
        principal: &AuthPrincipal,
        input: ReclaimAiBudgetReservationInput,
    ) -> Result<AiBudgetReservationCapacityView, AiError> {
        self.require_recent_mfa(principal)?;
        let scope: AiScope = input.scope.clone().into();
        self.require_access(
            principal,
            &scope,
            AiConfigurationAction::ManageBudgetReclamation,
        )
        .await?;
        let limits = self.budget_reclamation_limits.ok_or_else(|| {
            AiError::InvalidConfiguration(
                "budget reservation reclamation is not enabled".to_owned(),
            )
        })?;
        if input.expected_version < 0 {
            return Err(AiError::InvalidInput(
                "invalid budget reservation version".to_owned(),
            ));
        }
        let now = self.clock.now();
        let reclaimable_before = now
            .checked_sub(limits.minimum_expired_age())
            .ok_or_else(|| AiError::InvalidConfiguration("budget time overflow".to_owned()))?
            .unix_timestamp();
        let actor_kind = principal_kind(principal);
        let actor_subject = principal.subject().to_owned();
        let reservation_id = input.reservation_id;
        let expected_version = input.expected_version;
        let updated = self
            .database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let record = tx
                        .find_by_id::<AiBudgetReservationRecord>(&reservation_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    if record.scope_kind != scope.kind
                        || record.scope_id != scope.id
                        || record.tenant_id != scope.tenant_id
                    {
                        return Err(OrmPublicError::not_found());
                    }
                    if record.row_version != expected_version
                        || !UNRESOLVED_RESERVATION_STATES.contains(&record.state.as_str())
                        || record.expires_at > reclaimable_before
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    // Authoritative evidence: the owning run reached a durable
                    // terminal state and holds no lease, so no worker can ever
                    // reconcile this reservation from its own transport
                    // knowledge.
                    let run = tx
                        .find_by_id::<AiRunRecord>(&record.run_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                    if run.session_id != record.session_id
                        || !TERMINAL_RUN_STATES.contains(&run.state.as_str())
                        || run.lease_owner.is_some()
                        || run.lease_expires_at.is_some()
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    let previous_state = record.state.clone();
                    let updated =
                        crate::orm_budget::commit_stranded_reservation(tx, &record, now).await?;
                    insert_audit(
                        tx,
                        AuditFact {
                            actor_principal_kind: &actor_kind,
                            actor_subject: &actor_subject,
                            action: "ai.budget_reservation.reclaim",
                            resource_kind: "budget_reservation",
                            resource_reference: &updated.id.to_string(),
                            outcome: "allowed",
                            reason_code: if previous_state == "uncertain" {
                                "expired_uncertain_reservation_committed"
                            } else {
                                "expired_reserved_reservation_committed"
                            },
                            policy_version: Some(updated.row_version.to_string()),
                        },
                    )
                    .await?;
                    Ok(updated)
                })
            })
            .await
            .map_err(map_transaction)?;
        Ok(budget_reclaimed_reservation_view(&updated))
    }
}

#[async_trait]
impl AiContentProtectionPolicyResolver for OrmAiConfigurationService {
    async fn resolve(
        &self,
        principal: &AuthPrincipal,
        scope: &AiScope,
    ) -> Result<AiContentProtectionPolicy, AiError> {
        self.require_access(
            principal,
            scope,
            AiConfigurationAction::ReadContentProtection,
        )
        .await?;
        let record = load_content_policy(&self.database, scope)
            .await?
            .ok_or(AiError::RuntimeNotReady)?;
        let mode = parse_protection_mode(&record.protection_mode)?;
        Ok(AiContentProtectionPolicy {
            scope: scope.clone(),
            mode,
            key_policy_reference: record.key_policy_reference,
            version: u64::try_from(record.row_version).map_err(|_| AiError::PersistenceFailed)?,
            ready: record.ready && record.migration_state == "ready",
        })
    }
}

async fn load_content_policy(
    database: &Database<DefaultWriteBackend>,
    scope: &AiScope,
) -> Result<Option<AiContentProtectionPolicyRecord>, AiError> {
    let scope_hash = scope_key(scope);
    let rows = database
        .transaction(TransactionMode::Default, move |tx| {
            Box::pin(async move {
                tx.query::<AiContentProtectionPolicyRecord>()
                    .filter(AiContentProtectionPolicyRecordWhereInput {
                        scope_key: Some(StringFilter {
                            eq: Some(scope_hash),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .limit(2)
                    .fetch_all()
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await
        .map_err(map_transaction)?;
    if rows.len() > 1 {
        return Err(AiError::InvalidConfiguration(
            "multiple content-protection policies exist for one scope".to_owned(),
        ));
    }
    Ok(rows.into_iter().next())
}

async fn load_retention_policy(
    database: &Database<DefaultWriteBackend>,
    scope: &AiScope,
) -> Result<Option<AiRetentionPolicyRecord>, AiError> {
    let scope_hash = scope_key(scope);
    let rows = database
        .transaction(TransactionMode::Default, move |tx| {
            Box::pin(async move {
                tx.query::<AiRetentionPolicyRecord>()
                    .filter(AiRetentionPolicyRecordWhereInput {
                        scope_key: Some(StringFilter {
                            eq: Some(scope_hash),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .limit(2)
                    .fetch_all()
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await
        .map_err(map_transaction)?;
    if rows.len() > 1 {
        return Err(AiError::InvalidConfiguration(
            "multiple retention policies exist for one scope".to_owned(),
        ));
    }
    let record = rows.into_iter().next();
    if record.as_ref().is_some_and(|record| {
        record.inbox_event_retention_seconds.is_none() || record.inbox_minimum_events.is_none()
    }) {
        return Err(AiError::RuntimeNotReady);
    }
    Ok(record)
}

struct AuditFact<'a> {
    actor_principal_kind: &'a str,
    actor_subject: &'a str,
    action: &'a str,
    resource_kind: &'a str,
    resource_reference: &'a str,
    outcome: &'a str,
    reason_code: &'a str,
    policy_version: Option<String>,
}

async fn insert_audit(
    tx: &mut graphql_orm::graphql::orm::MutationContext<'_, DefaultWriteBackend>,
    fact: AuditFact<'_>,
) -> Result<(), OrmPublicError> {
    tx.insert::<AiAuditEventRecord>(CreateAiAuditEventRecordInput {
        actor_principal_kind: fact.actor_principal_kind.to_owned(),
        actor_subject: fact.actor_subject.to_owned(),
        action: fact.action.to_owned(),
        resource_kind: fact.resource_kind.to_owned(),
        resource_reference: fact.resource_reference.to_owned(),
        outcome: fact.outcome.to_owned(),
        reason_code: fact.reason_code.to_owned(),
        correlation_id: Uuid::new_v4().to_string(),
        causation_id: None,
        policy_version: fact.policy_version,
    })
    .await
    .map_err(OrmPublicError::from)?;
    Ok(())
}

const OPENAI_COMPATIBLE_DATA_POLICY_VERSION: u32 = 1;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredProviderDataPolicy {
    openai_compatible: StoredOpenAiCompatiblePolicy,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredOpenAiCompatiblePolicy {
    version: u32,
    retention: String,
    custom_tools: bool,
    parallel_tool_calls: bool,
    structured_output: bool,
    provider_retained_continuation: bool,
}

fn provider_data_policy(
    provider_kind: AiProviderKindInput,
    compatible: Option<AiOpenAiCompatibleProfileInput>,
) -> Result<serde_json::Value, AiError> {
    match (provider_kind, compatible) {
        (AiProviderKindInput::OpenAiCompatible, Some(profile)) => {
            validate_compatible_profile(
                &profile.retention,
                profile.custom_tools,
                profile.parallel_tool_calls,
            )
            .map_err(AiError::InvalidInput)?;
            serde_json::to_value(StoredProviderDataPolicy {
                openai_compatible: StoredOpenAiCompatiblePolicy {
                    version: OPENAI_COMPATIBLE_DATA_POLICY_VERSION,
                    retention: profile.retention,
                    custom_tools: profile.custom_tools,
                    parallel_tool_calls: profile.parallel_tool_calls,
                    structured_output: profile.structured_output,
                    provider_retained_continuation: profile.provider_retained_continuation,
                },
            })
            .map_err(|_| AiError::PersistenceFailed)
        }
        (AiProviderKindInput::OpenAiCompatible, None) => Err(AiError::InvalidInput(
            "OpenAI-compatible profiles require a reviewed capability and retention contract"
                .to_owned(),
        )),
        (_, Some(_)) => Err(AiError::InvalidInput(
            "OpenAI-compatible configuration is invalid for this provider kind".to_owned(),
        )),
        (_, None) => Ok(json!({})),
    }
}

fn provider_view(record: &AiProviderProfileRecord) -> Result<AiProviderProfileView, AiError> {
    let openai_compatible =
        if record.provider_kind == AiProviderKindInput::OpenAiCompatible.as_str() {
            if record
                .data_policy
                .as_object()
                .is_some_and(serde_json::Map::is_empty)
            {
                // Profiles created before the typed contract was introduced remain
                // visible but cannot construct the compatible transport adapter.
                None
            } else {
                let stored: StoredProviderDataPolicy =
                    serde_json::from_value(record.data_policy.clone()).map_err(|_| {
                        AiError::InvalidConfiguration(
                            "invalid OpenAI-compatible provider data policy".to_owned(),
                        )
                    })?;
                if stored.openai_compatible.version != OPENAI_COMPATIBLE_DATA_POLICY_VERSION {
                    return Err(AiError::InvalidConfiguration(
                        "unsupported OpenAI-compatible provider data-policy version".to_owned(),
                    ));
                }
                validate_compatible_profile(
                    &stored.openai_compatible.retention,
                    stored.openai_compatible.custom_tools,
                    stored.openai_compatible.parallel_tool_calls,
                )
                .map_err(AiError::InvalidConfiguration)?;
                Some(AiOpenAiCompatibleProfileView {
                    retention: stored.openai_compatible.retention,
                    custom_tools: stored.openai_compatible.custom_tools,
                    parallel_tool_calls: stored.openai_compatible.parallel_tool_calls,
                    structured_output: stored.openai_compatible.structured_output,
                    provider_retained_continuation: stored
                        .openai_compatible
                        .provider_retained_continuation,
                })
            }
        } else {
            if !record
                .data_policy
                .as_object()
                .is_some_and(serde_json::Map::is_empty)
            {
                return Err(AiError::InvalidConfiguration(
                    "unexpected provider data policy".to_owned(),
                ));
            }
            None
        };
    Ok(AiProviderProfileView {
        id: record.id,
        scope_kind: record.scope_kind.clone(),
        scope_id: record.scope_id.clone(),
        tenant_id: record.tenant_id.clone(),
        provider_kind: record.provider_kind.clone(),
        display_name: record.display_name.clone(),
        base_url: record.base_url.clone(),
        openai_compatible,
        credential_configured: record.credential_reference.is_some(),
        enabled: record.enabled,
        row_version: record.row_version,
        updated_at: record.updated_at,
    })
}

fn validate_compatible_profile(
    retention: &str,
    custom_tools: bool,
    parallel_tool_calls: bool,
) -> Result<(), String> {
    if retention.is_empty()
        || retention.len() > 200
        || !retention
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err("invalid OpenAI-compatible retention label".to_owned());
    }
    if parallel_tool_calls && !custom_tools {
        return Err(
            "parallel OpenAI-compatible tool calls require custom-tool capability".to_owned(),
        );
    }
    Ok(())
}

fn content_policy_view(record: &AiContentProtectionPolicyRecord) -> AiContentProtectionPolicyView {
    AiContentProtectionPolicyView {
        scope_kind: record.scope_kind.clone(),
        scope_id: record.scope_id.clone(),
        tenant_id: record.tenant_id.clone(),
        protection_mode: record.protection_mode.clone(),
        ready: record.ready && record.migration_state == "ready",
        row_version: record.row_version,
        effective_at: record.effective_at,
    }
}

fn retention_policy_view(record: &AiRetentionPolicyRecord) -> AiRetentionPolicyView {
    AiRetentionPolicyView {
        scope_kind: record.scope_kind.clone(),
        scope_id: record.scope_id.clone(),
        tenant_id: record.tenant_id.clone(),
        message_retention_seconds: record.message_retention_seconds,
        delta_retention_seconds: record.delta_retention_seconds,
        raw_payload_retention_seconds: record.raw_payload_retention_seconds,
        audit_retention_seconds: record.audit_retention_seconds,
        deleted_content_purge_seconds: record.deleted_content_purge_seconds,
        provider_file_delete_required: record.provider_file_delete_required,
        inbox_event_retention_seconds: record.inbox_event_retention_seconds.unwrap_or_default(),
        inbox_minimum_events: record.inbox_minimum_events.unwrap_or_default(),
        row_version: record.row_version,
        updated_at: record.updated_at,
    }
}

fn budget_policy_view(record: &AiBudgetPolicyRecord) -> AiBudgetPolicyView {
    AiBudgetPolicyView {
        id: record.id,
        scope_kind: record.scope_kind.clone(),
        scope_id: record.scope_id.clone(),
        tenant_id: record.tenant_id.clone(),
        principal_kind: record.principal_kind.clone(),
        principal_subject: record.principal_subject.clone(),
        interval_kind: record.interval_kind.clone(),
        maximum_input_tokens: record.maximum_input_tokens,
        maximum_output_tokens: record.maximum_output_tokens,
        maximum_tool_units: record.maximum_tool_units,
        maximum_image_units: record.maximum_image_units,
        maximum_cost_microunits: record.maximum_cost_microunits,
        maximum_runs: record.maximum_runs,
        enabled: record.enabled,
        row_version: record.row_version,
        updated_at: record.updated_at,
    }
}

fn budget_policy_capacity_view(
    policy: &AiBudgetPolicyRecord,
    period: &BudgetPeriod,
    counter: Option<&AiBudgetCounterRecord>,
) -> AiBudgetPolicyCapacityView {
    AiBudgetPolicyCapacityView {
        policy_id: policy.id,
        interval_kind: policy.interval_kind.clone(),
        enabled: policy.enabled,
        period_key: counter.map(|_| period.key.clone()),
        period_started_at: counter.map(|_| period.started_at),
        period_ends_at: counter.map(|_| period.ends_at),
        reserved_input_tokens: counter.map_or(0, |row| row.reserved_input_tokens),
        reserved_output_tokens: counter.map_or(0, |row| row.reserved_output_tokens),
        reserved_tool_units: counter.map_or(0, |row| row.reserved_tool_units),
        reserved_image_units: counter.map_or(0, |row| row.reserved_image_units),
        reserved_cost_microunits: counter.map_or(0, |row| row.reserved_cost_microunits),
        reserved_runs: counter.map_or(0, |row| row.reserved_runs),
        committed_input_tokens: counter.map_or(0, |row| row.committed_input_tokens),
        committed_output_tokens: counter.map_or(0, |row| row.committed_output_tokens),
        committed_tool_units: counter.map_or(0, |row| row.committed_tool_units),
        committed_image_units: counter.map_or(0, |row| row.committed_image_units),
        committed_cost_microunits: counter.map_or(0, |row| row.committed_cost_microunits),
        committed_runs: counter.map_or(0, |row| row.committed_runs),
        maximum_input_tokens: policy.maximum_input_tokens,
        maximum_output_tokens: policy.maximum_output_tokens,
        maximum_tool_units: policy.maximum_tool_units,
        maximum_image_units: policy.maximum_image_units,
        maximum_cost_microunits: policy.maximum_cost_microunits,
        maximum_runs: policy.maximum_runs,
    }
}

fn budget_reservation_capacity_view(
    record: &AiBudgetReservationRecord,
    run_terminal: bool,
    run_lease_free: bool,
    now: time::OffsetDateTime,
    reclamation: Option<AiBudgetReclamationLimits>,
) -> AiBudgetReservationCapacityView {
    let expired = record.expires_at <= now.unix_timestamp();
    let reclaimable = run_terminal
        && run_lease_free
        && reclamation.is_some_and(|limits| {
            now.checked_sub(limits.minimum_expired_age())
                .is_some_and(|threshold| record.expires_at <= threshold.unix_timestamp())
        });
    AiBudgetReservationCapacityView {
        id: record.id,
        run_id: record.run_id,
        state: record.state.clone(),
        expires_at: record.expires_at,
        created_at: record.created_at,
        expired,
        run_terminal,
        reclaimable,
        reserved_input_tokens: record.reserved_input_tokens,
        reserved_output_tokens: record.reserved_output_tokens,
        reserved_tool_units: record.reserved_tool_units,
        reserved_image_units: record.reserved_image_units,
        reserved_cost_microunits: record.reserved_cost_microunits,
        reserved_runs: record.reserved_runs,
        row_version: record.row_version,
    }
}

fn budget_reclaimed_reservation_view(
    record: &AiBudgetReservationRecord,
) -> AiBudgetReservationCapacityView {
    AiBudgetReservationCapacityView {
        id: record.id,
        run_id: record.run_id,
        state: record.state.clone(),
        expires_at: record.expires_at,
        created_at: record.created_at,
        expired: true,
        run_terminal: true,
        reclaimable: false,
        reserved_input_tokens: record.reserved_input_tokens,
        reserved_output_tokens: record.reserved_output_tokens,
        reserved_tool_units: record.reserved_tool_units,
        reserved_image_units: record.reserved_image_units,
        reserved_cost_microunits: record.reserved_cost_microunits,
        reserved_runs: record.reserved_runs,
        row_version: record.row_version,
    }
}

fn exact_scope_key_for_record(record: &AiBudgetPolicyRecord) -> String {
    scope_key(&AiScope {
        kind: record.scope_kind.clone(),
        id: record.scope_id.clone(),
        tenant_id: record.tenant_id.clone(),
    })
}

fn profile_scope(record: &AiProviderProfileRecord) -> AiScope {
    AiScope {
        kind: record.scope_kind.clone(),
        id: record.scope_id.clone(),
        tenant_id: record.tenant_id.clone(),
    }
}

pub(crate) fn scope_key(scope: &AiScope) -> String {
    crate::ai_scope_key(scope)
}

fn validate_retention_input(input: &SetAiRetentionPolicyInput) -> Result<(), AiError> {
    const MINIMUM_RETENTION_SECONDS: i64 = 60;
    const MAXIMUM_RETENTION_SECONDS: i64 = 315_576_000;
    let durations = [
        Some(input.delta_retention_seconds),
        Some(input.raw_payload_retention_seconds),
        Some(input.audit_retention_seconds),
        Some(input.deleted_content_purge_seconds),
        Some(input.inbox_event_retention_seconds),
        input.message_retention_seconds,
    ];
    if durations
        .into_iter()
        .flatten()
        .any(|seconds| !(MINIMUM_RETENTION_SECONDS..=MAXIMUM_RETENTION_SECONDS).contains(&seconds))
        || !(1..=100_000).contains(&input.inbox_minimum_events)
    {
        return Err(AiError::InvalidInput(
            "invalid AI retention policy bounds".to_owned(),
        ));
    }
    Ok(())
}

fn normalize_and_validate_budget_policy_input(
    input: &mut UpsertAiBudgetPolicyInput,
    maximum: AiBudgetAmounts,
) -> Result<(), AiError> {
    if input.id.is_some_and(|id| id.is_nil())
        || input.expected_version.is_some_and(|version| version < 0)
        || input.id.is_some() != input.expected_version.is_some()
        || input.principal_kind.is_some() != input.principal_subject.is_some()
    {
        return Err(AiError::InvalidInput(
            "invalid AI budget policy identity".to_owned(),
        ));
    }
    if let (Some(kind), Some(subject)) = (
        input.principal_kind.as_mut(),
        input.principal_subject.as_mut(),
    ) {
        *kind = kind.trim().to_owned();
        *subject = subject.trim().to_owned();
        let api_token_kind = kind.strip_prefix("api_token:");
        let valid_kind = kind == "user"
            || api_token_kind.is_some_and(|value| {
                !value.is_empty()
                    && value.len() <= 64
                    && value.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'_' | b'-')
                    })
            });
        if !valid_kind
            || subject.is_empty()
            || subject.len() > 512
            || subject.chars().any(char::is_control)
        {
            return Err(AiError::InvalidInput(
                "invalid AI budget policy principal".to_owned(),
            ));
        }
    }
    let configured = [
        (input.maximum_input_tokens, maximum.input_tokens),
        (input.maximum_output_tokens, maximum.output_tokens),
        (input.maximum_tool_units, maximum.tool_units),
        (input.maximum_image_units, maximum.image_units),
        (input.maximum_cost_microunits, maximum.cost_microunits),
        (input.maximum_runs, maximum.runs),
    ];
    if !configured.iter().any(|(value, _)| value.is_some())
        || configured.into_iter().any(|(value, hard_maximum)| {
            value.is_some_and(|value| {
                u64::try_from(value).map_or(true, |value| value > hard_maximum)
            })
        })
    {
        return Err(AiError::InvalidInput(
            "invalid AI budget policy ceilings".to_owned(),
        ));
    }
    Ok(())
}

fn normalize_endpoint(
    kind: AiProviderKindInput,
    base_url: Option<String>,
    endpoint_policy: &dyn AiProviderEndpointPolicy,
) -> Result<Option<String>, AiError> {
    let configurable = matches!(
        kind,
        AiProviderKindInput::Ollama | AiProviderKindInput::OpenAiCompatible
    );
    if !configurable {
        return if base_url.is_none() {
            Ok(None)
        } else {
            Err(AiError::InvalidInput(
                "native provider endpoints are deployment-fixed".to_owned(),
            ))
        };
    }
    let raw = base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AiError::InvalidInput("provider endpoint is required".to_owned()))?;
    let mut url = Url::parse(raw)
        .map_err(|_| AiError::InvalidInput("invalid provider endpoint".to_owned()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AiError::InvalidInput("unsafe provider endpoint".to_owned()));
    }
    url.set_query(None);
    url.set_fragment(None);
    let normalized = url.to_string();
    if !endpoint_policy.authorize_endpoint(kind, &normalized) {
        return Err(AiError::Forbidden);
    }
    Ok(Some(normalized))
}

fn validate_scope(scope: &AiScope) -> Result<(), AiError> {
    if scope.kind.trim().is_empty()
        || scope.id.trim().is_empty()
        || scope.kind.len() > 128
        || scope.id.len() > 512
        || scope
            .tenant_id
            .as_ref()
            .is_some_and(|tenant| tenant.trim().is_empty() || tenant.len() > 512)
    {
        return Err(AiError::InvalidInput("invalid AI scope".to_owned()));
    }
    Ok(())
}

fn principal_kind(principal: &AuthPrincipal) -> String {
    match principal {
        AuthPrincipal::User(_) => "user".to_owned(),
        AuthPrincipal::ApiToken(token) => {
            format!("api_token:{}", token.principal_kind.as_str())
        }
    }
}

fn protection_mode_value(mode: AiContentProtectionMode) -> &'static str {
    match mode {
        AiContentProtectionMode::DatabaseManaged => "database_managed",
        AiContentProtectionMode::ApplicationEncrypted => "application_encrypted",
    }
}

fn parse_protection_mode(value: &str) -> Result<AiContentProtectionMode, AiError> {
    match value {
        "database_managed" => Ok(AiContentProtectionMode::DatabaseManaged),
        "application_encrypted" => Ok(AiContentProtectionMode::ApplicationEncrypted),
        _ => Err(AiError::InvalidConfiguration(
            "unknown content-protection mode".to_owned(),
        )),
    }
}

fn unix_seconds() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

fn map_transaction(error: TransactionError) -> AiError {
    map_orm(error.public_error().clone())
}

fn map_orm(error: OrmPublicError) -> AiError {
    match error.code {
        OrmErrorCode::InvalidInput
        | OrmErrorCode::CursorInvalid
        | OrmErrorCode::PageLimitExceeded => AiError::InvalidInput(error.message),
        OrmErrorCode::Unauthenticated | OrmErrorCode::Forbidden => AiError::Forbidden,
        OrmErrorCode::NotFound => AiError::NotFound,
        OrmErrorCode::Conflict | OrmErrorCode::ConstraintViolation => AiError::Conflict,
        OrmErrorCode::ServiceUnavailable
        | OrmErrorCode::InternalError
        | OrmErrorCode::AuthorizationMisconfigured => AiError::PersistenceFailed,
    }
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use agql_auth::{
        AccessTokenMetadata, AssuranceMatchMode, AuthUser, FixedClock, MfaAcceptance,
        ResolvedPrincipal, SessionAssurance, SessionContext,
    };
    use graphql_orm::graphql::orm::{ApplyOptions, OrmSchemaModule};
    use graphql_orm::prelude::{Database, SqliteBackend};
    use time::Duration;

    use super::*;
    use crate::orm_budget::{AiBudgetServiceLimits, OrmAiBudgetService};
    use crate::{
        AiBudgetReconciliation, AiBudgetReconciliationOutcome, AiBudgetReservationRequest,
        AiBudgetService, AiRunId, AiSessionId, ModelReasoningEffort, ProviderKind,
    };

    const TENANT: &str = "tenant-1";
    const SUBJECT: &str = "budget-admin";
    const RESERVED_INPUT_TOKENS: u64 = 100;

    struct DenyConfiguration;

    #[async_trait]
    impl AiConfigurationAccessPolicy for DenyConfiguration {
        async fn can_configure(
            &self,
            _principal: &AuthPrincipal,
            _scope: &AiScope,
            _action: AiConfigurationAction,
        ) -> bool {
            false
        }
    }

    struct AllowConfiguration;

    #[async_trait]
    impl AiConfigurationAccessPolicy for AllowConfiguration {
        async fn can_configure(
            &self,
            _principal: &AuthPrincipal,
            _scope: &AiScope,
            _action: AiConfigurationAction,
        ) -> bool {
            true
        }
    }

    struct RejectEndpoints;

    impl AiProviderEndpointPolicy for RejectEndpoints {
        fn authorize_endpoint(
            &self,
            _provider_kind: AiProviderKindInput,
            _normalized_url: &str,
        ) -> bool {
            false
        }
    }

    #[derive(Default)]
    struct UnusedSecretStore;

    #[async_trait]
    impl AiSecretStore for UnusedSecretStore {
        async fn resolve(
            &self,
            _reference: &SecretRef,
        ) -> Result<SecretString, crate::SecretError> {
            Err(crate::SecretError::Unavailable)
        }

        async fn put(
            &self,
            _reference: Option<&SecretRef>,
            _value: SecretString,
        ) -> Result<SecretRef, crate::SecretError> {
            Err(crate::SecretError::Unavailable)
        }

        async fn delete(&self, _reference: &SecretRef) -> Result<(), crate::SecretError> {
            Ok(())
        }
    }

    fn scope() -> AiScope {
        AiScope::new("tenant", TENANT).with_tenant_id(TENANT)
    }

    fn scope_input() -> crate::AiScopeInput {
        crate::AiScopeInput {
            kind: "tenant".to_owned(),
            id: TENANT.to_owned(),
            tenant_id: Some(TENANT.to_owned()),
        }
    }

    fn admin_principal(now: time::OffsetDateTime) -> AuthPrincipal {
        let assurance = SessionAssurance::new(
            now,
            ["otp", "pwd"],
            Some("urn:test:loa:2".to_owned()),
            Some("test".to_owned()),
            MfaAcceptance::Satisfied,
        )
        .expect("test assurance should validate");
        AuthPrincipal::User(AuthUser {
            user_id: SUBJECT.to_owned(),
            session_id: Uuid::new_v4(),
            roles: vec!["admin".to_owned()],
            scopes: vec![],
            session: SessionContext::default().with_assurance(assurance),
            token_claims: AccessTokenMetadata {
                auth_time: Some(now.unix_timestamp()),
                amr: Some(vec!["otp".to_owned(), "pwd".to_owned()]),
                acr: Some("urn:test:loa:2".to_owned()),
                tenant_id: Some(TENANT.to_owned()),
                ..AccessTokenMetadata::default()
            },
        })
    }

    fn stale_mfa_principal() -> AuthPrincipal {
        AuthPrincipal::User(AuthUser {
            user_id: SUBJECT.to_owned(),
            session_id: Uuid::new_v4(),
            roles: vec!["admin".to_owned()],
            scopes: vec![],
            session: SessionContext::default(),
            token_claims: AccessTokenMetadata {
                tenant_id: Some(TENANT.to_owned()),
                ..AccessTokenMetadata::default()
            },
        })
    }

    fn configuration_service(
        database: Database<SqliteBackend>,
        access_policy: Arc<dyn AiConfigurationAccessPolicy>,
        now: time::OffsetDateTime,
        reclamation: bool,
    ) -> OrmAiConfigurationService {
        let service = OrmAiConfigurationService::new(
            database,
            access_policy,
            Arc::new(RejectEndpoints),
            RecentMfaPolicy {
                maximum_age: Duration::minutes(5),
                clock_skew: Duration::seconds(30),
                allowed_amr: vec!["otp".to_owned()],
                allowed_acr: vec!["urn:test:loa:2".to_owned()],
                match_mode: AssuranceMatchMode::All,
            },
            Arc::new(FixedClock::new(now)),
            Arc::new(UnusedSecretStore),
        );
        if reclamation {
            service.with_budget_reservation_reclamation(
                AiBudgetReclamationLimits::new(Duration::hours(1), 200)
                    .expect("reclamation limits should validate"),
            )
        } else {
            service
        }
    }

    /// Seeds a policy, session, running run, and one `uncertain` reservation
    /// created through the real budget service and marked uncertain through the
    /// real transport-boundary reconciliation.
    async fn stranded_reservation_fixture() -> (Database<SqliteBackend>, time::OffsetDateTime, Uuid)
    {
        let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
            .await
            .expect("in-memory SQLite should open");
        let module = crate::AiSchemaModule;
        let plan = database
            .schema()
            .plan_migration_to_entities(
                "ai-budget-reclaim-test-v1",
                "AI budget reclamation test",
                module.entities(),
            )
            .await
            .expect("AI schema migration should plan");
        database
            .schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await
            .expect("AI schema migration should apply");

        let now = time::OffsetDateTime::from_unix_timestamp(1_800_000_000)
            .expect("fixed test timestamp should be valid");
        AiBudgetPolicyRecord::insert(
            &database,
            CreateAiBudgetPolicyRecordInput {
                scope_key: scope_key(&scope()),
                scope_kind: "tenant".to_owned(),
                scope_id: TENANT.to_owned(),
                tenant_id: Some(TENANT.to_owned()),
                principal_kind: None,
                principal_subject: None,
                interval_kind: "month".to_owned(),
                maximum_input_tokens: Some(1_000),
                maximum_output_tokens: Some(1_000),
                maximum_tool_units: Some(100),
                maximum_image_units: Some(100),
                maximum_cost_microunits: Some(10_000),
                maximum_runs: Some(100),
                enabled: true,
            },
        )
        .await
        .expect("budget policy should seed");

        let principal = admin_principal(now);
        let resolved = ResolvedPrincipal::new(principal.reference(), principal.clone(), now)
            .expect("fresh principal should resolve");
        let session_id = AiSessionId::new();
        let run_id = AiRunId::new();
        let attempt_id = Uuid::new_v4();
        AiSessionRecord::insert(
            &database,
            CreateAiSessionRecordInput {
                id: session_id.0,
                owner_principal_kind: "user".to_owned(),
                owner_subject: SUBJECT.to_owned(),
                tenant_id: Some(TENANT.to_owned()),
                scope_kind: "tenant".to_owned(),
                scope_id: TENANT.to_owned(),
                title: "Budget reclamation".to_owned(),
                title_revision: 0,
                title_source: "default".to_owned(),
                state: "active".to_owned(),
                stream_head: 0,
                message_head: 0,
                last_activity_at: now.unix_timestamp(),
                archived_at: None,
                deleted_at: None,
            },
        )
        .await
        .expect("session should seed");
        AiRunRecord::insert(
            &database,
            CreateAiRunRecordInput {
                id: run_id.0,
                session_id: session_id.0,
                input_message_id: Uuid::new_v4(),
                principal_reference: serde_json::to_value(resolved.reference())
                    .expect("principal reference should serialize"),
                state: "running".to_owned(),
                attempt_id: Some(attempt_id),
                lease_owner: Some("worker-test".to_owned()),
                lease_generation: 1,
                lease_expires_at: Some((now + Duration::minutes(4)).unix_timestamp()),
                lease_heartbeat_at: Some(now.unix_timestamp()),
                retry_count: 0,
                next_attempt_at: None,
                error_code: None,
                latest_checkpoint_id: None,
                cancellation_request_id: None,
                cancellation_requested_at: None,
            },
        )
        .await
        .expect("running run should seed");

        let budget = OrmAiBudgetService::new(
            database.clone(),
            Arc::new(FixedClock::new(now)),
            AiBudgetServiceLimits::new(
                AiBudgetAmounts {
                    input_tokens: 1_000,
                    output_tokens: 1_000,
                    tool_units: 100,
                    image_units: 100,
                    cost_microunits: 10_000,
                    runs: 1,
                },
                Duration::minutes(5),
                Duration::seconds(30),
                16,
                8,
            )
            .expect("budget service limits should validate"),
        );
        let reservation = budget
            .reserve(
                &resolved,
                AiBudgetReservationRequest {
                    scope: scope(),
                    session_id,
                    run_id,
                    attempt_id,
                    lease_generation: 1,
                    provider_kind: ProviderKind::OpenAi,
                    model: "test-model".to_owned(),
                    reasoning_effort: ModelReasoningEffort::Unspecified,
                    pricing_policy_version: "pricing-test-v1".to_owned(),
                    estimate: AiBudgetAmounts {
                        input_tokens: RESERVED_INPUT_TOKENS,
                        output_tokens: 10,
                        tool_units: 0,
                        image_units: 0,
                        cost_microunits: 100,
                        runs: 1,
                    },
                    idempotency_key: "reclaim-test-1".to_owned(),
                    expires_at: now + Duration::minutes(2),
                },
            )
            .await
            .expect("reservation should be granted");
        budget
            .reconcile(
                &resolved,
                AiBudgetReconciliation {
                    reservation_id: reservation.id(),
                    attempt_id,
                    lease_generation: 1,
                    actual: None,
                    cached_input_tokens: None,
                    outcome: AiBudgetReconciliationOutcome::MarkUncertain,
                },
            )
            .await
            .expect("transport boundary should mark the reservation uncertain");
        (database, now, reservation.id().0)
    }

    async fn terminate_run(database: &Database<SqliteBackend>) {
        let run = database
            .transaction(TransactionMode::Default, |tx| {
                Box::pin(async move {
                    tx.query::<AiRunRecord>()
                        .limit(2)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .await
            .expect("run query should succeed")
            .into_iter()
            .next()
            .expect("one run was seeded");
        AiRunRecord::update_by_id(
            database,
            &run.id,
            UpdateAiRunRecordInput {
                state: Some("recovery_required".to_owned()),
                attempt_id: Some(None),
                lease_owner: Some(None),
                lease_expires_at: Some(None),
                lease_heartbeat_at: Some(None),
                error_code: Some(Some("provider_turn_uncertain".to_owned())),
                ..Default::default()
            },
        )
        .await
        .expect("run should reach a terminal state");
    }

    async fn make_run_terminal_without_releasing_lease(database: &Database<SqliteBackend>) {
        let run = database
            .transaction(TransactionMode::Default, |tx| {
                Box::pin(async move {
                    tx.query::<AiRunRecord>()
                        .limit(2)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .await
            .expect("run query should succeed")
            .into_iter()
            .next()
            .expect("one run was seeded");
        AiRunRecord::update_by_id(
            database,
            &run.id,
            UpdateAiRunRecordInput {
                state: Some("recovery_required".to_owned()),
                error_code: Some(Some("provider_turn_uncertain".to_owned())),
                ..Default::default()
            },
        )
        .await
        .expect("test should create inconsistent terminal lease evidence");
    }

    async fn expire_reservation(
        database: &Database<SqliteBackend>,
        reservation_id: Uuid,
        expires_at: i64,
    ) {
        AiBudgetReservationRecord::update_by_id(
            database,
            &reservation_id,
            UpdateAiBudgetReservationRecordInput {
                expires_at: Some(expires_at),
                ..Default::default()
            },
        )
        .await
        .expect("reservation expiry should rewind");
    }

    async fn usage_entries(database: &Database<SqliteBackend>) -> Vec<AiUsageEntryRecord> {
        database
            .transaction(TransactionMode::Default, |tx| {
                Box::pin(async move {
                    tx.query::<AiUsageEntryRecord>()
                        .limit(10)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .await
            .expect("usage query should succeed")
    }

    async fn audit_actions(database: &Database<SqliteBackend>) -> Vec<String> {
        database
            .transaction(TransactionMode::Default, |tx| {
                Box::pin(async move {
                    tx.query::<AiAuditEventRecord>()
                        .limit(20)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .await
            .expect("audit query should succeed")
            .into_iter()
            .map(|record| record.action)
            .collect()
    }

    async fn seed_other_tenant_reservation(database: &Database<SqliteBackend>) -> Uuid {
        let original = database
            .transaction(TransactionMode::Default, |tx| {
                Box::pin(async move {
                    tx.query::<AiBudgetReservationRecord>()
                        .limit(2)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .await
            .expect("reservation query should succeed")
            .into_iter()
            .next()
            .expect("one reservation was seeded");
        let inserted = AiBudgetReservationRecord::insert(
            database,
            CreateAiBudgetReservationRecordInput {
                budget_counter_ids: original.budget_counter_ids,
                scope_kind: original.scope_kind,
                scope_id: original.scope_id,
                tenant_id: Some("other-tenant".to_owned()),
                principal_kind: original.principal_kind,
                principal_subject: "other-user".to_owned(),
                session_id: original.session_id,
                run_id: original.run_id,
                attempt_id: original.attempt_id,
                lease_generation: original.lease_generation,
                provider_kind: original.provider_kind,
                provider_model: original.provider_model,
                reasoning_effort: original.reasoning_effort,
                pricing_policy_version: original.pricing_policy_version,
                reserved_input_tokens: original.reserved_input_tokens,
                reserved_output_tokens: original.reserved_output_tokens,
                reserved_tool_units: original.reserved_tool_units,
                reserved_image_units: original.reserved_image_units,
                reserved_cost_microunits: original.reserved_cost_microunits,
                reserved_runs: original.reserved_runs,
                actual_input_tokens: original.actual_input_tokens,
                actual_cached_input_tokens: original.actual_cached_input_tokens,
                actual_output_tokens: original.actual_output_tokens,
                actual_tool_units: original.actual_tool_units,
                actual_image_units: original.actual_image_units,
                actual_cost_microunits: original.actual_cost_microunits,
                actual_runs: original.actual_runs,
                idempotency_key: "other-tenant-reservation".to_owned(),
                state: original.state,
                expires_at: original.expires_at,
                reconciled_at: original.reconciled_at,
            },
        )
        .await
        .expect("other-tenant reservation should seed");
        inserted.id
    }

    #[tokio::test]
    async fn expired_uncertain_reservation_is_reclaimable_and_reported() {
        let (database, now, reservation_id) = stranded_reservation_fixture().await;
        let service =
            configuration_service(database.clone(), Arc::new(AllowConfiguration), now, true);
        let principal = admin_principal(now);

        let before = service
            .budget_scope_capacity(&principal, scope())
            .await
            .expect("capacity reporting should succeed");
        assert_eq!(before.policies.len(), 1);
        assert_eq!(
            before.policies[0].reserved_input_tokens,
            RESERVED_INPUT_TOKENS as i64
        );
        assert_eq!(before.policies[0].committed_input_tokens, 0);
        assert_eq!(before.uncertain_reservation_count, 1);
        assert_eq!(before.reserved_reservation_count, 0);
        assert!(!before.truncated);
        assert_eq!(before.reservations.len(), 1);
        assert!(!before.reservations[0].expired);
        assert!(!before.reservations[0].run_terminal);
        assert!(!before.reservations[0].reclaimable);

        terminate_run(&database).await;
        expire_reservation(
            &database,
            reservation_id,
            (now - Duration::days(2)).unix_timestamp(),
        )
        .await;

        let stranded = service
            .budget_scope_capacity(&principal, scope())
            .await
            .expect("capacity reporting should succeed");
        assert_eq!(stranded.expired_reservation_count, 1);
        assert_eq!(stranded.reclaimable_reservation_count, 1);
        assert!(stranded.reservations[0].reclaimable);
        let expected_version = stranded.reservations[0].row_version;

        assert!(matches!(
            service
                .reclaim_budget_reservation(
                    &principal,
                    ReclaimAiBudgetReservationInput {
                        scope: scope_input(),
                        reservation_id,
                        expected_version: expected_version + 5,
                    },
                )
                .await,
            Err(AiError::Conflict)
        ));

        let reclaimed = service
            .reclaim_budget_reservation(
                &principal,
                ReclaimAiBudgetReservationInput {
                    scope: scope_input(),
                    reservation_id,
                    expected_version,
                },
            )
            .await
            .expect("an expired uncertain reservation on a terminal run reclaims");
        assert_eq!(reclaimed.state, "committed");
        assert_eq!(
            reclaimed.reserved_input_tokens,
            RESERVED_INPUT_TOKENS as i64
        );

        let after = service
            .budget_scope_capacity(&principal, scope())
            .await
            .expect("capacity reporting should succeed");
        assert_eq!(after.policies[0].reserved_input_tokens, 0);
        assert_eq!(
            after.policies[0].committed_input_tokens,
            RESERVED_INPUT_TOKENS as i64
        );
        assert_eq!(after.uncertain_reservation_count, 0);
        assert_eq!(after.reclaimable_reservation_count, 0);
        assert!(after.reservations.is_empty());

        let usage = usage_entries(&database).await;
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].budget_reservation_id, reservation_id);
        assert_eq!(usage[0].input_tokens, RESERVED_INPUT_TOKENS as i64);
        assert_eq!(usage[0].cached_input_tokens, 0);
        assert!(
            audit_actions(&database)
                .await
                .contains(&"ai.budget_reservation.reclaim".to_owned())
        );

        // Replay of the same exact-version request cannot double-count.
        assert!(matches!(
            service
                .reclaim_budget_reservation(
                    &principal,
                    ReclaimAiBudgetReservationInput {
                        scope: scope_input(),
                        reservation_id,
                        expected_version,
                    },
                )
                .await,
            Err(AiError::Conflict)
        ));
    }

    #[tokio::test]
    async fn unexpired_or_active_uncertain_reservations_are_not_reclaimable() {
        let (database, now, reservation_id) = stranded_reservation_fixture().await;
        let service =
            configuration_service(database.clone(), Arc::new(AllowConfiguration), now, true);
        let principal = admin_principal(now);
        let version = service
            .budget_scope_capacity(&principal, scope())
            .await
            .expect("capacity reporting should succeed")
            .reservations[0]
            .row_version;

        // Expired long ago, but the owning run is still running.
        expire_reservation(
            &database,
            reservation_id,
            (now - Duration::days(2)).unix_timestamp(),
        )
        .await;
        assert!(matches!(
            service
                .reclaim_budget_reservation(
                    &principal,
                    ReclaimAiBudgetReservationInput {
                        scope: scope_input(),
                        reservation_id,
                        expected_version: version,
                    },
                )
                .await,
            Err(AiError::Conflict)
        ));

        // A terminal state alone is insufficient while stale lease evidence
        // remains. Reporting and mutation must agree on the same fail-closed
        // reclaimability predicate.
        make_run_terminal_without_releasing_lease(&database).await;
        let terminal_but_leased = service
            .budget_scope_capacity(&principal, scope())
            .await
            .expect("capacity reporting should succeed");
        assert!(terminal_but_leased.reservations[0].run_terminal);
        assert!(!terminal_but_leased.reservations[0].reclaimable);
        assert!(matches!(
            service
                .reclaim_budget_reservation(
                    &principal,
                    ReclaimAiBudgetReservationInput {
                        scope: scope_input(),
                        reservation_id,
                        expected_version: terminal_but_leased.reservations[0].row_version,
                    },
                )
                .await,
            Err(AiError::Conflict)
        ));

        // Terminal run, but the expiry grace has not elapsed.
        terminate_run(&database).await;
        expire_reservation(
            &database,
            reservation_id,
            (now - Duration::minutes(1)).unix_timestamp(),
        )
        .await;
        let capacity = service
            .budget_scope_capacity(&principal, scope())
            .await
            .expect("capacity reporting should succeed");
        assert!(capacity.reservations[0].expired);
        assert!(capacity.reservations[0].run_terminal);
        assert!(!capacity.reservations[0].reclaimable);
        assert!(matches!(
            service
                .reclaim_budget_reservation(
                    &principal,
                    ReclaimAiBudgetReservationInput {
                        scope: scope_input(),
                        reservation_id,
                        expected_version: capacity.reservations[0].row_version,
                    },
                )
                .await,
            Err(AiError::Conflict)
        ));

        // Nothing moved between the reserved and committed columns.
        assert_eq!(
            capacity.policies[0].reserved_input_tokens,
            RESERVED_INPUT_TOKENS as i64
        );
        assert_eq!(capacity.policies[0].committed_input_tokens, 0);
        assert!(usage_entries(&database).await.is_empty());
    }

    #[tokio::test]
    async fn reclamation_requires_authorization_recent_mfa_and_deployment_opt_in() {
        let (database, now, reservation_id) = stranded_reservation_fixture().await;
        terminate_run(&database).await;
        expire_reservation(
            &database,
            reservation_id,
            (now - Duration::days(2)).unix_timestamp(),
        )
        .await;
        let principal = admin_principal(now);
        let input = || ReclaimAiBudgetReservationInput {
            scope: scope_input(),
            reservation_id,
            expected_version: 1,
        };

        let denied =
            configuration_service(database.clone(), Arc::new(DenyConfiguration), now, true);
        assert!(matches!(
            denied.reclaim_budget_reservation(&principal, input()).await,
            Err(AiError::Forbidden)
        ));
        assert!(matches!(
            denied.budget_scope_capacity(&principal, scope()).await,
            Err(AiError::Forbidden)
        ));

        let allowed =
            configuration_service(database.clone(), Arc::new(AllowConfiguration), now, true);
        assert!(matches!(
            allowed
                .reclaim_budget_reservation(&stale_mfa_principal(), input())
                .await,
            Err(AiError::RecentMfaRequired)
        ));

        let unconfigured =
            configuration_service(database.clone(), Arc::new(AllowConfiguration), now, false);
        assert!(matches!(
            unconfigured
                .reclaim_budget_reservation(&principal, input())
                .await,
            Err(AiError::InvalidConfiguration(_))
        ));
        assert!(
            !unconfigured
                .budget_scope_capacity(&principal, scope())
                .await
                .expect("capacity reporting works without the reclamation opt-in")
                .reservations[0]
                .reclaimable
        );

        // No refused path may move capacity or append usage.
        assert!(usage_entries(&database).await.is_empty());
    }

    #[tokio::test]
    async fn capacity_reporting_filters_the_exact_tenant_before_its_bound() {
        let (database, now, reservation_id) = stranded_reservation_fixture().await;
        let _other_reservation = seed_other_tenant_reservation(&database).await;
        let service = configuration_service(database, Arc::new(AllowConfiguration), now, true);

        let capacity = service
            .budget_scope_capacity(&admin_principal(now), scope())
            .await
            .expect("exact-tenant capacity reporting should succeed");

        assert_eq!(capacity.reservations.len(), 1);
        assert_eq!(capacity.reservations[0].id, reservation_id);
        assert!(!capacity.truncated);
    }

    #[tokio::test]
    async fn reclamation_rejects_a_cross_scope_counter_link() {
        let (database, now, _reservation_id) = stranded_reservation_fixture().await;
        let other_reservation_id = seed_other_tenant_reservation(&database).await;
        terminate_run(&database).await;
        expire_reservation(
            &database,
            other_reservation_id,
            (now - Duration::days(2)).unix_timestamp(),
        )
        .await;
        let service =
            configuration_service(database.clone(), Arc::new(AllowConfiguration), now, true);
        let other_scope = AiScope::new("tenant", TENANT).with_tenant_id("other-tenant");
        let candidate = service
            .budget_scope_capacity(&admin_principal(now), other_scope.clone())
            .await
            .expect("the malformed reservation remains observable")
            .reservations
            .into_iter()
            .next()
            .expect("the other-tenant reservation should be visible");
        assert!(candidate.reclaimable);

        assert!(matches!(
            service
                .reclaim_budget_reservation(
                    &admin_principal(now),
                    ReclaimAiBudgetReservationInput {
                        scope: crate::AiScopeInput {
                            kind: other_scope.kind,
                            id: other_scope.id,
                            tenant_id: other_scope.tenant_id,
                        },
                        reservation_id: other_reservation_id,
                        expected_version: candidate.row_version,
                    },
                )
                .await,
            Err(AiError::PersistenceFailed)
        ));
        assert!(usage_entries(&database).await.is_empty());
        let original_capacity = service
            .budget_scope_capacity(&admin_principal(now), scope())
            .await
            .expect("the original scope capacity remains readable");
        assert_eq!(
            original_capacity.policies[0].reserved_input_tokens,
            RESERVED_INPUT_TOKENS as i64
        );
        assert_eq!(original_capacity.policies[0].committed_input_tokens, 0);
    }
}
