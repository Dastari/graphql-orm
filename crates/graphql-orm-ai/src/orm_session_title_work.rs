//! ORM-backed durable first-message session-title work.

#![cfg(any(feature = "sqlite", feature = "postgres"))]

use std::sync::Arc;

use agql_auth::{
    AuthPrincipal, Clock, CurrentPrincipalResolver, PrincipalReference, ResolvedPrincipal,
};
use async_trait::async_trait;
use graphql_orm::db::Database;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::graphql::filters::{IntFilter, StringFilter, UuidFilter};
use graphql_orm::graphql::orm::{
    ConditionalUpdateOutcome, DefaultWriteBackend, MutationContext, TransactionError,
    TransactionMode,
};
use serde_json::json;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::orm_inbox::{PreparedAiInboxEvent, append_inbox_event};
use crate::orm_runs::{canonical_second, validate_worker_id};
use crate::orm_sessions::{
    content_context, map_orm, map_protection, map_transaction, normalize_title, principal_identity,
    record_scope, session_title_hash, session_view,
};
use crate::persistence::*;
use crate::{
    AiAccessPolicy, AiContentProtectionPolicy, AiContentProtectionPolicyResolver,
    AiContentProtector, AiError, AiSessionAction, AiSessionId, AiSessionTitleActor,
    AiSessionTitleCommitOutcome, AiSessionTitleWorkClaim, AiSessionTitleWorkInput,
    AiSessionTitleWorkService, AiSessionWakeup, ContentProtectionContext, ProtectedContentEnvelope,
};

const MAXIMUM_SAFE_ERROR_CODE_BYTES: usize = 200;

/// Deployment hard limits for durable title workers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiSessionTitleWorkLimits {
    lease_ttl: Duration,
    maximum_retry_delay: Duration,
    maximum_principal_age: Duration,
    maximum_candidate_scan: usize,
    maximum_retries: u32,
    maximum_transaction_retries: usize,
    maximum_title_bytes: usize,
    maximum_message_bytes: usize,
}

impl AiSessionTitleWorkLimits {
    /// Creates validated deployment-owned title-work limits.
    ///
    /// # Errors
    ///
    /// Returns an error unless durations are positive and bounded, candidate
    /// scans are in `1..=256`, retries are at most 100, serialization retries
    /// are at most 16, titles are in `1..=4096` bytes, and opened first
    /// messages are in `1..=1 MiB` bytes.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        lease_ttl: Duration,
        maximum_retry_delay: Duration,
        maximum_principal_age: Duration,
        maximum_candidate_scan: usize,
        maximum_retries: u32,
        maximum_transaction_retries: usize,
        maximum_title_bytes: usize,
        maximum_message_bytes: usize,
    ) -> Result<Self, AiError> {
        if !lease_ttl.is_positive()
            || lease_ttl > Duration::hours(1)
            || !maximum_retry_delay.is_positive()
            || maximum_retry_delay > Duration::days(7)
            || !maximum_principal_age.is_positive()
            || maximum_principal_age > Duration::hours(1)
            || !(1..=256).contains(&maximum_candidate_scan)
            || maximum_retries > 100
            || maximum_transaction_retries > 16
            || !(1..=4096).contains(&maximum_title_bytes)
            || !(1..=1024 * 1024).contains(&maximum_message_bytes)
        {
            return Err(AiError::InvalidConfiguration(
                "invalid session-title work limits".to_owned(),
            ));
        }
        Ok(Self {
            lease_ttl,
            maximum_retry_delay,
            maximum_principal_age,
            maximum_candidate_scan,
            maximum_retries,
            maximum_transaction_retries,
            maximum_title_bytes,
            maximum_message_bytes,
        })
    }

    /// Exact duration of every new or renewed lease.
    pub const fn lease_ttl(self) -> Duration {
        self.lease_ttl
    }

    /// Maximum rows considered by one bounded claim pass.
    pub const fn maximum_candidate_scan(self) -> usize {
        self.maximum_candidate_scan
    }
}

impl Default for AiSessionTitleWorkLimits {
    fn default() -> Self {
        Self {
            lease_ttl: Duration::minutes(5),
            maximum_retry_delay: Duration::hours(1),
            maximum_principal_age: Duration::minutes(5),
            maximum_candidate_scan: 50,
            maximum_retries: 5,
            maximum_transaction_retries: 4,
            maximum_title_bytes: 256,
            maximum_message_bytes: 256 * 1024,
        }
    }
}

/// Generated-ORM-only title-work scheduler and conditional commit service.
///
/// The service has no provider dependency. It opens the protected first user
/// message only after exact current-principal and owner/scope authorization,
/// and it repeats that boundary before committing a generated title.
pub struct OrmAiSessionTitleWorkService {
    database: Database<DefaultWriteBackend>,
    access_policy: Arc<dyn AiAccessPolicy>,
    protection_policy: Arc<dyn AiContentProtectionPolicyResolver>,
    content_protector: Arc<dyn AiContentProtector>,
    principal_resolver: Arc<dyn CurrentPrincipalResolver>,
    clock: Arc<dyn Clock>,
    limits: AiSessionTitleWorkLimits,
}

impl OrmAiSessionTitleWorkService {
    /// Creates a provider-neutral durable title-work service.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        database: Database<DefaultWriteBackend>,
        access_policy: Arc<dyn AiAccessPolicy>,
        protection_policy: Arc<dyn AiContentProtectionPolicyResolver>,
        content_protector: Arc<dyn AiContentProtector>,
        principal_resolver: Arc<dyn CurrentPrincipalResolver>,
        clock: Arc<dyn Clock>,
        limits: AiSessionTitleWorkLimits,
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

    async fn load_owned_session(
        &self,
        principal: &AuthPrincipal,
        session_id: AiSessionId,
        action: AiSessionAction,
    ) -> Result<AiSessionRecord, AiError> {
        let session = AiSessionRecord::find_by_id(&self.database, &session_id.0)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::NotFound)?;
        let (kind, subject) = principal_identity(principal);
        if session.owner_principal_kind != kind
            || session.owner_subject != subject
            || session.deleted_at.is_some()
        {
            return Err(AiError::NotFound);
        }
        if !self
            .access_policy
            .can_access_session(principal, session_id, action)
            .await
            .is_allowed()
            || !self
                .access_policy
                .can_access_scope(principal, &record_scope(&session), action)
                .await
                .is_allowed()
        {
            return Err(AiError::Forbidden);
        }
        Ok(session)
    }

    async fn current_claim_record(
        &self,
        claim: &AiSessionTitleWorkClaim,
        now: OffsetDateTime,
    ) -> Result<AiSessionTitleWorkRecord, AiError> {
        let record = AiSessionTitleWorkRecord::find_by_id(&self.database, &claim.work_id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::NotFound)?;
        validate_active_claim_record(&record, claim, now).map_err(map_orm)?;
        Ok(record)
    }

    async fn protection_policy(
        &self,
        principal: &AuthPrincipal,
        scope: &crate::AiScope,
    ) -> Result<AiContentProtectionPolicy, AiError> {
        let policy = self.protection_policy.resolve(principal, scope).await?;
        if !policy.ready || policy.scope != *scope {
            return Err(AiError::RuntimeNotReady);
        }
        Ok(policy)
    }

    async fn protect_value(
        &self,
        policy: &AiContentProtectionPolicy,
        context: ContentProtectionContext,
        value: serde_json::Value,
    ) -> Result<serde_json::Value, AiError> {
        let envelope = self
            .content_protector
            .protect(policy, &context, value)
            .await
            .map_err(map_protection)?;
        serde_json::to_value(envelope).map_err(|_| AiError::PersistenceFailed)
    }

    async fn open_value(
        &self,
        policy: &AiContentProtectionPolicy,
        context: ContentProtectionContext,
        value: &serde_json::Value,
    ) -> Result<serde_json::Value, AiError> {
        let envelope: ProtectedContentEnvelope =
            serde_json::from_value(value.clone()).map_err(|_| AiError::PersistenceFailed)?;
        self.content_protector
            .open(policy, &context, &envelope)
            .await
            .map_err(map_protection)
    }

    async fn claim_once(
        &self,
        worker_id: String,
        now: OffsetDateTime,
    ) -> Result<Option<AiSessionTitleWorkClaim>, TransactionError> {
        let expiry = now.checked_add(self.limits.lease_ttl).ok_or_else(|| {
            TransactionError::Rejected(OrmPublicError::new(OrmErrorCode::InternalError))
        })?;
        let limit = i64::try_from(self.limits.maximum_candidate_scan).map_err(|_| {
            TransactionError::Rejected(OrmPublicError::new(OrmErrorCode::InvalidInput))
        })?;
        let filter_time = i32::try_from(now.unix_timestamp()).map_err(|_| {
            TransactionError::Rejected(OrmPublicError::new(OrmErrorCode::InternalError))
        })?;
        self.database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let mut candidates = tx
                        .query::<AiSessionTitleWorkRecord>()
                        .filter(AiSessionTitleWorkRecordWhereInput {
                            state: Some(StringFilter {
                                in_list: Some(vec![
                                    "queued".to_owned(),
                                    "retry_scheduled".to_owned(),
                                ]),
                                ..Default::default()
                            }),
                            next_attempt_at: Some(IntFilter {
                                lte: Some(filter_time),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .default_order()
                        .limit(limit)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)?;
                    let mut expired_leases = tx
                        .query::<AiSessionTitleWorkRecord>()
                        .filter(AiSessionTitleWorkRecordWhereInput {
                            state: Some(StringFilter {
                                eq: Some("leased".to_owned()),
                                ..Default::default()
                            }),
                            lease_expires_at: Some(IntFilter {
                                lte: Some(filter_time),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .default_order()
                        .limit(limit)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)?;
                    candidates.append(&mut expired_leases);
                    candidates.sort_by_key(|candidate| (candidate.created_at, candidate.id));
                    candidates.truncate(
                        usize::try_from(limit)
                            .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))?,
                    );
                    for candidate in candidates {
                        validate_work_record(&candidate)?;
                        if !work_is_eligible(&candidate, now.unix_timestamp()) {
                            continue;
                        }
                        let session = tx
                            .find_by_id::<AiSessionRecord>(&candidate.session_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if session.deleted_at.is_some()
                            || matches!(session.state.as_str(), "deleting" | "deleted")
                            || session.title_source != "default"
                            || session.title_revision != candidate.expected_title_revision
                        {
                            let outcome = tx
                                .compare_and_swap::<AiSessionTitleWorkRecord>(
                                    &candidate.id,
                                    candidate.row_version,
                                    AiSessionTitleWorkRecordWhereInput::default(),
                                    UpdateAiSessionTitleWorkRecordInput {
                                        state: Some("superseded".to_owned()),
                                        lease_owner: Some(None),
                                        lease_expires_at: Some(None),
                                        next_attempt_at: Some(None),
                                        completed_at: Some(Some(now.unix_timestamp())),
                                        ..Default::default()
                                    },
                                )
                                .await
                                .map_err(OrmPublicError::from)?;
                            if !matches!(outcome, ConditionalUpdateOutcome::Updated(_)) {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                            return Ok(None);
                        }
                        let generation = candidate
                            .lease_generation
                            .checked_add(1)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        let outcome = tx
                            .compare_and_swap::<AiSessionTitleWorkRecord>(
                                &candidate.id,
                                candidate.row_version,
                                AiSessionTitleWorkRecordWhereInput::default(),
                                UpdateAiSessionTitleWorkRecordInput {
                                    state: Some("leased".to_owned()),
                                    lease_owner: Some(Some(worker_id.clone())),
                                    lease_generation: Some(generation),
                                    lease_expires_at: Some(Some(expiry.unix_timestamp())),
                                    next_attempt_at: Some(None),
                                    error_code: Some(None),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        let ConditionalUpdateOutcome::Updated(updated) = outcome else {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        };
                        return lease_from_record(&updated).map(Some);
                    }
                    Ok(None)
                })
            })
            .await
    }

    async fn update_terminal_failure(
        &self,
        claim: &AiSessionTitleWorkClaim,
        error_code: String,
    ) -> Result<(), AiError> {
        validate_safe_error_code(&error_code)?;
        let now = canonical_second(self.clock.now());
        let claim = claim.clone();
        self.database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let current = load_active_claim(tx, &claim, now).await?;
                    let outcome = tx
                        .compare_and_swap::<AiSessionTitleWorkRecord>(
                            &current.id,
                            current.row_version,
                            AiSessionTitleWorkRecordWhereInput::default(),
                            UpdateAiSessionTitleWorkRecordInput {
                                state: Some("failed".to_owned()),
                                lease_owner: Some(None),
                                lease_expires_at: Some(None),
                                next_attempt_at: Some(None),
                                error_code: Some(Some(error_code)),
                                completed_at: Some(Some(now.unix_timestamp())),
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

#[async_trait]
impl AiSessionTitleWorkService for OrmAiSessionTitleWorkService {
    async fn claim_next(
        &self,
        worker_id: &str,
    ) -> Result<Option<AiSessionTitleWorkClaim>, AiError> {
        validate_worker_id(worker_id)?;
        let now = canonical_second(self.clock.now());
        for retry in 0..=self.limits.maximum_transaction_retries {
            match self.claim_once(worker_id.to_owned(), now).await {
                Ok(claim) => return Ok(claim),
                Err(TransactionError::Retryable(_))
                    if retry < self.limits.maximum_transaction_retries =>
                {
                    tokio::task::yield_now().await;
                }
                Err(error) => return Err(map_transaction(error)),
            }
        }
        Err(AiError::PersistenceFailed)
    }

    async fn open_first_message(
        &self,
        claim: &AiSessionTitleWorkClaim,
    ) -> Result<AiSessionTitleWorkInput, AiError> {
        let now = canonical_second(self.clock.now());
        self.current_claim_record(claim, now).await?;
        let first = self.resolve_current(&claim.principal_reference).await?;
        let session = self
            .load_owned_session(first.principal(), claim.session_id, AiSessionAction::Read)
            .await?;
        if !matches!(session.state.as_str(), "active" | "archived")
            || session.title_source != "default"
            || session.title_revision != claim.expected_title_revision
        {
            return Err(AiError::Conflict);
        }
        let message = AiMessageRecord::find_by_id(&self.database, &claim.input_message_id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::NotFound)?;
        if message.session_id != session.id
            || message.sequence != 1
            || message.message_role != "user"
            || message.content_purged_at.is_some()
            || message.block_count != 1
            || message.completion_state != "complete"
        {
            return Err(AiError::PersistenceFailed);
        }
        let message_id = message.id;
        let block = self
            .database
            .transaction(TransactionMode::Default, move |tx| {
                Box::pin(async move {
                    tx.query::<AiMessageBlockRecord>()
                        .filter(AiMessageBlockRecordWhereInput {
                            message_id: Some(UuidFilter {
                                eq: Some(message_id),
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
        if block.len() != 1
            || block[0].block_index != 0
            || block[0].block_kind != "text"
            || block[0].message_id != message.id
        {
            return Err(AiError::PersistenceFailed);
        }
        let scope = record_scope(&session);
        let policy = self.protection_policy(first.principal(), &scope).await?;
        let opened = self
            .open_value(
                &policy,
                content_context(
                    "graphql_orm_ai_message_blocks",
                    block[0].id,
                    "protected_content",
                    &scope,
                ),
                &block[0].protected_content,
            )
            .await?;
        let object = opened.as_object().ok_or(AiError::PersistenceFailed)?;
        let text = object
            .get("text")
            .and_then(serde_json::Value::as_str)
            .filter(|text| {
                object.len() == 1
                    && !text.trim().is_empty()
                    && text.len() <= self.limits.maximum_message_bytes
            })
            .ok_or(AiError::PersistenceFailed)?
            .to_owned();
        if i64::try_from(text.len()).ok() != Some(block[0].byte_count)
            || i64::try_from(text.lines().count().max(1)).ok() != Some(block[0].line_count)
        {
            return Err(AiError::PersistenceFailed);
        }

        let second = self.resolve_current(&claim.principal_reference).await?;
        if first.reference() != second.reference() {
            return Err(AiError::ReauthorizationFailed);
        }
        let current = self
            .load_owned_session(second.principal(), claim.session_id, AiSessionAction::Read)
            .await?;
        if current.owner_principal_kind != session.owner_principal_kind
            || current.owner_subject != session.owner_subject
            || current.scope_kind != session.scope_kind
            || current.scope_id != session.scope_id
            || current.tenant_id != session.tenant_id
            || current.deleted_at.is_some()
            || !matches!(current.state.as_str(), "active" | "archived")
            || current.title_source != "default"
            || current.title_revision != claim.expected_title_revision
        {
            return Err(AiError::Conflict);
        }
        let current_policy = self.protection_policy(second.principal(), &scope).await?;
        if current_policy != policy {
            return Err(AiError::ReauthorizationFailed);
        }
        let message_id = message.id;
        let message_row_version = message.row_version;
        let block_id = block[0].id;
        let protected_content = block[0].protected_content.clone();
        self.database
            .transaction(TransactionMode::Default, move |tx| {
                Box::pin(async move {
                    let current_message = tx
                        .find_by_id::<AiMessageRecord>(&message_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    let current_block = tx
                        .find_by_id::<AiMessageBlockRecord>(&block_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    if current_message.row_version != message_row_version
                        || current_message.content_purged_at.is_some()
                        || current_message.block_count != 1
                        || current_block.message_id != message_id
                        || current_block.protected_content != protected_content
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    Ok(())
                })
            })
            .await
            .map_err(map_transaction)?;
        self.current_claim_record(claim, canonical_second(self.clock.now()))
            .await?;
        Ok(AiSessionTitleWorkInput::new(claim.session_id, text))
    }

    async fn heartbeat(
        &self,
        claim: &AiSessionTitleWorkClaim,
    ) -> Result<AiSessionTitleWorkClaim, AiError> {
        let now = canonical_second(self.clock.now());
        let expiry = now
            .checked_add(self.limits.lease_ttl)
            .ok_or_else(|| AiError::InvalidConfiguration("title-work lease overflow".to_owned()))?;
        let claim = claim.clone();
        self.database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let current = load_active_claim(tx, &claim, now).await?;
                    let outcome = tx
                        .compare_and_swap::<AiSessionTitleWorkRecord>(
                            &current.id,
                            current.row_version,
                            AiSessionTitleWorkRecordWhereInput::default(),
                            UpdateAiSessionTitleWorkRecordInput {
                                lease_expires_at: Some(Some(expiry.unix_timestamp())),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?;
                    let ConditionalUpdateOutcome::Updated(updated) = outcome else {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    };
                    lease_from_record(&updated)
                })
            })
            .await
            .map_err(map_transaction)
    }

    async fn complete(
        &self,
        claim: &AiSessionTitleWorkClaim,
        title: String,
    ) -> Result<AiSessionTitleCommitOutcome, AiError> {
        let title = normalize_title(title, self.limits.maximum_title_bytes)?;
        let title_hash = session_title_hash(&title, AiSessionTitleActor::ReviewedTitleWorker);
        let resolved = self.resolve_current(&claim.principal_reference).await?;
        let observed = self
            .load_owned_session(
                resolved.principal(),
                claim.session_id,
                AiSessionAction::Write,
            )
            .await?;
        let scope = record_scope(&observed);
        let policy = self.protection_policy(resolved.principal(), &scope).await?;
        let event_id = Uuid::new_v4();
        let inbox_event_id = Uuid::new_v4();
        let next_revision = claim
            .expected_title_revision
            .checked_add(1)
            .ok_or(AiError::Conflict)?;
        let protected_event = self
            .protect_value(
                &policy,
                content_context(
                    "graphql_orm_ai_session_events",
                    event_id,
                    "protected_payload",
                    &scope,
                ),
                json!({
                    "sessionId": claim.session_id.0,
                    "title": title,
                    "titleRevision": next_revision,
                    "actor": AiSessionTitleActor::ReviewedTitleWorker.as_str(),
                }),
            )
            .await?;
        let protected_inbox_event = self
            .protect_value(
                &policy,
                content_context(
                    "graphql_orm_ai_inbox_events",
                    inbox_event_id,
                    "protected_payload",
                    &scope,
                ),
                json!({
                    "sessionId": claim.session_id.0,
                    "title": title,
                    "titleRevision": next_revision,
                    "actor": AiSessionTitleActor::ReviewedTitleWorker.as_str(),
                }),
            )
            .await?;
        let (principal_kind, principal_subject) = principal_identity(resolved.principal());
        let principal_subject = principal_subject.to_owned();
        let now = canonical_second(self.clock.now());
        let claim = claim.clone();
        let outcome = self
            .database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let work = tx
                        .find_by_id::<AiSessionTitleWorkRecord>(&claim.work_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    if work.state == "superseded" {
                        return Ok(TitleCompletion::Superseded);
                    }
                    if work.state == "completed" {
                        if work.result_title_hash.as_deref() != Some(title_hash.as_str()) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let session = tx
                            .find_by_id::<AiSessionRecord>(&claim.session_id.0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if session.owner_principal_kind != principal_kind
                            || session.owner_subject != principal_subject
                        {
                            return Err(OrmPublicError::not_found());
                        }
                        return Ok(TitleCompletion::Applied(Box::new(session)));
                    }
                    validate_active_claim_record(&work, &claim, now)?;
                    let session = tx
                        .find_by_id::<AiSessionRecord>(&claim.session_id.0)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    if session.owner_principal_kind != principal_kind
                        || session.owner_subject != principal_subject
                    {
                        return Err(OrmPublicError::not_found());
                    }
                    if session.deleted_at.is_some()
                        || !matches!(session.state.as_str(), "active" | "archived")
                        || record_scope(&session) != scope
                        || session.title_source != "default"
                        || session.title_revision != claim.expected_title_revision
                    {
                        let updated = tx
                            .compare_and_swap::<AiSessionTitleWorkRecord>(
                                &work.id,
                                work.row_version,
                                AiSessionTitleWorkRecordWhereInput::default(),
                                UpdateAiSessionTitleWorkRecordInput {
                                    state: Some("superseded".to_owned()),
                                    lease_owner: Some(None),
                                    lease_expires_at: Some(None),
                                    next_attempt_at: Some(None),
                                    completed_at: Some(Some(now.unix_timestamp())),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(updated, ConditionalUpdateOutcome::Updated(_)) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        return Ok(TitleCompletion::Superseded);
                    }
                    let event_sequence = session
                        .stream_head
                        .checked_add(1)
                        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                    let updated_session = tx
                        .compare_and_swap::<AiSessionRecord>(
                            &session.id,
                            session.row_version,
                            AiSessionRecordWhereInput::default(),
                            UpdateAiSessionRecordInput {
                                title: Some(title.clone()),
                                title_revision: Some(next_revision),
                                title_source: Some(
                                    AiSessionTitleActor::ReviewedTitleWorker.as_str().to_owned(),
                                ),
                                stream_head: Some(event_sequence),
                                last_activity_at: Some(now.unix_timestamp()),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?;
                    let ConditionalUpdateOutcome::Updated(updated_session) = updated_session else {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    };
                    tx.insert::<AiSessionEventRecord>(CreateAiSessionEventRecordInput {
                        id: event_id,
                        session_id: claim.session_id.0,
                        sequence: event_sequence,
                        event_type: "session_title_changed".to_owned(),
                        run_id: None,
                        causation_id: Some(claim.work_id.to_string()),
                        correlation_id: claim.work_id.to_string(),
                        protected_payload: protected_event,
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                    append_inbox_event(
                        tx,
                        PreparedAiInboxEvent {
                            id: inbox_event_id,
                            principal_kind,
                            principal_subject,
                            scope,
                            session_id: claim.session_id.0,
                            event_type: "session_title_changed".to_owned(),
                            protected_payload: protected_inbox_event,
                            created_at: now.unix_timestamp(),
                        },
                    )
                    .await?;
                    tx.queue_event(AiSessionWakeup {
                        session_id: claim.session_id.0,
                        sequence: event_sequence,
                    });
                    let updated_work = tx
                        .compare_and_swap::<AiSessionTitleWorkRecord>(
                            &work.id,
                            work.row_version,
                            AiSessionTitleWorkRecordWhereInput::default(),
                            UpdateAiSessionTitleWorkRecordInput {
                                state: Some("completed".to_owned()),
                                lease_owner: Some(None),
                                lease_expires_at: Some(None),
                                next_attempt_at: Some(None),
                                result_title_hash: Some(Some(title_hash)),
                                completed_at: Some(Some(now.unix_timestamp())),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?;
                    if !matches!(updated_work, ConditionalUpdateOutcome::Updated(_)) {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    Ok(TitleCompletion::Applied(Box::new(updated_session)))
                })
            })
            .await
            .map_err(map_transaction)?;
        Ok(match outcome {
            TitleCompletion::Applied(session) => {
                AiSessionTitleCommitOutcome::Applied(session_view(&session))
            }
            TitleCompletion::Superseded => AiSessionTitleCommitOutcome::Superseded,
        })
    }

    async fn schedule_retry(
        &self,
        claim: &AiSessionTitleWorkClaim,
        delay: Duration,
        error_code: String,
    ) -> Result<(), AiError> {
        validate_safe_error_code(&error_code)?;
        if delay.is_negative()
            || delay > self.limits.maximum_retry_delay
            || claim.retry_count >= self.limits.maximum_retries
        {
            return Err(AiError::InvalidInput("invalid title-work retry".to_owned()));
        }
        let now = canonical_second(self.clock.now());
        let eligible_at = now
            .checked_add(delay)
            .ok_or_else(|| AiError::InvalidConfiguration("title-work retry overflow".to_owned()))?;
        let claim = claim.clone();
        self.database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let current = load_active_claim(tx, &claim, now).await?;
                    let retry_count = current
                        .retry_count
                        .checked_add(1)
                        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                    let outcome = tx
                        .compare_and_swap::<AiSessionTitleWorkRecord>(
                            &current.id,
                            current.row_version,
                            AiSessionTitleWorkRecordWhereInput::default(),
                            UpdateAiSessionTitleWorkRecordInput {
                                state: Some("retry_scheduled".to_owned()),
                                lease_owner: Some(None),
                                lease_expires_at: Some(None),
                                retry_count: Some(retry_count),
                                next_attempt_at: Some(Some(eligible_at.unix_timestamp())),
                                error_code: Some(Some(error_code)),
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

    async fn fail(
        &self,
        claim: &AiSessionTitleWorkClaim,
        error_code: String,
    ) -> Result<(), AiError> {
        self.update_terminal_failure(claim, error_code).await
    }
}

enum TitleCompletion {
    Applied(Box<AiSessionRecord>),
    Superseded,
}

async fn load_active_claim(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    claim: &AiSessionTitleWorkClaim,
    now: OffsetDateTime,
) -> Result<AiSessionTitleWorkRecord, OrmPublicError> {
    let current = tx
        .find_by_id::<AiSessionTitleWorkRecord>(&claim.work_id)
        .await
        .map_err(OrmPublicError::from)?
        .ok_or_else(OrmPublicError::not_found)?;
    validate_active_claim_record(&current, claim, now)?;
    Ok(current)
}

fn validate_active_claim_record(
    current: &AiSessionTitleWorkRecord,
    claim: &AiSessionTitleWorkClaim,
    now: OffsetDateTime,
) -> Result<(), OrmPublicError> {
    let reference: PrincipalReference = serde_json::from_value(current.principal_reference.clone())
        .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))?;
    if current.id != claim.work_id
        || current.session_id != claim.session_id.0
        || current.input_message_id != claim.input_message_id
        || reference != claim.principal_reference
        || current.state != "leased"
        || current.lease_owner.as_deref() != Some(claim.worker_id.as_str())
        || current.lease_generation != claim.lease_generation
        || current.lease_expires_at != Some(claim.lease_expires_at.unix_timestamp())
        || current
            .lease_expires_at
            .is_none_or(|expires_at| expires_at <= now.unix_timestamp())
        || current.retry_count != i64::from(claim.retry_count)
        || current.row_version != claim.row_version
        || current.expected_title_revision != claim.expected_title_revision
    {
        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
    }
    Ok(())
}

fn lease_from_record(
    record: &AiSessionTitleWorkRecord,
) -> Result<AiSessionTitleWorkClaim, OrmPublicError> {
    validate_work_record(record)?;
    let principal_reference = serde_json::from_value(record.principal_reference.clone())
        .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))?;
    let worker_id = record
        .lease_owner
        .clone()
        .filter(|worker| validate_worker_id(worker).is_ok())
        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
    let lease_expires_at = record
        .lease_expires_at
        .and_then(|value| OffsetDateTime::from_unix_timestamp(value).ok())
        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
    if record.state != "leased" || record.lease_generation <= 0 {
        return Err(OrmPublicError::new(OrmErrorCode::InternalError));
    }
    Ok(AiSessionTitleWorkClaim {
        work_id: record.id,
        session_id: AiSessionId(record.session_id),
        input_message_id: record.input_message_id,
        principal_reference,
        worker_id,
        lease_generation: record.lease_generation,
        lease_expires_at,
        retry_count: u32::try_from(record.retry_count)
            .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))?,
        row_version: record.row_version,
        expected_title_revision: record.expected_title_revision,
    })
}

fn validate_work_record(record: &AiSessionTitleWorkRecord) -> Result<(), OrmPublicError> {
    let valid_state = matches!(
        record.state.as_str(),
        "queued" | "leased" | "retry_scheduled" | "completed" | "failed" | "superseded"
    );
    if record.id.is_nil()
        || record.session_id.is_nil()
        || record.input_message_id.is_nil()
        || record.expected_title_revision < 0
        || record.lease_generation < 0
        || record.retry_count < 0
        || record.row_version < 0
        || !valid_state
        || serde_json::from_value::<PrincipalReference>(record.principal_reference.clone()).is_err()
    {
        return Err(OrmPublicError::new(OrmErrorCode::InternalError));
    }
    let active_shape = match record.state.as_str() {
        "queued" | "retry_scheduled" => {
            record.lease_owner.is_none()
                && record.lease_expires_at.is_none()
                && record.next_attempt_at.is_some()
                && record.completed_at.is_none()
                && record.result_title_hash.is_none()
        }
        "leased" => {
            record
                .lease_owner
                .as_deref()
                .is_some_and(|worker| validate_worker_id(worker).is_ok())
                && record.lease_expires_at.is_some()
                && record.next_attempt_at.is_none()
                && record.completed_at.is_none()
                && record.result_title_hash.is_none()
        }
        "completed" => {
            record.lease_owner.is_none()
                && record.lease_expires_at.is_none()
                && record.next_attempt_at.is_none()
                && record.completed_at.is_some()
                && record
                    .result_title_hash
                    .as_ref()
                    .is_some_and(|hash| hash.len() == 64)
        }
        "failed" | "superseded" => {
            record.lease_owner.is_none()
                && record.lease_expires_at.is_none()
                && record.next_attempt_at.is_none()
                && record.completed_at.is_some()
                && record.result_title_hash.is_none()
        }
        _ => false,
    };
    if !active_shape {
        return Err(OrmPublicError::new(OrmErrorCode::InternalError));
    }
    Ok(())
}

fn work_is_eligible(record: &AiSessionTitleWorkRecord, now: i64) -> bool {
    match record.state.as_str() {
        "queued" | "retry_scheduled" => record.next_attempt_at.is_some_and(|value| value <= now),
        "leased" => record.lease_expires_at.is_some_and(|value| value <= now),
        _ => false,
    }
}

fn validate_safe_error_code(value: &str) -> Result<(), AiError> {
    if value.is_empty()
        || value.len() > MAXIMUM_SAFE_ERROR_CODE_BYTES
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
    {
        return Err(AiError::InvalidInput(
            "invalid redacted title-work error code".to_owned(),
        ));
    }
    Ok(())
}
