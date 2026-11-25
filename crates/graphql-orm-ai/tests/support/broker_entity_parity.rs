//! Capability broker version 2 over a real generated schema and database.
//!
//! Shared by the SQLite integration test and the owned PostgreSQL parity
//! lane: the same entity capability is discovered, described, expanded with
//! `describe_type`, authorized for one root and executed through the
//! generated schema against the backend under test.

use std::sync::Arc;

use agql_auth::{
    AccessTokenMetadata, AuthPrincipal, AuthUser, CurrentPrincipalResolver, PrincipalReference,
    ResolvedPrincipal, SessionContext, SystemClock,
};
use async_trait::async_trait;
use graphql_orm::graphql::orm::{ApplyOptions, Entity};
use graphql_orm::prelude::*;
use graphql_orm_ai::*;
use serde_json::json;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

pub mod surface {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "broker_parity_records",
        plural = "BrokerParityRecords",
        default_sort = "id ASC",
        keyset = "id asc",
        description = "A reviewed record reached through the capability broker"
    )]
    pub struct BrokerParityRecord {
        #[primary_key]
        #[graphql_orm(auto_generated = false, description = "Stable record identity")]
        #[filterable(type = "string")]
        #[sortable]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        #[graphql_orm(description = "Reviewed record subject")]
        pub subject: String,
    }

    schema_roots! {
        entities: [BrokerParityRecord],
    }
}

struct Resolver(AuthPrincipal);

#[async_trait]
impl CurrentPrincipalResolver for Resolver {
    async fn resolve(
        &self,
        reference: &PrincipalReference,
    ) -> agql_auth::AuthResult<ResolvedPrincipal> {
        ResolvedPrincipal::new(reference.clone(), self.0.clone(), OffsetDateTime::now_utc())
    }
}

struct CurrentIndex(Arc<AiCapabilityIndex>);

impl AiCurrentCapabilityIndex for CurrentIndex {
    fn current_index(
        &self,
        _run: &AiCapabilityRunBinding,
    ) -> Result<Arc<AiCapabilityIndex>, AiError> {
        Ok(Arc::clone(&self.0))
    }
}

struct AllowReads;

#[async_trait]
impl AiCapabilityAuthorityPolicy for AllowReads {
    async fn authorize(
        &self,
        _principal: &ResolvedPrincipal,
        _owning_index: &AiCapabilityIndex,
        entry: &AiCapabilityIndexEntry,
        _run: &AiCapabilityRunBinding,
    ) -> Result<AiCapabilityAuthorityDecision, AiError> {
        Ok(AiCapabilityAuthorityDecision {
            allowed: entry.risk == AiToolRisk::ReadOnly,
            policy_fingerprint: "broker-parity-policy-v1".to_owned(),
        })
    }
}

fn principal() -> AuthPrincipal {
    AuthPrincipal::User(AuthUser {
        user_id: "broker-parity-user".to_owned(),
        session_id: Uuid::from_u128(551),
        roles: Vec::new(),
        scopes: Vec::new(),
        session: SessionContext::default(),
        token_claims: AccessTokenMetadata::default(),
    })
}

/// Runs the version 2 broker round trip against `database`, which must be
/// empty.
pub async fn entity_capability_round_trip(database: Database) {
    use surface::{BrokerParityRecord, CreateBrokerParityRecordInput};

    let plan = database
        .schema()
        .plan_migration_to_entities(
            "broker-parity-v1",
            "capability broker parity records",
            &[BrokerParityRecord::metadata()],
        )
        .await
        .expect("parity table should plan");
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect("parity table should apply");
    for (id, subject) in [("record-1", "alpha"), ("record-2", "beta")] {
        BrokerParityRecord::insert(
            &database,
            CreateBrokerParityRecordInput {
                id: id.to_owned(),
                subject: subject.to_owned(),
            },
        )
        .await
        .expect("parity row should insert");
    }
    let schema = surface::schema_builder(database.clone())
        .data(AuthSubject::new("broker-parity-user"))
        .finish();
    let semantics = surface::graphql_orm_semantic_catalog();
    let target = GraphqlExecutionTargetId::parse("broker-parity").expect("target");
    let queries = AiGraphqlQueryCapabilityCatalog::compile(
        "brokerparity",
        target.clone(),
        &schema.sdl(),
        semantics,
        AiGraphqlQueryCapabilityLimits::default(),
    )
    .expect("generated query capabilities should compile");
    let index = Arc::new(
        AiCapabilityIndex::compile(
            target,
            queries.finished_schema_fingerprint(),
            semantics,
            Some(&queries),
            None,
            None,
            [],
            "broker-parity-target-policy-v1",
            AiCapabilityIndexLimits::default(),
        )
        .expect("capability index should compile"),
    );
    // One entity entry covers every generated query root of the entity.
    let entity = index
        .entries()
        .find(|entry| entry.kind == AiCapabilityKind::GeneratedEntity)
        .expect("entity capability")
        .clone();
    assert_eq!(entity.entity_name.as_deref(), Some("BrokerParityRecord"));
    assert!(entity.operations.len() >= 2, "{:?}", entity.operations);
    assert_eq!(
        index
            .entries()
            .filter(|entry| entry.entity_name.as_deref() == Some("BrokerParityRecord"))
            .count(),
        1
    );

    let principal = principal();
    let reference = principal.reference();
    let broker = AiCapabilityDiscoveryBroker::new(
        Arc::new(Resolver(principal)),
        Arc::new(CurrentIndex(index.clone())),
        Arc::new(AllowReads),
        Arc::new(SystemClock),
        Duration::seconds(60),
    )
    .expect("broker");
    let session = AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default())
        .expect("broker session");
    let run = AiCapabilityRunBinding {
        session_id: AiSessionId::new(),
        run_id: AiRunId::new(),
        attempt_id: Uuid::new_v4(),
        lease_generation: 1,
        provider_kind: ProviderKind::OpenAi,
        provider_session_fingerprint: "broker-parity-session-v2".to_owned(),
    };

    let discovered = broker
        .dispatch_discover(
            &reference,
            &run,
            &session,
            &json!({"text": "reviewed broker parity record", "maximumResults": 4}),
        )
        .await
        .expect("discover");
    let candidate = discovered["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .find(|candidate| candidate["kind"] == "generated_entity")
        .expect("entity candidate")
        .clone();
    let described = broker
        .dispatch_describe(
            &reference,
            &run,
            &session,
            &json!({
                "capabilityId": candidate["capabilityId"],
                "candidateFingerprint": candidate["candidateFingerprint"],
            }),
        )
        .await
        .expect("describe");
    let list = entity
        .operations
        .iter()
        .find(|member| member.operation_shape == AiCapabilityOperationShape::List)
        .expect("list operation");
    let described_list = described
        .operations()
        .iter()
        .find(|operation| operation.operation_name() == list.operation_name)
        .expect("described list operation")
        .clone();
    let capability = queries
        .capabilities()
        .find(|capability| capability.id() == described_list.capability_id())
        .expect("list capability");
    let signature = capability.capability_signature().expect("signature");
    let summaries = described
        .operations()
        .iter()
        .map(|operation| {
            queries
                .capabilities()
                .find(|capability| capability.id() == operation.capability_id())
                .expect("operation capability")
                .capability_signature()
                .expect("operation signature")
                .summary()
        })
        .collect::<Vec<_>>();
    let contract = described
        .clone()
        .with_operation_signatures(summaries, 64 * 1_024)
        .expect("signatures fit")
        .into_model_result();
    assert_eq!(contract["version"], 3);
    let encoded = serde_json::to_vec(&contract).expect("contract");
    assert!(
        encoded.len() < 16 * 1_024,
        "describe is compact: {} bytes",
        encoded.len()
    );
    let summary = signature.summary();
    let text = summary["signature"].as_str().expect("signature text");
    assert!(text.contains("where?:"), "{text}");

    // Expand the filter path through describe_type, as a model would.
    let expand = |name: &str| {
        let broker = &broker;
        let reference = &reference;
        let run = &run;
        let session = &session;
        let described = &described;
        let operation = list.operation_name.clone();
        let signature = &signature;
        let name = name.to_owned();
        async move {
            broker
                .dispatch_describe_type(
                    reference,
                    run,
                    session,
                    &json!({
                        "loadedReference": described.loaded_reference(),
                        "operation": operation,
                        "type": name,
                    }),
                )
                .await
                .expect("describe type")
                .render(signature)
                .expect("type page")
        }
    };
    let where_type = summary["inputTypes"]
        .as_array()
        .expect("input types")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .find(|name| name.ends_with("WhereInput") || name.ends_with("Where"))
        .expect("where input type")
        .to_owned();
    let where_page = expand(&where_type).await;
    let subject = where_page["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .find(|field| field["name"] == "subject")
        .expect("subject filter")
        .clone();
    let filter_type = subject["type"]
        .as_str()
        .expect("filter type")
        .split('|')
        .next()
        .expect("filter name")
        .to_owned();
    let filter_page = expand(&filter_type).await;
    assert!(
        filter_page["fields"]
            .as_array()
            .expect("filter fields")
            .iter()
            .any(|field| field["name"] == "eq")
    );
    let selection = expand("select").await;
    let subject_field = selection["fields"]
        .as_array()
        .expect("selection fields")
        .iter()
        .find(|field| field["name"] == "subject")
        .expect("subject selection")
        .clone();
    // The root selection carries the field's own index description.
    assert_eq!(subject_field["description"], "Reviewed record subject");

    let mut execute = json!({
        "loadedReference": described.loaded_reference(),
        "operation": list.operation_name,
        "arguments": [{"name": "where.subject.eq", "value": "beta"}],
        "selections": ["id", "subject"],
    });
    if summary["maximumItems"]["required"] == true {
        execute["maximumItems"] = json!(10);
    }
    let execution = broker
        .authorize_broker_execution(&reference, &run, &session, &execute)
        .await
        .expect("execute authorizes the list root");
    assert_eq!(execution.capability_id(), capability.id());
    assert_eq!(
        execution.capability_kind(),
        AiCapabilityKind::GeneratedQuery
    );
    let compiled = capability
        .compile_compact(execution.into_plan())
        .expect("the root's compiler accepts the broker plan");
    let response = schema
        .execute(
            graphql_orm::async_graphql::Request::new(compiled.descriptor().document.clone())
                .variables(graphql_orm::async_graphql::Variables::from_json(
                    compiled.variables().clone(),
                )),
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().expect("data").to_string();
    assert!(data.contains("beta"), "{data}");
    assert!(!data.contains("alpha"), "{data}");
    assert_eq!(session.amplification().describe_type_calls, 3);
}

/// Browses to the entity capability, then reads every row of its keyset
/// root two at a time through broker continuation handles, executing each
/// page through the generated schema against `database`, which must be
/// empty.
pub async fn keyset_continuation_round_trip(database: Database) {
    use surface::{BrokerParityRecord, CreateBrokerParityRecordInput};

    let plan = database
        .schema()
        .plan_migration_to_entities(
            "broker-parity-v1",
            "capability broker parity records",
            &[BrokerParityRecord::metadata()],
        )
        .await
        .expect("parity table should plan");
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect("parity table should apply");
    let ids = (1..=5)
        .map(|row| format!("record-{row}"))
        .collect::<Vec<_>>();
    for id in &ids {
        BrokerParityRecord::insert(
            &database,
            CreateBrokerParityRecordInput {
                id: id.clone(),
                subject: format!("subject of {id}"),
            },
        )
        .await
        .expect("parity row should insert");
    }
    let schema = surface::schema_builder(database.clone())
        .data(AuthSubject::new("broker-parity-user"))
        .finish();
    let semantics = surface::graphql_orm_semantic_catalog();
    let target = GraphqlExecutionTargetId::parse("broker-parity").expect("target");
    let queries = AiGraphqlQueryCapabilityCatalog::compile(
        "brokerparity",
        target.clone(),
        &schema.sdl(),
        semantics,
        AiGraphqlQueryCapabilityLimits::default(),
    )
    .expect("generated query capabilities should compile");
    let index = Arc::new(
        AiCapabilityIndex::compile(
            target,
            queries.finished_schema_fingerprint(),
            semantics,
            Some(&queries),
            None,
            None,
            [],
            "broker-parity-target-policy-v1",
            AiCapabilityIndexLimits::default(),
        )
        .expect("capability index should compile"),
    );
    let principal = principal();
    let reference = principal.reference();
    let broker = AiCapabilityDiscoveryBroker::new(
        Arc::new(Resolver(principal)),
        Arc::new(CurrentIndex(index.clone())),
        Arc::new(AllowReads),
        Arc::new(SystemClock),
        Duration::seconds(60),
    )
    .expect("broker");
    let session = AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default())
        .expect("broker session");
    let run = AiCapabilityRunBinding {
        session_id: AiSessionId::new(),
        run_id: AiRunId::new(),
        attempt_id: Uuid::new_v4(),
        lease_generation: 1,
        provider_kind: ProviderKind::OpenAi,
        provider_session_fingerprint: "broker-parity-session-v3".to_owned(),
    };

    // Browse the hierarchy instead of searching.
    let namespaces = broker
        .dispatch_browse(&reference, &run, &session, &json!({"level": "namespaces"}))
        .await
        .expect("browse namespaces");
    let namespace = namespaces["namespaces"][0]["namespace"]
        .as_str()
        .expect("namespace")
        .to_owned();
    let capabilities = broker
        .dispatch_browse(
            &reference,
            &run,
            &session,
            &json!({"level": "capabilities", "namespace": namespace, "entity": "BrokerParityRecord"}),
        )
        .await
        .expect("browse capabilities");
    let candidate = capabilities["capabilities"][0].clone();
    assert_eq!(candidate["kind"], "generated_entity", "{capabilities}");
    let operations = broker
        .dispatch_browse(
            &reference,
            &run,
            &session,
            &json!({"level": "operations", "namespace": namespace, "entity": "BrokerParityRecord"}),
        )
        .await
        .expect("browse operations");
    let keyset = operations["operations"]
        .as_array()
        .expect("operations")
        .iter()
        .filter(|operation| operation["shape"] == "keyset_list")
        .filter_map(|operation| operation["operation"].as_str())
        .find_map(|name| {
            queries
                .capabilities()
                .find(|capability| capability.field_name() == name)
                .filter(|capability| capability.supports_continuation())
        })
        .unwrap_or_else(|| panic!("a continuable keyset root: {operations}"));
    // Plain list roots are not continuable: they take a model-owned offset.
    assert!(
        queries
            .capabilities()
            .filter(|capability| capability.generated_entity_name() == Some("BrokerParityRecord"))
            .any(|capability| !capability.supports_continuation())
    );
    let described = broker
        .dispatch_describe(
            &reference,
            &run,
            &session,
            &json!({
                "capabilityId": candidate["capabilityId"],
                "candidateReference": candidate["candidateReference"],
            }),
        )
        .await
        .expect("describe the browsed candidate");

    let first = json!({
        "loadedReference": described.loaded_reference(),
        "operation": keyset.field_name(),
        "selections": ["id", "subject"],
        "maximumItems": 2,
    });
    let mut execution = broker
        .authorize_broker_execution(&reference, &run, &session, &first)
        .await
        .expect("first page authorizes");
    assert_eq!(execution.capability_id(), keyset.id());
    let unpaged = keyset
        .compile_compact(execution.plan().clone())
        .expect("unpaged compile");
    assert!(!unpaged.descriptor().document.contains("pageInfo"));
    let mut seen = Vec::new();
    let mut pages = 0;
    loop {
        let compiled = keyset
            .compile_compact_page(execution.plan().clone(), execution.continuation_cursor())
            .expect("page compiles");
        assert!(compiled.descriptor().document.contains("pageInfo"));
        assert_ne!(compiled.plan_fingerprint(), unpaged.plan_fingerprint());
        // The cursor a continued page carries never appears in Debug output:
        // not of the execution, the compiled page, or the host request.
        if let Some(cursor) = execution.continuation_cursor() {
            let contract = compiled
                .descriptor()
                .graphql_contract
                .clone()
                .expect("page contract");
            let request = ToolGraphqlRequest {
                document: compiled.descriptor().document.clone(),
                operation_name: contract.operation_name.clone(),
                contract,
                variables: compiled.variables().clone(),
                invocation: GraphqlInvocationContext {
                    run_id: run.run_id,
                    tool_call_id: AiToolCallId::new(),
                    scope: AiScope::new("tenant", "broker-parity"),
                    correlation_id: "broker-parity".to_owned(),
                    causation_id: "broker-parity".to_owned(),
                    delegation_reference: None,
                    idempotency_key: None,
                },
            };
            assert!(request.variables.to_string().contains(cursor));
            for debug in [
                format!("{execution:?}"),
                format!("{compiled:?}"),
                format!("{request:?}"),
            ] {
                assert!(!debug.contains(cursor), "{debug}");
            }
        }
        let response = schema
            .execute(
                graphql_orm::async_graphql::Request::new(compiled.descriptor().document.clone())
                    .variables(graphql_orm::async_graphql::Variables::from_json(
                        compiled.variables().clone(),
                    )),
            )
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let mut data = response.data.into_json().expect("data");
        let raw_response = ToolGraphqlResponse {
            data: data.clone(),
            error_codes: Vec::new(),
            application_audit_ref: None,
        };
        let page_info = compiled
            .take_page_info(&mut data)
            .expect("page info")
            .expect("a page reports its page info");
        if let Some(cursor) = page_info.end_cursor.as_deref() {
            assert!(raw_response.data.to_string().contains(cursor));
            assert!(!format!("{page_info:?}").contains(cursor));
            assert!(!format!("{raw_response:?}").contains(cursor));
        }
        // What remains is exactly the disclosed shape: no cursor.
        assert!(!data.to_string().contains("pageInfo"), "{data}");
        compiled
            .disclosure_schema()
            .evaluate_graphql_with_record_limit(&data, 1_000)
            .expect("the stripped page matches its disclosure schema");
        let text = data.to_string();
        let rows = ids
            .iter()
            .filter(|id| text.contains(&format!("\"{id}\"")))
            .cloned()
            .collect::<Vec<_>>();
        assert!(!rows.is_empty() && rows.len() <= 2, "{text}");
        seen.extend(rows);
        pages += 1;
        let Some(continuation) = broker.record_continuation(&session, &execution, &page_info)
        else {
            assert!(!page_info.has_next_page, "{page_info:?}");
            break;
        };
        assert!(
            !continuation
                .to_model_value()
                .to_string()
                .contains(page_info.end_cursor.as_deref().expect("cursor"))
        );
        execution = broker
            .authorize_broker_execution(
                &reference,
                &run,
                &session,
                &json!({
                    "loadedReference": described.loaded_reference(),
                    "operation": keyset.field_name(),
                    "continuation": continuation.handle(),
                }),
            )
            .await
            .expect("continued page authorizes");
        assert!(pages < 10, "the chain terminates");
    }
    assert_eq!(seen, ids, "every row once, in keyset order");
    assert_eq!(pages, 3);
    assert_eq!(session.amplification().browse_calls, 3);
}
