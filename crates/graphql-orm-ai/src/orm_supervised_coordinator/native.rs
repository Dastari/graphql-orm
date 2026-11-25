//! Explicit mixed native callback execution, separate from read-only coordination.
use super::*;
use crate::{
    AiApplicationToolDisposition, AiProviderDynamicToolExecution, AiProviderDynamicToolOutcome,
};

pub(super) struct NativeServices {
    pub(super) applications: Arc<OrmAiApplicationToolCallService>,
    pub(super) consequential: Arc<OrmAiConsequentialToolCallService>,
    pub(super) checkpoints: Arc<OrmAiCoordinatorCheckpointService>,
}

pub(super) struct NativeState {
    pub(super) usage: AiRuleRunUsage,
    pub(super) calls: u32,
    pub(super) pending: Option<crate::AiPreparedNativeApproval>,
    pub(super) failure_lease: Option<AiRunLease>,
}

pub(super) struct NativeExecution {
    pub(super) services: Arc<NativeServices>,
    pub(super) run_control: Arc<dyn AiAgentRunControl>,
    pub(super) rules: Arc<dyn AiAgentRuleResolver>,
    pub(super) scope: AiScope,
    pub(super) correlation: String,
    pub(super) route: AiToolResultEgressRoute,
    pub(super) fingerprint: String,
    pub(super) turn: u32,
    pub(super) maximum_calls: u32,
    pub(super) plan: AiProviderCallPlan,
    pub(super) capability_delivery: Option<crate::AiCapabilityDeliveryTurn>,
    pub(super) state: Mutex<NativeState>,
}

#[async_trait]
impl AiProviderDynamicToolExecution for NativeExecution {
    fn tool_result_source_service(&self) -> Option<&OrmAiApplicationToolCallService> {
        Some(self.services.applications.as_ref())
    }
    async fn execute_dynamic_tool(
        &self,
        _lease: &AiRunLease,
        _result: &crate::AiProviderCallResult,
        _index: usize,
    ) -> Result<crate::AiPersistedApplicationToolCall, AiError> {
        // No conversion of framework controls into domain results.
        Err(AiError::Forbidden)
    }

    async fn execute_dynamic_outcome(
        &self,
        lease: &AiRunLease,
        result: &crate::AiProviderCallResult,
        index: usize,
    ) -> Result<AiProviderDynamicToolOutcome, AiError> {
        self.state.lock().await.failure_lease = Some(lease.clone());
        if self.run_control.cancellation(lease).await?.is_some() {
            return Err(AiError::Conflict);
        }
        let resolution = self.rules.resolve_rules(lease, &self.scope).await?;
        let call = result.tool_calls().get(index).ok_or(AiError::Conflict)?;
        let (maturity, approval) = self
            .plan
            .classified_mutation_binding(call.tool_fingerprint())
            .ok_or(AiError::Forbidden)?;
        if resolution.rules().fingerprint() != self.fingerprint
            || resolution
                .rules()
                .constrain_tool(call.tool_fingerprint(), maturity, approval)
                != Some(approval)
        {
            return Err(AiError::Forbidden);
        }
        // Late classification of a broker execute that resolves to a
        // generated mutation. Boxed: an inline future would enlarge this
        // callback's stack frame in debug builds.
        let brokered = Box::pin(self.classify_brokered(lease, result, index, &resolution)).await?;
        let pending = {
            let mut state = self.state.lock().await;
            state.calls = state
                .calls
                .checked_add(1)
                .filter(|count| *count <= self.maximum_calls)
                .ok_or(AiError::BudgetDenied)?;
            state.usage = state.usage.accept_tool_calls(1, &resolution)?;
            state.pending.clone()
        };
        let context = AiApplicationToolCallContext::new(
            self.turn,
            index,
            self.scope.clone(),
            self.correlation.clone(),
            result.budget_reservation_id().0.to_string(),
        )?;
        let outcome = if let Some((resolved, disposition)) = brokered {
            Box::pin(self.brokered_outcome(lease, result, context, resolved, disposition, pending))
                .await?
        } else if crate::AiCapabilityBrokerOperation::from_tool_id(call.tool_id()).is_some() {
            let delivery = self
                .capability_delivery
                .as_ref()
                .ok_or(AiError::Forbidden)?;
            // Boxed like the other tool paths: in debug builds an inline
            // broker future enlarges this callback's stack frame.
            AiProviderDynamicToolOutcome::Application(Box::new(
                Box::pin(self.services.applications.execute_capability_broker_call(
                    lease,
                    result,
                    context,
                    self.route.clone(),
                    delivery,
                ))
                .await?,
            ))
        } else {
            let classification = Box::pin(self.services.applications.classify_native_tool_call(
                lease,
                result,
                &context,
                self.route.clone(),
            ))
            .await?;
            let disposition = match classification {
                crate::orm_tools::NativeToolClassification::Admitted(disposition) => disposition,
                crate::orm_tools::NativeToolClassification::Rejected(persisted) => {
                    self.state.lock().await.failure_lease = Some(persisted.lease().clone());
                    if self
                        .run_control
                        .cancellation(persisted.lease())
                        .await?
                        .is_some()
                    {
                        return Err(AiError::Conflict);
                    }
                    return Ok(AiProviderDynamicToolOutcome::Application(persisted));
                }
            };
            match (disposition, pending) {
                (AiApplicationToolDisposition::ReadOnly, _) => {
                    AiProviderDynamicToolOutcome::Application(Box::new(
                        Box::pin(self.services.applications.execute_read_only(
                            lease,
                            result,
                            context,
                            self.route.clone(),
                        ))
                        .await?,
                    ))
                }
                (_, Some(pending)) => AiProviderDynamicToolOutcome::NativeControl(Box::new(
                    self.services
                        .consequential
                        .pause_native_consequential_call(
                            lease,
                            result,
                            context,
                            &pending,
                            &self.route,
                        )
                        .await?,
                )),
                (AiApplicationToolDisposition::AutomaticMutation, None) => {
                    match Box::pin(self.services.applications.execute_automatic_mutation(
                        lease,
                        result,
                        context,
                        self.route.clone(),
                    ))
                    .await?
                    {
                        crate::AiConsequentialToolCallOutcome::Persisted(result) => {
                            AiProviderDynamicToolOutcome::Application(result)
                        }
                        crate::AiConsequentialToolCallOutcome::RecoveryRequired { .. } => {
                            return Err(AiError::Conflict);
                        }
                    }
                }
                (AiApplicationToolDisposition::ApprovalRequired, None) => {
                    let prepared = self
                        .services
                        .consequential
                        .prepare_native_approval(lease, result, context)
                        .await?;
                    // Retain the candidate before egress: any later failure abandons it under the active run fence.
                    {
                        let mut state = self.state.lock().await;
                        state.pending = Some(prepared.clone());
                        state.failure_lease = Some(prepared.lease().clone());
                    }
                    let receipt = self
                        .services
                        .consequential
                        .prepare_native_approval_receipt(prepared.lease(), &prepared, &self.route)
                        .await?;
                    AiProviderDynamicToolOutcome::NativeControl(Box::new(receipt))
                }
            }
        };
        let current_lease = match &outcome {
            AiProviderDynamicToolOutcome::Application(result) => result.lease(),
            AiProviderDynamicToolOutcome::NativeControl(receipt) => receipt.lease(),
        };
        // Persistence can rotate the fence while cancellation arrives. Preserve
        // that fence for cleanup, and withhold the callback after cancellation.
        self.state.lock().await.failure_lease = Some(current_lease.clone());
        if self
            .run_control
            .cancellation(current_lease)
            .await?
            .is_some()
        {
            return Err(AiError::Conflict);
        }
        Ok(outcome)
    }

    async fn failed_callback_lease(&self) -> Option<AiRunLease> {
        self.state.lock().await.failure_lease.clone()
    }

    async fn persist_dynamic_failure(
        &self,
        lease: &AiRunLease,
        result: &crate::AiProviderCallResult,
        index: usize,
        code: crate::AiApplicationToolFailureCode,
    ) -> Result<crate::AiPersistedApplicationToolCall, AiError> {
        if self.run_control.cancellation(lease).await?.is_some() {
            return Err(AiError::Conflict);
        }
        let resolution = self.rules.resolve_rules(lease, &self.scope).await?;
        let call = result.tool_calls().get(index).ok_or(AiError::Conflict)?;
        if resolution.rules().fingerprint() != self.fingerprint
            || self
                .plan
                .classified_mutation_binding(call.tool_fingerprint())
                != Some((crate::ToolMaturity::ReadOnly, crate::AiApprovalRule::None))
            || resolution.rules().constrain_tool(
                call.tool_fingerprint(),
                crate::ToolMaturity::ReadOnly,
                crate::AiApprovalRule::None,
            ) != Some(crate::AiApprovalRule::None)
            // A broker execute addressing a loaded mutation is consequential,
            // like a direct mutation: it never becomes a read-failure receipt.
            || (crate::AiCapabilityBrokerOperation::from_tool_id(call.tool_id())
                == Some(crate::AiCapabilityBrokerOperation::Execute)
                && self
                    .capability_delivery
                    .as_ref()
                    .is_some_and(|delivery| delivery.session().holds_loaded_mutation(call.arguments())))
        {
            return Err(AiError::Forbidden);
        }
        let index = u32::try_from(index).map_err(|_| AiError::Conflict)?;
        {
            let mut state = self.state.lock().await;
            if state.calls == index {
                state.calls = state
                    .calls
                    .checked_add(1)
                    .filter(|count| *count <= self.maximum_calls)
                    .ok_or(AiError::BudgetDenied)?;
                state.usage = state.usage.accept_tool_calls(1, &resolution)?;
            } else if state.calls != index.saturating_add(1) {
                return Err(AiError::Conflict);
            }
        }
        let index = usize::try_from(index).map_err(|_| AiError::Conflict)?;
        let context = AiApplicationToolCallContext::new(
            self.turn,
            index,
            self.scope.clone(),
            self.correlation.clone(),
            result.budget_reservation_id().0.to_string(),
        )?;
        // This service independently rejects consequential calls; uncertain effects never become retry receipts.
        let persisted = self
            .services
            .applications
            .persist_safe_read_failure(lease, result, context, self.route.clone(), code)
            .await?;
        self.state.lock().await.failure_lease = Some(persisted.lease().clone());
        if self
            .run_control
            .cancellation(persisted.lease())
            .await?
            .is_some()
        {
            return Err(AiError::Conflict);
        }
        Ok(persisted)
    }
}

/// A broker execute call resolved to a generated mutation, with the
/// disposition its reviewed classification selects.
type BrokeredCall = (
    crate::orm_tools::ResolvedApplicationCall,
    AiApplicationToolDisposition,
);

impl NativeExecution {
    /// Classifies a broker execute call that resolves to a generated mutation
    /// as that mutation, never as the broker definition, so admitting the
    /// broker fingerprints never admits a mutation. A call that does not
    /// resolve, or fails to, returns `None` and takes the ordinary broker
    /// path, which executes nothing consequential and answers with its own
    /// durable outcome.
    async fn classify_brokered(
        &self,
        lease: &AiRunLease,
        result: &crate::AiProviderCallResult,
        index: usize,
        resolution: &crate::AiAgentRuleResolution,
    ) -> Result<Option<BrokeredCall>, AiError> {
        let Some(delivery) = &self.capability_delivery else {
            return Ok(None);
        };
        let Ok(Some(resolved)) = Box::pin(
            self.services
                .applications
                .resolve_brokered_mutation(lease, result, index, delivery),
        )
        .await
        else {
            return Ok(None);
        };
        let (maturity, approval) = resolved
            .mutation_binding(self.services.applications.runtime().tool_catalog())
            .ok_or(AiError::Forbidden)?;
        if resolution
            .rules()
            .constrain_tool(&resolved.tool_fingerprint, maturity, approval)
            != Some(approval)
        {
            return Err(AiError::Forbidden);
        }
        let disposition = if approval == crate::AiApprovalRule::None {
            AiApplicationToolDisposition::AutomaticMutation
        } else {
            AiApplicationToolDisposition::ApprovalRequired
        };
        Ok(Some((resolved, disposition)))
    }

    /// Routes one broker execute call resolved to a generated mutation
    /// exactly as the direct classified definition is routed: paused while
    /// an approval is pending, executed once when automatic, or prepared for
    /// approval. The reply answers the provider's broker execute call.
    async fn brokered_outcome(
        &self,
        lease: &AiRunLease,
        result: &crate::AiProviderCallResult,
        context: AiApplicationToolCallContext,
        resolved: crate::orm_tools::ResolvedApplicationCall,
        disposition: AiApplicationToolDisposition,
        pending: Option<crate::AiPreparedNativeApproval>,
    ) -> Result<AiProviderDynamicToolOutcome, AiError> {
        Ok(match (disposition, pending) {
            // Each effect future is boxed: built inline, it would enlarge
            // this frame in debug builds.
            (_, Some(pending)) => AiProviderDynamicToolOutcome::NativeControl(Box::new(
                Box::pin(self.services.consequential.pause_brokered_native_call(
                    lease,
                    result,
                    context,
                    &pending,
                    &self.route,
                    resolved,
                ))
                .await?,
            )),
            (AiApplicationToolDisposition::AutomaticMutation, None) => {
                match Box::pin(
                    self.services
                        .applications
                        .execute_brokered_automatic_mutation(
                            lease,
                            result,
                            context,
                            self.route.clone(),
                            resolved,
                        ),
                )
                .await?
                {
                    crate::AiConsequentialToolCallOutcome::Persisted(result) => {
                        AiProviderDynamicToolOutcome::Application(result)
                    }
                    crate::AiConsequentialToolCallOutcome::RecoveryRequired { .. } => {
                        return Err(AiError::Conflict);
                    }
                }
            }
            (AiApplicationToolDisposition::ApprovalRequired, None) => {
                let prepared = Box::pin(
                    self.services
                        .consequential
                        .prepare_brokered_native_approval(lease, result, context, resolved),
                )
                .await?;
                {
                    let mut state = self.state.lock().await;
                    state.pending = Some(prepared.clone());
                    state.failure_lease = Some(prepared.lease().clone());
                }
                let receipt =
                    Box::pin(self.services.consequential.prepare_native_approval_receipt(
                        prepared.lease(),
                        &prepared,
                        &self.route,
                    ))
                    .await?;
                AiProviderDynamicToolOutcome::NativeControl(Box::new(receipt))
            }
            (AiApplicationToolDisposition::ReadOnly, None) => return Err(AiError::Forbidden),
        })
    }
}
