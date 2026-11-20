//! Entity references on the adapters that deliver a request to a local
//! process: the JSON-lines local harness, the Codex app-server and Grok ACP.
//!
//! Each test registers the real adapter with the runtime, drives it through
//! `AiProviderCallExecutor`, and asserts on the bytes the adapter's own
//! protocol code wrote for the process: the harness stdin frame, the
//! `turn/start` frame the Codex protocol actor encoded, and the
//! `session/prompt` frame the Grok wire process wrote.

use super::*;

/// Rewrites a plan sealed for the fixture's mock provider as one for a
/// local-process adapter, which registers as [`ProviderKind::LocalHarness`].
fn for_local_process(mut plan: AiProviderCallPlan) -> AiProviderCallPlan {
    plan.provider_kind = ProviderKind::LocalHarness;
    plan.budget.provider_kind = ProviderKind::LocalHarness;
    for transfer in &mut plan.transfers {
        transfer.provider_kind = ProviderKind::LocalHarness.as_str().to_owned();
    }
    plan
}

/// Every text and blob column of every table of the fixture database, so a
/// resolved value persisted anywhere, by any service, is found.
async fn persisted_state(fixture: &Fixture) -> String {
    use graphql_orm::sqlx::{self, Row};
    let pool = fixture.database.pool();
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table'")
            .fetch_all(pool)
            .await
            .expect("tables should list");
    assert!(tables.len() > 10, "the scan covers the AI schema");
    let mut state = String::new();
    for table in tables {
        let rows = sqlx::query(&format!(r#"SELECT * FROM "{table}""#))
            .fetch_all(pool)
            .await
            .expect("table should read");
        for row in rows {
            for column in 0..row.len() {
                if let Ok(Some(text)) = row.try_get::<Option<String>, _>(column) {
                    state.push_str(&text);
                } else if let Ok(Some(bytes)) = row.try_get::<Option<Vec<u8>>, _>(column) {
                    state.push_str(&String::from_utf8_lossy(&bytes));
                }
                state.push('\n');
            }
        }
    }
    state
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
    }

    struct Process {
        outputs: VecDeque<AiLocalHarnessProcessOutput>,
        frames: Frames,
    }

    #[async_trait]
    impl AiLocalHarnessProcessLauncher for Launcher {
        async fn launch(
            &self,
            _registration: Arc<AiLocalHarnessRegistration>,
        ) -> Result<Box<dyn AiLocalHarnessProcess>, AiLocalHarnessProcessError> {
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
            }))
        }
    }

    #[async_trait]
    impl AiLocalHarnessProcess for Process {
        async fn write_stdin(&mut self, bytes: &[u8]) -> Result<(), AiLocalHarnessProcessError> {
            self.frames
                .lock()
                .expect("frames should lock")
                .push(String::from_utf8(bytes.to_vec()).expect("frame should be UTF-8"));
            Ok(())
        }

        async fn close_stdin(&mut self) -> Result<(), AiLocalHarnessProcessError> {
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
            Ok(())
        }
    }

    fn registration(maximum_request_bytes: usize) -> AiLocalHarnessRegistration {
        let limits = AiLocalHarnessLimits::default();
        AiLocalHarnessRegistration::new(
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
        .expect("registration should validate")
    }

    struct Harness {
        references: References,
        frames: Frames,
    }

    /// The real provider and JSON-lines driver over a fake process launcher.
    async fn harness(
        turns: impl IntoIterator<Item = Vec<ProviderEvent>>,
        maximum_request_bytes: usize,
    ) -> Harness {
        let frames = Frames::default();
        let driver = AiJsonLinesLocalHarnessDriver::new(Arc::new(Launcher {
            turns: std::sync::Mutex::new(turns.into_iter().collect()),
            frames: frames.clone(),
        }));
        let provider = harness_provider(Arc::new(driver), maximum_request_bytes);
        Harness {
            references: references_for(MockProvider::new(Vec::new()), Some(provider)).await,
            frames,
        }
    }

    fn harness_provider(
        driver: Arc<dyn AiLocalHarnessDriver>,
        maximum_request_bytes: usize,
    ) -> Arc<dyn crate::AiProvider> {
        Arc::new(AiLocalHarnessProvider::new(
            AiLocalHarnessRegistry::new([registration(maximum_request_bytes)])
                .expect("registry should validate"),
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
        let Harness { references, frames } = harness([text_turn(), text_turn()], 1024 * 1024).await;
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
        let Harness { references, frames } =
            harness([tool_turn(), text_turn(), text_turn()], 1024 * 1024).await;
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
        let Harness { references, frames } = harness([text_turn()], marker_bytes + 8).await;
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
            Some(harness_provider(driver.clone(), 1024 * 1024)),
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

#[cfg(feature = "provider-codex-app-server")]
mod codex_app_server {
    use super::*;
    use crate::providers::{
        AI_CODEX_APP_SERVER_PROTOCOL_V2, AiCodexAppServerProvider, AiCodexAppServerRegistration,
        AiCodexAppServerRunLimits, AiCodexAppServerRunPool, ExecutorActorCapture,
        ExecutorActorFactory,
    };

    fn codex_plan(
        fixture: &Fixture,
        registry: &crate::AiEntityReferenceRegistry,
    ) -> AiProviderCallPlan {
        let mut plan = plan(fixture);
        plan.request.continuation_mode = ModelContinuationMode::StatelessReplay;
        for_local_process(with_reference(plan, registry))
    }

    fn lines(lines: &std::sync::Mutex<Vec<String>>) -> Vec<String> {
        lines.lock().expect("capture should lock").clone()
    }

    /// Revoke, then retry a cloned plan, with the turn cloned in between.
    ///
    /// The provider builds a cloneable turn and hands it to the process,
    /// which encodes `turn/start` with the real protocol actor. The first
    /// encoding carries the resolved value. A second encoding of the same
    /// turn, and an encoding of a clone taken before the first, carry the
    /// marker: the resolution is single-use. `Debug` output of the turn and
    /// its public input text never hold the value. After revocation the
    /// retried plan's turn encodes the marker the first time too.
    #[tokio::test]
    async fn the_codex_turn_is_resolved_once_and_a_retry_after_revocation_sends_the_marker() {
        let capture = Arc::new(ExecutorActorCapture::default());
        let registration = AiCodexAppServerRegistration::new(
            "mock-profile",
            "mock-model",
            "a".repeat(64),
            "1.0.0",
            "isolated-test",
            AI_CODEX_APP_SERVER_PROTOCOL_V2,
        )
        .expect("registration should validate");
        let provider = Arc::new(AiCodexAppServerProvider::new(
            Arc::new(registration),
            AiCodexAppServerRunPool::new(
                Arc::new(ExecutorActorFactory(capture.clone())),
                AiCodexAppServerRunLimits::default(),
            ),
        ));
        let References {
            fixture,
            access,
            calls,
            registry,
        } = references_for(MockProvider::new(Vec::new()), Some(provider)).await;
        let executor = executor(&fixture);
        let first = codex_plan(&fixture, &registry);
        let retry = attempt(first.clone(), &fixture, 2);

        let result = executor
            .execute(&fixture.lease, first)
            .await
            .expect("the first turn should complete");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let frames = lines(&capture.frames);
        assert_eq!(frames.len(), 1);
        assert_resolved(&frames[0]);
        let replays = lines(&capture.replays);
        assert_eq!(replays.len(), 2);
        assert_unavailable(&replays[0]);
        assert_unavailable(&replays[1]);
        let observed = lines(&capture.observed);
        assert_handles_only("the queued turn", &observed[0]);
        assert!(observed[0].contains(r#"\"status\":\"unavailable\""#));
        assert_handles_only("the call result", &format!("{result:?}"));
        assert_handles_only("persisted state", &persisted_state(&fixture).await);

        access.store(false, Ordering::SeqCst);
        executor
            .execute(&fixture.lease, retry)
            .await
            .expect("the retried turn should complete");
        assert_eq!(calls.load(Ordering::SeqCst), 2, "the retry resolved again");
        let frames = lines(&capture.frames);
        assert_eq!(frames.len(), 2);
        assert_unavailable(&frames[1]);
        for replay in lines(&capture.replays) {
            assert_unavailable(&replay);
        }
        assert_handles_only("persisted state", &persisted_state(&fixture).await);
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
    }

    /// The real provider over the real ACP wire process and a fake transport.
    async fn grok() -> Grok {
        let seed = fixture(vec![]).await;
        let tools = tool_plan(&seed).request.tools;
        let registration = Arc::new(
            AiGrokAcpRegistration::new(
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
            .expect("namespace should validate"),
        );
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
        let fixture = &references.fixture;
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
        Grok {
            references,
            factory,
            registration,
        }
    }

    /// One retained Grok turn carrying the reference, through
    /// `execute_with_provider_session`.
    async fn execute(grok: &Grok) -> AiProviderCallResult {
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
        let mut base = tool_plan(fixture);
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
                Arc::new(Mutex::new(fixture.lease.clone())),
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

    /// The executor resolves the reference for the run's principal and the
    /// real wire process writes the value in its one `session/prompt` frame
    /// and in no other frame. Nothing resolved is persisted or printed.
    #[tokio::test]
    async fn the_grok_prompt_frame_carries_the_resolved_reference_once() {
        let grok = grok().await;
        let result = execute(&grok).await;
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

    /// With access revoked before the turn is dispatched, the same handle is
    /// resolved for the run's principal, denied, and written as the marker.
    #[tokio::test]
    async fn a_grok_turn_dispatched_after_revocation_sends_the_marker() {
        let grok = grok().await;
        grok.references.access.store(false, Ordering::SeqCst);
        execute(&grok).await;
        assert_eq!(grok.references.calls.load(Ordering::SeqCst), 1);
        let prompts = grok.factory.prompts();
        assert_eq!(prompts.len(), 1);
        assert_unavailable(&prompts[0]);
        assert_handles_only("every written frame", &grok.factory.frames());
    }
}
