use super::*;
use crate::{
    AiProvider, AiProviderSessionActivation, AiSupervisedAgentCoordinator,
    AiSupervisedAgentCoordinatorLimits, AiSupervisedAgentRunOutcome, AiSupervisedAgentTurnPlan,
    AiSupervisedAgentTurnPlanner, OrmAiProviderOutputService, ProviderCapabilities,
    ProviderEventStream,
};

#[derive(Default)]
struct MixedRetainedProvider {
    turns: AtomicUsize,
    created: AtomicUsize,
    replies: Arc<Mutex<Vec<serde_json::Value>>>,
    database: std::sync::Mutex<Option<Database<SqliteBackend>>>,
}

#[async_trait]
impl AiProvider for MixedRetainedProvider {
    fn provider_kind(&self) -> ProviderKind {
        ProviderKind::OpenAiCompatible
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            custom_tools: true,
            provider_retained_continuation: true,
            capability_delivery_modes: [AiCapabilityDeliveryMode::EagerExact].into_iter().collect(),
            ..Default::default()
        }
    }

    async fn create_empty_session(
        &self,
        _: &crate::AiProviderRunBinding,
        _: &AiProviderSessionDescriptor,
        _: &ModelRequest,
    ) -> Result<AiProviderSessionCursor, ProviderError> {
        assert_eq!(self.created.fetch_add(1, Ordering::SeqCst), 0);
        AiProviderSessionCursor::new("test.thread", "coordinator-native-thread")
            .map_err(|_| ProviderError::Rejected)
    }

    async fn stream(
        &self,
        _: ModelRequest,
        _: ProviderRequestContext,
    ) -> Result<ProviderEventStream, ProviderError> {
        panic!("mixed coordinator must use the native callback transport")
    }

    async fn stream_with_dynamic_tools(
        &self,
        request: ModelRequest,
        context: ProviderRequestContext,
        responder: Arc<dyn ProviderDynamicToolResponder>,
    ) -> Result<ProviderEventStream, ProviderError> {
        context.validate_request(&self.provider_kind(), &request)?;
        let turn = self.turns.fetch_add(1, Ordering::SeqCst);
        let opened = context.provider_session().ok_or(ProviderError::Rejected)?;
        assert_eq!(opened.cursor().kind(), "test.thread");
        if turn == 0 {
            assert_eq!(
                opened.activation(),
                AiProviderSessionActivation::NewlyBoundEmpty
            );
            assert!(request.continuation.is_none());
        } else {
            assert_eq!(turn, 1, "approval resume must not replay a provider turn");
            assert_eq!(
                opened.activation(),
                AiProviderSessionActivation::ExistingRetained
            );
            assert!(
                matches!(request.input.as_slice(), [ModelInputBlock::Json { value }]
                if value["kind"] == "FrameworkApprovedToolOutcome" && value["state"] == "completed")
            );
            assert!(request.native_approved_outcome_hash().is_some());
        }
        let replies = self.replies.clone();
        let database = self.database.lock().unwrap().clone().unwrap();
        Ok(Box::pin(async_stream::try_stream! {
            let response = format!("native-coordinator-{turn}");
            yield ProviderEvent::ResponseStarted { response_id: Some(response.clone()) };
            if turn == 0 {
                for (index, tool_id, record_id) in [
                    (0, "records.automatic", "ordinary"),
                    (1, "records.automatic", "review"),
                    (2, "records.automatic", "later"),
                    (3, "records.read", "after-pending"),
                ] {
                    let definition = request.tools.iter().find(|tool| tool.tool_id == tool_id).unwrap();
                    let call_id = format!("native-coordinator-call-{index}");
                    let arguments = json!({"recordId":record_id});
                    yield ProviderEvent::ToolCallStarted { call_id:call_id.clone(), tool_id:tool_id.to_owned() };
                    let reply = responder.respond(ProviderDynamicToolCall::from_definition(
                        &response, &call_id, definition, arguments.clone(),
                    )?).await?;
                    replies.lock().await.push(reply.output().clone());
                    assert!(AiApprovalRecord::query(database.pool()).fetch_all().await.unwrap().is_empty(),
                        "pending callback must not expose an approvable grant before real settlement");
                    yield ProviderEvent::ToolCallCompleted { call_id, arguments };
                }
            } else {
                yield ProviderEvent::TextDelta { text:"Approved action completed.".to_owned() };
            }
            yield ProviderEvent::Usage { input_tokens:19, output_tokens:7, cached_input_tokens:0 };
            yield ProviderEvent::ResponseCompleted { response_id:Some(response) };
        }))
    }
}

struct MixedPlanner {
    fixture: Arc<Fixture>,
    descriptor: AiProviderSessionDescriptor,
}

impl MixedPlanner {
    async fn build(
        &self,
        lease: &AiRunLease,
        continuation: Option<crate::AiAgentContinuation>,
    ) -> Result<AiSupervisedAgentTurnPlan, AiError> {
        let mut base = plan(&self.fixture);
        let mut policy = AiToolPolicySet::new(ToolMaturity::AutonomousWrite);
        for id in ["records.read", "records.automatic"] {
            let descriptor = self
                .fixture
                .runtime
                .tool_catalog()
                .descriptor(&AiToolId::parse(id)?)
                .unwrap();
            policy.bind(AiToolPolicyBinding {
                tool_id: descriptor.id.clone(),
                fingerprint: descriptor.fingerprint.clone(),
                enabled: true,
            });
            base.request.tools.push(ModelToolDefinition {
                tool_id: id.to_owned(),
                provider_name: id.replace('.', "_"),
                fingerprint: descriptor.fingerprint.clone(),
                description: descriptor.description.clone(),
                parameters: descriptor.argument_schema.clone(),
                strict: true,
                defer_loading: false,
            });
        }
        base.budget.attempt_id = lease.attempt_id();
        base.budget.lease_generation = lease.lease_generation();
        base.budget.idempotency_key = format!("mixed-native:{}", lease.attempt_id());
        base.transfers[0].estimated_bytes = 16_384;
        let resumed = continuation.is_some();
        let provider = if let Some(continuation) = continuation {
            base.request.input.clear();
            AiProviderCallPlan::new_continuation_with_classified_tools(
                base.provider_kind,
                base.request,
                base.budget,
                base.transfers,
                "mixed-native-coordinator",
                continuation,
                self.fixture.runtime.tool_catalog(),
                &policy,
                &self.fixture.generated_target_policy,
            )?
        } else {
            AiProviderCallPlan::new_with_classified_tools(
                base.provider_kind,
                base.request,
                base.budget,
                base.transfers,
                "mixed-native-coordinator",
                self.fixture.runtime.tool_catalog(),
                &policy,
                &self.fixture.generated_target_policy,
            )?
        };
        let transcript = if resumed {
            let bindings = crate::orm_provider_session::AiProviderSessionBindingRecord::query(
                self.fixture.database.pool(),
            )
            .fetch_all()
            .await
            .map_err(|_| AiError::PersistenceFailed)?;
            assert_eq!(bindings.len(), 1);
            bindings[0].transcript_fingerprint.clone()
        } else {
            "d".repeat(64)
        };
        AiSupervisedAgentTurnPlan::new_classified_native(
            provider,
            AiProviderSessionTurnPlan::new(self.descriptor.clone(), transcript)?,
            automatic_mutation_route(),
            native_test_rules(self.fixture.scope.clone()),
            false,
        )
    }
}

#[async_trait]
impl AiSupervisedAgentTurnPlanner for MixedPlanner {
    async fn initial_plan(&self, lease: &AiRunLease) -> Result<AiSupervisedAgentTurnPlan, AiError> {
        self.build(lease, None).await
    }
    async fn continuation_plan(
        &self,
        lease: &AiRunLease,
        provider_turns: u32,
        continuation: crate::AiAgentContinuation,
    ) -> Result<AiSupervisedAgentTurnPlan, AiError> {
        assert_eq!(provider_turns, 1);
        self.build(lease, Some(continuation)).await
    }
}

#[tokio::test]
async fn native_coordinator_retained_mixed_turn_parks_then_resumes_only_pending_effect() {
    Box::pin(native_coordinator_lifecycle()).await;
}

async fn native_coordinator_lifecycle() {
    let provider = Arc::new(MixedRetainedProvider::default());
    let fixture = Arc::new(
        Box::pin(
            fixture_with_optional_provider_and_automatic_static_with_start(
                MockProvider::new(Vec::new()),
                Arc::new(AllowAccess),
                true,
                Some(provider.clone()),
                true,
                false,
            ),
        )
        .await,
    );
    *provider.database.lock().unwrap() = Some(fixture.database.clone());
    let session = AiSessionRecord::find_by_id(&fixture.database, &fixture.lease.session_id().0)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        AiSessionRecord::compare_and_swap(
            &fixture.database,
            &session.id,
            session.row_version,
            AiSessionRecordWhereInput::default(),
            UpdateAiSessionRecordInput {
                message_head: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap(),
        ConditionalUpdateOutcome::Updated(_)
    ));
    AiMessageRecord::insert(
        &fixture.database,
        CreateAiMessageRecordInput {
            id: fixture.lease.input_message_id(),
            session_id: session.id,
            sequence: 1,
            message_role: "user".to_owned(),
            author_principal_kind: Some("user".to_owned()),
            author_subject: Some(fixture.principal.subject().to_owned()),
            client_message_id: Some(Uuid::new_v4()),
            content_hash: Some("c".repeat(64)),
            run_id: Some(fixture.lease.run_id().0),
            provider_kind: None,
            provider_model: None,
            protected_preview: None,
            block_count: 1,
            completion_state: "complete".to_owned(),
            finalized_at: Some(OffsetDateTime::now_utc().unix_timestamp()),
            content_purged_at: None,
        },
    )
    .await
    .unwrap();
    let sessions = Arc::new(
        OrmAiProviderSessionService::new(
            fixture.database.clone(),
            Arc::new(AllowAccess),
            Arc::new(ProtectionPolicy),
            Arc::new(DatabaseManagedContentProtector),
            Arc::new(Resolver(fixture.principal.clone())),
            Arc::new(SystemClock),
            AiProviderSessionLimits::default(),
            Duration::minutes(5),
        )
        .unwrap(),
    );
    let (consequential, approvals) = consequential_test_service(&fixture);
    let consequential = Arc::new(consequential.with_provider_session_service(sessions.clone()));
    let applications = Arc::new(automatic_mutation_service(&fixture, fixture.audit.clone()));
    let checkpoints = Arc::new(OrmAiCoordinatorCheckpointService::new(
        fixture.run_service.clone(),
        Arc::new(Resolver(fixture.principal.clone())),
        Arc::new(AllowAccess),
        Arc::new(ProtectionPolicy),
        Arc::new(DatabaseManagedContentProtector),
        Arc::new(NativeTestRuleResolver),
        Arc::new(SystemClock),
        AiCoordinatorCheckpointLimits::new(256 * 1024, Duration::seconds(30)).unwrap(),
    ));
    let executor = Arc::new(AiProviderCallExecutor::new(
        fixture.runtime.clone(),
        fixture.budget_service.clone(),
        fixture.audit.clone(),
        Arc::new(TestUsageAccounting),
        Arc::new(SystemClock),
        AiProviderCallLimits::new(64, 8192, 65536).unwrap(),
    ));
    let output = Arc::new(OrmAiProviderOutputService::new(
        fixture.run_service.clone(),
        Arc::new(Resolver(fixture.principal.clone())),
        Arc::new(AllowAccess),
        Arc::new(ProtectionPolicy),
        Arc::new(DatabaseManagedContentProtector),
        Arc::new(SystemClock),
        Default::default(),
    ));
    let resume = Arc::new(OrmAiSupervisedResumeService::new(
        fixture.run_service.clone(),
        checkpoints.clone(),
        consequential.clone(),
    ));
    let planner = Arc::new(MixedPlanner {
        fixture: fixture.clone(),
        descriptor: AiProviderSessionDescriptor::new(
            ProviderKind::OpenAiCompatible,
            "mock-profile",
            "mock-model",
            "a".repeat(64),
            "mock/v1",
            "b".repeat(64),
        )
        .unwrap(),
    });
    let coordinator = AiSupervisedAgentCoordinator::new(
        Arc::new(fixture.run_service.clone()),
        executor,
        output,
        checkpoints.clone(),
        checkpoints.clone(),
        consequential.clone(),
        applications.clone(),
        resume,
        Arc::new(NativeTestRuleResolver),
        planner,
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
    .with_classified_native_tools(applications, consequential, checkpoints);
    let waiting = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        Box::pin(coordinator.execute_claimed(&fixture.lease)),
    )
    .await
    .unwrap()
    .unwrap();
    let AiSupervisedAgentRunOutcome::WaitingApproval {
        approval_id,
        provider_turns,
        total_tool_calls,
        ..
    } = waiting
    else {
        panic!("expected durable native approval wait: {waiting:?}")
    };
    assert_eq!((provider_turns, total_tool_calls), (1, 4));
    assert_eq!(fixture.completed_executions.load(Ordering::SeqCst), 1);
    let replies = provider.replies.lock().await.clone();
    assert_eq!(replies.len(), 4);
    assert_eq!(replies[1]["status"], "ApprovalPending");
    assert_eq!(replies[2]["status"], "ConsequentialCallsPaused");
    assert_eq!(replies[1]["effectExecuted"], false);
    assert_eq!(replies[2]["retryAllowed"], false);
    let calls = AiToolCallRecord::query(fixture.database.pool())
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(calls.len(), 4);
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.state == "completed")
            .count(),
        2
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.state == "control_blocked")
            .count(),
        1
    );
    let binding =
        crate::orm_provider_session::AiProviderSessionBindingRecord::query(fixture.database.pool())
            .fetch_all()
            .await
            .unwrap();
    assert_eq!(binding.len(), 1);
    assert_eq!(binding[0].state, "parked_wait");
    let approval = AiApprovalRecord::find_by_id(&fixture.database, &approval_id.0)
        .await
        .unwrap()
        .unwrap();
    approvals
        .decide_approval(
            &fixture.principal,
            DecideAiApprovalInput {
                id: approval.id,
                decision: AiApprovalDecision::Approve,
                expected_version: approval.row_version,
            },
        )
        .await
        .unwrap();
    let claim = fixture
        .run_service
        .claim_next_approved("coordinator-native-resume")
        .await
        .unwrap()
        .unwrap();
    let completed = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        Box::pin(coordinator.execute_approved_claim(&claim)),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        matches!(
            completed,
            AiSupervisedAgentRunOutcome::Completed {
                provider_turns: 2,
                total_tool_calls: 4,
                ..
            }
        ),
        "{completed:?}"
    );
    assert_eq!(fixture.completed_executions.load(Ordering::SeqCst), 2);
    assert_eq!(provider.turns.load(Ordering::SeqCst), 2);
    assert_eq!(provider.created.load(Ordering::SeqCst), 1);
    assert_eq!(provider.replies.lock().await.len(), 4);
    let calls = AiToolCallRecord::query(fixture.database.pool())
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(calls.len(), 4);
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.state == "completed")
            .count(),
        3
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.state == "control_blocked")
            .count(),
        1
    );
    let budgets = AiBudgetReservationRecord::query(fixture.database.pool())
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(budgets.len(), 2);
    assert!(budgets.iter().all(|budget| budget.state == "committed"));
    let approval = AiApprovalRecord::find_by_id(&fixture.database, &approval.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(approval.consumed_uses, 1);
    assert!(coordinator.execute_approved_claim(&claim).await.is_err());
    assert_eq!(fixture.completed_executions.load(Ordering::SeqCst), 2);
}
