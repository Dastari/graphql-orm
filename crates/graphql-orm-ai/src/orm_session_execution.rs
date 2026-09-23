//! Durable session-selection reads and exact legacy evidence checks.

use agql_auth::{Clock, CurrentPrincipalResolver};
use async_trait::async_trait;
use graphql_orm::db::Database;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::graphql::filters::{StringFilter, UuidFilter};
use graphql_orm::graphql::orm::{DefaultWriteBackend, MutationContext, TransactionMode};
use std::sync::Arc;

use crate::orm_runs::load_and_validate_active_lease;
use crate::orm_sessions::{map_transaction, principal_identity, record_scope};
use crate::persistence::*;
use crate::{
    AiAccessPolicy, AiError, AiRunExecutionSelectionReader, AiRunLease, AiSessionAction,
    AiSessionExecutionSelection,
};

/// Reads immutable enqueue-time choices under the existing durable lease fence.
/// This is routing metadata only; ordinary per-egress/tool authorization remains mandatory.
pub struct OrmAiRunExecutionSelectionReader {
    database: Database<DefaultWriteBackend>,
    principal_resolver: Arc<dyn CurrentPrincipalResolver>,
    access_policy: Arc<dyn AiAccessPolicy>,
    clock: Arc<dyn Clock>,
}

impl OrmAiRunExecutionSelectionReader {
    /// Creates a reader. Resolved principals must match the recorded reference,
    /// remain unexpired and be less than 30 seconds old at the final fenced read.
    pub fn new(
        database: Database<DefaultWriteBackend>,
        principal_resolver: Arc<dyn CurrentPrincipalResolver>,
        access_policy: Arc<dyn AiAccessPolicy>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            database,
            principal_resolver,
            access_policy,
            clock,
        }
    }
}

#[async_trait]
impl AiRunExecutionSelectionReader for OrmAiRunExecutionSelectionReader {
    async fn selection_for_run(
        &self,
        lease: &AiRunLease,
    ) -> Result<AiSessionExecutionSelection, AiError> {
        let current = self
            .principal_resolver
            .resolve(lease.principal_reference())
            .await
            .map_err(|_| AiError::ReauthorizationFailed)?;
        let session = AiSessionRecord::find_by_id(&self.database, &lease.session_id().0)
            .await
            .map_err(|_| AiError::PersistenceFailed)?
            .ok_or(AiError::NotFound)?;
        let (kind, subject) = principal_identity(current.principal());
        if session.owner_principal_kind != kind
            || session.owner_subject != subject
            || session.state != "active"
            || session.deleted_at.is_some()
        {
            return Err(AiError::NotFound);
        }
        if !self
            .access_policy
            .can_access_session(
                current.principal(),
                lease.session_id(),
                AiSessionAction::Write,
            )
            .await
            .is_allowed()
            || !self
                .access_policy
                .can_access_scope(
                    current.principal(),
                    &record_scope(&session),
                    AiSessionAction::Write,
                )
                .await
                .is_allowed()
        {
            return Err(AiError::Forbidden);
        }
        let now = self.clock.now();
        if current.reference() != lease.principal_reference()
            || current.resolved_at() > now
            || now - current.resolved_at() >= time::Duration::seconds(30)
            || current.reference().expires_at.is_some_and(|v| v <= now)
        {
            return Err(AiError::ReauthorizationFailed);
        }
        let lease = lease.clone();
        let clock = self.clock.clone();
        let resolved_at = current.resolved_at();
        let principal_expires_at = current.reference().expires_at;
        let encoded = self
            .database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let now = clock.now();
                    if resolved_at > now
                        || now - resolved_at >= time::Duration::seconds(30)
                        || principal_expires_at.is_some_and(|expires| expires <= now)
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::Forbidden));
                    }
                    let run = load_and_validate_active_lease(tx, &lease, now).await?;
                    let observed = tx
                        .find_by_id::<AiSessionRecord>(&session.id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    if observed.row_version != session.row_version
                        || observed.execution_selection != run.execution_selection
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    Ok(run.execution_selection)
                })
            })
            .await
            .map_err(map_transaction)?;
        AiSessionExecutionSelection::decode(
            encoded.as_deref().ok_or(AiError::SessionExecutionUnbound)?,
        )
    }
}

/// Checks bounded historical evidence inside the same transaction as the pin.
pub(crate) async fn prove_legacy_selection(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    session: &AiSessionRecord,
    selection: &AiSessionExecutionSelection,
    descriptor: &crate::AiProviderSessionDescriptor,
    binding_version: i64,
    now: i64,
) -> Result<bool, OrmPublicError> {
    use crate::orm_provider_session::{
        AiProviderSessionBindingRecord, AiProviderSessionBindingRecordWhereInput,
    };
    let busy = tx
        .query::<AiRunRecord>()
        .filter(AiRunRecordWhereInput {
            session_id: Some(UuidFilter {
                eq: Some(session.id),
                ..Default::default()
            }),
            state: Some(StringFilter {
                not_in: Some(
                    vec!["completed", "failed", "cancelled"]
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                ),
                ..Default::default()
            }),
            ..Default::default()
        })
        .limit(1)
        .fetch_one()
        .await
        .map_err(OrmPublicError::from)?;
    if busy.is_some() {
        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
    }
    let binding = tx
        .query::<AiProviderSessionBindingRecord>()
        .filter(AiProviderSessionBindingRecordWhereInput {
            session_id: Some(UuidFilter {
                eq: Some(session.id),
                ..Default::default()
            }),
            ..Default::default()
        })
        .limit(1)
        .fetch_one()
        .await
        .map_err(OrmPublicError::from)?;
    let Some(binding) = binding else {
        return Ok(false);
    };
    if binding.row_version != binding_version {
        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
    }
    if binding.through_message_sequence != session.message_head
        || binding.state != "active"
        || binding.claimed_run_id.is_some()
        || binding.claim_owner.is_some()
        || binding.protected_cursor.is_none()
        || binding.owner_principal_kind != session.owner_principal_kind
        || binding.owner_subject != session.owner_subject
        || binding.scope_kind != session.scope_kind
        || binding.scope_id != session.scope_id
        || binding.tenant_id != session.tenant_id
        || binding.idle_expires_at <= now
        || binding.absolute_expires_at <= now
        || binding.provider_expires_at.is_some_and(|v| v <= now)
        || crate::parse_provider_kind(&binding.provider_kind)
            .ok()
            .as_ref()
            != Some(descriptor.provider_kind())
        || binding.provider_profile_id != descriptor.provider_profile_id()
        || binding.provider_model != descriptor.provider_model()
        || binding.registration_fingerprint != descriptor.registration_fingerprint()
        || binding.protocol_version != descriptor.protocol_version()
        || binding.policy_fingerprint != descriptor.policy_fingerprint()
        || selection.provider_kind() != *descriptor.provider_kind()
        || selection.model() != descriptor.provider_model()
    {
        return Ok(false);
    }
    let Some(last_run) = binding.last_run_id else {
        return Ok(false);
    };
    let rows = tx
        .query::<AiBudgetReservationRecord>()
        .filter(AiBudgetReservationRecordWhereInput {
            run_id: Some(UuidFilter {
                eq: Some(last_run),
                ..Default::default()
            }),
            ..Default::default()
        })
        .limit(257)
        .fetch_all()
        .await
        .map_err(OrmPublicError::from)?;
    if rows.is_empty() || rows.len() > 256 {
        return Ok(false);
    }
    // Every reservation in the last retained run must corroborate the exact
    // selection and settled accounting. Ambiguity, a released attempt or an
    // unspecified effort requires manual recovery, never an inferred default.
    Ok(rows.iter().all(|row| {
        row.session_id == session.id
            && row.principal_kind == session.owner_principal_kind
            && row.principal_subject == session.owner_subject
            && row.provider_kind == selection.provider_kind().as_str()
            && row.provider_model == selection.model()
            && row.reasoning_effort == selection.reasoning_effort().as_str()
            && row.state == "committed"
    }))
}

pub(crate) async fn legacy_binding(
    database: &Database<DefaultWriteBackend>,
    session_id: uuid::Uuid,
) -> Result<crate::orm_provider_session::AiProviderSessionBindingRecord, AiError> {
    use crate::orm_provider_session::{
        AiProviderSessionBindingRecord, AiProviderSessionBindingRecordWhereInput,
    };
    database
        .transaction(TransactionMode::Default, move |tx| {
            Box::pin(async move {
                tx.query::<AiProviderSessionBindingRecord>()
                    .filter(AiProviderSessionBindingRecordWhereInput {
                        session_id: Some(UuidFilter {
                            eq: Some(session_id),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .limit(1)
                    .fetch_one()
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await
        .map_err(map_transaction)?
        .ok_or(AiError::SessionExecutionUnavailable)
}

pub(crate) fn legacy_descriptor(
    binding: &crate::orm_provider_session::AiProviderSessionBindingRecord,
) -> Result<crate::AiProviderSessionDescriptor, AiError> {
    crate::AiProviderSessionDescriptor::new(
        crate::parse_provider_kind(&binding.provider_kind)?,
        binding.provider_profile_id.clone(),
        binding.provider_model.clone(),
        binding.registration_fingerprint.clone(),
        binding.protocol_version.clone(),
        binding.policy_fingerprint.clone(),
    )
}
