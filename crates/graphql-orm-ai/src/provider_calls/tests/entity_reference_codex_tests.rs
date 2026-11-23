use super::*;
use crate::{
    AI_CODEX_APP_SERVER_PROTOCOL_V2, AiCodexAppServerBootstrapInstructions,
    AiCodexAppServerDispatchTurn, AiCodexAppServerLaunchProfile, AiCodexAppServerLaunchedProcess,
    AiCodexAppServerModelToolMode, AiCodexAppServerProtocolActor, AiCodexAppServerProvider,
    AiCodexAppServerRegistration, AiCodexAppServerRunLimits, AiCodexAppServerRunPool,
    AiCodexAppServerRunProcess, AiCodexAppServerRunProcessFactory, AiCodexAppServerTurnInput,
    AiOpenedProviderSession, AiProviderRunCloseReason, AiProviderSessionCursor,
    ModelReasoningEffort, ProviderDynamicToolResponder, ProviderEventStream,
};
use serde_json::Value;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::AsyncWrite;

const THREAD: &str = "codex-reference-test-thread";
const FRESH: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Default)]
struct Child {
    writes: std::sync::Mutex<Vec<Value>>,
    fail_before_write: AtomicBool,
    killed: AtomicUsize,
    shutdowns: AtomicUsize,
    frame_limit: Option<usize>,
    dynamic_tools: bool,
    stall_write: AtomicBool,
    partial_writes: AtomicUsize,
    delay_before_write: std::time::Duration,
}

struct StallingWriter<'a>(&'a Child);

impl AsyncWrite for StallingWriter<'_> {
    fn poll_write(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if bytes.len() > 1 && self.0.partial_writes.load(Ordering::SeqCst) == 0 {
            assert!(String::from_utf8_lossy(bytes).contains(FIELD));
            self.0.partial_writes.fetch_add(1, Ordering::SeqCst);
            Poll::Ready(Ok(1))
        } else {
            Poll::Pending
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        panic!("a partial frame must never flush")
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        panic!("an abandoned writer must never be shut down gracefully")
    }
}

struct Factory(Arc<Child>);

struct Process {
    child: Arc<Child>,
    actor: Mutex<AiCodexAppServerProtocolActor>,
}

#[async_trait]
impl AiCodexAppServerRunProcessFactory for Factory {
    fn supports_launch_profile(&self, _: AiCodexAppServerLaunchProfile) -> bool {
        true
    }

    async fn launch(
        &self,
        registration: Arc<AiCodexAppServerRegistration>,
    ) -> Result<AiCodexAppServerLaunchedProcess, ProviderError> {
        let mut actor = AiCodexAppServerProtocolActor::new(self.0.frame_limit.unwrap_or(4096))?;
        let initialize = if registration.experimental_dynamic_tools() {
            actor.initialize_with_dynamic_tools("fixture", "Fixture", "1.0.0")?
        } else {
            actor.initialize("fixture", "Fixture", "1.0.0")?
        };
        respond(&mut actor, &initialize, json!({}));
        let child = self.0.clone();
        Ok(AiCodexAppServerLaunchedProcess::new(
            Arc::new(Process {
                actor: Mutex::new(actor),
                child: child.clone(),
            }),
            Arc::new(move || {
                child.killed.fetch_add(1, Ordering::SeqCst);
            }),
        ))
    }
}

fn accept(actor: &mut AiCodexAppServerProtocolActor, frame: Value) {
    let inbound = actor
        .accept(&serde_json::to_vec(&frame).unwrap())
        .expect("the actor should accept the frame");
    assert_handles_only("the actor's projected event", &format!("{inbound:?}"));
    match inbound {
        crate::AiCodexAppServerInbound::Notification { params, .. } => {
            assert_handles_only("projected parameters", &params.to_string());
        }
        crate::AiCodexAppServerInbound::Response { result, .. } => {
            assert_handles_only("projected response", &result.to_string());
        }
        _ => {}
    }
}

fn respond(actor: &mut AiCodexAppServerProtocolActor, request: &[u8], result: Value) {
    let request: Value = serde_json::from_slice(request).unwrap();
    accept(actor, json!({"id": request["id"], "result": result}));
}

fn bind_thread(actor: &mut AiCodexAppServerProtocolActor, request: &[u8], thread: Value) {
    respond(actor, request, json!({"thread": thread}));
    accept(
        actor,
        json!({"method": "thread/started", "params": {"thread": thread}}),
    );
}

#[async_trait]
impl AiCodexAppServerRunProcess for Process {
    async fn create_empty_thread(
        &self,
        model: &str,
        effort: ModelReasoningEffort,
        bootstrap: &AiCodexAppServerBootstrapInstructions,
        tools: Vec<crate::ModelToolDefinition>,
    ) -> Result<AiProviderSessionCursor, ProviderError> {
        let mut actor = self.actor.lock().await;
        let frame = actor.start_persistent_empty_thread(model, effort, bootstrap, &tools)?;
        bind_thread(&mut actor, &frame, json!({"id": THREAD}));
        AiProviderSessionCursor::new("codex.app_server.thread.v2", THREAD)
            .map_err(|_| ProviderError::Rejected)
    }

    async fn start_fresh_turn(
        &self,
        _input: AiCodexAppServerTurnInput,
    ) -> Result<ProviderEventStream, ProviderError> {
        panic!("the pool must use the dispatch method")
    }

    async fn start_dispatch_turn(
        &self,
        dispatch: AiCodexAppServerDispatchTurn<'_>,
        session: Option<AiOpenedProviderSession>,
        responder: Option<Arc<dyn ProviderDynamicToolResponder>>,
    ) -> Result<ProviderEventStream, ProviderError> {
        assert_eq!(responder.is_some(), self.child.dynamic_tools);
        assert_handles_only("the dispatch debug", &format!("{dispatch:?}"));
        let cloned_input = dispatch.input().clone();
        assert_unavailable(&cloned_input.input().join("\n"));
        let mut actor = self.actor.lock().await;
        match session {
            None => {
                let frame = actor.start_fresh_thread(&cloned_input)?;
                bind_thread(&mut actor, &frame, json!({"id": THREAD}));
            }
            Some(session)
                if session.activation() == crate::AiProviderSessionActivation::ExistingRetained =>
            {
                let resume = actor.resume_thread(session.cursor(), &cloned_input)?;
                self.child
                    .writes
                    .lock()
                    .unwrap()
                    .push(serde_json::from_slice(&resume).unwrap());
                bind_thread(
                    &mut actor,
                    &resume,
                    json!({"id": THREAD, "preview": FIELD, "turns": [{
                        "id":"old-turn", "items":[{"type":"userMessage","id":"user-1","content":[{"type":"text","text":FIELD}]}]
                    }]}),
                );
            }
            Some(_) => {}
        }
        if self.child.fail_before_write.swap(false, Ordering::SeqCst) {
            return Err(ProviderError::Unavailable);
        }
        tokio::time::sleep(self.child.delay_before_write).await;
        if self.child.stall_write.swap(false, Ordering::SeqCst) {
            actor
                .write_dispatch_turn(THREAD, dispatch, &mut StallingWriter(&self.child))
                .await?;
            panic!("the partial write cannot complete");
        }
        let mut wire = Vec::new();
        actor
            .write_dispatch_turn(THREAD, dispatch, &mut wire)
            .await?;
        let frame: Value = serde_json::from_slice(&wire).unwrap();
        self.child.writes.lock().unwrap().push(frame.clone());
        assert!(actor.start_turn(THREAD, &cloned_input).is_err());

        let item =
            json!({"type":"userMessage", "id":"user-2", "content":[frame["params"]["input"][1]]});
        let turn = json!({"id":"turn-2", "items":[item]});
        respond(&mut actor, &wire, json!({"turn": turn}));
        accept(
            &mut actor,
            json!({"method":"turn/started","params":{"threadId":THREAD,"turn":turn}}),
        );
        for (method, timestamp) in [
            ("item/started", "startedAtMs"),
            ("item/completed", "completedAtMs"),
        ] {
            accept(
                &mut actor,
                json!({"method":method,"params":{
                    "threadId":THREAD,"turnId":"turn-2","item":item,(timestamp):1
                }}),
            );
        }
        accept(
            &mut actor,
            json!({"method":"turn/completed","params":{
                "threadId":THREAD,"turn":{"id":"turn-2","items":[item],"status":"completed"}
            }}),
        );
        assert_handles_only("the actor debug", &format!("{actor:?}"));
        Ok(Box::pin(futures::stream::iter(
            text_turn().into_iter().map(Ok),
        )))
    }

    async fn interrupt(&self) -> Result<(), ProviderError> {
        Ok(())
    }

    async fn shutdown(&self, _reason: AiProviderRunCloseReason) -> Result<(), ProviderError> {
        self.child.shutdowns.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

async fn codex(
    maximum_age: Option<std::time::Duration>,
    child: Arc<Child>,
) -> (References, Arc<AiCodexAppServerRegistration>) {
    let mut registration = AiCodexAppServerRegistration::new(
        "mock-profile",
        "mock-model",
        "a".repeat(64),
        "1.0.0",
        "test-wire-only",
        AI_CODEX_APP_SERVER_PROTOCOL_V2,
    )
    .unwrap();
    if let Some(age) = maximum_age {
        registration = registration
            .with_resolved_entity_reference_delivery(age)
            .unwrap();
    }
    if child.dynamic_tools {
        registration = registration.with_launch_profile(
            AiCodexAppServerLaunchProfile::experimental_dynamic_tools_only_v1(
                AiCodexAppServerModelToolMode::Direct,
            )
            .unwrap(),
        );
    }
    let registration = Arc::new(registration);
    let pool = AiCodexAppServerRunPool::new(
        Arc::new(Factory(child)),
        AiCodexAppServerRunLimits::default(),
    );
    let provider = AiCodexAppServerProvider::new(registration.clone(), pool);
    let references = references_for(MockProvider::new(Vec::new()), Some(Arc::new(provider))).await;
    (references, registration)
}

fn wire_text(frame: &Value) -> &str {
    assert_eq!(frame["method"], "turn/start");
    frame["params"]["input"][1]["text"].as_str().unwrap()
}

#[tokio::test]
async fn failed_codex_turn_cannot_send_its_resolution_on_retry() {
    let child = Arc::new(Child {
        fail_before_write: AtomicBool::new(true),
        ..Child::default()
    });
    let (references, _) = codex(Some(FRESH), child.clone()).await;
    let fixture = &references.fixture;
    let mut first = for_local_process(with_reference(plan(fixture), &references.registry));
    first.request.continuation_mode = ModelContinuationMode::StatelessReplay;
    let retry = attempt(first.clone(), fixture, 2);
    let executor = executor(fixture);
    assert!(executor.execute(&fixture.lease, first).await.is_err());
    assert_eq!(references.calls.load(Ordering::SeqCst), 1);
    assert_eq!(child.killed.load(Ordering::SeqCst), 1);
    assert!(child.writes.lock().unwrap().is_empty());

    references.access.store(false, Ordering::SeqCst);
    let result = executor.execute(&fixture.lease, retry).await.unwrap();
    assert_eq!(references.calls.load(Ordering::SeqCst), 2);
    let frames = child.writes.lock().unwrap().clone();
    assert_eq!(frames.len(), 1);
    assert_unavailable(wire_text(&frames[0]));
    assert_handles_only("result", &format!("{result:?}"));
    assert_handles_only("persisted state", &persisted_state(fixture).await);
}

#[tokio::test]
async fn codex_retained_turn_resolves_again_on_resume_after_revocation() {
    retained_resume_after_revocation(false).await;
}

#[tokio::test]
async fn codex_retained_dynamic_turn_resolves_again_on_resume_after_revocation() {
    retained_resume_after_revocation(true).await;
}

async fn retained_resume_after_revocation(dynamic_tools: bool) {
    let child = Arc::new(Child {
        dynamic_tools,
        ..Child::default()
    });
    let (references, registration) = codex(Some(FRESH), child.clone()).await;
    let fixture = &references.fixture;
    insert_retained_input_message(fixture).await;
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
    let descriptor = AiProviderSessionDescriptor::new(
        ProviderKind::LocalHarness,
        registration.provider_profile_id(),
        registration.logical_model(),
        registration
            .provider_session_fingerprint(ModelReasoningEffort::Unspecified)
            .unwrap(),
        registration.protocol_version(),
        "b".repeat(64),
    )
    .unwrap();
    let turn = AiProviderSessionTurnPlan::new(descriptor, "d".repeat(64)).unwrap();
    let lease = Arc::new(Mutex::new(fixture.lease.clone()));
    let base = if dynamic_tools {
        tool_plan(fixture)
    } else {
        plan(fixture)
    };
    let mut first = for_local_process(with_reference(base, &references.registry));
    first.request.instructions.clear();
    first.transfers[0].estimated_bytes = first.request.conservative_egress_bytes();
    let continuation = attempt(first.clone(), fixture, 2);
    let dynamic_execution: Option<Arc<dyn AiProviderDynamicToolExecution>> =
        dynamic_tools.then(|| {
            Arc::new(PersistingDynamicToolExecution {
                service: automatic_mutation_service(fixture, fixture.audit.clone()),
                scope: fixture.scope.clone(),
                calls: Arc::new(AtomicUsize::new(0)),
            }) as Arc<dyn AiProviderDynamicToolExecution>
        });
    let executor = executor(fixture);
    let result = executor
        .execute_with_provider_session(
            lease.clone(),
            first,
            turn.clone(),
            sessions.clone(),
            dynamic_execution.clone(),
        )
        .await
        .unwrap();
    assert_eq!(references.calls.load(Ordering::SeqCst), 1);
    let frames = child.writes.lock().unwrap().clone();
    assert_eq!(frames.len(), 1);
    assert_resolved(wire_text(&frames[0]));
    assert_handles_only("result", &format!("{result:?}"));
    assert_handles_only("persisted state", &persisted_state(fixture).await);

    let binding = crate::AiProviderRunBinding::from_lease(&*lease.lock().await).unwrap();
    fixture
        .runtime
        .close_provider_run(
            &ProviderKind::LocalHarness,
            &binding,
            AiProviderRunCloseReason::Parked,
        )
        .await
        .unwrap();
    references.access.store(false, Ordering::SeqCst);
    let result = executor
        .execute_with_provider_session(lease, continuation, turn, sessions, dynamic_execution)
        .await
        .unwrap();
    assert_eq!(references.calls.load(Ordering::SeqCst), 2);
    let frames = child.writes.lock().unwrap().clone();
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[1]["method"], "thread/resume");
    assert_eq!(frames[1]["params"]["threadId"], THREAD);
    assert_unavailable(wire_text(&frames[2]));
    assert_handles_only("continuation result", &format!("{result:?}"));
    assert_handles_only("persisted state", &persisted_state(fixture).await);
}

#[tokio::test]
async fn codex_defaults_to_markers_and_downgrades_expired_dispatches() {
    for age in [None, Some(std::time::Duration::from_millis(1))] {
        let child = Arc::new(Child {
            delay_before_write: std::time::Duration::from_millis(10),
            ..Child::default()
        });
        let (references, _) = codex(age, child.clone()).await;
        let fixture = &references.fixture;
        let mut plan = for_local_process(with_reference(plan(fixture), &references.registry));
        plan.request.continuation_mode = ModelContinuationMode::StatelessReplay;
        executor(fixture)
            .execute(&fixture.lease, plan)
            .await
            .unwrap();
        assert_eq!(references.calls.load(Ordering::SeqCst), 1);
        let frames = child.writes.lock().unwrap();
        assert_eq!(frames.len(), 1);
        assert_unavailable(wire_text(&frames[0]));
    }
}

#[tokio::test]
async fn codex_kills_the_process_if_a_resolved_write_stalls() {
    let child = Arc::new(Child {
        stall_write: AtomicBool::new(true),
        ..Child::default()
    });
    let (references, _) = codex(Some(std::time::Duration::from_secs(1)), child.clone()).await;
    let fixture = &references.fixture;
    let mut first = for_local_process(with_reference(plan(fixture), &references.registry));
    first.request.continuation_mode = ModelContinuationMode::StatelessReplay;
    let retry = attempt(first.clone(), fixture, 2);
    let executor = executor(fixture);
    assert!(executor.execute(&fixture.lease, first).await.is_err());
    assert_eq!(child.partial_writes.load(Ordering::SeqCst), 1);
    assert_eq!(child.killed.load(Ordering::SeqCst), 1);
    assert_eq!(child.shutdowns.load(Ordering::SeqCst), 0);
    assert!(child.writes.lock().unwrap().is_empty());

    references.access.store(false, Ordering::SeqCst);
    executor.execute(&fixture.lease, retry).await.unwrap();
    assert_eq!(references.calls.load(Ordering::SeqCst), 2);
    let frames = child.writes.lock().unwrap().clone();
    assert_eq!(frames.len(), 1);
    assert_unavailable(wire_text(&frames[0]));
    assert_handles_only("persisted state", &persisted_state(fixture).await);
}

#[tokio::test]
async fn codex_uses_the_marker_frame_when_only_it_fits() {
    let mut limit = None;
    for age in [None, Some(FRESH)] {
        let child = Arc::new(Child {
            frame_limit: limit,
            ..Child::default()
        });
        let (references, _) = codex(age, child.clone()).await;
        let fixture = &references.fixture;
        let mut plan = for_local_process(with_reference(plan(fixture), &references.registry));
        plan.request.continuation_mode = ModelContinuationMode::StatelessReplay;
        plan.request.input[0] = ModelInputBlock::Text {
            text: "a".repeat(2048),
        };
        plan.transfers[0].estimated_bytes = plan.request.conservative_egress_bytes();
        let result = executor(fixture).execute(&fixture.lease, plan).await;
        assert!(
            result.is_ok(),
            "the marker should fit with resolved delivery {age:?} and a frame limit of {limit:?}: {result:?}"
        );
        assert_eq!(references.calls.load(Ordering::SeqCst), 1);
        let frames = child.writes.lock().unwrap();
        assert_eq!(frames.len(), 1);
        assert_unavailable(wire_text(&frames[0]));
        limit = Some(frames[0].to_string().len() + 1);
    }
}
