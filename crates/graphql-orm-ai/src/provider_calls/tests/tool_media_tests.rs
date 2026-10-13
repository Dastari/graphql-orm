//! Temporary SQLite owner/provenance tests; no consumer application or external endpoint.
use super::*;
use crate::{AiToolImageFieldBindings, AiToolResultMediaLimits, AiToolResultMediaPolicy};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

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
    opens: AtomicUsize,
    allowed: AtomicBool,
    revoke_before_delivery: AtomicBool,
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
        Ok(ToolGraphqlResponse {
            data: json!({"Preview":{
                "Reference":reference.reference(),"Sha256":reference.sha256(),"Mime":reference.mime(),
                "Width":reference.width(),"Height":reference.height(),"CapturedAtMs":reference.captured_at_ms(),
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
            || capture.origin.original_invocation()
                != context.execution_provenance().original_invocation()
            || descriptor.fingerprint != capture.origin.tool_fingerprint()
        {
            return Err(AiError::Conflict);
        }
        self.opens.fetch_add(1, Ordering::SeqCst);
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
        AiEgressDecision::allow(
            manifest,
            "preview-policy-v1",
            principal.principal().subject(),
        )
    }
    async fn authorize_tool_sources(
        &self,
        principal: &ResolvedPrincipal,
        manifest: &AiEgressManifest,
        sources: &[crate::AiToolResultDisclosureContext],
    ) -> AiEgressDecision {
        if self.allowed.load(Ordering::SeqCst)
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
        MockProvider::new(Vec::new()).capabilities()
    }
    fn supports_tool_result_images(&self) -> bool {
        self.supported
    }
    fn supports_tool_result_image_policy(&self, _: &AiToolDescriptor) -> bool {
        self.supported
    }
    async fn stream(
        &self,
        _: ModelRequest,
        _: ProviderRequestContext,
    ) -> Result<crate::ProviderEventStream, ProviderError> {
        Err(ProviderError::Rejected)
    }
    async fn stream_with_dynamic_tools(
        &self,
        request: ModelRequest,
        _: ProviderRequestContext,
        responder: Arc<dyn ProviderDynamicToolResponder>,
    ) -> Result<crate::ProviderEventStream, ProviderError> {
        let definition = request.tools.first().ok_or(ProviderError::Rejected)?;
        let call = ProviderDynamicToolCall::from_definition(
            "preview-turn",
            "preview-call",
            definition,
            json!({}),
        )
        .unwrap();
        let result = responder.respond(call).await?;
        if self.custody.revoke_before_delivery.load(Ordering::SeqCst) {
            self.custody.allowed.store(false, Ordering::SeqCst);
        }
        responder.authorize_delivery(&result).await?;
        if result.images().len() != 1 || result.images()[0].bytes().is_empty() {
            return Err(ProviderError::Rejected);
        }
        Ok(Box::pin(futures_util::stream::iter([
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
    let mut fixture = fixture(Vec::new()).await;
    let custody = Arc::new(Custody::default());
    custody.allowed.store(true, Ordering::SeqCst);
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
            rule.clone(),
            [(
                "Preview".into(),
                AiDisclosureShape::object(
                    rule.clone(),
                    fields
                        .into_iter()
                        .map(|name| (name.into(), AiDisclosureShape::scalar(rule.clone()))),
                ),
            )],
        ),
    )
    .unwrap();
    let document = "query Preview { Preview { Reference Sha256 Mime Width Height CapturedAtMs RetainedUntilMs ActionExpiresAtMs } }";
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
        AiToolResultMediaLimits::new(1, 4 * 1024 * 1024, 1536, 4_000_000, 300_000, 5000).unwrap(),
        DataClassification::Restricted,
    )
    .unwrap();
    let descriptor = AiToolDescriptor::new(
        "records.read",
        "Read a bounded retained preview",
        AiToolOperationKind::Query,
        document,
        json!({"type":"object","properties":{},"additionalProperties":false}),
    )
    .unwrap()
    .with_result_projection("preview-projection-v1")
    .with_graphql_contract(contract)
    .with_result_media(media)
    .unwrap();
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
    let runtime = AiRuntime::builder()
        .principal_resolver(Arc::new(CountingResolver {
            principal: fixture.principal.clone(),
            resolutions: fixture.principal_resolutions.clone(),
        }))
        .access_policy(Arc::new(AllowAccess))
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
        .content_protector(Arc::new(DatabaseManagedContentProtector))
        .provider(Arc::new(MediaProvider {
            custody: custody.clone(),
            supported,
        }))
        .unwrap()
        .build()
        .unwrap();
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
    plan.request.tools[0].parameters =
        json!({"type":"object","properties":{},"additionalProperties":false});
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
    let reopened = service
        .authorize_tool_images(call.lease(), call.id(), call.egress_manifest().unwrap())
        .await
        .unwrap();
    assert_eq!(reopened.len(), 1);
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
    assert!(
        !serde_json::to_string(&saved.protected_result)
            .unwrap()
            .contains("image/png")
    );
}
#[tokio::test]
async fn absent_image_capacity_and_unsupported_adapter_refuse_before_effect() {
    for (supported, units) in [(true, 0), (false, 4)] {
        let (fixture, custody, service) = media_fixture(supported).await;
        assert!(preview_turn(&fixture, service, units).await.is_err());
        assert_eq!(custody.effects.load(Ordering::SeqCst), 0);
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
