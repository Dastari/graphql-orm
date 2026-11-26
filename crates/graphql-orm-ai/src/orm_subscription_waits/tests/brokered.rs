//! Generated subscriptions reached through the fixed capability broker.
//!
//! A broker-routed wait answers the provider's `graphql.capabilities.execute`
//! call, and observes the subscription that call resolved to. These tests
//! prove that both identities are durable, that the resolved subscription is
//! what rules and authority classify, and that a removed or replaced
//! subscription closes the wait as `capability_stale`.
use super::*;
use crate::{
    AI_CAPABILITY_EXECUTE_TOOL_ID, AiCapabilityBrokerOperation, AiCapabilityBrokerSession,
    AiCapabilityDeliveryLimits, AiCapabilityDeliveryMode, AiCapabilityDeliveryTurn,
    AiCapabilityDiscoveryBroker, AiCapabilityIndex, AiCapabilityIndexLimits, AiCapabilityIndexSet,
    AiCapabilityRunBinding, AiProviderCapabilitySessionBinding, ModelInputBlock,
    ModelReasoningEffort, ProviderCapabilities, ProviderKind,
};

struct WaitCurrentIndex(Arc<AiCapabilityIndex>);

impl crate::AiCurrentCapabilityIndex for WaitCurrentIndex {
    fn current_index(
        &self,
        _run: &AiCapabilityRunBinding,
    ) -> Result<Arc<AiCapabilityIndex>, AiError> {
        Ok(Arc::clone(&self.0))
    }
}

/// Host capability policy that admits every entry while `allowed` holds.
struct WaitBrokerAuthority(Arc<AtomicBool>);

#[async_trait]
impl crate::AiCapabilityAuthorityPolicy for WaitBrokerAuthority {
    async fn authorize(
        &self,
        _principal: &ResolvedPrincipal,
        _owning_index: &AiCapabilityIndex,
        _entry: &crate::AiCapabilityIndexEntry,
        _run: &AiCapabilityRunBinding,
    ) -> Result<crate::AiCapabilityAuthorityDecision, AiError> {
        Ok(crate::AiCapabilityAuthorityDecision {
            allowed: self.0.load(Ordering::SeqCst),
            policy_fingerprint: "wait-broker-policy-v1".to_owned(),
        })
    }
}

/// The fixture's rules, narrowed to exactly `fingerprints`.
fn rules_admitting(scope: &AiScope, fingerprints: BTreeSet<String>) -> crate::AiResolvedRuleSet {
    let base = test_rules(scope.clone());
    let mut constraints = base.constraints().clone();
    constraints.allowed_tool_fingerprints = Some(fingerprints);
    crate::AiResolvedRuleSet::new(scope.clone(), constraints, base.applied_layers().to_vec())
        .expect("narrowed rules")
}

/// The fixed broker's five definition fingerprints.
fn broker_fingerprints(delivery: &AiCapabilityDeliveryTurn) -> BTreeSet<String> {
    delivery
        .current_tools()
        .into_iter()
        .map(|definition| definition.fingerprint)
        .collect()
}

fn broker_execute_fingerprint() -> String {
    crate::capability_delivery::fixed_broker_execute_fingerprint().to_owned()
}

/// Execute arguments of one bounded wait for the `wanted` parent.
fn wait_arguments(loaded_reference: &str) -> Value {
    json!({
        "loadedReference": loaded_reference,
        "arguments": [{"name": "id", "value": "wanted"}],
        "selections": ["id", "name"],
        "observation": {
            "timeoutSeconds": 60,
            "maximumEvents": 2,
            "condition": {"field": "id", "operator": "equal", "value": "wanted"}
        }
    })
}

impl WaitFixture {
    /// A fixed-broker delivery over the fixture's subscription catalogue,
    /// compiled in process, with subscriptions brokered when `opted_in`.
    fn brokered_delivery(&self, opted_in: bool) -> AiCapabilityDeliveryTurn {
        self.delivery_over(opted_in, Arc::new(AtomicBool::new(true)))
    }

    fn delivery_over(&self, opted_in: bool, allowed: Arc<AtomicBool>) -> AiCapabilityDeliveryTurn {
        let index = Arc::new(
            AiCapabilityIndex::compile(
                self.target_id.clone(),
                self.capability_catalog.finished_schema_fingerprint(),
                &self.semantics,
                None,
                None,
                Some(&self.capability_catalog),
                [],
                "target-policy-v1",
                AiCapabilityIndexLimits::default(),
            )
            .expect("subscription capability index"),
        );
        assert!(index.subscriptions_checked_against_finished_schema());
        let index_set = AiCapabilityIndexSet::compile([index.clone()]).expect("index set");
        let broker = Arc::new(
            AiCapabilityDiscoveryBroker::new(
                Arc::new(TestPrincipalResolver {
                    principal: self.principal.clone(),
                    active: self.principal_active.clone(),
                    clock: self.clock.clone(),
                }),
                Arc::new(WaitCurrentIndex(index)),
                Arc::new(WaitBrokerAuthority(allowed)),
                self.clock.clone(),
                Duration::seconds(30),
            )
            .expect("broker"),
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
        .expect("session binding");
        let session = AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default())
            .expect("broker session");
        let session = if opted_in {
            session.with_brokered_subscriptions()
        } else {
            session
        };
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
        .expect("fixed-broker delivery")
    }

    /// Discovers and describes the fixture's subscription through the
    /// broker and returns its loaded reference.
    async fn load_subscription(
        &self,
        delivery: &AiCapabilityDeliveryTurn,
        lease: &AiRunLease,
    ) -> String {
        let run = AiCapabilityRunBinding::from_lease(
            lease,
            ProviderKind::OpenAi,
            delivery.session_binding(),
        );
        let discovery = delivery
            .broker()
            .dispatch_discover(
                lease.principal_reference(),
                &run,
                delivery.session(),
                &json!({
                    "text": "parent changed",
                    "kind": "generated_subscription",
                    "maximumResults": 4
                }),
            )
            .await
            .expect("subscription discovery");
        let candidate = &discovery["candidates"][0];
        assert_eq!(
            candidate["capabilityId"],
            self.capability_id.as_str(),
            "{discovery}"
        );
        assert_eq!(candidate["kind"], "generated_subscription");
        let description = delivery
            .broker()
            .dispatch_describe(
                lease.principal_reference(),
                &run,
                delivery.session(),
                &json!({
                    "capabilityId": candidate["capabilityId"],
                    "candidateReference": candidate["candidateReference"],
                }),
            )
            .await
            .expect("subscription description");
        assert_eq!(description.contract()["effect"], "wait");
        description.loaded_reference().to_owned()
    }

    /// One provider turn whose calls are broker executes of `loaded_reference`.
    fn broker_execute_result(
        &self,
        delivery: &AiCapabilityDeliveryTurn,
        lease: &AiRunLease,
        call_ids: &[&str],
        loaded_reference: &str,
    ) -> AiProviderCallResult {
        let execute = delivery
            .broker_definition(AiCapabilityBrokerOperation::Execute)
            .expect("execute definition");
        let mut result = AiProviderCallResult::test_result(
            lease,
            None,
            "brokered-subscription-response",
            call_ids
                .iter()
                .map(|call_id| {
                    (
                        *call_id,
                        AI_CAPABILITY_EXECUTE_TOOL_ID,
                        wait_arguments(loaded_reference),
                    )
                })
                .collect(),
        );
        for index in 0..call_ids.len() {
            result = result.test_with_tool_binding(
                index,
                execute.provider_name.clone(),
                execute.fingerprint.clone(),
            );
        }
        result.test_with_offered_tools(delivery.current_tools())
    }

    /// A direct subscription call of the fixture's capability.
    fn direct_result(&self, lease: &AiRunLease, call_ids: &[&str]) -> AiProviderCallResult {
        let plan = json!({
            "arguments": {"id": "wanted"},
            "fields": {"id": true, "name": true},
            "relationships": {},
            "condition": {"field": "id", "operator": "equal", "value": "wanted"},
            "timeoutSeconds": 60,
            "maximumEvents": 2
        });
        let mut result = AiProviderCallResult::test_result(
            lease,
            None,
            "direct-subscription-response",
            call_ids
                .iter()
                .map(|call_id| (*call_id, self.capability_id.as_str(), plan.clone()))
                .collect(),
        );
        for index in 0..call_ids.len() {
            result = result.test_with_tool_binding(
                index,
                self.provider_name.clone(),
                self.capability_fingerprint.clone(),
            );
        }
        result
    }

    /// A wait service over the same database, sources and target, with
    /// `catalog` as its registered subscriptions and `rules` as current
    /// rules.
    async fn service_with(
        &self,
        catalog: Option<&crate::AiGraphqlSubscriptionCapabilityCatalog>,
        rules: crate::AiResolvedRuleSet,
    ) -> Arc<OrmAiSubscriptionWaitService> {
        let runtime = subscription_runtime(
            &self.database,
            &self.principal,
            &self.principal_active,
            &self.access_allowed,
            &self.clock,
            &self.target_id,
            &self.capability_catalog,
            catalog,
        )
        .await;
        Arc::new(OrmAiSubscriptionWaitService::new(
            self.run_service.clone(),
            Arc::new(runtime),
            self.sources.clone(),
            Arc::new(TestRules(rules)),
            Arc::new(TestEgressAudit),
            self.clock.clone(),
            AiSubscriptionWaitLimits::default(),
        ))
    }

    /// The fixture's subscription recompiled under a lower event ceiling: the
    /// same root and source with a different capability fingerprint.
    fn replaced_catalog(&self) -> crate::AiGraphqlSubscriptionCapabilityCatalog {
        let replaced = crate::AiGraphqlSubscriptionCapabilityCatalog::compile(
            "subscription-test",
            self.target_id.clone(),
            WAIT_SDL,
            &self.semantics,
            crate::AiGraphqlSubscriptionCapabilityLimits {
                maximum_events: 10,
                ..Default::default()
            },
        )
        .expect("replaced subscription capabilities compile");
        let capability = replaced
            .capabilities()
            .next()
            .expect("replaced subscription capability");
        assert_eq!(capability.id(), &self.capability_id);
        assert_ne!(capability.fingerprint(), self.capability_fingerprint);
        replaced
    }

    /// Registers one brokered wait on a fresh run through `service`.
    async fn register_brokered(
        &self,
        service: &OrmAiSubscriptionWaitService,
        rules: &crate::AiResolvedRuleSet,
        call_id: &str,
    ) -> Result<AiRegisteredSubscriptionWait, AiError> {
        let delivery = self.brokered_delivery(true);
        let lease = self.start_lease().await;
        let loaded = self.load_subscription(&delivery, &lease).await;
        let result = self.broker_execute_result(&delivery, &lease, &[call_id], &loaded);
        self.register_result(
            service,
            rules,
            &lease,
            result,
            Some(delivery),
            test_rule_usage(),
        )
        .await
    }

    /// Claims and starts the fresh attempt of one queued wait outcome.
    async fn resume(&self) -> AiRunLease {
        let claimed = self
            .run_service
            .claim_next("brokered-resume-worker")
            .await
            .expect("fresh attempt claims")
            .expect("queued wait adoption exists");
        self.run_service
            .start(&claimed)
            .await
            .expect("fresh attempt starts")
    }
}

fn assert_stale(error: &AiError) {
    assert!(
        matches!(error, AiError::InvalidInput(message) if message == crate::AI_CAPABILITY_STALE),
        "{error:?}"
    );
}

#[tokio::test]
async fn sqlite_brokered_wait_keeps_both_identities_and_answers_the_broker_call() {
    let fixture = wait_fixture().await;
    let delivery = fixture.brokered_delivery(true);
    let lease = fixture.start_lease().await;
    let loaded = fixture.load_subscription(&delivery, &lease).await;
    assert!(
        delivery
            .session()
            .holds_loaded_subscription(&json!({"loadedReference": loaded}))
    );
    let result = fixture.broker_execute_result(&delivery, &lease, &["brokered-wait-call"], &loaded);
    let broker_arguments = result.tool_calls()[0].arguments().clone();
    let registered = fixture
        .register_result(
            &fixture.service,
            &fixture.rules,
            &lease,
            result,
            Some(delivery),
            test_rule_usage(),
        )
        .await
        .expect("brokered wait registers");

    // The row records the provider-facing broker call and the resolved
    // subscription: rules, authority and the plan use the resolved one.
    let call = AiToolCallRecord::find_by_id(&fixture.database, &registered.tool_call_id().0)
        .await
        .expect("tool call loads")
        .expect("tool call exists");
    assert_eq!(call.provider_call_id, "brokered-wait-call");
    assert_eq!(call.tool_id, fixture.capability_id.as_str());
    assert_eq!(call.tool_fingerprint, fixture.capability_fingerprint);
    assert_eq!(
        call.provider_tool_id.as_deref(),
        Some(AI_CAPABILITY_EXECUTE_TOOL_ID)
    );
    assert_eq!(
        call.provider_tool_fingerprint.as_deref(),
        Some(broker_execute_fingerprint().as_str())
    );
    assert_eq!(
        call.provider_facing_tool_id(),
        AI_CAPABILITY_EXECUTE_TOOL_ID
    );
    let protected = call
        .protected_arguments
        .clone()
        .expect("protected arguments");
    let (plan, evidence) = crate::orm_tools::open_call_arguments(&call, protected["value"].clone())
        .expect("brokered evidence opens");
    let evidence = evidence.expect("a brokered row keeps its broker evidence");
    assert_eq!(
        evidence.format,
        crate::orm_tools::BROKERED_CALL_EVIDENCE_FORMAT
    );
    assert_eq!(
        evidence.provider_tool_fingerprint,
        broker_execute_fingerprint()
    );
    assert_eq!(
        evidence.provider_argument_hash,
        canonical_json_hash(&broker_arguments).unwrap()
    );
    assert_eq!(evidence.rule_fingerprint, fixture.rules.fingerprint());
    assert_eq!(plan["timeoutSeconds"], 60);
    assert_eq!(plan["maximumEvents"], 2);
    assert_eq!(call.argument_hash, canonical_json_hash(&plan).unwrap());
    let waiter =
        AiSubscriptionWaiterRecord::find_by_id(&fixture.database, &registered.waiter_id().0)
            .await
            .expect("waiter loads")
            .expect("waiter exists");
    assert_eq!(
        waiter.capability_fingerprint,
        fixture.capability_fingerprint
    );
    assert_eq!(waiter.maximum_events, 2);

    // A non-matching event is examined under the resolved subscription; the
    // matching one completes the wait.
    fixture.source.push(fixture.event(1, "other"));
    assert!(matches!(
        fixture.service.process_next("brokered-wait-worker-a").await,
        Ok(AiSubscriptionWaitWorkerOutcome::Waiting { .. })
    ));
    fixture.source.push(fixture.event(2, "wanted"));
    assert!(matches!(
        fixture.service.process_next("brokered-wait-worker-b").await,
        Ok(AiSubscriptionWaitWorkerOutcome::Queued { .. })
    ));

    // The adopted result answers the provider's original broker call.
    let lease = fixture.resume().await;
    let control = AiSupervisedSubscriptionCheckpointControl::new(
        fixture.service.clone(),
        Arc::new(EmptySupervisedCheckpointControl),
    );
    let adopted = crate::AiAgentSupervisedCheckpointControl::adopt_classified(&control, &lease)
        .await
        .expect("brokered adoption validates")
        .expect("subscription checkpoint classifies");
    let crate::AiAdoptedClassifiedMutationBatch::Subscription(adopted) = adopted else {
        panic!("a wait checkpoint is never a mutation checkpoint");
    };
    let [
        ModelInputBlock::ToolResult {
            call_id,
            tool_id,
            output,
        },
    ] = adopted.continuation().input()
    else {
        panic!("one tool result answers the broker call");
    };
    assert_eq!(call_id, "brokered-wait-call");
    assert_eq!(tool_id, AI_CAPABILITY_EXECUTE_TOOL_ID);
    assert_eq!(output["data"]["ParentChanged"]["id"], "wanted", "{output}");
    let consumed =
        crate::AiAgentSupervisedCheckpointControl::consume_subscription(&control, &lease, &adopted)
            .await
            .expect("brokered wait consumes once");
    assert!(consumed.latest_checkpoint_id().is_none());
    let call = AiToolCallRecord::find_by_id(&fixture.database, &registered.tool_call_id().0)
        .await
        .expect("tool call loads")
        .expect("tool call exists");
    assert_eq!(call.state, "completed");
    assert_eq!(
        call.provider_tool_id.as_deref(),
        Some(AI_CAPABILITY_EXECUTE_TOOL_ID)
    );
}

#[tokio::test]
async fn brokered_subscriptions_need_the_opt_in_and_the_delivery() {
    let fixture = wait_fixture().await;
    let lease = fixture.start_lease().await;
    // A session that did not opt in neither discovers nor loads one.
    let closed = fixture.brokered_delivery(false);
    let run =
        AiCapabilityRunBinding::from_lease(&lease, ProviderKind::OpenAi, closed.session_binding());
    assert!(matches!(
        closed
            .broker()
            .dispatch_discover(
                lease.principal_reference(),
                &run,
                closed.session(),
                &json!({"text": "parent changed", "kind": "generated_subscription", "maximumResults": 4}),
            )
            .await,
        Err(AiError::InvalidInput(_))
    ));
    let unfiltered = closed
        .broker()
        .dispatch_discover(
            lease.principal_reference(),
            &run,
            closed.session(),
            &json!({"text": "parent changed", "maximumResults": 8}),
        )
        .await
        .expect("unfiltered discovery");
    assert!(
        unfiltered["candidates"]
            .as_array()
            .expect("candidates")
            .iter()
            .all(|candidate| candidate["kind"] != "generated_subscription"),
        "{unfiltered}"
    );

    // An opted-in session loads it, but registration refuses the broker
    // call without that delivery, or with a delivery that did not opt in.
    let open = fixture.brokered_delivery(true);
    let loaded = fixture.load_subscription(&open, &lease).await;
    assert!(
        !closed
            .session()
            .holds_loaded_subscription(&json!({"loadedReference": loaded}))
    );
    for delivery in [None, Some(closed.clone())] {
        let lease = fixture.start_lease().await;
        let loaded = fixture.load_subscription(&open, &lease).await;
        let result = fixture.broker_execute_result(&open, &lease, &["unrouted-wait-call"], &loaded);
        assert!(matches!(
            fixture
                .register_result(
                    &fixture.service,
                    &fixture.rules,
                    &lease,
                    result,
                    delivery,
                    test_rule_usage(),
                )
                .await,
            Err(AiError::Forbidden)
        ));
    }
    assert_eq!(
        AiSubscriptionWaiterRecord::query(fixture.database.pool())
            .fetch_all()
            .await
            .expect("waiters load")
            .len(),
        0
    );
}

#[tokio::test]
async fn brokered_waits_are_classified_by_the_resolved_subscription_fingerprint() {
    let fixture = wait_fixture().await;
    let delivery = fixture.brokered_delivery(true);
    // Rules that admit only the five broker definitions admit no
    // subscription: classification never uses the broker fingerprint.
    let broker_only = rules_admitting(&fixture.scope, broker_fingerprints(&delivery));
    let service = fixture
        .service_with(Some(&fixture.capability_catalog), broker_only.clone())
        .await;
    assert!(matches!(
        fixture
            .register_brokered(&service, &broker_only, "broker-only-call")
            .await,
        Err(AiError::Forbidden)
    ));

    // Rules that admit the subscription but not the broker execute
    // definition admit the direct call, and refuse the broker call the
    // provider actually made.
    let subscription_only = rules_admitting(
        &fixture.scope,
        BTreeSet::from([fixture.capability_fingerprint.clone()]),
    );
    let service = fixture
        .service_with(Some(&fixture.capability_catalog), subscription_only.clone())
        .await;
    assert!(matches!(
        fixture
            .register_brokered(&service, &subscription_only, "unadmitted-broker-call")
            .await,
        Err(AiError::Forbidden)
    ));
    let lease = fixture.start_lease().await;
    let direct = fixture.direct_result(&lease, &["direct-wait-call"]);
    fixture
        .register_result(
            &service,
            &subscription_only,
            &lease,
            direct,
            None,
            test_rule_usage(),
        )
        .await
        .expect("the direct subscription is admitted");

    // Rules that admit both register the brokered wait.
    let mut both = broker_fingerprints(&delivery);
    both.insert(fixture.capability_fingerprint.clone());
    let both = rules_admitting(&fixture.scope, both);
    let service = fixture
        .service_with(Some(&fixture.capability_catalog), both.clone())
        .await;
    fixture
        .register_brokered(&service, &both, "admitted-broker-call")
        .await
        .expect("rules admitting the resolved subscription register it");
}

#[tokio::test]
async fn brokered_waits_keep_the_direct_turn_and_run_limits() {
    let fixture = wait_fixture().await;
    // One wait per provider turn: a turn with two calls registers neither,
    // through the broker or directly.
    let delivery = fixture.brokered_delivery(true);
    let lease = fixture.start_lease().await;
    let loaded = fixture.load_subscription(&delivery, &lease).await;
    let two = fixture.broker_execute_result(
        &delivery,
        &lease,
        &["brokered-wait-a", "brokered-wait-b"],
        &loaded,
    );
    assert!(matches!(
        fixture
            .register_result(
                &fixture.service,
                &fixture.rules,
                &lease,
                two,
                Some(delivery.clone()),
                test_rule_usage(),
            )
            .await,
        Err(AiError::Conflict)
    ));
    let lease = fixture.start_lease().await;
    let two = fixture.direct_result(&lease, &["direct-wait-a", "direct-wait-b"]);
    assert!(matches!(
        fixture
            .register_result(
                &fixture.service,
                &fixture.rules,
                &lease,
                two,
                None,
                test_rule_usage(),
            )
            .await,
        Err(AiError::Conflict)
    ));

    // Rule run usage beyond the rules' step ceiling refuses both.
    let exhausted: AiRuleRunUsage = serde_json::from_value(json!({
        "startedAtUnix": TEST_NOW,
        "providerCalls": 1,
        "steps": 11,
        "outputTokens": 1,
        "costMicrounits": 0,
        "toolUnits": 0,
        "webSearchCalls": 0,
        "imageUnits": 0
    }))
    .expect("exhausted usage");
    let lease = fixture.start_lease().await;
    let loaded = fixture.load_subscription(&delivery, &lease).await;
    let brokered = fixture.broker_execute_result(&delivery, &lease, &["over-budget"], &loaded);
    let brokered = fixture
        .register_result(
            &fixture.service,
            &fixture.rules,
            &lease,
            brokered,
            Some(delivery.clone()),
            exhausted,
        )
        .await;
    let lease = fixture.start_lease().await;
    let direct = fixture.direct_result(&lease, &["over-budget-direct"]);
    let direct = fixture
        .register_result(
            &fixture.service,
            &fixture.rules,
            &lease,
            direct,
            None,
            exhausted,
        )
        .await;
    assert!(
        brokered.is_err() && direct.is_err(),
        "{brokered:?} {direct:?}"
    );
    assert_eq!(
        std::mem::discriminant(brokered.as_ref().unwrap_err()),
        std::mem::discriminant(direct.as_ref().unwrap_err())
    );

    // One wait per run: the parked run's lease cannot register another.
    let lease = fixture.start_lease().await;
    let loaded = fixture.load_subscription(&delivery, &lease).await;
    let first = fixture.broker_execute_result(&delivery, &lease, &["first-wait"], &loaded);
    fixture
        .register_result(
            &fixture.service,
            &fixture.rules,
            &lease,
            first,
            Some(delivery.clone()),
            test_rule_usage(),
        )
        .await
        .expect("the first wait registers");
    let loaded = fixture.load_subscription(&delivery, &lease).await;
    let second = fixture.broker_execute_result(&delivery, &lease, &["second-wait"], &loaded);
    assert!(
        fixture
            .register_result(
                &fixture.service,
                &fixture.rules,
                &lease,
                second,
                Some(delivery),
                test_rule_usage(),
            )
            .await
            .is_err()
    );
    assert_eq!(
        AiSubscriptionWaiterRecord::query(fixture.database.pool())
            .fetch_all()
            .await
            .expect("waiters load")
            .len(),
        1
    );
}

#[tokio::test]
async fn sqlite_a_replaced_or_removed_subscription_closes_the_wait_as_capability_stale() {
    let fixture = wait_fixture().await;
    let replaced = fixture.replaced_catalog();
    let cases = [
        ("replaced-brokered", true, Some(&replaced)),
        ("removed-brokered", true, None),
        ("replaced-direct", false, Some(&replaced)),
    ];
    for (call_id, brokered, catalog) in cases {
        let registered = if brokered {
            fixture
                .register_brokered(&fixture.service, &fixture.rules, call_id)
                .await
        } else {
            let lease = fixture.start_lease().await;
            let result = fixture.direct_result(&lease, &[call_id]);
            fixture
                .register_result(
                    &fixture.service,
                    &fixture.rules,
                    &lease,
                    result,
                    None,
                    test_rule_usage(),
                )
                .await
        }
        .expect("wait registers against the current subscription");
        let stale = fixture.service_with(catalog, fixture.rules.clone()).await;
        fixture.source.push(fixture.event(1, "wanted"));
        assert!(
            matches!(
                stale.process_next("stale-wait-worker").await,
                Ok(AiSubscriptionWaitWorkerOutcome::Closed { waiter_id, .. })
                    if waiter_id == registered.waiter_id()
            ),
            "{call_id}"
        );
        // Drop the unconsumed event so the next case starts clean.
        fixture
            .source
            .items
            .lock()
            .expect("source queue lock")
            .clear();
        let run = AiRunRecord::find_by_id(&fixture.database, &registered.run_id().0)
            .await
            .expect("run loads")
            .expect("run exists");
        assert_eq!(run.state, AiRunState::Failed.as_str(), "{call_id}");
        assert_eq!(
            run.error_code.as_deref(),
            Some("capability_stale"),
            "{call_id}"
        );
        let call = AiToolCallRecord::find_by_id(&fixture.database, &registered.tool_call_id().0)
            .await
            .expect("tool call loads")
            .expect("tool call exists");
        assert_eq!(call.authorization_code.as_deref(), Some("capability_stale"));
        assert_eq!(
            AiSubscriptionWaitAdoptionRecord::query(fixture.database.pool())
                .fetch_all()
                .await
                .expect("adoptions load")
                .len(),
            0,
            "{call_id}: nothing is delivered"
        );
    }
}

#[tokio::test]
async fn sqlite_a_subscription_replaced_before_resumption_is_stale_and_delivers_nothing() {
    let fixture = wait_fixture().await;
    fixture
        .register_brokered(&fixture.service, &fixture.rules, "resume-stale-call")
        .await
        .expect("brokered wait registers");
    fixture.source.push(fixture.event(1, "wanted"));
    assert!(matches!(
        fixture.service.process_next("resume-stale-worker").await,
        Ok(AiSubscriptionWaitWorkerOutcome::Queued { .. })
    ));
    let lease = fixture.resume().await;
    let replaced = fixture.replaced_catalog();
    let stale = fixture
        .service_with(Some(&replaced), fixture.rules.clone())
        .await;
    let control = AiSupervisedSubscriptionCheckpointControl::new(
        stale,
        Arc::new(EmptySupervisedCheckpointControl),
    );
    let error = crate::AiAgentSupervisedCheckpointControl::adopt_classified(&control, &lease)
        .await
        .expect_err("a replaced subscription is never resumed");
    assert_stale(&error);
    let adoptions = AiSubscriptionWaitAdoptionRecord::query(fixture.database.pool())
        .fetch_all()
        .await
        .expect("adoptions load");
    assert_eq!(adoptions.len(), 1);
    assert_eq!(adoptions[0].state, "queued", "the adoption was not claimed");
}

#[tokio::test]
async fn sqlite_a_brokered_wait_row_cannot_be_downgraded_to_a_direct_one() {
    let fixture = wait_fixture().await;
    let registered = fixture
        .register_brokered(&fixture.service, &fixture.rules, "downgraded-wait-call")
        .await
        .expect("brokered wait registers");
    AiToolCallRecord::update_by_id(
        &fixture.database,
        &registered.tool_call_id().0,
        UpdateAiToolCallRecordInput {
            provider_tool_id: Some(None),
            provider_tool_fingerprint: Some(None),
            ..Default::default()
        },
    )
    .await
    .expect("columns clear");
    fixture.source.push(fixture.event(1, "wanted"));
    assert!(matches!(
        fixture.service.process_next("downgraded-wait-worker").await,
        Err(AiError::Conflict)
    ));
    assert_eq!(
        AiSubscriptionWaitAdoptionRecord::query(fixture.database.pool())
            .fetch_all()
            .await
            .expect("adoptions load")
            .len(),
        0
    );
}

#[tokio::test]
async fn sqlite_the_read_broker_path_describes_a_subscription_and_never_runs_one() {
    let fixture = wait_fixture().await;
    let delivery = fixture.brokered_delivery(true);
    let lease = fixture.start_lease().await;
    let run = AiCapabilityRunBinding::from_lease(
        &lease,
        ProviderKind::OpenAi,
        delivery.session_binding(),
    );
    let discovery = delivery
        .broker()
        .dispatch_discover(
            lease.principal_reference(),
            &run,
            delivery.session(),
            &json!({"text": "parent changed", "kind": "generated_subscription", "maximumResults": 4}),
        )
        .await
        .expect("subscription discovery");
    let candidate = &discovery["candidates"][0];
    let runtime = subscription_runtime(
        &fixture.database,
        &fixture.principal,
        &fixture.principal_active,
        &fixture.access_allowed,
        &fixture.clock,
        &fixture.target_id,
        &fixture.capability_catalog,
        Some(&fixture.capability_catalog),
    )
    .await;
    let tools = crate::OrmAiApplicationToolCallService::new(
        fixture.run_service.clone(),
        Arc::new(runtime),
        Arc::new(TestEgressAudit),
        fixture.clock.clone(),
        crate::AiApplicationToolCallLimits::new(
            8_192,
            65_536,
            4,
            4,
            Duration::seconds(30),
            Duration::seconds(10),
        )
        .expect("tool limits"),
    );
    let route = AiToolResultEgressRoute::new(
        "test-profile",
        "local-test",
        crate::AiDestinationTrust::Local,
        "continue_broker_call",
        "none",
        "egress-v1",
    )
    .expect("egress route");
    let broker_call = |lease: &AiRunLease,
                       operation: AiCapabilityBrokerOperation,
                       call_id: &str,
                       arguments: Value| {
        let definition = delivery
            .broker_definition(operation)
            .expect("broker definition");
        AiProviderCallResult::test_result(
            lease,
            None,
            "read-broker-response",
            vec![(call_id, operation.tool_id(), arguments)],
        )
        .test_with_tool_binding(
            0,
            definition.provider_name.clone(),
            definition.fingerprint.clone(),
        )
        .test_with_offered_tools(delivery.current_tools())
    };
    let context = |turn: u32| {
        crate::AiApplicationToolCallContext::new(
            turn,
            0,
            fixture.scope.clone(),
            "read-broker-correlation",
            "read-broker-response",
        )
        .expect("tool context")
    };

    // Describe returns the compact signature and the observation bounds.
    let describe = broker_call(
        &lease,
        AiCapabilityBrokerOperation::Describe,
        "describe-call",
        json!({
            "capabilityId": candidate["capabilityId"],
            "candidateReference": candidate["candidateReference"],
        }),
    );
    let described = tools
        .execute_capability_broker_call(&lease, &describe, context(0), route.clone(), &delivery)
        .await
        .expect("describe persists");
    let Some(ModelInputBlock::ToolResult { output, .. }) = described.model_input() else {
        panic!("describe answers the broker call");
    };
    let rendered = output.to_string();
    assert!(rendered.contains("ParentChanged("), "{rendered}");
    assert!(rendered.contains(r#""effect":"wait""#), "{rendered}");
    assert!(
        rendered.contains(r#""maximumTimeoutSeconds":120"#),
        "{rendered}"
    );
    assert!(rendered.contains(r#""maximumEvents":20"#), "{rendered}");
    assert!(rendered.contains(r#""operators":["equal"]"#), "{rendered}");
    let loaded_reference = delivery
        .session()
        .loaded_bindings()
        .first()
        .expect("describe loaded the subscription")
        .audit_reference();
    assert!(rendered.contains(&loaded_reference), "{rendered}");
    let lease = described.into_lease();

    // Execute on the read path waits for nothing and runs nothing: the
    // fixed correction answers the broker call.
    let execute = broker_call(
        &lease,
        AiCapabilityBrokerOperation::Execute,
        "unrouted-execute-call",
        wait_arguments(&loaded_reference),
    );
    let refused = tools
        .execute_capability_broker_call(&lease, &execute, context(1), route, &delivery)
        .await
        .expect("a durable refusal");
    assert_eq!(
        refused.state(),
        crate::AiApplicationToolCallState::ExecutionFailed
    );
    let Some(ModelInputBlock::ToolResult {
        call_id,
        tool_id,
        output,
    }) = refused.model_input()
    else {
        panic!("the refusal answers the broker call");
    };
    assert_eq!(call_id, "unrouted-execute-call");
    assert_eq!(tool_id, AI_CAPABILITY_EXECUTE_TOOL_ID);
    assert!(
        output["correction"]
            .as_str()
            .is_some_and(|text| text.contains("cannot wait for events")),
        "{output}"
    );
    assert_eq!(
        AiSubscriptionWaiterRecord::query(fixture.database.pool())
            .fetch_all()
            .await
            .expect("waiters load")
            .len(),
        0
    );
}
