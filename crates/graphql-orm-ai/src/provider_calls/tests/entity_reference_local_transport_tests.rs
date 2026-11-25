//! Entity references on the adapters that deliver a request to a local
//! process and may send resolved data.
//!
//! Each test registers the real adapter with the runtime, drives it through
//! `AiProviderCallExecutor`, and asserts on the bytes the adapter's own
//! protocol code wrote for the process.

use super::*;

#[cfg(feature = "provider-codex-app-server")]
#[path = "entity_reference_codex_tests.rs"]
mod codex;

/// Rewrites a plan sealed for the fixture's mock provider as one for a
/// local-process adapter, which registers as [`ProviderKind::LocalHarness`].
/// The Grok tests seal their own plan with the registration's tools.
#[cfg(any(feature = "local-harness", feature = "provider-codex-app-server"))]
fn for_local_process(mut plan: AiProviderCallPlan) -> AiProviderCallPlan {
    plan.provider_kind = ProviderKind::LocalHarness;
    plan.budget.provider_kind = ProviderKind::LocalHarness;
    for transfer in &mut plan.transfers {
        transfer.provider_kind = ProviderKind::LocalHarness.as_str().to_owned();
    }
    plan
}

/// Every row of every AI table that a session, a run, a provider call, a
/// tool call, a checkpoint, an egress decision or a usage entry is written
/// to, read through the ORM and serialized. Protected content is stored as
/// plain JSON by the fixture's protector, so a resolved value persisted in
/// any of these rows is found.
async fn persisted_state(fixture: &Fixture) -> String {
    let pool = fixture.database.pool();
    let mut state = String::new();
    macro_rules! scan {
        ($($record:ty),+ $(,)?) => {$(
            let rows = <$record>::query(pool)
                .limit(100)
                .fetch_all()
                .await
                .expect("records should load");
            assert!(rows.len() < 100, "the scan reads every row");
            state.push_str(&serde_json::to_string(&rows).expect("records serialize"));
            state.push('\n');
        )+};
    }
    scan!(
        AiSessionRecord,
        AiMessageRecord,
        AiMessageBlockRecord,
        AiSessionEventRecord,
        AiRunRecord,
        AiRunStepRecord,
        AiRunAttemptRecord,
        AiRunAttemptOutcomeRecord,
        AiRunCheckpointRecord,
        AiContextCheckpointRecord,
        AiToolCallRecord,
        AiToolResultSourceRecord,
        AiEgressEventRecord,
        AiAuditEventRecord,
        AiUsageEntryRecord,
        AiBudgetReservationRecord,
        AiInboxEventRecord,
        AiProviderBackgroundSubmissionRecord,
        crate::orm_provider_session::AiProviderSessionBindingRecord,
        AiStoredProviderResponseRecord,
        AiApprovalRecord,
        AiProposalRecord,
        AiProposalItemRecord,
        AiNativeApprovalCandidateRecord,
        AiSubscriptionWaiterRecord,
    );
    state
}

#[cfg(any(feature = "provider-grok-acp", feature = "provider-codex-app-server"))]
async fn insert_retained_input_message(fixture: &Fixture) {
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
}

#[cfg(feature = "local-harness")]
mod local_harness {
    use super::*;
    use crate::{
        AiJsonLinesLocalHarnessDriver, AiLocalHarnessDriver, AiLocalHarnessLimits,
        AiLocalHarnessProcess, AiLocalHarnessProcessError, AiLocalHarnessProcessLauncher,
        AiLocalHarnessProcessOutput, AiLocalHarnessProvider, AiLocalHarnessRegistration,
        AiLocalHarnessRegistry,
    };
    use std::collections::VecDeque;

    type Frames = Arc<std::sync::Mutex<Vec<String>>>;

    /// Launches one fake process per turn. Every `write_stdin` call is
    /// recorded as its own entry, so a second write of a frame would show.
    struct Launcher {
        turns: std::sync::Mutex<VecDeque<Vec<ProviderEvent>>>,
        frames: Frames,
        /// How long the process takes to become ready.
        launch_delay: std::time::Duration,
        /// What each process did, in order: `write` (a `write_stdin` call),
        /// `close`, `terminate` and `drop`.
        events: Events,
        /// Whether a write of a frame holding the resolved value accepts
        /// part of it and then never completes.
        stall_resolved_write: bool,
    }

    type Events = Arc<std::sync::Mutex<Vec<&'static str>>>;

    struct Process {
        outputs: VecDeque<AiLocalHarnessProcessOutput>,
        frames: Frames,
        events: Events,
        stall_resolved_write: bool,
    }

    impl Process {
        fn record(&self, event: &'static str) {
            self.events.lock().expect("events should lock").push(event);
        }
    }

    impl Drop for Process {
        fn drop(&mut self) {
            self.record("drop");
        }
    }

    #[async_trait]
    impl AiLocalHarnessProcessLauncher for Launcher {
        async fn launch(
            &self,
            _registration: Arc<AiLocalHarnessRegistration>,
        ) -> Result<Box<dyn AiLocalHarnessProcess>, AiLocalHarnessProcessError> {
            tokio::time::sleep(self.launch_delay).await;
            let turn = self
                .turns
                .lock()
                .expect("turns should lock")
                .pop_front()
                .ok_or(AiLocalHarnessProcessError::Unavailable)?;
            let mut stdout = Vec::new();
            for event in &turn {
                stdout.extend(serde_json::to_vec(event).expect("event should encode"));
                stdout.push(b'\n');
            }
            Ok(Box::new(Process {
                outputs: [
                    AiLocalHarnessProcessOutput::Stdout(stdout),
                    AiLocalHarnessProcessOutput::Exited { success: true },
                ]
                .into(),
                frames: self.frames.clone(),
                events: self.events.clone(),
                stall_resolved_write: self.stall_resolved_write,
            }))
        }
    }

    #[async_trait]
    impl AiLocalHarnessProcess for Process {
        async fn write_stdin(&mut self, bytes: &[u8]) -> Result<(), AiLocalHarnessProcessError> {
            self.record("write");
            if self.stall_resolved_write && String::from_utf8_lossy(bytes).contains(FIELD) {
                // Part of the frame is in the pipe; the rest never drains.
                std::future::pending::<()>().await;
            }
            self.frames
                .lock()
                .expect("frames should lock")
                .push(String::from_utf8(bytes.to_vec()).expect("frame should be UTF-8"));
            Ok(())
        }

        async fn close_stdin(&mut self) -> Result<(), AiLocalHarnessProcessError> {
            self.record("close");
            Ok(())
        }

        async fn next_output(
            &mut self,
        ) -> Result<AiLocalHarnessProcessOutput, AiLocalHarnessProcessError> {
            self.outputs
                .pop_front()
                .ok_or(AiLocalHarnessProcessError::OutputFailed)
        }

        async fn terminate(&mut self) -> Result<(), AiLocalHarnessProcessError> {
            self.record("terminate");
            Ok(())
        }
    }

    /// A freshness bound no test reaches by accident.
    const FRESH: std::time::Duration = std::time::Duration::from_secs(30);

    /// A registration that opted in to resolved delivery with the bound
    /// `delivery`, or that did not opt in.
    fn registration(
        maximum_request_bytes: usize,
        delivery: Option<std::time::Duration>,
    ) -> AiLocalHarnessRegistration {
        let limits = AiLocalHarnessLimits::default();
        let registration = AiLocalHarnessRegistration::new(
            "mock-model",
            "/opt/reviewed/bin/synthetic-harness",
            vec!["--json-lines".to_owned()],
            "/var/empty/graphql-orm-ai-harness",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "synthetic-1.0.0",
            "isolated-no-network-v1",
            AiLocalHarnessLimits::new(
                maximum_request_bytes,
                limits.maximum_frame_bytes(),
                limits.maximum_total_output_bytes(),
                limits.maximum_stderr_bytes(),
                limits.maximum_frames(),
                limits.startup_timeout(),
                limits.turn_timeout(),
                limits.shutdown_timeout(),
                limits.maximum_memory_bytes(),
                limits.maximum_cpu_time(),
            )
            .expect("limits should validate"),
            ProviderCapabilities {
                streaming: true,
                custom_tools: true,
                parallel_tool_calls: true,
                stateless_continuation: true,
                local: true,
                maximum_context_tokens: Some(1_000_000),
                maximum_output_tokens: Some(128),
                ..ProviderCapabilities::default()
            },
        )
        .expect("registration should validate");
        match delivery {
            Some(maximum_age) => registration
                .with_resolved_entity_reference_delivery(maximum_age)
                .expect("the delivery bound should validate"),
            None => registration,
        }
    }

    struct Harness {
        references: References,
        frames: Frames,
        events: Events,
    }

    /// The real provider and JSON-lines driver over a fake process launcher,
    /// for a registration that opted in to resolved delivery.
    async fn harness(
        turns: impl IntoIterator<Item = Vec<ProviderEvent>>,
        maximum_request_bytes: usize,
    ) -> Harness {
        harness_with(
            turns,
            registration(maximum_request_bytes, Some(FRESH)),
            std::time::Duration::ZERO,
        )
        .await
    }

    async fn harness_with(
        turns: impl IntoIterator<Item = Vec<ProviderEvent>>,
        registration: AiLocalHarnessRegistration,
        launch_delay: std::time::Duration,
    ) -> Harness {
        harness_stalling(turns, registration, launch_delay, false).await
    }

    async fn harness_stalling(
        turns: impl IntoIterator<Item = Vec<ProviderEvent>>,
        registration: AiLocalHarnessRegistration,
        launch_delay: std::time::Duration,
        stall_resolved_write: bool,
    ) -> Harness {
        let frames = Frames::default();
        let events = Events::default();
        let driver = AiJsonLinesLocalHarnessDriver::new(Arc::new(Launcher {
            turns: std::sync::Mutex::new(turns.into_iter().collect()),
            frames: frames.clone(),
            launch_delay,
            events: events.clone(),
            stall_resolved_write,
        }));
        let provider = harness_provider(Arc::new(driver), registration);
        Harness {
            references: references_for(MockProvider::new(Vec::new()), Some(provider)).await,
            frames,
            events,
        }
    }

    fn harness_provider(
        driver: Arc<dyn AiLocalHarnessDriver>,
        registration: AiLocalHarnessRegistration,
    ) -> Arc<dyn crate::AiProvider> {
        Arc::new(AiLocalHarnessProvider::new(
            AiLocalHarnessRegistry::new([registration]).expect("registry should validate"),
            driver,
        ))
    }

    fn frames(frames: &Frames) -> Vec<String> {
        frames.lock().expect("frames should lock").clone()
    }

    /// Revoke, then retry a cloned plan.
    ///
    /// The first call writes the resolved value into the one stdin frame of
    /// the harness process. The retry, executed from a clone of the plan
    /// after access is revoked, launches a new process and writes the marker.
    /// Each process receives exactly one write.
    #[tokio::test]
    async fn the_harness_frame_is_resolved_once_and_a_retry_after_revocation_sends_the_marker() {
        let Harness {
            references, frames, ..
        } = harness([text_turn(), text_turn()], 1024 * 1024).await;
        let References {
            fixture,
            access,
            calls,
            registry,
        } = references;
        let executor = executor(&fixture);
        let first = for_local_process(with_reference(plan(&fixture), &registry));
        let retry = attempt(first.clone(), &fixture, 2);
        assert_handles_only("the plan", &format!("{first:?}"));

        let result = executor
            .execute(&fixture.lease, first)
            .await
            .expect("the first call should complete");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let written = self::frames(&frames);
        assert_eq!(written.len(), 1, "one process, one stdin write");
        assert_resolved(&written[0]);
        assert_handles_only("the call result", &format!("{result:?}"));
        assert_handles_only("persisted state", &persisted_state(&fixture).await);

        access.store(false, Ordering::SeqCst);
        let result = executor
            .execute(&fixture.lease, retry)
            .await
            .expect("the retry should complete");
        assert_eq!(calls.load(Ordering::SeqCst), 2, "the retry resolved again");
        let written = self::frames(&frames);
        assert_eq!(written.len(), 2);
        assert_unavailable(&written[1]);
        assert_handles_only("the retry result", &format!("{result:?}"));
        assert_handles_only("persisted state", &persisted_state(&fixture).await);
    }

    /// Continuation: a stateless tool loop replays turn one's user message.
    ///
    /// While access holds, the replayed handle is resolved again and written
    /// into the continuation's frame. After revocation the same continuation
    /// writes the marker. The frames are the only place the value appears.
    #[tokio::test]
    async fn a_harness_continuation_after_revocation_sends_the_marker() {
        let Harness {
            references, frames, ..
        } = harness([tool_turn(), text_turn(), text_turn()], 1024 * 1024).await;
        let ToolLoop {
            references,
            continuation,
            batch_lease,
            ..
        } = tool_loop_on(references, for_local_process, &|| self::frames(&frames)).await;
        let References {
            mut fixture,
            access,
            calls,
            ..
        } = references;
        fixture.lease = batch_lease;
        // The scan reads protected content: the stored checkpoints hold the
        // handle, and nothing it resolved to.
        let stored = persisted_state(&fixture).await;
        assert!(stored.contains("r-1"), "the scan sees the stored handle");
        assert_handles_only("persisted state", &stored);

        let allowed = continuation_plan_on(&fixture, continuation.clone(), 2, for_local_process);
        let after_revocation = continuation_plan_on(&fixture, continuation, 3, for_local_process);
        let executor = executor(&fixture);
        let resolutions = calls.load(Ordering::SeqCst);
        executor
            .execute(&fixture.lease, allowed)
            .await
            .expect("the continuation should complete");
        assert_eq!(calls.load(Ordering::SeqCst), resolutions + 1);
        let written = self::frames(&frames);
        assert_eq!(written.len(), 2);
        assert!(written[1].contains("stateless_conversation"));
        assert_resolved(&written[1]);

        access.store(false, Ordering::SeqCst);
        let result = executor
            .execute(&fixture.lease, after_revocation)
            .await
            .expect("the continuation should complete");
        let written = self::frames(&frames);
        assert_eq!(written.len(), 3);
        assert_unavailable(&written[2]);
        assert_handles_only("the continuation result", &format!("{result:?}"));
        assert_handles_only("persisted state", &persisted_state(&fixture).await);
    }

    /// A frame that would exceed the registered request ceiling once the
    /// reference is resolved is written with the marker instead; the request
    /// is not refused and the value is not truncated into the frame.
    #[tokio::test]
    async fn a_resolved_frame_over_the_request_ceiling_is_sent_as_the_marker() {
        let probe = harness([text_turn()], 1024 * 1024).await;
        let marker_plan = for_local_process(with_reference(
            plan(&probe.references.fixture),
            &probe.references.registry,
        ));
        probe.references.access.store(false, Ordering::SeqCst);
        executor(&probe.references.fixture)
            .execute(&probe.references.fixture.lease, marker_plan)
            .await
            .expect("the probe should complete");
        let marker_bytes = self::frames(&probe.frames)[0].len();

        // Room for the marker frame, not for the resolved one.
        let Harness {
            references, frames, ..
        } = harness([text_turn()], marker_bytes + 8).await;
        let fixture = &references.fixture;
        let plan = for_local_process(with_reference(plan(fixture), &references.registry));
        executor(fixture)
            .execute(&fixture.lease, plan)
            .await
            .expect("the call should complete with the marker");
        assert_eq!(references.calls.load(Ordering::SeqCst), 1);
        let written = self::frames(&frames);
        assert_eq!(written.len(), 1);
        assert_unavailable(&written[0]);
    }

    /// Default off: a registration that has not opted in is sent the marker
    /// by the built-in driver, although the deployment has a registry and
    /// the executor resolved the reference.
    #[tokio::test]
    async fn a_registration_that_has_not_opted_in_receives_the_marker() {
        let Harness {
            references, frames, ..
        } = harness_with(
            [text_turn()],
            registration(1024 * 1024, None),
            std::time::Duration::ZERO,
        )
        .await;
        let fixture = &references.fixture;
        let plan = for_local_process(with_reference(plan(fixture), &references.registry));
        executor(fixture)
            .execute(&fixture.lease, plan)
            .await
            .expect("the call should complete");
        assert_eq!(references.calls.load(Ordering::SeqCst), 1);
        let written = self::frames(&frames);
        assert_eq!(written.len(), 1);
        assert_unavailable(&written[0]);
    }

    /// The freshness bound runs from resolution to the write. A process that
    /// takes longer than the bound to become ready is sent the marker frame;
    /// the resolution is not written late.
    #[tokio::test]
    async fn a_resolution_older_than_the_freshness_bound_is_sent_as_the_marker() {
        let Harness {
            references, frames, ..
        } = harness_with(
            [text_turn()],
            registration(1024 * 1024, Some(std::time::Duration::from_millis(40))),
            std::time::Duration::from_millis(400),
        )
        .await;
        let fixture = &references.fixture;
        let plan = for_local_process(with_reference(plan(fixture), &references.registry));
        executor(fixture)
            .execute(&fixture.lease, plan)
            .await
            .expect("the call should complete with the marker");
        assert_eq!(references.calls.load(Ordering::SeqCst), 1);
        let written = self::frames(&frames);
        assert_eq!(written.len(), 1);
        assert_unavailable(&written[0]);
    }

    /// The freshness bound passes after the resolved frame was encoded and
    /// before its write starts. The marker frame is written and the turn
    /// completes; the resolved frame never reaches the process.
    #[tokio::test]
    async fn a_deadline_that_expires_between_encoding_and_writing_sends_the_marker() {
        let Harness {
            references,
            frames,
            events,
        } = harness_with(
            [text_turn()],
            registration(1024 * 1024, Some(std::time::Duration::from_millis(1_000))),
            std::time::Duration::ZERO,
        )
        .await;
        let fixture = &references.fixture;
        let plan = for_local_process(with_reference(plan(fixture), &references.registry));
        // The seam lets more than the bound pass between the encoding of the
        // resolved frame and the start check of its write.
        crate::entity_references::DELAY_BEFORE_RESOLVED_WRITE
            .set(std::time::Duration::from_millis(1_500));
        let result = executor(fixture).execute(&fixture.lease, plan).await;
        crate::entity_references::DELAY_BEFORE_RESOLVED_WRITE.set(std::time::Duration::ZERO);
        result.expect("the call should complete with the marker");
        assert_eq!(references.calls.load(Ordering::SeqCst), 1);
        let written = self::frames(&frames);
        assert_eq!(written.len(), 1);
        assert_unavailable(&written[0]);
        assert_eq!(
            events.lock().expect("events should lock")[..2],
            ["write", "close"],
            "one write, of the marker frame"
        );
    }

    /// The freshness bound passes while the resolved write is in progress and
    /// the pipe stays full. The turn fails. The process is dropped, which is
    /// its forced kill: it is never asked to close stdin or to terminate
    /// gracefully, either of which could flush the remainder of the frame,
    /// and no second write is made.
    #[tokio::test]
    async fn a_resolved_write_still_pending_at_the_deadline_fails_the_turn_and_kills_the_process() {
        let Harness {
            references,
            frames,
            events,
        } = harness_stalling(
            [text_turn()],
            registration(1024 * 1024, Some(std::time::Duration::from_millis(800))),
            std::time::Duration::ZERO,
            true,
        )
        .await;
        let fixture = &references.fixture;
        let plan = for_local_process(with_reference(plan(fixture), &references.registry));
        executor(fixture)
            .execute(&fixture.lease, plan)
            .await
            .expect_err("the abandoned write should fail the turn");
        assert_eq!(references.calls.load(Ordering::SeqCst), 1);
        assert!(
            self::frames(&frames).is_empty(),
            "no frame was written completely"
        );
        assert_eq!(
            *events.lock().expect("events should lock"),
            ["write", "drop"],
            "one write began; the process was then dropped without close or terminate"
        );
    }

    /// The opt-in rejects a zero bound and one above the crate ceiling.
    #[test]
    fn the_delivery_bound_is_validated() {
        let base = || registration(1024 * 1024, None);
        assert!(base().resolved_entity_reference_delivery().is_none());
        assert!(
            base()
                .with_resolved_entity_reference_delivery(std::time::Duration::ZERO)
                .is_err()
        );
        assert!(
            base()
                .with_resolved_entity_reference_delivery(
                    crate::AI_MAXIMUM_ENTITY_REFERENCE_DELIVERY_AGE
                        + std::time::Duration::from_millis(1)
                )
                .is_err()
        );
        assert_eq!(
            base()
                .with_resolved_entity_reference_delivery(FRESH)
                .expect("a bound within the ceiling validates")
                .resolved_entity_reference_delivery(),
            Some(FRESH)
        );
    }

    /// A driver that does not implement `stream_dispatch` receives a plain
    /// request in which the reference is the marker, although the executor
    /// resolved it.
    #[tokio::test]
    async fn a_driver_without_a_dispatch_ingress_receives_the_marker() {
        struct Legacy(std::sync::Mutex<Vec<ModelRequest>>);
        #[async_trait]
        impl AiLocalHarnessDriver for Legacy {
            async fn stream(
                &self,
                _registration: Arc<AiLocalHarnessRegistration>,
                request: ModelRequest,
            ) -> Result<ProviderEventStream, ProviderError> {
                self.0.lock().expect("requests should lock").push(request);
                Ok(Box::pin(futures::stream::iter(
                    text_turn().into_iter().map(Ok),
                )))
            }
        }
        let driver = Arc::new(Legacy(std::sync::Mutex::new(Vec::new())));
        let references = references_for(
            MockProvider::new(Vec::new()),
            Some(harness_provider(
                driver.clone(),
                registration(1024 * 1024, Some(FRESH)),
            )),
        )
        .await;
        let fixture = &references.fixture;
        let plan = for_local_process(with_reference(plan(fixture), &references.registry));
        executor(fixture)
            .execute(&fixture.lease, plan)
            .await
            .expect("the call should complete");
        assert_eq!(references.calls.load(Ordering::SeqCst), 1);
        let received = driver.0.lock().expect("requests should lock");
        assert_eq!(received.len(), 1);
        assert_unavailable(&serde_json::to_string(&received[0]).expect("request serializes"));
    }
}

#[cfg(feature = "provider-grok-acp")]
mod grok_acp {
    use super::*;
    use crate::providers::{AiGrokAcpProvider, AiGrokAcpRegistration, ReferenceWireFactory};

    struct Grok {
        references: References,
        factory: Arc<ReferenceWireFactory>,
        registration: Arc<AiGrokAcpRegistration>,
        /// The run's lease, shared by every turn of the run.
        lease: Arc<Mutex<AiRunLease>>,
    }

    /// The real provider over the real ACP wire process and a fake transport.
    /// `delivery` is the registration's opt-in to resolved delivery.
    async fn grok(delivery: Option<std::time::Duration>) -> Grok {
        let seed = fixture(vec![]).await;
        let tools = tool_plan(&seed).request.tools;
        let registration = AiGrokAcpRegistration::new(
            "mock-profile".into(),
            "grok-4.7".into(),
            "a".repeat(64),
            "1.0.40".into(),
            "test-wire-only".into(),
            crate::ModelReasoningEffortProfile::new(
                "grok-4.7",
                [crate::ModelReasoningEffort::Low],
                crate::ModelReasoningEffort::Low,
            )
            .expect("effort profile should validate"),
            crate::ModelReasoningEffort::Low,
            "Use authorized tools".into(),
            tools,
            16384,
            2048,
            8,
        )
        .expect("registration should validate")
        .with_usage_model("grok-4.7-build".into())
        .expect("usage model should validate")
        .with_retained_namespace("d".repeat(64))
        .expect("namespace should validate");
        let registration = Arc::new(match delivery {
            Some(maximum_age) => registration
                .with_resolved_entity_reference_delivery(maximum_age)
                .expect("the delivery bound should validate"),
            None => registration,
        });
        let factory = Arc::new(ReferenceWireFactory::default());
        let provider = Arc::new(
            AiGrokAcpProvider::new(
                registration.clone(),
                factory.clone(),
                2,
                std::time::Duration::from_secs(5),
            )
            .expect("provider should validate"),
        );
        let references = references_for(MockProvider::new(Vec::new()), Some(provider)).await;
        insert_retained_input_message(&references.fixture).await;
        let lease = Arc::new(Mutex::new(references.fixture.lease.clone()));
        Grok {
            references,
            factory,
            registration,
            lease,
        }
    }

    /// The `number`-th retained Grok turn of the run, carrying the reference,
    /// through `execute_with_provider_session`.
    async fn execute(grok: &Grok, number: u32) -> AiProviderCallResult {
        let fixture = &grok.references.fixture;
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
            .expect("provider-session service should validate"),
        );
        let mut base = attempt(tool_plan(fixture), fixture, number);
        base.request.model = "grok-4.7".into();
        base.request.instructions.clear();
        base.request.reasoning_effort = crate::ModelReasoningEffort::Low;
        base.request
            .input
            .push(grok.references.registry.model_input_block(&reference()));
        base.budget.provider_kind = ProviderKind::LocalHarness;
        base.budget.model = base.request.model.clone();
        base.budget.reasoning_effort = base.request.reasoning_effort;
        base.transfers[0].provider_kind = ProviderKind::LocalHarness.as_str().into();
        base.transfers[0].model = base.request.model.clone();
        base.transfers[0].estimated_bytes = base.request.conservative_egress_bytes();
        let plan = AiProviderCallPlan::new_with_tools(
            ProviderKind::LocalHarness,
            base.request,
            base.budget,
            base.transfers,
            base.correlation_id,
            fixture.runtime.tool_catalog(),
            &static_read_policy(fixture),
        )
        .expect("the plan should seal");
        assert_handles_only("the plan", &format!("{plan:?}"));
        let descriptor = AiProviderSessionDescriptor::new(
            ProviderKind::LocalHarness,
            grok.registration.provider_profile_id(),
            grok.registration.model(),
            grok.registration.session_fingerprint(),
            grok.registration.protocol_version(),
            "b".repeat(64),
        )
        .expect("descriptor should validate");
        executor(fixture)
            .execute_with_provider_session(
                grok.lease.clone(),
                plan,
                AiProviderSessionTurnPlan::new(descriptor, "d".repeat(64))
                    .expect("session plan should validate"),
                sessions,
                Some(Arc::new(PersistingDynamicToolExecution {
                    service: automatic_mutation_service(fixture, fixture.audit.clone()),
                    scope: fixture.scope.clone(),
                    calls: Arc::new(AtomicUsize::new(0)),
                })),
            )
            .await
            .expect("the retained Grok turn should complete")
    }

    const FRESH: std::time::Duration = std::time::Duration::from_secs(30);

    /// The executor resolves the reference for the run's principal and the
    /// real wire process writes the value in its one `session/prompt` frame
    /// and in no other frame. Nothing resolved is persisted or printed.
    #[tokio::test]
    async fn the_grok_prompt_frame_carries_the_resolved_reference_once() {
        let grok = grok(Some(FRESH)).await;
        let result = execute(&grok, 1).await;
        assert_eq!(grok.references.calls.load(Ordering::SeqCst), 1);
        let prompts = grok.factory.prompts();
        assert_eq!(prompts.len(), 1, "one process, one prompt write");
        assert_resolved(&prompts[0]);
        assert_eq!(
            grok.factory.frames().matches("owner_note").count(),
            1,
            "the value is written in one frame only"
        );
        assert_handles_only("the call result", &format!("{result:?}"));
        assert_handles_only(
            "persisted state",
            &persisted_state(&grok.references.fixture).await,
        );
    }

    /// Default off: a registration that has not opted in is sent the marker,
    /// although the deployment has a registry and the executor resolved the
    /// reference.
    #[tokio::test]
    async fn a_grok_registration_that_has_not_opted_in_receives_the_marker() {
        let grok = grok(None).await;
        execute(&grok, 1).await;
        assert_eq!(grok.references.calls.load(Ordering::SeqCst), 1);
        let prompts = grok.factory.prompts();
        assert_eq!(prompts.len(), 1);
        assert_unavailable(&prompts[0]);
        assert_handles_only("every written frame", &grok.factory.frames());
    }

    /// With access revoked before the turn is dispatched, the same handle is
    /// resolved for the run's principal, denied, and written as the marker.
    #[tokio::test]
    async fn a_grok_turn_dispatched_after_revocation_sends_the_marker() {
        let grok = grok(Some(FRESH)).await;
        grok.references.access.store(false, Ordering::SeqCst);
        execute(&grok, 1).await;
        assert_eq!(grok.references.calls.load(Ordering::SeqCst), 1);
        let prompts = grok.factory.prompts();
        assert_eq!(prompts.len(), 1);
        assert_unavailable(&prompts[0]);
        assert_handles_only("every written frame", &grok.factory.frames());
    }

    /// Same session: a successful turn, then revocation, then a second turn
    /// that resumes the same cursor in a new process.
    ///
    /// The first turn writes the resolved value. The run's process is then
    /// closed, as when a run parks or its worker restarts. After revocation
    /// the second turn resolves again, the new process resumes the retained
    /// cursor, and its prompt carries the marker. No frame of the second
    /// process (initialize, `session/resume`, `session/prompt`) and no
    /// crate-persisted row holds the value.
    #[tokio::test]
    async fn a_grok_session_resumed_after_revocation_sends_the_marker() {
        let grok = grok(Some(FRESH)).await;
        let fixture = &grok.references.fixture;
        execute(&grok, 1).await;
        assert_eq!(grok.references.calls.load(Ordering::SeqCst), 1);
        assert_resolved(&grok.factory.prompts()[0]);
        assert_handles_only("persisted state", &persisted_state(fixture).await);

        let binding = crate::AiProviderRunBinding::from_lease(&*grok.lease.lock().await)
            .expect("the lease should bind");
        fixture
            .runtime
            .close_provider_run(
                &ProviderKind::LocalHarness,
                &binding,
                crate::AiProviderRunCloseReason::Parked,
            )
            .await
            .expect("the run's process should close");
        grok.references.access.store(false, Ordering::SeqCst);

        let result = execute(&grok, 2).await;
        assert_eq!(
            grok.references.calls.load(Ordering::SeqCst),
            2,
            "the resumed turn resolved again"
        );
        let prompts = grok.factory.prompts();
        assert_eq!(prompts.len(), 2);
        assert_unavailable(&prompts[1]);
        let resumed = grok.factory.process_frames(1);
        assert!(resumed.contains(r#""method":"session/resume""#));
        assert!(
            resumed.contains(r#""sessionId":"session-1""#),
            "the second process resumed the first turn's cursor"
        );
        assert!(!resumed.contains(r#""method":"session/new""#));
        assert_handles_only("the resumed process's frames", &resumed);
        assert_handles_only("the resumed turn's result", &format!("{result:?}"));
        assert_handles_only("persisted state", &persisted_state(fixture).await);
    }
}
