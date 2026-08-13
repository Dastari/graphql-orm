//! ORM-backed protected provider-session binding lifecycle.

#![allow(missing_docs)]

use graphql_orm::prelude::*;

/// One protected, provider-neutral resume binding per AI session.
///
/// This private entity is intentionally not composed as GraphQL CRUD. The
/// cursor can be opened only through the fenced service below.
#[backend_selected_graphql_entity(
    table = "graphql_orm_ai_provider_session_bindings",
    plural = "GraphqlOrmAiProviderSessionBindings",
    default_sort = "updated_at ASC, id ASC",
    unique_index = "session_id",
    index(
        name = "idx_graphql_orm_ai_provider_sessions_cleanup",
        columns = ["state", "cleanup_next_attempt_at", "updated_at", "id"],
        directions = ["asc", "asc", "asc", "asc"]
    ),
    index(
        name = "idx_graphql_orm_ai_provider_sessions_expired_claim",
        columns = ["state", "claim_expires_at", "updated_at", "id"],
        directions = ["asc", "asc", "asc", "asc"]
    ),
    index(
        name = "idx_graphql_orm_ai_provider_sessions_parked_wait",
        columns = ["state", "parked_expires_at", "updated_at", "id"],
        directions = ["asc", "asc", "asc", "asc"]
    )
)]
#[cfg_attr(feature = "mssql", derive(GraphQLSchemaEntity))]
#[cfg_attr(
    any(feature = "sqlite", feature = "postgres"),
    derive(GraphQLEntity, GraphQLOperations)
)]
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct AiProviderSessionBindingRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,
    #[filterable(type = "uuid")]
    pub session_id: graphql_orm::uuid::Uuid,
    pub owner_principal_kind: String,
    pub owner_subject: String,
    #[graphql_orm(json, read = false, filter = false, order = false, subscribe = false)]
    pub principal_reference: serde_json::Value,
    #[filterable(type = "string")]
    pub scope_key: String,
    pub scope_kind: String,
    pub scope_id: String,
    pub tenant_id: Option<String>,
    pub provider_kind: String,
    pub provider_profile_id: String,
    pub provider_model: String,
    pub registration_fingerprint: String,
    pub protocol_version: String,
    pub policy_fingerprint: String,
    pub cursor_kind: String,
    pub cursor_fingerprint: String,
    /// The cursor is never exposed through generated reads and is redacted
    /// from portable backups. Raw restore must quarantine the binding rather
    /// than resume it.
    #[backup(redact)]
    #[graphql_orm(json, read = false, filter = false, order = false, subscribe = false)]
    pub protected_cursor: Option<serde_json::Value>,
    pub through_message_sequence: i64,
    pub transcript_fingerprint: String,
    pub last_run_id: Option<graphql_orm::uuid::Uuid>,
    pub last_assistant_message_id: Option<graphql_orm::uuid::Uuid>,
    #[filterable(type = "string")]
    pub state: String,
    pub claimed_run_id: Option<graphql_orm::uuid::Uuid>,
    pub claimed_attempt_id: Option<graphql_orm::uuid::Uuid>,
    pub claimed_run_lease_generation: Option<i64>,
    pub claim_owner: Option<String>,
    pub claim_generation: i64,
    #[filterable(type = "number")]
    pub claim_expires_at: Option<i64>,
    /// Closed approval/subscription wait kind while the retained cursor is
    /// parked. This is safe lifecycle metadata, never authority.
    pub parked_wait_kind: Option<String>,
    /// Exact durable approval or subscription-waiter identity.
    pub parked_wait_id: Option<graphql_orm::uuid::Uuid>,
    /// Monotonic parking generation, independent of run and claim fencing.
    pub park_generation: i64,
    /// Exact provider-turn checkpoint committed before parking.
    pub parked_source_checkpoint_id: Option<graphql_orm::uuid::Uuid>,
    /// Verified source checkpoint hash.
    pub parked_source_checkpoint_fingerprint: Option<String>,
    /// Latest durable wait checkpoint observed when parking was confirmed.
    pub parked_checkpoint_id: Option<graphql_orm::uuid::Uuid>,
    /// Verified hash of the confirmed durable wait checkpoint.
    pub parked_checkpoint_fingerprint: Option<String>,
    /// Fingerprint of the complete provider-retained continuation and source
    /// checkpoint; provider content is never stored here.
    pub parked_continuation_fingerprint: Option<String>,
    /// Time at which the exact run/wait/checkpoint graph was confirmed.
    pub parked_confirmed_at: Option<i64>,
    /// Bounded expiry inherited from the durable wait after confirmation.
    #[filterable(type = "number")]
    pub parked_expires_at: Option<i64>,
    /// One-shot reclaim timestamp retained as audit evidence.
    pub parked_reclaimed_at: Option<i64>,
    #[filterable(type = "number")]
    pub provider_expires_at: Option<i64>,
    #[filterable(type = "number")]
    pub idle_expires_at: i64,
    #[filterable(type = "number")]
    pub absolute_expires_at: i64,
    pub cleanup_owner: Option<String>,
    pub cleanup_generation: i64,
    #[filterable(type = "number")]
    pub cleanup_lease_expires_at: Option<i64>,
    pub cleanup_retry_count: i64,
    #[filterable(type = "number")]
    pub cleanup_next_attempt_at: Option<i64>,
    pub cleanup_reason_code: Option<String>,
    pub provider_absence_observed_at: Option<i64>,
    #[sortable]
    pub updated_at: i64,
    #[graphql_orm(version, default = "0")]
    pub row_version: i64,
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
mod service {
    use std::sync::Arc;

    use agql_auth::{AuthPrincipal, Clock, CurrentPrincipalResolver, PrincipalReference};
    use async_trait::async_trait;
    use graphql_orm::db::Database;
    use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
    use graphql_orm::graphql::filters::{IntFilter, StringFilter, UuidFilter};
    use graphql_orm::graphql::orm::{
        ConditionalUpdateOutcome, DefaultWriteBackend, MutationContext, TransactionMode,
    };
    use serde::{Deserialize, Serialize};
    use sha2::Digest;
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    use super::*;
    use crate::orm_runs::{canonical_second, load_and_validate_active_lease, validate_worker_id};
    use crate::orm_sessions::{
        content_context, map_orm, map_protection, map_transaction, principal_identity, record_scope,
    };
    use crate::persistence::{
        AiApprovalRecord, AiAuditEventRecord, AiMessageRecord, AiRunCheckpointRecord,
        AiRunCheckpointRecordWhereInput, AiRunRecord, AiSessionRecord,
        AiSubscriptionWaitAdoptionRecord, AiSubscriptionWaitAdoptionRecordWhereInput,
        AiSubscriptionWaiterRecord, AiToolCallRecord, CreateAiAuditEventRecordInput,
    };
    use crate::{
        AiAccessPolicy, AiContentProtectionPolicy, AiContentProtectionPolicyResolver,
        AiContentProtector, AiError, AiOpenedProviderSession, AiProviderSessionAbsenceProof,
        AiProviderSessionBindRequest, AiProviderSessionBindingView, AiProviderSessionClaim,
        AiProviderSessionCleanupClaim, AiProviderSessionCommit, AiProviderSessionCursor,
        AiProviderSessionDeletionRequest, AiProviderSessionDescriptor, AiProviderSessionLimits,
        AiProviderSessionParkedWait, AiProviderSessionRebindAuthorization,
        AiProviderSessionRunDisposition, AiProviderSessionService, AiProviderSessionState,
        AiProviderSessionTurnPlan, AiProviderSessionWaitIdentity, AiProviderSessionWaitKind,
        AiProviderSessionWaitParkRequest, AiRunId, AiRunLease, AiRunState, AiScope,
        AiSessionAction, AiSessionId, ProtectedContentEnvelope, ai_scope_key, parse_provider_kind,
        provider_kind_value, provider_session::MAXIMUM_PROVIDER_SESSION_REBIND_AUTHORIZATION_TTL,
    };

    const CURSOR_FORMAT_VERSION: u16 = 1;
    const MAXIMUM_SAFE_REASON_CODE_BYTES: usize = 200;

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields, rename_all = "camelCase")]
    struct ProtectedProviderSessionCursor {
        format_version: u16,
        cursor_kind: String,
        cursor: String,
        binding_hash: String,
    }

    /// Generated-ORM provider-session store with protected cursors and exact
    /// run/session fencing.
    pub struct OrmAiProviderSessionService {
        database: Database<DefaultWriteBackend>,
        access_policy: Arc<dyn AiAccessPolicy>,
        protection_policy: Arc<dyn AiContentProtectionPolicyResolver>,
        content_protector: Arc<dyn AiContentProtector>,
        principal_resolver: Arc<dyn CurrentPrincipalResolver>,
        clock: Arc<dyn Clock>,
        limits: AiProviderSessionLimits,
        maximum_principal_age: Duration,
    }

    impl OrmAiProviderSessionService {
        /// Creates a protected provider-session lifecycle.
        ///
        /// `maximum_principal_age` applies at every owner-authorized binding,
        /// claim, open, heartbeat, and commit boundary.
        ///
        /// # Errors
        ///
        /// Returns [`AiError::InvalidConfiguration`] unless the freshness
        /// bound is positive and at most one hour.
        #[allow(clippy::too_many_arguments)]
        pub fn new(
            database: Database<DefaultWriteBackend>,
            access_policy: Arc<dyn AiAccessPolicy>,
            protection_policy: Arc<dyn AiContentProtectionPolicyResolver>,
            content_protector: Arc<dyn AiContentProtector>,
            principal_resolver: Arc<dyn CurrentPrincipalResolver>,
            clock: Arc<dyn Clock>,
            limits: AiProviderSessionLimits,
            maximum_principal_age: Duration,
        ) -> Result<Self, AiError> {
            if !maximum_principal_age.is_positive() || maximum_principal_age > Duration::hours(1) {
                return Err(AiError::InvalidConfiguration(
                    "invalid provider-session principal freshness".to_owned(),
                ));
            }
            Ok(Self {
                database,
                access_policy,
                protection_policy,
                content_protector,
                principal_resolver,
                clock,
                limits,
                maximum_principal_age,
            })
        }

        /// Returns the underlying database for host-scheduled composition.
        pub const fn database(&self) -> &Database<DefaultWriteBackend> {
            &self.database
        }

        async fn resolve_current(
            &self,
            reference: &PrincipalReference,
        ) -> Result<agql_auth::ResolvedPrincipal, AiError> {
            let current = self
                .principal_resolver
                .resolve(reference)
                .await
                .map_err(|_| AiError::ReauthorizationFailed)?;
            let now = self.clock.now();
            if current.reference() != reference
                || current.resolved_at() > now
                || now - current.resolved_at() >= self.maximum_principal_age
                || reference
                    .expires_at
                    .is_some_and(|expires_at| expires_at <= now)
            {
                return Err(AiError::ReauthorizationFailed);
            }
            Ok(current)
        }

        async fn authorize_session(
            &self,
            principal: &AuthPrincipal,
            session: &AiSessionRecord,
        ) -> Result<AiScope, AiError> {
            let (kind, subject) = principal_identity(principal);
            if session.owner_principal_kind != kind
                || session.owner_subject != subject
                || session.state != "active"
                || session.deleted_at.is_some()
            {
                return Err(AiError::NotFound);
            }
            let scope = record_scope(session);
            if !self
                .access_policy
                .can_access_session(principal, AiSessionId(session.id), AiSessionAction::Write)
                .await
                .is_allowed()
                || !self
                    .access_policy
                    .can_access_scope(principal, &scope, AiSessionAction::Write)
                    .await
                    .is_allowed()
            {
                return Err(AiError::Forbidden);
            }
            Ok(scope)
        }

        async fn protection_policy(
            &self,
            principal: &AuthPrincipal,
            scope: &AiScope,
        ) -> Result<AiContentProtectionPolicy, AiError> {
            let policy = self.protection_policy.resolve(principal, scope).await?;
            if !policy.ready || policy.scope != *scope {
                return Err(AiError::RuntimeNotReady);
            }
            Ok(policy)
        }

        #[allow(clippy::too_many_arguments)]
        async fn protect_cursor(
            &self,
            binding_id: Uuid,
            session_id: AiSessionId,
            owner_principal_kind: &str,
            owner_subject: &str,
            scope: &AiScope,
            policy: &AiContentProtectionPolicy,
            descriptor: &AiProviderSessionDescriptor,
            cursor: AiProviderSessionCursor,
        ) -> Result<(serde_json::Value, String, String), AiError> {
            let cursor_fingerprint = cursor.fingerprint();
            let binding_hash = binding_hash(
                binding_id,
                session_id,
                owner_principal_kind,
                owner_subject,
                scope,
                descriptor,
                &cursor_fingerprint,
            )?;
            let (cursor_kind, cursor) = cursor.into_parts();
            let value = serde_json::to_value(ProtectedProviderSessionCursor {
                format_version: CURSOR_FORMAT_VERSION,
                cursor_kind: cursor_kind.clone(),
                cursor,
                binding_hash,
            })
            .map_err(|_| AiError::PersistenceFailed)?;
            let envelope = self
                .content_protector
                .protect(
                    policy,
                    &content_context(
                        "graphql_orm_ai_provider_session_bindings",
                        binding_id,
                        "protected_cursor",
                        scope,
                    ),
                    value,
                )
                .await
                .map_err(map_protection)?;
            let protected =
                serde_json::to_value(envelope).map_err(|_| AiError::PersistenceFailed)?;
            Ok((protected, cursor_kind, cursor_fingerprint))
        }

        async fn open_cursor(
            &self,
            record: &AiProviderSessionBindingRecord,
            policy: &AiContentProtectionPolicy,
        ) -> Result<AiProviderSessionCursor, AiError> {
            let scope = record_scope_from_binding(record);
            if policy.scope != scope || !policy.ready {
                return Err(AiError::RuntimeNotReady);
            }
            let protected = record
                .protected_cursor
                .as_ref()
                .ok_or(AiError::PersistenceFailed)?;
            let envelope: ProtectedContentEnvelope = serde_json::from_value(protected.clone())
                .map_err(|_| AiError::PersistenceFailed)?;
            let value = self
                .content_protector
                .open(
                    policy,
                    &content_context(
                        "graphql_orm_ai_provider_session_bindings",
                        record.id,
                        "protected_cursor",
                        &scope,
                    ),
                    &envelope,
                )
                .await
                .map_err(map_protection)?;
            let payload: ProtectedProviderSessionCursor =
                serde_json::from_value(value).map_err(|_| AiError::PersistenceFailed)?;
            let descriptor = descriptor_from_record(record)?;
            let expected_hash = binding_hash(
                record.id,
                AiSessionId(record.session_id),
                &record.owner_principal_kind,
                &record.owner_subject,
                &scope,
                &descriptor,
                &record.cursor_fingerprint,
            )?;
            if payload.format_version != CURSOR_FORMAT_VERSION
                || payload.cursor_kind != record.cursor_kind
                || payload.binding_hash != expected_hash
            {
                return Err(AiError::PersistenceFailed);
            }
            let cursor = AiProviderSessionCursor::new(payload.cursor_kind, payload.cursor)?;
            if cursor.fingerprint() != record.cursor_fingerprint {
                return Err(AiError::PersistenceFailed);
            }
            Ok(cursor)
        }

        async fn load_owned_active_context(
            &self,
            lease: &AiRunLease,
        ) -> Result<(agql_auth::ResolvedPrincipal, AiSessionRecord, AiScope), AiError> {
            let current = self.resolve_current(lease.principal_reference()).await?;
            let session = AiSessionRecord::find_by_id(&self.database, &lease.session_id().0)
                .await
                .map_err(|error| map_orm(OrmPublicError::from(error)))?
                .ok_or(AiError::NotFound)?;
            let scope = self
                .authorize_session(current.principal(), &session)
                .await?;
            Ok((current, session, scope))
        }
    }

    #[async_trait]
    impl AiProviderSessionService for OrmAiProviderSessionService {
        async fn inspect_for_run(
            &self,
            lease: &AiRunLease,
        ) -> Result<Option<AiProviderSessionBindingView>, AiError> {
            let (current, session, scope) = self.load_owned_active_context(lease).await?;
            self.protection_policy(current.principal(), &scope).await?;
            let records = AiProviderSessionBindingRecord::query(self.database.pool())
                .filter(AiProviderSessionBindingRecordWhereInput {
                    session_id: Some(UuidFilter {
                        eq: Some(session.id),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .limit(2)
                .fetch_all()
                .await
                .map_err(|error| map_orm(OrmPublicError::from(error)))?;
            if records.len() > 1 {
                return Err(AiError::PersistenceFailed);
            }
            let Some(record) = records.first() else {
                return Ok(None);
            };
            let (owner_kind, owner_subject) = principal_identity(current.principal());
            if record.owner_principal_kind != owner_kind
                || record.owner_subject != owner_subject
                || record_scope_from_binding(record) != scope
            {
                return Err(AiError::Conflict);
            }
            binding_view(record).map(Some)
        }

        async fn disposition_for_run(
            &self,
            lease: &AiRunLease,
            planned: &AiProviderSessionTurnPlan,
        ) -> Result<AiProviderSessionRunDisposition, AiError> {
            let (current, observed_session, scope) = self.load_owned_active_context(lease).await?;
            self.protection_policy(current.principal(), &scope).await?;
            let now = canonical_second(self.clock.now());
            let authorization_expires_at = now
                .checked_add(MAXIMUM_PROVIDER_SESSION_REBIND_AUTHORIZATION_TTL)
                .map(|expiry| expiry.min(lease.lease_expires_at()))
                .filter(|expiry| *expiry > now)
                .ok_or(AiError::Conflict)?;
            let expected_owner = principal_identity(current.principal());
            let expected_owner_kind = expected_owner.0;
            let expected_owner_subject = expected_owner.1.to_owned();
            let lease = lease.clone();
            let claim_principal_reference = lease.principal_reference().clone();
            let planned = planned.clone();
            self.database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let run = load_and_validate_active_lease(tx, &lease, now).await?;
                        if run.state != AiRunState::Running.as_str() {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let session = tx
                            .find_by_id::<AiSessionRecord>(&lease.session_id().0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if session != observed_session
                            || session.state != "active"
                            || session.deleted_at.is_some()
                            || session.owner_principal_kind != expected_owner_kind
                            || session.owner_subject != expected_owner_subject
                            || record_scope(&session) != scope
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let input = tx
                            .find_by_id::<AiMessageRecord>(&lease.input_message_id())
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        if input.session_id != session.id
                            || input.message_role != "user"
                            || input.completion_state != "complete"
                            || input.sequence != session.message_head
                            || input.sequence <= 0
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let records = tx
                            .query::<AiProviderSessionBindingRecord>()
                            .filter(AiProviderSessionBindingRecordWhereInput {
                                session_id: Some(UuidFilter {
                                    eq: Some(session.id),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .limit(2)
                            .fetch_all()
                            .await
                            .map_err(OrmPublicError::from)?;
                        if records.len() > 1 {
                            return Err(OrmPublicError::new(OrmErrorCode::InternalError));
                        }
                        let Some(binding) = records.first() else {
                            return Ok(AiProviderSessionRunDisposition::New);
                        };
                        validate_binding_record(binding)?;
                        if binding.owner_principal_kind != session.owner_principal_kind
                            || binding.owner_subject != session.owner_subject
                            || record_scope_from_binding(binding) != scope
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let state = AiProviderSessionState::from_persisted(&binding.state)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        if state == AiProviderSessionState::Claimed
                            && binding.claimed_run_id == Some(lease.run_id().0)
                            && binding.claimed_attempt_id == Some(lease.attempt_id())
                            && binding.claimed_run_lease_generation
                                == Some(lease.lease_generation())
                            && descriptor_from_record(binding).map_err(ai_error_to_orm)?
                                == *planned.descriptor()
                            && binding.transcript_fingerprint == planned.transcript_fingerprint()
                            && binding
                                .claim_expires_at
                                .is_some_and(|expiry| expiry > now.unix_timestamp())
                        {
                            return claim_from_record(binding, &claim_principal_reference)
                                .map(Box::new)
                                .map(AiProviderSessionRunDisposition::Reclaimed)
                                .map_err(ai_error_to_orm);
                        }
                        if state == AiProviderSessionState::Active
                            && descriptor_from_record(binding).map_err(ai_error_to_orm)?
                                == *planned.descriptor()
                            && binding.transcript_fingerprint == planned.transcript_fingerprint()
                            && binding.through_message_sequence.checked_add(1)
                                == Some(input.sequence)
                            && binding.idle_expires_at > now.unix_timestamp()
                            && binding.absolute_expires_at > now.unix_timestamp()
                            && binding
                                .provider_expires_at
                                .is_none_or(|expiry| expiry > now.unix_timestamp())
                        {
                            return binding_view(binding)
                                .map(Box::new)
                                .map(AiProviderSessionRunDisposition::Resume)
                                .map_err(ai_error_to_orm);
                        }
                        if state == AiProviderSessionState::Deleted
                            && binding.last_run_id != Some(lease.run_id().0)
                            && descriptor_from_record(binding).map_err(ai_error_to_orm)?
                                == *planned.descriptor()
                        {
                            let provider_absence_observed_at = binding
                                .provider_absence_observed_at
                                .and_then(|value| OffsetDateTime::from_unix_timestamp(value).ok())
                                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                            return Ok(AiProviderSessionRunDisposition::RebindAllowed(Box::new(
                                AiProviderSessionRebindAuthorization {
                                    binding_id: binding.id,
                                    session_id: AiSessionId(binding.session_id),
                                    run_id: lease.run_id(),
                                    attempt_id: lease.attempt_id(),
                                    run_lease_generation: lease.lease_generation(),
                                    binding_row_version: binding.row_version,
                                    binding_claim_generation: binding.claim_generation,
                                    cleanup_generation: binding.cleanup_generation,
                                    provider_absence_observed_at,
                                    expires_at: authorization_expires_at,
                                    principal_reference: lease.principal_reference().clone(),
                                    scope: scope.clone(),
                                    descriptor: planned.descriptor().clone(),
                                    transcript_fingerprint: planned
                                        .transcript_fingerprint()
                                        .to_owned(),
                                },
                            )));
                        }
                        Ok(AiProviderSessionRunDisposition::Unavailable(state))
                    })
                })
                .await
                .map_err(map_transaction)
        }

        async fn bind_for_run(
            &self,
            lease: &AiRunLease,
            request: AiProviderSessionBindRequest,
        ) -> Result<AiProviderSessionClaim, AiError> {
            let (current, observed_session, scope) = self.load_owned_active_context(lease).await?;
            let policy = self.protection_policy(current.principal(), &scope).await?;
            let now = canonical_second(self.clock.now());
            let binding_id = Uuid::new_v4();
            let (descriptor, cursor, transcript_fingerprint, provider_expires_at) =
                request.into_parts();
            let absolute_expires_at = now
                .checked_add(self.limits.absolute_lifetime())
                .ok_or(AiError::PersistenceFailed)?;
            let idle_expires_at = now
                .checked_add(self.limits.idle_ttl())
                .ok_or(AiError::PersistenceFailed)?;
            if provider_expires_at.is_some_and(|expiry| expiry <= now) {
                return Err(AiError::InvalidInput(
                    "invalid provider-session expiry".to_owned(),
                ));
            }
            let expected_owner = principal_identity(current.principal());
            let expected_owner_kind = expected_owner.0;
            let expected_owner_subject = expected_owner.1.to_owned();
            let (protected_cursor, cursor_kind, cursor_fingerprint) = self
                .protect_cursor(
                    binding_id,
                    lease.session_id(),
                    &expected_owner_kind,
                    &expected_owner_subject,
                    &scope,
                    &policy,
                    &descriptor,
                    cursor,
                )
                .await?;
            let claim_expires_at = now
                .checked_add(self.limits.claim_lease_ttl())
                .map(|expiry| expiry.min(lease.lease_expires_at()))
                .filter(|expiry| *expiry > now)
                .ok_or(AiError::Conflict)?;
            let principal_reference = serde_json::to_value(lease.principal_reference())
                .map_err(|_| AiError::PersistenceFailed)?;
            let principal_reference_for_insert = principal_reference.clone();
            let descriptor_for_insert = descriptor.clone();
            let lease = lease.clone();
            let claim_principal_reference = lease.principal_reference().clone();
            let scope_for_insert = scope.clone();
            let record = self
                .database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let current_run = load_and_validate_active_lease(tx, &lease, now).await?;
                        if current_run.state != AiRunState::Running.as_str() {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let session = tx
                            .find_by_id::<AiSessionRecord>(&lease.session_id().0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if session != observed_session
                            || session.state != "active"
                            || session.deleted_at.is_some()
                            || session.owner_principal_kind != expected_owner_kind
                            || session.owner_subject != expected_owner_subject
                            || record_scope(&session) != scope_for_insert
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let input = tx
                            .find_by_id::<AiMessageRecord>(&lease.input_message_id())
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        if input.session_id != session.id
                            || input.message_role != "user"
                            || input.completion_state != "complete"
                            || input.sequence != session.message_head
                            || input.sequence <= 0
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let existing = tx
                            .query::<AiProviderSessionBindingRecord>()
                            .filter(AiProviderSessionBindingRecordWhereInput {
                                session_id: Some(UuidFilter {
                                    eq: Some(session.id),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .limit(1)
                            .fetch_all()
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !existing.is_empty() {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let inserted = tx
                            .insert::<AiProviderSessionBindingRecord>(
                                CreateAiProviderSessionBindingRecordInput {
                                    id: binding_id,
                                    session_id: session.id,
                                    owner_principal_kind: expected_owner_kind.clone(),
                                    owner_subject: expected_owner_subject.clone(),
                                    principal_reference: principal_reference_for_insert,
                                    scope_key: ai_scope_key(&scope_for_insert),
                                    scope_kind: scope_for_insert.kind.clone(),
                                    scope_id: scope_for_insert.id.clone(),
                                    tenant_id: scope_for_insert.tenant_id.clone(),
                                    provider_kind: provider_kind_value(
                                        descriptor_for_insert.provider_kind(),
                                    )
                                    .map_err(ai_error_to_orm)?,
                                    provider_profile_id: descriptor_for_insert
                                        .provider_profile_id()
                                        .to_owned(),
                                    provider_model: descriptor_for_insert
                                        .provider_model()
                                        .to_owned(),
                                    registration_fingerprint: descriptor_for_insert
                                        .registration_fingerprint()
                                        .to_owned(),
                                    protocol_version: descriptor_for_insert
                                        .protocol_version()
                                        .to_owned(),
                                    policy_fingerprint: descriptor_for_insert
                                        .policy_fingerprint()
                                        .to_owned(),
                                    cursor_kind,
                                    cursor_fingerprint,
                                    protected_cursor: Some(protected_cursor),
                                    through_message_sequence: input.sequence - 1,
                                    transcript_fingerprint,
                                    last_run_id: None,
                                    last_assistant_message_id: None,
                                    state: AiProviderSessionState::Claimed.as_str().to_owned(),
                                    claimed_run_id: Some(lease.run_id().0),
                                    claimed_attempt_id: Some(lease.attempt_id()),
                                    claimed_run_lease_generation: Some(lease.lease_generation()),
                                    claim_owner: Some(lease.worker_id().to_owned()),
                                    claim_generation: 1,
                                    claim_expires_at: Some(claim_expires_at.unix_timestamp()),
                                    parked_wait_kind: None,
                                    parked_wait_id: None,
                                    park_generation: 0,
                                    parked_source_checkpoint_id: None,
                                    parked_source_checkpoint_fingerprint: None,
                                    parked_checkpoint_id: None,
                                    parked_checkpoint_fingerprint: None,
                                    parked_continuation_fingerprint: None,
                                    parked_confirmed_at: None,
                                    parked_expires_at: None,
                                    parked_reclaimed_at: None,
                                    provider_expires_at: provider_expires_at
                                        .map(OffsetDateTime::unix_timestamp),
                                    idle_expires_at: idle_expires_at.unix_timestamp(),
                                    absolute_expires_at: absolute_expires_at.unix_timestamp(),
                                    cleanup_owner: None,
                                    cleanup_generation: 0,
                                    cleanup_lease_expires_at: None,
                                    cleanup_retry_count: 0,
                                    cleanup_next_attempt_at: None,
                                    cleanup_reason_code: None,
                                    provider_absence_observed_at: None,
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        append_audit(
                            tx,
                            "ai.provider_session.bound",
                            inserted.id,
                            "provider_session_bound_empty",
                            lease.run_id().0,
                            now,
                        )
                        .await?;
                        Ok(inserted)
                    })
                })
                .await
                .map_err(map_transaction)?;
            claim_from_record(&record, &claim_principal_reference)
        }

        async fn rebind_for_run(
            &self,
            lease: &AiRunLease,
            authorization: AiProviderSessionRebindAuthorization,
            request: AiProviderSessionBindRequest,
        ) -> Result<AiProviderSessionClaim, AiError> {
            let (current, observed_session, scope) = self.load_owned_active_context(lease).await?;
            let policy = self.protection_policy(current.principal(), &scope).await?;
            let now = canonical_second(self.clock.now());
            let (descriptor, cursor, transcript_fingerprint, provider_expires_at) =
                request.into_parts();
            if authorization.session_id != lease.session_id()
                || authorization.run_id != lease.run_id()
                || authorization.attempt_id != lease.attempt_id()
                || authorization.run_lease_generation != lease.lease_generation()
                || authorization.principal_reference != *lease.principal_reference()
                || authorization.scope != scope
                || authorization.descriptor != descriptor
                || authorization.transcript_fingerprint != transcript_fingerprint
                || authorization.expires_at <= now
                || authorization.provider_absence_observed_at > now
                || provider_expires_at.is_some_and(|expiry| expiry <= now)
            {
                return Err(AiError::Conflict);
            }
            let expected_owner = principal_identity(current.principal());
            let expected_owner_kind = expected_owner.0;
            let expected_owner_subject = expected_owner.1.to_owned();
            let (protected_cursor, cursor_kind, cursor_fingerprint) = self
                .protect_cursor(
                    authorization.binding_id,
                    lease.session_id(),
                    &expected_owner_kind,
                    &expected_owner_subject,
                    &scope,
                    &policy,
                    &descriptor,
                    cursor,
                )
                .await?;
            let absolute_expires_at = now
                .checked_add(self.limits.absolute_lifetime())
                .ok_or(AiError::PersistenceFailed)?;
            let idle_expires_at = now
                .checked_add(self.limits.idle_ttl())
                .ok_or(AiError::PersistenceFailed)?;
            let claim_expires_at = now
                .checked_add(self.limits.claim_lease_ttl())
                .map(|expiry| expiry.min(lease.lease_expires_at()))
                .filter(|expiry| *expiry > now)
                .ok_or(AiError::Conflict)?;
            let principal_reference = serde_json::to_value(lease.principal_reference())
                .map_err(|_| AiError::PersistenceFailed)?;
            let descriptor_for_update = descriptor.clone();
            let scope_for_update = scope.clone();
            let lease = lease.clone();
            let claim_principal_reference = lease.principal_reference().clone();
            let record = self
                .database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let current_run = load_and_validate_active_lease(tx, &lease, now).await?;
                        if current_run.state != AiRunState::Running.as_str() {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let session = tx
                            .find_by_id::<AiSessionRecord>(&lease.session_id().0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if session != observed_session
                            || session.state != "active"
                            || session.deleted_at.is_some()
                            || session.owner_principal_kind != expected_owner_kind
                            || session.owner_subject != expected_owner_subject
                            || record_scope(&session) != scope_for_update
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let input = tx
                            .find_by_id::<AiMessageRecord>(&lease.input_message_id())
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        if input.session_id != session.id
                            || input.message_role != "user"
                            || input.completion_state != "complete"
                            || input.sequence != session.message_head
                            || input.sequence <= 0
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let binding = tx
                            .find_by_id::<AiProviderSessionBindingRecord>(&authorization.binding_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        validate_binding_record(&binding)?;
                        let absence_observed_at =
                            authorization.provider_absence_observed_at.unix_timestamp();
                        if binding.session_id != session.id
                            || binding.owner_principal_kind != expected_owner_kind
                            || binding.owner_subject != expected_owner_subject
                            || record_scope_from_binding(&binding) != scope_for_update
                            || binding.state != AiProviderSessionState::Deleted.as_str()
                            || binding.protected_cursor.is_some()
                            || binding.provider_absence_observed_at != Some(absence_observed_at)
                            || binding.row_version != authorization.binding_row_version
                            || binding.claim_generation != authorization.binding_claim_generation
                            || binding.cleanup_generation != authorization.cleanup_generation
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let claim_generation = binding
                            .claim_generation
                            .checked_add(1)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        let outcome = tx
                            .compare_and_swap::<AiProviderSessionBindingRecord>(
                                &binding.id,
                                authorization.binding_row_version,
                                AiProviderSessionBindingRecordWhereInput {
                                    state: Some(StringFilter {
                                        eq: Some(
                                            AiProviderSessionState::Deleted.as_str().to_owned(),
                                        ),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                },
                                UpdateAiProviderSessionBindingRecordInput {
                                    owner_principal_kind: Some(expected_owner_kind.clone()),
                                    owner_subject: Some(expected_owner_subject.clone()),
                                    principal_reference: Some(principal_reference),
                                    scope_key: Some(ai_scope_key(&scope_for_update)),
                                    scope_kind: Some(scope_for_update.kind.clone()),
                                    scope_id: Some(scope_for_update.id.clone()),
                                    tenant_id: Some(scope_for_update.tenant_id.clone()),
                                    provider_kind: Some(
                                        provider_kind_value(descriptor_for_update.provider_kind())
                                            .map_err(ai_error_to_orm)?,
                                    ),
                                    provider_profile_id: Some(
                                        descriptor_for_update.provider_profile_id().to_owned(),
                                    ),
                                    provider_model: Some(
                                        descriptor_for_update.provider_model().to_owned(),
                                    ),
                                    registration_fingerprint: Some(
                                        descriptor_for_update.registration_fingerprint().to_owned(),
                                    ),
                                    protocol_version: Some(
                                        descriptor_for_update.protocol_version().to_owned(),
                                    ),
                                    policy_fingerprint: Some(
                                        descriptor_for_update.policy_fingerprint().to_owned(),
                                    ),
                                    cursor_kind: Some(cursor_kind),
                                    cursor_fingerprint: Some(cursor_fingerprint),
                                    protected_cursor: Some(Some(protected_cursor)),
                                    through_message_sequence: Some(input.sequence - 1),
                                    transcript_fingerprint: Some(transcript_fingerprint),
                                    last_run_id: Some(None),
                                    last_assistant_message_id: Some(None),
                                    state: Some(
                                        AiProviderSessionState::Claimed.as_str().to_owned(),
                                    ),
                                    claimed_run_id: Some(Some(lease.run_id().0)),
                                    claimed_attempt_id: Some(Some(lease.attempt_id())),
                                    claimed_run_lease_generation: Some(Some(
                                        lease.lease_generation(),
                                    )),
                                    claim_owner: Some(Some(lease.worker_id().to_owned())),
                                    claim_generation: Some(claim_generation),
                                    claim_expires_at: Some(Some(claim_expires_at.unix_timestamp())),
                                    parked_wait_kind: Some(None),
                                    parked_wait_id: Some(None),
                                    parked_source_checkpoint_id: Some(None),
                                    parked_source_checkpoint_fingerprint: Some(None),
                                    parked_checkpoint_id: Some(None),
                                    parked_checkpoint_fingerprint: Some(None),
                                    parked_continuation_fingerprint: Some(None),
                                    parked_confirmed_at: Some(None),
                                    parked_expires_at: Some(None),
                                    parked_reclaimed_at: Some(None),
                                    provider_expires_at: Some(
                                        provider_expires_at.map(OffsetDateTime::unix_timestamp),
                                    ),
                                    idle_expires_at: Some(idle_expires_at.unix_timestamp()),
                                    absolute_expires_at: Some(absolute_expires_at.unix_timestamp()),
                                    cleanup_owner: Some(None),
                                    cleanup_lease_expires_at: Some(None),
                                    cleanup_retry_count: Some(0),
                                    cleanup_next_attempt_at: Some(None),
                                    cleanup_reason_code: Some(None),
                                    provider_absence_observed_at: Some(None),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        let ConditionalUpdateOutcome::Updated(updated) = outcome else {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        };
                        append_audit(
                            tx,
                            "ai.provider_session.rebound",
                            updated.id,
                            "provider_session_absence_rebound",
                            lease.run_id().0,
                            now,
                        )
                        .await?;
                        Ok(updated)
                    })
                })
                .await
                .map_err(map_transaction)?;
            claim_from_record(&record, &claim_principal_reference)
        }

        async fn claim_for_run(
            &self,
            lease: &AiRunLease,
            expected: &AiProviderSessionDescriptor,
            expected_transcript_fingerprint: &str,
        ) -> Result<AiProviderSessionClaim, AiError> {
            if !crate::valid_sha256(expected_transcript_fingerprint) {
                return Err(AiError::InvalidInput(
                    "invalid provider-session transcript fingerprint".to_owned(),
                ));
            }
            let (current, observed_session, scope) = self.load_owned_active_context(lease).await?;
            self.protection_policy(current.principal(), &scope).await?;
            let now = canonical_second(self.clock.now());
            let claim_expires_at = now
                .checked_add(self.limits.claim_lease_ttl())
                .map(|expiry| expiry.min(lease.lease_expires_at()))
                .filter(|expiry| *expiry > now)
                .ok_or(AiError::Conflict)?;
            let expected = expected.clone();
            let expected_transcript_fingerprint = expected_transcript_fingerprint.to_owned();
            let lease = lease.clone();
            let claim_principal_reference = lease.principal_reference().clone();
            let record = self
                .database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let run = load_and_validate_active_lease(tx, &lease, now).await?;
                        if run.state != AiRunState::Running.as_str() {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let session = tx
                            .find_by_id::<AiSessionRecord>(&lease.session_id().0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if session != observed_session || record_scope(&session) != scope {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let input = tx
                            .find_by_id::<AiMessageRecord>(&lease.input_message_id())
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        let records = tx
                            .query::<AiProviderSessionBindingRecord>()
                            .filter(AiProviderSessionBindingRecordWhereInput {
                                session_id: Some(UuidFilter {
                                    eq: Some(session.id),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .limit(2)
                            .fetch_all()
                            .await
                            .map_err(OrmPublicError::from)?;
                        if records.len() != 1 {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let binding = &records[0];
                        validate_binding_record(binding)?;
                        if binding.state != AiProviderSessionState::Active.as_str()
                            || descriptor_from_record(binding).map_err(ai_error_to_orm)? != expected
                            || binding.owner_principal_kind != session.owner_principal_kind
                            || binding.owner_subject != session.owner_subject
                            || record_scope_from_binding(binding) != scope
                            || input.session_id != session.id
                            || input.message_role != "user"
                            || input.completion_state != "complete"
                            || input.sequence != session.message_head
                            || binding.through_message_sequence.checked_add(1)
                                != Some(input.sequence)
                            || binding.transcript_fingerprint != expected_transcript_fingerprint
                            || binding.idle_expires_at <= now.unix_timestamp()
                            || binding.absolute_expires_at <= now.unix_timestamp()
                            || binding
                                .provider_expires_at
                                .is_some_and(|expiry| expiry <= now.unix_timestamp())
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let generation = binding
                            .claim_generation
                            .checked_add(1)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        let outcome = tx
                            .compare_and_swap::<AiProviderSessionBindingRecord>(
                                &binding.id,
                                binding.row_version,
                                AiProviderSessionBindingRecordWhereInput {
                                    state: Some(StringFilter {
                                        eq: Some(
                                            AiProviderSessionState::Active.as_str().to_owned(),
                                        ),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                },
                                UpdateAiProviderSessionBindingRecordInput {
                                    state: Some(
                                        AiProviderSessionState::Claimed.as_str().to_owned(),
                                    ),
                                    claimed_run_id: Some(Some(lease.run_id().0)),
                                    claimed_attempt_id: Some(Some(lease.attempt_id())),
                                    claimed_run_lease_generation: Some(Some(
                                        lease.lease_generation(),
                                    )),
                                    claim_owner: Some(Some(lease.worker_id().to_owned())),
                                    claim_generation: Some(generation),
                                    claim_expires_at: Some(Some(claim_expires_at.unix_timestamp())),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        match outcome {
                            ConditionalUpdateOutcome::Updated(updated) => Ok(updated),
                            ConditionalUpdateOutcome::NotFound
                            | ConditionalUpdateOutcome::Conflict => {
                                Err(OrmPublicError::new(OrmErrorCode::Conflict))
                            }
                        }
                    })
                })
                .await
                .map_err(map_transaction)?;
            claim_from_record(&record, &claim_principal_reference)
        }

        async fn open_for_run(
            &self,
            lease: &AiRunLease,
            claim: &AiProviderSessionClaim,
        ) -> Result<AiOpenedProviderSession, AiError> {
            let (current, session, scope) = self.load_owned_active_context(lease).await?;
            let policy = self.protection_policy(current.principal(), &scope).await?;
            let now = canonical_second(self.clock.now());
            let lease_for_read = lease.clone();
            let claim_for_read = claim.clone();
            let record = self
                .database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        load_and_validate_active_lease(tx, &lease_for_read, now).await?;
                        let record = tx
                            .find_by_id::<AiProviderSessionBindingRecord>(
                                &claim_for_read.binding_id,
                            )
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        validate_active_claim(&record, &claim_for_read, &lease_for_read, now)?;
                        Ok(record)
                    })
                })
                .await
                .map_err(map_transaction)?;
            if record.session_id != session.id || record_scope_from_binding(&record) != scope {
                return Err(AiError::Conflict);
            }
            let cursor = self.open_cursor(&record, &policy).await?;
            Ok(AiOpenedProviderSession::new(claim.clone(), cursor))
        }

        async fn heartbeat(
            &self,
            lease: &AiRunLease,
            claim: &AiProviderSessionClaim,
        ) -> Result<AiProviderSessionClaim, AiError> {
            let (current, _session, scope) = self.load_owned_active_context(lease).await?;
            self.protection_policy(current.principal(), &scope).await?;
            let now = canonical_second(self.clock.now());
            let expiry = now
                .checked_add(self.limits.claim_lease_ttl())
                .map(|expiry| expiry.min(lease.lease_expires_at()))
                .filter(|expiry| *expiry > now)
                .ok_or(AiError::Conflict)?;
            let lease = lease.clone();
            let claim_principal_reference = lease.principal_reference().clone();
            let claim = claim.clone();
            let record = self
                .database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        load_and_validate_active_lease(tx, &lease, now).await?;
                        let record = tx
                            .find_by_id::<AiProviderSessionBindingRecord>(&claim.binding_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        validate_active_claim(&record, &claim, &lease, now)?;
                        let outcome = tx
                            .compare_and_swap::<AiProviderSessionBindingRecord>(
                                &record.id,
                                record.row_version,
                                AiProviderSessionBindingRecordWhereInput::default(),
                                UpdateAiProviderSessionBindingRecordInput {
                                    claim_expires_at: Some(Some(expiry.unix_timestamp())),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        match outcome {
                            ConditionalUpdateOutcome::Updated(updated) => Ok(updated),
                            ConditionalUpdateOutcome::NotFound
                            | ConditionalUpdateOutcome::Conflict => {
                                Err(OrmPublicError::new(OrmErrorCode::Conflict))
                            }
                        }
                    })
                })
                .await
                .map_err(map_transaction)?;
            claim_from_record(&record, &claim_principal_reference)
        }

        async fn park_for_wait(
            &self,
            lease: &AiRunLease,
            request: AiProviderSessionWaitParkRequest,
        ) -> Result<AiProviderSessionParkedWait, AiError> {
            if request.wait.wait_id().is_nil()
                || request.session_id() != lease.session_id()
                || request.run_id() != lease.run_id()
                || request.attempt_id() != lease.attempt_id()
                || request.run_lease_generation() != lease.lease_generation()
                || !crate::valid_sha256(&request.source_checkpoint_fingerprint)
                || !crate::valid_sha256(&request.continuation_fingerprint)
                || !request.has_valid_fingerprint()
            {
                return Err(AiError::Conflict);
            }
            let claim = request.claim.clone();
            if claim.session_id() != lease.session_id()
                || claim.run_id() != lease.run_id()
                || claim.attempt_id() != lease.attempt_id()
                || claim.run_lease_generation() != lease.lease_generation()
            {
                return Err(AiError::Conflict);
            }
            let continuation_fingerprint = request.continuation_fingerprint.clone();
            let wait = request.wait;
            let (current, observed_session, scope) = self.load_owned_active_context(lease).await?;
            self.protection_policy(current.principal(), &scope).await?;
            let now = canonical_second(self.clock.now());
            let lease = lease.clone();
            let record = self
                .database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let run = load_and_validate_active_lease(tx, &lease, now).await?;
                        if run.state != AiRunState::Running.as_str() {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let session = tx
                            .find_by_id::<AiSessionRecord>(&lease.session_id().0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if session != observed_session || record_scope(&session) != scope {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let binding = tx
                            .find_by_id::<AiProviderSessionBindingRecord>(&claim.binding_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        validate_active_claim(&binding, &claim, &lease, now)?;
                        let source_checkpoint_id = run
                            .latest_checkpoint_id
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        let checkpoint = tx
                            .query::<AiRunCheckpointRecord>()
                            .filter(AiRunCheckpointRecordWhereInput {
                                id: Some(UuidFilter {
                                    eq: Some(source_checkpoint_id),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .limit(1)
                            .fetch_one()
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        validate_provider_turn_checkpoint(&checkpoint, &binding, &lease, &request)?;
                        let park_generation = binding
                            .park_generation
                            .checked_add(1)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        let parked_expires_at = binding
                            .claim_expires_at
                            .filter(|expiry| *expiry > now.unix_timestamp())
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        let outcome = tx
                            .compare_and_swap::<AiProviderSessionBindingRecord>(
                                &binding.id,
                                binding.row_version,
                                AiProviderSessionBindingRecordWhereInput {
                                    state: Some(StringFilter {
                                        eq: Some(
                                            AiProviderSessionState::Claimed.as_str().to_owned(),
                                        ),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                },
                                UpdateAiProviderSessionBindingRecordInput {
                                    state: Some(
                                        AiProviderSessionState::ParkedWait.as_str().to_owned(),
                                    ),
                                    park_generation: Some(park_generation),
                                    parked_wait_kind: Some(Some(wait.kind().as_str().to_owned())),
                                    parked_wait_id: Some(Some(wait.wait_id())),
                                    parked_source_checkpoint_id: Some(Some(checkpoint.id)),
                                    parked_source_checkpoint_fingerprint: Some(Some(
                                        checkpoint.checkpoint_hash.clone(),
                                    )),
                                    parked_checkpoint_id: Some(None),
                                    parked_checkpoint_fingerprint: Some(None),
                                    parked_continuation_fingerprint: Some(Some(
                                        continuation_fingerprint.clone(),
                                    )),
                                    parked_confirmed_at: Some(None),
                                    parked_expires_at: Some(Some(parked_expires_at)),
                                    parked_reclaimed_at: Some(None),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        let ConditionalUpdateOutcome::Updated(updated) = outcome else {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        };
                        append_audit(
                            tx,
                            "ai.provider_session.wait_parked",
                            binding.id,
                            "provider_session_wait_parked_unconfirmed",
                            lease.run_id().0,
                            now,
                        )
                        .await?;
                        Ok(updated)
                    })
                })
                .await
                .map_err(map_transaction)?;
            parked_wait_from_record(&record)
        }

        async fn confirm_parked_wait(
            &self,
            parked: &AiProviderSessionParkedWait,
        ) -> Result<(), AiError> {
            let observed =
                AiProviderSessionBindingRecord::find_by_id(&self.database, &parked.binding_id)
                    .await
                    .map_err(|error| map_orm(OrmPublicError::from(error)))?
                    .ok_or(AiError::NotFound)?;
            let principal_reference: PrincipalReference =
                serde_json::from_value(observed.principal_reference.clone())
                    .map_err(|_| AiError::PersistenceFailed)?;
            let current = self.resolve_current(&principal_reference).await?;
            let session = AiSessionRecord::find_by_id(&self.database, &observed.session_id)
                .await
                .map_err(|error| map_orm(OrmPublicError::from(error)))?
                .ok_or(AiError::NotFound)?;
            let scope = self
                .authorize_session(current.principal(), &session)
                .await?;
            self.protection_policy(current.principal(), &scope).await?;
            let now = canonical_second(self.clock.now());
            let parked = parked.clone();
            self.database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let binding = tx
                            .find_by_id::<AiProviderSessionBindingRecord>(&parked.binding_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        validate_parked_proof(&binding, &parked)?;
                        let durable_session = tx
                            .find_by_id::<AiSessionRecord>(&binding.session_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if durable_session != session
                            || record_scope_from_binding(&binding) != scope
                            || binding.owner_principal_kind != durable_session.owner_principal_kind
                            || binding.owner_subject != durable_session.owner_subject
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let run = tx
                            .find_by_id::<AiRunRecord>(&parked.source_run_id.0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        let source_checkpoint = tx
                            .query::<AiRunCheckpointRecord>()
                            .filter(AiRunCheckpointRecordWhereInput {
                                id: Some(UuidFilter {
                                    eq: Some(parked.source_checkpoint_id),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .limit(1)
                            .fetch_one()
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if source_checkpoint.run_id != run.id
                            || source_checkpoint.attempt_id != parked.source_attempt_id
                            || source_checkpoint.lease_generation
                                != parked.source_run_lease_generation
                            || source_checkpoint.checkpoint_kind != "provider_turn_persisted"
                            || source_checkpoint.checkpoint_hash
                                != parked.source_checkpoint_fingerprint
                            || run.lease_owner.is_some()
                            || run.lease_expires_at.is_some()
                            || run.lease_heartbeat_at.is_some()
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let parked_checkpoint_id = run
                            .latest_checkpoint_id
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        let parked_checkpoint = tx
                            .query::<AiRunCheckpointRecord>()
                            .filter(AiRunCheckpointRecordWhereInput {
                                id: Some(UuidFilter {
                                    eq: Some(parked_checkpoint_id),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .limit(1)
                            .fetch_one()
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        let wait_expires_at = validate_wait_confirmation(
                            tx,
                            &binding,
                            &run,
                            &source_checkpoint,
                            &parked_checkpoint,
                            parked.wait,
                            now,
                        )
                        .await?;
                        if binding.parked_confirmed_at.is_some() {
                            if binding.parked_checkpoint_id != Some(parked_checkpoint.id)
                                || binding.parked_checkpoint_fingerprint.as_deref()
                                    != Some(parked_checkpoint.checkpoint_hash.as_str())
                                || binding.parked_expires_at != Some(wait_expires_at)
                            {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                            return Ok(());
                        }
                        let outcome = tx
                            .compare_and_swap::<AiProviderSessionBindingRecord>(
                                &binding.id,
                                binding.row_version,
                                AiProviderSessionBindingRecordWhereInput {
                                    state: Some(StringFilter {
                                        eq: Some(
                                            AiProviderSessionState::ParkedWait.as_str().to_owned(),
                                        ),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                },
                                UpdateAiProviderSessionBindingRecordInput {
                                    parked_checkpoint_id: Some(Some(parked_checkpoint.id)),
                                    parked_checkpoint_fingerprint: Some(Some(
                                        parked_checkpoint.checkpoint_hash,
                                    )),
                                    parked_confirmed_at: Some(Some(now.unix_timestamp())),
                                    parked_expires_at: Some(Some(wait_expires_at)),
                                    claim_owner: Some(None),
                                    claim_expires_at: Some(None),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(outcome, ConditionalUpdateOutcome::Updated(_)) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        append_audit(
                            tx,
                            "ai.provider_session.wait_confirmed",
                            binding.id,
                            "provider_session_wait_graph_confirmed",
                            run.id,
                            now,
                        )
                        .await
                    })
                })
                .await
                .map_err(map_transaction)
        }

        async fn require_parked_wait_cleanup(
            &self,
            parked: &AiProviderSessionParkedWait,
            reason_code: &str,
        ) -> Result<(), AiError> {
            validate_reason_code(reason_code)?;
            let now = canonical_second(self.clock.now());
            let parked = parked.clone();
            let reason_code = reason_code.to_owned();
            self.database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let binding = tx
                            .find_by_id::<AiProviderSessionBindingRecord>(&parked.binding_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        validate_parked_proof(&binding, &parked)?;
                        let outcome = tx
                            .compare_and_swap::<AiProviderSessionBindingRecord>(
                                &binding.id,
                                binding.row_version,
                                AiProviderSessionBindingRecordWhereInput {
                                    state: Some(StringFilter {
                                        eq: Some(
                                            AiProviderSessionState::ParkedWait.as_str().to_owned(),
                                        ),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                },
                                cleanup_required_update(
                                    reason_code.clone(),
                                    parked.source_run_id.0,
                                ),
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(outcome, ConditionalUpdateOutcome::Updated(_)) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        append_audit(
                            tx,
                            "ai.provider_session.cleanup_required",
                            binding.id,
                            &reason_code,
                            parked.source_run_id.0,
                            now,
                        )
                        .await
                    })
                })
                .await
                .map_err(map_transaction)
        }

        async fn require_wait_handoff_cleanup(
            &self,
            request: &AiProviderSessionWaitParkRequest,
            reason_code: &str,
        ) -> Result<(), AiError> {
            validate_reason_code(reason_code)?;
            if !request.has_valid_fingerprint() {
                return Err(AiError::Conflict);
            }
            let now = canonical_second(self.clock.now());
            let request = request.clone();
            let reason_code = reason_code.to_owned();
            self.database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let binding = tx
                            .find_by_id::<AiProviderSessionBindingRecord>(&request.claim.binding_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        validate_wait_handoff_record(&binding, &request)?;
                        let outcome = tx
                            .compare_and_swap::<AiProviderSessionBindingRecord>(
                                &binding.id,
                                binding.row_version,
                                AiProviderSessionBindingRecordWhereInput::default(),
                                cleanup_required_update(
                                    reason_code.clone(),
                                    request.claim.run_id.0,
                                ),
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(outcome, ConditionalUpdateOutcome::Updated(_)) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        append_audit(
                            tx,
                            "ai.provider_session.cleanup_required",
                            binding.id,
                            &reason_code,
                            request.claim.run_id.0,
                            now,
                        )
                        .await
                    })
                })
                .await
                .map_err(map_transaction)
        }

        async fn reclaim_after_wait(
            &self,
            lease: &AiRunLease,
        ) -> Result<AiProviderSessionClaim, AiError> {
            let (current, observed_session, scope) = self.load_owned_active_context(lease).await?;
            self.protection_policy(current.principal(), &scope).await?;
            let now = canonical_second(self.clock.now());
            let claim_expires_at = now
                .checked_add(self.limits.claim_lease_ttl())
                .map(|expiry| expiry.min(lease.lease_expires_at()))
                .filter(|expiry| *expiry > now)
                .ok_or(AiError::Conflict)?;
            let lease = lease.clone();
            let principal_reference = lease.principal_reference().clone();
            let record = self
                .database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let run = load_and_validate_active_lease(tx, &lease, now).await?;
                        if run.state != AiRunState::Running.as_str() {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let session = tx
                            .find_by_id::<AiSessionRecord>(&lease.session_id().0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if session != observed_session || record_scope(&session) != scope {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let records = tx
                            .query::<AiProviderSessionBindingRecord>()
                            .filter(AiProviderSessionBindingRecordWhereInput {
                                session_id: Some(UuidFilter {
                                    eq: Some(session.id),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .limit(2)
                            .fetch_all()
                            .await
                            .map_err(OrmPublicError::from)?;
                        if records.len() != 1 {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let binding = &records[0];
                        validate_confirmed_parked_record(binding, now)?;
                        if binding.session_id != session.id
                            || binding.claimed_run_id != Some(lease.run_id().0)
                            || binding.owner_principal_kind != session.owner_principal_kind
                            || binding.owner_subject != session.owner_subject
                            || record_scope_from_binding(binding) != scope
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        validate_wait_reclaim(tx, binding, &run, &lease, now).await?;
                        let claim_generation = binding
                            .claim_generation
                            .checked_add(1)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        let outcome = tx
                            .compare_and_swap::<AiProviderSessionBindingRecord>(
                                &binding.id,
                                binding.row_version,
                                AiProviderSessionBindingRecordWhereInput {
                                    state: Some(StringFilter {
                                        eq: Some(
                                            AiProviderSessionState::ParkedWait.as_str().to_owned(),
                                        ),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                },
                                UpdateAiProviderSessionBindingRecordInput {
                                    state: Some(
                                        AiProviderSessionState::Claimed.as_str().to_owned(),
                                    ),
                                    claimed_attempt_id: Some(Some(lease.attempt_id())),
                                    claimed_run_lease_generation: Some(Some(
                                        lease.lease_generation(),
                                    )),
                                    claim_owner: Some(Some(lease.worker_id().to_owned())),
                                    claim_generation: Some(claim_generation),
                                    claim_expires_at: Some(Some(claim_expires_at.unix_timestamp())),
                                    parked_reclaimed_at: Some(Some(now.unix_timestamp())),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        let ConditionalUpdateOutcome::Updated(updated) = outcome else {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        };
                        append_audit(
                            tx,
                            "ai.provider_session.wait_reclaimed",
                            binding.id,
                            "provider_session_wait_adoption_reclaimed",
                            lease.run_id().0,
                            now,
                        )
                        .await?;
                        Ok(updated)
                    })
                })
                .await
                .map_err(map_transaction)?;
            claim_from_record(&record, &principal_reference)
        }

        async fn commit_turn(
            &self,
            lease: &AiRunLease,
            claim: &AiProviderSessionClaim,
            commit: AiProviderSessionCommit,
        ) -> Result<AiProviderSessionBindingView, AiError> {
            let (current, observed_session, scope) = self.load_owned_active_context(lease).await?;
            self.protection_policy(current.principal(), &scope).await?;
            let now = canonical_second(self.clock.now());
            let idle_expires_at = now
                .checked_add(self.limits.idle_ttl())
                .ok_or(AiError::PersistenceFailed)?;
            let lease = lease.clone();
            let claim = claim.clone();
            let record = self
                .database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let run = load_and_validate_completed_run(tx, &lease).await?;
                        let binding = tx
                            .find_by_id::<AiProviderSessionBindingRecord>(&claim.binding_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        validate_active_claim(&binding, &claim, &lease, now)?;
                        let session = tx
                            .find_by_id::<AiSessionRecord>(&lease.session_id().0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        let message = tx
                            .find_by_id::<AiMessageRecord>(&commit.assistant_message_id())
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        let checkpoint_id = run
                            .latest_checkpoint_id
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        let checkpoint = tx
                            .query::<AiRunCheckpointRecord>()
                            .filter(AiRunCheckpointRecordWhereInput {
                                id: Some(UuidFilter {
                                    eq: Some(checkpoint_id),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .limit(1)
                            .fetch_one()
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        if session != observed_session
                            || message.session_id != session.id
                            || message.run_id != Some(lease.run_id().0)
                            || message.message_role != "assistant"
                            || message.completion_state != "complete"
                            || message.finalized_at.is_none()
                            || message.protected_preview.is_none()
                            || message.sequence != session.message_head
                            || message.sequence != commit.through_message_sequence()
                            || message.sequence <= binding.through_message_sequence
                            || checkpoint.run_id != lease.run_id().0
                            || checkpoint.attempt_id != lease.attempt_id()
                            || checkpoint.lease_generation != lease.lease_generation()
                            || checkpoint.checkpoint_kind != "assistant_output_persisted"
                            || checkpoint.assistant_message_id != Some(message.id)
                            || checkpoint.protected_state.is_some()
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let bounded_idle_expiry = idle_expires_at
                            .unix_timestamp()
                            .min(binding.absolute_expires_at)
                            .min(binding.provider_expires_at.unwrap_or(i64::MAX));
                        if bounded_idle_expiry <= now.unix_timestamp() {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let outcome = tx
                            .compare_and_swap::<AiProviderSessionBindingRecord>(
                                &binding.id,
                                binding.row_version,
                                AiProviderSessionBindingRecordWhereInput::default(),
                                UpdateAiProviderSessionBindingRecordInput {
                                    through_message_sequence: Some(
                                        commit.through_message_sequence(),
                                    ),
                                    transcript_fingerprint: Some(
                                        commit.transcript_fingerprint().to_owned(),
                                    ),
                                    last_run_id: Some(Some(lease.run_id().0)),
                                    last_assistant_message_id: Some(Some(message.id)),
                                    state: Some(AiProviderSessionState::Active.as_str().to_owned()),
                                    claimed_run_id: Some(None),
                                    claimed_attempt_id: Some(None),
                                    claimed_run_lease_generation: Some(None),
                                    claim_owner: Some(None),
                                    claim_expires_at: Some(None),
                                    idle_expires_at: Some(bounded_idle_expiry),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        match outcome {
                            ConditionalUpdateOutcome::Updated(updated) => Ok(updated),
                            ConditionalUpdateOutcome::NotFound
                            | ConditionalUpdateOutcome::Conflict => {
                                Err(OrmPublicError::new(OrmErrorCode::Conflict))
                            }
                        }
                    })
                })
                .await
                .map_err(map_transaction)?;
            binding_view(&record)
        }

        async fn require_cleanup(
            &self,
            claim: &AiProviderSessionClaim,
            reason_code: &str,
        ) -> Result<(), AiError> {
            validate_reason_code(reason_code)?;
            let now = canonical_second(self.clock.now());
            let claim = claim.clone();
            let reason_code = reason_code.to_owned();
            self.database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let binding = tx
                            .find_by_id::<AiProviderSessionBindingRecord>(&claim.binding_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        validate_claim_record_without_run(&binding, &claim)?;
                        let outcome = tx
                            .compare_and_swap::<AiProviderSessionBindingRecord>(
                                &binding.id,
                                binding.row_version,
                                AiProviderSessionBindingRecordWhereInput::default(),
                                cleanup_required_update(reason_code.clone(), claim.run_id.0),
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(outcome, ConditionalUpdateOutcome::Updated(_)) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        append_audit(
                            tx,
                            "ai.provider_session.cleanup_required",
                            binding.id,
                            &reason_code,
                            claim.run_id.0,
                            now,
                        )
                        .await
                    })
                })
                .await
                .map_err(map_transaction)
        }

        async fn claim_cleanup(
            &self,
            worker_id: &str,
        ) -> Result<Option<AiProviderSessionCleanupClaim>, AiError> {
            validate_worker_id(worker_id)?;
            let now = canonical_second(self.clock.now());
            let expiry = now
                .checked_add(self.limits.cleanup_lease_ttl())
                .ok_or(AiError::PersistenceFailed)?;
            let worker_id = worker_id.to_owned();
            let limit = i64::try_from(self.limits.maximum_candidate_scan())
                .map_err(|_| AiError::InvalidConfiguration("invalid cleanup limit".to_owned()))?;
            let record = self
                .database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let mut candidates = tx
                            .query::<AiProviderSessionBindingRecord>()
                            .filter(AiProviderSessionBindingRecordWhereInput {
                                state: Some(StringFilter {
                                    in_list: Some(vec![
                                        AiProviderSessionState::CleanupRequired.as_str().to_owned(),
                                        AiProviderSessionState::CleanupBackoff.as_str().to_owned(),
                                        AiProviderSessionState::RestoreQuarantined
                                            .as_str()
                                            .to_owned(),
                                    ]),
                                    ..Default::default()
                                }),
                                cleanup_next_attempt_at: Some(IntFilter {
                                    lte: Some(i32::try_from(now.unix_timestamp()).map_err(
                                        |_| OrmPublicError::new(OrmErrorCode::InternalError),
                                    )?),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .default_order()
                            .limit(limit)
                            .fetch_all()
                            .await
                            .map_err(OrmPublicError::from)?;
                        let mut expired_claims = tx
                            .query::<AiProviderSessionBindingRecord>()
                            .filter(AiProviderSessionBindingRecordWhereInput {
                                state: Some(StringFilter {
                                    eq: Some(AiProviderSessionState::Claimed.as_str().to_owned()),
                                    ..Default::default()
                                }),
                                claim_expires_at: Some(IntFilter {
                                    lte: Some(i32::try_from(now.unix_timestamp()).map_err(
                                        |_| OrmPublicError::new(OrmErrorCode::InternalError),
                                    )?),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .default_order()
                            .limit(limit)
                            .fetch_all()
                            .await
                            .map_err(OrmPublicError::from)?;
                        candidates.append(&mut expired_claims);
                        let mut parked_candidates = tx
                            .query::<AiProviderSessionBindingRecord>()
                            .filter(AiProviderSessionBindingRecordWhereInput {
                                state: Some(StringFilter {
                                    eq: Some(
                                        AiProviderSessionState::ParkedWait.as_str().to_owned(),
                                    ),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .default_order()
                            .limit(limit)
                            .fetch_all()
                            .await
                            .map_err(OrmPublicError::from)?;
                        candidates.append(&mut parked_candidates);
                        let mut active_candidates = tx
                            .query::<AiProviderSessionBindingRecord>()
                            .filter(AiProviderSessionBindingRecordWhereInput {
                                state: Some(StringFilter {
                                    eq: Some(AiProviderSessionState::Active.as_str().to_owned()),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .default_order()
                            .limit(limit)
                            .fetch_all()
                            .await
                            .map_err(OrmPublicError::from)?;
                        candidates.append(&mut active_candidates);
                        let mut expired_cleanup_claims = tx
                            .query::<AiProviderSessionBindingRecord>()
                            .filter(AiProviderSessionBindingRecordWhereInput {
                                state: Some(StringFilter {
                                    eq: Some(
                                        AiProviderSessionState::CleanupInProgress
                                            .as_str()
                                            .to_owned(),
                                    ),
                                    ..Default::default()
                                }),
                                cleanup_lease_expires_at: Some(IntFilter {
                                    lte: Some(i32::try_from(now.unix_timestamp()).map_err(
                                        |_| OrmPublicError::new(OrmErrorCode::InternalError),
                                    )?),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .default_order()
                            .limit(limit)
                            .fetch_all()
                            .await
                            .map_err(OrmPublicError::from)?;
                        candidates.append(&mut expired_cleanup_claims);
                        candidates.sort_by_key(|record| (record.updated_at, record.id));
                        candidates.truncate(limit as usize);
                        for candidate in candidates {
                            validate_binding_record(&candidate)?;
                            let state = AiProviderSessionState::from_persisted(&candidate.state)
                                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                            if state == AiProviderSessionState::ParkedWait
                                && candidate.parked_confirmed_at.is_none()
                                && reconcile_parked_confirmation(tx, &candidate, now).await?
                            {
                                continue;
                            }
                            let ready = match state {
                                AiProviderSessionState::CleanupRequired
                                | AiProviderSessionState::RestoreQuarantined => true,
                                AiProviderSessionState::CleanupBackoff => candidate
                                    .cleanup_next_attempt_at
                                    .is_some_and(|value| value <= now.unix_timestamp()),
                                AiProviderSessionState::Claimed => candidate
                                    .claim_expires_at
                                    .is_some_and(|value| value <= now.unix_timestamp()),
                                AiProviderSessionState::ParkedWait => {
                                    let run_id = candidate.claimed_run_id.ok_or_else(|| {
                                        OrmPublicError::new(OrmErrorCode::InternalError)
                                    })?;
                                    let run = tx
                                        .find_by_id::<AiRunRecord>(&run_id)
                                        .await
                                        .map_err(OrmPublicError::from)?;
                                    let terminal = run.as_ref().is_none_or(|run| {
                                        matches!(
                                            AiRunState::from_persisted(&run.state),
                                            Some(
                                                AiRunState::Completed
                                                    | AiRunState::Failed
                                                    | AiRunState::Cancelled
                                                    | AiRunState::RecoveryRequired
                                            )
                                        )
                                    });
                                    terminal
                                        || candidate.parked_confirmed_at.is_none()
                                            && candidate
                                                .claim_expires_at
                                                .is_some_and(|value| value <= now.unix_timestamp())
                                        || candidate.parked_confirmed_at.is_some()
                                            && candidate
                                                .parked_expires_at
                                                .is_some_and(|value| value <= now.unix_timestamp())
                                }
                                AiProviderSessionState::Active => {
                                    candidate.idle_expires_at <= now.unix_timestamp()
                                        || candidate.absolute_expires_at <= now.unix_timestamp()
                                        || candidate
                                            .provider_expires_at
                                            .is_some_and(|value| value <= now.unix_timestamp())
                                }
                                AiProviderSessionState::CleanupInProgress => candidate
                                    .cleanup_lease_expires_at
                                    .is_some_and(|value| value <= now.unix_timestamp()),
                                AiProviderSessionState::Deleted => false,
                            };
                            if !ready {
                                continue;
                            }
                            let generation = candidate
                                .cleanup_generation
                                .checked_add(1)
                                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                            let outcome = tx
                                .compare_and_swap::<AiProviderSessionBindingRecord>(
                                    &candidate.id,
                                    candidate.row_version,
                                    AiProviderSessionBindingRecordWhereInput::default(),
                                    UpdateAiProviderSessionBindingRecordInput {
                                        state: Some(
                                            AiProviderSessionState::CleanupInProgress
                                                .as_str()
                                                .to_owned(),
                                        ),
                                        claimed_run_id: Some(None),
                                        claimed_attempt_id: Some(None),
                                        claimed_run_lease_generation: Some(None),
                                        claim_owner: Some(None),
                                        claim_expires_at: Some(None),
                                        cleanup_owner: Some(Some(worker_id.clone())),
                                        cleanup_generation: Some(generation),
                                        cleanup_lease_expires_at: Some(Some(
                                            expiry.unix_timestamp(),
                                        )),
                                        cleanup_next_attempt_at: Some(None),
                                        cleanup_reason_code: Some(Some(
                                            candidate.cleanup_reason_code.unwrap_or_else(|| {
                                                match state {
                                                    AiProviderSessionState::Claimed => {
                                                        "provider_session_claim_expired".to_owned()
                                                    }
                                                    AiProviderSessionState::ParkedWait => {
                                                        "provider_session_parked_wait_closed"
                                                            .to_owned()
                                                    }
                                                    AiProviderSessionState::CleanupInProgress => {
                                                        "provider_session_cleanup_lease_expired"
                                                            .to_owned()
                                                    }
                                                    _ => "provider_session_expired".to_owned(),
                                                }
                                            }),
                                        )),
                                        ..Default::default()
                                    },
                                )
                                .await
                                .map_err(OrmPublicError::from)?;
                            if let ConditionalUpdateOutcome::Updated(updated) = outcome {
                                return Ok(Some(updated));
                            }
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        Ok(None)
                    })
                })
                .await
                .map_err(map_transaction)?;
            record.as_ref().map(cleanup_claim_from_record).transpose()
        }

        async fn open_for_cleanup(
            &self,
            claim: &AiProviderSessionCleanupClaim,
            policy: &AiContentProtectionPolicy,
        ) -> Result<AiProviderSessionDeletionRequest, AiError> {
            let now = canonical_second(self.clock.now());
            let record =
                AiProviderSessionBindingRecord::find_by_id(&self.database, &claim.binding_id)
                    .await
                    .map_err(|error| map_orm(OrmPublicError::from(error)))?
                    .ok_or(AiError::NotFound)?;
            validate_cleanup_claim(&record, claim, now).map_err(map_orm)?;
            let cursor = self.open_cursor(&record, policy).await?;
            Ok(AiProviderSessionDeletionRequest::new(claim.clone(), cursor))
        }

        async fn complete_cleanup(
            &self,
            claim: &AiProviderSessionCleanupClaim,
            proof: AiProviderSessionAbsenceProof,
        ) -> Result<(), AiError> {
            let now = canonical_second(self.clock.now());
            let observed_at = canonical_second(proof.observed_at());
            if proof.binding_id() != claim.binding_id
                || observed_at > now
                || observed_at < now - Duration::hours(1)
            {
                return Err(AiError::InvalidInput(
                    "invalid provider-session absence proof".to_owned(),
                ));
            }
            let claim = claim.clone();
            self.database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let record = tx
                            .find_by_id::<AiProviderSessionBindingRecord>(&claim.binding_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        validate_cleanup_claim(&record, &claim, now)?;
                        if proof.cursor_fingerprint() != record.cursor_fingerprint {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let outcome = tx
                            .compare_and_swap::<AiProviderSessionBindingRecord>(
                                &record.id,
                                record.row_version,
                                AiProviderSessionBindingRecordWhereInput {
                                    state: Some(StringFilter {
                                        eq: Some(
                                            AiProviderSessionState::CleanupInProgress
                                                .as_str()
                                                .to_owned(),
                                        ),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                },
                                UpdateAiProviderSessionBindingRecordInput {
                                    protected_cursor: Some(None),
                                    state: Some(
                                        AiProviderSessionState::Deleted.as_str().to_owned(),
                                    ),
                                    claimed_run_id: Some(None),
                                    claimed_attempt_id: Some(None),
                                    claimed_run_lease_generation: Some(None),
                                    claim_owner: Some(None),
                                    claim_expires_at: Some(None),
                                    cleanup_owner: Some(None),
                                    cleanup_lease_expires_at: Some(None),
                                    cleanup_next_attempt_at: Some(None),
                                    provider_absence_observed_at: Some(Some(
                                        observed_at.unix_timestamp(),
                                    )),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(outcome, ConditionalUpdateOutcome::Updated(_)) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        append_cleanup_completion_audit(tx, &record, observed_at).await
                    })
                })
                .await
                .map_err(map_transaction)
        }

        async fn schedule_cleanup_retry(
            &self,
            claim: &AiProviderSessionCleanupClaim,
            delay: Duration,
            reason_code: &str,
        ) -> Result<(), AiError> {
            validate_reason_code(reason_code)?;
            if !delay.is_positive() || delay > self.limits.maximum_retry_delay() {
                return Err(AiError::InvalidInput(
                    "invalid provider-session cleanup retry".to_owned(),
                ));
            }
            let now = canonical_second(self.clock.now());
            let next_attempt_at = now.checked_add(delay).ok_or(AiError::PersistenceFailed)?;
            let claim = claim.clone();
            let reason_code = reason_code.to_owned();
            let maximum_retries = self.limits.maximum_retries();
            self.database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let record = tx
                            .find_by_id::<AiProviderSessionBindingRecord>(&claim.binding_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        validate_cleanup_claim(&record, &claim, now)?;
                        let retry_count = record
                            .cleanup_retry_count
                            .checked_add(1)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?
                            .min(i64::from(maximum_retries));
                        let outcome = tx
                            .compare_and_swap::<AiProviderSessionBindingRecord>(
                                &record.id,
                                record.row_version,
                                AiProviderSessionBindingRecordWhereInput::default(),
                                UpdateAiProviderSessionBindingRecordInput {
                                    state: Some(
                                        AiProviderSessionState::CleanupBackoff.as_str().to_owned(),
                                    ),
                                    cleanup_owner: Some(None),
                                    cleanup_lease_expires_at: Some(None),
                                    cleanup_retry_count: Some(retry_count),
                                    cleanup_next_attempt_at: Some(Some(
                                        next_attempt_at.unix_timestamp(),
                                    )),
                                    cleanup_reason_code: Some(Some(reason_code)),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if matches!(outcome, ConditionalUpdateOutcome::Updated(_)) {
                            Ok(())
                        } else {
                            Err(OrmPublicError::new(OrmErrorCode::Conflict))
                        }
                    })
                })
                .await
                .map_err(map_transaction)
        }
    }

    fn cleanup_required_update(
        reason_code: String,
        last_run_id: Uuid,
    ) -> UpdateAiProviderSessionBindingRecordInput {
        UpdateAiProviderSessionBindingRecordInput {
            state: Some(AiProviderSessionState::CleanupRequired.as_str().to_owned()),
            last_run_id: Some(Some(last_run_id)),
            claimed_run_id: Some(None),
            claimed_attempt_id: Some(None),
            claimed_run_lease_generation: Some(None),
            claim_owner: Some(None),
            claim_expires_at: Some(None),
            cleanup_owner: Some(None),
            cleanup_lease_expires_at: Some(None),
            cleanup_next_attempt_at: Some(Some(0)),
            cleanup_reason_code: Some(Some(reason_code)),
            ..Default::default()
        }
    }

    async fn load_and_validate_completed_run(
        tx: &mut MutationContext<'_, DefaultWriteBackend>,
        lease: &AiRunLease,
    ) -> Result<AiRunRecord, OrmPublicError> {
        let run = tx
            .find_by_id::<AiRunRecord>(&lease.run_id().0)
            .await
            .map_err(OrmPublicError::from)?
            .ok_or_else(OrmPublicError::not_found)?;
        let stored_reference: PrincipalReference =
            serde_json::from_value(run.principal_reference.clone())
                .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))?;
        if lease.state() != AiRunState::Running
            || run.session_id != lease.session_id().0
            || run.input_message_id != lease.input_message_id()
            || stored_reference != *lease.principal_reference()
            || run.attempt_id != Some(lease.attempt_id())
            || run.lease_generation != lease.lease_generation()
            || run.retry_count != i64::from(lease.retry_count())
            || run.latest_checkpoint_id != lease.latest_checkpoint_id()
            || run.state != AiRunState::Completed.as_str()
            || run.lease_owner.is_some()
            || run.lease_expires_at.is_some()
        {
            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
        }
        Ok(run)
    }

    fn validate_active_claim(
        record: &AiProviderSessionBindingRecord,
        claim: &AiProviderSessionClaim,
        lease: &AiRunLease,
        now: OffsetDateTime,
    ) -> Result<(), OrmPublicError> {
        validate_claim_record_without_run(record, claim)?;
        if lease.session_id() != claim.session_id
            || lease.run_id() != claim.run_id
            || lease.attempt_id() != claim.attempt_id
            || lease.lease_generation() != claim.run_lease_generation
            || lease.principal_reference() != &claim.principal_reference
            || record.claim_owner.as_deref() != Some(lease.worker_id())
            || record
                .claim_expires_at
                .is_none_or(|expiry| expiry <= now.unix_timestamp())
        {
            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
        }
        Ok(())
    }

    fn validate_provider_turn_checkpoint(
        checkpoint: &AiRunCheckpointRecord,
        binding: &AiProviderSessionBindingRecord,
        lease: &AiRunLease,
        request: &AiProviderSessionWaitParkRequest,
    ) -> Result<(), OrmPublicError> {
        if checkpoint.id != request.source_checkpoint_id
            || checkpoint.checkpoint_hash != request.source_checkpoint_fingerprint
            || checkpoint.run_id != lease.run_id().0
            || checkpoint.attempt_id != lease.attempt_id()
            || checkpoint.lease_generation != lease.lease_generation()
            || checkpoint.checkpoint_kind != "provider_turn_persisted"
            || checkpoint.provider_response_id.is_none()
            || checkpoint.budget_reservation_id.is_none()
            || checkpoint.assistant_message_id.is_some()
            || checkpoint.protected_state.is_none()
            || binding.id != request.binding_id()
            || descriptor_from_record(binding).map_err(ai_error_to_orm)? != *request.descriptor()
            || binding.transcript_fingerprint != request.transcript_fingerprint()
        {
            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
        }
        Ok(())
    }

    fn parked_wait_from_record(
        record: &AiProviderSessionBindingRecord,
    ) -> Result<AiProviderSessionParkedWait, AiError> {
        validate_binding_record(record).map_err(map_orm)?;
        if record.state != AiProviderSessionState::ParkedWait.as_str() {
            return Err(AiError::Conflict);
        }
        Ok(AiProviderSessionParkedWait {
            binding_id: record.id,
            session_id: AiSessionId(record.session_id),
            source_run_id: AiRunId(record.claimed_run_id.ok_or(AiError::PersistenceFailed)?),
            source_attempt_id: record
                .claimed_attempt_id
                .ok_or(AiError::PersistenceFailed)?,
            source_run_lease_generation: record
                .claimed_run_lease_generation
                .ok_or(AiError::PersistenceFailed)?,
            source_binding_claim_generation: record.claim_generation,
            park_generation: record.park_generation,
            wait: persisted_wait_identity(record).map_err(map_orm)?,
            source_checkpoint_id: record
                .parked_source_checkpoint_id
                .ok_or(AiError::PersistenceFailed)?,
            source_checkpoint_fingerprint: record
                .parked_source_checkpoint_fingerprint
                .clone()
                .ok_or(AiError::PersistenceFailed)?,
            continuation_fingerprint: record
                .parked_continuation_fingerprint
                .clone()
                .ok_or(AiError::PersistenceFailed)?,
            binding_row_version: record.row_version,
        })
    }

    fn validate_parked_proof(
        record: &AiProviderSessionBindingRecord,
        parked: &AiProviderSessionParkedWait,
    ) -> Result<(), OrmPublicError> {
        validate_binding_record(record)?;
        if record.id != parked.binding_id
            || record.session_id != parked.session_id.0
            || record.state != AiProviderSessionState::ParkedWait.as_str()
            || record.claimed_run_id != Some(parked.source_run_id.0)
            || record.claimed_attempt_id != Some(parked.source_attempt_id)
            || record.claimed_run_lease_generation != Some(parked.source_run_lease_generation)
            || record.claim_generation != parked.source_binding_claim_generation
            || record.park_generation != parked.park_generation
            || persisted_wait_identity(record)? != parked.wait
            || record.parked_source_checkpoint_id != Some(parked.source_checkpoint_id)
            || record.parked_source_checkpoint_fingerprint.as_deref()
                != Some(parked.source_checkpoint_fingerprint.as_str())
            || record.parked_continuation_fingerprint.as_deref()
                != Some(parked.continuation_fingerprint.as_str())
            || (record.parked_confirmed_at.is_none()
                && record.row_version != parked.binding_row_version)
        {
            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
        }
        Ok(())
    }

    fn validate_wait_handoff_record(
        record: &AiProviderSessionBindingRecord,
        request: &AiProviderSessionWaitParkRequest,
    ) -> Result<(), OrmPublicError> {
        validate_binding_record(record)?;
        let exact_claim = record.id == request.claim.binding_id
            && record.session_id == request.claim.session_id.0
            && record.claimed_run_id == Some(request.claim.run_id.0)
            && record.claimed_attempt_id == Some(request.claim.attempt_id)
            && record.claimed_run_lease_generation == Some(request.claim.run_lease_generation)
            && record.claim_generation == request.claim.binding_claim_generation
            && record.through_message_sequence == request.claim.through_message_sequence
            && record.transcript_fingerprint == request.claim.transcript_fingerprint
            && descriptor_from_record(record).map_err(ai_error_to_orm)? == request.claim.descriptor;
        let exact_state = if record.state == AiProviderSessionState::Claimed.as_str() {
            record.row_version == request.claim.binding_row_version
        } else if record.state == AiProviderSessionState::ParkedWait.as_str() {
            persisted_wait_identity(record)? == request.wait
                && record.parked_source_checkpoint_id == Some(request.source_checkpoint_id)
                && record.parked_source_checkpoint_fingerprint.as_deref()
                    == Some(request.source_checkpoint_fingerprint.as_str())
                && record.parked_continuation_fingerprint.as_deref()
                    == Some(request.continuation_fingerprint.as_str())
                && record.parked_confirmed_at.is_none()
        } else {
            false
        };
        if !exact_claim || !exact_state {
            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
        }
        Ok(())
    }

    fn persisted_wait_identity(
        record: &AiProviderSessionBindingRecord,
    ) -> Result<AiProviderSessionWaitIdentity, OrmPublicError> {
        let kind = record
            .parked_wait_kind
            .as_deref()
            .and_then(AiProviderSessionWaitKind::from_persisted)
            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
        let wait_id = record
            .parked_wait_id
            .filter(|value| !value.is_nil())
            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
        AiProviderSessionWaitIdentity::from_parts(kind, wait_id).map_err(ai_error_to_orm)
    }

    async fn validate_wait_confirmation(
        tx: &mut MutationContext<'_, DefaultWriteBackend>,
        binding: &AiProviderSessionBindingRecord,
        run: &AiRunRecord,
        source_checkpoint: &AiRunCheckpointRecord,
        parked_checkpoint: &AiRunCheckpointRecord,
        wait: AiProviderSessionWaitIdentity,
        now: OffsetDateTime,
    ) -> Result<i64, OrmPublicError> {
        if run.id
            != binding
                .claimed_run_id
                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?
            || run.session_id != binding.session_id
            || source_checkpoint.id
                != binding
                    .parked_source_checkpoint_id
                    .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?
            || parked_checkpoint.id == source_checkpoint.id
            || parked_checkpoint.run_id != run.id
            || parked_checkpoint.provider_response_id != source_checkpoint.provider_response_id
            || parked_checkpoint.budget_reservation_id != source_checkpoint.budget_reservation_id
            || parked_checkpoint.protected_state.is_none()
            || !crate::valid_sha256(&parked_checkpoint.checkpoint_hash)
        {
            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
        }
        match wait.kind() {
            AiProviderSessionWaitKind::Approval => {
                let approval = tx
                    .find_by_id::<AiApprovalRecord>(&wait.wait_id())
                    .await
                    .map_err(OrmPublicError::from)?
                    .ok_or_else(OrmPublicError::not_found)?;
                let call = tx
                    .find_by_id::<AiToolCallRecord>(&approval.tool_call_id)
                    .await
                    .map_err(OrmPublicError::from)?
                    .ok_or_else(OrmPublicError::not_found)?;
                if run.state != AiRunState::WaitingApproval.as_str()
                    || run.attempt_id != Some(source_checkpoint.attempt_id)
                    || run.lease_generation != source_checkpoint.lease_generation
                    || run.latest_checkpoint_id != Some(parked_checkpoint.id)
                    || parked_checkpoint.checkpoint_kind != "approval_wait_parked"
                    || parked_checkpoint.attempt_id != source_checkpoint.attempt_id
                    || parked_checkpoint.lease_generation != source_checkpoint.lease_generation
                    || approval.session_id != run.session_id
                    || approval.tool_call_id != call.id
                    || !matches!(approval.state.as_str(), "pending" | "approved")
                    || approval.consumed_uses != 0
                    || approval.consumed_at.is_some()
                    || approval.expires_at <= now.unix_timestamp()
                    || call.run_id != run.id
                    || call.approval_id != Some(approval.id)
                    || call.state != "waiting_approval"
                    || call.lease_generation != source_checkpoint.lease_generation
                    || call.provider_response_id != source_checkpoint.provider_response_id
                    || call.budget_reservation_id != source_checkpoint.budget_reservation_id
                {
                    return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                }
                Ok(approval.expires_at)
            }
            AiProviderSessionWaitKind::Subscription => {
                let waiter = tx
                    .find_by_id::<AiSubscriptionWaiterRecord>(&wait.wait_id())
                    .await
                    .map_err(OrmPublicError::from)?
                    .ok_or_else(OrmPublicError::not_found)?;
                let call = tx
                    .find_by_id::<AiToolCallRecord>(&waiter.tool_call_id)
                    .await
                    .map_err(OrmPublicError::from)?
                    .ok_or_else(OrmPublicError::not_found)?;
                if run.state != AiRunState::WaitingSubscription.as_str()
                    || run.attempt_id.is_some()
                    || run.latest_checkpoint_id != Some(parked_checkpoint.id)
                    || parked_checkpoint.checkpoint_kind != "subscription_wait_parked"
                    || parked_checkpoint.attempt_id != waiter.source_attempt_id
                    || parked_checkpoint.lease_generation != waiter.source_lease_generation
                    || waiter.run_id != run.id
                    || waiter.session_id != run.session_id
                    || waiter.source_attempt_id != source_checkpoint.attempt_id
                    || waiter.source_lease_generation != source_checkpoint.lease_generation
                    || waiter.source_checkpoint_id != source_checkpoint.id
                    || waiter.source_checkpoint_fingerprint != source_checkpoint.checkpoint_hash
                    || waiter.parked_checkpoint_id != parked_checkpoint.id
                    || waiter.parked_checkpoint_fingerprint != parked_checkpoint.checkpoint_hash
                    || waiter.state != "waiting"
                    || waiter.expires_at <= now.unix_timestamp()
                    || call.id != waiter.tool_call_id
                    || call.run_id != run.id
                    || call.state != "waiting_subscription"
                    || call.completed_at.is_some()
                {
                    return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                }
                Ok(waiter.expires_at)
            }
        }
    }

    async fn reconcile_parked_confirmation(
        tx: &mut MutationContext<'_, DefaultWriteBackend>,
        binding: &AiProviderSessionBindingRecord,
        now: OffsetDateTime,
    ) -> Result<bool, OrmPublicError> {
        let Some(run_id) = binding.claimed_run_id else {
            return Ok(false);
        };
        let Some(source_checkpoint_id) = binding.parked_source_checkpoint_id else {
            return Ok(false);
        };
        let Some(run) = tx
            .find_by_id::<AiRunRecord>(&run_id)
            .await
            .map_err(OrmPublicError::from)?
        else {
            return Ok(false);
        };
        let Some(parked_checkpoint_id) = run.latest_checkpoint_id else {
            return Ok(false);
        };
        let source_checkpoint = tx
            .query::<AiRunCheckpointRecord>()
            .filter(AiRunCheckpointRecordWhereInput {
                id: Some(UuidFilter {
                    eq: Some(source_checkpoint_id),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .limit(1)
            .fetch_one()
            .await
            .map_err(OrmPublicError::from)?;
        let parked_checkpoint = tx
            .query::<AiRunCheckpointRecord>()
            .filter(AiRunCheckpointRecordWhereInput {
                id: Some(UuidFilter {
                    eq: Some(parked_checkpoint_id),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .limit(1)
            .fetch_one()
            .await
            .map_err(OrmPublicError::from)?;
        let (Some(source_checkpoint), Some(parked_checkpoint)) =
            (source_checkpoint, parked_checkpoint)
        else {
            return Ok(false);
        };
        let wait = persisted_wait_identity(binding)?;
        let Ok(wait_expires_at) = validate_wait_confirmation(
            tx,
            binding,
            &run,
            &source_checkpoint,
            &parked_checkpoint,
            wait,
            now,
        )
        .await
        else {
            return Ok(false);
        };
        let outcome = tx
            .compare_and_swap::<AiProviderSessionBindingRecord>(
                &binding.id,
                binding.row_version,
                AiProviderSessionBindingRecordWhereInput {
                    state: Some(StringFilter {
                        eq: Some(AiProviderSessionState::ParkedWait.as_str().to_owned()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                UpdateAiProviderSessionBindingRecordInput {
                    parked_checkpoint_id: Some(Some(parked_checkpoint.id)),
                    parked_checkpoint_fingerprint: Some(Some(parked_checkpoint.checkpoint_hash)),
                    parked_confirmed_at: Some(Some(now.unix_timestamp())),
                    parked_expires_at: Some(Some(wait_expires_at)),
                    claim_owner: Some(None),
                    claim_expires_at: Some(None),
                    ..Default::default()
                },
            )
            .await
            .map_err(OrmPublicError::from)?;
        Ok(matches!(outcome, ConditionalUpdateOutcome::Updated(_)))
    }

    fn validate_confirmed_parked_record(
        record: &AiProviderSessionBindingRecord,
        now: OffsetDateTime,
    ) -> Result<(), OrmPublicError> {
        validate_binding_record(record)?;
        if record.state != AiProviderSessionState::ParkedWait.as_str()
            || record.parked_confirmed_at.is_none()
            || record.parked_checkpoint_id.is_none()
            || record
                .parked_checkpoint_fingerprint
                .as_deref()
                .is_none_or(|value| !crate::valid_sha256(value))
            || record
                .parked_expires_at
                .is_none_or(|expiry| expiry <= now.unix_timestamp())
            || record.parked_reclaimed_at.is_some()
        {
            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
        }
        Ok(())
    }

    async fn validate_wait_reclaim(
        tx: &mut MutationContext<'_, DefaultWriteBackend>,
        binding: &AiProviderSessionBindingRecord,
        run: &AiRunRecord,
        lease: &AiRunLease,
        _now: OffsetDateTime,
    ) -> Result<(), OrmPublicError> {
        let wait = persisted_wait_identity(binding)?;
        if run.id
            != binding
                .claimed_run_id
                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?
            || run.latest_checkpoint_id.is_some()
        {
            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
        }
        match wait.kind() {
            AiProviderSessionWaitKind::Approval => {
                let approval = tx
                    .find_by_id::<AiApprovalRecord>(&wait.wait_id())
                    .await
                    .map_err(OrmPublicError::from)?
                    .ok_or_else(OrmPublicError::not_found)?;
                let call = tx
                    .find_by_id::<AiToolCallRecord>(&approval.tool_call_id)
                    .await
                    .map_err(OrmPublicError::from)?
                    .ok_or_else(OrmPublicError::not_found)?;
                if approval.session_id != run.session_id
                    || approval.state != "consumed"
                    || approval.maximum_uses != 1
                    || approval.consumed_uses != 1
                    || approval.consumed_at.is_none()
                    || call.run_id != run.id
                    || call.state != "completed"
                    || call.approval_id != Some(approval.id)
                {
                    return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                }
            }
            AiProviderSessionWaitKind::Subscription => {
                let waiter = tx
                    .find_by_id::<AiSubscriptionWaiterRecord>(&wait.wait_id())
                    .await
                    .map_err(OrmPublicError::from)?
                    .ok_or_else(OrmPublicError::not_found)?;
                let adoptions = tx
                    .query::<AiSubscriptionWaitAdoptionRecord>()
                    .filter(AiSubscriptionWaitAdoptionRecordWhereInput {
                        waiter_id: Some(UuidFilter {
                            eq: Some(waiter.id),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .limit(2)
                    .fetch_all()
                    .await
                    .map_err(OrmPublicError::from)?;
                let adoption = adoptions
                    .first()
                    .filter(|_| adoptions.len() == 1)
                    .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                if waiter.run_id != run.id
                    || waiter.state != "adopted"
                    || adoption.waiter_id != waiter.id
                    || adoption.run_id != run.id
                    || adoption.state != "consumed"
                    || adoption.claimed_attempt_id != Some(lease.attempt_id())
                    || adoption.claimed_lease_generation != Some(lease.lease_generation())
                    || adoption.consumed_at.is_none()
                {
                    return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                }
            }
        }
        Ok(())
    }

    fn validate_claim_record_without_run(
        record: &AiProviderSessionBindingRecord,
        claim: &AiProviderSessionClaim,
    ) -> Result<(), OrmPublicError> {
        validate_binding_record(record)?;
        if record.id != claim.binding_id
            || record.session_id != claim.session_id.0
            || record.state != AiProviderSessionState::Claimed.as_str()
            || record.claimed_run_id != Some(claim.run_id.0)
            || record.claimed_attempt_id != Some(claim.attempt_id)
            || record.claimed_run_lease_generation != Some(claim.run_lease_generation)
            || record.claim_generation != claim.binding_claim_generation
            || record.row_version != claim.binding_row_version
            || record.through_message_sequence != claim.through_message_sequence
            || record.transcript_fingerprint != claim.transcript_fingerprint
            || descriptor_from_record(record).map_err(ai_error_to_orm)? != claim.descriptor
        {
            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
        }
        Ok(())
    }

    fn validate_cleanup_claim(
        record: &AiProviderSessionBindingRecord,
        claim: &AiProviderSessionCleanupClaim,
        now: OffsetDateTime,
    ) -> Result<(), OrmPublicError> {
        validate_binding_record(record)?;
        if record.id != claim.binding_id
            || record.session_id != claim.session_id.0
            || record_scope_from_binding(record) != claim.scope
            || descriptor_from_record(record).map_err(ai_error_to_orm)? != claim.descriptor
            || record.state != AiProviderSessionState::CleanupInProgress.as_str()
            || record.cleanup_owner.as_deref() != Some(claim.cleanup_worker_id.as_str())
            || record.cleanup_generation != claim.cleanup_generation
            || record.row_version != claim.row_version
            || record
                .cleanup_lease_expires_at
                .is_none_or(|expiry| expiry <= now.unix_timestamp())
        {
            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
        }
        Ok(())
    }

    fn claim_from_record(
        record: &AiProviderSessionBindingRecord,
        principal_reference: &PrincipalReference,
    ) -> Result<AiProviderSessionClaim, AiError> {
        validate_binding_record(record).map_err(map_orm)?;
        let persisted_reference: PrincipalReference =
            serde_json::from_value(record.principal_reference.clone())
                .map_err(|_| AiError::PersistenceFailed)?;
        if &persisted_reference != principal_reference
            || record.state != AiProviderSessionState::Claimed.as_str()
        {
            return Err(AiError::Conflict);
        }
        Ok(AiProviderSessionClaim {
            binding_id: record.id,
            session_id: AiSessionId(record.session_id),
            run_id: AiRunId(record.claimed_run_id.ok_or(AiError::PersistenceFailed)?),
            attempt_id: record
                .claimed_attempt_id
                .ok_or(AiError::PersistenceFailed)?,
            run_lease_generation: record
                .claimed_run_lease_generation
                .ok_or(AiError::PersistenceFailed)?,
            binding_claim_generation: record.claim_generation,
            binding_row_version: record.row_version,
            claim_expires_at: parse_time(record.claim_expires_at)?,
            through_message_sequence: record.through_message_sequence,
            transcript_fingerprint: record.transcript_fingerprint.clone(),
            principal_reference: persisted_reference,
            descriptor: descriptor_from_record(record)?,
        })
    }

    fn cleanup_claim_from_record(
        record: &AiProviderSessionBindingRecord,
    ) -> Result<AiProviderSessionCleanupClaim, AiError> {
        validate_binding_record(record).map_err(map_orm)?;
        if record.state != AiProviderSessionState::CleanupInProgress.as_str() {
            return Err(AiError::Conflict);
        }
        Ok(AiProviderSessionCleanupClaim {
            binding_id: record.id,
            session_id: AiSessionId(record.session_id),
            scope: record_scope_from_binding(record),
            descriptor: descriptor_from_record(record)?,
            cleanup_worker_id: record
                .cleanup_owner
                .clone()
                .ok_or(AiError::PersistenceFailed)?,
            cleanup_generation: record.cleanup_generation,
            cleanup_expires_at: parse_time(record.cleanup_lease_expires_at)?,
            row_version: record.row_version,
        })
    }

    fn binding_view(
        record: &AiProviderSessionBindingRecord,
    ) -> Result<AiProviderSessionBindingView, AiError> {
        validate_binding_record(record).map_err(map_orm)?;
        Ok(AiProviderSessionBindingView {
            binding_id: record.id,
            session_id: AiSessionId(record.session_id),
            scope: record_scope_from_binding(record),
            descriptor: descriptor_from_record(record)?,
            state: AiProviderSessionState::from_persisted(&record.state)
                .ok_or(AiError::PersistenceFailed)?,
            through_message_sequence: record.through_message_sequence,
            transcript_fingerprint: record.transcript_fingerprint.clone(),
            provider_expires_at: record
                .provider_expires_at
                .map(parse_required_time)
                .transpose()?,
            idle_expires_at: parse_required_time(record.idle_expires_at)?,
            absolute_expires_at: parse_required_time(record.absolute_expires_at)?,
            row_version: record.row_version,
        })
    }

    fn descriptor_from_record(
        record: &AiProviderSessionBindingRecord,
    ) -> Result<AiProviderSessionDescriptor, AiError> {
        AiProviderSessionDescriptor::new(
            parse_provider_kind(&record.provider_kind)?,
            record.provider_profile_id.clone(),
            record.provider_model.clone(),
            record.registration_fingerprint.clone(),
            record.protocol_version.clone(),
            record.policy_fingerprint.clone(),
        )
    }

    fn record_scope_from_binding(record: &AiProviderSessionBindingRecord) -> AiScope {
        AiScope {
            kind: record.scope_kind.clone(),
            id: record.scope_id.clone(),
            tenant_id: record.tenant_id.clone(),
        }
    }

    fn validate_binding_record(
        record: &AiProviderSessionBindingRecord,
    ) -> Result<(), OrmPublicError> {
        let scope = record_scope_from_binding(record);
        let state = AiProviderSessionState::from_persisted(&record.state)
            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
        let principal_reference =
            serde_json::from_value::<PrincipalReference>(record.principal_reference.clone())
                .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))?;
        descriptor_from_record(record).map_err(ai_error_to_orm)?;
        let common_valid = !record.id.is_nil()
            && !record.session_id.is_nil()
            && !record.owner_principal_kind.is_empty()
            && !record.owner_subject.is_empty()
            && ai_scope_key(&scope) == record.scope_key
            && !principal_reference.subject.is_empty()
            && crate::valid_sha256(&record.cursor_fingerprint)
            && crate::valid_sha256(&record.transcript_fingerprint)
            && record.through_message_sequence >= 0
            && record.claim_generation >= 0
            && record.park_generation >= 0
            && record.cleanup_generation >= 0
            && record.cleanup_retry_count >= 0
            && record.absolute_expires_at > 0
            && record.idle_expires_at > 0
            && record.idle_expires_at <= record.absolute_expires_at
            && record
                .cleanup_reason_code
                .as_deref()
                .is_none_or(|reason| validate_reason_code(reason).is_ok())
            && record.row_version >= 0;
        let state_valid = match state {
            AiProviderSessionState::Active => {
                record.protected_cursor.is_some()
                    && record.claimed_run_id.is_none()
                    && record.claimed_attempt_id.is_none()
                    && record.claimed_run_lease_generation.is_none()
                    && record.claim_owner.is_none()
                    && record.claim_expires_at.is_none()
                    && record.cleanup_owner.is_none()
                    && record.cleanup_lease_expires_at.is_none()
            }
            AiProviderSessionState::Claimed => {
                record.protected_cursor.is_some()
                    && record.claimed_run_id.is_some()
                    && record.claimed_attempt_id.is_some()
                    && record
                        .claimed_run_lease_generation
                        .is_some_and(|value| value > 0)
                    && record
                        .claim_owner
                        .as_deref()
                        .is_some_and(|value| !value.is_empty())
                    && record.claim_expires_at.is_some()
                    && record.cleanup_owner.is_none()
                    && record.cleanup_lease_expires_at.is_none()
            }
            AiProviderSessionState::ParkedWait => {
                record.protected_cursor.is_some()
                    && record.claimed_run_id.is_some()
                    && record.claimed_attempt_id.is_some()
                    && record
                        .claimed_run_lease_generation
                        .is_some_and(|value| value > 0)
                    && record.park_generation > 0
                    && record.parked_wait_kind.as_deref().is_some_and(|value| {
                        AiProviderSessionWaitKind::from_persisted(value).is_some()
                    })
                    && record.parked_wait_id.is_some_and(|value| !value.is_nil())
                    && record
                        .parked_source_checkpoint_id
                        .is_some_and(|value| !value.is_nil())
                    && record
                        .parked_source_checkpoint_fingerprint
                        .as_deref()
                        .is_some_and(crate::valid_sha256)
                    && record
                        .parked_continuation_fingerprint
                        .as_deref()
                        .is_some_and(crate::valid_sha256)
                    && record.cleanup_owner.is_none()
                    && record.cleanup_lease_expires_at.is_none()
                    && match record.parked_confirmed_at {
                        None => {
                            record
                                .claim_owner
                                .as_deref()
                                .is_some_and(|value| !value.is_empty())
                                && record.claim_expires_at.is_some()
                                && record.parked_checkpoint_id.is_none()
                                && record.parked_checkpoint_fingerprint.is_none()
                        }
                        Some(_) => {
                            record.claim_owner.is_none()
                                && record.claim_expires_at.is_none()
                                && record
                                    .parked_checkpoint_id
                                    .is_some_and(|value| !value.is_nil())
                                && record
                                    .parked_checkpoint_fingerprint
                                    .as_deref()
                                    .is_some_and(crate::valid_sha256)
                                && record.parked_expires_at.is_some()
                        }
                    }
            }
            AiProviderSessionState::CleanupRequired
            | AiProviderSessionState::CleanupBackoff
            | AiProviderSessionState::RestoreQuarantined => {
                record.protected_cursor.is_some()
                    && record.claimed_run_id.is_none()
                    && record.claimed_attempt_id.is_none()
                    && record.claimed_run_lease_generation.is_none()
                    && record.claim_owner.is_none()
                    && record.claim_expires_at.is_none()
                    && record.cleanup_owner.is_none()
                    && record.cleanup_lease_expires_at.is_none()
                    && record.cleanup_reason_code.is_some()
            }
            AiProviderSessionState::CleanupInProgress => {
                record.protected_cursor.is_some()
                    && record.claimed_run_id.is_none()
                    && record.claimed_attempt_id.is_none()
                    && record.claimed_run_lease_generation.is_none()
                    && record.claim_owner.is_none()
                    && record.claim_expires_at.is_none()
                    && record
                        .cleanup_owner
                        .as_deref()
                        .is_some_and(|value| !value.is_empty())
                    && record.cleanup_lease_expires_at.is_some()
                    && record.cleanup_reason_code.is_some()
            }
            AiProviderSessionState::Deleted => {
                record.protected_cursor.is_none()
                    && record.provider_absence_observed_at.is_some()
                    && record.claimed_run_id.is_none()
                    && record.cleanup_owner.is_none()
            }
        };
        if common_valid && state_valid {
            Ok(())
        } else {
            Err(OrmPublicError::new(OrmErrorCode::InternalError))
        }
    }

    fn binding_hash(
        binding_id: Uuid,
        session_id: AiSessionId,
        owner_principal_kind: &str,
        owner_subject: &str,
        scope: &AiScope,
        descriptor: &AiProviderSessionDescriptor,
        cursor_fingerprint: &str,
    ) -> Result<String, AiError> {
        let value = serde_json::json!({
            "format": "graphql-orm-ai/provider-session-binding/v1",
            "bindingId": binding_id,
            "sessionId": session_id.0,
            "ownerPrincipalKind": owner_principal_kind,
            "ownerSubject": owner_subject,
            "scope": scope,
            "descriptor": descriptor,
            "cursorFingerprint": cursor_fingerprint,
        });
        let bytes = serde_json::to_vec(&value).map_err(|_| AiError::PersistenceFailed)?;
        Ok(hex::encode(sha2::Sha256::digest(bytes)))
    }

    async fn append_audit(
        tx: &mut graphql_orm::graphql::orm::MutationContext<'_, DefaultWriteBackend>,
        action: &str,
        binding_id: Uuid,
        reason_code: &str,
        causation_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<(), OrmPublicError> {
        tx.insert::<AiAuditEventRecord>(CreateAiAuditEventRecordInput {
            actor_principal_kind: "system".to_owned(),
            actor_subject: "provider-session-lifecycle".to_owned(),
            action: action.to_owned(),
            resource_kind: "ai_provider_session".to_owned(),
            resource_reference: binding_id.to_string(),
            outcome: "allowed".to_owned(),
            reason_code: reason_code.to_owned(),
            correlation_id: binding_id.to_string(),
            causation_id: Some(causation_id.to_string()),
            policy_version: None,
        })
        .await
        .map_err(OrmPublicError::from)?;
        let _ = now;
        Ok(())
    }

    async fn append_cleanup_completion_audit(
        tx: &mut graphql_orm::graphql::orm::MutationContext<'_, DefaultWriteBackend>,
        record: &AiProviderSessionBindingRecord,
        observed_at: OffsetDateTime,
    ) -> Result<(), OrmPublicError> {
        let reason_code = record
            .cleanup_reason_code
            .as_deref()
            .unwrap_or("provider_session_absence_confirmed");
        tx.insert::<AiAuditEventRecord>(CreateAiAuditEventRecordInput {
            actor_principal_kind: "system".to_owned(),
            actor_subject: "provider-session-lifecycle".to_owned(),
            action: "ai.provider_session.deleted".to_owned(),
            resource_kind: "ai_provider_session".to_owned(),
            resource_reference: record.id.to_string(),
            outcome: "allowed".to_owned(),
            reason_code: reason_code.to_owned(),
            correlation_id: format!(
                "claim-{}:cleanup-{}:absence-{}",
                record.claim_generation,
                record.cleanup_generation,
                observed_at.unix_timestamp()
            ),
            causation_id: Some(record.last_run_id.unwrap_or(record.session_id).to_string()),
            policy_version: None,
        })
        .await
        .map_err(OrmPublicError::from)?;
        Ok(())
    }

    fn validate_reason_code(reason_code: &str) -> Result<(), AiError> {
        if reason_code.is_empty()
            || reason_code.len() > MAXIMUM_SAFE_REASON_CODE_BYTES
            || !reason_code.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'_' | b'-')
            })
        {
            return Err(AiError::InvalidInput(
                "invalid provider-session reason code".to_owned(),
            ));
        }
        Ok(())
    }

    fn parse_time(value: Option<i64>) -> Result<OffsetDateTime, AiError> {
        value
            .and_then(|value| OffsetDateTime::from_unix_timestamp(value).ok())
            .ok_or(AiError::PersistenceFailed)
    }

    fn parse_required_time(value: i64) -> Result<OffsetDateTime, AiError> {
        OffsetDateTime::from_unix_timestamp(value).map_err(|_| AiError::PersistenceFailed)
    }

    fn ai_error_to_orm(_error: AiError) -> OrmPublicError {
        OrmPublicError::new(OrmErrorCode::InternalError)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        use std::sync::Arc;

        use agql_auth::{
            AccessTokenMetadata, AuthUser, FixedClock, ResolvedPrincipal, SessionContext,
        };
        use graphql_orm::graphql::orm::{ApplyOptions, OrmSchemaModule};
        use graphql_orm::prelude::SqliteBackend;

        use crate::AiProviderCallResult;

        struct TestAccess;

        #[async_trait]
        impl AiAccessPolicy for TestAccess {
            async fn can_access_scope(
                &self,
                _principal: &AuthPrincipal,
                _scope: &AiScope,
                _action: AiSessionAction,
            ) -> crate::AiAccessDecision {
                crate::AiAccessDecision::allow("parked-wait-test", "v1")
            }

            async fn can_access_session(
                &self,
                _principal: &AuthPrincipal,
                _session_id: AiSessionId,
                _action: AiSessionAction,
            ) -> crate::AiAccessDecision {
                crate::AiAccessDecision::allow("parked-wait-test", "v1")
            }
        }

        struct TestProtection;

        #[async_trait]
        impl AiContentProtectionPolicyResolver for TestProtection {
            async fn resolve(
                &self,
                _principal: &AuthPrincipal,
                scope: &AiScope,
            ) -> Result<AiContentProtectionPolicy, AiError> {
                Ok(AiContentProtectionPolicy {
                    scope: scope.clone(),
                    mode: crate::AiContentProtectionMode::DatabaseManaged,
                    key_policy_reference: None,
                    version: 1,
                    ready: true,
                })
            }
        }

        struct TestPrincipalResolver {
            principal: AuthPrincipal,
            clock: Arc<FixedClock>,
        }

        #[async_trait]
        impl CurrentPrincipalResolver for TestPrincipalResolver {
            async fn resolve(
                &self,
                reference: &PrincipalReference,
            ) -> agql_auth::AuthResult<ResolvedPrincipal> {
                if reference != &self.principal.reference() {
                    return Err(agql_auth::AuthError::Forbidden);
                }
                ResolvedPrincipal::new(reference.clone(), self.principal.clone(), self.clock.now())
            }
        }

        struct ParkFixture {
            service: Arc<OrmAiProviderSessionService>,
            database: Database<SqliteBackend>,
            clock: Arc<FixedClock>,
            lease: AiRunLease,
            request: AiProviderSessionWaitParkRequest,
            approval_id: crate::AiApprovalId,
            source_checkpoint: AiRunCheckpointRecord,
            binding_id: Uuid,
        }

        async fn parked_wait_fixture() -> ParkFixture {
            let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
                .await
                .expect("parked-wait SQLite should open");
            let plan = database
                .schema()
                .plan_migration_to_entities(
                    "provider-session-parked-wait-test",
                    "provider session parked wait test",
                    crate::AiSchemaModule.entities(),
                )
                .await
                .expect("AI schema should plan");
            database
                .schema()
                .apply_migration(&plan, ApplyOptions::default())
                .await
                .expect("AI schema should apply");
            let now = OffsetDateTime::now_utc();
            let clock = Arc::new(FixedClock::new(now));
            let principal = AuthPrincipal::User(AuthUser {
                user_id: "parked-wait-owner".to_owned(),
                session_id: Uuid::new_v4(),
                roles: Vec::new(),
                scopes: Vec::new(),
                session: SessionContext::default(),
                token_claims: AccessTokenMetadata {
                    tenant_id: Some("parked-wait-tenant".to_owned()),
                    ..AccessTokenMetadata::default()
                },
            });
            let scope = AiScope::new("workspace", "parked-wait-workspace")
                .with_tenant_id("parked-wait-tenant");
            let service = Arc::new(
                OrmAiProviderSessionService::new(
                    database.clone(),
                    Arc::new(TestAccess),
                    Arc::new(TestProtection),
                    Arc::new(crate::DatabaseManagedContentProtector),
                    Arc::new(TestPrincipalResolver {
                        principal: principal.clone(),
                        clock: clock.clone(),
                    }),
                    clock.clone(),
                    AiProviderSessionLimits::default(),
                    Duration::minutes(5),
                )
                .expect("provider-session service should construct"),
            );
            let session_id = AiSessionId::new();
            let run_id = AiRunId::new();
            let input_message_id = Uuid::new_v4();
            let attempt_id = Uuid::new_v4();
            let source_checkpoint_id = Uuid::new_v4();
            let budget_id = Uuid::new_v4();
            let source_checkpoint_hash = "d".repeat(64);
            AiSessionRecord::insert(
                &database,
                crate::persistence::CreateAiSessionRecordInput {
                    id: session_id.0,
                    owner_principal_kind: "user".to_owned(),
                    owner_subject: principal.subject().to_owned(),
                    tenant_id: scope.tenant_id.clone(),
                    scope_kind: scope.kind.clone(),
                    scope_id: scope.id.clone(),
                    title: "Parked wait test".to_owned(),
                    title_revision: 0,
                    title_source: "default".to_owned(),
                    state: "active".to_owned(),
                    stream_head: 0,
                    message_head: 1,
                    last_activity_at: now.unix_timestamp(),
                    archived_at: None,
                    deleted_at: None,
                },
            )
            .await
            .expect("session should insert");
            AiMessageRecord::insert(
                &database,
                crate::persistence::CreateAiMessageRecordInput {
                    id: input_message_id,
                    session_id: session_id.0,
                    sequence: 1,
                    message_role: "user".to_owned(),
                    author_principal_kind: Some("user".to_owned()),
                    author_subject: Some(principal.subject().to_owned()),
                    client_message_id: Some(Uuid::new_v4()),
                    content_hash: Some("1".repeat(64)),
                    run_id: Some(run_id.0),
                    provider_kind: None,
                    provider_model: None,
                    protected_preview: None,
                    block_count: 1,
                    completion_state: "complete".to_owned(),
                    finalized_at: Some(now.unix_timestamp()),
                    content_purged_at: None,
                },
            )
            .await
            .expect("input message should insert");
            AiRunRecord::insert(
                &database,
                crate::persistence::CreateAiRunRecordInput {
                    id: run_id.0,
                    session_id: session_id.0,
                    input_message_id,
                    principal_reference: serde_json::to_value(principal.reference())
                        .expect("principal reference should serialize"),
                    state: AiRunState::Running.as_str().to_owned(),
                    attempt_id: Some(attempt_id),
                    lease_owner: Some("parked-source-worker".to_owned()),
                    lease_generation: 1,
                    lease_expires_at: Some((now + Duration::minutes(5)).unix_timestamp()),
                    lease_heartbeat_at: Some(now.unix_timestamp()),
                    retry_count: 0,
                    next_attempt_at: None,
                    error_code: None,
                    latest_checkpoint_id: Some(source_checkpoint_id),
                    cancellation_request_id: None,
                    cancellation_requested_at: None,
                },
            )
            .await
            .expect("run should insert");
            let source_checkpoint = AiRunCheckpointRecord::insert(
                &database,
                crate::persistence::CreateAiRunCheckpointRecordInput {
                    id: source_checkpoint_id,
                    run_id: run_id.0,
                    attempt_id,
                    lease_generation: 1,
                    checkpoint_kind: "provider_turn_persisted".to_owned(),
                    provider_response_id: Some("parked-response".to_owned()),
                    budget_reservation_id: Some(budget_id),
                    assistant_message_id: None,
                    protected_state: Some(serde_json::json!({"protected": true})),
                    checkpoint_hash: source_checkpoint_hash.clone(),
                },
            )
            .await
            .expect("source checkpoint should insert");
            let run = AiRunRecord::find_by_id(&database, &run_id.0)
                .await
                .expect("run lookup should succeed")
                .expect("run should exist");
            let lease = crate::orm_runs::lease_from_record(&run)
                .expect("seeded running lease should validate");
            let descriptor = AiProviderSessionDescriptor::new(
                crate::ProviderKind::OpenAi,
                "parked-wait-profile",
                "coordinator-test-model",
                "a".repeat(64),
                "responses/v1",
                "b".repeat(64),
            )
            .expect("descriptor should validate");
            let binding_id = Uuid::new_v4();
            let policy = TestProtection
                .resolve(&principal, &scope)
                .await
                .expect("test policy should resolve");
            let (protected_cursor, cursor_kind, cursor_fingerprint) = service
                .protect_cursor(
                    binding_id,
                    session_id,
                    "user",
                    principal.subject(),
                    &scope,
                    &policy,
                    &descriptor,
                    AiProviderSessionCursor::new("test.thread", "parked-thread")
                        .expect("cursor should validate"),
                )
                .await
                .expect("cursor should protect");
            let record = AiProviderSessionBindingRecord::insert(
                &database,
                CreateAiProviderSessionBindingRecordInput {
                    id: binding_id,
                    session_id: session_id.0,
                    owner_principal_kind: "user".to_owned(),
                    owner_subject: principal.subject().to_owned(),
                    principal_reference: serde_json::to_value(principal.reference())
                        .expect("principal reference should serialize"),
                    scope_key: ai_scope_key(&scope),
                    scope_kind: scope.kind.clone(),
                    scope_id: scope.id.clone(),
                    tenant_id: scope.tenant_id.clone(),
                    provider_kind: "open_ai".to_owned(),
                    provider_profile_id: "parked-wait-profile".to_owned(),
                    provider_model: "coordinator-test-model".to_owned(),
                    registration_fingerprint: "a".repeat(64),
                    protocol_version: "responses/v1".to_owned(),
                    policy_fingerprint: "b".repeat(64),
                    cursor_kind,
                    cursor_fingerprint,
                    protected_cursor: Some(protected_cursor),
                    through_message_sequence: 0,
                    transcript_fingerprint: "c".repeat(64),
                    last_run_id: None,
                    last_assistant_message_id: None,
                    state: AiProviderSessionState::Claimed.as_str().to_owned(),
                    claimed_run_id: Some(run_id.0),
                    claimed_attempt_id: Some(attempt_id),
                    claimed_run_lease_generation: Some(1),
                    claim_owner: Some(lease.worker_id().to_owned()),
                    claim_generation: 1,
                    claim_expires_at: Some(lease.lease_expires_at().unix_timestamp()),
                    parked_wait_kind: None,
                    parked_wait_id: None,
                    park_generation: 0,
                    parked_source_checkpoint_id: None,
                    parked_source_checkpoint_fingerprint: None,
                    parked_checkpoint_id: None,
                    parked_checkpoint_fingerprint: None,
                    parked_continuation_fingerprint: None,
                    parked_confirmed_at: None,
                    parked_expires_at: None,
                    parked_reclaimed_at: None,
                    provider_expires_at: None,
                    idle_expires_at: (now + Duration::hours(1)).unix_timestamp(),
                    absolute_expires_at: (now + Duration::hours(2)).unix_timestamp(),
                    cleanup_owner: None,
                    cleanup_generation: 0,
                    cleanup_lease_expires_at: None,
                    cleanup_retry_count: 0,
                    cleanup_next_attempt_at: None,
                    cleanup_reason_code: None,
                    provider_absence_observed_at: None,
                },
            )
            .await
            .expect("provider binding should insert");
            let claim = claim_from_record(&record, lease.principal_reference())
                .expect("claim should validate");
            let approval_id = crate::AiApprovalId::new();
            let result = AiProviderCallResult::test_result(
                &lease,
                None,
                "parked-response",
                vec![(
                    "parked-call",
                    "records.update",
                    serde_json::json!({"id": 7}),
                )],
            )
            .test_with_provider_session_claim(claim.clone());
            let request = result
                .provider_session_wait_park_request(
                    &lease,
                    AiProviderSessionWaitIdentity::approval(approval_id),
                    source_checkpoint_id,
                    source_checkpoint_hash,
                )
                .expect("opaque request should construct");
            ParkFixture {
                service,
                database,
                clock,
                lease,
                request,
                approval_id,
                source_checkpoint,
                binding_id,
            }
        }

        async fn persist_approval_wait_graph(
            fixture: &ParkFixture,
            parked: &AiProviderSessionParkedWait,
        ) -> Uuid {
            let now = fixture.clock.now();
            let tool_call_id = Uuid::new_v4();
            AiToolCallRecord::insert(
                &fixture.database,
                crate::persistence::CreateAiToolCallRecordInput {
                    id: tool_call_id,
                    run_id: fixture.lease.run_id().0,
                    provider_call_key: format!("parked-call-{tool_call_id}"),
                    provider_call_id: "parked-call".to_owned(),
                    provider_kind: Some("open_ai".to_owned()),
                    provider_model: Some("coordinator-test-model".to_owned()),
                    provider_response_id: fixture.source_checkpoint.provider_response_id.clone(),
                    budget_reservation_id: fixture.source_checkpoint.budget_reservation_id,
                    provider_turn_index: 0,
                    tool_call_index: 0,
                    tool_id: "records.update".to_owned(),
                    tool_fingerprint: "e".repeat(64),
                    protected_arguments: Some(serde_json::json!({"protected": true})),
                    argument_hash: "f".repeat(64),
                    protected_result: None,
                    payload_purged_at: None,
                    risk: "high_impact".to_owned(),
                    authorization_code: None,
                    authorization_policy_version: None,
                    authorization_state_digest: None,
                    disclosure_schema_fingerprint: None,
                    result_classification: None,
                    result_egress_decision_id: None,
                    result_egress_manifest_hash: None,
                    application_audit_ref: None,
                    approval_id: Some(fixture.approval_id.0),
                    idempotency_key: Some(tool_call_id.to_string()),
                    correlation_id: Some("parked-wait-correlation".to_owned()),
                    causation_id: Some(fixture.source_checkpoint.id.to_string()),
                    delegation_reference: None,
                    lease_generation: fixture.lease.lease_generation(),
                    state: "waiting_approval".to_owned(),
                    completed_at: None,
                },
            )
            .await
            .expect("approval tool call should insert");
            AiApprovalRecord::insert(
                &fixture.database,
                crate::persistence::CreateAiApprovalRecordInput {
                    id: fixture.approval_id.0,
                    tool_call_id,
                    session_id: fixture.lease.session_id().0,
                    principal_subject: "parked-wait-owner".to_owned(),
                    principal_reference_fingerprint: "1".repeat(64),
                    delegated_actor_subject: None,
                    delegation_reference: None,
                    argument_hash: "f".repeat(64),
                    tool_fingerprint: "e".repeat(64),
                    binding_hash: "2".repeat(64),
                    execution_target_id: "local-graphql".to_owned(),
                    target_schema_fingerprint: "3".repeat(64),
                    operation_name: "UpdateRecord".to_owned(),
                    operation_document_hash: "4".repeat(64),
                    result_projection_fingerprint: "5".repeat(64),
                    disclosure_schema_fingerprint: "6".repeat(64),
                    policy_version: "approval-v1".to_owned(),
                    authorization_state_digest: "7".repeat(64),
                    protected_resource_bindings: Some(serde_json::json!({"protected": true})),
                    protected_action_preview: Some(serde_json::json!({"protected": true})),
                    payload_purged_at: None,
                    action_preview_hash: "8".repeat(64),
                    state: "pending".to_owned(),
                    recent_mfa_required: false,
                    approver_subject: None,
                    expires_at: (now + Duration::minutes(10)).unix_timestamp(),
                    decided_at: None,
                    maximum_uses: 1,
                    consumed_uses: 0,
                    consumed_at: None,
                },
            )
            .await
            .expect("approval should insert");
            let parked_checkpoint_id = Uuid::new_v4();
            AiRunCheckpointRecord::insert(
                &fixture.database,
                crate::persistence::CreateAiRunCheckpointRecordInput {
                    id: parked_checkpoint_id,
                    run_id: fixture.lease.run_id().0,
                    attempt_id: fixture.lease.attempt_id(),
                    lease_generation: fixture.lease.lease_generation(),
                    checkpoint_kind: "approval_wait_parked".to_owned(),
                    provider_response_id: fixture.source_checkpoint.provider_response_id.clone(),
                    budget_reservation_id: fixture.source_checkpoint.budget_reservation_id,
                    assistant_message_id: None,
                    protected_state: Some(serde_json::json!({
                        "waitId": parked.wait().wait_id(),
                        "continuationFingerprint": parked.continuation_fingerprint,
                    })),
                    checkpoint_hash: "9".repeat(64),
                },
            )
            .await
            .expect("parked checkpoint should insert");
            let run = AiRunRecord::find_by_id(&fixture.database, &fixture.lease.run_id().0)
                .await
                .expect("run lookup should succeed")
                .expect("run should exist");
            let updated = AiRunRecord::compare_and_swap(
                &fixture.database,
                &run.id,
                run.row_version,
                crate::persistence::AiRunRecordWhereInput::default(),
                crate::persistence::UpdateAiRunRecordInput {
                    state: Some(AiRunState::WaitingApproval.as_str().to_owned()),
                    lease_owner: Some(None),
                    lease_expires_at: Some(None),
                    lease_heartbeat_at: Some(None),
                    latest_checkpoint_id: Some(Some(parked_checkpoint_id)),
                    ..Default::default()
                },
            )
            .await
            .expect("run wait transition should persist");
            assert!(matches!(updated, ConditionalUpdateOutcome::Updated(_)));
            tool_call_id
        }

        async fn consume_approval_and_reclaim_lease(
            fixture: &ParkFixture,
            tool_call_id: Uuid,
        ) -> AiRunLease {
            let now = fixture.clock.now();
            let approval = AiApprovalRecord::find_by_id(&fixture.database, &fixture.approval_id.0)
                .await
                .expect("approval lookup should succeed")
                .expect("approval should exist");
            assert!(matches!(
                AiApprovalRecord::compare_and_swap(
                    &fixture.database,
                    &approval.id,
                    approval.row_version,
                    crate::persistence::AiApprovalRecordWhereInput::default(),
                    crate::persistence::UpdateAiApprovalRecordInput {
                        state: Some("consumed".to_owned()),
                        approver_subject: Some(Some("parked-wait-owner".to_owned())),
                        decided_at: Some(Some(now.unix_timestamp())),
                        consumed_uses: Some(1),
                        consumed_at: Some(Some(now.unix_timestamp())),
                        ..Default::default()
                    },
                )
                .await
                .expect("approval consumption should persist"),
                ConditionalUpdateOutcome::Updated(_)
            ));
            let call = AiToolCallRecord::find_by_id(&fixture.database, &tool_call_id)
                .await
                .expect("call lookup should succeed")
                .expect("call should exist");
            assert!(matches!(
                AiToolCallRecord::compare_and_swap(
                    &fixture.database,
                    &call.id,
                    call.row_version,
                    crate::persistence::AiToolCallRecordWhereInput::default(),
                    crate::persistence::UpdateAiToolCallRecordInput {
                        state: Some("completed".to_owned()),
                        completed_at: Some(Some(now.unix_timestamp())),
                        ..Default::default()
                    },
                )
                .await
                .expect("tool completion should persist"),
                ConditionalUpdateOutcome::Updated(_)
            ));
            let run = AiRunRecord::find_by_id(&fixture.database, &fixture.lease.run_id().0)
                .await
                .expect("run lookup should succeed")
                .expect("run should exist");
            let fresh_attempt = Uuid::new_v4();
            let updated = AiRunRecord::compare_and_swap(
                &fixture.database,
                &run.id,
                run.row_version,
                crate::persistence::AiRunRecordWhereInput::default(),
                crate::persistence::UpdateAiRunRecordInput {
                    state: Some(AiRunState::Running.as_str().to_owned()),
                    attempt_id: Some(Some(fresh_attempt)),
                    lease_owner: Some(Some("parked-resume-worker".to_owned())),
                    lease_generation: Some(run.lease_generation + 1),
                    lease_expires_at: Some(Some((now + Duration::minutes(5)).unix_timestamp())),
                    lease_heartbeat_at: Some(Some(now.unix_timestamp())),
                    latest_checkpoint_id: Some(None),
                    ..Default::default()
                },
            )
            .await
            .expect("fresh run claim should persist");
            let ConditionalUpdateOutcome::Updated(updated) = updated else {
                panic!("fresh run claim should win")
            };
            crate::orm_runs::lease_from_record(&updated).expect("fresh lease should validate")
        }

        #[tokio::test]
        async fn parked_wait_lifecycle_is_exact_one_shot_and_openable() {
            let fixture = parked_wait_fixture().await;
            for mutate in 0..9 {
                let mut stale = fixture.request.clone();
                match mutate {
                    0 => stale.claim.run_id = AiRunId::new(),
                    1 => stale.claim.attempt_id = Uuid::new_v4(),
                    2 => stale.claim.run_lease_generation += 1,
                    3 => stale.source_checkpoint_id = Uuid::new_v4(),
                    4 => stale.source_checkpoint_fingerprint = "0".repeat(64),
                    5 => stale.continuation_fingerprint = "0".repeat(64),
                    6 => {
                        stale.wait =
                            AiProviderSessionWaitIdentity::approval(crate::AiApprovalId::new())
                    }
                    7 => stale.claim.transcript_fingerprint = "0".repeat(64),
                    _ => {
                        let descriptor = &stale.claim.descriptor;
                        stale.claim.descriptor = AiProviderSessionDescriptor::new(
                            descriptor.provider_kind().clone(),
                            descriptor.provider_profile_id(),
                            descriptor.provider_model(),
                            descriptor.registration_fingerprint(),
                            descriptor.protocol_version(),
                            "0".repeat(64),
                        )
                        .expect("changed descriptor should remain structurally valid");
                    }
                }
                assert!(matches!(
                    fixture.service.park_for_wait(&fixture.lease, stale).await,
                    Err(AiError::Conflict)
                ));
            }
            let parked = fixture
                .service
                .park_for_wait(&fixture.lease, fixture.request.clone())
                .await
                .expect("exact provider session should park");
            assert_eq!(parked.source_run_id(), fixture.lease.run_id());
            assert!(!format!("{parked:?}").contains("parked-response"));
            let mut swapped = parked.clone();
            swapped.continuation_fingerprint = "0".repeat(64);
            assert!(matches!(
                fixture.service.confirm_parked_wait(&swapped).await,
                Err(AiError::Conflict)
            ));
            let tool_call_id = persist_approval_wait_graph(&fixture, &parked).await;
            fixture
                .service
                .confirm_parked_wait(&parked)
                .await
                .expect("exact cleared wait graph should confirm");
            fixture
                .service
                .confirm_parked_wait(&parked)
                .await
                .expect("same exact confirmation should be idempotent");
            let fresh_lease = consume_approval_and_reclaim_lease(&fixture, tool_call_id).await;
            let (left, right) = tokio::join!(
                fixture.service.reclaim_after_wait(&fresh_lease),
                fixture.service.reclaim_after_wait(&fresh_lease),
            );
            let claim = match (left, right) {
                (Ok(claim), Err(AiError::Conflict)) | (Err(AiError::Conflict), Ok(claim)) => claim,
                outcome => panic!("exactly one reclaim should win: {outcome:?}"),
            };
            assert_eq!(claim.binding_id(), fixture.binding_id);
            assert_eq!(claim.attempt_id(), fresh_lease.attempt_id());
            let opened = fixture
                .service
                .open_for_run(&fresh_lease, &claim)
                .await
                .expect("fresh claim should open exact protected cursor");
            assert_eq!(
                opened.cursor().expose_to_provider_adapter(),
                "parked-thread"
            );
            assert!(matches!(
                fixture.service.reclaim_after_wait(&fresh_lease).await,
                Err(AiError::Conflict)
            ));
        }

        #[tokio::test]
        async fn unconfirmed_expired_park_converges_to_cleanup_not_reclaim() {
            let fixture = parked_wait_fixture().await;
            let parked = fixture
                .service
                .park_for_wait(&fixture.lease, fixture.request.clone())
                .await
                .expect("exact provider session should park");
            assert_eq!(parked.wait(), fixture.request.wait());
            fixture.clock.advance_seconds(6 * 60);
            let cleanup = fixture
                .service
                .claim_cleanup("parked-wait-cleanup-worker")
                .await
                .expect("expired unconfirmed park should scan")
                .expect("expired unconfirmed park should require deletion");
            assert_eq!(cleanup.binding_id(), fixture.binding_id);
            assert!(matches!(
                fixture.service.reclaim_after_wait(&fixture.lease).await,
                Err(AiError::Conflict | AiError::ReauthorizationFailed)
            ));
        }

        #[tokio::test]
        async fn failed_wait_handoff_immediately_quarantines_the_exact_park() {
            let fixture = parked_wait_fixture().await;
            let parked = fixture
                .service
                .park_for_wait(&fixture.lease, fixture.request.clone())
                .await
                .expect("exact provider session should park");
            let mut swapped = parked.clone();
            swapped.park_generation += 1;
            assert!(matches!(
                fixture
                    .service
                    .require_parked_wait_cleanup(
                        &swapped,
                        "provider_session_approval_staging_failed",
                    )
                    .await,
                Err(AiError::Conflict)
            ));
            fixture
                .service
                .require_parked_wait_cleanup(&parked, "provider_session_approval_staging_failed")
                .await
                .expect("exact failed handoff should require cleanup immediately");
            let cleanup = fixture
                .service
                .claim_cleanup("failed-wait-handoff-cleaner")
                .await
                .expect("cleanup scan should succeed")
                .expect("failed wait handoff should be immediately eligible");
            assert_eq!(cleanup.binding_id(), fixture.binding_id);
            assert!(matches!(
                fixture.service.confirm_parked_wait(&parked).await,
                Err(AiError::Conflict)
            ));
        }

        #[tokio::test]
        async fn ambiguous_wait_handoff_quarantines_claimed_or_unconfirmed_parked_state() {
            let claimed = parked_wait_fixture().await;
            let mut swapped = claimed.request.clone();
            swapped.wait = AiProviderSessionWaitIdentity::approval(crate::AiApprovalId::new());
            assert!(matches!(
                claimed
                    .service
                    .require_wait_handoff_cleanup(
                        &swapped,
                        "provider_session_approval_park_ambiguous",
                    )
                    .await,
                Err(AiError::Conflict)
            ));
            claimed
                .service
                .require_wait_handoff_cleanup(
                    &claimed.request,
                    "provider_session_approval_request_invalid",
                )
                .await
                .expect("exact still-claimed handoff should require cleanup");
            let cleanup = claimed
                .service
                .claim_cleanup("claimed-handoff-cleaner")
                .await
                .expect("claimed handoff cleanup should scan")
                .expect("claimed cursor must not remain eligible");
            assert_eq!(cleanup.binding_id(), claimed.binding_id);

            let ambiguous = parked_wait_fixture().await;
            ambiguous
                .service
                .park_for_wait(&ambiguous.lease, ambiguous.request.clone())
                .await
                .expect("parking commit should win before response ambiguity");
            ambiguous
                .service
                .require_wait_handoff_cleanup(
                    &ambiguous.request,
                    "provider_session_approval_park_ambiguous",
                )
                .await
                .expect("exact unconfirmed parked handoff should require cleanup");
            let cleanup = ambiguous
                .service
                .claim_cleanup("ambiguous-park-cleaner")
                .await
                .expect("ambiguous park cleanup should scan")
                .expect("ambiguous parked cursor must not remain eligible");
            assert_eq!(cleanup.binding_id(), ambiguous.binding_id);
        }

        #[tokio::test]
        async fn owner_scope_drift_and_restore_quarantine_cannot_park() {
            let owner_drift = parked_wait_fixture().await;
            let session = AiSessionRecord::find_by_id(
                &owner_drift.database,
                &owner_drift.lease.session_id().0,
            )
            .await
            .expect("session lookup should succeed")
            .expect("session should exist");
            assert!(matches!(
                AiSessionRecord::compare_and_swap(
                    &owner_drift.database,
                    &session.id,
                    session.row_version,
                    crate::persistence::AiSessionRecordWhereInput::default(),
                    crate::persistence::UpdateAiSessionRecordInput {
                        owner_subject: Some("changed-owner".to_owned()),
                        scope_id: Some("changed-scope".to_owned()),
                        ..Default::default()
                    },
                )
                .await
                .expect("owner/scope drift should persist"),
                ConditionalUpdateOutcome::Updated(_)
            ));
            assert!(matches!(
                owner_drift
                    .service
                    .park_for_wait(&owner_drift.lease, owner_drift.request.clone())
                    .await,
                Err(AiError::NotFound | AiError::Conflict)
            ));

            let quarantined = parked_wait_fixture().await;
            let binding = AiProviderSessionBindingRecord::find_by_id(
                &quarantined.database,
                &quarantined.binding_id,
            )
            .await
            .expect("binding lookup should succeed")
            .expect("binding should exist");
            assert!(matches!(
                AiProviderSessionBindingRecord::compare_and_swap(
                    &quarantined.database,
                    &binding.id,
                    binding.row_version,
                    AiProviderSessionBindingRecordWhereInput::default(),
                    UpdateAiProviderSessionBindingRecordInput {
                        state: Some(
                            AiProviderSessionState::RestoreQuarantined
                                .as_str()
                                .to_owned(),
                        ),
                        claimed_run_id: Some(None),
                        claimed_attempt_id: Some(None),
                        claimed_run_lease_generation: Some(None),
                        claim_owner: Some(None),
                        claim_expires_at: Some(None),
                        cleanup_reason_code: Some(Some(
                            "provider_session_restore_quarantined".to_owned(),
                        )),
                        ..Default::default()
                    },
                )
                .await
                .expect("restore quarantine should persist"),
                ConditionalUpdateOutcome::Updated(_)
            ));
            assert!(
                quarantined
                    .service
                    .park_for_wait(&quarantined.lease, quarantined.request)
                    .await
                    .is_err(),
                "restore-quarantined provider state must never park or resume",
            );
        }

        #[test]
        fn protected_cursor_payload_rejects_unknown_fields() {
            let value = serde_json::json!({
                "formatVersion": 1,
                "cursorKind": "codex.thread",
                "cursor": "thread-1",
                "bindingHash": "a".repeat(64),
                "unexpected": true,
            });
            assert!(serde_json::from_value::<ProtectedProviderSessionCursor>(value).is_err());
        }

        #[test]
        fn reason_codes_are_log_safe() {
            assert!(validate_reason_code("provider_session_cancelled").is_ok());
            assert!(validate_reason_code("contains secret text").is_err());
            assert!(validate_reason_code("line\nbreak").is_err());
        }

        #[test]
        fn cursor_binding_hash_covers_session_owner_scope_and_runtime() {
            let binding_id = Uuid::new_v4();
            let session_id = AiSessionId(Uuid::new_v4());
            let scope = AiScope::new("workspace", "workspace-1").with_tenant_id("tenant-1");
            let descriptor = AiProviderSessionDescriptor::new(
                crate::ProviderKind::LocalHarness,
                "local-reviewed",
                "reviewed-model",
                "a".repeat(64),
                "codex-app-server/v1",
                "b".repeat(64),
            )
            .expect("descriptor should validate");
            let base = binding_hash(
                binding_id,
                session_id,
                "user",
                "owner-1",
                &scope,
                &descriptor,
                &"c".repeat(64),
            )
            .expect("binding should hash");
            let other_owner = binding_hash(
                binding_id,
                session_id,
                "user",
                "owner-2",
                &scope,
                &descriptor,
                &"c".repeat(64),
            )
            .expect("binding should hash");
            let other_session = binding_hash(
                binding_id,
                AiSessionId(Uuid::new_v4()),
                "user",
                "owner-1",
                &scope,
                &descriptor,
                &"c".repeat(64),
            )
            .expect("binding should hash");
            assert_ne!(base, other_owner);
            assert_ne!(base, other_session);
        }
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub use service::*;
