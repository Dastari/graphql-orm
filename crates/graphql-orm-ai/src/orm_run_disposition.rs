//! ORM-backed owner-authorized disposition of failed runs.

#![cfg(any(feature = "sqlite", feature = "postgres"))]

use std::sync::Arc;

use agql_auth::{
    AuthPrincipal, Clock, CurrentPrincipalResolver, PrincipalReference, ResolvedPrincipal,
};
use async_trait::async_trait;
use graphql_orm::db::Database;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::graphql::filters::UuidFilter;
use graphql_orm::graphql::orm::{ConditionalUpdateOutcome, DefaultWriteBackend, TransactionMode};
use serde_json::json;
use time::Duration;
use uuid::Uuid;

use crate::orm_inbox::{PreparedAiInboxEvent, append_inbox_event};
use crate::orm_runs::run_produced_assistant_output;
use crate::orm_sessions::{
    content_context, map_orm, map_protection, map_transaction, principal_identity, record_scope,
};
use crate::persistence::*;
use crate::{
    AcknowledgeAiRunFailureInput, AiAccessPolicy, AiContentProtectionPolicy,
    AiContentProtectionPolicyResolver, AiContentProtector, AiError, AiRunDisposition,
    AiRunDispositionService, AiRunDispositionView, AiRunRetryAdmission, AiRunRetryEvidence,
    AiRunState, AiRunTerminalEvent, AiScope, AiSessionAction, AiSessionId, AiSessionWakeup,
    RetryAiRunInput, classify_run_retry,
};

/// Deployment bounds for owner disposition and current-principal freshness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiRunDispositionLimits {
    maximum_principal_age: Duration,
}

impl AiRunDispositionLimits {
    /// Creates validated disposition bounds.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] for a non-positive or
    /// longer-than-one-hour principal age.
    pub fn new(maximum_principal_age: Duration) -> Result<Self, AiError> {
        if !maximum_principal_age.is_positive() || maximum_principal_age > Duration::hours(1) {
            return Err(AiError::InvalidConfiguration(
                "invalid run disposition limits".to_owned(),
            ));
        }
        Ok(Self {
            maximum_principal_age,
        })
    }
}

impl Default for AiRunDispositionLimits {
    fn default() -> Self {
        Self {
            maximum_principal_age: Duration::minutes(5),
        }
    }
}

/// Generated-ORM failure-disposition service for application hosts.
pub struct OrmAiRunDispositionService {
    database: Database<DefaultWriteBackend>,
    access_policy: Arc<dyn AiAccessPolicy>,
    protection_policy: Arc<dyn AiContentProtectionPolicyResolver>,
    content_protector: Arc<dyn AiContentProtector>,
    principal_resolver: Arc<dyn CurrentPrincipalResolver>,
    clock: Arc<dyn Clock>,
    limits: AiRunDispositionLimits,
}

impl OrmAiRunDispositionService {
    /// Creates an owner-authorized failure-disposition service.
    pub fn new(
        database: Database<DefaultWriteBackend>,
        access_policy: Arc<dyn AiAccessPolicy>,
        protection_policy: Arc<dyn AiContentProtectionPolicyResolver>,
        content_protector: Arc<dyn AiContentProtector>,
        principal_resolver: Arc<dyn CurrentPrincipalResolver>,
        clock: Arc<dyn Clock>,
        limits: AiRunDispositionLimits,
    ) -> Self {
        Self {
            database,
            access_policy,
            protection_policy,
            content_protector,
            principal_resolver,
            clock,
            limits,
        }
    }

    async fn resolve_current(
        &self,
        reference: &PrincipalReference,
    ) -> Result<ResolvedPrincipal, AiError> {
        let resolved = self
            .principal_resolver
            .resolve(reference)
            .await
            .map_err(|_| AiError::ReauthorizationFailed)?;
        let now = self.clock.now();
        if resolved.reference() != reference
            || resolved.resolved_at() > now
            || now - resolved.resolved_at() >= self.limits.maximum_principal_age
            || reference
                .expires_at
                .is_some_and(|expires_at| expires_at <= now)
        {
            return Err(AiError::ReauthorizationFailed);
        }
        Ok(resolved)
    }

    async fn authorize(
        &self,
        resolved: &ResolvedPrincipal,
        session: &AiSessionRecord,
    ) -> Result<(), AiError> {
        let principal = resolved.principal();
        let (kind, subject) = principal_identity(principal);
        if session.owner_principal_kind != kind
            || session.owner_subject != subject
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
        Ok(())
    }

    async fn protect(
        &self,
        policy: &AiContentProtectionPolicy,
        entity: &str,
        row_id: Uuid,
        scope: &AiScope,
        value: serde_json::Value,
    ) -> Result<serde_json::Value, AiError> {
        let envelope = self
            .content_protector
            .protect(
                policy,
                &content_context(entity, row_id, "protected_payload", scope),
                value,
            )
            .await
            .map_err(map_protection)?;
        serde_json::to_value(envelope).map_err(|_| AiError::PersistenceFailed)
    }

    /// Runs the shared admission, idempotency, and durable write path.
    async fn dispose(
        &self,
        principal: &AuthPrincipal,
        session_id: Uuid,
        run_id: Uuid,
        client_request_id: Uuid,
        disposition: AiRunDisposition,
    ) -> Result<AiRunDispositionView, AiError> {
        if session_id.is_nil() || run_id.is_nil() || client_request_id.is_nil() {
            return Err(AiError::InvalidInput(
                "invalid run disposition identity".to_owned(),
            ));
        }
        let requested_reference = principal.reference();
        let current = self.resolve_current(&requested_reference).await?;
        let session = AiSessionRecord::find_by_id(&self.database, &session_id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::NotFound)?;
        self.authorize(&current, &session).await?;

        // Idempotent replay: the same key returns the original decision without
        // authoring a second run.
        if let Some(existing) =
            AiRunFailureDispositionRecord::find_by_id(&self.database, &client_request_id)
                .await
                .map_err(|error| map_orm(OrmPublicError::from(error)))?
        {
            if existing.session_id != session.id || existing.source_run_id != run_id {
                return Err(AiError::Conflict);
            }
            return disposition_view(&existing);
        }

        let run = AiRunRecord::find_by_id(&self.database, &run_id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .filter(|run| run.session_id == session.id)
            .ok_or(AiError::NotFound)?;
        let state = AiRunState::from_persisted(&run.state).ok_or(AiError::PersistenceFailed)?;
        let terminal = match state {
            AiRunState::Failed => AiRunTerminalEvent::Failed,
            AiRunState::RecoveryRequired => AiRunTerminalEvent::RecoveryRequired,
            AiRunState::Cancelled => AiRunTerminalEvent::Cancelled,
            _ => return Err(AiError::Conflict),
        };

        // Reauthorize immediately before the durable write so a revocation
        // between the read and the commit cannot be used.
        let current = self.resolve_current(&requested_reference).await?;
        self.authorize(&current, &session).await?;
        let scope = record_scope(&session);
        let policy = self
            .protection_policy
            .resolve(current.principal(), &scope)
            .await?;
        if !policy.ready || policy.scope != scope {
            return Err(AiError::RuntimeNotReady);
        }
        // The retry runs under the current principal, not the one the source
        // run captured. A stale reference must never be resurrected.
        let principal_reference =
            serde_json::to_value(current.reference()).map_err(|_| AiError::PersistenceFailed)?;
        let (principal_kind, principal_subject) = principal_identity(current.principal());
        let principal_subject = principal_subject.to_owned();

        let event_id = Uuid::new_v4();
        let inbox_event_id = Uuid::new_v4();
        let retry_run_id = matches!(disposition, AiRunDisposition::Retried).then(Uuid::new_v4);
        let event_type = match disposition {
            AiRunDisposition::Retried => "run_retry_queued",
            AiRunDisposition::Acknowledged => "run_failure_acknowledged",
        };
        let identifiers = json!({
            "sessionId": session_id,
            "sourceRunId": run_id,
            "retryRunId": retry_run_id,
            "clientRequestId": client_request_id,
            "disposition": disposition.as_str(),
        });
        let protected_event = self
            .protect(
                &policy,
                "graphql_orm_ai_session_events",
                event_id,
                &scope,
                identifiers.clone(),
            )
            .await?;
        let protected_inbox_event = self
            .protect(
                &policy,
                "graphql_orm_ai_inbox_events",
                inbox_event_id,
                &scope,
                identifiers,
            )
            .await?;

        let now = self
            .clock
            .now()
            .replace_nanosecond(0)
            .unwrap_or_else(|_| self.clock.now());
        let now_unix = now.unix_timestamp();
        let input_message_id = run.input_message_id;
        let source_state = run.state.clone();
        let source_outcome_code = run.error_code.clone();
        let expected_run = run.clone();
        let owner_kind = principal_kind.clone();
        let owner_subject = principal_subject.clone();

        let record = self
            .database
            .transaction(TransactionMode::StateMachine, move |tx| {
                let principal_reference = principal_reference.clone();
                let protected_event = protected_event.clone();
                let protected_inbox_event = protected_inbox_event.clone();
                let owner_kind = owner_kind.clone();
                let owner_subject = owner_subject.clone();
                let source_state = source_state.clone();
                let source_outcome_code = source_outcome_code.clone();
                let expected_run = expected_run.clone();
                Box::pin(async move {
                    let session = tx
                        .find_by_id::<AiSessionRecord>(&session_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    if !matches!(session.state.as_str(), "active") || session.deleted_at.is_some() {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    let run = tx
                        .find_by_id::<AiRunRecord>(&run_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    // The run must not have changed between admission and the
                    // commit; a terminal run should be immutable, and anything
                    // else means the decision was made against stale evidence.
                    if run != expected_run {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    // At most one disposition wins per source run.
                    let existing = tx
                        .query::<AiRunFailureDispositionRecord>()
                        .filter(AiRunFailureDispositionRecordWhereInput {
                            source_run_id: Some(UuidFilter {
                                eq: Some(run_id),
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

                    if let Some(retry_run_id) = retry_run_id {
                        // Re-decide admission from committed rows inside the
                        // same transaction that authors the new run.
                        let evidence = AiRunRetryEvidence {
                            terminal,
                            produced_assistant_output: run_produced_assistant_output(
                                tx, session_id, run_id,
                            )
                            .await?,
                        };
                        if classify_run_retry(evidence, run.error_code.as_deref())
                            != AiRunRetryAdmission::Allowed
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let message = tx
                            .find_by_id::<AiMessageRecord>(&input_message_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        // Retry reuses the durable user message; it never
                        // rewrites one, and it refuses a purged one because the
                        // prompt no longer exists.
                        if message.session_id != session_id
                            || message.message_role != "user"
                            || message.content_purged_at.is_some()
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        tx.insert::<AiRunRecord>(CreateAiRunRecordInput {
                            id: retry_run_id,
                            session_id,
                            input_message_id,
                            principal_reference,
                            state: AiRunState::Queued.as_str().to_owned(),
                            attempt_id: None,
                            lease_owner: None,
                            lease_generation: 0,
                            lease_expires_at: None,
                            lease_heartbeat_at: None,
                            retry_count: 0,
                            next_attempt_at: Some(now_unix),
                            error_code: None,
                            latest_checkpoint_id: None,
                            cancellation_request_id: None,
                            cancellation_requested_at: None,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                    }

                    let sequence = session
                        .stream_head
                        .checked_add(1)
                        .filter(|sequence| *sequence <= i64::from(i32::MAX))
                        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                    if !matches!(
                        tx.compare_and_swap::<AiSessionRecord>(
                            &session.id,
                            session.row_version,
                            AiSessionRecordWhereInput::default(),
                            UpdateAiSessionRecordInput {
                                stream_head: Some(sequence),
                                last_activity_at: Some(now_unix),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?,
                        ConditionalUpdateOutcome::Updated(_)
                    ) {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }

                    let record = tx
                        .insert::<AiRunFailureDispositionRecord>(
                            CreateAiRunFailureDispositionRecordInput {
                                id: client_request_id,
                                session_id,
                                source_run_id: run_id,
                                input_message_id,
                                disposition: disposition.as_str().to_owned(),
                                retry_run_id,
                                source_state,
                                source_outcome_code,
                                principal_kind: owner_kind.clone(),
                                principal_subject: owner_subject.clone(),
                                decided_at: now_unix,
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?;
                    tx.insert::<AiSessionEventRecord>(CreateAiSessionEventRecordInput {
                        id: event_id,
                        session_id,
                        sequence,
                        event_type: event_type.to_owned(),
                        run_id: Some(run_id),
                        causation_id: Some(client_request_id.to_string()),
                        correlation_id: client_request_id.to_string(),
                        protected_payload: protected_event,
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                    tx.queue_event(AiSessionWakeup {
                        session_id,
                        sequence,
                    });
                    append_inbox_event(
                        tx,
                        PreparedAiInboxEvent {
                            id: inbox_event_id,
                            principal_kind: owner_kind,
                            principal_subject: owner_subject,
                            scope: AiScope {
                                kind: session.scope_kind.clone(),
                                id: session.scope_id.clone(),
                                tenant_id: session.tenant_id.clone(),
                            },
                            session_id,
                            event_type: event_type.to_owned(),
                            protected_payload: protected_inbox_event,
                            created_at: now_unix,
                        },
                    )
                    .await?;
                    Ok(record)
                })
            })
            .await
            .map_err(map_transaction)?;
        disposition_view(&record)
    }
}

fn disposition_view(
    record: &AiRunFailureDispositionRecord,
) -> Result<AiRunDispositionView, AiError> {
    Ok(AiRunDispositionView {
        session_id: record.session_id,
        run_id: record.source_run_id,
        client_request_id: record.id,
        disposition: AiRunDisposition::from_persisted(&record.disposition)
            .ok_or(AiError::PersistenceFailed)?,
        retry_run_id: record.retry_run_id,
        input_message_id: record.input_message_id,
        decided_at: record.decided_at,
    })
}

#[async_trait]
impl AiRunDispositionService for OrmAiRunDispositionService {
    async fn retry_run(
        &self,
        principal: &AuthPrincipal,
        input: RetryAiRunInput,
    ) -> Result<AiRunDispositionView, AiError> {
        self.dispose(
            principal,
            input.session_id,
            input.run_id,
            input.client_request_id,
            AiRunDisposition::Retried,
        )
        .await
    }

    async fn acknowledge_run_failure(
        &self,
        principal: &AuthPrincipal,
        input: AcknowledgeAiRunFailureInput,
    ) -> Result<AiRunDispositionView, AiError> {
        self.dispose(
            principal,
            input.session_id,
            input.run_id,
            input.client_request_id,
            AiRunDisposition::Acknowledged,
        )
        .await
    }
}
