//! Ledger of provider-retained responses and their deletion after session
//! retention.
//!
//! A response the provider retains (`store_responses = true`) is recorded in
//! the same transaction that first persists its identifier for a run. Session
//! retention releases a deleting session's rows once the purge cutoff has
//! passed, and the session shell is finalized only after every row was
//! released. The cleanup pass then deletes each released response through
//! the host-installed deletion boundary: intent and attempt count are
//! committed before any request, only confirmed absence removes a row, and
//! the bounded attempts end in the terminal `cleanup_unconfirmed` state.

use graphql_orm::graphql::filters::Int64Filter;
use graphql_orm::graphql::orm::MutationContext;
use sha2::{Digest, Sha256};

use super::*;
use crate::{
    AiStoredProviderResponseDeletionRequest, AiStoredProviderResponseDeletionService,
    AiStoredResponseCleanupLimits, AiStoredResponseCleanupReport, ProviderKind,
};

pub(super) const RETAINED: &str = "retained";
const DELETE_REQUIRED: &str = "delete_required";
const IN_PROGRESS: &str = "cleanup_in_progress";
const BACKOFF: &str = "cleanup_backoff";
const UNCONFIRMED: &str = "cleanup_unconfirmed";

/// Deterministic ledger identity, so a response recorded twice (a retried
/// write, an adopted checkpoint) keeps one row.
fn stored_response_identity(provider_kind: &str, profile: &str, response_id: &str) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(b"graphql-orm-ai/stored-provider-response/v1\0");
    hasher.update(provider_kind.as_bytes());
    hasher.update([0]);
    hasher.update(profile.as_bytes());
    hasher.update([0]);
    hasher.update(response_id.as_bytes());
    let digest = hasher.finalize();
    let mut id = [0_u8; 16];
    id.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(id)
}

fn parse_recorded_kind(value: &str) -> Option<ProviderKind> {
    (value == ProviderKind::OpenAi.as_str()).then_some(ProviderKind::OpenAi)
}

/// Records one provider-retained response in the caller's transaction.
///
/// Only OpenAI responses are recorded; it is the provider family with a
/// deletion boundary in this crate. A response already in the ledger is left
/// unchanged.
pub(crate) async fn record_retained_provider_response(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    session_id: Uuid,
    provider_kind: &str,
    provider_profile_id: &str,
    provider_response_id: &str,
) -> Result<(), OrmPublicError> {
    if parse_recorded_kind(provider_kind).is_none() {
        return Ok(());
    }
    if !crate::attachments::valid_safe_reference(provider_profile_id, 512)
        || !crate::attachments::valid_safe_reference(provider_response_id, 1_024)
    {
        return Err(OrmPublicError::new(OrmErrorCode::InvalidInput));
    }
    let id = stored_response_identity(provider_kind, provider_profile_id, provider_response_id);
    if tx
        .find_by_id::<AiStoredProviderResponseRecord>(&id)
        .await
        .map_err(OrmPublicError::from)?
        .is_some()
    {
        return Ok(());
    }
    tx.insert::<AiStoredProviderResponseRecord>(CreateAiStoredProviderResponseRecordInput {
        id,
        session_id,
        provider_kind: provider_kind.to_owned(),
        provider_profile_id: provider_profile_id.to_owned(),
        provider_response_id: provider_response_id.to_owned(),
        cleanup_state: RETAINED.to_owned(),
        cleanup_generation: 0,
        cleanup_lease_expires_at: None,
        cleanup_next_attempt_at: None,
        provider_delete_requested_at: None,
        provider_delete_attempts: 0,
    })
    .await
    .map_err(OrmPublicError::from)?;
    Ok(())
}

/// Whether a session still has retained responses that retention has not
/// released. Finalization waits for this to become false.
pub(super) async fn session_has_retained_responses(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    session_id: Uuid,
) -> Result<bool, OrmPublicError> {
    Ok(!tx
        .query::<AiStoredProviderResponseRecord>()
        .filter(retained_for_session(session_id))
        .internal_limit(1)
        .fetch_all()
        .await
        .map_err(OrmPublicError::from)?
        .is_empty())
}

fn retained_for_session(session_id: Uuid) -> AiStoredProviderResponseRecordWhereInput {
    AiStoredProviderResponseRecordWhereInput {
        session_id: Some(UuidFilter {
            eq: Some(session_id),
            ..Default::default()
        }),
        cleanup_state: Some(StringFilter {
            eq: Some(RETAINED.to_owned()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn retry_at(attempts: i64, now: i64) -> Result<i64, AiError> {
    let shift = u32::try_from(attempts.clamp(0, 6)).unwrap_or(6);
    let delay = 60_i64.checked_shl(shift).unwrap_or(3_600).min(3_600);
    now.checked_add(delay).ok_or(AiError::PersistenceFailed)
}

/// What the deletion boundary reported for one claimed response.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DeletionResult {
    /// Acknowledged and confirmed absent.
    Confirmed,
    /// Sent but absence was not confirmed.
    Unconfirmed,
    /// The boundary does not own this kind or profile and sent nothing.
    Unrouted,
}

/// Delay before a row the installed boundary does not own is offered again.
const UNROUTED_RETRY_SECONDS: i64 = 3_600;

enum StoredResponseClaim {
    /// Claimed with intent recorded; send the deletion request.
    Send(Box<AiStoredProviderResponseRecord>, bool),
    /// The attempt bound was already reached; recorded as unconfirmed.
    Unconfirmed,
    /// Another worker claimed or changed the row first.
    Deferred,
}

impl OrmAiSessionRetentionService {
    /// Installs the deletion boundary for provider-retained responses.
    ///
    /// Without it, [`Self::delete_stored_provider_responses`] does nothing
    /// and released responses stay in the ledger until a boundary is
    /// installed. Installing it grants authority only for responses that
    /// session retention released after their session's purge cutoff.
    #[must_use]
    pub fn with_stored_response_deletion_service(
        mut self,
        service: Arc<dyn AiStoredProviderResponseDeletionService>,
    ) -> Self {
        self.stored_response_deletion = Some(service);
        self
    }

    /// Overrides the bounds of one stored-response cleanup pass.
    #[must_use]
    pub fn with_stored_response_cleanup_limits(
        mut self,
        limits: AiStoredResponseCleanupLimits,
    ) -> Self {
        self.stored_response_cleanup_limits = limits;
        self
    }

    /// Releases a deleting session's retained responses for deletion once
    /// the session's purge cutoff has passed. Returns how many rows it
    /// released; one bounded page per call.
    pub(super) async fn release_stored_provider_responses(
        &self,
        session_id: Uuid,
        now: i64,
    ) -> Result<usize, AiError> {
        let page = retention_query_page_size(&self.database)?;
        self.database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let Some(session) = tx
                        .find_by_id::<AiSessionRecord>(&session_id)
                        .await
                        .map_err(OrmPublicError::from)?
                    else {
                        return Ok(0);
                    };
                    validate_session(&session)?;
                    if session.state != "deleting" {
                        return Ok(0);
                    }
                    let deleted_at = session
                        .deleted_at
                        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                    let scope = session_scope(&session);
                    let scope_key = crate::ai_scope_key(&scope);
                    let policies = tx
                        .query::<AiRetentionPolicyRecord>()
                        .filter(AiRetentionPolicyRecordWhereInput {
                            scope_key: Some(StringFilter {
                                eq: Some(scope_key.clone()),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .internal_limit(2)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)?;
                    let [policy] = policies.as_slice() else {
                        return Ok(0);
                    };
                    if !valid_policy(policy, &scope, &scope_key)
                        || deleted_at
                            .checked_add(policy.deleted_content_purge_seconds)
                            .is_none_or(|cutoff| cutoff > now)
                    {
                        return Ok(0);
                    }
                    let rows = tx
                        .query::<AiStoredProviderResponseRecord>()
                        .filter(retained_for_session(session_id))
                        .order_by(AiStoredProviderResponseRecordOrderByInput {
                            created_at: Some(OrderDirection::Asc),
                        })
                        .internal_limit(page)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)?;
                    let mut released = 0_usize;
                    for row in rows {
                        let outcome = tx
                            .compare_and_swap::<AiStoredProviderResponseRecord>(
                                &row.id,
                                row.row_version,
                                retained_for_session(session_id),
                                UpdateAiStoredProviderResponseRecordInput {
                                    cleanup_state: Some(DELETE_REQUIRED.to_owned()),
                                    cleanup_next_attempt_at: Some(Some(now)),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(outcome, ConditionalUpdateOutcome::Updated(_)) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        released += 1;
                    }
                    Ok(released)
                })
            })
            .await
            .map_err(map_transaction)
    }

    /// Deletes one bounded batch of released provider-retained responses.
    ///
    /// Each response is claimed under a generation fence that also records
    /// the deletion intent and attempt count before the request is sent. Only
    /// confirmed absence removes a ledger row. An unconfirmed deletion,
    /// including a not-found answer to the delete itself, is retried with
    /// backoff; after the configured attempts the row becomes the terminal
    /// `cleanup_unconfirmed` state and keeps the identifier for operator
    /// follow-up. Without an installed deletion boundary the pass does
    /// nothing.
    ///
    /// # Errors
    ///
    /// Returns a safe error when the candidate scan or a ledger transaction
    /// fails. Provider failures are counted, never returned.
    pub async fn delete_stored_provider_responses(
        &self,
    ) -> Result<AiStoredResponseCleanupReport, AiError> {
        let mut report = AiStoredResponseCleanupReport::default();
        let Some(service) = self.stored_response_deletion.clone() else {
            return Ok(report);
        };
        let limits = self.stored_response_cleanup_limits;
        let now = self.clock.now().unix_timestamp();
        for candidate in self.stored_response_candidates(now, limits).await? {
            match self.claim_stored_response(&candidate, now, limits).await? {
                StoredResponseClaim::Deferred => {
                    report.deferred = report.deferred.saturating_add(1);
                }
                StoredResponseClaim::Unconfirmed => {
                    report.unconfirmed = report.unconfirmed.saturating_add(1);
                }
                StoredResponseClaim::Send(claimed, previous) => {
                    let Some(kind) = parse_recorded_kind(&claimed.provider_kind) else {
                        return Err(AiError::PersistenceFailed);
                    };
                    let request = AiStoredProviderResponseDeletionRequest::new(
                        kind,
                        claimed.provider_profile_id.clone(),
                        claimed.provider_response_id.clone(),
                        previous,
                    );
                    let result = match service.delete_and_confirm_absent(&request).await {
                        Ok(()) => DeletionResult::Confirmed,
                        Err(AiError::InvalidConfiguration(_)) => DeletionResult::Unrouted,
                        Err(_) => DeletionResult::Unconfirmed,
                    };
                    let counter = match self
                        .finish_stored_response(&claimed, result, now, limits)
                        .await?
                    {
                        None => &mut report.deferred,
                        Some(DeletionResult::Confirmed) => &mut report.deleted,
                        Some(DeletionResult::Unrouted) => &mut report.unrouted,
                        Some(DeletionResult::Unconfirmed)
                            if claimed.provider_delete_attempts
                                >= i64::from(limits.maximum_delete_attempts()) =>
                        {
                            &mut report.unconfirmed
                        }
                        Some(DeletionResult::Unconfirmed) => &mut report.retried,
                    };
                    *counter = counter.saturating_add(1);
                }
            }
        }
        Ok(report)
    }

    async fn stored_response_candidates(
        &self,
        now: i64,
        limits: AiStoredResponseCleanupLimits,
    ) -> Result<Vec<AiStoredProviderResponseRecord>, AiError> {
        let batch = limits.maximum_batch_size() as usize;
        let state = |value: &str| {
            Some(StringFilter {
                eq: Some(value.to_owned()),
                ..Default::default()
            })
        };
        let due = || {
            Some(Int64Filter {
                lte: Some(now),
                ..Default::default()
            })
        };
        // Each query selects only rows that are due, so rows waiting out a
        // backoff or a live claim never fill the window ahead of due ones.
        let queries = [
            AiStoredProviderResponseRecordWhereInput {
                cleanup_state: state(DELETE_REQUIRED),
                ..Default::default()
            },
            AiStoredProviderResponseRecordWhereInput {
                cleanup_state: state(BACKOFF),
                cleanup_next_attempt_at: due(),
                ..Default::default()
            },
            AiStoredProviderResponseRecordWhereInput {
                cleanup_state: state(IN_PROGRESS),
                cleanup_lease_expires_at: due(),
                ..Default::default()
            },
        ];
        let mut candidates = Vec::new();
        for filter in queries {
            if candidates.len() >= batch {
                break;
            }
            let remaining =
                i64::try_from(batch - candidates.len()).map_err(|_| AiError::PersistenceFailed)?;
            let rows = self
                .database
                .transaction(TransactionMode::Default, move |tx| {
                    Box::pin(async move {
                        tx.query::<AiStoredProviderResponseRecord>()
                            .filter(filter)
                            .order_by(AiStoredProviderResponseRecordOrderByInput {
                                created_at: Some(OrderDirection::Asc),
                            })
                            .internal_limit(remaining)
                            .fetch_all()
                            .await
                            .map_err(OrmPublicError::from)
                    })
                })
                .await
                .map_err(map_transaction)?;
            candidates.extend(rows);
        }
        Ok(candidates)
    }

    async fn claim_stored_response(
        &self,
        candidate: &AiStoredProviderResponseRecord,
        now: i64,
        limits: AiStoredResponseCleanupLimits,
    ) -> Result<StoredResponseClaim, AiError> {
        let id = candidate.id;
        let lease_expires_at = now
            .checked_add(limits.cleanup_lease_ttl().whole_seconds())
            .ok_or(AiError::PersistenceFailed)?;
        let maximum_attempts = i64::from(limits.maximum_delete_attempts());
        let correlation_id = format!("stored-response-cleanup:{id}");
        self.database
            .transaction(TransactionMode::StateMachine, move |tx| {
                let correlation_id = correlation_id.clone();
                Box::pin(async move {
                    let Some(current) = tx
                        .find_by_id::<AiStoredProviderResponseRecord>(&id)
                        .await
                        .map_err(OrmPublicError::from)?
                    else {
                        return Ok(StoredResponseClaim::Deferred);
                    };
                    let due = match current.cleanup_state.as_str() {
                        DELETE_REQUIRED => true,
                        BACKOFF => current
                            .cleanup_next_attempt_at
                            .is_some_and(|due| due <= now),
                        IN_PROGRESS => current
                            .cleanup_lease_expires_at
                            .is_some_and(|expiry| expiry <= now),
                        _ => false,
                    };
                    if !due {
                        return Ok(StoredResponseClaim::Deferred);
                    }
                    let generation = current
                        .cleanup_generation
                        .checked_add(1)
                        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                    let fence = AiStoredProviderResponseRecordWhereInput {
                        cleanup_state: Some(StringFilter {
                            eq: Some(current.cleanup_state.clone()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    };
                    if current.provider_delete_attempts >= maximum_attempts {
                        // An interrupted last attempt: the bound is spent.
                        let outcome = tx
                            .compare_and_swap::<AiStoredProviderResponseRecord>(
                                &id,
                                current.row_version,
                                fence,
                                UpdateAiStoredProviderResponseRecordInput {
                                    cleanup_state: Some(UNCONFIRMED.to_owned()),
                                    cleanup_generation: Some(generation),
                                    cleanup_lease_expires_at: Some(None),
                                    cleanup_next_attempt_at: Some(None),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(outcome, ConditionalUpdateOutcome::Updated(_)) {
                            return Ok(StoredResponseClaim::Deferred);
                        }
                        insert_cleanup_audit(tx, id, "unconfirmed", correlation_id).await?;
                        return Ok(StoredResponseClaim::Unconfirmed);
                    }
                    let previous = current.provider_delete_requested_at.is_some();
                    let attempts = current
                        .provider_delete_attempts
                        .checked_add(1)
                        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                    // Intent and attempt count commit with the claim, before
                    // any request is sent.
                    let outcome = tx
                        .compare_and_swap::<AiStoredProviderResponseRecord>(
                            &id,
                            current.row_version,
                            fence,
                            UpdateAiStoredProviderResponseRecordInput {
                                cleanup_state: Some(IN_PROGRESS.to_owned()),
                                cleanup_generation: Some(generation),
                                cleanup_lease_expires_at: Some(Some(lease_expires_at)),
                                cleanup_next_attempt_at: Some(None),
                                provider_delete_requested_at: Some(Some(now)),
                                provider_delete_attempts: Some(attempts),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?;
                    match outcome {
                        ConditionalUpdateOutcome::Updated(claimed) => {
                            Ok(StoredResponseClaim::Send(Box::new(claimed), previous))
                        }
                        ConditionalUpdateOutcome::NotFound | ConditionalUpdateOutcome::Conflict => {
                            Ok(StoredResponseClaim::Deferred)
                        }
                    }
                })
            })
            .await
            .map_err(map_transaction)
    }

    /// Settles one claimed deletion. Returns `None` when the claim was
    /// superseded, otherwise the settled result.
    async fn finish_stored_response(
        &self,
        claimed: &AiStoredProviderResponseRecord,
        result: DeletionResult,
        now: i64,
        limits: AiStoredResponseCleanupLimits,
    ) -> Result<Option<DeletionResult>, AiError> {
        let id = claimed.id;
        let version = claimed.row_version;
        let generation = claimed.cleanup_generation;
        let confirmed = result == DeletionResult::Confirmed;
        let unrouted = result == DeletionResult::Unrouted;
        let terminal = result == DeletionResult::Unconfirmed
            && claimed.provider_delete_attempts >= i64::from(limits.maximum_delete_attempts());
        let next_attempt_at = if unrouted {
            now.checked_add(UNROUTED_RETRY_SECONDS)
                .ok_or(AiError::PersistenceFailed)?
        } else {
            retry_at(claimed.provider_delete_attempts, now)?
        };
        // Nothing was sent for an unrouted row, so its attempt is returned.
        let attempts_after = if unrouted {
            claimed.provider_delete_attempts.saturating_sub(1).max(0)
        } else {
            claimed.provider_delete_attempts
        };
        let correlation_id = format!("stored-response-cleanup:{id}:{generation}");
        self.database
            .transaction(TransactionMode::StateMachine, move |tx| {
                let correlation_id = correlation_id.clone();
                Box::pin(async move {
                    let Some(current) = tx
                        .find_by_id::<AiStoredProviderResponseRecord>(&id)
                        .await
                        .map_err(OrmPublicError::from)?
                    else {
                        return Ok(None);
                    };
                    if current.row_version != version
                        || current.cleanup_generation != generation
                        || current.cleanup_state != IN_PROGRESS
                    {
                        return Ok(None);
                    }
                    if confirmed {
                        if !tx
                            .delete_by_id::<AiStoredProviderResponseRecord>(&id)
                            .await
                            .map_err(OrmPublicError::from)?
                        {
                            return Ok(None);
                        }
                        insert_cleanup_audit(tx, id, "succeeded", correlation_id).await?;
                        return Ok(Some(DeletionResult::Confirmed));
                    }
                    let update = if terminal {
                        UpdateAiStoredProviderResponseRecordInput {
                            cleanup_state: Some(UNCONFIRMED.to_owned()),
                            cleanup_lease_expires_at: Some(None),
                            cleanup_next_attempt_at: Some(None),
                            ..Default::default()
                        }
                    } else {
                        UpdateAiStoredProviderResponseRecordInput {
                            cleanup_state: Some(BACKOFF.to_owned()),
                            cleanup_lease_expires_at: Some(None),
                            cleanup_next_attempt_at: Some(Some(next_attempt_at)),
                            provider_delete_attempts: Some(attempts_after),
                            ..Default::default()
                        }
                    };
                    let outcome = tx
                        .compare_and_swap::<AiStoredProviderResponseRecord>(
                            &id,
                            version,
                            AiStoredProviderResponseRecordWhereInput::default(),
                            update,
                        )
                        .await
                        .map_err(OrmPublicError::from)?;
                    if !matches!(outcome, ConditionalUpdateOutcome::Updated(_)) {
                        return Ok(None);
                    }
                    insert_cleanup_audit(
                        tx,
                        id,
                        if terminal {
                            "unconfirmed"
                        } else if unrouted {
                            "unrouted"
                        } else {
                            "failed"
                        },
                        correlation_id,
                    )
                    .await?;
                    Ok(Some(result))
                })
            })
            .await
            .map_err(map_transaction)
    }
}

async fn insert_cleanup_audit(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    id: Uuid,
    outcome: &str,
    correlation_id: String,
) -> Result<(), OrmPublicError> {
    let reason_code = match outcome {
        "succeeded" => "stored_response_deleted",
        "unconfirmed" => "stored_response_cleanup_attempts_exhausted",
        "unrouted" => "stored_response_boundary_not_configured",
        _ => "stored_response_cleanup_unconfirmed",
    };
    tx.insert::<AiAuditEventRecord>(CreateAiAuditEventRecordInput {
        actor_principal_kind: "system".to_owned(),
        actor_subject: "session-retention".to_owned(),
        action: "delete_stored_provider_response".to_owned(),
        resource_kind: "ai_stored_provider_response".to_owned(),
        resource_reference: id.to_string(),
        outcome: outcome.to_owned(),
        reason_code: reason_code.to_owned(),
        correlation_id,
        causation_id: Some(id.to_string()),
        policy_version: Some("stored-response-cleanup-v1".to_owned()),
    })
    .await
    .map_err(OrmPublicError::from)?;
    Ok(())
}
