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
    assert_eq!(contract["version"], 2);
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
    assert!(
        selection["fields"]
            .as_array()
            .expect("selection fields")
            .iter()
            .any(|field| field["name"] == "subject")
    );

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
