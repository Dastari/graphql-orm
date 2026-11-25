//! Entity references on every executor dispatch path.
//!
//! Each test drives the real entry points (`AiProviderCallExecutor::execute`,
//! `execute_with_provider_session`, the checkpoint service, run recovery and
//! the background-submission service) and asserts on what the registered
//! provider received, never on a helper that resolves a request.

use super::*;

#[cfg(any(
    feature = "local-harness",
    feature = "provider-grok-acp",
    feature = "provider-codex-app-server"
))]
#[path = "entity_reference_local_transport_tests.rs"]
mod local_transport;

/// A field value the host hook returns while access holds.
const FIELD: &str = "field value readable before revocation";

/// Resolves `record:*` while `access` is set and counts every call.
struct Hook {
    access: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl crate::AiEntityReferenceResolver for Hook {
    async fn resolve(
        &self,
        _principal: &AuthPrincipal,
        _scope: &AiScope,
        _reference: &crate::AiEntityReferenceInput,
    ) -> Result<Option<crate::AiResolvedEntityReference>, AiError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .access
            .load(Ordering::SeqCst)
            .then(|| crate::AiResolvedEntityReference::new(json!({"owner_note": FIELD}))))
    }
}

struct References {
    fixture: Fixture,
    access: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
    registry: crate::AiEntityReferenceRegistry,
}

fn reference() -> crate::AiEntityReferenceInput {
    crate::AiEntityReferenceInput {
        kind: "record".to_owned(),
        id: "r-1".to_owned(),
    }
}

fn registry_with_limits(
    access: &Arc<AtomicBool>,
    calls: &Arc<AtomicUsize>,
    limits: crate::AiEntityReferenceLimits,
) -> crate::AiEntityReferenceRegistry {
    crate::AiEntityReferenceRegistry::new(
        ["record"],
        Arc::new(Hook {
            access: access.clone(),
            calls: calls.clone(),
        }),
    )
    .expect("registry should validate")
    .with_limits(limits)
}

/// A fixture whose runtime resolves references and whose provider records
/// what it was sent.
async fn references_with(mock: MockProvider) -> References {
    references_for(mock, None).await
}

/// The same fixture with `provider` registered in place of the mock, for the
/// adapters that deliver a request to a local process.
async fn references_for(
    mock: MockProvider,
    provider: Option<Arc<dyn crate::AiProvider>>,
) -> References {
    let access = Arc::new(AtomicBool::new(true));
    let calls = Arc::new(AtomicUsize::new(0));
    // Small bounds: the fixture's deployment boundary admits 16 KiB a call.
    let registry = registry_with_limits(
        &access,
        &calls,
        crate::AiEntityReferenceLimits::new(8, 64, 512, 2_048).expect("limits should validate"),
    );
    let mut fixture = fixture_with_optional_provider(
        mock.with_wire_capture(),
        Arc::new(AllowAccess),
        true,
        provider,
    )
    .await;
    Arc::get_mut(&mut fixture.runtime)
        .expect("the fixture owns its runtime")
        .install_entity_references_for_test(registry.clone());
    References {
        fixture,
        access,
        calls,
        registry,
    }
}

async fn references(batches: impl IntoIterator<Item = Vec<ProviderEvent>>) -> References {
    references_with(MockProvider::new(Vec::new()).with_event_batches(batches)).await
}

fn text_turn() -> Vec<ProviderEvent> {
    vec![
        ProviderEvent::ResponseStarted { response_id: None },
        ProviderEvent::TextDelta {
            text: "answered".to_owned(),
        },
        ProviderEvent::Usage {
            input_tokens: 9,
            output_tokens: 2,
            cached_input_tokens: 0,
        },
        ProviderEvent::ResponseCompleted { response_id: None },
    ]
}

fn tool_turn() -> Vec<ProviderEvent> {
    vec![
        ProviderEvent::ResponseStarted { response_id: None },
        ProviderEvent::ToolCallStarted {
            call_id: "reference-call-1".to_owned(),
            tool_id: "records.read".to_owned(),
        },
        ProviderEvent::ToolCallCompleted {
            call_id: "reference-call-1".to_owned(),
            arguments: json!({"recordId": "54"}),
        },
        ProviderEvent::Usage {
            input_tokens: 18,
            output_tokens: 5,
            cached_input_tokens: 0,
        },
        ProviderEvent::ResponseCompleted { response_id: None },
    ]
}

fn executor(fixture: &Fixture) -> AiProviderCallExecutor {
    AiProviderCallExecutor::new(
        fixture.runtime.clone(),
        fixture.budget_service.clone(),
        fixture.audit.clone(),
        Arc::new(TestUsageAccounting),
        Arc::new(SystemClock),
        AiProviderCallLimits::new(64, 8_192, 64 * 1_024)
            .expect("test provider limits should validate"),
    )
}

/// Adds the reference to a plan as the handle block a host builds, and seals
/// the plan's inference manifest over the handle-only request.
fn with_reference(
    mut plan: AiProviderCallPlan,
    registry: &crate::AiEntityReferenceRegistry,
) -> AiProviderCallPlan {
    plan.request
        .input
        .push(registry.model_input_block(&reference()));
    plan.transfers[0].estimated_bytes = plan.request.conservative_egress_bytes();
    plan
}

fn attempt(mut plan: AiProviderCallPlan, fixture: &Fixture, number: u32) -> AiProviderCallPlan {
    plan.budget.idempotency_key = format!("provider:{}:{number}", fixture.lease.attempt_id());
    plan
}

fn assert_resolved(wire: &str) {
    assert!(wire.contains(FIELD), "the provider received the value");
    assert!(wire.contains(r#""status":"resolved""#));
    assert!(wire.contains(crate::AI_ENTITY_REFERENCE_PROVENANCE));
    assert!(!wire.contains("reserved_bytes"));
}

fn assert_unavailable(wire: &str) {
    assert!(
        !wire.contains(FIELD),
        "a revoked value reached the provider"
    );
    assert!(!wire.contains("owner_note"));
    assert!(wire.contains(r#""status":"unavailable""#));
    assert!(wire.contains(r#""id":"r-1""#));
    assert!(!wire.contains(r#""status":"handle""#));
}

fn assert_handles_only(what: &str, text: &str) {
    assert!(!text.contains(FIELD), "{what} holds a resolved value");
    assert!(!text.contains("owner_note"), "{what} holds resolved data");
}

/// Review regression: a cloned plan retried after revocation.
///
/// The plan is cloned before the first call. The first call resolves the
/// reference and sends the value. Access is revoked and the clone is
/// executed. The executor resolves again, so the clone sends the marker.
#[tokio::test]
async fn a_cloned_plan_retried_after_revocation_sends_the_marker() {
    let References {
        fixture,
        access,
        calls,
        registry,
    } = references([text_turn(), text_turn()]).await;
    let executor = executor(&fixture);
    let first = with_reference(plan(&fixture), &registry);
    let retry = attempt(first.clone(), &fixture, 2);

    // A plan can be cloned, logged and stored: it holds the handle only.
    assert_handles_only("the plan", &format!("{first:?}"));
    assert!(
        serde_json::to_string(&first.request)
            .expect("request serializes")
            .contains(r#""status":"handle""#)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0, "sealing resolves nothing");

    let result = executor
        .execute(&fixture.lease, first)
        .await
        .expect("the first call should complete");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_resolved(&fixture.mock.captured_wire()[0]);
    // What the call returns holds the handle, in `Debug` too.
    assert_handles_only("the call result", &format!("{result:?}"));
    assert_handles_only("the mock provider", &format!("{:?}", fixture.mock));

    access.store(false, Ordering::SeqCst);
    executor
        .execute(&fixture.lease, retry)
        .await
        .expect("the retry should complete");
    assert_eq!(calls.load(Ordering::SeqCst), 2, "the retry resolved again");
    let wire = fixture.mock.captured_wire();
    assert_eq!(wire.len(), 2);
    assert_unavailable(&wire[1]);
}

/// A caller cannot smuggle resolved data past the executor inside a request:
/// an envelope that already carries data is reduced to its handle when the
/// plan is sealed, and resolved again at dispatch.
#[tokio::test]
async fn a_request_that_arrives_resolved_is_reduced_to_its_handle() {
    let References {
        fixture,
        access,
        registry,
        ..
    } = references([text_turn()]).await;
    access.store(false, Ordering::SeqCst);
    let mut base = with_reference(plan(&fixture), &registry);
    let stale = ModelInputBlock::Json {
        value: json!({
            "type": crate::AI_ENTITY_REFERENCE_MODEL_BLOCK_TYPE,
            "status": "resolved",
            "reference": {"kind": "record", "id": "r-1"},
            "reserved_bytes": 512,
            "data": {"owner_note": FIELD},
        }),
    };
    *base.request.input.last_mut().expect("reference block") = stale;
    let sealed = AiProviderCallPlan::new(
        base.provider_kind.clone(),
        base.request.clone(),
        base.budget.clone(),
        base.transfers.clone(),
        "stale-resolved-request",
    )
    .expect("the plan should seal");
    assert_handles_only("the sealed plan", &format!("{sealed:?}"));

    executor(&fixture)
        .execute(&fixture.lease, sealed)
        .await
        .expect("the call should complete");
    assert_unavailable(&fixture.mock.captured_wire()[0]);
}

/// Without a registry on the runtime there is no hook to ask, so every
/// reference is sent as the same marker.
#[tokio::test]
async fn without_a_registry_every_reference_is_the_marker() {
    let access = Arc::new(AtomicBool::new(true));
    let calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_with_limits(
        &access,
        &calls,
        crate::AiEntityReferenceLimits::new(8, 64, 512, 2_048).expect("limits should validate"),
    );
    let fixture = fixture_with_provider(
        MockProvider::new(Vec::new())
            .with_event_batches([text_turn()])
            .with_wire_capture(),
    )
    .await;
    executor(&fixture)
        .execute(&fixture.lease, with_reference(plan(&fixture), &registry))
        .await
        .expect("the call should complete");
    assert_unavailable(&fixture.mock.captured_wire()[0]);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

/// The sealed estimate reserves the bytes a reference may resolve to, and the
/// dispatch step never exceeds the reservation.
#[tokio::test]
async fn the_sealed_estimate_reserves_resolved_bytes_and_refuses_what_does_not_fit() {
    let References {
        fixture,
        access,
        calls,
        registry,
    } = references([text_turn(), text_turn()]).await;
    let bare = plan(&fixture);
    let reserved = with_reference(plan(&fixture), &registry);
    let handle_bytes = serde_json::to_vec(reserved.request.input.last().expect("handle"))
        .expect("handle serializes")
        .len() as u64;
    // The handle, its 64-byte block overhead, the bytes reserved for its data
    // and the fixed envelope reservation are all in the sealed figure.
    assert_eq!(
        reserved.request.conservative_egress_bytes(),
        bare.request.conservative_egress_bytes()
            + handle_bytes
            + 64
            + 512
            + crate::AI_ENTITY_REFERENCE_ENVELOPE_RESERVED_BYTES
    );
    let sealed = reserved
        .transfers
        .iter()
        .find(|manifest| manifest.capability == AiEgressCapability::ModelInference)
        .expect("inference manifest")
        .estimated_bytes;
    assert_eq!(sealed, reserved.request.conservative_egress_bytes());

    // What is sent for the block fits the bytes sealed for it.
    executor(&fixture)
        .execute(&fixture.lease, reserved)
        .await
        .expect("the call should complete");
    let wire = fixture.mock.captured_wire();
    assert_resolved(&wire[0]);
    let sent = wire[0]
        .lines()
        .find(|line| line.contains(FIELD))
        .expect("resolved block");
    let embedded = serde_json::to_string(sent).expect("string encodes").len() as u64;
    assert!(
        embedded <= handle_bytes + 512 + crate::AI_ENTITY_REFERENCE_ENVELOPE_RESERVED_BYTES,
        "the resolved block exceeded its sealed reservation"
    );

    // A handle that reserved fewer bytes than its data needs is not sent
    // resolved, even though the principal may read it.
    let tight = registry_with_limits(
        &access,
        &calls,
        crate::AiEntityReferenceLimits::new(8, 64, 16, 2_048).expect("limits should validate"),
    );
    executor(&fixture)
        .execute(
            &fixture.lease,
            attempt(with_reference(plan(&fixture), &tight), &fixture, 2),
        )
        .await
        .expect("the call should complete");
    assert_unavailable(&fixture.mock.captured_wire()[1]);

    // A reservation the deployment's egress boundary cannot admit refuses the
    // call before transport: the default limits reserve 16 KiB a reference.
    let wide = registry_with_limits(&access, &calls, crate::AiEntityReferenceLimits::default());
    let refused = executor(&fixture)
        .execute(
            &fixture.lease,
            attempt(with_reference(plan(&fixture), &wide), &fixture, 3),
        )
        .await;
    assert!(
        refused.is_err(),
        "an over-large reservation must be refused"
    );
    assert_eq!(fixture.mock.captured_wire().len(), 2, "nothing was sent");
}

struct ToolLoop {
    references: References,
    checkpoint_service: OrmAiCoordinatorCheckpointService,
    continuation: crate::AiAgentContinuation,
    batch_lease: AiRunLease,
}

/// Turn one with a reference, a tool request, the tool's execution and both
/// checkpoints, all through the real services.
async fn tool_loop(batches: impl IntoIterator<Item = Vec<ProviderEvent>>) -> ToolLoop {
    let references = references(batches).await;
    let mock = references.fixture.mock.clone();
    tool_loop_on(references, |plan| plan, &move || mock.captured_wire()).await
}

/// [`tool_loop`] over any registered provider: `adapt` turns the fixture's
/// plan into one for that provider and `wire` returns what it has been sent.
async fn tool_loop_on(
    references: References,
    adapt: fn(AiProviderCallPlan) -> AiProviderCallPlan,
    wire: &dyn Fn() -> Vec<String>,
) -> ToolLoop {
    let fixture = &references.fixture;
    let first = adapt(with_reference(
        stateless_tool_plan(fixture),
        &references.registry,
    ));
    let result = executor(fixture)
        .execute(&fixture.lease, first)
        .await
        .expect("turn one should normalize");
    assert_resolved(&wire()[0]);
    assert_handles_only("the call result", &format!("{result:?}"));
    assert_handles_only(
        "the next continuation",
        &serde_json::to_string(&result.next_continuation().expect("history builds"))
            .expect("continuation serializes"),
    );

    let mut guard = AiAgentLoopGuard::new(
        &fixture.lease,
        AiAgentLoopLimits::new(4, 4).expect("test loop limits should validate"),
    );
    guard
        .observe_provider_turn(&result)
        .expect("turn one should bind");
    let route = AiToolResultEgressRoute::new(
        "mock-profile",
        "local-mock",
        AiDestinationTrust::Local,
        "continue_authorized_tool_result",
        "none",
        "egress-v1",
    )
    .expect("tool-result route should validate");
    let checkpoint_service = OrmAiCoordinatorCheckpointService::new(
        fixture.run_service.clone(),
        Arc::new(Resolver(fixture.principal.clone())),
        Arc::new(AllowAccess),
        Arc::new(ProtectionPolicy),
        Arc::new(DatabaseManagedContentProtector),
        Arc::new(TestRuleResolver::default()),
        Arc::new(SystemClock),
        AiCoordinatorCheckpointLimits::new(256 * 1_024, Duration::seconds(30))
            .expect("checkpoint limits should validate"),
    );
    let (rules, provider_rule_usage) = test_rule_checkpoint(&fixture.scope, &[&result], 0);
    let lease = checkpoint_service
        .persist_provider_turn(
            &fixture.lease,
            &result,
            &fixture.scope,
            "reference-tool-loop",
            &route,
            &rules,
            provider_rule_usage,
            guard.provider_turns(),
            guard.total_tool_calls(),
        )
        .await
        .expect("provider turn should checkpoint");
    let tool_service = OrmAiApplicationToolCallService::new(
        fixture.run_service.clone(),
        fixture.runtime.clone(),
        fixture.audit.clone(),
        Arc::new(SystemClock),
        AiApplicationToolCallLimits::new(
            8_192,
            16_384,
            4,
            4,
            Duration::seconds(30),
            Duration::seconds(10),
        )
        .expect("test tool limits should validate"),
    );
    let persisted = tool_service
        .execute_read_only(
            &lease,
            &result,
            AiApplicationToolCallContext::new(
                0,
                0,
                fixture.scope.clone(),
                "reference-tool-loop",
                "reference-provider-turn-1",
            )
            .expect("tool context should validate"),
            route.clone(),
        )
        .await
        .expect("tool should execute through the resolver");
    guard
        .observe_tool_result(&persisted)
        .expect("tool output should match the turn");
    let continuation = guard
        .continuation()
        .expect("complete batch should continue");
    let (_, batch_rule_usage) = test_rule_checkpoint(&fixture.scope, &[&result], 1);
    let batch_lease = checkpoint_service
        .persist_tool_batch(
            persisted.lease(),
            &result,
            std::slice::from_ref(&persisted),
            &continuation,
            &fixture.scope,
            "reference-tool-loop",
            &route,
            &rules,
            batch_rule_usage,
            guard.provider_turns(),
            guard.total_tool_calls(),
        )
        .await
        .expect("tool batch should checkpoint");
    // A continuation is cloned, parked and stored: handles only, in `Debug`.
    assert_handles_only("the continuation", &format!("{continuation:?}"));
    assert_handles_only(
        "the stored continuation",
        &continuation.checkpoint_value().to_string(),
    );
    ToolLoop {
        references,
        checkpoint_service,
        continuation,
        batch_lease,
    }
}

/// The provider plan of the turn that sends the tool result back.
fn continuation_plan(
    fixture: &Fixture,
    continuation: crate::AiAgentContinuation,
    number: u32,
) -> AiProviderCallPlan {
    continuation_plan_on(fixture, continuation, number, |plan| plan)
}

/// [`continuation_plan`] for any registered provider: `adapt` turns the
/// fixture's base plan into one for that provider before the plan is sealed.
fn continuation_plan_on(
    fixture: &Fixture,
    continuation: crate::AiAgentContinuation,
    number: u32,
    adapt: fn(AiProviderCallPlan) -> AiProviderCallPlan,
) -> AiProviderCallPlan {
    let descriptor = fixture
        .runtime
        .tool_catalog()
        .descriptor(&AiToolId::parse("records.read").expect("tool ID should parse"))
        .expect("tool should remain registered");
    let mut policy = AiToolPolicySet::new(ToolMaturity::ReadOnly);
    policy.bind(AiToolPolicyBinding {
        tool_id: descriptor.id.clone(),
        fingerprint: descriptor.fingerprint.clone(),
        enabled: true,
    });
    let base = adapt(attempt(plan(fixture), fixture, number));
    AiProviderCallPlan::new_continuation_with_read_capabilities(
        base.provider_kind,
        ModelRequest {
            model: "mock-model".to_owned(),
            instructions: Vec::new(),
            input: Vec::new(),
            continuation: None,
            continuation_mode: ModelContinuationMode::StatelessReplay,
            tools: mixed_read_definitions(fixture),
            builtin_tools: Vec::new(),
            maximum_builtin_tool_calls: None,
            reasoning_summary: crate::ModelReasoningSummaryRequest::Disabled,
            reasoning_effort: crate::ModelReasoningEffort::Unspecified,
            output_schema: None,
            maximum_output_tokens: Some(100),
        },
        base.budget,
        base.transfers,
        "reference-continuation",
        continuation,
        fixture.runtime.tool_catalog(),
        &policy,
        &fixture.generated_target_policy,
    )
    .expect("continuation plan should seal")
}

async fn stored_checkpoints(fixture: &Fixture) -> String {
    let checkpoints = AiRunCheckpointRecord::query(fixture.database.pool())
        .limit(50)
        .fetch_all()
        .await
        .expect("checkpoints should load");
    assert!(checkpoints.len() >= 2);
    serde_json::to_string(&checkpoints).expect("checkpoints serialize")
}

/// Revoke, then continue with the tool result.
///
/// The continuation replays turn one's user message. While access holds, the
/// executor resolves the replayed handle again. After revocation the same
/// continuation sends the marker. No persisted checkpoint ever holds the
/// value.
#[tokio::test]
async fn a_tool_continuation_after_revocation_sends_the_marker_and_checkpoints_hold_handles() {
    let ToolLoop {
        references,
        continuation,
        batch_lease,
        ..
    } = tool_loop([tool_turn(), text_turn(), text_turn()]).await;
    let References {
        mut fixture,
        access,
        calls,
        ..
    } = references;
    fixture.lease = batch_lease;

    // Persisted checkpoints hold the handle and nothing it resolved to.
    let stored = stored_checkpoints(&fixture).await;
    assert_handles_only("a persisted checkpoint", &stored);
    assert!(stored.contains(r#"\"status\":\"handle\""#) || stored.contains(r#""status":"handle""#));

    // The continuation plan is sealed over handles and reserves their bytes.
    let allowed = continuation_plan(&fixture, continuation.clone(), 2);
    assert_handles_only("the continuation plan", &format!("{allowed:?}"));
    let after_revocation = continuation_plan(&fixture, continuation, 3);
    let chain = |plan: &AiProviderCallPlan| {
        plan.request
            .continuation
            .as_ref()
            .expect("continuation is applied")
            .chain_reference(&plan.request.input)
    };
    assert_eq!(chain(&allowed), chain(&after_revocation));

    // While access holds, the replayed handle is resolved again.
    let executor = executor(&fixture);
    let resolutions = calls.load(Ordering::SeqCst);
    executor
        .execute(&fixture.lease, allowed)
        .await
        .expect("the continuation should complete");
    assert_eq!(calls.load(Ordering::SeqCst), resolutions + 1);
    assert_resolved(&fixture.mock.captured_wire()[1]);

    // Access is revoked before the tool result continues the run.
    access.store(false, Ordering::SeqCst);
    let result = executor
        .execute(&fixture.lease, after_revocation)
        .await
        .expect("the continuation should complete");
    let wire = fixture.mock.captured_wire();
    assert_eq!(wire.len(), 3);
    assert_unavailable(&wire[2]);
    assert_handles_only("the continuation result", &format!("{result:?}"));
    assert_handles_only(
        "a persisted checkpoint",
        &stored_checkpoints(&fixture).await,
    );
}

/// Resume and recovery after revocation.
///
/// The run's lease expires after the tool batch is checkpointed. Recovery
/// requeues the run, a new worker claims it under a new fence and adopts the
/// stored checkpoint. Access was revoked in between. The resumed run's next
/// provider request carries the marker.
#[tokio::test]
async fn a_recovered_run_resumed_after_revocation_sends_the_marker() {
    let ToolLoop {
        references,
        checkpoint_service,
        batch_lease,
        ..
    } = tool_loop([tool_turn(), text_turn()]).await;
    let References {
        mut fixture,
        access,
        ..
    } = references;

    // Same-fence resume: adopting the stored checkpoint yields handles.
    let resumed = checkpoint_service
        .adopt_tool_batch(&batch_lease)
        .await
        .expect("the stored batch should validate")
        .expect("the stored batch should be adoptable");
    assert_handles_only("an adopted checkpoint", &format!("{resumed:?}"));
    assert_handles_only(
        "an adopted continuation",
        &resumed.continuation().checkpoint_value().to_string(),
    );

    access.store(false, Ordering::SeqCst);

    // The lease expires; recovery requeues the run from its checkpoint.
    fixture
        .database
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                let run = tx
                    .find_by_id::<AiRunRecord>(&batch_lease.run_id().0)
                    .await
                    .map_err(OrmPublicError::from)?
                    .ok_or_else(OrmPublicError::not_found)?;
                let outcome = tx
                    .compare_and_swap::<AiRunRecord>(
                        &run.id,
                        run.row_version,
                        AiRunRecordWhereInput::default(),
                        UpdateAiRunRecordInput {
                            lease_expires_at: Some(Some(0)),
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(OrmPublicError::from)?;
                if !matches!(outcome, ConditionalUpdateOutcome::Updated(_)) {
                    return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                }
                Ok(())
            })
        })
        .await
        .expect("the lease should expire");
    let recovery = fixture
        .run_service
        .recover_expired_leases()
        .await
        .expect("the checkpoint should reconcile");
    assert_eq!(recovery.checkpoint_requeued, 1);
    let replacement = fixture
        .run_service
        .claim_next("reference-recovery-worker")
        .await
        .expect("replacement claim should succeed")
        .expect("the recovered run should be eligible");
    assert_eq!(replacement.lease_generation(), 2);
    fixture.lease = fixture
        .run_service
        .start(&replacement)
        .await
        .expect("the recovered attempt should start");
    let restored = checkpoint_service
        .adopt_tool_batch(&fixture.lease)
        .await
        .expect("the new fence should revalidate the history")
        .expect("the restored checkpoint should be adoptable");
    let continuation = restored.continuation().clone();
    assert_handles_only("a recovered continuation", &format!("{continuation:?}"));

    executor(&fixture)
        .execute(&fixture.lease, continuation_plan(&fixture, continuation, 1))
        .await
        .expect("the recovered run should continue");
    let wire = fixture.mock.captured_wire();
    assert_eq!(wire.len(), 2);
    assert_unavailable(&wire[1]);
    assert_handles_only(
        "a persisted checkpoint",
        &stored_checkpoints(&fixture).await,
    );
}

/// Streaming: live deltas are persisted while the provider streams. The
/// request the provider streams from carries the resolved value; nothing the
/// stream persists does.
#[tokio::test]
async fn a_streamed_turn_resolves_at_dispatch_and_persists_no_resolved_value() {
    let References {
        fixture,
        access,
        registry,
        ..
    } = references([text_turn(), text_turn()]).await;
    let live_sink = Arc::new(OrmAiLiveDeltaService::new(
        fixture.run_service.clone(),
        fixture.runtime.clone(),
        Arc::new(SystemClock),
        AiLiveDeltaPersistenceLimits::new(4_096, Duration::seconds(30))
            .expect("test live persistence limits should validate"),
    ));
    let executor = executor(&fixture).with_live_delta_sink(
        live_sink,
        AiLiveDeltaCoalescerLimits::new(std::time::Duration::from_millis(50), 4_096)
            .expect("test live coalescer limits should validate"),
    );
    let first = with_reference(plan(&fixture), &registry);
    let retry = attempt(first.clone(), &fixture, 2);
    let result = executor
        .execute(&fixture.lease, first)
        .await
        .expect("the streamed turn should complete");
    assert_resolved(&fixture.mock.captured_wire()[0]);
    assert_handles_only("the streamed result", &format!("{result:?}"));

    access.store(false, Ordering::SeqCst);
    executor
        .execute(&fixture.lease, retry)
        .await
        .expect("the streamed retry should complete");
    assert_unavailable(&fixture.mock.captured_wire()[1]);

    let events = fixture
        .database
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                tx.query::<AiSessionEventRecord>()
                    .limit(50)
                    .fetch_all()
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await
        .expect("session events should query");
    assert!(!events.is_empty(), "the stream persisted live events");
    assert_handles_only(
        "a persisted session event",
        &serde_json::to_string(&events).expect("events serialize"),
    );
}

/// Provider sessions: a turn bound to a retained provider session passes the
/// same dispatch step.
#[tokio::test]
async fn a_provider_session_turn_resolves_at_dispatch() {
    let cursor = AiProviderSessionCursor::new("mock.thread", "reference-thread")
        .expect("test cursor should validate");
    let References {
        fixture,
        calls,
        registry,
        ..
    } = references_with(
        MockProvider::new(Vec::new())
            .with_event_batches([text_turn()])
            .with_provider_session_cursor(cursor),
    )
    .await;
    let session = AiSessionRecord::find_by_id(&fixture.database, &fixture.lease.session_id().0)
        .await
        .expect("session lookup should succeed")
        .expect("session should exist");
    let update = AiSessionRecord::compare_and_swap(
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
    .expect("session watermark update should succeed");
    assert!(matches!(update, ConditionalUpdateOutcome::Updated(_)));
    AiMessageRecord::insert(
        &fixture.database,
        CreateAiMessageRecordInput {
            id: fixture.lease.input_message_id(),
            session_id: fixture.lease.session_id().0,
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
    .expect("input message should insert");
    let provider_sessions = Arc::new(
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
        .expect("provider-session service should validate"),
    );
    let descriptor = AiProviderSessionDescriptor::new(
        ProviderKind::OpenAiCompatible,
        "mock-profile",
        "mock-model",
        "a".repeat(64),
        "mock-retained/v1",
        "b".repeat(64),
    )
    .expect("descriptor should validate");
    let session_plan = AiProviderSessionTurnPlan::new(descriptor, "d".repeat(64))
        .expect("session plan should validate");
    let result = executor(&fixture)
        .execute_with_provider_session(
            Arc::new(Mutex::new(fixture.lease.clone())),
            with_reference(plan(&fixture), &registry),
            session_plan,
            provider_sessions,
            None,
        )
        .await
        .expect("the retained provider turn should succeed");
    assert!(result.provider_session_claim().is_some());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_resolved(&fixture.mock.captured_wire()[0]);
    assert_handles_only("the session turn result", &format!("{result:?}"));
}

/// Background submission is its own dispatch path and resolves there too.
#[cfg(feature = "provider-openai-core")]
#[tokio::test]
async fn a_background_submission_resolves_at_dispatch_and_stores_handles() {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let mock = MockProvider::new(Vec::new())
        .with_kind(ProviderKind::OpenAi)
        .with_capabilities(ProviderCapabilities {
            streaming: true,
            structured_output: true,
            background: true,
            provider_retained_continuation: true,
            local: true,
            ..ProviderCapabilities::default()
        })
        .with_background_submission("resp_background_reference_1", "queued", now);
    let References {
        fixture,
        calls,
        registry,
        ..
    } = references_with(mock).await;
    let service = OrmAiOpenAiBackgroundSubmissionService::new(
        fixture.run_service.clone(),
        fixture.runtime.clone(),
        fixture.budget_service.clone(),
        fixture.audit.clone(),
        Arc::new(SystemClock),
    );
    let mut submitted = background_plan(&fixture);
    submitted
        .request
        .input
        .push(registry.model_input_block(&reference()));
    let sealed = submitted.request.conservative_egress_bytes();
    for manifest in &mut submitted.transfers {
        manifest.estimated_bytes = sealed;
        manifest.estimated_tokens = sealed;
    }
    submitted.budget.estimate.input_tokens = sealed;
    assert_handles_only("the background plan", &format!("{submitted:?}"));

    let accepted = service
        .submit(&fixture.lease, submitted)
        .await
        .expect("the background submission should be accepted");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_resolved(&fixture.mock.captured_wire()[0]);
    assert_handles_only("the accepted submission", &format!("{accepted:?}"));
    let stored = AiProviderBackgroundSubmissionRecord::find_by_id(
        &fixture.database,
        &accepted.submission_id(),
    )
    .await
    .expect("submission lookup should succeed")
    .expect("submission should exist");
    assert_handles_only(
        "the stored submission",
        &serde_json::to_string(&stored).expect("submission serializes"),
    );
}

/// `Debug` of every value that holds resolved content prints none of it.
#[tokio::test]
async fn debug_output_of_resolved_values_is_redacted() {
    let access = Arc::new(AtomicBool::new(true));
    let calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_with_limits(
        &access,
        &calls,
        crate::AiEntityReferenceLimits::new(8, 64, 512, 2_048).expect("limits should validate"),
    );
    let principal = AuthPrincipal::User(agql_auth::AuthUser {
        user_id: "debug-user".to_owned(),
        session_id: Uuid::new_v4(),
        roles: Vec::new(),
        scopes: Vec::new(),
        session: agql_auth::SessionContext::default(),
        token_claims: agql_auth::AccessTokenMetadata::default(),
    });
    let scope = AiScope::new("test", "scope");

    let host_value = crate::AiResolvedEntityReference::new(json!({"owner_note": FIELD}));
    assert_handles_only("a host-resolved value", &format!("{host_value:?}"));

    let outcomes = registry
        .resolve_outcomes(&principal, &scope, &[reference()])
        .await;
    assert!(matches!(
        outcomes[0],
        crate::entity_references::AiEntityReferenceOutcome::Resolved { .. }
    ));
    assert_handles_only("a resolution outcome", &format!("{outcomes:?}"));

    let used = registry
        .resolve_for_use(&principal, &scope, &[reference()])
        .await;
    assert!(used[0].view_content().to_string().contains(FIELD));
    assert_handles_only("a transcript use", &format!("{used:?}"));

    let view = crate::AiMessageBlockView {
        id: Uuid::new_v4(),
        message_id: Uuid::new_v4(),
        block_index: 1,
        kind: crate::AI_ENTITY_REFERENCE_BLOCK_KIND.to_owned(),
        content: async_graphql::Json(used[0].view_content()),
        byte_count: 1,
        line_count: 1,
    };
    assert_handles_only("a transcript block view", &format!("{view:?}"));

    let request = ModelRequest {
        model: "mock-model".to_owned(),
        instructions: Vec::new(),
        input: vec![registry.model_input_block(&reference())],
        continuation: None,
        continuation_mode: ModelContinuationMode::ProviderRetained,
        tools: Vec::new(),
        builtin_tools: Vec::new(),
        maximum_builtin_tool_calls: None,
        reasoning_summary: crate::ModelReasoningSummaryRequest::Disabled,
        reasoning_effort: crate::ModelReasoningEffort::Unspecified,
        output_schema: None,
        maximum_output_tokens: Some(64),
    };
    let dispatch =
        crate::ModelDispatchRequest::resolve(Some(&registry), &principal, &scope, request).await;
    let ModelInputBlock::Json { value } = &dispatch.request().input[0] else {
        panic!("expected a JSON block");
    };
    // The dispatch value holds the resolved data and prints none of it.
    assert!(
        dispatch
            .references()
            .wire_json(value)
            .to_string()
            .contains(FIELD)
    );
    assert_handles_only("a dispatch request", &format!("{dispatch:?}"));
    assert_handles_only(
        "dispatch references",
        &format!("{:?}", dispatch.references()),
    );
    // Its request half is the handle, and discarding the resolution leaves
    // the marker.
    assert_handles_only(
        "the handle-only request",
        &serde_json::to_string(dispatch.request()).expect("request serializes"),
    );
    let withheld =
        serde_json::to_string(&dispatch.into_unavailable_request()).expect("request serializes");
    assert_handles_only("a withheld request", &withheld);
    assert!(withheld.contains(r#""status":"unavailable""#));
}

struct DirectProvider(MockProvider);

#[async_trait]
impl AiProvider for DirectProvider {
    fn provider_kind(&self) -> ProviderKind {
        self.0.provider_kind()
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.0.capabilities()
    }

    async fn stream(
        &self,
        dispatch: ModelDispatchRequest,
        context: ProviderRequestContext,
    ) -> Result<ProviderEventStream, ProviderError> {
        self.0.stream(dispatch, context).await
    }

    async fn stream_with_dynamic_tools(
        &self,
        dispatch: ModelDispatchRequest,
        context: ProviderRequestContext,
        _responder: Arc<dyn ProviderDynamicToolResponder>,
    ) -> Result<ProviderEventStream, ProviderError> {
        self.0.stream(dispatch, context).await
    }

    async fn submit_background(
        &self,
        dispatch: ModelDispatchRequest,
        context: ProviderRequestContext,
        binding: ProviderBackgroundBinding,
    ) -> Result<ProviderBackgroundSubmission, ProviderError> {
        self.0.submit_background(dispatch, context, binding).await
    }
}

struct NoDirectToolCalls;

#[async_trait]
impl ProviderDynamicToolResponder for NoDirectToolCalls {
    async fn respond(
        &self,
        _call: ProviderDynamicToolCall,
    ) -> Result<ProviderDynamicToolResult, ProviderError> {
        panic!("the direct-provider test never invokes an application tool")
    }
}

fn direct_context(plan: &AiProviderCallPlan) -> ProviderRequestContext {
    let budget = &plan.budget;
    let now = OffsetDateTime::now_utc();
    let proof = AiBudgetReservation::new_reserved_with_reasoning_effort(
        AiBudgetReservationId::new(),
        budget.run_id,
        budget.attempt_id,
        budget.lease_generation,
        budget.provider_kind.clone(),
        &budget.model,
        budget.reasoning_effort,
        &budget.pricing_policy_version,
        AiBudgetAmounts {
            input_tokens: plan.request.conservative_egress_bytes(),
            output_tokens: 100,
            runs: 1,
            ..Default::default()
        },
        budget.expires_at,
    )
    .unwrap()
    .authorize_provider_call_with_reasoning_effort(
        budget.run_id,
        budget.attempt_id,
        budget.lease_generation,
        &budget.provider_kind,
        &budget.model,
        budget.reasoning_effort,
        100,
        0,
        now,
    )
    .unwrap();
    let authorize = |manifest: &AiEgressManifest| {
        AiEgressDecision::allow(manifest, "test-policy", "test-user")
            .authorize(manifest)
            .unwrap()
    };
    let mut manifests = plan.transfers.iter();
    let manifest = manifests.next().unwrap();
    let mut context = ProviderRequestContext::new(
        budget.session_id,
        budget.run_id,
        &plan.correlation_id,
        proof,
        manifest.clone(),
        authorize(manifest),
    )
    .unwrap();
    for manifest in manifests {
        context = context
            .with_authorized_transfer(manifest.clone(), authorize(manifest))
            .unwrap();
    }
    context
}

async fn assert_direct_markers(fixture: &Fixture, plan: &AiProviderCallPlan) {
    let runtime = &fixture.runtime;
    let kind = &plan.provider_kind;
    let request = || plan.request.clone();
    let context = || direct_context(plan);
    let responder = Arc::new(NoDirectToolCalls);
    assert!(matches!(
        runtime.dispatch_provider(kind, request(), context()).await,
        AiProviderDispatchOutcome::Dispatched(_)
    ));
    let _ = runtime
        .stream_provider(kind, request(), context())
        .await
        .unwrap();
    assert!(matches!(
        runtime
            .dispatch_provider_with_dynamic_tools(kind, request(), context(), responder.clone())
            .await,
        AiProviderDispatchOutcome::Dispatched(_)
    ));
    let _ = runtime
        .stream_provider_with_dynamic_tools(kind, request(), context(), responder)
        .await
        .unwrap();
    let binding = ProviderBackgroundBinding::new(
        Uuid::new_v4(),
        "test-submission".into(),
        "mock-profile".into(),
    );
    let result = runtime
        .submit_provider_background(kind, request(), context(), binding)
        .await
        .unwrap();
    assert_handles_only("background acknowledgement", &format!("{result:?}"));
}

#[tokio::test]
async fn direct_provider_apis_keep_markers_on_retry_and_replayed_history() {
    let mock = MockProvider::new(text_turn())
        .with_capabilities(ProviderCapabilities {
            streaming: true,
            custom_tools: true,
            parallel_tool_calls: true,
            stateless_continuation: true,
            background: true,
            local: true,
            maximum_context_tokens: Some(1_000_000),
            maximum_output_tokens: Some(100),
            ..Default::default()
        })
        .with_event_batches(
            std::iter::once(tool_turn()).chain(std::iter::repeat_n(text_turn(), 12)),
        )
        .with_background_submission(
            "resp-direct",
            "queued",
            OffsetDateTime::now_utc().unix_timestamp(),
        )
        .with_wire_capture();
    let references =
        references_for(mock.clone(), Some(Arc::new(DirectProvider(mock.clone())))).await;
    let ToolLoop {
        mut references,
        continuation,
        batch_lease,
        ..
    } = tool_loop_on(references, |plan| plan, &|| mock.captured_wire()).await;
    references.fixture.lease = batch_lease;
    let initial = with_reference(plan(&references.fixture), &references.registry);
    let replay = continuation_plan(&references.fixture, continuation, 2);
    assert_direct_markers(&references.fixture, &initial).await;
    references.access.store(false, Ordering::SeqCst);
    assert_direct_markers(&references.fixture, &initial).await;
    assert_direct_markers(&references.fixture, &replay).await;
    assert_eq!(
        references.calls.load(Ordering::SeqCst),
        1,
        "only the executor resolved"
    );
    let wire = mock.captured_wire();
    assert_eq!(wire.len(), 16);
    for request in &wire[1..] {
        assert_unavailable(request);
    }
    assert_handles_only(
        "persisted checkpoints",
        &stored_checkpoints(&references.fixture).await,
    );
}
