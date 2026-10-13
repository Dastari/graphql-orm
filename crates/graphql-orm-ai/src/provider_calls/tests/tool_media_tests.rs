//! Temporary SQLite owner/provenance tests; no consumer application or external endpoint.
use super::*;
use crate::{AiToolImageFieldBindings, AiToolResultMediaLimits, AiToolResultMediaPolicy};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[path = "static_offering_media_tests.rs"]
mod static_offering_tests;

#[derive(Clone)]
struct Capture {
    reference: crate::AiToolImageReference,
    origin: crate::AiToolExecutionProvenance,
    bytes: Arc<[u8]>,
}
#[derive(Default)]
struct Custody {
    captures: Mutex<BTreeMap<Uuid, Capture>>,
    effects: AtomicUsize,
    created_threads: AtomicUsize,
    complete_after_capture: AtomicBool,
    opens: AtomicUsize,
    allowed: AtomicBool,
    ordinary_allowed: AtomicBool,
    revoke_before_delivery: AtomicBool,
    fail_next_source_open: AtomicBool,
    invalid_image_metadata: AtomicBool,
    model_disclosures: AtomicUsize,
    deny_model_disclosure_at: AtomicUsize,
    requests: Mutex<Vec<ModelRequest>>,
    delivered_images: AtomicUsize,
    delay_open: AtomicBool,
    source_delivery_allowed: AtomicBool,
    withdraw_access_after_effect: AtomicUsize,
    access_withdrawn: AtomicBool,
    #[cfg(feature = "provider-codex-app-server")]
    legacy_factory_launches: AtomicUsize,
}
#[async_trait]
impl crate::AiAccessPolicy for Custody {
    async fn can_access_scope(
        &self,
        _principal: &agql_auth::AuthPrincipal,
        _scope: &AiScope,
        _action: crate::AiSessionAction,
    ) -> crate::AiAccessDecision {
        if self.access_withdrawn.load(Ordering::SeqCst)
            && self.withdraw_access_after_effect.load(Ordering::SeqCst) == 1
        {
            crate::AiAccessDecision::deny("scope_withdrawn", "preview-access-v1")
        } else {
            crate::AiAccessDecision::allow("current_scope", "preview-access-v1")
        }
    }
    async fn can_access_session(
        &self,
        _principal: &agql_auth::AuthPrincipal,
        _session: AiSessionId,
        _action: crate::AiSessionAction,
    ) -> crate::AiAccessDecision {
        if self.access_withdrawn.load(Ordering::SeqCst)
            && self.withdraw_access_after_effect.load(Ordering::SeqCst) == 2
        {
            crate::AiAccessDecision::deny("session_withdrawn", "preview-access-v1")
        } else {
            crate::AiAccessDecision::allow("current_session", "preview-access-v1")
        }
    }
}
#[cfg(feature = "provider-codex-app-server")]
struct LegacyDynamicFactory(Arc<Custody>);
#[cfg(feature = "provider-codex-app-server")]
#[async_trait]
impl crate::providers::AiCodexAppServerRunProcessFactory for LegacyDynamicFactory {
    fn supports_launch_profile(&self, _: crate::providers::AiCodexAppServerLaunchProfile) -> bool {
        true
    }
    async fn launch(
        &self,
        _: Arc<crate::providers::AiCodexAppServerRegistration>,
    ) -> Result<crate::providers::AiCodexAppServerLaunchedProcess, ProviderError> {
        self.0
            .legacy_factory_launches
            .fetch_add(1, Ordering::SeqCst);
        Err(ProviderError::Unavailable)
    }
}
#[cfg(feature = "provider-codex-app-server")]
fn legacy_dynamic_provider(
    custody: Arc<Custody>,
) -> Arc<crate::providers::AiCodexAppServerProvider> {
    use crate::providers::*;
    let registration = AiCodexAppServerRegistration::new(
        "mock-profile",
        "mock-model",
        "a".repeat(64),
        "1.0.0",
        "isolated-test",
        AI_CODEX_APP_SERVER_PROTOCOL_V2,
    )
    .unwrap()
    .with_launch_profile(
        AiCodexAppServerLaunchProfile::experimental_dynamic_tools_only_v1(
            AiCodexAppServerModelToolMode::Direct,
        )
        .unwrap(),
    )
    .with_guarded_tool_result_source_delivery();
    Arc::new(AiCodexAppServerProvider::new(
        Arc::new(registration),
        AiCodexAppServerRunPool::new(
            Arc::new(LegacyDynamicFactory(custody)),
            AiCodexAppServerRunLimits::default(),
        ),
    ))
}
struct SourceProtector(Arc<Custody>);
#[async_trait]
impl crate::AiContentProtector for SourceProtector {
    async fn protect(
        &self,
        policy: &crate::AiContentProtectionPolicy,
        context: &crate::ContentProtectionContext,
        value: serde_json::Value,
    ) -> Result<crate::ProtectedContentEnvelope, crate::ContentProtectionError> {
        crate::AiContentProtector::protect(&DatabaseManagedContentProtector, policy, context, value)
            .await
    }
    async fn open(
        &self,
        policy: &crate::AiContentProtectionPolicy,
        context: &crate::ContentProtectionContext,
        envelope: &crate::ProtectedContentEnvelope,
    ) -> Result<serde_json::Value, crate::ContentProtectionError> {
        if context.field == "protected_result_source"
            && self.0.fail_next_source_open.swap(false, Ordering::SeqCst)
        {
            return Err(crate::ContentProtectionError::KeyUnavailable);
        }
        crate::AiContentProtector::open(&DatabaseManagedContentProtector, policy, context, envelope)
            .await
    }
}
struct PreviewExecutor(Arc<Custody>);
#[async_trait]
impl AuthenticatedGraphqlExecutor for PreviewExecutor {
    async fn execute(
        &self,
        _context: GraphqlRequestContext,
        _request: ToolGraphqlRequest,
    ) -> Result<ToolGraphqlResponse, ToolExecutionError> {
        Err(ToolExecutionError::StaleContract)
    }
    async fn execute_registered(
        &self,
        _context: GraphqlRequestContext,
        binding: &crate::AiRegisteredToolExecutionBinding,
        _request: ToolGraphqlRequest,
    ) -> Result<ToolGraphqlResponse, ToolExecutionError> {
        let origin = binding
            .provenance()
            .ok_or(ToolExecutionError::StaleContract)?
            .clone();
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 3)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let bytes: Arc<[u8]> = bytes.into_inner().into();
        let reference_id = Uuid::new_v4();
        let now_ms =
            i64::try_from(OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000).unwrap();
        let metadata = json!({"reference":reference_id,"sha256":format!("{:x}",Sha256::digest(&bytes)),
            "mime":"image/png","width":2,"height":3,"captured_at_ms":now_ms,
            "action_expires_at_ms":now_ms+10,"retained_until_ms":now_ms+300_000});
        let reference: crate::AiToolImageReference = serde_json::from_value(metadata).unwrap();
        self.0.captures.lock().await.insert(
            reference_id,
            Capture {
                reference: reference.clone(),
                origin,
                bytes,
            },
        );
        self.0.effects.fetch_add(1, Ordering::SeqCst);
        if self.0.withdraw_access_after_effect.load(Ordering::SeqCst) != 0 {
            // The real resolver completes its effect while current disclosure access is lost.
            self.0.access_withdrawn.store(true, Ordering::SeqCst);
        }
        Ok(ToolGraphqlResponse {
            data: json!({"Preview":{
                "Reference":reference.reference(),"Sha256":reference.sha256(),"Mime":reference.mime(),
                "Width":if self.0.invalid_image_metadata.load(Ordering::SeqCst) {json!(-1)} else {json!(reference.width())},"Height":reference.height(),"CapturedAtMs":reference.captured_at_ms(),
                "RetainedUntilMs":reference.retained_until_ms(),"ActionExpiresAtMs":reference.action_expires_at_ms()
            }}),
            error_codes: Vec::new(),
            application_audit_ref: Some("preview-effect".into()),
        })
    }
}
#[async_trait]
impl crate::AiToolResultMediaResolver for Custody {
    async fn resolve_for_provider(
        &self,
        principal: &ResolvedPrincipal,
        descriptor: &AiToolDescriptor,
        context: &crate::AiToolResultDisclosureContext,
        reference: &crate::AiToolImageReference,
    ) -> Result<crate::AiResolvedToolImage, AiError> {
        if !self.allowed.load(Ordering::SeqCst)
            || principal.reference() != context.current_principal_reference()
            || principal.principal().subject() != context.original_principal_reference().subject
        {
            return Err(AiError::Forbidden);
        }
        let capture = self
            .captures
            .lock()
            .await
            .get(&reference.reference())
            .cloned()
            .ok_or(AiError::NotFound)?;
        if capture.reference != *reference
            || &capture.origin != context.execution_provenance()
            || descriptor.fingerprint != capture.origin.tool_fingerprint()
        {
            return Err(AiError::Conflict);
        }
        self.opens.fetch_add(1, Ordering::SeqCst);
        if self.delay_open.load(Ordering::SeqCst) {
            // Deliberately model a host callback that returns late without yielding. A timeout
            // future alone can observe its ready value before an elapsed timer is polled.
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        crate::AiResolvedToolImage::decode_bounded(
            reference.clone(),
            crate::AiToolMediaResourceId::new("preview-v1", reference.reference())?,
            capture.bytes,
            descriptor
                .result_media
                .as_ref()
                .ok_or(AiError::Forbidden)?
                .limits(),
        )
        .await
    }
}
#[async_trait]
impl AiEgressPolicy for Custody {
    async fn authorize(
        &self,
        principal: &ResolvedPrincipal,
        manifest: &AiEgressManifest,
    ) -> AiEgressDecision {
        if self.ordinary_allowed.load(Ordering::SeqCst) {
            AiEgressDecision::allow(
                manifest,
                "preview-policy-v1",
                principal.principal().subject(),
            )
        } else {
            AiEgressDecision::deny(
                manifest,
                crate::AiEgressReason::PolicyDenied,
                "preview-policy-v1",
                principal.principal().subject(),
            )
        }
    }
    async fn authorize_tool_sources(
        &self,
        principal: &ResolvedPrincipal,
        manifest: &AiEgressManifest,
        sources: &[crate::AiToolResultDisclosureContext],
    ) -> AiEgressDecision {
        if sources.is_empty() {
            // Withheld output has no media/source consent to admit; ordinary current
            // egress policy still governs the exact saved-task/status projection.
            return self.authorize(principal, manifest).await;
        }
        let model_denied = manifest.capability == AiEgressCapability::ModelInference
            && self.deny_model_disclosure_at.load(Ordering::SeqCst) > 0
            && self.model_disclosures.fetch_add(1, Ordering::SeqCst) + 1
                >= self.deny_model_disclosure_at.load(Ordering::SeqCst);
        if self.allowed.load(Ordering::SeqCst)
            && !model_denied
            && sources.iter().all(|source| {
                source.current_principal_reference() == principal.reference()
                    && source.scope() == &manifest.scope
                    && source.execution_provenance().tool_fingerprint().len() == 64
            })
        {
            AiEgressDecision::allow(
                manifest,
                "preview-current-consent-v1",
                principal.principal().subject(),
            )
        } else {
            AiEgressDecision::deny(
                manifest,
                crate::AiEgressReason::PolicyDenied,
                "preview-current-consent-v1",
                principal.principal().subject(),
            )
        }
    }
}
struct MediaProvider {
    custody: Arc<Custody>,
    supported: bool,
}
#[async_trait]
impl crate::AiProvider for MediaProvider {
    fn provider_kind(&self) -> ProviderKind {
        ProviderKind::OpenAiCompatible
    }
    fn capabilities(&self) -> crate::ProviderCapabilities {
        crate::ProviderCapabilities {
            image_input: self.supported,
            provider_retained_continuation: true,
            ..MockProvider::new(Vec::new()).capabilities()
        }
    }
    fn supports_tool_result_images(&self) -> bool {
        self.supported
    }
    fn supports_tool_result_source_delivery(&self) -> bool {
        self.supported && self.custody.source_delivery_allowed.load(Ordering::SeqCst)
    }
    fn supports_tool_result_image_policy(&self, _: &AiToolDescriptor) -> bool {
        self.supported
    }
    async fn create_empty_session(
        &self,
        _: &crate::AiProviderRunBinding,
        _: &AiProviderSessionDescriptor,
        _: &ModelRequest,
    ) -> Result<AiProviderSessionCursor, ProviderError> {
        AiProviderSessionCursor::new(
            "test.thread",
            format!(
                "preview-thread-{}",
                self.custody.created_threads.fetch_add(1, Ordering::SeqCst)
            ),
        )
        .map_err(|_| ProviderError::InvalidRequest)
    }
    async fn stream(
        &self,
        request: ModelRequest,
        context: ProviderRequestContext,
    ) -> Result<crate::ProviderEventStream, ProviderError> {
        context.authorize_tool_source_delivery().await?;
        self.custody.delivered_images.fetch_add(
            context.retained_tool_result_images().len(),
            Ordering::SeqCst,
        );
        self.custody.requests.lock().await.push(request.clone());
        if request.tools.is_empty() || self.custody.complete_after_capture.load(Ordering::SeqCst) {
            return Ok(Box::pin(futures::stream::iter([
                Ok(ProviderEvent::TextDelta {
                    text: "The saved task is complete.".into(),
                }),
                Ok(ProviderEvent::Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cached_input_tokens: 0,
                }),
                Ok(ProviderEvent::ResponseCompleted {
                    response_id: Some("fresh-text-turn".into()),
                }),
            ])));
        }
        let definition = request.tools.first().ok_or(ProviderError::Rejected)?;
        Ok(Box::pin(futures::stream::iter([
            Ok(ProviderEvent::ResponseStarted {
                response_id: Some("preview-turn".into()),
            }),
            Ok(ProviderEvent::ToolCallStarted {
                call_id: "preview-call".into(),
                tool_id: definition.tool_id.clone(),
            }),
            Ok(ProviderEvent::ToolCallCompleted {
                call_id: "preview-call".into(),
                arguments: json!({}),
            }),
            Ok(ProviderEvent::Usage {
                input_tokens: 1,
                output_tokens: 1,
                cached_input_tokens: 0,
            }),
            Ok(ProviderEvent::ResponseCompleted {
                response_id: Some("preview-turn".into()),
            }),
        ])))
    }
    async fn stream_with_dynamic_tools(
        &self,
        request: ModelRequest,
        context: ProviderRequestContext,
        responder: Arc<dyn ProviderDynamicToolResponder>,
    ) -> Result<crate::ProviderEventStream, ProviderError> {
        context.authorize_tool_source_delivery().await?;
        if self.custody.complete_after_capture.load(Ordering::SeqCst) {
            return self.stream(request, context).await;
        }
        let definition = request
            .tools
            .iter()
            .find(|definition| definition.tool_id == "records.read")
            .ok_or(ProviderError::Rejected)?
            .clone();
        let custody = self.custody.clone();
        // Real callbacks occur while polling the dispatched stream, after the SDK has
        // durably marked its atomic reservation uncertain. Do not execute effects while
        // merely preparing/returning the stream.
        Ok(Box::pin(async_stream::try_stream! {
            yield ProviderEvent::ResponseStarted { response_id: Some("preview-turn".into()) };
            yield ProviderEvent::ToolCallStarted { call_id: "preview-call".into(), tool_id: definition.tool_id.clone() };
            let call = ProviderDynamicToolCall::from_definition("preview-turn", "preview-call", &definition, json!({}))?;
            let result = responder.respond(call).await?;
            if custody.revoke_before_delivery.load(Ordering::SeqCst) {
                custody.allowed.store(false, Ordering::SeqCst);
            }
            responder.authorize_delivery(&result).await?;
            if result.output()["status"] != "ApprovalPending"
                && (result.images().len() != 1 || result.images()[0].bytes().is_empty()) {
                Err::<(), _>(ProviderError::Rejected)?;
            }
            yield ProviderEvent::ToolCallCompleted { call_id: "preview-call".into(), arguments: json!({}) };
            yield ProviderEvent::Usage { input_tokens: 1, output_tokens: 1, cached_input_tokens: 0 };
            yield ProviderEvent::ResponseCompleted { response_id: Some("preview-turn".into()) };
        }))
    }
}
struct ImageAccounting;
#[async_trait]
impl AiProviderUsageAccounting for ImageAccounting {
    async fn settle(
        &self,
        observation: &AiProviderUsageObservation,
    ) -> Result<AiBudgetAmounts, AiError> {
        Ok(AiBudgetAmounts {
            input_tokens: observation.input_tokens(),
            output_tokens: observation.output_tokens(),
            image_units: observation.tool_result_image_units(),
            runs: 1,
            ..Default::default()
        })
    }
}
struct PreviewExecution {
    service: Arc<OrmAiApplicationToolCallService>,
    scope: AiScope,
}
#[async_trait]
impl AiProviderDynamicToolExecution for PreviewExecution {
    fn tool_result_source_service(&self) -> Option<&OrmAiApplicationToolCallService> {
        Some(self.service.as_ref())
    }
    async fn execute_dynamic_tool(
        &self,
        lease: &AiRunLease,
        result: &AiProviderCallResult,
        index: usize,
    ) -> Result<AiPersistedApplicationToolCall, AiError> {
        self.service
            .execute_read_only(
                lease,
                result,
                AiApplicationToolCallContext::new(
                    0,
                    index,
                    self.scope.clone(),
                    "preview-test",
                    "preview-turn",
                )?,
                preview_route(),
            )
            .await
    }
}
fn preview_route() -> AiToolResultEgressRoute {
    AiToolResultEgressRoute::new(
        "mock-profile",
        "local-mock",
        AiDestinationTrust::ManagedProvider,
        "preview_test",
        "none",
        "preview-policy-v1",
    )
    .unwrap()
}
fn boundary(classification: DataClassification, attachments: u32) -> AiDeploymentEgressBoundary {
    AiDeploymentEgressBoundary {
        allowed_destination_trust: [AiDestinationTrust::ManagedProvider].into_iter().collect(),
        allowed_capabilities: [
            AiEgressCapability::ModelInference,
            AiEgressCapability::ToolResult,
            AiEgressCapability::ImageAnalysis,
        ]
        .into_iter()
        .collect(),
        maximum_classification: classification,
        maximum_bytes: 16 * 1024 * 1024,
        maximum_attachments: attachments,
    }
}
async fn media_fixture(
    supported: bool,
) -> (Fixture, Arc<Custody>, Arc<OrmAiApplicationToolCallService>) {
    media_fixture_mode(supported, false).await
}
async fn media_fixture_mode(
    supported: bool,
    supervised: bool,
) -> (Fixture, Arc<Custody>, Arc<OrmAiApplicationToolCallService>) {
    media_fixture_resolution(supported, supervised, 5000).await
}
async fn media_fixture_resolution(
    supported: bool,
    supervised: bool,
    resolution_ms: u64,
) -> (Fixture, Arc<Custody>, Arc<OrmAiApplicationToolCallService>) {
    let mut fixture = fixture(Vec::new()).await;
    let custody = Arc::new(Custody::default());
    custody.allowed.store(true, Ordering::SeqCst);
    custody.ordinary_allowed.store(true, Ordering::SeqCst);
    custody
        .source_delivery_allowed
        .store(true, Ordering::SeqCst);
    let rule = AiDisclosureRule::exportable(DataClassification::Restricted);
    let fields = [
        "Reference",
        "Sha256",
        "Mime",
        "Width",
        "Height",
        "CapturedAtMs",
        "RetainedUntilMs",
        "ActionExpiresAtMs",
    ];
    let disclosure = AiDisclosureSchema::new(
        "preview-disclosure-v1",
        AiDisclosureShape::object(
            rule,
            [(
                "Preview".into(),
                AiDisclosureShape::object(
                    rule,
                    fields
                        .into_iter()
                        .map(|name| (name.into(), AiDisclosureShape::scalar(rule))),
                ),
            )],
        ),
    )
    .unwrap();
    let document = if supervised {
        "mutation Preview { Preview { Reference Sha256 Mime Width Height CapturedAtMs RetainedUntilMs ActionExpiresAtMs } }"
    } else {
        "query Preview { Preview { Reference Sha256 Mime Width Height CapturedAtMs RetainedUntilMs ActionExpiresAtMs } }"
    };
    let contract = GraphqlOperationContract::new(
        GraphqlExecutionTargetId::parse("local-application").unwrap(),
        "schema-v1",
        "Preview",
        document,
        "preview-projection-v1",
        disclosure.fingerprint.clone(),
    )
    .unwrap();
    let media = AiToolResultMediaPolicy::new(
        "preview-v1",
        vec![vec!["Preview".into()]],
        AiToolImageFieldBindings::new(
            "Reference",
            "Sha256",
            "Mime",
            "Width",
            "Height",
            "CapturedAtMs",
            "RetainedUntilMs",
        )
        .unwrap()
        .with_action_expires_at_ms("ActionExpiresAtMs")
        .unwrap(),
        AiToolResultMediaLimits::new(1, 4 * 1024 * 1024, 1536, 4_000_000, 300_000, resolution_ms)
            .unwrap(),
        DataClassification::Restricted,
    )
    .unwrap();
    let descriptor = AiToolDescriptor::new(
        "records.read",
        "Read a bounded retained preview",
        if supervised { AiToolOperationKind::Mutation } else { AiToolOperationKind::Query },
        document,
        json!({"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{},"additionalProperties":false}),
    )
    .unwrap()
    .with_result_projection("preview-projection-v1")
    .with_graphql_contract(contract)
    .with_result_media(media)
    .unwrap();
    let descriptor = if supervised {
        descriptor
            .with_maturity(ToolMaturity::SupervisedWrite)
            .with_risk(AiToolRisk::NonIdempotentWrite, AiApprovalRule::OneShot)
            .with_idempotent(false)
    } else {
        descriptor
    };
    let scoped = crate::AiToolResultEgressBoundary::new(
        descriptor.fingerprint.clone(),
        "mock-profile",
        "local-mock",
        boundary(DataClassification::Restricted, 4),
    )
    .unwrap();
    let mut catalog = AiToolCatalog::new();
    catalog
        .register_with_disclosure(descriptor, disclosure)
        .unwrap();
    let mut targets = GraphqlExecutionTargetRegistry::new();
    targets
        .register(GraphqlExecutionTarget {
            id: GraphqlExecutionTargetId::parse("local-application").unwrap(),
            class: GraphqlExecutionTargetClass::Local,
            audience: None,
            resource_type: None,
            resource_id: None,
            schema_fingerprint: "schema-v1".into(),
        })
        .unwrap();
    let builder = AiRuntime::builder()
        .maximum_tool_maturity(ToolMaturity::SupervisedWrite)
        .principal_resolver(Arc::new(CountingResolver {
            principal: fixture.principal.clone(),
            resolutions: fixture.principal_resolutions.clone(),
        }))
        .access_policy(custody.clone())
        .tool_authorization_policy(Arc::new(AllowTools(fixture.tool_policy_version.clone())))
        .request_context_factory(Arc::new(ContextFactory))
        .graphql_executor(Arc::new(PreviewExecutor(custody.clone())))
        .graphql_targets(targets)
        .egress_policy(custody.clone())
        .deployment_egress(boundary(DataClassification::Internal, 0))
        .tool_result_egress_boundary(scoped)
        .tool_result_media_resolver("preview-v1", custody.clone())
        .unwrap()
        .tool_catalog(catalog)
        .secret_store(Arc::new(EnvironmentSecretStore::new()))
        .content_protection_policy_resolver(Arc::new(ProtectionPolicy))
        .content_protector(Arc::new(SourceProtector(custody.clone())))
        .provider(Arc::new(MediaProvider {
            custody: custody.clone(),
            supported,
        }))
        .unwrap();
    #[cfg(feature = "provider-codex-app-server")]
    let builder = builder
        .provider(legacy_dynamic_provider(custody.clone()))
        .unwrap();
    let runtime = builder.build().unwrap();
    runtime
        .start_gate()
        .open(&AiRuntimeReadinessReport {
            module_fingerprint: runtime.start_gate().expected_module_fingerprint().into(),
            executor_bound: true,
            restore_reconciled: true,
            fatal_issue_count: 0,
        })
        .unwrap();
    fixture.runtime = Arc::new(runtime);
    let service = Arc::new(OrmAiApplicationToolCallService::new(
        fixture.run_service.clone(),
        fixture.runtime.clone(),
        fixture.audit.clone(),
        Arc::new(SystemClock),
        AiApplicationToolCallLimits::new(
            8192,
            16384,
            4,
            4,
            Duration::seconds(30),
            Duration::seconds(10),
        )
        .unwrap(),
    ));
    (fixture, custody, service)
}
async fn preview_turn(
    fixture: &Fixture,
    service: Arc<OrmAiApplicationToolCallService>,
    image_units: u64,
) -> Result<AiProviderCallResult, AiError> {
    let mut plan = tool_plan(fixture);
    plan.budget.estimate.image_units = image_units;
    let executor = AiProviderCallExecutor::new(
        fixture.runtime.clone(),
        fixture.budget_service.clone(),
        fixture.audit.clone(),
        Arc::new(ImageAccounting),
        Arc::new(SystemClock),
        AiProviderCallLimits::new(64, 8192, 65536).unwrap(),
    );
    executor
        .execute_with_dynamic_tools(
            Arc::new(Mutex::new(fixture.lease.clone())),
            plan,
            Arc::new(PreviewExecution {
                service,
                scope: fixture.scope.clone(),
            }),
        )
        .await
}
#[tokio::test]
async fn typed_pixels_charge_atomic_budget_and_reopen_without_effect_replay() {
    let (fixture, custody, service) = media_fixture(true).await;
    let result = preview_turn(&fixture, service.clone(), 4).await.unwrap();
    assert_eq!(result.usage().image_units, 1);
    let call = &result.interactive_tool_results()[0];
    let source = service
        .reopen_tool_result_source(
            call.lease(),
            call.id(),
            crate::AiToolResultDisclosurePurpose::History,
        )
        .await
        .unwrap();
    assert_eq!(
        source.execution_provenance().argument_hash(),
        crate::tools::canonical_json_digest(&json!({}))
    );
    assert_eq!(source.current_attempt_id(), fixture.lease.attempt_id());
    assert_eq!(source.effect_attempt_id(), fixture.lease.attempt_id());
    assert_eq!(
        source.execution_provenance().effect_attempt_id(),
        Some(fixture.lease.attempt_id())
    );
    assert_eq!(
        source.execution_provenance().effect_lease_generation(),
        Some(fixture.lease.lease_generation())
    );
    let reopened = service
        .authorize_tool_images(call.lease(), call.id(), call.egress_manifest().unwrap())
        .await
        .unwrap();
    assert_eq!(reopened.len(), 1);
    assert!(source.binds_egress_source(&source.data_source()));
    let image_source = source.images()[0].egress_source(&source, reopened[0].byte_count());
    assert!(source.binds_egress_source(&image_source));
    for replacement in [
        AiDataSourceRef {
            reference: "v2:unknown".into(),
            ..image_source.clone()
        },
        AiDataSourceRef {
            kind: "unknown_image".into(),
            ..image_source.clone()
        },
        AiDataSourceRef {
            classification: DataClassification::Internal,
            ..image_source.clone()
        },
        AiDataSourceRef {
            trust: AiSourceTrust::UserProvided,
            ..image_source.clone()
        },
    ] {
        assert!(!source.binds_egress_source(&replacement));
    }
    assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
    let reservation =
        AiBudgetReservationRecord::find_by_id(&fixture.database, &result.budget_reservation_id().0)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(reservation.state, "committed");
    assert_eq!(reservation.actual_image_units, Some(1));
    let saved = AiToolCallRecord::find_by_id(&fixture.database, &call.id().0)
        .await
        .unwrap()
        .unwrap();
    assert!(saved.protected_result_source.is_some());
    // Database-managed protection deliberately retains structured metadata; actual pixels stay
    // in the owning custody and are never inserted into the durable JSON result.
    let json = serde_json::to_string(&saved.protected_result).unwrap();
    assert!(!json.contains("data:image"));
    let captures = custody.captures.lock().await;
    let capture = captures.values().next().unwrap();
    assert!(!json.contains(&hex::encode(&capture.bytes)));
}
#[tokio::test]
async fn ordinary_attachments_and_reopened_pixels_share_complete_request_limits() {
    let (fixture, custody, service) = media_fixture(true).await;
    let result = preview_turn(&fixture, service.clone(), 4).await.unwrap();
    let call = &result.interactive_tool_results()[0];
    let images = service
        .authorize_tool_images(call.lease(), call.id(), call.egress_manifest().unwrap())
        .await
        .unwrap();
    let mut request = plan(&fixture).request;
    let attachment = ModelInputBlock::Attachment {
        attachment_id: Uuid::new_v4().to_string(),
        mime: "image/png".into(),
        byte_count: images[0].byte_count(),
        sha256: images[0].reference().sha256().into(),
    };
    request.input.push(attachment.clone());
    let raw = images[0].byte_count() * 2;
    let exact = AiProviderAttachmentResolutionLimits::new(2, raw, raw).unwrap();
    assert!(validate_combined_media_batch(&request, &images, exact, 2).is_ok());
    let over_bytes = AiProviderAttachmentResolutionLimits::new(2, raw - 1, raw - 1).unwrap();
    assert!(validate_combined_media_batch(&request, &images, over_bytes, 2).is_err());
    assert!(validate_combined_media_batch(&request, &images, exact, 1).is_err());
    let over_count = AiProviderAttachmentResolutionLimits::new(1, raw, raw).unwrap();
    assert!(validate_combined_media_batch(&request, &images, over_count, 2).is_err());
    request
        .input
        .extend([attachment.clone(), attachment.clone(), attachment]);
    assert!(
        validate_combined_media_batch(
            &request,
            &images,
            AiProviderAttachmentResolutionLimits::default(),
            5
        )
        .is_err()
    );
    assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn absent_image_capacity_and_unsupported_adapter_refuse_before_effect() {
    for (supported, units) in [(true, 0), (false, 4)] {
        let (fixture, custody, service) = media_fixture(supported).await;
        assert!(preview_turn(&fixture, service, units).await.is_err());
        assert_eq!(custody.effects.load(Ordering::SeqCst), 0);
    }
}

#[cfg(feature = "provider-codex-app-server")]
#[tokio::test]
async fn legacy_codex_dynamic_factory_refuses_protected_tool_before_launch_or_effect() {
    let (fixture, custody, service) = media_fixture(true).await;
    let mut plan = tool_plan(&fixture);
    plan.provider_kind = ProviderKind::LocalHarness;
    plan.budget.provider_kind = ProviderKind::LocalHarness;
    plan.budget.estimate.image_units = 4;
    for transfer in &mut plan.transfers {
        transfer.provider_kind = ProviderKind::LocalHarness.as_str().into();
    }
    let plan = AiProviderCallPlan::new_with_tools(
        plan.provider_kind,
        plan.request,
        plan.budget,
        plan.transfers,
        plan.correlation_id,
        fixture.runtime.tool_catalog(),
        &static_read_policy(&fixture),
    )
    .unwrap();
    let executor = AiProviderCallExecutor::new(
        fixture.runtime.clone(),
        fixture.budget_service.clone(),
        fixture.audit.clone(),
        Arc::new(ImageAccounting),
        Arc::new(SystemClock),
        AiProviderCallLimits::new(64, 8192, 65536).unwrap(),
    );
    let result = executor
        .execute_with_dynamic_tools(
            Arc::new(Mutex::new(fixture.lease.clone())),
            plan,
            Arc::new(PreviewExecution {
                service,
                scope: fixture.scope.clone(),
            }),
        )
        .await;
    assert!(matches!(result, Err(AiError::PreTransportProviderFailed)));
    assert_eq!(custody.legacy_factory_launches.load(Ordering::SeqCst), 0);
    assert_eq!(custody.effects.load(Ordering::SeqCst), 0);
    assert!(custody.captures.lock().await.is_empty());
    let budgets = AiBudgetReservationRecord::query(fixture.database.pool())
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(budgets.len(), 1);
    assert_eq!(budgets[0].state, "released");

    // The legacy factory remains usable for ordinary text. Its intentional launch failure
    // proves the protected refusal above happened earlier than the factory transport seam.
    let mut ordinary = super::plan(&fixture);
    ordinary.provider_kind = ProviderKind::LocalHarness;
    ordinary.budget.provider_kind = ProviderKind::LocalHarness;
    ordinary.budget.idempotency_key = "legacy-ordinary-text".into();
    ordinary.request.continuation_mode = ModelContinuationMode::StatelessReplay;
    for transfer in &mut ordinary.transfers {
        transfer.provider_kind = ProviderKind::LocalHarness.as_str().into();
    }
    let ordinary = AiProviderCallPlan::new(
        ordinary.provider_kind,
        ordinary.request,
        ordinary.budget,
        ordinary.transfers,
        ordinary.correlation_id,
    )
    .unwrap();
    assert!(executor.execute(&fixture.lease, ordinary).await.is_err());
    assert_eq!(custody.legacy_factory_launches.load(Ordering::SeqCst), 1);
    assert_eq!(custody.effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn image_capable_adapter_without_current_source_delivery_refuses_before_effect() {
    let (fixture, custody, service) = media_fixture(true).await;
    custody
        .source_delivery_allowed
        .store(false, Ordering::SeqCst);
    assert!(preview_turn(&fixture, service, 4).await.is_err());
    assert_eq!(custody.effects.load(Ordering::SeqCst), 0);
    assert!(custody.captures.lock().await.is_empty());
}

#[tokio::test]
async fn absent_or_changed_retained_effect_fence_is_refused_without_reexecution() {
    for omit in [true, false] {
        let (fixture, custody, service) = media_fixture(true).await;
        let result = preview_turn(&fixture, service.clone(), 4).await.unwrap();
        let call = &result.interactive_tool_results()[0];
        let saved = AiToolCallRecord::find_by_id(&fixture.database, &call.id().0)
            .await
            .unwrap()
            .unwrap();
        let mut envelope: crate::ProtectedContentEnvelope =
            serde_json::from_value(saved.protected_result_source.clone().unwrap()).unwrap();
        let crate::ProtectedContentEnvelope::DatabaseManaged { value } = &mut envelope else {
            unreachable!()
        };
        if omit {
            value["execution"]
                .as_object_mut()
                .unwrap()
                .remove("effect_attempt_id");
            value["execution"]
                .as_object_mut()
                .unwrap()
                .remove("effect_lease_generation");
        } else {
            value["execution"]["effect_attempt_id"] = json!(Uuid::new_v4());
        }
        AiToolCallRecord::compare_and_swap(
            &fixture.database,
            &saved.id,
            saved.row_version,
            AiToolCallRecordWhereInput::default(),
            UpdateAiToolCallRecordInput {
                protected_result_source: Some(Some(serde_json::to_value(envelope).unwrap())),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(
            service
                .reopen_tool_result_source(
                    call.lease(),
                    call.id(),
                    crate::AiToolResultDisclosurePurpose::Continuation
                )
                .await
                .is_err()
        );
        assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
    }
}
#[tokio::test]
async fn current_consent_revocation_before_wire_keeps_effect_result_and_uncertain_budget() {
    let (fixture, custody, service) = media_fixture(true).await;
    custody.revoke_before_delivery.store(true, Ordering::SeqCst);
    assert!(preview_turn(&fixture, service.clone(), 4).await.is_err());
    assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
    let sources = AiToolResultSourceRecord::query(fixture.database.pool())
        .limit(2)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(sources.len(), 1);
    let call = AiToolCallRecord::find_by_id(&fixture.database, &sources[0].id)
        .await
        .unwrap()
        .unwrap();
    assert!(call.protected_result_source.is_some());
    assert!(call.protected_result.is_some());
    let budget = AiBudgetReservationRecord::find_by_id(
        &fixture.database,
        &call.budget_reservation_id.unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(budget.state, "uncertain");
    assert_eq!(budget.actual_image_units, None);
    assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn retained_origin_body_and_mixed_source_boundary_cannot_be_forged() {
    let (fixture, custody, service) = media_fixture(true).await;
    let result = preview_turn(&fixture, service.clone(), 4).await.unwrap();
    let call = &result.interactive_tool_results()[0];
    let source = service
        .reopen_tool_result_source(
            call.lease(),
            call.id(),
            crate::AiToolResultDisclosurePurpose::History,
        )
        .await
        .unwrap();
    let mut manifest = call.egress_manifest().unwrap().clone();
    manifest.sources.push(AiDataSourceRef {
        kind: "caller_marker".into(),
        reference: source.source_reference(),
        classification: DataClassification::Restricted,
        trust: AiSourceTrust::ResolverResult,
    });
    assert!(
        fixture
            .runtime
            .authorize_tool_source_egress(
                call.lease().principal_reference(),
                &manifest,
                std::slice::from_ref(&source)
            )
            .await
            .unwrap()
            .authorize(&manifest)
            .is_err()
    );
    let saved = AiToolCallRecord::find_by_id(&fixture.database, &call.id().0)
        .await
        .unwrap()
        .unwrap();
    AiToolCallRecord::compare_and_swap(
        &fixture.database,
        &saved.id,
        saved.row_version,
        AiToolCallRecordWhereInput::default(),
        UpdateAiToolCallRecordInput {
            argument_hash: Some("0".repeat(64)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        service
            .reopen_tool_result_source(
                call.lease(),
                call.id(),
                crate::AiToolResultDisclosurePurpose::History
            )
            .await
            .is_err()
    );
    assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn delivery_history_race_denies_protected_sources_but_preserves_general_denials() {
    let (fixture, custody, service) = media_fixture(true).await;
    let result = preview_turn(&fixture, service.clone(), 4).await.unwrap();
    let call = &result.interactive_tool_results()[0];
    let mut manifest = call.egress_manifest().unwrap().clone();
    manifest.capability = AiEgressCapability::ModelInference;
    let delivery = ToolSourceDelivery::new(
        service,
        Arc::new(Mutex::new(call.lease().clone())),
        &[manifest],
        4,
        None,
    )
    .unwrap()
    .unwrap();
    delivery.authorize().await.unwrap();
    custody.allowed.store(false, Ordering::SeqCst);
    assert!(matches!(
        delivery.authorize().await,
        Err(ProviderError::ToolSourceUnavailable)
    ));
    custody.ordinary_allowed.store(false, Ordering::SeqCst);
    assert!(matches!(
        delivery.authorize().await,
        Err(ProviderError::EgressDenied)
    ));
    assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn post_effect_source_open_failure_retains_actual_output_for_later_read_without_reexecution()
{
    let (fixture, custody, service) = media_fixture(true).await;
    custody.fail_next_source_open.store(true, Ordering::SeqCst);
    assert!(preview_turn(&fixture, service.clone(), 4).await.is_err());
    let sources = AiToolResultSourceRecord::query(fixture.database.pool())
        .limit(2)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(sources.len(), 1);
    let call = AiToolCallRecord::find_by_id(&fixture.database, &sources[0].id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.state, "source_retained");
    assert!(call.protected_result.is_some());
    assert!(call.protected_result_source.is_some());
    let actual_run = AiRunRecord::find_by_id(&fixture.database, &fixture.lease.run_id().0)
        .await
        .unwrap()
        .unwrap();
    let current = crate::orm_runs::lease_from_record(&actual_run).unwrap();
    let context = service
        .reopen_tool_result_source(
            &current,
            AiToolCallId(call.id),
            crate::AiToolResultDisclosurePurpose::Continuation,
        )
        .await
        .unwrap();
    let images = service
        .reopen_tool_images(&current, &context)
        .await
        .unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn invalid_post_effect_media_is_saved_but_cannot_be_admitted_or_replayed() {
    let (fixture, custody, service) = media_fixture(true).await;
    custody.invalid_image_metadata.store(true, Ordering::SeqCst);
    assert!(preview_turn(&fixture, service.clone(), 4).await.is_err());
    let calls = AiToolCallRecord::query(fixture.database.pool())
        .limit(2)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].protected_result.is_some());
    assert!(calls[0].protected_result_source.is_none());
    assert_eq!(calls[0].state, "executing");
    assert!(
        AiToolResultSourceRecord::query(fixture.database.pool())
            .limit(1)
            .fetch_all()
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        service
            .reopen_tool_result_source(
                &fixture.lease,
                AiToolCallId(calls[0].id),
                crate::AiToolResultDisclosurePurpose::Continuation
            )
            .await
            .is_err()
    );
    assert!(preview_turn(&fixture, service, 4).await.is_err());
    assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
    assert_eq!(custody.opens.load(Ordering::SeqCst), 0);
}

struct MediaActionPreview;
struct NativePreviewExecution {
    application: Arc<OrmAiApplicationToolCallService>,
    consequential: Arc<OrmAiConsequentialToolCallService>,
    scope: AiScope,
    prepared: Mutex<Option<crate::AiPreparedNativeApproval>>,
    last_lease: Mutex<Option<AiRunLease>>,
}
#[async_trait]
impl AiProviderDynamicToolExecution for NativePreviewExecution {
    fn tool_result_source_service(&self) -> Option<&OrmAiApplicationToolCallService> {
        Some(&self.application)
    }
    async fn execute_dynamic_tool(
        &self,
        _: &AiRunLease,
        _: &AiProviderCallResult,
        _: usize,
    ) -> Result<AiPersistedApplicationToolCall, AiError> {
        Err(AiError::Forbidden)
    }
    async fn execute_dynamic_outcome(
        &self,
        lease: &AiRunLease,
        result: &AiProviderCallResult,
        index: usize,
    ) -> Result<AiProviderDynamicToolOutcome, AiError> {
        let prepared = self
            .consequential
            .prepare_native_approval(
                lease,
                result,
                AiApplicationToolCallContext::new(
                    0,
                    index,
                    self.scope.clone(),
                    "approved-media-test",
                    result.budget_reservation_id().0.to_string(),
                )?,
            )
            .await?;
        *self.last_lease.lock().await = Some(prepared.lease().clone());
        let receipt = self
            .consequential
            .prepare_native_approval_receipt(prepared.lease(), &prepared, &preview_route())
            .await?;
        *self.last_lease.lock().await = Some(receipt.lease().clone());
        *self.prepared.lock().await = Some(prepared);
        Ok(AiProviderDynamicToolOutcome::NativeControl(Box::new(
            receipt,
        )))
    }
    async fn failed_callback_lease(&self) -> Option<AiRunLease> {
        self.last_lease.lock().await.clone()
    }
}
#[tokio::test]
async fn late_owning_image_resolution_is_discarded_and_saved_effect_is_not_repeated() {
    let (fixture, custody, service) = media_fixture_resolution(true, false, 25).await;
    custody.delay_open.store(true, Ordering::SeqCst);
    assert!(preview_turn(&fixture, service, 4).await.is_err());
    assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
    let call = AiToolCallRecord::query(fixture.database.pool())
        .limit(1)
        .fetch_one()
        .await
        .unwrap()
        .unwrap();
    assert!(call.protected_result.is_some());
    assert!(call.protected_result_source.is_some());
    assert_eq!(custody.delivered_images.load(Ordering::SeqCst), 0);
}

#[async_trait]
impl AiCanonicalActionPreviewBuilder for MediaActionPreview {
    async fn build_preview(
        &self,
        _: &ResolvedPrincipal,
        _: &AiToolDescriptor,
        _: &ToolGraphqlRequest,
    ) -> Result<AiCanonicalActionPreview, AiError> {
        Ok(AiCanonicalActionPreview {
            action_kind: "capture_preview".into(),
            title: "Capture one authorized preview".into(),
            targets: vec![AiApprovalResourceBinding {
                resource_type: "preview".into(),
                resource_id: "original-resource".into(),
                expected_version: "1".into(),
            }],
            details: json!({"capture":true}),
        })
    }
}

async fn media_input_and_sessions(fixture: &Fixture) -> Arc<OrmAiProviderSessionService> {
    // Initial fixture input uses generated ORM entities; all lifecycle transitions below use
    // the owning session/approval/run services, never direct writes to their state fields.
    let session = AiSessionRecord::find_by_id(&fixture.database, &fixture.lease.session_id().0)
        .await
        .unwrap()
        .unwrap();
    AiSessionRecord::compare_and_swap(
        &fixture.database,
        &session.id,
        session.row_version,
        AiSessionRecordWhereInput::default(),
        UpdateAiSessionRecordInput {
            message_head: Some(1),
            stream_head: Some(1),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    AiMessageRecord::insert(
        &fixture.database,
        CreateAiMessageRecordInput {
            id: fixture.lease.input_message_id(),
            session_id: session.id,
            sequence: 1,
            message_role: "user".into(),
            author_principal_kind: Some("user".into()),
            author_subject: Some(fixture.principal.subject().into()),
            client_message_id: Some(Uuid::new_v4()),
            content_hash: Some("c".repeat(64)),
            run_id: Some(fixture.lease.run_id().0),
            provider_kind: None,
            provider_model: None,
            protected_preview: None,
            block_count: 1,
            completion_state: "complete".into(),
            finalized_at: Some(OffsetDateTime::now_utc().unix_timestamp()),
            content_purged_at: None,
        },
    )
    .await
    .unwrap();
    let message = AiMessageRecord::find_by_id(&fixture.database, &fixture.lease.input_message_id())
        .await
        .unwrap()
        .unwrap();
    let event_id = Uuid::new_v4();
    AiSessionEventRecord::insert(
        &fixture.database,
        crate::persistence::CreateAiSessionEventRecordInput {
            id: event_id,
            session_id: session.id,
            sequence: 1,
            event_type: "message_queued".into(),
            run_id: Some(fixture.lease.run_id().0),
            causation_id: Some(message.client_message_id.unwrap().to_string()),
            correlation_id: message.client_message_id.unwrap().to_string(),
            protected_payload: serde_json::to_value(
                crate::ProtectedContentEnvelope::DatabaseManaged {
                    value: json!({"messageId":message.id,"runId":fixture.lease.run_id().0}),
                },
            )
            .unwrap(),
        },
    )
    .await
    .unwrap();
    let block_id = Uuid::new_v4();
    let policy = ProtectionPolicy
        .resolve(&fixture.principal, &fixture.scope)
        .await
        .unwrap();
    let envelope = crate::AiContentProtector::protect(
        &DatabaseManagedContentProtector,
        &policy,
        &crate::ContentProtectionContext {
            entity: "graphql_orm_ai_message_blocks".into(),
            row_id: block_id.to_string(),
            field: "protected_content".into(),
            scope: fixture.scope.clone(),
        },
        json!({"text":"hello"}),
    )
    .await
    .unwrap();
    AiMessageBlockRecord::insert(
        &fixture.database,
        CreateAiMessageBlockRecordInput {
            id: block_id,
            message_id: fixture.lease.input_message_id(),
            block_index: 0,
            block_kind: "text".into(),
            protected_content: serde_json::to_value(envelope).unwrap(),
            byte_count: 5,
            line_count: 1,
        },
    )
    .await
    .unwrap();
    Arc::new(
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
    )
}

#[tokio::test]
async fn approved_media_effect_reopens_under_new_lease_without_consuming_or_executing_again() {
    approved_media_lifecycle(false, false, 0, None).await;
}

#[tokio::test]
async fn native_history_normal_deny_after_preflight_withholds_saved_effect_and_releases_unstarted_budget()
 {
    approved_media_lifecycle(true, false, 0, None).await;
}

#[tokio::test]
async fn approved_effect_with_invalid_media_keeps_output_and_enters_recovery_without_replay() {
    approved_media_lifecycle(false, true, 0, None).await;
}

#[tokio::test]
async fn approved_completed_effect_survives_mid_effect_scope_and_session_withdrawal() {
    for withdrawal in [1, 2] {
        approved_media_lifecycle(false, false, withdrawal, None).await;
    }
}

async fn approved_media_lifecycle(
    deny_history: bool,
    invalid_metadata: bool,
    withdrawal: usize,
    reset_mode: Option<usize>,
) {
    let (mut fixture, custody, application) = media_fixture_mode(true, true).await;
    if reset_mode.is_some_and(|mode| mode < 8) {
        fixture.budget_service = static_offering_tests::extended_task_budget_service(&fixture);
        let policy = AiBudgetPolicyRecord::query(fixture.database.pool())
            .limit(1)
            .fetch_one()
            .await
            .unwrap()
            .unwrap();
        AiBudgetPolicyRecord::update_by_id(
            &fixture.database,
            &policy.id,
            crate::persistence::UpdateAiBudgetPolicyRecordInput {
                maximum_input_tokens: Some(Some(20_000)),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    let descriptor = fixture
        .runtime
        .tool_catalog()
        .descriptor(&AiToolId::parse("records.read").unwrap())
        .unwrap();
    let mut base = plan(&fixture);
    base.budget.estimate.image_units = 4;
    base.request.tools = vec![ModelToolDefinition {
        tool_id: descriptor.id.as_str().into(),
        provider_name: "records_read".into(),
        fingerprint: descriptor.fingerprint.clone(),
        description: descriptor.description.clone(),
        parameters: descriptor.argument_schema.clone(),
        strict: true,
        defer_loading: false,
    }];
    base.transfers[0].estimated_bytes = base.request.conservative_egress_bytes();
    let mut policy = AiToolPolicySet::new(ToolMaturity::SupervisedWrite);
    policy.bind(AiToolPolicyBinding {
        tool_id: descriptor.id.clone(),
        fingerprint: descriptor.fingerprint.clone(),
        enabled: true,
    });
    let coordinator_delivery = reset_mode.filter(|mode| (8..=14).contains(mode)).map(|_| {
        super::native_coordinator_tests::mixed_broker_delivery(&fixture, base.request.tools.clone())
    });
    let offered = if let Some(delivery) = &coordinator_delivery {
        base.request.tools = delivery.current_tools();
        base.budget.estimate.input_tokens = 8000;
        base.transfers[0].estimated_bytes = base.request.conservative_egress_bytes();
        AiProviderCallPlan::new_with_classified_capability_surface(
            base.provider_kind,
            base.request,
            base.budget,
            base.transfers,
            "approved-media-test",
            &delivery.current_surface(),
            fixture.runtime.tool_catalog(),
            &policy,
            &fixture.generated_target_policy,
        )
    } else {
        AiProviderCallPlan::new_with_classified_tools(
            base.provider_kind,
            base.request,
            base.budget,
            base.transfers,
            "approved-media-test",
            fixture.runtime.tool_catalog(),
            &policy,
            &fixture.generated_target_policy,
        )
    }
    .unwrap();
    let executor = AiProviderCallExecutor::new(
        fixture.runtime.clone(),
        fixture.budget_service.clone(),
        fixture.audit.clone(),
        Arc::new(ImageAccounting),
        Arc::new(SystemClock),
        AiProviderCallLimits::new(64, 8192, 65536).unwrap(),
    );
    let sessions = media_input_and_sessions(&fixture).await;
    assert_eq!(custody.effects.load(Ordering::SeqCst), 0);
    let checkpoints = Arc::new(OrmAiCoordinatorCheckpointService::new(
        fixture.run_service.clone(),
        Arc::new(Resolver(fixture.principal.clone())),
        Arc::new(AllowAccess),
        Arc::new(ProtectionPolicy),
        Arc::new(DatabaseManagedContentProtector),
        Arc::new(NativeTestRuleResolver),
        Arc::new(SystemClock),
        AiCoordinatorCheckpointLimits::new(256 * 1024, Duration::minutes(5)).unwrap(),
    ));
    let (_, approvals) = consequential_test_service(&fixture);
    let consequential = Arc::new(
        OrmAiConsequentialToolCallService::new(
            fixture.run_service.clone(),
            fixture.runtime.clone(),
            approvals.clone(),
            Arc::new(MediaActionPreview),
            fixture.audit.clone(),
            Arc::new(SystemClock),
            AiApplicationToolCallLimits::new(
                8192,
                16384,
                4,
                4,
                Duration::seconds(30),
                Duration::seconds(10),
            )
            .unwrap(),
        )
        .with_provider_session_service(sessions.clone()),
    );
    let native = Arc::new(NativePreviewExecution {
        application: application.clone(),
        consequential: consequential.clone(),
        scope: fixture.scope.clone(),
        prepared: Mutex::new(None),
        last_lease: Mutex::new(None),
    });
    let active = Arc::new(Mutex::new(fixture.lease.clone()));
    let selected = AiProviderSessionDescriptor::new(
        ProviderKind::OpenAiCompatible,
        "mock-profile",
        "mock-model",
        coordinator_delivery.as_ref().map_or_else(
            || "a".repeat(64),
            |delivery| delivery.session_binding().fingerprint().to_owned(),
        ),
        "mock/v1",
        "b".repeat(64),
    )
    .unwrap();
    let provider = executor
        .with_tool_result_source_service(application.clone())
        .unwrap()
        .execute_with_provider_session(
            active.clone(),
            offered,
            crate::AiProviderSessionTurnPlan::new(selected, "d".repeat(64)).unwrap(),
            sessions.clone(),
            Some(native.clone()),
        )
        .await
        .unwrap();
    let prepared = native.prepared.lock().await.take().unwrap();
    let active = active.lock().await.clone();
    let (_, usage) = test_rule_checkpoint(&fixture.scope, &[&provider], 1);
    let protected = checkpoints
        .persist_native_approval_provider_turn(
            &active,
            &provider,
            &prepared,
            &fixture.scope,
            "approved-media-test",
            &preview_route(),
            &native_test_rules(fixture.scope.clone()),
            usage,
            1,
            1,
        )
        .await
        .unwrap();
    let requested = consequential
        .finalize_prepared_approval(
            &protected,
            &provider,
            &prepared,
            OffsetDateTime::now_utc() + Duration::minutes(5),
            false,
        )
        .await
        .unwrap();
    assert_eq!(requested.lease().state(), AiRunState::WaitingApproval);
    assert_eq!(custody.effects.load(Ordering::SeqCst), 0);
    let pending = AiApprovalRecord::find_by_id(&fixture.database, &requested.approval_id().0)
        .await
        .unwrap()
        .unwrap();
    approvals
        .decide_approval(
            &fixture.principal,
            DecideAiApprovalInput {
                id: pending.id,
                decision: AiApprovalDecision::Approve,
                expected_version: pending.row_version,
            },
        )
        .await
        .unwrap();
    let claim = fixture
        .run_service
        .claim_next_approved("approved-media-resume")
        .await
        .unwrap()
        .unwrap();
    assert!(claim.lease().lease_generation() > fixture.lease.lease_generation());
    let resume = OrmAiSupervisedResumeService::new(
        fixture.run_service.clone(),
        checkpoints.clone(),
        consequential.clone(),
    );
    if let Some(delivery) = coordinator_delivery {
        Box::pin(static_offering_tests::owning_coordinator_reset(
            fixture,
            custody,
            application,
            sessions,
            checkpoints,
            consequential,
            Arc::new(resume),
            delivery,
            claim,
            reset_mode.unwrap(),
        ))
        .await;
        return;
    }
    custody
        .invalid_image_metadata
        .store(invalid_metadata, Ordering::SeqCst);
    custody
        .withdraw_access_after_effect
        .store(withdrawal, Ordering::SeqCst);
    let completed = resume.execute_claimed(&claim).await.unwrap();
    if invalid_metadata || withdrawal != 0 {
        assert!(matches!(
            completed,
            crate::AiSupervisedResumeOutcome::RecoveryRequired { .. }
        ));
        let call = AiToolCallRecord::find_by_id(&fixture.database, &requested.tool_call_id().0)
            .await
            .unwrap()
            .unwrap();
        assert!(call.protected_result.is_some());
        assert!(call.protected_result_source.is_none());
        if withdrawal != 0 {
            let envelope: crate::ProtectedContentEnvelope =
                serde_json::from_value(call.protected_result.clone().unwrap()).unwrap();
            let crate::ProtectedContentEnvelope::DatabaseManaged { value } = envelope else {
                unreachable!()
            };
            let captures = custody.captures.lock().await;
            let capture = captures.values().next().unwrap();
            assert_eq!(
                value["data"]["Preview"]["Reference"],
                json!(capture.reference.reference())
            );
            assert_eq!(
                value["data"]["Preview"]["Sha256"],
                capture.reference.sha256()
            );
            assert!(custody.access_withdrawn.load(Ordering::SeqCst));
            assert_eq!(custody.opens.load(Ordering::SeqCst), 0);
            assert_eq!(custody.model_disclosures.load(Ordering::SeqCst), 0);
            assert!(
                AiToolResultSourceRecord::query(fixture.database.pool())
                    .fetch_all()
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
        let run = AiRunRecord::find_by_id(&fixture.database, &claim.lease().run_id().0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run.state, "recovery_required");
        assert!(resume.execute_claimed(&claim).await.is_err());
        assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
        assert_eq!(custody.delivered_images.load(Ordering::SeqCst), 0);
        return;
    }
    let current = completed.checkpointed().unwrap().lease();
    let source = application
        .reopen_tool_result_source(
            current,
            requested.tool_call_id(),
            crate::AiToolResultDisclosurePurpose::Continuation,
        )
        .await
        .unwrap();
    assert_eq!(
        source.execution_provenance().attempt_id(),
        fixture.lease.attempt_id()
    );
    assert_eq!(source.effect_attempt_id(), claim.lease().attempt_id());
    assert_eq!(
        source.execution_provenance().effect_attempt_id(),
        Some(claim.lease().attempt_id())
    );
    assert_eq!(
        source.execution_provenance().effect_lease_generation(),
        Some(claim.lease().lease_generation())
    );
    assert_eq!(
        source.effect_lease_generation(),
        claim.lease().lease_generation()
    );
    assert_eq!(
        source.execution_provenance().approval_id(),
        Some(requested.approval_id())
    );
    assert_eq!(source.current_attempt_id(), claim.lease().attempt_id());
    assert_eq!(
        application
            .reopen_tool_images(current, &source)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
    assert!(resume.execute_claimed(&claim).await.is_err());
    let consumed = AiApprovalRecord::find_by_id(&fixture.database, &pending.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(consumed.consumed_uses, 1);
    assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
    let adopted = checkpoints
        .adopt_classified_mutation_batch(current)
        .await
        .unwrap()
        .unwrap();
    let crate::AiAdoptedClassifiedMutationBatch::Supervised(adopted) = adopted else {
        panic!("approved effect must retain its supervised checkpoint")
    };
    let current = checkpoints
        .consume_supervised_before_provider(current, &adopted)
        .await
        .unwrap();
    if reset_mode == Some(15) {
        continuation_context_state::assert_native_context_states(
            &fixture,
            &custody,
            application.as_ref(),
            &current,
            adopted.continuation(),
        )
        .await;
        return;
    }
    if let Some(mode) = reset_mode {
        exercise_owned_static_reset(
            &fixture,
            &custody,
            application,
            sessions,
            native,
            &current,
            &adopted,
            mode,
        )
        .await;
        return;
    }
    let mut next = plan(&fixture);
    next.budget.attempt_id = current.attempt_id();
    next.budget.lease_generation = current.lease_generation();
    next.budget.idempotency_key = "approved-media-history".into();
    next.budget.estimate.image_units = 4;
    custody.requests.lock().await.clear();
    if deny_history {
        custody.deny_model_disclosure_at.store(2, Ordering::SeqCst);
    }
    let executor = AiProviderCallExecutor::new(
        fixture.runtime.clone(),
        fixture.budget_service.clone(),
        fixture.audit.clone(),
        Arc::new(ImageAccounting),
        Arc::new(SystemClock),
        AiProviderCallLimits::new(64, 8192, 65536).unwrap(),
    )
    .with_tool_result_source_service(application.clone())
    .unwrap();
    let selected = AiProviderSessionDescriptor::new(
        ProviderKind::OpenAiCompatible,
        "mock-profile",
        "mock-model",
        "a".repeat(64),
        "mock/v1",
        "b".repeat(64),
    )
    .unwrap();
    let continued = executor
        .execute_with_provider_session(
            Arc::new(Mutex::new(current)),
            next,
            crate::AiProviderSessionTurnPlan::new(selected, "d".repeat(64)).unwrap(),
            sessions,
            None,
        )
        .await
        .unwrap();
    assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
    let sent = custody.requests.lock().await;
    assert_eq!(sent.len(), 1);
    if deny_history {
        assert_eq!(custody.model_disclosures.load(Ordering::SeqCst), 2);
        assert_eq!(continued.usage().image_units, 0);
        assert_eq!(custody.delivered_images.load(Ordering::SeqCst), 0);
        assert!(sent[0].tools.is_empty());
        assert!(sent[0].continuation.is_none());
        assert!(matches!(&sent[0].input[0],ModelInputBlock::Text{text} if text=="hello"));
        let serialized = serde_json::to_string(&sent[0].input).unwrap();
        assert!(serialized.contains("historicalToolContentWithheld"));
        assert!(!serialized.contains("Preview"));
        let budgets = AiBudgetReservationRecord::query(fixture.database.pool())
            .fetch_all()
            .await
            .unwrap();
        assert_eq!(
            budgets.iter().filter(|row| row.state == "released").count(),
            1
        );
        assert_eq!(
            budgets
                .iter()
                .filter(|row| row.state == "committed")
                .count(),
            2
        );
        assert!(budgets.iter().all(|row| row.state != "uncertain"));
        assert_eq!(
            budgets
                .iter()
                .find(|row| row.id == continued.budget_reservation_id().0)
                .unwrap()
                .reserved_image_units,
            0
        );
    } else {
        assert_eq!(continued.usage().image_units, 1);
        assert_eq!(custody.delivered_images.load(Ordering::SeqCst), 1);
        let inference = continued.model_inference_manifest();
        assert_eq!(inference.attachment_count, 1);
        assert_eq!(
            inference
                .sources
                .iter()
                .filter(|source| source.kind == "tool_result_image")
                .count(),
            1
        );
        let raw = custody
            .captures
            .lock()
            .await
            .values()
            .next()
            .unwrap()
            .bytes
            .len() as u64;
        assert!(inference.estimated_bytes >= raw.div_ceil(3) * 4 + 1024);
    }
}

#[tokio::test]
async fn ordinary_text_with_zero_image_budget_does_not_offer_or_execute_media() {
    let (fixture, custody, _) = media_fixture(false).await;
    let executor = AiProviderCallExecutor::new(
        fixture.runtime.clone(),
        fixture.budget_service.clone(),
        fixture.audit.clone(),
        Arc::new(ImageAccounting),
        Arc::new(SystemClock),
        AiProviderCallLimits::new(64, 8192, 65536).unwrap(),
    );
    let result = executor
        .execute(&fixture.lease, plan(&fixture))
        .await
        .unwrap();
    assert_eq!(result.usage().image_units, 0);
    assert_eq!(custody.effects.load(Ordering::SeqCst), 0);
    assert_eq!(custody.delivered_images.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn owner_cancellation_closes_source_retained_call_and_preserves_effect_custody() {
    let (fixture, custody, application) = media_fixture(true).await;
    custody.fail_next_source_open.store(true, Ordering::SeqCst);
    assert!(
        preview_turn(&fixture, application.clone(), 4)
            .await
            .is_err()
    );
    let call = AiToolCallRecord::query(fixture.database.pool())
        .limit(1)
        .fetch_one()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.state, "source_retained");
    let cancellation = crate::OrmAiRunCancellationService::new(
        fixture.database.clone(),
        Arc::new(AllowAccess),
        Arc::new(ProtectionPolicy),
        Arc::new(DatabaseManagedContentProtector),
        Arc::new(Resolver(fixture.principal.clone())),
        Arc::new(SystemClock),
        crate::AiRunCancellationLimits::default(),
        Arc::new(crate::AiRunCancellationHub::default()),
    );
    cancellation
        .request_cancellation(
            &fixture.principal,
            crate::CancelAiRunInput {
                session_id: fixture.lease.session_id().0,
                run_id: fixture.lease.run_id().0,
                client_request_id: Uuid::new_v4(),
            },
        )
        .await
        .unwrap();
    let cancelled = AiToolCallRecord::find_by_id(&fixture.database, &call.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cancelled.state, "cancelled");
    assert_eq!(cancelled.protected_result, call.protected_result);
    assert_eq!(
        cancelled.protected_result_source,
        call.protected_result_source
    );
    assert!(
        application
            .reopen_tool_result_source(
                &fixture.lease,
                AiToolCallId(call.id),
                crate::AiToolResultDisclosurePurpose::Continuation
            )
            .await
            .is_err()
    );
    assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn prepared_original_task_survives_a_later_queued_message_without_new_human_dispatch() {
    let (mut fixture, custody, application) = media_fixture(true).await;
    let sessions = OrmAiSessionService::new(
        fixture.database.clone(),
        Arc::new(AllowAccess),
        Arc::new(ProtectionPolicy),
        Arc::new(DatabaseManagedContentProtector),
    );
    let session = sessions
        .create_session(
            &fixture.principal,
            crate::CreateAiSessionInput {
                execution_selection: None,
                scope: crate::AiScopeInput {
                    kind: fixture.scope.kind.clone(),
                    id: fixture.scope.id.clone(),
                    tenant_id: fixture.scope.tenant_id.clone(),
                },
                title: None,
            },
        )
        .await
        .unwrap();
    let original_client_id = Uuid::new_v4();
    let original = sessions
        .send_message(
            &fixture.principal,
            crate::SendAiMessageInput {
                session_id: session.id,
                text: "Complete this exact original task".into(),
                attachment_ids: Vec::new(),
                client_message_id: original_client_id,
            },
        )
        .await
        .unwrap();
    let claimed = fixture
        .run_service
        .claim_next("original-context")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.run_id().0, original.run_id);
    fixture.lease = fixture.run_service.start(&claimed).await.unwrap();
    let before = application
        .prepare_task_continuation_context(&fixture.lease, &fixture.scope)
        .await
        .unwrap();
    let later = sessions
        .send_message(
            &fixture.principal,
            crate::SendAiMessageInput {
                session_id: session.id,
                text: "A separate later task must not replace the active task".into(),
                attachment_ids: Vec::new(),
                client_message_id: Uuid::new_v4(),
            },
        )
        .await
        .unwrap();
    assert_ne!(later.run_id, original.run_id);
    let events_before = AiSessionEventRecord::query(fixture.database.pool())
        .count()
        .await
        .unwrap();
    let after = application
        .prepare_task_continuation_context(&fixture.lease, &fixture.scope)
        .await
        .unwrap();
    assert_eq!(before.source_fingerprint, after.source_fingerprint);
    assert_eq!(before.frame, after.frame);
    application
        .validate_task_continuation_context(&fixture.lease, &fixture.scope, &before)
        .await
        .unwrap();
    let ModelInputBlock::Text { text } = &after.frame else {
        panic!("task context stays untrusted text")
    };
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(value["originalMessageId"], original.message_id.to_string());
    assert_eq!(
        value["originalClientMessageId"],
        original_client_id.to_string()
    );
    assert_eq!(
        value["goalContext"]["text"],
        "Complete this exact original task"
    );
    assert_eq!(value["newHumanEventCreated"], false);
    assert_eq!(value["priorProtectedContentIncluded"], false);
    assert_eq!(value["savedInvocationStatuses"], json!([]));
    assert_eq!(custody.effects.load(Ordering::SeqCst), 0);
    assert_eq!(
        AiSessionEventRecord::query(fixture.database.pool())
            .count()
            .await
            .unwrap(),
        events_before
    );
    assert!(
        sessions
            .delete_session(&fixture.principal, crate::AiSessionId(session.id))
            .await
            .unwrap()
    );
    assert!(
        application
            .validate_task_continuation_context(&fixture.lease, &fixture.scope, &before)
            .await
            .is_err()
    );
    assert_eq!(custody.effects.load(Ordering::SeqCst), 0);
}

struct ConfirmPreviewDeletion {
    custody: Arc<Custody>,
    fail: bool,
    requests: Mutex<Vec<String>>,
    created_at_delete: Mutex<Vec<usize>>,
}
#[async_trait]
impl crate::AiProviderSessionDeletionService for ConfirmPreviewDeletion {
    async fn delete_or_confirm_absent(
        &self,
        request: &crate::AiProviderSessionDeletionRequest,
    ) -> Result<crate::AiProviderSessionAbsenceProof, ProviderError> {
        self.requests
            .lock()
            .await
            .push(request.cursor().fingerprint());
        assert_eq!(self.custody.effects.load(Ordering::SeqCst), 1);
        self.created_at_delete
            .lock()
            .await
            .push(self.custody.created_threads.load(Ordering::SeqCst));
        if self.fail {
            return Err(ProviderError::Rejected);
        }
        Ok(crate::AiProviderSessionAbsenceProof::for_request(
            request,
            OffsetDateTime::now_utc(),
        ))
    }
}

#[tokio::test]
async fn owning_approved_static_reset_requires_confirmed_cleanup_and_current_source_without_effect_replay()
 {
    for mode in [0, 1, 2, 3, 4, 5, 6, 7] {
        Box::pin(approved_media_lifecycle(false, false, 0, Some(mode))).await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn exercise_owned_static_reset(
    fixture: &Fixture,
    custody: &Arc<Custody>,
    application: Arc<OrmAiApplicationToolCallService>,
    sessions: Arc<OrmAiProviderSessionService>,
    native: Arc<NativePreviewExecution>,
    lease: &AiRunLease,
    adopted: &crate::AiAdoptedSupervisedToolBatch,
    mode: usize,
) {
    let descriptor = fixture
        .runtime
        .tool_catalog()
        .descriptor(&AiToolId::parse("records.read").unwrap())
        .unwrap();
    let definition = ModelToolDefinition {
        tool_id: descriptor.id.as_str().into(),
        provider_name: "records_read".into(),
        fingerprint: descriptor.fingerprint.clone(),
        description: descriptor.description.clone(),
        parameters: descriptor.argument_schema.clone(),
        strict: true,
        defer_loading: false,
    };
    let delivery =
        super::native_coordinator_tests::mixed_broker_delivery(fixture, vec![definition]);
    delivery
        .set_static_bootstrap_offering(&BTreeSet::new())
        .unwrap();
    let mut policy = AiToolPolicySet::new(ToolMaturity::SupervisedWrite);
    policy.bind(AiToolPolicyBinding {
        tool_id: descriptor.id.clone(),
        fingerprint: descriptor.fingerprint.clone(),
        enabled: true,
    });
    let prepared = application
        .prepare_task_continuation_context(lease, &fixture.scope)
        .await
        .unwrap();
    let continuation = adopted
        .continuation()
        .clone()
        .with_prepared_task_context(prepared)
        .unwrap();
    let original_reference = continuation.chain_reference().unwrap();
    let mut base = plan(fixture);
    base.request.input.clear();
    base.request.tools = delivery.current_tools();
    base.budget.attempt_id = lease.attempt_id();
    base.budget.lease_generation = lease.lease_generation();
    base.budget.idempotency_key = "owned-static-reset".into();
    let mut complete_projection = base.request.clone();
    continuation
        .clone()
        .apply_with_bound_task_context(&mut complete_projection)
        .unwrap();
    base.budget.estimate.input_tokens = complete_projection
        .conservative_provider_egress_tokens(&base.provider_kind)
        .unwrap();
    base.budget.estimate.image_units = 4;
    assert!(
        AiProviderCallPlan::new_continuation_with_classified_tools(
            base.provider_kind.clone(),
            base.request.clone(),
            base.budget.clone(),
            base.transfers.clone(),
            "owned-static-reset",
            continuation.clone(),
            fixture.runtime.tool_catalog(),
            &policy,
            &fixture.generated_target_policy
        )
        .is_err(),
        "another constructor cannot drop owning prepared-context evidence"
    );
    let mut next = AiProviderCallPlan::new_continuation_with_classified_capability_surface(
        base.provider_kind,
        base.request,
        base.budget,
        base.transfers,
        "owned-static-reset",
        continuation,
        &delivery.current_surface(),
        fixture.runtime.tool_catalog(),
        &policy,
        &fixture.generated_target_policy,
    )
    .unwrap();
    let selected = AiProviderSessionDescriptor::new(
        ProviderKind::OpenAiCompatible,
        "mock-profile",
        "mock-model",
        "a".repeat(64),
        "mock/v1",
        "b".repeat(64),
    )
    .unwrap();
    let session_plan = crate::AiProviderSessionTurnPlan::new(selected, "d".repeat(64)).unwrap();
    let claim = sessions.reclaim_after_wait(lease).await.unwrap();
    let old_cursor = sessions
        .open_for_run(lease, &claim)
        .await
        .unwrap()
        .cursor()
        .fingerprint();
    let deletion = Arc::new(ConfirmPreviewDeletion {
        custody: custody.clone(),
        fail: mode == 1,
        requests: Mutex::new(Vec::new()),
        created_at_delete: Mutex::new(Vec::new()),
    });
    let executor = AiProviderCallExecutor::new(
        fixture.runtime.clone(),
        fixture.budget_service.clone(),
        fixture.audit.clone(),
        Arc::new(ImageAccounting),
        Arc::new(SystemClock),
        AiProviderCallLimits::new(64, 8192, 65536).unwrap(),
    )
    .with_tool_result_source_service(application.clone())
    .unwrap()
    .with_static_offering_reset_deletion_service(deletion.clone());
    if mode == 7 {
        let candidate =
            crate::AiUnstartedStaticOfferingReplan::prepare(&next, &delivery, &session_plan)
                .unwrap();
        assert!(candidate.ready_for_callback().is_err());
        drop(next);
        candidate.ready_for_callback().unwrap();
        assert!(
            candidate
                .validate_unstarted_cleanup(lease, sessions.as_ref(), OffsetDateTime::now_utc(),)
                .await
                .is_err(),
            "an unstarted error category cannot manufacture deletion proof"
        );
        assert!(deletion.requests.lock().await.is_empty());
        assert_eq!(custody.created_threads.load(Ordering::SeqCst), 1);
        assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
        let binding = crate::orm_provider_session::AiProviderSessionBindingRecord::find_by_id(
            &fixture.database,
            &claim.binding_id(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(binding.state, "claimed");
        assert!(binding.protected_cursor.is_some());
        return;
    }
    if mode == 3 {
        let original_task_context = next
            .request
            .input
            .iter()
            .find(|input| matches!(input, ModelInputBlock::Text { text } if text.contains("FrameworkTaskContinuationContext")))
            .unwrap()
            .clone();
        let owning_sessions = OrmAiSessionService::new(
            fixture.database.clone(),
            Arc::new(AllowAccess),
            Arc::new(ProtectionPolicy),
            Arc::new(DatabaseManagedContentProtector),
        );
        let later = owning_sessions
            .send_message(
                &fixture.principal,
                crate::SendAiMessageInput {
                    session_id: lease.session_id().0,
                    text: "A later independent task must not replace this completed effect's task"
                        .into(),
                    attachment_ids: Vec::new(),
                    client_message_id: Uuid::new_v4(),
                },
            )
            .await
            .unwrap();
        assert_ne!(later.run_id, lease.run_id().0);
        let events_before = AiSessionEventRecord::query(fixture.database.pool())
            .limit(32)
            .fetch_all()
            .await
            .unwrap();
        let event_ids_before = events_before
            .iter()
            .map(|event| event.id)
            .collect::<BTreeSet<_>>();
        let human_ids_before = events_before
            .iter()
            .filter(|event| event.event_type == "message_queued")
            .map(|event| event.id)
            .collect::<BTreeSet<_>>();
        assert_eq!(human_ids_before.len(), 2);
        // An ordinary caller cannot admit the older input; only the owning completed-effect
        // proof, attached privately by the executor, admits its exact original run context.
        assert!(
            sessions
                .disposition_for_run(lease, &session_plan)
                .await
                .is_err()
        );
        custody.requests.lock().await.clear();
        // Continue through the same owning native responder after the actual approved
        // effect; this scripted turn completes without invoking another tool.
        custody.complete_after_capture.store(true, Ordering::SeqCst);
        let result = executor
            .execute_with_provider_session(
                Arc::new(Mutex::new(lease.clone())),
                next,
                session_plan,
                sessions,
                Some(native),
            )
            .await
            .unwrap();
        assert!(result.proves_static_offering_reset_from(Some(&original_reference)));
        assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
        assert_eq!(custody.created_threads.load(Ordering::SeqCst), 2);
        assert_eq!(*deletion.created_at_delete.lock().await, vec![1]);
        assert_eq!(result.usage().image_units, 1);
        assert_eq!(custody.delivered_images.load(Ordering::SeqCst), 1);
        let approvals = AiApprovalRecord::query(fixture.database.pool())
            .fetch_all()
            .await
            .unwrap();
        assert_eq!(approvals.len(), 1);
        assert_eq!(approvals[0].consumed_uses, 1);
        let sent = custody.requests.lock().await;
        assert_eq!(sent.len(), 1);
        assert!(sent[0].input.contains(&original_task_context));
        let contents = serde_json::to_string(&sent[0].input).unwrap();
        assert!(contents.contains("FrameworkTaskContinuationContext"));
        assert!(contents.contains(&lease.input_message_id().to_string()));
        assert!(!contents.contains("A later independent task"));
        let events_after = AiSessionEventRecord::query(fixture.database.pool())
            .limit(32)
            .fetch_all()
            .await
            .unwrap();
        let human_ids_after = events_after
            .iter()
            .filter(|event| event.event_type == "message_queued")
            .map(|event| event.id)
            .collect::<BTreeSet<_>>();
        assert_eq!(human_ids_after, human_ids_before);
        assert!(
            events_before
                .iter()
                .all(|before| events_after.iter().any(|after| after.id == before.id))
        );
        let new_events = events_after
            .iter()
            .filter(|event| !event_ids_before.contains(&event.id))
            .collect::<Vec<_>>();
        assert_eq!(new_events.len(), 1);
        assert_eq!(
            new_events[0].event_type,
            crate::orm_runs::PROVIDER_SESSION_REBOUND_EVENT
        );
        assert_eq!(new_events[0].session_id, lease.session_id().0);
        assert_eq!(new_events[0].run_id, Some(lease.run_id().0));
        assert_eq!(new_events[0].correlation_id, claim.binding_id().to_string());
        return;
    }
    if mode == 6 {
        let cancellation = crate::OrmAiRunCancellationService::new(
            fixture.database.clone(),
            Arc::new(AllowAccess),
            Arc::new(ProtectionPolicy),
            Arc::new(DatabaseManagedContentProtector),
            Arc::new(Resolver(fixture.principal.clone())),
            Arc::new(SystemClock),
            crate::AiRunCancellationLimits::default(),
            Arc::new(crate::AiRunCancellationHub::default()),
        );
        cancellation
            .request_cancellation(
                &fixture.principal,
                crate::CancelAiRunInput {
                    session_id: lease.session_id().0,
                    run_id: lease.run_id().0,
                    client_request_id: Uuid::new_v4(),
                },
            )
            .await
            .unwrap();
    }
    if mode == 2 {
        custody
            .withdraw_access_after_effect
            .store(1, Ordering::SeqCst);
        custody.access_withdrawn.store(true, Ordering::SeqCst);
    }
    if mode == 4 || mode == 5 {
        if mode == 4 {
            custody.allowed.store(false, Ordering::SeqCst);
        }
        if mode == 5 {
            next.budget.estimate.image_units = 0;
        }
        custody.requests.lock().await.clear();
        // The admission preflight proves the old image is unavailable under current
        // consent/quote. The SDK performs the same protected withholding path used by
        // the executor, with confirmed deletion and real fresh retained cursor ownership.
        assert!(
            executor
                .prepare_retained_tool_history(lease, &mut next, &claim)
                .await
                .is_err()
        );
        let result = executor
            .execute_withheld_native_task_context(
                Arc::new(Mutex::new(lease.clone())),
                next,
                &session_plan,
                &(sessions.clone() as Arc<dyn crate::AiProviderSessionService>),
                &claim,
            )
            .await
            .unwrap();
        assert!(result.proves_static_offering_reset_from(Some(&original_reference)));
        assert!(result.is_withheld_native_task_context());
        assert_eq!(result.usage().image_units, 0);
        assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
        assert_eq!(custody.created_threads.load(Ordering::SeqCst), 2);
        assert_eq!(custody.delivered_images.load(Ordering::SeqCst), 0);
        assert_eq!(*deletion.created_at_delete.lock().await, vec![1]);
        let sent = custody.requests.lock().await;
        assert_eq!(sent.len(), 1);
        assert!(sent[0].tools.is_empty());
        assert!(sent[0].continuation.is_none());
        assert!(
            sent[0]
                .input
                .iter()
                .all(|input| matches!(input, ModelInputBlock::Text { .. }))
        );
        let contents = serde_json::to_string(&sent[0].input).unwrap();
        assert!(contents.contains("FrameworkTaskContinuationContext"));
        assert!(contents.contains("historicalToolContentWithheld"));
        assert!(contents.contains("invocationAlreadyCompleted"));
        assert!(!contents.contains("FrameworkApprovedToolOutcome"));
        assert!(!contents.contains("Preview"));
        assert!(!contents.contains("data:image"));
        let inference = result.model_inference_manifest();
        assert_eq!(inference.attachment_count, 0);
        assert!(inference.sources.iter().all(|source| {
            source.kind != "application_tool_result" && source.kind != "tool_result_image"
        }));
        custody.ordinary_allowed.store(false, Ordering::SeqCst);
        let denied = fixture
            .runtime
            .authorize_tool_source_egress(lease.principal_reference(), inference, &[])
            .await
            .unwrap();
        assert!(
            denied.authorize(inference).is_err(),
            "zero sources never waive ordinary policy"
        );
        assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
        let budgets = AiBudgetReservationRecord::query(fixture.database.pool())
            .fetch_all()
            .await
            .unwrap();
        let current = budgets
            .iter()
            .find(|budget| budget.id == result.budget_reservation_id().0)
            .unwrap();
        assert_eq!(current.state, "committed");
        assert_eq!(current.reserved_image_units, 0);
        return;
    }
    let result = executor
        .reset_static_offering(
            lease,
            &mut next,
            &session_plan,
            &(sessions.clone() as Arc<dyn crate::AiProviderSessionService>),
            Some(&claim),
        )
        .await;
    assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
    assert_eq!(custody.delivered_images.load(Ordering::SeqCst), 0);
    if mode == 0 {
        let (claim, cursor) = result.unwrap();
        assert_ne!(cursor.fingerprint(), old_cursor);
        assert_eq!(custody.created_threads.load(Ordering::SeqCst), 2);
        assert_eq!(*deletion.requests.lock().await, vec![old_cursor]);
        assert!(next.request.continuation.is_none());
        let receipt = next.static_offering_reset_receipt.as_ref().unwrap();
        assert_eq!(receipt.original_continuation_reference, original_reference);
        assert_eq!(receipt.replacement_binding_id, claim.binding_id());
        assert_eq!(receipt.replacement_cursor_fingerprint, cursor.fingerprint());
        assert!(
            matches!(&next.request.input[1],ModelInputBlock::Text{text} if text.contains("FrameworkTaskContinuationContext"))
        );
        let row = crate::orm_provider_session::AiProviderSessionBindingRecord::find_by_id(
            &fixture.database,
            &claim.binding_id(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(row.state, "claimed");
        assert_eq!(
            row.installed_offering_fingerprint.as_deref(),
            Some(next.offering_fingerprint().as_str())
        );
    } else {
        assert!(result.is_err());
        assert_eq!(custody.created_threads.load(Ordering::SeqCst), 1);
        let row = crate::orm_provider_session::AiProviderSessionBindingRecord::find_by_id(
            &fixture.database,
            &claim.binding_id(),
        )
        .await
        .unwrap()
        .unwrap();
        if mode == 1 {
            assert_eq!(row.state, "cleanup_in_progress");
            assert!(row.protected_cursor.is_some());
        } else {
            assert!(deletion.requests.lock().await.is_empty());
            assert_eq!(row.state, "claimed");
        }
    }
    let call = AiToolCallRecord::find_by_id(&fixture.database, &adopted.tool_call_id().0)
        .await
        .unwrap()
        .unwrap();
    assert!(call.protected_result.is_some());
    assert_eq!(call.state, "completed");
    assert_eq!(custody.effects.load(Ordering::SeqCst), 1);
}

mod continuation_context_state;
