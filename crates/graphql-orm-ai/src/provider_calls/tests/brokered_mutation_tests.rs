//! Generated mutations reached through the fixed capability broker.
use super::*;

/// A fixed-broker delivery over the fixture's registered automatic mutation,
/// with mutations brokered under `rules`.
fn brokered_mutation_delivery(
    fixture: &Fixture,
    rules: &AiResolvedRuleSet,
) -> AiCapabilityDeliveryTurn {
    let (semantics, mutation_catalog) = automatic_mutation_catalog();
    let index = Arc::new(
        AiCapabilityIndex::compile(
            GraphqlExecutionTargetId::parse("generated-application").unwrap(),
            mutation_catalog.finished_schema_fingerprint(),
            &semantics,
            None,
            Some(&mutation_catalog),
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
        BTreeSet::new(),
        "test-provider-projection-v1",
        "coordinator-test-model",
        ModelReasoningEffort::Unspecified,
        "a".repeat(64),
    )
    .unwrap();
    let session = AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default())
        .unwrap()
        .with_brokered_mutations();
    session.bind_mutation_rules(rules).unwrap();
    AiCapabilityDeliveryTurn::select(
        &ProviderCapabilities {
            custom_tools: true,
            capability_delivery_modes: [AiCapabilityDeliveryMode::FixedBroker]
                .into_iter()
                .collect(),
            ..Default::default()
        },
        index_set.fingerprint(),
        Vec::new(),
        true,
        binding,
        broker,
        session,
    )
    .unwrap()
}

/// Discovers and describes the fixture's automatic mutation through the
/// broker and returns its loaded reference.
async fn load_mutation(fixture: &Fixture, delivery: &AiCapabilityDeliveryTurn) -> String {
    let run = AiCapabilityRunBinding::from_lease(
        &fixture.lease,
        ProviderKind::OpenAi,
        delivery.session_binding(),
    );
    let principal = fixture.lease.principal_reference();
    let discovery = delivery
        .broker()
        .dispatch_discover(
            principal,
            &run,
            delivery.session(),
            &json!({"text": "create record", "kind": "generated_mutation", "maximumResults": 4}),
        )
        .await
        .expect("mutation discovery");
    let candidate = &discovery["candidates"][0];
    assert_eq!(
        candidate["capabilityId"],
        fixture.automatic_mutation_id.as_ref().unwrap().as_str(),
        "{discovery}"
    );
    let description = delivery
        .broker()
        .dispatch_describe(
            principal,
            &run,
            delivery.session(),
            &json!({
                "capabilityId": candidate["capabilityId"],
                "candidateFingerprint": candidate["candidateFingerprint"],
            }),
        )
        .await
        .expect("mutation description");
    assert_eq!(description.contract()["effect"], "automatic");
    description.loaded_reference().to_owned()
}

fn broker_execute_result(
    fixture: &Fixture,
    delivery: &AiCapabilityDeliveryTurn,
    call_id: &str,
    loaded_reference: &str,
) -> AiProviderCallResult {
    let execute = delivery
        .broker_definition(AiCapabilityBrokerOperation::Execute)
        .expect("execute definition");
    let mut result = AiProviderCallResult::test_result(
        &fixture.lease,
        None,
        "brokered-mutation-response",
        vec![(
            call_id,
            AI_CAPABILITY_EXECUTE_TOOL_ID,
            json!({
                "loadedReference": loaded_reference,
                "arguments": [{"name": "input.subject", "value": "created through the broker"}],
                "selections": ["recordId", "subject"]
            }),
        )],
    );
    result.tool_calls[0].tool_fingerprint = execute.fingerprint.clone();
    result.request_snapshot.tools = delivery.current_tools();
    result
}

/// Gives `result` the committed budget reservation of a settled provider
/// turn, as approved execution requires.
async fn with_committed_budget(
    fixture: &Fixture,
    result: AiProviderCallResult,
) -> AiProviderCallResult {
    let lease = &fixture.lease;
    let budget = AiBudgetReservationRecord::insert(
        &fixture.database,
        CreateAiBudgetReservationRecordInput {
            budget_counter_ids: json!([]),
            scope_kind: fixture.scope.kind.clone(),
            scope_id: fixture.scope.id.clone(),
            tenant_id: fixture.scope.tenant_id.clone(),
            principal_kind: "user".to_owned(),
            principal_subject: fixture.principal.subject().to_owned(),
            session_id: lease.session_id().0,
            run_id: lease.run_id().0,
            attempt_id: lease.attempt_id(),
            lease_generation: lease.lease_generation(),
            provider_kind: result.provider_kind().as_str().to_owned(),
            provider_model: result.provider_model().to_owned(),
            reasoning_effort: result.reasoning_effort().as_str().to_owned(),
            pricing_policy_version: "brokered-mutation-pricing-v1".to_owned(),
            reserved_input_tokens: 1,
            reserved_output_tokens: 1,
            reserved_tool_units: 1,
            reserved_image_units: 0,
            reserved_cost_microunits: 1,
            reserved_runs: 1,
            actual_input_tokens: Some(1),
            actual_cached_input_tokens: Some(0),
            actual_output_tokens: Some(1),
            actual_tool_units: Some(0),
            actual_image_units: Some(0),
            actual_cost_microunits: Some(0),
            actual_runs: Some(1),
            idempotency_key: format!("brokered-mutation-budget-{}", Uuid::new_v4()),
            state: "committed".to_owned(),
            expires_at: (OffsetDateTime::now_utc() + Duration::hours(1)).unix_timestamp(),
            reconciled_at: Some(OffsetDateTime::now_utc().unix_timestamp()),
            settlement_examined_at: 0,
        },
    )
    .await
    .expect("test budget inserts");
    result.test_with_budget_reservation(AiBudgetReservationId(budget.id))
}

#[tokio::test]
async fn brokered_automatic_mutation_runs_once_as_itself_and_answers_the_broker_call() {
    let fixture = automatic_mutation_fixture().await;
    let mutation_id = fixture.automatic_mutation_id.clone().unwrap();
    let mutation_fingerprint = fixture.automatic_mutation_fingerprint.clone().unwrap();
    let rules = native_test_rules(fixture.scope.clone());
    let delivery = brokered_mutation_delivery(&fixture, &rules);
    let loaded_reference = load_mutation(&fixture, &delivery).await;
    let result = broker_execute_result(
        &fixture,
        &delivery,
        "brokered-mutation-call",
        &loaded_reference,
    );
    let service = automatic_mutation_service(&fixture, fixture.audit.clone());

    let resolved = service
        .resolve_brokered_mutation(&fixture.lease, &result, 0, &delivery)
        .await
        .expect("resolution")
        .expect("a loaded mutation resolves");
    assert_eq!(resolved.tool_id, mutation_id);
    assert_eq!(resolved.tool_fingerprint, mutation_fingerprint);
    assert_eq!(
        resolved.provider_tool_id.as_str(),
        AI_CAPABILITY_EXECUTE_TOOL_ID
    );
    // The coordinator classifies the resolved mutation, never the broker.
    assert_eq!(
        resolved.mutation_binding(fixture.runtime.tool_catalog()),
        Some((ToolMaturity::AutonomousWrite, AiApprovalRule::None))
    );
    assert_eq!(
        rules.constrain_tool(
            &resolved.tool_fingerprint,
            ToolMaturity::AutonomousWrite,
            AiApprovalRule::None
        ),
        Some(AiApprovalRule::None)
    );

    let outcome = service
        .execute_brokered_automatic_mutation(
            &fixture.lease,
            &result,
            automatic_mutation_context(&fixture),
            automatic_mutation_route(),
            resolved,
        )
        .await
        .expect("brokered automatic mutation");
    let persisted = outcome.persisted().expect("known durable result");
    assert_eq!(persisted.state(), AiApplicationToolCallState::Completed);
    assert_eq!(fixture.completed_executions.load(Ordering::SeqCst), 1);
    match persisted.model_input() {
        Some(ModelInputBlock::ToolResult {
            call_id, tool_id, ..
        }) => {
            assert_eq!(call_id, "brokered-mutation-call");
            assert_eq!(tool_id, AI_CAPABILITY_EXECUTE_TOOL_ID);
        }
        other => panic!("unexpected model input: {other:?}"),
    }
    let call = AiToolCallRecord::find_by_id(&fixture.database, &persisted.id().0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.tool_id, mutation_id.as_str());
    assert_eq!(call.tool_fingerprint, mutation_fingerprint);
    assert_eq!(
        call.provider_tool_id.as_deref(),
        Some(AI_CAPABILITY_EXECUTE_TOOL_ID)
    );
    assert_eq!(
        call.provider_tool_fingerprint.as_deref(),
        Some(result.tool_calls()[0].tool_fingerprint())
    );
    assert_eq!(call.risk, "non_idempotent_write");
    assert!(crate::orm_tools::row_records_call(&call, &result.tool_calls()[0]).unwrap());
}

#[tokio::test]
async fn the_read_broker_path_refuses_a_brokered_mutation_without_any_effect() {
    let fixture = automatic_mutation_fixture().await;
    let rules = native_test_rules(fixture.scope.clone());
    let delivery = brokered_mutation_delivery(&fixture, &rules);
    let loaded_reference = load_mutation(&fixture, &delivery).await;
    let result = broker_execute_result(
        &fixture,
        &delivery,
        "unrouted-mutation-call",
        &loaded_reference,
    );
    let service = automatic_mutation_service(&fixture, fixture.audit.clone());
    let persisted = service
        .execute_capability_broker_call(
            &fixture.lease,
            &result,
            automatic_mutation_context(&fixture),
            automatic_mutation_route(),
            &delivery,
        )
        .await
        .expect("a durable refusal");
    assert_eq!(
        persisted.state(),
        AiApplicationToolCallState::ExecutionFailed
    );
    assert_eq!(fixture.completed_executions.load(Ordering::SeqCst), 0);
    let Some(ModelInputBlock::ToolResult { output, .. }) = persisted.model_input() else {
        panic!("refusal should answer the broker call");
    };
    assert!(
        output["correction"]
            .as_str()
            .is_some_and(|text| text.contains("cannot request changes")),
        "{output}"
    );
}

#[tokio::test]
async fn rules_that_admit_only_the_broker_never_reach_the_mutation() {
    let fixture = automatic_mutation_fixture().await;
    let base = native_test_rules(fixture.scope.clone());
    let mut constraints = base.constraints().clone();
    // Admit exactly the fixed broker definitions, which are the same for
    // every delivery.
    constraints.allowed_tool_fingerprints = Some(
        brokered_mutation_delivery(&fixture, &base)
            .current_tools()
            .into_iter()
            .map(|definition| definition.fingerprint)
            .collect(),
    );
    let broker_only = AiResolvedRuleSet::new(
        fixture.scope.clone(),
        constraints,
        base.applied_layers().to_vec(),
    )
    .unwrap();
    let delivery = brokered_mutation_delivery(&fixture, &broker_only);
    let run = AiCapabilityRunBinding::from_lease(
        &fixture.lease,
        ProviderKind::OpenAi,
        delivery.session_binding(),
    );
    let discovery = delivery
        .broker()
        .dispatch_discover(
            fixture.lease.principal_reference(),
            &run,
            delivery.session(),
            &json!({"text": "create record", "kind": "generated_mutation", "maximumResults": 4}),
        )
        .await
        .unwrap();
    assert!(discovery["candidates"].as_array().unwrap().is_empty());
}

/// Requests and approves one approval-required generated mutation reached
/// through the broker, returning the approved request.
async fn approved_brokered_mutation(
    fixture: &Fixture,
    call_id: &str,
) -> (
    OrmAiConsequentialToolCallService,
    AiRequestedConsequentialToolCall,
) {
    let mutation_id = fixture.automatic_mutation_id.clone().unwrap();
    let mutation_fingerprint = fixture.automatic_mutation_fingerprint.clone().unwrap();
    // One-shot supervised writes are within the ordinary test rules.
    let rules = test_rules(fixture.scope.clone());
    let delivery = brokered_mutation_delivery(fixture, &rules);
    let run = AiCapabilityRunBinding::from_lease(
        &fixture.lease,
        ProviderKind::OpenAi,
        delivery.session_binding(),
    );
    let principal = fixture.lease.principal_reference();
    let discovery = delivery
        .broker()
        .dispatch_discover(
            principal,
            &run,
            delivery.session(),
            &json!({"text": "create record", "kind": "generated_mutation", "maximumResults": 4}),
        )
        .await
        .unwrap();
    let candidate = &discovery["candidates"][0];
    let description = delivery
        .broker()
        .dispatch_describe(
            principal,
            &run,
            delivery.session(),
            &json!({
                "capabilityId": candidate["capabilityId"],
                "candidateFingerprint": candidate["candidateFingerprint"],
            }),
        )
        .await
        .unwrap();
    assert_eq!(description.contract()["effect"], "approval_required");
    let result = with_committed_budget(
        fixture,
        broker_execute_result(fixture, &delivery, call_id, description.loaded_reference()),
    )
    .await;
    let (service, approvals) = consequential_test_service(fixture);
    let resolved = service
        .application_tools()
        .resolve_brokered_mutation(&fixture.lease, &result, 0, &delivery)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        resolved.mutation_binding(fixture.runtime.tool_catalog()),
        Some((ToolMaturity::SupervisedWrite, AiApprovalRule::OneShot))
    );
    // Through the broker an approval-required mutation is never automatic.
    assert!(
        service
            .application_tools()
            .execute_brokered_automatic_mutation(
                &fixture.lease,
                &result,
                automatic_mutation_context(fixture),
                automatic_mutation_route(),
                resolved.clone(),
            )
            .await
            .is_err()
    );
    let requested = service
        .request_brokered_approval(
            &fixture.lease,
            &result,
            automatic_mutation_context(fixture),
            OffsetDateTime::now_utc() + Duration::minutes(5),
            resolved,
        )
        .await
        .expect("brokered approval request");
    assert_eq!(fixture.completed_executions.load(Ordering::SeqCst), 0);
    let call = AiToolCallRecord::find_by_id(&fixture.database, &requested.tool_call_id().0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.tool_id, mutation_id.as_str());
    assert_eq!(call.tool_fingerprint, mutation_fingerprint);
    assert_eq!(call.provider_call_id, call_id);
    assert_eq!(
        call.provider_tool_id.as_deref(),
        Some(AI_CAPABILITY_EXECUTE_TOOL_ID)
    );
    let approval = AiApprovalRecord::find_by_id(&fixture.database, &requested.approval_id().0)
        .await
        .unwrap()
        .unwrap();
    // The approval binds the resolved mutation and its closed plan.
    assert_eq!(approval.tool_fingerprint, mutation_fingerprint);
    assert_eq!(approval.argument_hash, call.argument_hash);
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
        .expect("approve");
    (service, requested)
}

#[tokio::test]
async fn brokered_approval_required_mutation_keeps_its_approval_and_answers_the_broker_call() {
    let fixture = approval_required_mutation_fixture().await;
    let (service, requested) = approved_brokered_mutation(&fixture, "brokered-approval-call").await;
    let outcome = service
        .execute_approved(
            requested.lease(),
            requested.approval_id(),
            requested.tool_call_id(),
            automatic_mutation_route(),
        )
        .await
        .expect("approved execution");
    let persisted = outcome.persisted().expect("durable approved result");
    assert_eq!(fixture.completed_executions.load(Ordering::SeqCst), 1);
    match persisted.model_input() {
        Some(ModelInputBlock::ToolResult {
            call_id, tool_id, ..
        }) => {
            assert_eq!(call_id, "brokered-approval-call");
            assert_eq!(tool_id, AI_CAPABILITY_EXECUTE_TOOL_ID);
        }
        other => panic!("unexpected model input: {other:?}"),
    }
    // One-shot: the consumed approval never replays.
    assert!(
        service
            .execute_approved(
                requested.lease(),
                requested.approval_id(),
                requested.tool_call_id(),
                automatic_mutation_route(),
            )
            .await
            .is_err()
    );
    assert_eq!(fixture.completed_executions.load(Ordering::SeqCst), 1);
}

/// The generated resolver re-authorizes at consumption exactly as for the
/// direct definition: a refusal there (here, fresh authentication) runs
/// nothing and answers with no success.
#[tokio::test]
async fn resolver_refusal_at_consumption_runs_nothing_for_a_brokered_approval() {
    let fixture = approval_required_mutation_fixture().await;
    let (service, requested) = approved_brokered_mutation(&fixture, "refused-approval-call").await;
    fixture.refusal.store(1, Ordering::SeqCst);
    let refused = service
        .execute_approved(
            requested.lease(),
            requested.approval_id(),
            requested.tool_call_id(),
            automatic_mutation_route(),
        )
        .await;
    assert_eq!(fixture.completed_executions.load(Ordering::SeqCst), 0);
    if let Ok(outcome) = &refused
        && let Some(persisted) = outcome.persisted()
    {
        assert_ne!(persisted.state(), AiApplicationToolCallState::Completed);
    }
}

#[tokio::test]
async fn a_malformed_provider_identity_is_refused_at_approved_execution() {
    let fixture = approval_required_mutation_fixture().await;
    let (service, requested) = approved_brokered_mutation(&fixture, "tampered-approval-call").await;
    // A restored or tampered row whose provider-facing identity is not the
    // broker execute definition never executes.
    AiToolCallRecord::update_by_id(
        &fixture.database,
        &requested.tool_call_id().0,
        UpdateAiToolCallRecordInput {
            provider_tool_id: Some(Some("records.read".to_owned())),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        service
            .execute_approved(
                requested.lease(),
                requested.approval_id(),
                requested.tool_call_id(),
                automatic_mutation_route(),
            )
            .await
            .is_err()
    );
    assert_eq!(fixture.completed_executions.load(Ordering::SeqCst), 0);
}

/// Regression: an approved generated mutation executes. The approval binds
/// the hash of the provider plan, which the runtime now checks instead of
/// the hash of the compiled variables.
#[tokio::test]
async fn direct_approval_required_generated_mutation_executes_once_after_approval() {
    let fixture = approval_required_mutation_fixture().await;
    let result = with_committed_budget(&fixture, automatic_mutation_result(&fixture)).await;
    let (service, approvals) = consequential_test_service(&fixture);
    let requested = service
        .request_approval(
            &fixture.lease,
            &result,
            automatic_mutation_context(&fixture),
            OffsetDateTime::now_utc() + Duration::minutes(5),
            false,
        )
        .await
        .expect("direct approval request");
    let approval = AiApprovalRecord::find_by_id(&fixture.database, &requested.approval_id().0)
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
        .expect("approve");
    let outcome = service
        .execute_approved(
            requested.lease(),
            requested.approval_id(),
            requested.tool_call_id(),
            automatic_mutation_route(),
        )
        .await
        .expect("approved execution");
    assert!(outcome.persisted().is_some());
    assert_eq!(fixture.completed_executions.load(Ordering::SeqCst), 1);
}
