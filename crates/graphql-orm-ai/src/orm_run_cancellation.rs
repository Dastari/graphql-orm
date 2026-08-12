//! ORM-backed owner-authorized durable run cancellation.

#![cfg(any(feature = "sqlite", feature = "postgres"))]

use std::sync::Arc;

use agql_auth::{
    AuthPrincipal, Clock, CurrentPrincipalResolver, PrincipalReference, ResolvedPrincipal,
};
use async_trait::async_trait;
use graphql_orm::db::Database;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::graphql::filters::{StringFilter, UuidFilter};
use graphql_orm::graphql::orm::{ConditionalUpdateOutcome, DefaultWriteBackend, TransactionMode};
use serde_json::json;
use time::Duration;
use uuid::Uuid;

use crate::orm_inbox::{PreparedAiInboxEvent, append_inbox_event};
use crate::orm_sessions::{
    content_context, map_orm, map_protection, map_transaction, principal_identity, record_scope,
};
use crate::persistence::*;
use crate::{
    AiAccessPolicy, AiContentProtectionPolicy, AiContentProtectionPolicyResolver,
    AiContentProtector, AiError, AiRunCancellationHub, AiRunCancellationService,
    AiRunCancellationView, AiRunState, AiRunTerminalEvent, AiScope, AiSessionAction, AiSessionId,
    AiSessionWakeup, CancelAiRunInput,
};

const MAXIMUM_ACTIVE_TOOL_CALLS_HARD_LIMIT: usize = 256;

/// Deployment bounds for owner cancellation and current-principal freshness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiRunCancellationLimits {
    maximum_principal_age: Duration,
    maximum_active_tool_calls: usize,
}

impl AiRunCancellationLimits {
    /// Creates validated cancellation bounds.
    pub fn new(
        maximum_principal_age: Duration,
        maximum_active_tool_calls: usize,
    ) -> Result<Self, AiError> {
        if !maximum_principal_age.is_positive()
            || maximum_principal_age > Duration::hours(1)
            || !(1..=MAXIMUM_ACTIVE_TOOL_CALLS_HARD_LIMIT).contains(&maximum_active_tool_calls)
        {
            return Err(AiError::InvalidConfiguration(
                "invalid run cancellation limits".to_owned(),
            ));
        }
        Ok(Self {
            maximum_principal_age,
            maximum_active_tool_calls,
        })
    }
}

impl Default for AiRunCancellationLimits {
    fn default() -> Self {
        Self {
            maximum_principal_age: Duration::minutes(5),
            maximum_active_tool_calls: 64,
        }
    }
}

/// Generated-ORM cancellation service for application hosts.
pub struct OrmAiRunCancellationService {
    database: Database<DefaultWriteBackend>,
    access_policy: Arc<dyn AiAccessPolicy>,
    protection_policy: Arc<dyn AiContentProtectionPolicyResolver>,
    content_protector: Arc<dyn AiContentProtector>,
    principal_resolver: Arc<dyn CurrentPrincipalResolver>,
    clock: Arc<dyn Clock>,
    limits: AiRunCancellationLimits,
    hub: Arc<AiRunCancellationHub>,
}

impl OrmAiRunCancellationService {
    /// Creates an owner-authorized cancellation service.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        database: Database<DefaultWriteBackend>,
        access_policy: Arc<dyn AiAccessPolicy>,
        protection_policy: Arc<dyn AiContentProtectionPolicyResolver>,
        content_protector: Arc<dyn AiContentProtector>,
        principal_resolver: Arc<dyn CurrentPrincipalResolver>,
        clock: Arc<dyn Clock>,
        limits: AiRunCancellationLimits,
        hub: Arc<AiRunCancellationHub>,
    ) -> Self {
        Self {
            database,
            access_policy,
            protection_policy,
            content_protector,
            principal_resolver,
            clock,
            limits,
            hub,
        }
    }

    /// Returns the process-local hub that should also be installed on the run
    /// service used by coordinators.
    pub fn cancellation_hub(&self) -> Arc<AiRunCancellationHub> {
        self.hub.clone()
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
}

#[async_trait]
impl AiRunCancellationService for OrmAiRunCancellationService {
    async fn request_cancellation(
        &self,
        principal: &AuthPrincipal,
        input: CancelAiRunInput,
    ) -> Result<AiRunCancellationView, AiError> {
        if input.session_id.is_nil() || input.run_id.is_nil() || input.client_request_id.is_nil() {
            return Err(AiError::InvalidInput(
                "invalid run cancellation identity".to_owned(),
            ));
        }
        let requested_reference = principal.reference();
        let current = self.resolve_current(&requested_reference).await?;
        let session = AiSessionRecord::find_by_id(&self.database, &input.session_id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::NotFound)?;
        self.authorize(&current, &session).await?;
        let run = AiRunRecord::find_by_id(&self.database, &input.run_id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .filter(|run| run.session_id == session.id)
            .ok_or(AiError::NotFound)?;
        let duplicate = run.state == AiRunState::Cancelled.as_str()
            && run.cancellation_request_id == Some(input.client_request_id);
        let state = AiRunState::from_persisted(&run.state).ok_or(AiError::PersistenceFailed)?;
        if state.is_terminal() && !duplicate {
            return Err(AiError::Conflict);
        }

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
        let request_event_id = Uuid::new_v4();
        let final_event_id = Uuid::new_v4();
        let request_inbox_event_id = Uuid::new_v4();
        let final_inbox_event_id = Uuid::new_v4();
        let identifiers = json!({
            "sessionId": input.session_id,
            "runId": input.run_id,
            "clientRequestId": input.client_request_id,
        });
        let request_payload = self
            .protect(
                &policy,
                "graphql_orm_ai_session_events",
                request_event_id,
                &scope,
                json!({"cancellation": identifiers, "state": "requested"}),
            )
            .await?;
        let final_payload = self
            .protect(
                &policy,
                "graphql_orm_ai_session_events",
                final_event_id,
                &scope,
                json!({"cancellation": identifiers, "state": "cancelled"}),
            )
            .await?;
        let request_inbox_payload = self
            .protect(
                &policy,
                "graphql_orm_ai_inbox_events",
                request_inbox_event_id,
                &scope,
                json!({"sessionId": input.session_id, "runId": input.run_id, "state": "requested"}),
            )
            .await?;
        let final_inbox_payload = self
            .protect(
                &policy,
                "graphql_orm_ai_inbox_events",
                final_inbox_event_id,
                &scope,
                json!({"sessionId": input.session_id, "runId": input.run_id, "state": "cancelled"}),
            )
            .await?;
        let (principal_kind, principal_subject) = principal_identity(current.principal());
        let principal_subject = principal_subject.to_owned();
        let now = self
            .clock
            .now()
            .replace_nanosecond(0)
            .unwrap_or_else(|_| self.clock.now());
        let now_unix = now.unix_timestamp();
        let maximum_active_tool_calls = self.limits.maximum_active_tool_calls;
        let session_id = input.session_id;
        let run_id = input.run_id;
        let client_request_id = input.client_request_id;
        let view = self
            .database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    if let Some(existing) = tx
                        .find_by_id::<AiRunCancellationRequestRecord>(&client_request_id)
                        .await
                        .map_err(OrmPublicError::from)?
                    {
                        if existing.session_id != session_id
                            || existing.run_id != run_id
                            || existing.principal_kind != principal_kind
                            || existing.principal_subject != principal_subject
                            || existing.outcome_state != AiRunState::Cancelled.as_str()
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let run = tx
                            .find_by_id::<AiRunRecord>(&run_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if run.session_id != session_id
                            || run.state != AiRunState::Cancelled.as_str()
                            || run.cancellation_request_id != Some(client_request_id)
                            || run.cancellation_requested_at != Some(existing.requested_at)
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        return Ok(AiRunCancellationView {
                            session_id,
                            run_id,
                            client_request_id,
                            state: existing.outcome_state,
                            requested_at: existing.requested_at,
                        });
                    }

                    let session = tx
                        .find_by_id::<AiSessionRecord>(&session_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    if session.owner_principal_kind != principal_kind
                        || session.owner_subject != principal_subject
                        || session.deleted_at.is_some()
                        || !matches!(session.state.as_str(), "active" | "archived")
                    {
                        return Err(OrmPublicError::not_found());
                    }
                    let run = tx
                        .find_by_id::<AiRunRecord>(&run_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    let state = AiRunState::from_persisted(&run.state)
                        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                    if run.session_id != session_id
                        || state.is_terminal()
                        || run.cancellation_request_id.is_some()
                        || run.cancellation_requested_at.is_some()
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    if let Some(checkpoint_id) = run.latest_checkpoint_id {
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
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        if checkpoint.run_id != run.id
                            || checkpoint.checkpoint_kind == "assistant_output_persisted"
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                    }

                    cancel_active_tool_calls(tx, &run, maximum_active_tool_calls, now_unix).await?;
                    cancel_background_submission(tx, &run, now_unix).await?;

                    let request_sequence = session
                        .stream_head
                        .checked_add(1)
                        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                    let final_sequence = request_sequence
                        .checked_add(1)
                        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                    if !matches!(
                        tx.compare_and_swap::<AiSessionRecord>(
                            &session.id,
                            session.row_version,
                            AiSessionRecordWhereInput::default(),
                            UpdateAiSessionRecordInput {
                                stream_head: Some(final_sequence),
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
                    if !matches!(
                        tx.compare_and_swap::<AiRunRecord>(
                            &run.id,
                            run.row_version,
                            AiRunRecordWhereInput {
                                state: Some(StringFilter {
                                    eq: Some(run.state.clone()),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            },
                            UpdateAiRunRecordInput {
                                state: Some(AiRunState::Cancelled.as_str().to_owned()),
                                lease_owner: Some(None),
                                lease_expires_at: Some(None),
                                lease_heartbeat_at: Some(None),
                                next_attempt_at: Some(None),
                                error_code: Some(Some("owner_cancelled".to_owned())),
                                cancellation_request_id: Some(Some(client_request_id)),
                                cancellation_requested_at: Some(Some(now_unix)),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?,
                        ConditionalUpdateOutcome::Updated(_)
                    ) {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }

                    append_cancelled_attempt_outcome(tx, &run, now_unix).await?;
                    tx.insert::<AiRunCancellationRequestRecord>(
                        CreateAiRunCancellationRequestRecordInput {
                            id: client_request_id,
                            session_id,
                            run_id,
                            principal_kind: principal_kind.clone(),
                            principal_subject: principal_subject.clone(),
                            outcome_state: AiRunState::Cancelled.as_str().to_owned(),
                            requested_at: now_unix,
                        },
                    )
                    .await
                    .map_err(OrmPublicError::from)?;
                    tx.insert::<AiSessionEventRecord>(CreateAiSessionEventRecordInput {
                        id: request_event_id,
                        session_id,
                        sequence: request_sequence,
                        event_type: "run_cancellation_requested".to_owned(),
                        run_id: Some(run_id),
                        causation_id: Some(client_request_id.to_string()),
                        correlation_id: client_request_id.to_string(),
                        protected_payload: request_payload,
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                    tx.insert::<AiSessionEventRecord>(CreateAiSessionEventRecordInput {
                        id: final_event_id,
                        session_id,
                        sequence: final_sequence,
                        event_type: AiRunTerminalEvent::Cancelled.event_type().to_owned(),
                        run_id: Some(run_id),
                        causation_id: Some(request_event_id.to_string()),
                        correlation_id: client_request_id.to_string(),
                        protected_payload: final_payload,
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                    tx.queue_event(AiSessionWakeup {
                        session_id,
                        sequence: request_sequence,
                    });
                    tx.queue_event(AiSessionWakeup {
                        session_id,
                        sequence: final_sequence,
                    });
                    append_inbox_event(
                        tx,
                        PreparedAiInboxEvent {
                            id: request_inbox_event_id,
                            principal_kind: principal_kind.clone(),
                            principal_subject: principal_subject.clone(),
                            scope: scope.clone(),
                            session_id,
                            event_type: "run_cancellation_requested".to_owned(),
                            protected_payload: request_inbox_payload,
                            created_at: now_unix,
                        },
                    )
                    .await?;
                    append_inbox_event(
                        tx,
                        PreparedAiInboxEvent {
                            id: final_inbox_event_id,
                            principal_kind: principal_kind.clone(),
                            principal_subject: principal_subject.clone(),
                            scope,
                            session_id,
                            event_type: AiRunTerminalEvent::Cancelled.event_type().to_owned(),
                            protected_payload: final_inbox_payload,
                            created_at: now_unix,
                        },
                    )
                    .await?;
                    tx.insert::<AiAuditEventRecord>(CreateAiAuditEventRecordInput {
                        actor_principal_kind: principal_kind,
                        actor_subject: principal_subject,
                        action: "ai.run.cancel".to_owned(),
                        resource_kind: "ai_run".to_owned(),
                        resource_reference: run_id.to_string(),
                        outcome: "cancelled".to_owned(),
                        reason_code: "owner_cancelled".to_owned(),
                        correlation_id: client_request_id.to_string(),
                        causation_id: None,
                        policy_version: None,
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                    Ok(AiRunCancellationView {
                        session_id,
                        run_id,
                        client_request_id,
                        state: AiRunState::Cancelled.as_str().to_owned(),
                        requested_at: now_unix,
                    })
                })
            })
            .await
            .map_err(map_transaction)?;
        self.hub.notify(crate::AiRunId(run_id));
        Ok(view)
    }
}

async fn cancel_active_tool_calls(
    tx: &mut graphql_orm::graphql::orm::MutationContext<'_, DefaultWriteBackend>,
    run: &AiRunRecord,
    maximum: usize,
    now: i64,
) -> Result<(), OrmPublicError> {
    let calls = tx
        .query::<AiToolCallRecord>()
        .filter(AiToolCallRecordWhereInput {
            run_id: Some(UuidFilter {
                eq: Some(run.id),
                ..Default::default()
            }),
            state: Some(StringFilter {
                in_list: Some(vec!["executing".to_owned(), "waiting_approval".to_owned()]),
                ..Default::default()
            }),
            ..Default::default()
        })
        .limit(
            i64::try_from(maximum + 1)
                .map_err(|_| OrmPublicError::new(OrmErrorCode::InvalidInput))?,
        )
        .fetch_all()
        .await
        .map_err(OrmPublicError::from)?;
    if calls.len() > maximum {
        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
    }
    for call in calls {
        if let Some(approval_id) = call.approval_id {
            let approval = tx
                .find_by_id::<AiApprovalRecord>(&approval_id)
                .await
                .map_err(OrmPublicError::from)?
                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
            if matches!(
                approval.state.as_str(),
                "pending" | "approved" | "resume_claimed"
            ) && !matches!(
                tx.compare_and_swap::<AiApprovalRecord>(
                    &approval.id,
                    approval.row_version,
                    AiApprovalRecordWhereInput::default(),
                    UpdateAiApprovalRecordInput {
                        state: Some("revoked".to_owned()),
                        decided_at: Some(Some(now)),
                        ..Default::default()
                    },
                )
                .await
                .map_err(OrmPublicError::from)?,
                ConditionalUpdateOutcome::Updated(_)
            ) {
                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
            }
        }
        let step = tx
            .find_by_id::<AiRunStepRecord>(&call.id)
            .await
            .map_err(OrmPublicError::from)?
            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
        if step.run_id != run.id || step.lease_generation != call.lease_generation {
            return Err(OrmPublicError::new(OrmErrorCode::InternalError));
        }
        if !matches!(
            tx.compare_and_swap::<AiToolCallRecord>(
                &call.id,
                call.row_version,
                AiToolCallRecordWhereInput::default(),
                UpdateAiToolCallRecordInput {
                    state: Some("cancelled".to_owned()),
                    authorization_code: Some(Some("run_cancelled".to_owned())),
                    completed_at: Some(Some(now)),
                    ..Default::default()
                },
            )
            .await
            .map_err(OrmPublicError::from)?,
            ConditionalUpdateOutcome::Updated(_)
        ) || !matches!(
            tx.compare_and_swap::<AiRunStepRecord>(
                &step.id,
                step.row_version,
                AiRunStepRecordWhereInput::default(),
                UpdateAiRunStepRecordInput {
                    state: Some("cancelled".to_owned()),
                    finished_at: Some(Some(now)),
                    error_code: Some(Some("run_cancelled".to_owned())),
                    ..Default::default()
                },
            )
            .await
            .map_err(OrmPublicError::from)?,
            ConditionalUpdateOutcome::Updated(_)
        ) {
            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
        }
    }
    Ok(())
}

async fn cancel_background_submission(
    tx: &mut graphql_orm::graphql::orm::MutationContext<'_, DefaultWriteBackend>,
    run: &AiRunRecord,
    now: i64,
) -> Result<(), OrmPublicError> {
    let submissions = tx
        .query::<AiProviderBackgroundSubmissionRecord>()
        .filter(AiProviderBackgroundSubmissionRecordWhereInput {
            run_id: Some(UuidFilter {
                eq: Some(run.id),
                ..Default::default()
            }),
            ..Default::default()
        })
        .limit(2)
        .fetch_all()
        .await
        .map_err(OrmPublicError::from)?;
    if submissions.len() > 1 {
        return Err(OrmPublicError::new(OrmErrorCode::InternalError));
    }
    if let Some(submission) = submissions.into_iter().next()
        && matches!(
            submission.state.as_str(),
            "prepared" | "waiting_provider" | "reconciling"
        )
        && !matches!(
            tx.compare_and_swap::<AiProviderBackgroundSubmissionRecord>(
                &submission.id,
                submission.row_version,
                AiProviderBackgroundSubmissionRecordWhereInput::default(),
                UpdateAiProviderBackgroundSubmissionRecordInput {
                    state: Some("cancelled".to_owned()),
                    safe_error_code: Some(Some("run_cancelled".to_owned())),
                    reconciliation_owner: Some(None),
                    reconciliation_lease_expires_at: Some(None),
                    reconciled_at: Some(Some(now)),
                    ..Default::default()
                },
            )
            .await
            .map_err(OrmPublicError::from)?,
            ConditionalUpdateOutcome::Updated(_)
        )
    {
        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
    }
    Ok(())
}

async fn append_cancelled_attempt_outcome(
    tx: &mut graphql_orm::graphql::orm::MutationContext<'_, DefaultWriteBackend>,
    run: &AiRunRecord,
    now: i64,
) -> Result<(), OrmPublicError> {
    let Some(attempt_id) = run.attempt_id else {
        return Ok(());
    };
    let attempt = tx
        .query::<AiRunAttemptRecord>()
        .filter(AiRunAttemptRecordWhereInput {
            id: Some(UuidFilter {
                eq: Some(attempt_id),
                ..Default::default()
            }),
            ..Default::default()
        })
        .limit(1)
        .fetch_one()
        .await
        .map_err(OrmPublicError::from)?
        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
    if attempt.run_id != run.id || attempt.lease_generation != run.lease_generation {
        return Err(OrmPublicError::new(OrmErrorCode::InternalError));
    }
    tx.insert::<AiRunAttemptOutcomeRecord>(CreateAiRunAttemptOutcomeRecordInput {
        attempt_id,
        run_id: run.id,
        lease_generation: run.lease_generation,
        worker_id: attempt.worker_id,
        final_state: AiRunState::Cancelled.as_str().to_owned(),
        outcome_code: "owner_cancelled".to_owned(),
        provider_response_id: None,
        finished_at: now,
    })
    .await
    .map_err(OrmPublicError::from)?;
    Ok(())
}
