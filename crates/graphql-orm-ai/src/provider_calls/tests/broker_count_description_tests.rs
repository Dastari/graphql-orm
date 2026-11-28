use async_graphql_parser::types::{TypeKind, TypeSystemDefinition};

use super::*;

mod surface {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "count_records",
        plural = "CountRecords",
        default_sort = "id ASC",
        aggregate = true,
        description = "Records for the count description fixture"
    )]
    pub struct CountRecord {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[filterable(type = "string")]
        #[sortable]
        pub id: String,
    }

    schema_roots! {
        entities: [CountRecord],
    }
}

const ALTERNATE_AGGREGATE: &str = "countRecordsAlternate";

#[derive(Default)]
struct CountAuthority(AtomicBool);

#[async_trait]
impl AiCapabilityAuthorityPolicy for CountAuthority {
    async fn authorize(
        &self,
        _principal: &ResolvedPrincipal,
        _owning_index: &AiCapabilityIndex,
        entry: &AiCapabilityIndexEntry,
        _run: &AiCapabilityRunBinding,
    ) -> Result<AiCapabilityAuthorityDecision, AiError> {
        Ok(AiCapabilityAuthorityDecision {
            allowed: entry.operation_name != ALTERNATE_AGGREGATE || self.0.load(Ordering::SeqCst),
            policy_fingerprint: "current-policy-v1".to_owned(),
        })
    }
}

struct CountEgress(AtomicBool);

#[async_trait]
impl AiEgressPolicy for CountEgress {
    async fn authorize(
        &self,
        principal: &ResolvedPrincipal,
        manifest: &AiEgressManifest,
    ) -> AiEgressDecision {
        let subject = principal.principal().subject();
        if manifest.capability == AiEgressCapability::ToolResult && !self.0.load(Ordering::SeqCst) {
            AiEgressDecision::deny(manifest, AiEgressReason::PolicyDenied, "egress-v1", subject)
        } else {
            AiEgressDecision::allow(manifest, "egress-v1", subject)
        }
    }
}

#[tokio::test]
async fn count_description_reaches_provider_input_only_with_egress() {
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    let schema = surface::schema_builder(db).finish();
    let semantics = surface::graphql_orm_semantic_catalog();
    // Give the same entity a second valid aggregate to exercise ambiguity
    // after both of its aggregate roots are admitted.
    let aggregate = semantics
        .operations
        .iter()
        .find(|operation| {
            operation.generated_category
                == Some(graphql_orm_operation_catalog::GeneratedGraphqlOperationCategory::Aggregate)
        })
        .unwrap();
    let sdl = schema.sdl();
    let document = async_graphql_parser::parse_schema(&sdl).unwrap();
    let (query_type, aggregate_field) = document
        .definitions
        .iter()
        .find_map(|definition| {
            let TypeSystemDefinition::Type(definition) = definition else {
                return None;
            };
            let TypeKind::Object(object) = &definition.node.kind else {
                return None;
            };
            let field = object
                .fields
                .iter()
                .find(|field| field.node.name.node.as_str() == aggregate.field_name)?;
            Some((&definition.node.name.node, &field.node))
        })
        .unwrap();
    let arguments = aggregate_field
        .arguments
        .iter()
        .map(|argument| {
            let argument = &argument.node;
            let mut declaration = format!("{}: {}", argument.name.node, argument.ty.node);
            if let Some(default) = &argument.default_value {
                declaration.push_str(&format!(" = {}", default.node));
            }
            declaration
        })
        .collect::<Vec<_>>()
        .join(", ");
    let query_header = format!("type {query_type} {{");
    assert!(sdl.contains(&query_header));
    let sdl = sdl.replacen(
        &query_header,
        &format!(
            "{query_header}\n  {ALTERNATE_AGGREGATE}({arguments}): {}",
            aggregate_field.ty.node
        ),
        1,
    );
    let alternate = GraphqlSemanticOperationDescriptor::generated(
        GraphqlOperationKind::Query,
        aggregate.generated_category.unwrap(),
        aggregate.generated_entity_name.as_ref().unwrap(),
        ALTERNATE_AGGREGATE,
        "An alternate aggregate of count records",
        aggregate.arguments.clone(),
        aggregate.result_type.clone(),
        aggregate.has_authorization_policy,
    )
    .unwrap();
    let semantics = GraphqlSemanticCatalog::compose_descriptors(
        semantics.entities.iter().cloned(),
        semantics.operations.iter().cloned().chain([alternate]),
    )
    .unwrap();
    let target = GraphqlExecutionTargetId::parse("count-fixture").unwrap();
    let queries = AiGraphqlQueryCapabilityCatalog::compile(
        "countfixture",
        target.clone(),
        &sdl,
        &semantics,
        AiGraphqlQueryCapabilityLimits::default(),
    )
    .unwrap();
    let index = Arc::new(
        AiCapabilityIndex::compile(
            target,
            queries.finished_schema_fingerprint(),
            &semantics,
            Some(&queries),
            None,
            None,
            [],
            "count-policy-v1",
            AiCapabilityIndexLimits::default(),
        )
        .unwrap(),
    );
    let index_set = AiCapabilityIndexSet::compile([Arc::clone(&index)]).unwrap();
    let mut catalog = AiToolCatalog::new();
    catalog.register_query_capability_catalog(&queries).unwrap();
    let egress = Arc::new(CountEgress(AtomicBool::new(true)));
    let fixture = fixture_with_tools_and_egress(
        MockProvider::new(Vec::new()),
        Arc::new(AllowAccess),
        None,
        None,
        false,
        true,
        Some((catalog, egress.clone())),
    )
    .await;
    let authority = Arc::new(CountAuthority::default());
    let broker = Arc::new(
        AiCapabilityDiscoveryBroker::new(
            Arc::new(Resolver(fixture.principal.clone())),
            Arc::new(BrokerCurrentIndex(index)),
            authority.clone(),
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
        "f".repeat(64),
    )
    .unwrap();
    let delivery = AiCapabilityDeliveryTurn::select(
        &ProviderCapabilities {
            custom_tools: true,
            capability_delivery_modes: BTreeSet::from([AiCapabilityDeliveryMode::FixedBroker]),
            ..Default::default()
        },
        index_set.fingerprint(),
        Vec::new(),
        true,
        binding,
        broker,
        AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default()).unwrap(),
    )
    .unwrap();
    let run = AiCapabilityRunBinding::from_lease(
        &fixture.lease,
        ProviderKind::OpenAi,
        delivery.session_binding(),
    );
    let definition = delivery
        .broker_definition(AiCapabilityBrokerOperation::Describe)
        .unwrap();
    let definitions = delivery.current_tools();
    let mut fixed_definitions = capability_broker_definitions();
    fixed_definitions.sort_by(|left, right| left.tool_id.cmp(&right.tool_id));
    assert_eq!(definitions, fixed_definitions);

    let mut lease = fixture.lease.clone();
    let mut enriched_fingerprint = None;
    let mut plain_bytes = 0;
    // The third call's limit allows signatures and suppresses only the example.
    // The fourth call admits both aggregates, leaving the shape ambiguous.
    for (turn, allow) in [true, false, true, true].into_iter().enumerate() {
        authority.0.store(turn == 3, Ordering::SeqCst);
        let discovered = delivery
            .broker()
            .dispatch_discover(
                fixture.lease.principal_reference(),
                &run,
                delivery.session(),
                &json!({"text": "count records", "maximumResults": 1}),
            )
            .await
            .unwrap();
        let candidate = &discovered["candidates"][0];
        let arguments = json!({
            "capabilityId": candidate["capabilityId"],
            "candidateReference": candidate["candidateReference"],
        });

        egress.0.store(allow, Ordering::SeqCst);
        let name = format!("count-describe-{turn}");
        let result = commit_broker_provider_budget(
            &fixture,
            broker_provider_result(&lease, None, &name, &name, &definition, arguments),
        )
        .await;
        let tool_service = OrmAiApplicationToolCallService::new(
            fixture.run_service.clone(),
            fixture.runtime.clone(),
            fixture.audit.clone(),
            Arc::new(SystemClock),
            AiApplicationToolCallLimits::new(
                8_192,
                if turn == 2 { plain_bytes } else { 16_384 },
                4,
                4,
                Duration::seconds(30),
                Duration::seconds(10),
            )
            .unwrap(),
        );
        let context = AiApplicationToolCallContext::new(
            turn as u32,
            0,
            fixture.scope.clone(),
            "count-broker-test",
            &name,
        )
        .unwrap();
        let persisted = tool_service
            .execute_capability_broker_call(
                &lease,
                &result,
                context,
                automatic_mutation_route(),
                &delivery,
            )
            .await
            .unwrap();
        let row = AiToolCallRecord::find_by_id(&fixture.database, &persisted.id().0)
            .await
            .unwrap()
            .unwrap();
        lease = persisted.lease().clone();

        if !allow {
            assert_eq!(persisted.state(), AiApplicationToolCallState::EgressDenied);
            assert!(persisted.model_input().is_none());
            assert_eq!(row.disclosure_schema_fingerprint, enriched_fingerprint);
            continue;
        }
        assert_eq!(persisted.state(), AiApplicationToolCallState::Completed);
        let output = broker_output(&persisted);
        if turn == 3 {
            let operations = output["operations"].as_array().unwrap();
            let aggregates = operations
                .iter()
                .filter(|operation| operation["shape"] == "aggregate")
                .collect::<Vec<_>>();
            assert_eq!(aggregates.len(), 2);
            assert!(aggregates.iter().all(|operation| {
                operation["signature"].is_string() && operation.get("ungroupedCount").is_none()
            }));
            assert_ne!(row.disclosure_schema_fingerprint, enriched_fingerprint);
            continue;
        }
        let position = output["operations"]
            .as_array()
            .unwrap()
            .iter()
            .position(|operation| operation["shape"] == "aggregate")
            .unwrap();
        let aggregate = &output["operations"][position];
        assert!(aggregate["signature"].is_string());
        if turn == 2 {
            assert!(aggregate.get("ungroupedCount").is_none());
            assert_ne!(row.disclosure_schema_fingerprint, enriched_fingerprint);
            continue;
        }

        assert!(aggregate["ungroupedCount"].is_object());
        enriched_fingerprint = row.disclosure_schema_fingerprint;
        assert!(enriched_fingerprint.is_some());
        let mut plain = output.clone();
        plain["operations"][position]
            .as_object_mut()
            .unwrap()
            .remove("ungroupedCount");
        plain_bytes = serde_json::to_vec(&plain).unwrap().len();
        let manifest = persisted.egress_manifest().unwrap();
        assert_eq!(
            manifest.estimated_bytes as usize,
            serde_json::to_vec(output).unwrap().len() + name.len() + definition.tool_id.len()
        );

        #[cfg(feature = "provider-openai-core")]
        {
            let mut request = result.request_snapshot.clone();
            request.input = vec![persisted.model_input().unwrap().clone()];
            request.continuation = Some(ModelContinuation::ProviderResponse {
                response_id: name.clone(),
            });
            request.tools = definitions.clone();
            let wire = crate::providers::OpenAiProvider::serialize_fixture_request(&request);
            assert_eq!(wire["input"][0]["type"], "function_call_output");
            assert_eq!(wire["input"][0]["call_id"], name);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(
                    wire["input"][0]["output"].as_str().unwrap()
                )
                .unwrap(),
                *output
            );
            let tools = wire["tools"].as_array().unwrap();
            assert_eq!(tools.len(), definitions.len());
            assert!(
                tools
                    .iter()
                    .all(|tool| !tool.to_string().contains("ungroupedCount"))
            );
        }
    }
    assert_eq!(delivery.current_tools(), definitions);
    assert_eq!(fixture.mock.request_count(), 0);
}
