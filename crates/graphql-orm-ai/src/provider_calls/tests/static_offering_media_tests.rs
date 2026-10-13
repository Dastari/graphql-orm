//! Actual approval, SQLite budget denial and same-run native subset retry fixtures.
use super::*;
use crate::{
    AiSupervisedAgentCoordinator, AiSupervisedAgentCoordinatorLimits, AiSupervisedAgentRunOutcome,
    AiSupervisedAgentTurnPlan, AiSupervisedAgentTurnPlanner, OrmAiProviderOutputService,
};

struct QuotaRetryPlanner {
    fixture: Arc<Fixture>,
    application: Arc<OrmAiApplicationToolCallService>,
    delivery: crate::AiCapabilityDeliveryTurn,
    mode: usize,
    retries: AtomicUsize,
    held_plan: Mutex<Option<AiProviderCallPlan>>,
}

impl QuotaRetryPlanner {
    async fn build(
        &self,
        lease: &AiRunLease,
        continuation: crate::AiAgentContinuation,
        exact_budget: Option<AiBudgetReservationRequest>,
    ) -> Result<AiSupervisedAgentTurnPlan, AiError> {
        let descriptor = self
            .fixture
            .runtime
            .tool_catalog()
            .descriptor(&AiToolId::parse("records.read")?)
            .unwrap();
        let mut policy = AiToolPolicySet::new(ToolMaturity::SupervisedWrite);
        policy.bind(AiToolPolicyBinding {
            tool_id: descriptor.id.clone(),
            fingerprint: descriptor.fingerprint.clone(),
            enabled: true,
        });
        let mut base = plan(&self.fixture);
        base.request.input.clear();
        base.request.tools = self.delivery.current_tools();
        base.budget.attempt_id = lease.attempt_id();
        base.budget.lease_generation = lease.lease_generation();
        base.budget.idempotency_key = "owning-quota-original".into();
        let input_contribution = continuation.conservative_input_tokens()?;
        let base_projection = base
            .request
            .conservative_provider_egress_tokens(&base.provider_kind)
            .map_err(|_| AiError::Conflict)?;
        let mut complete_projection = base.request.clone();
        // Price the SDK's exact opaque input/context, including its actual reference
        // and framing. No guessed allowance substitutes for the complete request.
        continuation
            .clone()
            .apply_with_bound_task_context(&mut complete_projection)?;
        let complete_quote = complete_projection
            .conservative_provider_egress_tokens(&base.provider_kind)
            .map_err(|_| AiError::Conflict)?;
        assert!(
            complete_quote
                >= base_projection
                    .checked_add(input_contribution)
                    .ok_or(AiError::Conflict)?
        );
        base.budget.estimate.input_tokens = complete_quote;
        base.budget.estimate.image_units = 4;
        if let Some(mut budget) = exact_budget {
            budget.estimate.image_units = 0;
            budget.idempotency_key = "owning-quota-reduced".into();
            base.budget = budget;
        }
        let provider = AiProviderCallPlan::new_continuation_with_classified_capability_surface(
            base.provider_kind,
            base.request,
            base.budget,
            base.transfers,
            "approved-media-test",
            continuation,
            &self.delivery.current_surface(),
            self.fixture.runtime.tool_catalog(),
            &policy,
            &self.fixture.generated_target_policy,
        )?;
        assert!(
            provider.budget.estimate.input_tokens
                >= provider
                    .request
                    .conservative_provider_egress_tokens(&provider.provider_kind)
                    .map_err(|_| AiError::Conflict)?
        );
        if self.mode == 9 {
            *self.held_plan.lock().await = Some(provider.clone());
        }
        let selected = AiProviderSessionDescriptor::new(
            ProviderKind::OpenAiCompatible,
            "mock-profile",
            "mock-model",
            self.delivery.session_binding().fingerprint(),
            "mock/v1",
            "b".repeat(64),
        )?;
        AiSupervisedAgentTurnPlan::new_classified_native(
            provider,
            crate::AiProviderSessionTurnPlan::new(selected, "d".repeat(64))?,
            preview_route(),
            native_test_rules(self.fixture.scope.clone()),
            false,
        )?
        .with_capability_delivery(self.delivery.clone())
    }
}

#[async_trait]
impl AiSupervisedAgentTurnPlanner for QuotaRetryPlanner {
    async fn initial_plan(&self, _: &AiRunLease) -> Result<AiSupervisedAgentTurnPlan, AiError> {
        Err(AiError::Conflict)
    }
    async fn continuation_plan(
        &self,
        lease: &AiRunLease,
        _: u32,
        continuation: crate::AiAgentContinuation,
    ) -> Result<AiSupervisedAgentTurnPlan, AiError> {
        let prepared = self
            .application
            .prepare_task_continuation_context(lease, &self.fixture.scope)
            .await?;
        self.build(
            lease,
            continuation.with_prepared_task_context(prepared)?,
            None,
        )
        .await
    }
    async fn replan_unstarted_static_offering(
        &self,
        lease: &AiRunLease,
        _: u32,
        context: crate::AiUnstartedStaticOfferingReplan,
    ) -> Result<Option<AiSupervisedAgentTurnPlan>, AiError> {
        assert_eq!(self.retries.fetch_add(1, Ordering::SeqCst), 0);
        assert_eq!(context.image_unit_quote(), 4);
        assert!(
            context
                .capability_delivery()
                .shares_run_state(&self.delivery)
        );
        // The owning SDK must have dropped EVERY plan freeze before this callback.
        context
            .capability_delivery()
            .set_static_bootstrap_offering(&BTreeSet::new())?;
        if matches!(self.mode, 11 | 13) {
            crate::OrmAiRunCancellationService::new(
                self.fixture.database.clone(),
                Arc::new(AllowAccess),
                Arc::new(ProtectionPolicy),
                Arc::new(DatabaseManagedContentProtector),
                Arc::new(Resolver(self.fixture.principal.clone())),
                Arc::new(SystemClock),
                crate::AiRunCancellationLimits::default(),
                Arc::new(crate::AiRunCancellationHub::default()),
            )
            .request_cancellation(
                &self.fixture.principal,
                crate::CancelAiRunInput {
                    session_id: lease.session_id().0,
                    run_id: lease.run_id().0,
                    client_request_id: Uuid::new_v4(),
                },
            )
            .await?;
        }
        if matches!(self.mode, 12 | 13) {
            // Actual durable cancellation must interrupt a host that never returns;
            // without cancellation the same finite lease/reset deadline must end it.
            return std::future::pending().await;
        }
        self.build(
            lease,
            context.continuation(),
            Some(context.original_budget_request().clone()),
        )
        .await
        .map(Some)
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn owning_coordinator_reset(
    fixture: Fixture,
    custody: Arc<Custody>,
    application: Arc<OrmAiApplicationToolCallService>,
    sessions: Arc<OrmAiProviderSessionService>,
    checkpoints: Arc<OrmAiCoordinatorCheckpointService>,
    consequential: Arc<OrmAiConsequentialToolCallService>,
    resume: Arc<OrmAiSupervisedResumeService>,
    delivery: crate::AiCapabilityDeliveryTurn,
    approved: crate::AiApprovedRunClaim,
    mode: usize,
) {
    let mut fixture = fixture;
    // This extended original-task continuation is larger than the parent's 10,000
    // token test profile. Give only this fixture finite capacity for its complete
    // SDK-priced request; the separate zero image policy still forces the retry.
    fixture.budget_service = Arc::new(OrmAiBudgetService::new(
        fixture.database.clone(),
        Arc::new(SystemClock),
        crate::AiBudgetServiceLimits::new(
            AiBudgetAmounts {
                input_tokens: 20_000,
                output_tokens: 10_000,
                tool_units: 100,
                image_units: 100,
                cost_microunits: 1_000_000,
                runs: 1,
            },
            Duration::minutes(5),
            Duration::seconds(30),
            16,
            8,
        )
        .unwrap(),
    ));
    let fixture = Arc::new(fixture);
    let policy = AiBudgetPolicyRecord::query(fixture.database.pool())
        .limit(1)
        .fetch_one()
        .await
        .unwrap()
        .unwrap();
    // This is actual owning budget configuration, not a fake reservation failure. The
    // already-settled approval turn remains charged; zero capacity denies only new images.
    AiBudgetPolicyRecord::update_by_id(
        &fixture.database,
        &policy.id,
        crate::persistence::UpdateAiBudgetPolicyRecordInput {
            maximum_image_units: Some(Some(0)),
            maximum_input_tokens: Some(Some(20_000)),
            maximum_runs: (mode == 10).then_some(Some(1)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let planner = Arc::new(QuotaRetryPlanner {
        fixture: fixture.clone(),
        application: application.clone(),
        delivery,
        mode,
        retries: AtomicUsize::new(0),
        held_plan: Mutex::new(None),
    });
    let deletion = Arc::new(ConfirmPreviewDeletion {
        custody: custody.clone(),
        fail: false,
        requests: Mutex::new(Vec::new()),
        created_at_delete: Mutex::new(Vec::new()),
    });
    let executor = Arc::new(
        AiProviderCallExecutor::new(
            fixture.runtime.clone(),
            fixture.budget_service.clone(),
            fixture.audit.clone(),
            Arc::new(ImageAccounting),
            Arc::new(SystemClock),
            AiProviderCallLimits::new(64, 8192, 65536).unwrap(),
        )
        .with_tool_result_source_service(application.clone())
        .unwrap()
        .with_static_offering_reset_deletion_service(deletion.clone()),
    );
    let output = Arc::new(OrmAiProviderOutputService::new(
        fixture.run_service.clone(),
        Arc::new(Resolver(fixture.principal.clone())),
        Arc::new(AllowAccess),
        Arc::new(ProtectionPolicy),
        Arc::new(DatabaseManagedContentProtector),
        Arc::new(SystemClock),
        Default::default(),
    ));
    let coordinator = AiSupervisedAgentCoordinator::new(
        Arc::new(fixture.run_service.clone()),
        executor,
        output,
        checkpoints.clone(),
        checkpoints.clone(),
        consequential.clone(),
        application.clone(),
        resume,
        Arc::new(NativeTestRuleResolver),
        planner.clone(),
        Arc::new(SystemClock),
        AiSupervisedAgentCoordinatorLimits::new(
            AiAgentLoopLimits::new(4, 8).unwrap(),
            Duration::seconds(1),
            Duration::minutes(5),
            false,
        )
        .unwrap(),
    )
    .with_provider_session_service(sessions)
    .with_classified_native_tools(application, consequential, checkpoints);
    custody.complete_after_capture.store(true, Ordering::SeqCst);
    custody.requests.lock().await.clear();
    let events_before = AiSessionEventRecord::query(fixture.database.pool())
        .count()
        .await
        .unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(35),
        Box::pin(coordinator.execute_approved_claim(&approved)),
    )
    .await
    .expect("a pending host retry must end within its finite same-lease bound");
    assert_eq!(custody.effects.load(Ordering::SeqCst), 1, "{result:?}");
    assert_eq!(custody.delivered_images.load(Ordering::SeqCst), 0);
    let calls = AiToolCallRecord::query(fixture.database.pool())
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].state, "completed");
    assert!(calls[0].protected_result.is_some());
    assert_eq!(
        AiApprovalRecord::find_by_id(&fixture.database, &approved.approval_id().0)
            .await
            .unwrap()
            .unwrap()
            .consumed_uses,
        1
    );
    // No human message was redispatched. Budget/assistant status events may be appended
    // by the owning coordinator, so count the exact original message event separately.
    let queued = AiSessionEventRecord::query(fixture.database.pool())
        .filter(crate::persistence::AiSessionEventRecordWhereInput {
            event_type: Some(graphql_orm::graphql::filters::StringFilter {
                eq: Some("message_queued".into()),
                ..Default::default()
            }),
            ..Default::default()
        })
        .count()
        .await
        .unwrap();
    assert_eq!(queued, 1);
    assert!(
        AiSessionEventRecord::query(fixture.database.pool())
            .count()
            .await
            .unwrap()
            >= events_before
    );
    let requests = custody.requests.lock().await;
    match mode {
        8 => {
            assert!(
                matches!(result, Ok(AiSupervisedAgentRunOutcome::Completed { .. })),
                "{result:?}"
            );
            assert_eq!(planner.retries.load(Ordering::SeqCst), 1);
            assert_eq!(requests.len(), 1);
            assert!(requests[0].tools.is_empty());
            assert!(requests[0].continuation.is_none());
            let serialized = serde_json::to_string(&requests[0].input).unwrap();
            assert!(serialized.contains("FrameworkTaskContinuationContext"));
            assert!(serialized.contains("historicalToolContentWithheld"));
            assert!(!serialized.contains("FrameworkApprovedToolOutcome"));
            assert!(!serialized.contains("Preview"));
            assert!(!serialized.contains("data:image"));
            // Approval thread, denied empty thread, reduced empty thread, and the exact
            // tool-free withheld projection each have real retained custody.
            assert!(custody.created_threads.load(Ordering::SeqCst) >= 3);
            assert!(deletion.requests.lock().await.len() >= 2);
        }
        9 => {
            assert!(
                matches!(
                    result,
                    Ok(AiSupervisedAgentRunOutcome::RecoveryRequired { .. })
                ),
                "{result:?}"
            );
            assert_eq!(planner.retries.load(Ordering::SeqCst), 0);
            assert!(planner.held_plan.lock().await.is_some());
            assert!(requests.is_empty());
        }
        10 | 12 => {
            assert!(
                matches!(result, Ok(AiSupervisedAgentRunOutcome::Failed { .. })),
                "{result:?}"
            );
            assert_eq!(planner.retries.load(Ordering::SeqCst), 1);
            assert!(requests.is_empty());
        }
        11 | 13 => {
            assert!(
                matches!(
                    result,
                    Ok(AiSupervisedAgentRunOutcome::Cancelled { .. }) | Err(AiError::Conflict)
                ),
                "{result:?}"
            );
            assert_eq!(planner.retries.load(Ordering::SeqCst), 1);
            assert!(requests.is_empty());
        }
        _ => unreachable!(),
    }
    let budgets = AiBudgetReservationRecord::query(fixture.database.pool())
        .fetch_all()
        .await
        .unwrap();
    assert!(
        budgets
            .iter()
            .all(|row| matches!(row.state.as_str(), "committed" | "released"))
    );
    assert_eq!(
        budgets
            .iter()
            .map(|row| row.actual_image_units.unwrap_or(0))
            .sum::<i64>(),
        0
    );
    drop(requests);
    assert!(
        Box::pin(coordinator.execute_approved_claim(&approved))
            .await
            .is_err()
    );
    assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn approved_native_quota_retry_is_once_and_preserves_effect_task_and_external_freeze() {
    for mode in [8, 9, 10, 11] {
        Box::pin(approved_media_lifecycle(false, false, 0, Some(mode))).await;
    }
}

#[tokio::test]
async fn approved_native_pending_quota_replan_is_bounded_and_owner_cancellable() {
    for mode in [12, 13] {
        Box::pin(approved_media_lifecycle(false, false, 0, Some(mode))).await;
    }
}
