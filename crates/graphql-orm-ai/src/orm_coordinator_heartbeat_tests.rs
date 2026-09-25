// Provider-held persistence must keep making progress during renewal.
use super::*;

#[derive(Default)]
struct WriterGate {
    writer: tokio::sync::Mutex<()>,
    provider_started: AtomicBool,
    heartbeat_started: tokio::sync::Notify,
    heartbeat_finished: AtomicBool,
}

struct WriterRunControl(Arc<WriterGate>);
#[async_trait]
impl AiAgentRunControl for WriterRunControl {
    async fn start(&self, lease: &AiRunLease) -> Result<AiRunLease, AiError> {
        Ok(lease.clone())
    }
    async fn heartbeat(&self, lease: &AiRunLease) -> Result<AiRunLease, AiError> {
        self.0.heartbeat_started.notify_one();
        let _writer = self.0.writer.lock().await;
        tokio::task::yield_now().await;
        self.0.heartbeat_finished.store(true, Ordering::SeqCst);
        Ok(lease.clone())
    }
    async fn wait_for_cancellation(
        &self,
        _: &AiRunLease,
        _: std::time::Duration,
    ) -> Result<Option<AiRunCancellation>, AiError> {
        while !self.0.provider_started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        Ok(None)
    }
    async fn finish(&self, _: &AiRunLease, _: AiRunCompletion) -> Result<(), AiError> {
        Ok(())
    }
}

struct WriterProvider(Arc<WriterGate>);
impl WriterProvider {
    async fn finish_writer(&self, lease: &AiRunLease) -> Result<AiProviderCallResult, AiError> {
        let writer = self.0.writer.lock().await;
        self.0.provider_started.store(true, Ordering::SeqCst);
        self.0.heartbeat_started.notified().await;
        // The provider must be polled to release the simulated DB transaction.
        drop(writer);
        Ok(AiProviderCallResult::test_result(
            lease,
            None,
            "writer-response",
            vec![],
        ))
    }
}
#[async_trait]
impl AiAgentProviderTurnExecutor for WriterProvider {
    async fn execute_turn(
        &self,
        lease: &AiRunLease,
        _: AiProviderCallPlan,
    ) -> Result<AiProviderCallResult, AiError> {
        self.finish_writer(lease).await
    }
    async fn execute_dynamic_turn(
        &self,
        lease: Arc<tokio::sync::Mutex<AiRunLease>>,
        _: AiProviderCallPlan,
        _: Arc<dyn AiProviderDynamicToolExecution>,
    ) -> Result<AiProviderCallResult, AiError> {
        let snapshot = lease.lock().await.clone();
        self.finish_writer(&snapshot).await
    }
    async fn execute_retained_turn(
        &self,
        lease: Arc<tokio::sync::Mutex<AiRunLease>>,
        _: AiProviderCallPlan,
        _: crate::AiProviderSessionTurnPlan,
        _: Arc<dyn crate::AiProviderSessionService>,
        _: Option<Arc<dyn AiProviderDynamicToolExecution>>,
    ) -> Result<AiProviderCallResult, AiError> {
        let snapshot = lease.lock().await.clone();
        self.finish_writer(&snapshot).await
    }
}
struct NoDynamicTools;
#[async_trait]
impl AiProviderDynamicToolExecution for NoDynamicTools {
    async fn execute_dynamic_tool(
        &self,
        _: &AiRunLease,
        _: &AiProviderCallResult,
        _: usize,
    ) -> Result<AiPersistedApplicationToolCall, AiError> {
        panic!("heartbeat regression must not execute application tools")
    }
}

async fn writer_heartbeat_case(mode: u8) {
    let gate = Arc::new(WriterGate::default());
    let forbidden = Arc::new(ChatForbiddenBoundaries::default());
    let coordinator = AiReadOnlyAgentCoordinator::new(
        Arc::new(WriterRunControl(gate.clone())),
        Arc::new(WriterProvider(gate.clone())),
        forbidden.clone(),
        Arc::new(TestOutputWriter),
        forbidden,
        Arc::new(TestCheckpointWriter),
        Arc::new(TestRuleResolver),
        Arc::new(TestChatPlanner {
            scope: test_scope(),
            continuation_count: AtomicUsize::new(0),
        }),
        limits(1),
    );
    let mut lease = AiRunLease::test_running(principal_reference());
    let plan = AiProviderCallPlan::test_chat_plan(&lease, test_scope());
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        let result = match mode {
            0 => {
                coordinator
                    .execute_provider_with_heartbeats(&mut lease, plan)
                    .await
            }
            1 => {
                coordinator
                    .execute_dynamic_provider_with_heartbeats(
                        &mut lease,
                        plan,
                        Arc::new(NoDynamicTools),
                    )
                    .await
            }
            _ => {
                coordinator
                    .execute_retained_provider_with_heartbeats(
                        &mut lease,
                        plan,
                        crate::AiProviderSessionTurnPlan::new(
                            retained_descriptor(),
                            "c".repeat(64),
                        )
                        .unwrap(),
                        Arc::new(InterruptSessionService::default()),
                        None,
                    )
                    .await
            }
        };
        assert!(
            result.is_ok(),
            "provider-held persistence must finish during heartbeat"
        );
    })
    .await
    .expect("heartbeat must not suspend its own database writer");
    assert!(
        gate.heartbeat_finished.load(Ordering::SeqCst),
        "a started renewal must settle even if the provider completes first"
    );
}
#[tokio::test]
async fn stateless_heartbeat_keeps_provider_writer_polled() {
    writer_heartbeat_case(0).await;
}
#[tokio::test]
async fn dynamic_heartbeat_keeps_provider_writer_polled() {
    writer_heartbeat_case(1).await;
}
#[tokio::test]
async fn retained_heartbeat_keeps_provider_writer_polled() {
    writer_heartbeat_case(2).await;
}
