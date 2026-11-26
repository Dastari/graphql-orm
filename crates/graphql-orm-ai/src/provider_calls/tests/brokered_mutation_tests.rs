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
    .with_mutation_rules(rules)
    .unwrap()
}

/// Current rules that stay `rules` for the next `admitting` resolutions and
/// are then revoked: a different rule set that admits no tool fingerprint.
struct RevocableRuleResolver {
    rules: AiResolvedRuleSet,
    admitting: AtomicUsize,
}

impl RevocableRuleResolver {
    fn new(rules: AiResolvedRuleSet) -> Arc<Self> {
        Self::admitting_for(rules, usize::MAX)
    }

    fn admitting_for(rules: AiResolvedRuleSet, admitting: usize) -> Arc<Self> {
        Arc::new(Self {
            rules,
            admitting: AtomicUsize::new(admitting),
        })
    }

    fn revoke_after(&self, admitting: usize) {
        self.admitting.store(admitting, Ordering::SeqCst);
    }
}

#[async_trait]
impl AiAgentRuleResolver for RevocableRuleResolver {
    async fn resolve_rules(
        &self,
        _lease: &AiRunLease,
        _scope: &AiScope,
    ) -> Result<AiAgentRuleResolution, AiError> {
        let admits = self
            .admitting
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        let rules = if admits {
            self.rules.clone()
        } else {
            let mut constraints = self.rules.constraints().clone();
            constraints.allowed_tool_fingerprints = Some(BTreeSet::new());
            AiResolvedRuleSet::new(
                self.rules.target_scope().clone(),
                constraints,
                self.rules.applied_layers().to_vec(),
            )?
        };
        AiAgentRuleResolution::new(rules, OffsetDateTime::now_utc())
    }
}

fn fence(
    fixture: &Fixture,
    rules: Arc<RevocableRuleResolver>,
) -> crate::orm_tools::BrokeredRuleFence {
    crate::orm_tools::BrokeredRuleFence {
        rules,
        scope: fixture.scope.clone(),
    }
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
            fence(&fixture, RevocableRuleResolver::new(rules.clone())),
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
    Arc<RevocableRuleResolver>,
) {
    let mutation_id = fixture.automatic_mutation_id.clone().unwrap();
    let mutation_fingerprint = fixture.automatic_mutation_fingerprint.clone().unwrap();
    // One-shot supervised writes are within the ordinary test rules.
    let current_rules = test_rules(fixture.scope.clone());
    let rules = RevocableRuleResolver::new(current_rules.clone());
    let delivery = brokered_mutation_delivery(fixture, &current_rules);
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
    let service = service.with_rule_resolver(rules.clone());
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
                fence(fixture, rules.clone()),
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
    (service, requested, rules)
}

#[tokio::test]
async fn brokered_approval_required_mutation_keeps_its_approval_and_answers_the_broker_call() {
    let fixture = approval_required_mutation_fixture().await;
    let (service, requested, _) =
        approved_brokered_mutation(&fixture, "brokered-approval-call").await;
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
    let (service, requested, _) =
        approved_brokered_mutation(&fixture, "refused-approval-call").await;
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
    let (service, requested, _) =
        approved_brokered_mutation(&fixture, "tampered-approval-call").await;
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

/// Review round 2, item 1: a rule revoked after the coordinator admitted a
/// brokered automatic mutation, and before its dispatch, wins. The fence
/// runs after the pre-effect checkpoint, so the refusal is a durable
/// not-started outcome and nothing is dispatched.
#[tokio::test]
async fn a_rule_revoked_before_dispatch_refuses_the_brokered_automatic_mutation_without_effect() {
    let fixture = automatic_mutation_fixture().await;
    let rules = native_test_rules(fixture.scope.clone());
    let delivery = brokered_mutation_delivery(&fixture, &rules);
    let loaded_reference = load_mutation(&fixture, &delivery).await;
    let result = broker_execute_result(
        &fixture,
        &delivery,
        "revoked-mutation-call",
        &loaded_reference,
    );
    let service = automatic_mutation_service(&fixture, fixture.audit.clone());
    let resolved = service
        .resolve_brokered_mutation(&fixture.lease, &result, 0, &delivery)
        .await
        .unwrap()
        .unwrap();
    // The coordinator's admission resolves current rules once and admits;
    // the rule is revoked before the mutation reaches dispatch.
    let current = RevocableRuleResolver::admitting_for(rules.clone(), 1);
    let admitted = current
        .resolve_rules(&fixture.lease, &fixture.scope)
        .await
        .unwrap();
    assert_eq!(
        admitted.rules().constrain_tool(
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
            fence(&fixture, current),
        )
        .await
        .expect("a durable refusal");
    assert_eq!(fixture.completed_executions.load(Ordering::SeqCst), 0);
    let persisted = outcome.persisted().expect("refusal is a known outcome");
    assert_eq!(
        persisted.state(),
        AiApplicationToolCallState::ExecutionFailed
    );
    let Some(ModelInputBlock::ToolResult {
        output, tool_id, ..
    }) = persisted.model_input()
    else {
        panic!("refusal should answer the broker call");
    };
    assert_eq!(tool_id, AI_CAPABILITY_EXECUTE_TOOL_ID);
    assert_eq!(
        output["code"], "not_started_authorization_denied",
        "{output}"
    );
    let call = AiToolCallRecord::find_by_id(&fixture.database, &persisted.id().0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.state, "execution_failed");
    assert_eq!(
        call.authorization_code.as_deref(),
        Some("not_started_authorization_denied")
    );
}

/// Review round 2, item 2: public approved execution re-proves current rules
/// for a broker-routed approval and refuses once they no longer admit it.
#[tokio::test]
async fn approved_execution_refuses_a_brokered_mutation_whose_rules_no_longer_admit_it() {
    let fixture = approval_required_mutation_fixture().await;
    let (service, requested, rules) =
        approved_brokered_mutation(&fixture, "rule-revoked-approval-call").await;
    rules.revoke_after(0);
    assert!(matches!(
        service
            .execute_approved(
                requested.lease(),
                requested.approval_id(),
                requested.tool_call_id(),
                automatic_mutation_route(),
            )
            .await,
        Err(AiError::ReauthorizationFailed)
    ));
    assert_eq!(fixture.completed_executions.load(Ordering::SeqCst), 0);
    // Nothing was consumed: the approval stays approved and unused.
    let approval = AiApprovalRecord::find_by_id(&fixture.database, &requested.approval_id().0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(approval.consumed_uses, 0);
}

/// A rule revoked after the approval is consumed and before dispatch still
/// wins: the consumed approval runs nothing and answers not-started.
#[tokio::test]
async fn a_rule_revoked_between_consumption_and_dispatch_runs_nothing() {
    let fixture = approval_required_mutation_fixture().await;
    let (service, requested, rules) =
        approved_brokered_mutation(&fixture, "dispatch-revoked-approval-call").await;
    // The pre-consumption proof admits; the pre-dispatch proof sees the
    // revocation.
    rules.revoke_after(1);
    let outcome = service
        .execute_approved(
            requested.lease(),
            requested.approval_id(),
            requested.tool_call_id(),
            automatic_mutation_route(),
        )
        .await
        .expect("a durable refusal");
    assert_eq!(fixture.completed_executions.load(Ordering::SeqCst), 0);
    let persisted = outcome.persisted().expect("refusal is a known outcome");
    let Some(ModelInputBlock::ToolResult {
        output,
        call_id,
        tool_id,
    }) = persisted.model_input()
    else {
        panic!("refusal should answer the broker call");
    };
    assert_eq!(call_id, "dispatch-revoked-approval-call");
    assert_eq!(tool_id, AI_CAPABILITY_EXECUTE_TOOL_ID);
    assert_eq!(
        output["code"], "not_started_authorization_denied",
        "{output}"
    );
}

/// Without a rule resolver the service refuses every broker-routed approval.
#[tokio::test]
async fn approved_execution_without_a_rule_resolver_refuses_brokered_approvals() {
    let fixture = approval_required_mutation_fixture().await;
    let (_, requested, _) = approved_brokered_mutation(&fixture, "unresolved-approval-call").await;
    let (without_rules, _) = consequential_test_service(&fixture);
    assert!(matches!(
        without_rules
            .execute_approved(
                requested.lease(),
                requested.approval_id(),
                requested.tool_call_id(),
                automatic_mutation_route(),
            )
            .await,
        Err(AiError::Forbidden)
    ));
    assert_eq!(fixture.completed_executions.load(Ordering::SeqCst), 0);
}

/// Review round 2, item 4: the row's protected evidence records that it was
/// broker-routed, so clearing the clear provider-facing columns never makes
/// it a direct row.
#[tokio::test]
async fn a_brokered_row_cannot_be_downgraded_to_a_direct_one() {
    let fixture = approval_required_mutation_fixture().await;
    let (service, requested, _) =
        approved_brokered_mutation(&fixture, "downgraded-approval-call").await;
    AiToolCallRecord::update_by_id(
        &fixture.database,
        &requested.tool_call_id().0,
        UpdateAiToolCallRecordInput {
            provider_tool_id: Some(None),
            provider_tool_fingerprint: Some(None),
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

/// The protected evidence retains the exact offered broker fingerprint and
/// the provider's original argument hash, beside the resolved plan hash.
#[tokio::test]
async fn brokered_rows_retain_the_exact_provider_request_as_protected_evidence() {
    let fixture = automatic_mutation_fixture().await;
    let rules = native_test_rules(fixture.scope.clone());
    let delivery = brokered_mutation_delivery(&fixture, &rules);
    let loaded_reference = load_mutation(&fixture, &delivery).await;
    let result = broker_execute_result(
        &fixture,
        &delivery,
        "evidence-mutation-call",
        &loaded_reference,
    );
    let service = automatic_mutation_service(&fixture, fixture.audit.clone());
    let resolved = service
        .resolve_brokered_mutation(&fixture.lease, &result, 0, &delivery)
        .await
        .unwrap()
        .unwrap();
    let plan = resolved.arguments.clone();
    let outcome = service
        .execute_brokered_automatic_mutation(
            &fixture.lease,
            &result,
            automatic_mutation_context(&fixture),
            automatic_mutation_route(),
            resolved,
            fence(&fixture, RevocableRuleResolver::new(rules.clone())),
        )
        .await
        .unwrap();
    let persisted = outcome.persisted().unwrap();
    let call = AiToolCallRecord::find_by_id(&fixture.database, &persisted.id().0)
        .await
        .unwrap()
        .unwrap();
    let ProtectedContentEnvelope::DatabaseManaged { value } =
        serde_json::from_value(call.protected_arguments.clone().unwrap()).unwrap()
    else {
        panic!("fixture protection mode")
    };
    let requested = &result.tool_calls()[0];
    assert_eq!(
        value["format"],
        crate::orm_tools::BROKERED_CALL_EVIDENCE_FORMAT
    );
    assert_eq!(
        value["providerToolFingerprint"],
        requested.tool_fingerprint()
    );
    assert_eq!(
        value["providerArgumentHash"],
        crate::canonical_json_hash(requested.arguments())
    );
    assert_eq!(value["ruleFingerprint"], rules.fingerprint());
    assert_eq!(value["plan"], plan);
    assert_eq!(call.argument_hash, crate::canonical_json_hash(&plan));
    // The evidence matches only the exact requested call.
    assert!(
        crate::orm_tools::protected_arguments_match(
            &call,
            requested.tool_fingerprint(),
            requested.arguments(),
            &value
        )
        .unwrap()
    );
    let mut altered = requested.arguments().clone();
    altered["selections"] = json!(["recordId"]);
    assert!(
        !crate::orm_tools::protected_arguments_match(
            &call,
            requested.tool_fingerprint(),
            &altered,
            &value
        )
        .unwrap()
    );
    assert!(
        !crate::orm_tools::protected_arguments_match(
            &call,
            &"f".repeat(64),
            requested.arguments(),
            &value
        )
        .unwrap()
    );
    let mut downgraded = call.clone();
    downgraded.provider_tool_id = None;
    downgraded.provider_tool_fingerprint = None;
    assert!(crate::orm_tools::open_call_arguments(&downgraded, value.clone()).is_err());
    let mut forged = call.clone();
    forged.provider_tool_fingerprint = Some("e".repeat(64));
    assert!(crate::orm_tools::open_call_arguments(&forged, value).is_err());
}

/// Review round 3, item 3: only the clear dual-identity columns claim
/// broker origin. Direct arguments that collide with the envelope marker
/// stay ordinary arguments; columns added to a direct row are refused.
#[test]
fn only_the_dual_identity_columns_claim_broker_origin() {
    let collision = json!({
        "format": crate::orm_tools::BROKERED_CALL_EVIDENCE_FORMAT,
        "providerToolId": AI_CAPABILITY_EXECUTE_TOOL_ID,
        "recordId": "record-1"
    });
    let direct = AiToolCallRecord {
        id: Uuid::new_v4(),
        run_id: Uuid::new_v4(),
        provider_call_key: "legacy-key".to_owned(),
        provider_call_id: "legacy-call".to_owned(),
        provider_kind: None,
        provider_model: None,
        provider_response_id: None,
        budget_reservation_id: None,
        provider_turn_index: 0,
        tool_call_index: 0,
        tool_id: "records.update".to_owned(),
        tool_fingerprint: "a".repeat(64),
        provider_tool_id: None,
        provider_tool_fingerprint: None,
        execution_provenance: None,
        protected_arguments: None,
        argument_hash: crate::canonical_json_hash(&collision),
        protected_result: None,
        protected_result_source: None,
        payload_purged_at: None,
        risk: "high_impact".to_owned(),
        authorization_code: None,
        authorization_policy_version: None,
        authorization_state_digest: None,
        disclosure_schema_fingerprint: None,
        result_classification: None,
        result_egress_decision_id: None,
        result_egress_manifest_hash: None,
        application_audit_ref: None,
        approval_id: None,
        idempotency_key: None,
        correlation_id: None,
        causation_id: None,
        delegation_reference: None,
        lease_generation: 1,
        state: "waiting_approval".to_owned(),
        created_at: 1,
        completed_at: None,
        row_version: 0,
    };
    // A legacy direct row keeps its colliding arguments verbatim.
    let (arguments, evidence) =
        crate::orm_tools::open_call_arguments(&direct, collision.clone()).unwrap();
    assert_eq!(arguments, collision);
    assert!(evidence.is_none());
    assert!(
        crate::orm_tools::protected_arguments_match(
            &direct,
            &"a".repeat(64),
            &collision,
            &collision
        )
        .unwrap()
    );
    // Columns added to a direct row: its arguments are no envelope.
    let mut upgraded = direct.clone();
    upgraded.provider_tool_id = Some(AI_CAPABILITY_EXECUTE_TOOL_ID.to_owned());
    upgraded.provider_tool_fingerprint =
        Some(crate::capability_delivery::fixed_broker_execute_fingerprint().to_owned());
    assert!(crate::orm_tools::open_call_arguments(&upgraded, collision).is_err());
}

/// Review round 3, items 2 and 3: the approval binds the broker origin, so
/// erasing it from the durable call (columns and envelope, which
/// database-managed protection stores as readable JSON) refuses the
/// approval instead of running the mutation as a direct call.
#[tokio::test]
async fn erasing_the_broker_origin_refuses_the_approval_instead_of_running_it_as_direct() {
    let fixture = approval_required_mutation_fixture().await;
    let (service, requested, _) =
        approved_brokered_mutation(&fixture, "erased-origin-approval-call").await;
    let call = AiToolCallRecord::find_by_id(&fixture.database, &requested.tool_call_id().0)
        .await
        .unwrap()
        .unwrap();
    let ProtectedContentEnvelope::DatabaseManaged { value } =
        serde_json::from_value(call.protected_arguments.clone().unwrap()).unwrap()
    else {
        panic!("fixture protection mode")
    };
    // Rewrite the row as a plausible direct call: bare plan, matching hash,
    // no provider-facing columns.
    let plan = value["plan"].clone();
    assert_eq!(call.argument_hash, crate::canonical_json_hash(&plan));
    AiToolCallRecord::update_by_id(
        &fixture.database,
        &call.id,
        UpdateAiToolCallRecordInput {
            provider_tool_id: Some(None),
            provider_tool_fingerprint: Some(None),
            protected_arguments: Some(Some(
                serde_json::to_value(ProtectedContentEnvelope::DatabaseManaged { value: plan })
                    .unwrap(),
            )),
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

/// Review round 3, item 3: columns added to a direct approval's call never
/// make it a broker-routed one.
#[tokio::test]
async fn adding_broker_columns_to_a_direct_approval_refuses_it() {
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
        .unwrap();
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
        .unwrap();
    AiToolCallRecord::update_by_id(
        &fixture.database,
        &requested.tool_call_id().0,
        UpdateAiToolCallRecordInput {
            provider_tool_id: Some(Some(AI_CAPABILITY_EXECUTE_TOOL_ID.to_owned())),
            provider_tool_fingerprint: Some(Some(
                crate::capability_delivery::fixed_broker_execute_fingerprint().to_owned(),
            )),
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
