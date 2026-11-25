use super::*;
use crate::providers::{
    AI_CODEX_APP_SERVER_PROTOCOL_V2, AiCodexAppServerBootstrapInstructions,
    AiCodexAppServerLaunchProfile, AiCodexAppServerLaunchedProcess, AiCodexAppServerModelToolMode,
    AiCodexAppServerProtocolActor, AiCodexAppServerProvider, AiCodexAppServerRegistration,
    AiCodexAppServerRunLimits, AiCodexAppServerRunPool, AiCodexAppServerRunProcess,
    AiCodexAppServerRunProcessFactory, AiCodexAppServerTurnDispatch, AiCodexAppServerWriter,
};
use serde_json::Value;
use std::{pin::Pin, sync::Mutex as StdMutex};

const THREAD: &str = "reference-thread";
const TURN: &str = "reference-turn";
const CURSOR_KIND: &str = "codex.app_server.thread.v2";
const FRESH: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Default)]
struct WireLog {
    launches: AtomicUsize,
    kills: AtomicUsize,
    shutdowns: AtomicUsize,
    fail_before_write: AtomicBool,
    stall_write: AtomicBool,
    partial_writes: AtomicUsize,
    frames: StdMutex<Vec<(usize, Value)>>,
}

impl WireLog {
    fn turns(&self) -> Vec<String> {
        self.frames
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, frame)| frame["method"] == "turn/start")
            .map(|(_, frame)| Self::input(frame))
            .collect()
    }

    fn input(frame: &Value) -> String {
        frame["params"]["input"]
            .as_array()
            .unwrap()
            .iter()
            .map(|block| block["text"].as_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

struct Transcript {
    log: Arc<WireLog>,
    process: usize,
}

impl tokio::io::AsyncWrite for Transcript {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        if self.log.stall_write.load(Ordering::SeqCst) {
            if self.log.partial_writes.load(Ordering::SeqCst) > 0 {
                return std::task::Poll::Pending;
            }
            if String::from_utf8_lossy(bytes).contains(FIELD) {
                self.log.partial_writes.fetch_add(1, Ordering::SeqCst);
                return std::task::Poll::Ready(Ok(1));
            }
        }
        let frame = serde_json::from_slice(bytes).expect("a complete protocol frame");
        self.log.frames.lock().unwrap().push((self.process, frame));
        std::task::Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

struct Factory {
    log: Arc<WireLog>,
    frame_ceiling: usize,
    queue_past_deadline: bool,
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
        let process = self.log.launches.fetch_add(1, Ordering::SeqCst);
        let log = self.log.clone();
        let writer = AiCodexAppServerWriter::new(Transcript {
            log: log.clone(),
            process,
        });
        let mut actor = AiCodexAppServerProtocolActor::new(self.frame_ceiling)?;
        let frame = if registration.experimental_dynamic_tools() {
            actor.initialize_with_dynamic_tools("reference-test", "Reference Test", "1.0.0")?
        } else {
            actor.initialize("reference-test", "Reference Test", "1.0.0")?
        };
        writer.write_frame(&frame).await?;
        actor.accept(br#"{"id":1,"result":{"userAgent":"fixture"}}"#)?;
        writer.write_frame(&actor.initialized()?).await?;
        Ok(AiCodexAppServerLaunchedProcess::new(
            Arc::new(Process {
                actor: Mutex::new(actor),
                log: log.clone(),
                process,
                writer,
                queue_past_deadline: self.queue_past_deadline,
            }),
            Arc::new(move || {
                log.kills.fetch_add(1, Ordering::SeqCst);
            }),
        ))
    }
}

struct Process {
    actor: Mutex<AiCodexAppServerProtocolActor>,
    log: Arc<WireLog>,
    process: usize,
    writer: AiCodexAppServerWriter<Transcript>,
    queue_past_deadline: bool,
}

impl Process {
    async fn thread(
        &self,
        actor: &mut AiCodexAppServerProtocolActor,
        frame: Vec<u8>,
    ) -> Result<(), ProviderError> {
        let request: Value = serde_json::from_slice(&frame).unwrap();
        self.writer.write_frame(&frame).await?;
        let mut thread = json!({"id": THREAD});
        let mut projected = thread.clone();
        if request["method"] == "thread/resume" {
            let first_input = self.log.turns().into_iter().next().unwrap();
            assert_resolved(&first_input);
            thread["preview"] = json!(first_input);
            thread["name"] = json!(first_input);
            thread["futureContentField"] = json!(first_input);
            thread["turns"] = json!([{
                "id": TURN, "status": "completed", "error": {"message": first_input},
                "items": [{"id": "user-1", "type": "userMessage", "content": first_input}],
            }]);
            projected["turns"] = json!([{
                "id": TURN, "status": "completed",
                "items": [{"id": "user-1", "type": "userMessage"}],
            }]);
        }
        for frame in [
            json!({"id": request["id"], "result": {"thread": thread}}),
            json!({"method": "thread/started", "params": {"thread": thread}}),
        ] {
            let inbound = actor.accept(frame.to_string().as_bytes())?;
            let payload = match inbound {
                crate::providers::AiCodexAppServerInbound::Response { result, .. } => result,
                crate::providers::AiCodexAppServerInbound::Notification { params, .. } => params,
                _ => panic!("expected a thread lifecycle frame"),
            };
            assert_handles_only("host-facing thread snapshot", &payload.to_string());
            assert_eq!(payload, json!({"thread": projected}));
        }
        Ok(())
    }

    async fn turn(
        &self,
        actor: &mut AiCodexAppServerProtocolActor,
        dispatch: AiCodexAppServerTurnDispatch,
    ) -> Result<ProviderEventStream, ProviderError> {
        assert_handles_only("dispatch debug", &format!("{dispatch:?}"));
        let marker = dispatch.turn_input().clone();
        assert_unavailable(&marker.input().join("\n"));
        if self.log.fail_before_write.swap(false, Ordering::SeqCst) {
            return Err(ProviderError::Unavailable);
        }
        if self.queue_past_deadline {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
        actor.write_turn(THREAD, dispatch, &self.writer).await?;
        assert!(actor.start_turn(THREAD, &marker).is_err());
        let turn_start = self
            .log
            .frames
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(process, frame)| *process == self.process && frame["method"] == "turn/start")
            .unwrap()
            .1
            .clone();
        let echo = json!({"type": "userMessage", "id": "user-1", "content": turn_start["params"]["input"]});
        let turn = json!({"id": TURN, "items": [echo], "status": "inProgress"});
        let completed = json!({"id": TURN, "items": [echo], "status": "completed"});
        for frame in [
            json!({"id": turn_start["id"], "result": {"turn": turn}}),
            json!({"method": "turn/started", "params": {"threadId": THREAD, "turn": turn}}),
            json!({"method": "item/started", "params": {"threadId": THREAD, "turnId": TURN,
                "startedAtMs": 1, "item": echo}}),
            json!({"method": "item/completed", "params": {"threadId": THREAD, "turnId": TURN,
                "completedAtMs": 2, "item": echo}}),
            json!({"method": "turn/completed", "params": {"threadId": THREAD, "turn": completed}}),
        ] {
            let inbound = actor.accept(frame.to_string().as_bytes())?;
            assert_handles_only("echo accepted by the actor", &format!("{inbound:?}"));
            match inbound {
                crate::providers::AiCodexAppServerInbound::Response { result, .. } => {
                    assert_handles_only("correlated response", &result.to_string());
                }
                crate::providers::AiCodexAppServerInbound::Notification { params, .. } => {
                    assert_handles_only("notification", &params.to_string());
                }
                _ => panic!("expected a correlated response or notification"),
            }
        }
        Ok(Box::pin(futures::stream::iter(
            text_turn().into_iter().map(Ok),
        )))
    }
}

#[async_trait]
impl AiCodexAppServerRunProcess for Process {
    async fn create_empty_thread(
        &self,
        model: &str,
        effort: crate::ModelReasoningEffort,
        bootstrap: &AiCodexAppServerBootstrapInstructions,
        tools: Vec<ModelToolDefinition>,
    ) -> Result<AiProviderSessionCursor, ProviderError> {
        let mut actor = self.actor.lock().await;
        let frame = actor.start_persistent_empty_thread(model, effort, bootstrap, &tools)?;
        self.thread(&mut actor, frame).await?;
        AiProviderSessionCursor::new(CURSOR_KIND, THREAD).map_err(|_| ProviderError::Rejected)
    }

    async fn start_fresh_turn(
        &self,
        dispatch: AiCodexAppServerTurnDispatch,
    ) -> Result<ProviderEventStream, ProviderError> {
        let mut actor = self.actor.lock().await;
        let frame = actor.start_fresh_thread(dispatch.turn_input())?;
        self.thread(&mut actor, frame).await?;
        self.turn(&mut actor, dispatch).await
    }

    async fn start_bound_turn(
        &self,
        session: crate::AiOpenedProviderSession,
        dispatch: AiCodexAppServerTurnDispatch,
    ) -> Result<ProviderEventStream, ProviderError> {
        assert_eq!(session.cursor().expose_to_provider_adapter(), THREAD);
        self.turn(&mut *self.actor.lock().await, dispatch).await
    }

    async fn start_retained_turn(
        &self,
        session: crate::AiOpenedProviderSession,
        dispatch: AiCodexAppServerTurnDispatch,
    ) -> Result<ProviderEventStream, ProviderError> {
        let mut actor = self.actor.lock().await;
        let frame = actor.resume_thread(session.cursor(), dispatch.turn_input())?;
        self.thread(&mut actor, frame).await?;
        self.turn(&mut actor, dispatch).await
    }

    async fn start_bound_dynamic_turn(
        &self,
        session: crate::AiOpenedProviderSession,
        dispatch: AiCodexAppServerTurnDispatch,
        _: Arc<dyn crate::ProviderDynamicToolResponder>,
    ) -> Result<ProviderEventStream, ProviderError> {
        self.start_bound_turn(session, dispatch).await
    }

    async fn start_retained_dynamic_turn(
        &self,
        session: crate::AiOpenedProviderSession,
        dispatch: AiCodexAppServerTurnDispatch,
        _: Arc<dyn crate::ProviderDynamicToolResponder>,
    ) -> Result<ProviderEventStream, ProviderError> {
        self.start_retained_turn(session, dispatch).await
    }

    async fn interrupt(&self) -> Result<(), ProviderError> {
        Err(ProviderError::Unsupported)
    }

    async fn shutdown(&self, _: crate::AiProviderRunCloseReason) -> Result<(), ProviderError> {
        self.log.shutdowns.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct Codex {
    references: References,
    log: Arc<WireLog>,
    registration: Arc<AiCodexAppServerRegistration>,
    lease: Arc<Mutex<AiRunLease>>,
}

impl Codex {
    async fn new(
        delivery: Option<std::time::Duration>,
        frame_ceiling: usize,
        queue_past_deadline: bool,
    ) -> Self {
        Self::with_tools(delivery, frame_ceiling, queue_past_deadline, false).await
    }

    async fn with_tools(
        delivery: Option<std::time::Duration>,
        frame_ceiling: usize,
        queue_past_deadline: bool,
        dynamic_tools: bool,
    ) -> Self {
        let registration = AiCodexAppServerRegistration::new(
            "mock-profile",
            "mock-model",
            "a".repeat(64),
            "0.148.0",
            "test-wire-only",
            AI_CODEX_APP_SERVER_PROTOCOL_V2,
        )
        .unwrap();
        let registration = match delivery {
            Some(age) => registration
                .with_resolved_entity_reference_delivery(age)
                .unwrap(),
            None => registration,
        };
        let registration = if dynamic_tools {
            registration.with_launch_profile(
                AiCodexAppServerLaunchProfile::experimental_dynamic_tools_only_v1(
                    AiCodexAppServerModelToolMode::Direct,
                )
                .unwrap(),
            )
        } else {
            registration
        };
        let registration = Arc::new(registration);
        let log = Arc::new(WireLog::default());
        let factory = Arc::new(Factory {
            log: log.clone(),
            frame_ceiling,
            queue_past_deadline,
        });
        let provider = Arc::new(AiCodexAppServerProvider::new(
            registration.clone(),
            AiCodexAppServerRunPool::new(factory, AiCodexAppServerRunLimits::default()),
        ));
        let references = references_for(MockProvider::new(Vec::new()), Some(provider)).await;
        insert_retained_input_message(&references.fixture).await;
        let lease = Arc::new(Mutex::new(references.fixture.lease.clone()));
        Self {
            references,
            log,
            registration,
            lease,
        }
    }

    async fn execute(&self, number: u32) -> AiProviderCallResult {
        let fixture = &self.references.fixture;
        let dynamic_tools = self.registration.experimental_dynamic_tools();
        let base = if dynamic_tools {
            tool_plan(fixture)
        } else {
            plan(fixture)
        };
        let mut plan = attempt(
            with_reference(base, &self.references.registry),
            fixture,
            number,
        );
        plan.request.instructions.clear();
        // Make turn/start larger than the fixed thread bootstrap so the frame
        // ceiling test can admit setup and distinguish the two turn frames.
        plan.request.input[0] = ModelInputBlock::Text {
            text: "x".repeat(2048),
        };
        plan.budget.provider_kind = ProviderKind::LocalHarness;
        plan.provider_kind = ProviderKind::LocalHarness;
        plan.transfers[0].provider_kind = ProviderKind::LocalHarness.as_str().into();
        plan.transfers[0].estimated_bytes = plan.request.conservative_egress_bytes();
        let descriptor = AiProviderSessionDescriptor::new(
            ProviderKind::LocalHarness,
            self.registration.provider_profile_id(),
            self.registration.logical_model(),
            self.registration
                .provider_session_fingerprint(plan.request.reasoning_effort)
                .unwrap(),
            self.registration.protocol_version(),
            "b".repeat(64),
        )
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
        let dynamic_execution = dynamic_tools.then(|| {
            Arc::new(PersistingDynamicToolExecution {
                service: automatic_mutation_service(fixture, fixture.audit.clone()),
                scope: fixture.scope.clone(),
                calls: Arc::new(AtomicUsize::new(0)),
            }) as Arc<dyn AiProviderDynamicToolExecution>
        });
        executor(fixture)
            .execute_with_provider_session(
                self.lease.clone(),
                plan,
                AiProviderSessionTurnPlan::new(descriptor, "d".repeat(64)).unwrap(),
                sessions,
                dynamic_execution,
            )
            .await
            .expect("Codex turn should complete")
    }
}

#[tokio::test]
async fn codex_retries_the_same_session_after_revocation() {
    retained_resume_after_revocation(false).await;
}

#[tokio::test]
async fn codex_retained_dynamic_turn_resolves_again_on_resume_after_revocation() {
    retained_resume_after_revocation(true).await;
}

async fn retained_resume_after_revocation(dynamic_tools: bool) {
    let codex = Codex::with_tools(Some(FRESH), 64 * 1024, false, dynamic_tools).await;
    let result = codex.execute(1).await;
    let turns = codex.log.turns();
    assert_eq!(turns.len(), 1);
    assert_resolved(&turns[0]);
    assert_handles_only("the first result", &format!("{result:?}"));
    assert_handles_only(
        "persisted state",
        &persisted_state(&codex.references.fixture).await,
    );

    let binding = crate::AiProviderRunBinding::from_lease(&*codex.lease.lock().await).unwrap();
    codex
        .references
        .fixture
        .runtime
        .close_provider_run(
            &ProviderKind::LocalHarness,
            &binding,
            crate::AiProviderRunCloseReason::Parked,
        )
        .await
        .unwrap();
    codex.references.access.store(false, Ordering::SeqCst);
    let result = codex.execute(2).await;
    assert_eq!(codex.references.calls.load(Ordering::SeqCst), 2);
    let turns = codex.log.turns();
    assert_eq!(turns.len(), 2);
    assert_unavailable(&turns[1]);
    {
        let frames = codex.log.frames.lock().unwrap();
        let resumed: Vec<_> = frames.iter().filter(|(process, _)| *process == 1).collect();
        assert!(
            resumed
                .iter()
                .any(|(_, frame)| frame["method"] == "thread/resume"
                    && frame["params"]["threadId"] == THREAD)
        );
        assert!(
            !resumed
                .iter()
                .any(|(_, frame)| frame["method"] == "thread/start")
        );
        assert_handles_only("resumed process frames", &format!("{resumed:?}"));
        assert_eq!(format!("{frames:?}").matches("owner_note").count(), 1);
    }
    assert_handles_only("the resumed result", &format!("{result:?}"));
    assert_handles_only(
        "persisted state",
        &persisted_state(&codex.references.fixture).await,
    );
}

#[tokio::test]
async fn codex_continues_a_loaded_thread_with_fresh_resolution() {
    let codex = Codex::new(Some(FRESH), 64 * 1024, false).await;
    codex.execute(1).await;
    codex.execute(2).await;
    codex.references.access.store(false, Ordering::SeqCst);
    let result = codex.execute(3).await;
    assert_eq!(codex.references.calls.load(Ordering::SeqCst), 3);
    assert_eq!(codex.log.launches.load(Ordering::SeqCst), 1);
    let turns = codex.log.turns();
    assert_eq!(turns.len(), 3);
    assert_resolved(&turns[0]);
    assert_resolved(&turns[1]);
    assert_unavailable(&turns[2]);
    {
        let frames = codex.log.frames.lock().unwrap();
        assert_eq!(format!("{frames:?}").matches("owner_note").count(), 2);
        for (_, frame) in frames
            .iter()
            .filter(|(_, frame)| frame["method"] != "turn/start")
        {
            assert_handles_only("non-turn frame", &frame.to_string());
        }
    }
    assert_handles_only("the continuation result", &format!("{result:?}"));
    assert_handles_only(
        "persisted state",
        &persisted_state(&codex.references.fixture).await,
    );
}

#[tokio::test]
async fn codex_sends_markers_when_delivery_is_disabled_stale_or_too_large() {
    let default = Codex::new(None, 64 * 1024, false).await;
    default.execute(1).await;
    let frame_ceiling = {
        let frames = default.log.frames.lock().unwrap();
        let (_, frame) = frames
            .iter()
            .find(|(_, frame)| frame["method"] == "turn/start")
            .unwrap();
        assert_unavailable(&WireLog::input(frame));
        let ceiling = frame.to_string().len() + 100;
        assert!(
            frames
                .iter()
                .all(|(_, frame)| frame.to_string().len() < ceiling)
        );
        ceiling
    };
    let small = Codex::new(Some(FRESH), frame_ceiling, false).await;
    small.execute(1).await;
    assert_unavailable(&small.log.turns()[0]);

    let stale = Codex::new(Some(std::time::Duration::from_millis(30)), 64 * 1024, true).await;
    stale.execute(1).await;
    assert_unavailable(&stale.log.turns()[0]);
}

async fn execute_fresh_turn(references: &References, log: &WireLog) -> String {
    let fixture = &references.fixture;
    let mut plan = for_local_process(with_reference(plan(fixture), &references.registry));
    plan.request.continuation_mode = ModelContinuationMode::StatelessReplay;
    assert_handles_only("the plan", &format!("{plan:?}"));
    let result = executor(fixture)
        .execute(&fixture.lease, plan)
        .await
        .expect("the fresh turn should complete");
    assert_handles_only("the result", &format!("{result:?}"));
    assert_handles_only("persisted state", &persisted_state(fixture).await);
    let frames = log.turns();
    assert_eq!(frames.len(), 1);
    frames.into_iter().next().unwrap()
}

#[tokio::test]
async fn a_fresh_codex_turn_sends_resolved_data_without_persisting_it() {
    let codex = Codex::new(Some(FRESH), 64 * 1024, false).await;
    let references = &codex.references;
    assert_resolved(&execute_fresh_turn(references, &codex.log).await);
    assert_eq!(references.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn codex_writes_identical_markers_for_missing_failed_and_denied_references() {
    enum Outcome {
        Missing,
        Failed,
        Denied,
    }

    struct UnavailableReference {
        outcome: Outcome,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl crate::AiEntityReferenceResolver for UnavailableReference {
        async fn resolve(
            &self,
            _principal: &AuthPrincipal,
            _scope: &AiScope,
            reference_input: &crate::AiEntityReferenceInput,
        ) -> Result<Option<crate::AiResolvedEntityReference>, AiError> {
            assert_eq!(reference_input, &reference());
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.outcome {
                Outcome::Missing => Ok(None),
                Outcome::Failed => Err(AiError::PersistenceFailed),
                Outcome::Denied => Err(AiError::Forbidden),
            }
        }
    }

    let mut markers = Vec::new();
    for outcome in [Outcome::Missing, Outcome::Failed, Outcome::Denied] {
        let mut codex = Codex::new(Some(FRESH), 64 * 1024, false).await;
        let references = &mut codex.references;
        let registry = crate::AiEntityReferenceRegistry::new(
            ["record"],
            Arc::new(UnavailableReference {
                outcome,
                calls: references.calls.clone(),
            }),
        )
        .unwrap()
        .with_limits(references.registry.limits());
        Arc::get_mut(&mut references.fixture.runtime)
            .expect("the fixture owns its runtime")
            .install_entity_references_for_test(registry.clone());
        references.registry = registry;

        let marker = execute_fresh_turn(references, &codex.log).await;
        assert_unavailable(&marker);
        assert_eq!(references.calls.load(Ordering::SeqCst), 1);
        markers.push(marker.into_bytes());
    }
    assert_eq!(markers[0], markers[1]);
    assert_eq!(markers[0], markers[2]);
}

#[tokio::test]
async fn failed_codex_turn_cannot_send_its_resolution_on_retry() {
    fresh_turn_retry_after_transport_refusal(false).await;
}

#[tokio::test]
async fn codex_kills_the_process_if_a_resolved_write_stalls() {
    fresh_turn_retry_after_transport_refusal(true).await;
}

async fn fresh_turn_retry_after_transport_refusal(stall: bool) {
    let codex = Codex::new(Some(std::time::Duration::from_secs(1)), 64 * 1024, false).await;
    let references = &codex.references;
    let log = &codex.log;
    log.stall_write.store(stall, Ordering::SeqCst);
    log.fail_before_write.store(!stall, Ordering::SeqCst);
    let fixture = &references.fixture;
    let mut first = for_local_process(with_reference(plan(fixture), &references.registry));
    first.request.continuation_mode = ModelContinuationMode::StatelessReplay;
    let retry = attempt(first.clone(), fixture, 2);
    let executor = executor(fixture);
    assert!(executor.execute(&fixture.lease, first).await.is_err());
    assert_eq!(log.kills.load(Ordering::SeqCst), 1);
    assert_eq!(
        log.partial_writes.load(Ordering::SeqCst),
        usize::from(stall)
    );
    if stall {
        assert_eq!(log.shutdowns.load(Ordering::SeqCst), 0);
    }
    assert!(log.turns().is_empty());
    assert_eq!(references.calls.load(Ordering::SeqCst), 1);

    log.stall_write.store(false, Ordering::SeqCst);
    references.access.store(false, Ordering::SeqCst);
    let result = executor.execute(&fixture.lease, retry).await.unwrap();
    let turns = log.turns();
    assert_eq!(turns.len(), 1);
    assert_unavailable(&turns[0]);
    assert_eq!(references.calls.load(Ordering::SeqCst), 2);
    assert_handles_only("result", &format!("{result:?}"));
    assert_handles_only("persisted state", &persisted_state(fixture).await);
}
