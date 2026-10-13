// Dedicated native static-offering reset. Generic rebind reasons remain unchanged.

async fn validate_static_reset_graph(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    lease: &AiRunLease,
    request: &crate::AiProviderStaticOfferingResetRequest,
    now: OffsetDateTime,
) -> Result<(), OrmPublicError> {
    use crate::persistence::{
        AiBudgetReservationRecord, AiBudgetReservationRecordWhereInput, AiRunStepRecord,
    };
    let conflict = || OrmPublicError::new(OrmErrorCode::Conflict);
    let evidence = &request.evidence;
    let run = load_and_validate_active_lease(tx, lease, now).await?;
    if run.state != AiRunState::Running.as_str()
        || run.latest_checkpoint_id.is_some()
        || run.cancellation_request_id.is_some()
        || evidence.session_id != lease.session_id()
        || evidence.run_id != lease.run_id()
        || evidence.attempt_id != lease.attempt_id()
        || evidence.lease_generation != lease.lease_generation()
        || !crate::valid_sha256(&request.desired_offering_fingerprint)
    {
        return Err(conflict());
    }
    let checkpoint = tx
        .find_by_id::<AiRunCheckpointRecord>(&evidence.checkpoint_id)
        .await
        .map_err(OrmPublicError::from)?
        .ok_or_else(conflict)?;
    let call = tx
        .find_by_id::<AiToolCallRecord>(&evidence.tool_call_id)
        .await
        .map_err(OrmPublicError::from)?
        .ok_or_else(conflict)?;
    let step = tx
        .find_by_id::<AiRunStepRecord>(&evidence.tool_call_id)
        .await
        .map_err(OrmPublicError::from)?
        .ok_or_else(conflict)?;
    let approval = tx
        .find_by_id::<AiApprovalRecord>(&evidence.approval_id)
        .await
        .map_err(OrmPublicError::from)?
        .ok_or_else(conflict)?;
    if checkpoint.run_id != run.id
        || checkpoint.attempt_id != lease.attempt_id()
        || checkpoint.lease_generation != lease.lease_generation()
        || checkpoint.checkpoint_kind != crate::orm_runs::native_checkpoints::OUTCOME_KIND
        || checkpoint.checkpoint_hash != evidence.checkpoint_hash
        || checkpoint.protected_state.is_none()
        || call.run_id != run.id
        || call.row_version != evidence.tool_row_version
        || call.lease_generation != lease.lease_generation()
        || !matches!(call.state.as_str(), "completed" | "execution_failed")
        || call.protected_result.is_none()
        || call.completed_at.is_none()
        || call.payload_purged_at.is_some()
        || call.approval_id != Some(approval.id)
        || step.run_id != run.id
        || step.lease_generation != lease.lease_generation()
        || step.state != call.state
        || step.finished_at.is_none()
        || approval.tool_call_id != call.id
        || approval.session_id != lease.session_id().0
        || approval.state != "consumed"
        || approval.maximum_uses != 1
        || approval.consumed_uses != 1
        || approval.consumed_at.is_none()
    {
        return Err(conflict());
    }
    // Query for any unsettled row directly: a default page cap must never turn a large
    // retained cohort into a falsely settled prefix.
    let unsettled_budget = tx
        .query::<AiBudgetReservationRecord>()
        .filter(AiBudgetReservationRecordWhereInput {
            run_id: Some(UuidFilter {
                eq: Some(run.id),
                ..Default::default()
            }),
            state: Some(StringFilter {
                not_in: Some(vec!["committed".into(), "released".into()]),
                ..Default::default()
            }),
            ..Default::default()
        })
        .limit(1)
        .fetch_one()
        .await
        .map_err(OrmPublicError::from)?;
    let unsettled_call = tx
        .query::<AiToolCallRecord>()
        .filter(AiToolCallRecordWhereInput {
            run_id: Some(UuidFilter {
                eq: Some(run.id),
                ..Default::default()
            }),
            state: Some(StringFilter {
                not_in: Some(vec!["completed".into(), "execution_failed".into()]),
                ..Default::default()
            }),
            ..Default::default()
        })
        .limit(1)
        .fetch_one()
        .await
        .map_err(OrmPublicError::from)?;
    if unsettled_budget.is_some() || unsettled_call.is_some() {
        return Err(conflict());
    }
    Ok(())
}

impl OrmAiProviderSessionService {
    async fn prepare_static_offering_reset_inner(
        &self,
        lease: &AiRunLease,
        claim: &AiProviderSessionClaim,
        request: &crate::AiProviderStaticOfferingResetRequest,
    ) -> Result<
        (
            AiProviderSessionCleanupClaim,
            AiProviderSessionDeletionRequest,
        ),
        AiError,
    > {
        let (current, session, scope) = self.load_owned_active_context(lease).await?;
        let policy = self.protection_policy(current.principal(), &scope).await?;
        let now = canonical_second(self.clock.now());
        let expiry = now
            .checked_add(self.limits.cleanup_lease_ttl())
            .map(|expiry| expiry.min(lease.lease_expires_at()))
            .filter(|expiry| *expiry > now)
            .ok_or(AiError::Conflict)?;
        let lease = lease.clone();
        let claim = claim.clone();
        let request = request.clone();
        let record = self
            .database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    validate_static_reset_graph(tx, &lease, &request, now).await?;
                    let record = tx
                        .find_by_id::<AiProviderSessionBindingRecord>(&claim.binding_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    validate_active_claim(&record, &claim, &lease, now)?;
                    if record.session_id != session.id
                        || record_scope_from_binding(&record) != scope
                        || descriptor_from_record(&record).map_err(ai_error_to_orm)?
                            != *request.planned.descriptor()
                        || record.transcript_fingerprint != request.planned.transcript_fingerprint()
                        || (!request.definitely_unstarted_budget_denial
                            && record.installed_offering_fingerprint.as_deref()
                                == Some(request.offering_fingerprint()))
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    let cleanup_generation = record
                        .cleanup_generation
                        .checked_add(1)
                        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                    let outcome = tx
                        .compare_and_swap::<AiProviderSessionBindingRecord>(
                            &record.id,
                            record.row_version,
                            AiProviderSessionBindingRecordWhereInput::default(),
                            UpdateAiProviderSessionBindingRecordInput {
                                state: Some(
                                    AiProviderSessionState::CleanupInProgress.as_str().into(),
                                ),
                                last_run_id: Some(Some(lease.run_id().0)),
                                claimed_run_id: Some(None),
                                claimed_attempt_id: Some(None),
                                claimed_run_lease_generation: Some(None),
                                claim_owner: Some(None),
                                claim_expires_at: Some(None),
                                cleanup_owner: Some(Some(lease.worker_id().to_owned())),
                                cleanup_generation: Some(cleanup_generation),
                                cleanup_lease_expires_at: Some(Some(expiry.unix_timestamp())),
                                cleanup_reason_code: Some(Some(
                                    if request.definitely_unstarted_budget_denial {
                                        "native_static_offering_budget_denied"
                                    } else {
                                        "native_static_offering_reset"
                                    }
                                    .into(),
                                )),
                                cleanup_next_attempt_at: Some(None),
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
                        "ai.provider_session.static_offering_cleanup",
                        updated.id,
                        "native_static_offering_reset",
                        lease.run_id().0,
                        now,
                    )
                    .await?;
                    Ok(updated)
                })
            })
            .await
            .map_err(map_transaction)?;
        let cleanup = cleanup_claim_from_record(&record)?;
        let deletion = self.open_for_cleanup(&cleanup, &policy).await?;
        Ok((cleanup, deletion))
    }

    async fn authorize_static_offering_rebind_inner(
        &self,
        lease: &AiRunLease,
        cleanup: &AiProviderSessionCleanupClaim,
        request: &crate::AiProviderStaticOfferingResetRequest,
    ) -> Result<AiProviderSessionRebindAuthorization, AiError> {
        let (current, session, scope) = self.load_owned_active_context(lease).await?;
        self.protection_policy(current.principal(), &scope).await?;
        let now = canonical_second(self.clock.now());
        let expires_at = now
            .checked_add(MAXIMUM_PROVIDER_SESSION_REBIND_AUTHORIZATION_TTL)
            .map(|expiry| expiry.min(lease.lease_expires_at()))
            .filter(|expiry| *expiry > now)
            .ok_or(AiError::Conflict)?;
        let lease = lease.clone();
        let cleanup = cleanup.clone();
        let request = request.clone();
        self.database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    validate_static_reset_graph(tx, &lease, &request, now).await?;
                    let record = tx
                        .find_by_id::<AiProviderSessionBindingRecord>(&cleanup.binding_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    validate_binding_record(&record)?;
                    if record.session_id != session.id
                        || record_scope_from_binding(&record) != scope
                        || record.state != AiProviderSessionState::Deleted.as_str()
                        || record.protected_cursor.is_some()
                        || record.last_run_id != Some(lease.run_id().0)
                        || record.cleanup_generation != cleanup.cleanup_generation
                        || record.cleanup_reason_code.as_deref()
                            != Some(if request.definitely_unstarted_budget_denial {
                                "native_static_offering_budget_denied"
                            } else {
                                "native_static_offering_reset"
                            })
                        || descriptor_from_record(&record).map_err(ai_error_to_orm)?
                            != *request.planned.descriptor()
                        || record.transcript_fingerprint != request.planned.transcript_fingerprint()
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    let provider_absence_observed_at = record
                        .provider_absence_observed_at
                        .and_then(|value| OffsetDateTime::from_unix_timestamp(value).ok())
                        .filter(|observed| {
                            *observed <= now && *observed >= now - Duration::hours(1)
                        })
                        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                    Ok(AiProviderSessionRebindAuthorization {
                        binding_id: record.id,
                        session_id: lease.session_id(),
                        run_id: lease.run_id(),
                        attempt_id: lease.attempt_id(),
                        run_lease_generation: lease.lease_generation(),
                        binding_row_version: record.row_version,
                        binding_claim_generation: record.claim_generation,
                        cleanup_generation: record.cleanup_generation,
                        provider_absence_observed_at,
                        expires_at,
                        principal_reference: lease.principal_reference().clone(),
                        scope,
                        descriptor: request.planned.descriptor().clone(),
                        transcript_fingerprint: request.planned.transcript_fingerprint().to_owned(),
                        static_offering_reset: Some(request),
                    })
                })
            })
            .await
            .map_err(map_transaction)
    }
    async fn authorize_static_offering_budget_rebind_inner(
        &self,
        lease: &AiRunLease,
        request: &crate::AiProviderStaticOfferingResetRequest,
    ) -> Result<(AiProviderSessionRebindAuthorization, String), AiError> {
        if !request.definitely_unstarted_budget_denial {
            return Err(AiError::Conflict);
        }
        let (_, session, _) = self.load_owned_active_context(lease).await?;
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
            .map_err(|_| AiError::PersistenceFailed)?;
        let [record] = records.as_slice() else {
            return Err(AiError::Conflict);
        };
        if record.state != AiProviderSessionState::Deleted.as_str()
            || record.cleanup_reason_code.as_deref() != Some("native_static_offering_budget_denied")
            || !crate::valid_sha256(&record.cursor_fingerprint)
        {
            return Err(AiError::Conflict);
        }
        // The tombstone retains the exact old cursor fingerprint and cleanup generation.
        // It has no opened bytes; confirmed absence and current original graph are checked
        // again transactionally by the dedicated authorization method below.
        let cleanup = AiProviderSessionCleanupClaim {
            binding_id: record.id,
            session_id: lease.session_id(),
            scope: record_scope_from_binding(record),
            descriptor: request.planned.descriptor().clone(),
            cleanup_worker_id: lease.worker_id().to_owned(),
            cleanup_generation: record.cleanup_generation,
            cleanup_expires_at: lease.lease_expires_at(),
            row_version: record.row_version,
        };
        let authorization = self
            .authorize_static_offering_rebind_inner(lease, &cleanup, request)
            .await?;
        Ok((authorization, record.cursor_fingerprint.clone()))
    }
}
