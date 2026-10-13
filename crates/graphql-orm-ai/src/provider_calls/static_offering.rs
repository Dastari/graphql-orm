//! Verified cleanup and fresh-thread installation of an original static subset.
use super::*;

impl AiProviderCallPlan {
    pub(super) fn offering_fingerprint(&self) -> String {
        crate::tools::canonical_json_digest(&serde_json::json!({
            "formatVersion":1,"kind":"installed_provider_definitions","tools":self.request.tools
        }))
    }
}

impl AiProviderCallExecutor {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn reset_static_offering(
        &self,
        lease: &AiRunLease,
        plan: &mut AiProviderCallPlan,
        planned: &crate::AiProviderSessionTurnPlan,
        sessions: &Arc<dyn crate::AiProviderSessionService>,
        claim: Option<&crate::AiProviderSessionClaim>,
    ) -> Result<
        (
            crate::AiProviderSessionClaim,
            crate::AiProviderSessionCursor,
        ),
        AiError,
    > {
        let evidence = plan
            .native_reset_evidence
            .clone()
            .ok_or(AiError::Conflict)?;
        let context = plan.task_context.as_ref().ok_or(AiError::Conflict)?;
        let sources = self
            .tool_result_source_service
            .as_ref()
            .ok_or(AiError::RuntimeNotReady)?;
        let deletion = self
            .static_offering_deletion
            .as_ref()
            .ok_or(AiError::RuntimeNotReady)?;
        sources
            .validate_task_continuation_context(lease, &plan.budget.scope, context)
            .await?;
        let original_outcome_hash = if plan.task_context_only {
            plan.replan_continuation
                .as_ref()
                .and_then(|original| original.input().first())
                .and_then(|block| match block {
                    ModelInputBlock::Json { value } => {
                        crate::provider::native_approved_outcome_hash(value)
                    }
                    _ => None,
                })
        } else {
            plan.request.native_approved_outcome_hash()
        };
        if original_outcome_hash.as_deref() != Some(evidence.outcome_hash.as_str())
            || evidence.session_id != lease.session_id()
            || evidence.run_id != lease.run_id()
            || evidence.attempt_id != lease.attempt_id()
            || evidence.lease_generation != lease.lease_generation()
            || plan.static_offering_reset_receipt.is_some()
        {
            return Err(AiError::Conflict);
        }
        let original_continuation_reference = if plan.task_context_only {
            plan.replan_continuation
                .as_ref()
                .ok_or(AiError::Conflict)?
                .chain_reference()?
        } else {
            plan.request
                .continuation
                .as_ref()
                .and_then(|continuation| continuation.chain_reference(&plan.request.input))
                .ok_or(AiError::Conflict)?
        };
        // Price the exact fresh projection before cleanup or empty-thread creation. The
        // executor may refuse a host quote; it never increases a monetary reservation.
        let mut fresh_request = plan.request.clone();
        fresh_request.continuation = None;
        let inference_bytes = fresh_request
            .conservative_provider_egress_bytes(&plan.provider_kind)
            .map_err(|_| AiError::EgressDenied)?;
        let inference_tokens = fresh_request
            .conservative_provider_egress_tokens(&plan.provider_kind)
            .map_err(|_| AiError::EgressDenied)?;
        if plan.budget.estimate.input_tokens < inference_tokens {
            return Err(AiError::PreTransportBudgetDenied);
        }
        let desired_offering_fingerprint = plan.offering_fingerprint();
        let request = crate::AiProviderStaticOfferingResetRequest {
            evidence: evidence.clone(),
            planned: planned.clone(),
            desired_offering_fingerprint: desired_offering_fingerprint.clone(),
            definitely_unstarted_budget_denial: claim.is_none(),
        };
        let binding = crate::AiProviderRunBinding::from_lease(lease)?;
        let operation = async {
            let (authorization, original_cursor_fingerprint, cleanup_generation) =
                if let Some(claim) = claim {
                    let (cleanup, deletion_request) = sessions
                        .prepare_static_offering_reset(lease, claim, &request)
                        .await?;
                    let original_cursor_fingerprint = deletion_request.cursor().fingerprint();
                    self.runtime
                        .close_all_provider_runs(
                            &binding,
                            crate::AiProviderRunCloseReason::Completed,
                        )
                        .await
                        .map_err(|_| AiError::ProviderFailed)?;
                    let absence = deletion
                        .delete_or_confirm_absent(&deletion_request)
                        .await
                        .map_err(|_| AiError::ProviderFailed)?;
                    sessions.complete_cleanup(&cleanup, absence).await?;
                    let authorization = sessions
                        .authorize_static_offering_rebind(lease, &cleanup, &request)
                        .await?;
                    (
                        authorization,
                        original_cursor_fingerprint,
                        cleanup.cleanup_generation(),
                    )
                } else {
                    // Only the exact SDK-marked unstarted denial tombstone can reach this route.
                    // Its actual absence and original completed graph are freshly revalidated.
                    let (authorization, fingerprint) = sessions
                        .authorize_static_offering_budget_rebind(lease, &request)
                        .await?;
                    let generation = authorization.cleanup_generation;
                    (authorization, fingerprint, generation)
                };
            sources
                .validate_task_continuation_context(lease, &plan.budget.scope, context)
                .await?;
            if let Some(offering) = &plan.capability_offering {
                offering.validate()?;
            }
            let cursor = self
                .runtime
                .create_empty_provider_session(
                    planned.descriptor().provider_kind(),
                    &binding,
                    planned.descriptor(),
                    &fresh_request,
                )
                .await
                .map_err(|_| AiError::ProviderFailed)?;
            if cursor.fingerprint() == original_cursor_fingerprint {
                let _ = self
                    .runtime
                    .discard_empty_provider_session(
                        planned.descriptor().provider_kind(),
                        &binding,
                        planned.descriptor(),
                        &cursor,
                    )
                    .await;
                return Err(AiError::Conflict);
            }
            let bind = crate::AiProviderSessionBindRequest::new(
                planned.descriptor().clone(),
                cursor.clone(),
                planned.transcript_fingerprint(),
                None,
            )?
            .with_installed_offering(desired_offering_fingerprint.clone())?;
            let fresh_claim = match sessions.rebind_for_run(lease, authorization, bind).await {
                Ok(claim) => claim,
                Err(error) => {
                    let _ = self
                        .runtime
                        .discard_empty_provider_session(
                            planned.descriptor().provider_kind(),
                            &binding,
                            planned.descriptor(),
                            &cursor,
                        )
                        .await;
                    return Err(error);
                }
            };
            Ok((
                fresh_claim,
                cursor,
                cleanup_generation,
                original_cursor_fingerprint,
                fresh_request,
            ))
        };
        let remaining = lease.lease_expires_at() - self.clock.now();
        let seconds =
            u64::try_from(remaining.whole_seconds().clamp(1, 30)).map_err(|_| AiError::Conflict)?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(seconds);
        let (claim, cursor, cleanup_generation, original_cursor_fingerprint, fresh_request) =
            tokio::time::timeout_at(deadline, operation)
                .await
                .map_err(|_| AiError::ProviderFailed)??;
        // A non-yielding owning callback cannot outlive the same finite whole reset deadline.
        if tokio::time::Instant::now() >= deadline {
            return Err(AiError::ProviderFailed);
        }
        plan.request = fresh_request;
        let inference = plan
            .transfers
            .iter_mut()
            .find(|manifest| manifest.capability == AiEgressCapability::ModelInference)
            .ok_or(AiError::EgressDenied)?;
        inference.estimated_bytes = inference_bytes;
        inference.estimated_tokens = inference_tokens;
        plan.static_offering_reset_receipt = Some(Arc::new(
            crate::provider_session::AiStaticOfferingResetReceipt {
                session_id: lease.session_id(),
                run_id: lease.run_id(),
                attempt_id: lease.attempt_id(),
                lease_generation: lease.lease_generation(),
                original_continuation_reference,
                original_cursor_fingerprint,
                replacement_cursor_fingerprint: cursor.fingerprint(),
                replacement_binding_id: claim.binding_id(),
                task_context_only: plan.task_context_only,
                cleanup_generation,
                offering_fingerprint: desired_offering_fingerprint,
                outcome_hash: evidence.outcome_hash.clone(),
            },
        ));
        Ok((claim, cursor))
    }

    pub(super) async fn execute_withheld_native_task_context(
        &self,
        lease_state: Arc<Mutex<AiRunLease>>,
        plan: AiProviderCallPlan,
        planned: &crate::AiProviderSessionTurnPlan,
        sessions: &Arc<dyn crate::AiProviderSessionService>,
        claim: &crate::AiProviderSessionClaim,
    ) -> Result<AiProviderCallResult, AiError> {
        let current = lease_state.lock().await;
        let mut safe = self.withheld_tool_history_plan(&current, plan).await?;
        // A preceding proved empty-thread reset may already carry a bridge. This new
        // definitely-unstarted source withholding deletes that actual cursor and mints a
        // replacement bridge to the SAME original continuation, never a relabeled response.
        safe.static_offering_reset_receipt = None;
        if !safe.task_context_only
            || !safe.request.tools.is_empty()
            || safe.budget.estimate.image_units != 0
        {
            return Err(AiError::Conflict);
        }
        let (fresh_claim, cursor) = self
            .reset_static_offering(&current, &mut safe, planned, sessions, Some(claim))
            .await?;
        let binding = crate::AiProviderRunBinding::from_lease(&current)?;
        let opened = sessions
            .open_for_run(&current, &fresh_claim)
            .await?
            .activate_newly_bound_empty(binding, &cursor)?;
        drop(current);
        let shared_claim = Arc::new(Mutex::new(fresh_claim.clone()));
        let outcome = Box::pin(self.execute_inner(
            lease_state,
            safe,
            None,
            Some(opened),
            Some(shared_claim),
            0,
        ))
        .await;
        match outcome {
            Ok(mut result) => {
                result.provider_session_claim = Some(fresh_claim);
                Ok(result)
            }
            Err(error) => {
                let _ = sessions
                    .require_cleanup(&fresh_claim, "native_withheld_task_context_failed")
                    .await;
                // This quote already has zero images; it cannot mint another image reduction.
                Err(if matches!(error, AiError::PreTransportBudgetDenied) {
                    AiError::PreTransportProviderFailed
                } else {
                    error
                })
            }
        }
    }

    pub(super) async fn confirm_unstarted_static_offering_cleanup(
        &self,
        lease: &AiRunLease,
        plan: &AiProviderCallPlan,
        planned: &crate::AiProviderSessionTurnPlan,
        sessions: &Arc<dyn crate::AiProviderSessionService>,
        claim: &crate::AiProviderSessionClaim,
    ) -> Result<(), AiError> {
        let evidence = plan
            .native_reset_evidence
            .clone()
            .ok_or(AiError::Conflict)?;
        let context = plan.task_context.as_ref().ok_or(AiError::Conflict)?;
        let sources = self
            .tool_result_source_service
            .as_ref()
            .ok_or(AiError::RuntimeNotReady)?;
        let deletion = self
            .static_offering_deletion
            .as_ref()
            .ok_or(AiError::RuntimeNotReady)?;
        let request = crate::AiProviderStaticOfferingResetRequest {
            evidence,
            planned: planned.clone(),
            desired_offering_fingerprint: plan.offering_fingerprint(),
            definitely_unstarted_budget_denial: true,
        };
        let binding = crate::AiProviderRunBinding::from_lease(lease)?;
        let operation = async {
            sources
                .validate_task_continuation_context(lease, &plan.budget.scope, context)
                .await?;
            let (cleanup, deletion_request) = sessions
                .prepare_static_offering_reset(lease, claim, &request)
                .await?;
            self.runtime
                .close_all_provider_runs(&binding, crate::AiProviderRunCloseReason::Completed)
                .await
                .map_err(|_| AiError::ProviderFailed)?;
            let absence = deletion
                .delete_or_confirm_absent(&deletion_request)
                .await
                .map_err(|_| AiError::ProviderFailed)?;
            sessions.complete_cleanup(&cleanup, absence).await?;
            Ok::<(), AiError>(())
        };
        let seconds = u64::try_from(
            (lease.lease_expires_at() - self.clock.now())
                .whole_seconds()
                .clamp(1, 30),
        )
        .map_err(|_| AiError::Conflict)?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(seconds);
        tokio::time::timeout_at(deadline, operation)
            .await
            .map_err(|_| AiError::ProviderFailed)??;
        if tokio::time::Instant::now() >= deadline {
            return Err(AiError::ProviderFailed);
        }
        Ok(())
    }
}

/// SDK-minted opportunity to reduce one definitely unstarted native continuation.
///
/// The value has no public constructor or deserializer. It retains the exact original
/// continuation and prepared task context, plus the same shared capability delivery state.
/// The host may reduce only originally offered static definitions and image capacity;
/// provider/model/selection, scope, source, consent, rules and effect evidence stay fixed.
/// At most one callback is admitted after an atomic reservation denial before transport.
#[derive(Clone)]
pub struct AiUnstartedStaticOfferingReplan {
    failed_plan: AiProviderCallPlan,
    delivery: crate::AiCapabilityDeliveryTurn,
    session: crate::AiProviderSessionTurnPlan,
}

impl std::fmt::Debug for AiUnstartedStaticOfferingReplan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AiUnstartedStaticOfferingReplan(<original fenced continuation>)")
    }
}

impl AiUnstartedStaticOfferingReplan {
    /// Reopens the same SDK-owned original continuation and prepared pricing context.
    pub fn continuation(&self) -> crate::AiAgentContinuation {
        self.failed_plan
            .replan_continuation
            .as_ref()
            .expect("private mint requires continuation")
            .as_ref()
            .clone()
    }

    /// Same shared broker/static offering state. All failed-plan freezes must be released.
    pub fn capability_delivery(&self) -> &crate::AiCapabilityDeliveryTurn {
        &self.delivery
    }

    /// Original conservative typed quote. Retain every non-image field exactly; only image
    /// capacity may decrease and the content-bound unstarted idempotency key must change.
    /// This read-only request is pricing evidence, never a reservation or disclosure permit.
    pub fn original_budget_request(&self) -> &AiBudgetReservationRequest {
        &self.failed_plan.budget
    }

    /// Failed finite image quote; the replacement quote must be strictly smaller.
    pub fn image_unit_quote(&self) -> u64 {
        self.failed_plan.budget.estimate.image_units
    }

    pub(crate) fn prepare(
        plan: &AiProviderCallPlan,
        delivery: &crate::AiCapabilityDeliveryTurn,
        session: &crate::AiProviderSessionTurnPlan,
    ) -> Option<Self> {
        if plan.native_reset_evidence.is_none()
            || plan.task_context.is_none()
            || plan.replan_continuation.is_none()
            || plan.capability_offering.is_none()
            || plan.budget.estimate.image_units == 0
            || !delivery.matches_offered_tools(plan.offered_tools())
            || !plan.matches_provider_session_descriptor(session.descriptor())
        {
            return None;
        }
        let mut failed_plan = plan.clone();
        // Retain evidence, never a frozen plan. The coordinator additionally refuses the
        // callback while any external or in-flight clone still owns the shared freeze.
        failed_plan.capability_offering = None;
        Some(Self {
            failed_plan,
            delivery: delivery.clone(),
            session: session.clone(),
        })
    }

    pub(crate) fn ready_for_callback(&self) -> Result<(), AiError> {
        self.delivery.require_unfrozen_static_offering()
    }

    pub(crate) async fn validate_unstarted_cleanup(
        &self,
        lease: &AiRunLease,
        sessions: &dyn crate::AiProviderSessionService,
        now: time::OffsetDateTime,
    ) -> Result<(), AiError> {
        self.ready_for_callback()?;
        self.validate_current_unstarted_cleanup(lease, sessions, now)
            .await
    }

    // After a successful host callback the replacement owns its legitimate new
    // freeze. Recheck the same durable custody, without reapplying the pre-callback
    // requirement that all failed-plan freezes have already been released.
    pub(crate) async fn validate_current_unstarted_cleanup(
        &self,
        lease: &AiRunLease,
        sessions: &dyn crate::AiProviderSessionService,
        now: time::OffsetDateTime,
    ) -> Result<(), AiError> {
        let request = crate::AiProviderStaticOfferingResetRequest {
            evidence: self
                .failed_plan
                .native_reset_evidence
                .clone()
                .ok_or(AiError::Conflict)?,
            planned: self.session.clone(),
            desired_offering_fingerprint: self.failed_plan.offering_fingerprint(),
            definitely_unstarted_budget_denial: true,
        };
        // The error category alone proves neither a deletion nor its cause. Read the exact
        // SDK-marked Deleted tombstone through the dedicated owning authorization path.
        // This performs no rebind or cursor creation and grants the host no deletion proof.
        let seconds = u64::try_from(
            (lease.lease_expires_at() - now)
                .whole_seconds()
                .clamp(1, 30),
        )
        .map_err(|_| AiError::Conflict)?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(seconds);
        tokio::time::timeout_at(
            deadline,
            sessions.authorize_static_offering_budget_rebind(lease, &request),
        )
        .await
        .map_err(|_| AiError::ProviderFailed)??;
        if tokio::time::Instant::now() >= deadline {
            return Err(AiError::ProviderFailed);
        }
        Ok(())
    }

    pub(crate) fn validate_replacement(
        &self,
        next: &AiProviderCallPlan,
        delivery: &crate::AiCapabilityDeliveryTurn,
        session: &crate::AiProviderSessionTurnPlan,
    ) -> Result<(), AiError> {
        let old = &self.failed_plan;
        let mut original = old.request.clone();
        original.tools.clear();
        let mut replacement = next.request.clone();
        replacement.tools.clear();
        let old_tools = old
            .request
            .tools
            .iter()
            .map(|tool| tool.fingerprint.as_str())
            .collect::<BTreeSet<_>>();
        let new_tools = next
            .request
            .tools
            .iter()
            .map(|tool| tool.fingerprint.as_str())
            .collect::<BTreeSet<_>>();
        let mut next_budget = next.budget.clone();
        next_budget.estimate.image_units = old.budget.estimate.image_units;
        next_budget.idempotency_key = old.budget.idempotency_key.clone();
        let same_transfers = old.transfers.len() == next.transfers.len()
            && old
                .transfers
                .iter()
                .zip(&next.transfers)
                .all(|(old, next)| {
                    if next.estimated_bytes > old.estimated_bytes
                        || next.estimated_tokens > old.estimated_tokens
                    {
                        return false;
                    }
                    let mut normalized = next.clone();
                    normalized.estimated_bytes = old.estimated_bytes;
                    normalized.estimated_tokens = old.estimated_tokens;
                    &normalized == old
                });
        let same_context = old
            .task_context
            .as_ref()
            .zip(next.task_context.as_ref())
            .is_some_and(|(old, next)| Arc::ptr_eq(old, next));
        let same_evidence = old
            .native_reset_evidence
            .as_ref()
            .zip(next.native_reset_evidence.as_ref())
            .is_some_and(|(old, next)| Arc::ptr_eq(old, next));
        if old.provider_kind != next.provider_kind
            || original != replacement
            || old.correlation_id != next.correlation_id
            || !new_tools.is_subset(&old_tools)
            || new_tools == old_tools
            || !self.delivery.shares_run_state(delivery)
            || !delivery.matches_offered_tools(next.offered_tools())
            || session.descriptor() != self.session.descriptor()
            || session.transcript_fingerprint() != self.session.transcript_fingerprint()
            || old.budget.idempotency_key == next.budget.idempotency_key
            || next.budget.estimate.image_units >= old.budget.estimate.image_units
            || next_budget != old.budget
            || !same_transfers
            || !same_context
            || !same_evidence
        {
            return Err(AiError::Conflict);
        }
        Ok(())
    }
}
