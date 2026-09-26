use super::*;
use crate::{
    AiProvider, AiProviderSessionActivation, AiSupervisedAgentCoordinator,
    AiSupervisedAgentCoordinatorLimits, AiSupervisedAgentRunOutcome, AiSupervisedAgentTurnPlan,
    AiSupervisedAgentTurnPlanner, OrmAiProviderOutputService, ProviderCapabilities,
    ProviderEventStream,
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum CallbackScenario {
    #[default]
    MixedApproval,
    InvalidReadFirst,
    InvalidMutation,
    FixedBroker,
}

#[derive(Default)]
struct MixedRetainedProvider {
    scenario: CallbackScenario,
    turns: AtomicUsize,
    created: AtomicUsize,
    invalid_mutations_rejected: Arc<AtomicUsize>,
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
            capability_delivery_modes: [
                AiCapabilityDeliveryMode::EagerExact,
                AiCapabilityDeliveryMode::FixedBroker,
            ]
            .into_iter()
            .collect(),
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
        let scenario = self.scenario;
        let invalid_mutations_rejected = self.invalid_mutations_rejected.clone();
        let replies = self.replies.clone();
        let database = self.database.lock().unwrap().clone().unwrap();
        Ok(Box::pin(async_stream::try_stream! {
            let response = format!("native-coordinator-{turn}");
            yield ProviderEvent::ResponseStarted { response_id: Some(response.clone()) };
            if turn == 0 {
                if matches!(scenario, CallbackScenario::InvalidReadFirst | CallbackScenario::InvalidMutation) {
                    let tool_id = if scenario == CallbackScenario::InvalidReadFirst {
                        "records.read"
                    } else {
                        "records.automatic"
                    };
                    let definition = request.tools.iter().find(|tool| tool.tool_id == tool_id).unwrap();
                    let call_id = "native-coordinator-invalid".to_owned();
                    let arguments = json!({"recordId":false});
                    yield ProviderEvent::ToolCallStarted { call_id:call_id.clone(), tool_id:tool_id.to_owned() };
                    let reply = responder.reject_invalid_arguments(
                        crate::ProviderInvalidDynamicToolCall::from_definition(
                            &response, &call_id, definition, arguments.clone(),
                        )?,
                    ).await;
                    if scenario == CallbackScenario::InvalidMutation {
                        assert!(matches!(reply, Err(ProviderError::Rejected)),
                            "a schema-invalid mutation must never become a safe read retry");
                        invalid_mutations_rejected.fetch_add(1, Ordering::SeqCst);
                        Err::<(), _>(ProviderError::Rejected)?;
                    }
                    let reply = reply?;
                    assert_eq!(reply.output(), &crate::AiApplicationToolFailureEnvelope::new(
                        crate::AiApplicationToolFailureCode::InvalidArguments,
                    ).to_json());
                    replies.lock().await.push(reply.output().clone());
                    let calls = AiToolCallRecord::query(database.pool()).fetch_all().await.unwrap();
                    assert_eq!(calls.len(), 1);
                    assert_eq!(calls[0].state, "execution_failed");
                    assert_eq!(calls[0].tool_call_index, 0);
                    assert_eq!(calls[0].authorization_code.as_deref(), Some("invalid_arguments"));
                    assert!(calls[0].protected_result.is_some());
                    assert!(calls[0].completed_at.is_some());
                    yield ProviderEvent::ToolCallCompleted { call_id, arguments };
                }
                if scenario == CallbackScenario::FixedBroker {
                    let definition = request.tools.iter().find(|tool| tool.tool_id == AI_CAPABILITY_DISCOVER_TOOL_ID).unwrap();
                    let call_id = "native-broker-discover".to_owned();
                    let arguments = json!({"text":"records", "maximumResults":2});
                    yield ProviderEvent::ToolCallStarted { call_id:call_id.clone(), tool_id:definition.tool_id.clone() };
                    let reply = responder.respond(ProviderDynamicToolCall::from_definition(
                        &response, &call_id, definition, arguments.clone(),
                    )?).await?;
                    assert!(reply.output()["candidates"].is_array(), "broker should return bounded discovery metadata: {:?}", reply.output());
                    let candidate = reply.output()["candidates"][0].clone();
                    replies.lock().await.push(reply.output().clone());
                    yield ProviderEvent::ToolCallCompleted { call_id, arguments };
                    let describe = request.tools.iter().find(|tool| tool.tool_id == AI_CAPABILITY_DESCRIBE_TOOL_ID).unwrap();
                    let call_id = "native-broker-describe".to_owned();
                    let arguments = json!({"capabilityId":candidate["capabilityId"], "candidateFingerprint":candidate["candidateFingerprint"]});
                    yield ProviderEvent::ToolCallStarted { call_id:call_id.clone(), tool_id:describe.tool_id.clone() };
                    let reply = responder.respond(ProviderDynamicToolCall::from_definition(&response, &call_id, describe, arguments.clone())?).await?;
                    let loaded = reply.output()["loadedReference"].clone();
                    assert!(loaded.is_string());
                    replies.lock().await.push(reply.output().clone());
                    yield ProviderEvent::ToolCallCompleted { call_id, arguments };
                    let execute = request.tools.iter().find(|tool| tool.tool_id == AI_CAPABILITY_EXECUTE_TOOL_ID).unwrap();
                    let call_id = "native-broker-execute".to_owned();
                    let arguments = json!({"loadedReference":loaded,
                        "arguments":[{"name":"recordId", "value":"record-42"}],
                        "selections":["recordId", "subject"], "relationshipArguments":[],
                        "relationshipMaximumItems":[], "maximumItems":null});
                    yield ProviderEvent::ToolCallStarted { call_id:call_id.clone(), tool_id:execute.tool_id.clone() };
                    let reply = responder.respond(ProviderDynamicToolCall::from_definition(&response, &call_id, execute, arguments.clone())?).await?;
                    assert_eq!(reply.output()["data"]["GeneratedRecord"]["recordId"], "record-42", "broker execute should return exact resolver result: {:?}", reply.output());
                    replies.lock().await.push(reply.output().clone());
                    yield ProviderEvent::ToolCallCompleted { call_id, arguments };
                }
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
    broker: bool,
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
        let delivery = self
            .broker
            .then(|| mixed_broker_delivery(&self.fixture, base.request.tools.clone()));
        if let Some(delivery) = &delivery {
            base.request.tools = delivery.current_tools();
        }
        let resumed = continuation.is_some();
        if let Some(delivery) = &delivery {
            let surface = delivery.current_surface();
            let check = |request: ModelRequest| {
                AiProviderCallPlan::new_with_classified_capability_surface(
                    base.provider_kind.clone(),
                    request,
                    base.budget.clone(),
                    base.transfers.clone(),
                    "mixed-native-coordinator",
                    &surface,
                    self.fixture.runtime.tool_catalog(),
                    &policy,
                    &self.fixture.generated_target_policy,
                )
            };
            let mut swapped = base.request.clone();
            swapped.tools[0].description.push_str(" altered");
            assert!(
                check(swapped).is_err(),
                "exact minted definitions must not be editable"
            );
            let mut omitted = base.request.clone();
            omitted.tools.pop();
            assert!(
                check(omitted).is_err(),
                "a partial surface must not pass exact binding"
            );
            assert!(
                AiProviderCallPlan::new_with_capability_surface(
                    base.provider_kind.clone(),
                    base.request.clone(),
                    base.budget.clone(),
                    base.transfers.clone(),
                    "mixed-native-coordinator",
                    &surface,
                    self.fixture.runtime.tool_catalog(),
                    &policy,
                    &self.fixture.generated_target_policy,
                )
                .is_err(),
                "legacy surface constructors must remain read-only"
            );
        }
        let provider = if let Some(delivery) = &delivery {
            let surface = delivery.current_surface();
            if let Some(continuation) = continuation {
                base.request.input.clear();
                AiProviderCallPlan::new_continuation_with_classified_capability_surface(
                    base.provider_kind,
                    base.request,
                    base.budget,
                    base.transfers,
                    "mixed-native-coordinator",
                    continuation,
                    &surface,
                    self.fixture.runtime.tool_catalog(),
                    &policy,
                    &self.fixture.generated_target_policy,
                )?
            } else {
                AiProviderCallPlan::new_with_classified_capability_surface(
                    base.provider_kind,
                    base.request,
                    base.budget,
                    base.transfers,
                    "mixed-native-coordinator",
                    &surface,
                    self.fixture.runtime.tool_catalog(),
                    &policy,
                    &self.fixture.generated_target_policy,
                )?
            }
        } else if let Some(continuation) = continuation {
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
        if let Some(delivery) = &delivery {
            let swapped_session = AiSupervisedAgentTurnPlan::new_classified_native(
                provider.clone(),
                AiProviderSessionTurnPlan::new(self.descriptor.clone(), transcript.clone())?,
                automatic_mutation_route(),
                native_test_rules(self.fixture.scope.clone()),
                false,
            )?;
            assert!(
                swapped_session
                    .with_capability_delivery(delivery.clone())
                    .is_err(),
                "surface fingerprint must match the retained provider session"
            );
        }
        let descriptor = if let Some(delivery) = &delivery {
            AiProviderSessionDescriptor::new(
                ProviderKind::OpenAiCompatible,
                "mock-profile",
                "mock-model",
                delivery.session_binding().fingerprint(),
                "mock/v1",
                "b".repeat(64),
            )?
        } else {
            self.descriptor.clone()
        };
        let plan = AiSupervisedAgentTurnPlan::new_classified_native(
            provider,
            AiProviderSessionTurnPlan::new(descriptor, transcript)?,
            automatic_mutation_route(),
            native_test_rules(self.fixture.scope.clone()),
            false,
        )?;
        if let Some(delivery) = delivery {
            plan.with_capability_delivery(delivery)
        } else {
            Ok(plan)
        }
    }
}

fn mixed_broker_delivery(
    fixture: &Fixture,
    definitions: Vec<ModelToolDefinition>,
) -> AiCapabilityDeliveryTurn {
    let (semantics, query_catalog) = generated_query_catalog();
    let index = Arc::new(
        AiCapabilityIndex::compile(
            GraphqlExecutionTargetId::parse("generated-read-application").unwrap(),
            query_catalog.finished_schema_fingerprint(),
            &semantics,
            Some(&query_catalog),
            None,
            None,
            [],
            "target-policy-v1",
            AiCapabilityIndexLimits::default(),
        )
        .unwrap(),
    );
    let index_set = AiCapabilityIndexSet::compile([index.clone()]).unwrap();
    let broker = Arc::new(
        AiCapabilityDiscoveryBroker::new(
            Arc::new(Resolver(fixture.principal.clone())),
            Arc::new(BrokerCurrentIndex(index)),
            Arc::new(BrokerAuthority {
                allowed: AtomicBool::new(true),
                policy_fingerprint: RwLock::new("current-policy-v1".to_owned()),
            }),
            Arc::new(SystemClock),
            Duration::seconds(30),
        )
        .unwrap(),
    );
    let binding = AiProviderCapabilitySessionBinding::new(
        AiCapabilityDeliveryMode::FixedBroker,
        index_set.fingerprint(),
        definitions.iter().map(|d| d.fingerprint.clone()).collect(),
        "test-provider-projection-v1",
        "mock-model",
        ModelReasoningEffort::Unspecified,
        "a".repeat(64),
    )
    .unwrap();
    AiCapabilityDeliveryTurn::select(
        &ProviderCapabilities {
            custom_tools: true,
            capability_delivery_modes: [AiCapabilityDeliveryMode::FixedBroker]
                .into_iter()
                .collect(),
            ..Default::default()
        },
        index_set.fingerprint(),
        definitions,
        true,
        binding,
        broker,
        AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default()).unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn native_coordinator_fixed_broker_survives_mixed_approval_without_replay() {
    Box::pin(native_coordinator_lifecycle(CallbackScenario::FixedBroker)).await;
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
    Box::pin(native_coordinator_lifecycle(
        CallbackScenario::MixedApproval,
    ))
    .await;
}

#[tokio::test]
async fn native_coordinator_schema_invalid_read_accounts_once_and_continues() {
    Box::pin(native_coordinator_lifecycle(
        CallbackScenario::InvalidReadFirst,
    ))
    .await;
}

#[tokio::test]
async fn native_coordinator_schema_invalid_mutation_denies_without_effect_or_approval() {
    Box::pin(native_coordinator_lifecycle(
        CallbackScenario::InvalidMutation,
    ))
    .await;
}

async fn native_coordinator_lifecycle(scenario: CallbackScenario) {
    let provider = Arc::new(MixedRetainedProvider {
        scenario,
        ..Default::default()
    });
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
    let (_, approvals) = consequential_test_service(&fixture);
    let consequential = OrmAiConsequentialToolCallService::new(
        fixture.run_service.clone(),
        fixture.runtime.clone(),
        approvals.clone(),
        Arc::new(PreviewBuilder),
        fixture.audit.clone(),
        Arc::new(SystemClock),
        AiApplicationToolCallLimits::new(
            8_192,
            16_384,
            4,
            8,
            Duration::seconds(30),
            Duration::seconds(10),
        )
        .unwrap(),
    );
    let consequential = Arc::new(consequential.with_provider_session_service(sessions.clone()));
    // This fixture can prepend a rejected read to the four mixed callbacks.
    // Keep application admission aligned with its eight-call coordinator limit.
    let applications = Arc::new(OrmAiApplicationToolCallService::new(
        fixture.run_service.clone(),
        fixture.runtime.clone(),
        fixture.audit.clone(),
        Arc::new(SystemClock),
        AiApplicationToolCallLimits::new(
            8_192,
            16_384,
            4,
            8,
            Duration::seconds(30),
            Duration::seconds(10),
        )
        .unwrap(),
    ));
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
        broker: scenario == CallbackScenario::FixedBroker,
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
    .unwrap();
    if scenario == CallbackScenario::InvalidMutation {
        if let Ok(outcome) = &waiting {
            assert!(!matches!(
                outcome,
                AiSupervisedAgentRunOutcome::Completed { .. }
                    | AiSupervisedAgentRunOutcome::WaitingApproval { .. }
            ));
        }
        assert_eq!(fixture.completed_executions.load(Ordering::SeqCst), 0);
        assert_eq!(
            provider.invalid_mutations_rejected.load(Ordering::SeqCst),
            1
        );
        assert_eq!(provider.turns.load(Ordering::SeqCst), 1);
        assert!(provider.replies.lock().await.is_empty());
        assert!(
            AiToolCallRecord::query(fixture.database.pool())
                .fetch_all()
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            AiApprovalRecord::query(fixture.database.pool())
                .fetch_all()
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            crate::persistence::AiNativeApprovalCandidateRecord::query(fixture.database.pool())
                .fetch_all()
                .await
                .unwrap()
                .is_empty()
        );
        return;
    }
    let waiting = waiting.unwrap();
    let extra_read = usize::from(scenario == CallbackScenario::InvalidReadFirst);
    let broker_executions = usize::from(scenario == CallbackScenario::FixedBroker);
    let broker_calls = 3 * broker_executions;
    let expected_calls = 4 + extra_read + broker_calls;
    let AiSupervisedAgentRunOutcome::WaitingApproval {
        approval_id,
        provider_turns,
        total_tool_calls,
        ..
    } = waiting
    else {
        panic!(
            "expected durable native approval wait: {waiting:?}; provider turns={}, created={}, replies={:?}",
            provider.turns.load(Ordering::SeqCst),
            provider.created.load(Ordering::SeqCst),
            provider.replies.lock().await
        )
    };
    assert_eq!(
        (provider_turns, total_tool_calls),
        (1, expected_calls as u32)
    );
    assert_eq!(
        fixture.completed_executions.load(Ordering::SeqCst),
        1 + broker_executions
    );
    let replies = provider.replies.lock().await.clone();
    assert_eq!(replies.len(), expected_calls);
    let replies = &replies[extra_read + broker_calls..];
    assert_eq!(replies[1]["status"], "ApprovalPending");
    assert_eq!(replies[2]["status"], "ConsequentialCallsPaused");
    assert_eq!(replies[1]["effectExecuted"], false);
    assert_eq!(replies[2]["retryAllowed"], false);
    let calls = AiToolCallRecord::query(fixture.database.pool())
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(calls.len(), expected_calls);
    let mut indices = calls
        .iter()
        .map(|call| call.tool_call_index)
        .collect::<Vec<_>>();
    indices.sort_unstable();
    assert_eq!(indices, (0..expected_calls as i64).collect::<Vec<_>>());
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.state == "execution_failed")
            .count(),
        extra_read
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.state == "completed")
            .count(),
        2 + broker_calls
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
                total_tool_calls,
                ..
            } if total_tool_calls == expected_calls as u32
        ),
        "{completed:?}"
    );
    assert_eq!(
        fixture.completed_executions.load(Ordering::SeqCst),
        2 + broker_executions
    );
    assert_eq!(provider.turns.load(Ordering::SeqCst), 2);
    assert_eq!(provider.created.load(Ordering::SeqCst), 1);
    assert_eq!(provider.replies.lock().await.len(), expected_calls);
    let calls = AiToolCallRecord::query(fixture.database.pool())
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(calls.len(), expected_calls);
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.state == "execution_failed")
            .count(),
        extra_read
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.state == "completed")
            .count(),
        3 + broker_calls
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
    assert_eq!(
        fixture.completed_executions.load(Ordering::SeqCst),
        2 + broker_executions
    );
}
