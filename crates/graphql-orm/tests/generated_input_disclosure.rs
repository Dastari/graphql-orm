#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::graphql::orm::*;
use graphql_orm::prelude::*;
use graphql_orm_ai_tool_profiles::*;

async fn database() -> graphql_orm::db::Database {
    graphql_orm::db::Database::new(sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap())
}

mod optional {
    use super::*;
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "optional_input_records",
        plural = "OptionalInputRecords",
        upsert = "label",
        mutation_shapes = "create,update,upsert"
    )]
    pub struct OptionalInputRecord {
        #[primary_key]
        pub id: String,
        #[unique]
        #[filterable(type = "string")]
        #[sortable]
        pub label: String,
        #[input_only]
        #[filterable(type = "string")]
        #[sortable]
        pub credential: Option<String>,
        #[graphql_orm(read = false)]
        #[filterable(type = "string")]
        #[sortable]
        pub unread_tag: Option<String>,
        #[graphql_orm(private, write = true, filter = true, order = true)]
        #[input_only]
        #[filterable(type = "string")]
        #[sortable]
        pub private_note: Option<String>,
        #[graphql_orm(sensitive)]
        #[filterable(type = "string")]
        pub sensitive_note: Option<String>,
        #[graphql_orm(non_exportable)]
        pub internal_note: Option<String>,
        #[input_only]
        pub private_list: Option<Vec<u8>>,
        #[input_only]
        #[graphql_orm(json)]
        pub private_json: Option<serde_json::Value>,
    }
    schema_roots! { query_custom_ops: [], entities: [OptionalInputRecord] }
}

mod required {
    use super::*;
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "required_input_records",
        plural = "RequiredInputRecords",
        upsert = "label",
        mutation_shapes = "create,update,upsert"
    )]
    pub struct RequiredInputRecord {
        #[primary_key]
        pub id: String,
        #[unique]
        #[filterable(type = "string")]
        #[sortable]
        pub label: String,
        #[input_only]
        #[filterable(type = "string")]
        #[sortable]
        pub credential: String,
        #[graphql_orm(read = false)]
        #[filterable(type = "string")]
        #[sortable]
        pub unread_tag: String,
        #[graphql_orm(private, write = true, filter = true, order = true)]
        #[input_only]
        #[filterable(type = "string")]
        #[sortable]
        pub private_note: String,
        #[graphql_orm(sensitive)]
        #[filterable(type = "string")]
        pub sensitive_note: String,
        #[graphql_orm(non_exportable)]
        pub internal_note: String,
    }
    schema_roots! { query_custom_ops: [], entities: [RequiredInputRecord] }
}

mod composite {
    use super::*;
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "composite_input_records",
        plural = "CompositeInputRecords",
        repository_mutations = true,
        keyset = "tenant asc, hidden_code asc"
    )]
    pub struct CompositeInputRecord {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        pub tenant: String,
        #[primary_key]
        #[graphql_orm(auto_generated = false, non_exportable)]
        #[filterable(type = "string")]
        #[sortable]
        pub hidden_code: String,
        #[filterable(type = "string")]
        #[sortable]
        pub label: String,
    }
    schema_roots! {query_custom_ops:[],entities:[CompositeInputRecord]}
}

fn probes(mut sdl: String, catalog: GraphqlSemanticCatalog) -> (String, GraphqlSemanticCatalog) {
    let entity = &catalog.entities[0].entity_name;
    let start = sdl.find("type Query {").unwrap();
    let end = start + sdl[start..].find("\n}").unwrap() + 2;
    sdl.replace_range(start..end, "type Query {\n}");
    let mut operations = Vec::new();
    for (root, input) in [
        ("createProbe", format!("Create{entity}Input")),
        ("updateProbe", format!("Update{entity}Input")),
        ("whereProbe", format!("{entity}WhereInput")),
        ("orderProbe", format!("{entity}OrderByInput")),
        ("keyProbe", format!("{entity}KeyInput")),
        ("keysetProbe", format!("{entity}KeysetOrderByInput")),
    ] {
        if !sdl.contains(&format!("input {input} {{")) {
            continue;
        }
        sdl = sdl.replacen(
            "type Query {",
            &format!("type Query {{\n {root}(input: {input}): {entity}"),
            1,
        );
        operations.push(
            GraphqlSemanticOperationDescriptor::generated(
                GraphqlOperationKind::Query,
                GeneratedGraphqlOperationCategory::SingleRead,
                entity,
                root,
                "Inspect generated inputs.",
                vec![GraphqlSemanticArgumentDescriptor {
                    graphql_name: "input".into(),
                    description: "Generated input.".into(),
                    type_ref: GraphqlSemanticTypeRef::named(
                        input,
                        GraphqlSemanticTypeKind::Object,
                        true,
                    ),
                }],
                GraphqlSemanticTypeRef::named(entity, GraphqlSemanticTypeKind::Object, true),
                true,
            )
            .unwrap(),
        );
    }
    (
        sdl,
        GraphqlSemanticCatalog::compose_descriptors(catalog.entities, operations).unwrap(),
    )
}

fn compile(
    sdl: &str,
    catalog: &GraphqlSemanticCatalog,
) -> Result<AiGraphqlQueryCapabilityCatalog, AiError> {
    AiGraphqlQueryCapabilityCatalog::compile(
        "input-contracts",
        GraphqlExecutionTargetId::parse("input.contracts").unwrap(),
        sdl,
        catalog,
        AiGraphqlQueryCapabilityLimits::default(),
    )
}

fn input_properties(capability: &AiGraphqlQueryCapability) -> &serde_json::Value {
    let input =
        &capability.compact_argument_schema()["properties"]["arguments"]["properties"]["input"];
    let input = input
        .get("anyOf")
        .and_then(|values| values.as_array())
        .map(|values| &values[0])
        .unwrap_or(input);
    &input["properties"]
}

#[tokio::test]
async fn generated_optional_undisclosed_members_preserve_readable_inputs() {
    let schema = optional::schema_builder(database().await).finish();
    let (sdl, semantic) = probes(
        schema.sdl(),
        optional::graphql_orm_semantic_catalog().clone(),
    );
    let capabilities = compile(&sdl, &semantic).expect("generated optional members stay closed");
    assert_eq!(capabilities.capabilities().count(), 4);
    let entity = &semantic.entities[0];
    let label = entity
        .fields
        .iter()
        .find(|field| field.field_name.eq_ignore_ascii_case("label"))
        .unwrap();
    let eq = &label
        .filter_input
        .as_ref()
        .unwrap()
        .operators
        .iter()
        .find(|operator| operator.operator == GraphqlSemanticFilterOperator::Equal)
        .unwrap()
        .graphql_name;
    for capability in capabilities.capabilities() {
        assert!(
            input_properties(capability)
                .as_object()
                .unwrap()
                .contains_key(&label.field_name)
        );
        let value = match capability.field_name() {
            "whereProbe" => serde_json::json!({(eq):"value"}),
            "orderProbe" => serde_json::json!("ASC"),
            _ => serde_json::json!("value"),
        };
        let mut plan = serde_json::json!({"arguments":{"input":{(label.field_name.clone()):value}},"selections":[label.field_name],"relationshipArguments":{},"relationshipMaximumItems":{}});
        assert!(capability.compile_compact(plan.clone()).is_ok());
        for hidden in [
            "credential",
            "unreadTag",
            "privateNote",
            "sensitiveNote",
            "internalNote",
            "privateList",
            "privateJson",
        ] {
            assert!(
                !capability
                    .description()
                    .to_ascii_lowercase()
                    .replace('_', "")
                    .contains(&hidden.to_ascii_lowercase())
            );
            for surface in [
                capability.compact_argument_schema(),
                capability.argument_schema(),
            ] {
                assert!(
                    !surface
                        .to_string()
                        .to_ascii_lowercase()
                        .replace('_', "")
                        .contains(&hidden.to_ascii_lowercase())
                );
            }
        }
        let input = semantic
            .operations
            .iter()
            .find(|operation| operation.field_name == capability.field_name())
            .unwrap()
            .arguments[0]
            .type_ref
            .clone();
        let input = match input {
            GraphqlSemanticTypeRef::Named { name, .. } => name,
            _ => unreachable!(),
        };
        for hidden in entity
            .generated_input_members
            .iter()
            .filter(|member| member.input_type == input)
        {
            plan["arguments"]["input"][&hidden.field_name] = serde_json::json!("withheld");
            let error = capability
                .compile_compact(plan.clone())
                .err()
                .expect("undisclosed values are not accepted");
            assert!(!error.to_string().contains(&hidden.field_name));
            plan["arguments"]["input"]
                .as_object_mut()
                .unwrap()
                .remove(&hidden.field_name);
        }
    }
    // Public exports and their fingerprints do not carry the host sidecar.
    assert!(!entity.generated_input_members.is_empty());
    let bytes = serde_json::to_vec(&semantic).unwrap();
    let mut public = semantic.clone();
    for entity in &mut public.entities {
        entity.generated_input_members.clear();
    }
    let public =
        GraphqlSemanticCatalog::compose_descriptors(public.entities, public.operations).unwrap();
    assert_eq!(format!("{semantic:#?}"), format!("{public:#?}"));
    assert_eq!(semantic.fingerprint, public.fingerprint);
    assert_eq!(bytes, serde_json::to_vec(&public).unwrap());
    assert_eq!(
        semantic
            .operations
            .iter()
            .map(|operation| &operation.fingerprint)
            .collect::<Vec<_>>(),
        public
            .operations
            .iter()
            .map(|operation| &operation.fingerprint)
            .collect::<Vec<_>>()
    );
    for hidden in ["credential", "unreadtag", "privatenote"] {
        assert!(
            !String::from_utf8(bytes.clone())
                .unwrap()
                .to_ascii_lowercase()
                .replace('_', "")
                .contains(hidden)
        );
    }
    #[cfg(feature = "router-protocol")]
    assert!(
        !serde_json::to_string(&semantic.router_protocol_extension().unwrap())
            .unwrap()
            .contains("generated_input_members")
    );
    let disclosure = AiCustomInputDisclosure::new()
        .with_exportable_field(
            "CreateOptionalInputRecordInput",
            &entity
                .generated_input_members
                .iter()
                .find(|member| member.input_type == "CreateOptionalInputRecordInput")
                .unwrap()
                .field_name,
        )
        .unwrap();
    let error = AiGraphqlQueryCapabilityCatalog::compile_with_options(
        "input-contracts",
        GraphqlExecutionTargetId::parse("input.contracts").unwrap(),
        &sdl,
        &semantic,
        AiGraphqlQueryCapabilityLimits::default(),
        AiGraphqlQueryCapabilityOptions::default().with_custom_input_disclosure(disclosure),
    )
    .err()
    .expect("host disclosure does not open producer-hidden fields");
    assert!(error.to_string().contains("CreateOptionalInputRecordInput"));
}

#[tokio::test]
async fn generated_required_undisclosed_member_skips_only_its_capability() {
    let schema = required::schema_builder(database().await).finish();
    let (sdl, semantic) = probes(
        schema.sdl(),
        required::graphql_orm_semantic_catalog().clone(),
    );
    let capabilities =
        compile(&sdl, &semantic).expect("required hidden inputs omit one capability");
    assert_eq!(capabilities.capabilities().count(), 3);
    assert_eq!(capabilities.diagnostics().len(), 5);
    for diagnostic in capabilities.diagnostics() {
        assert_eq!(diagnostic.input_type, "CreateRequiredInputRecordInput");
        assert_eq!(diagnostic.field_name, "createProbe");
        assert!(!format!("{diagnostic:?}").contains(&diagnostic.member));
    }
    assert!(
        capabilities
            .capabilities()
            .all(|capability| capability.field_name() != "createProbe")
    );
    assert!(capabilities.capabilities().all(|capability| {
        input_properties(capability)
            .as_object()
            .unwrap()
            .keys()
            .any(|name| name.eq_ignore_ascii_case("label"))
    }));
}

#[tokio::test]
async fn malformed_generated_member_reports_its_host_coordinate() {
    let schema = optional::schema_builder(database().await).finish();
    let (sdl, semantic) = probes(
        schema.sdl(),
        optional::graphql_orm_semantic_catalog().clone(),
    );
    let label = semantic.entities[0]
        .fields
        .iter()
        .find(|field| field.field_name.eq_ignore_ascii_case("label"))
        .unwrap()
        .field_name
        .clone();
    for input in [
        "CreateOptionalInputRecordInput",
        "UpdateOptionalInputRecordInput",
        "OptionalInputRecordWhereInput",
        "OptionalInputRecordOrderByInput",
    ] {
        let start = sdl.find(&format!("input {input} {{")).unwrap();
        let end = start + sdl[start..].find("\n}").unwrap();
        let mut changed = sdl.clone();
        let body = &sdl[start..end];
        let line = body
            .lines()
            .find(|line| line.trim_start().starts_with(&format!("{label}:")))
            .unwrap();
        let malformed = body.replace(line, &format!("\t{label}: Int"));
        changed.replace_range(start..end, &malformed);
        let error = compile(&changed, &semantic)
            .err()
            .expect("canonical generated mismatches are host configuration errors");
        let message = error.to_string();
        assert!(
            message.contains(input) && message.contains(&label),
            "{message}"
        );
    }
}

fn classified_mutations(semantic: &GraphqlSemanticCatalog) -> GraphqlSemanticCatalog {
    let operations = semantic
        .operations
        .iter()
        .cloned()
        .map(|operation| {
            if operation.kind == GraphqlOperationKind::Mutation {
                operation
                    .with_ai_mutation_execution(AiMutationExecutionPolicy::Automatic)
                    .unwrap()
            } else {
                operation
            }
        })
        .collect::<Vec<_>>();
    GraphqlSemanticCatalog::compose_descriptors(semantic.entities.clone(), operations).unwrap()
}

#[tokio::test]
async fn actual_generated_mutations_and_upsert_keep_hidden_inputs_closed() {
    let schema = optional::schema_builder(database().await).finish();
    let semantic = classified_mutations(optional::graphql_orm_semantic_catalog());
    let capabilities = AiGraphqlMutationCapabilityCatalog::compile(
        "input-mutations",
        GraphqlExecutionTargetId::parse("input.mutations").unwrap(),
        &schema.sdl(),
        &semantic,
        AiGraphqlQueryCapabilityLimits::default(),
    )
    .unwrap();
    assert_eq!(capabilities.capabilities().count(), 3);
    assert!(capabilities.diagnostics().is_empty());
    for capability in capabilities.capabilities() {
        for hidden in &semantic.entities[0].generated_input_members {
            let normalized = hidden.field_name.to_ascii_lowercase().replace('_', "");
            assert!(
                !capability
                    .compact_argument_schema()
                    .to_string()
                    .to_ascii_lowercase()
                    .replace('_', "")
                    .contains(&normalized)
            );
            assert!(
                !capability
                    .argument_schema()
                    .to_string()
                    .to_ascii_lowercase()
                    .replace('_', "")
                    .contains(&normalized)
            );
            assert!(
                !capability
                    .description()
                    .to_ascii_lowercase()
                    .replace('_', "")
                    .contains(&normalized)
            );
        }
    }
    let schema = required::schema_builder(database().await).finish();
    let semantic = classified_mutations(required::graphql_orm_semantic_catalog());
    let capabilities = AiGraphqlMutationCapabilityCatalog::compile(
        "input-mutations",
        GraphqlExecutionTargetId::parse("input.mutations").unwrap(),
        &schema.sdl(),
        &semantic,
        AiGraphqlQueryCapabilityLimits::default(),
    )
    .unwrap();
    assert_eq!(capabilities.capabilities().count(), 1);
    assert!(
        capabilities
            .capabilities()
            .next()
            .unwrap()
            .field_name()
            .to_ascii_lowercase()
            .contains("update")
    );
    assert_eq!(capabilities.diagnostics().len(), 10);
    assert!(
        capabilities
            .diagnostics()
            .iter()
            .all(|diagnostic| diagnostic.input_type == "CreateRequiredInputRecordInput")
    );
}

#[tokio::test]
async fn required_generated_input_on_subscription_returns_only_host_diagnostics() {
    let schema = required::schema_builder(database().await).finish();
    let mut sdl = schema.sdl();
    let start = sdl.find("type Subscription {").unwrap();
    let end = start + sdl[start..].find("\n}").unwrap() + 2;
    sdl.replace_range(start..end,"type Subscription {\n inspectRequired(input: CreateRequiredInputRecordInput): RequiredInputRecord\n}");
    let operation = GraphqlSemanticOperationDescriptor::generated(
        GraphqlOperationKind::Subscription,
        GeneratedGraphqlOperationCategory::SingleRead,
        "RequiredInputRecord",
        "inspectRequired",
        "Observe public records.",
        vec![GraphqlSemanticArgumentDescriptor {
            graphql_name: "input".into(),
            description: "Generated input.".into(),
            type_ref: GraphqlSemanticTypeRef::named(
                "CreateRequiredInputRecordInput",
                GraphqlSemanticTypeKind::Object,
                true,
            ),
        }],
        GraphqlSemanticTypeRef::named("RequiredInputRecord", GraphqlSemanticTypeKind::Object, true),
        true,
    )
    .unwrap()
    .with_subscription_observation(GraphqlSubscriptionObservationDescriptor {
        replay_mode: GraphqlSubscriptionReplayMode::ReplayThenLive,
        maximum_duration_seconds: Some(60),
        maximum_events: Some(1),
        condition_fields: Vec::new(),
    })
    .unwrap();
    let semantic = GraphqlSemanticCatalog::compose_descriptors(
        required::graphql_orm_semantic_catalog().entities.clone(),
        [operation],
    )
    .unwrap();
    let capabilities = AiGraphqlSubscriptionCapabilityCatalog::compile(
        "input-observations",
        GraphqlExecutionTargetId::parse("input.observations").unwrap(),
        &sdl,
        &semantic,
        AiGraphqlSubscriptionCapabilityLimits::default(),
    )
    .unwrap();
    assert_eq!(capabilities.capabilities().count(), 0);
    assert_eq!(capabilities.diagnostics().len(), 5);
}

#[tokio::test]
async fn required_hidden_values_are_checked_only_on_offered_input_paths() {
    let schema = required::schema_builder(database().await).finish();
    let (mut sdl, semantic) = probes(
        schema.sdl(),
        required::graphql_orm_semantic_catalog().clone(),
    );
    sdl.push_str("\ninput HostEnvelope { payload: CreateRequiredInputRecordInput }\n");
    sdl = sdl.replacen(
        "type Query {",
        "type Query {\n closedEnvelope(input: HostEnvelope): RequiredInputRecord",
        1,
    );
    let mut operations = semantic.operations;
    operations.push(
        GraphqlSemanticOperationDescriptor::custom(
            GraphqlOperationKind::Query,
            "closedEnvelope",
            "Inspect a host envelope.",
            vec![GraphqlSemanticArgumentDescriptor {
                graphql_name: "input".into(),
                description: "Host envelope.".into(),
                type_ref: GraphqlSemanticTypeRef::named(
                    "HostEnvelope",
                    GraphqlSemanticTypeKind::Object,
                    true,
                ),
            }],
            GraphqlSemanticTypeRef::named(
                "RequiredInputRecord",
                GraphqlSemanticTypeKind::Object,
                true,
            ),
            true,
        )
        .unwrap(),
    );
    let semantic =
        GraphqlSemanticCatalog::compose_descriptors(semantic.entities, operations).unwrap();
    let capabilities = compile(&sdl, &semantic).unwrap();
    assert!(
        capabilities
            .capabilities()
            .any(|capability| capability.field_name() == "closedEnvelope")
    );
    assert_eq!(capabilities.diagnostics().len(), 5);
    let options = AiGraphqlQueryCapabilityOptions::default().with_custom_input_disclosure(
        AiCustomInputDisclosure::new()
            .with_exportable_field("HostEnvelope", "payload")
            .unwrap(),
    );
    let capabilities = AiGraphqlQueryCapabilityCatalog::compile_with_options(
        "input-contracts",
        GraphqlExecutionTargetId::parse("input.contracts").unwrap(),
        &sdl,
        &semantic,
        AiGraphqlQueryCapabilityLimits::default(),
        options,
    )
    .unwrap();
    assert!(
        !capabilities
            .capabilities()
            .any(|capability| capability.field_name() == "closedEnvelope")
    );
    assert_eq!(capabilities.diagnostics().len(), 10);
}

#[tokio::test]
async fn generated_composite_keys_and_keyset_keep_non_exportable_members_closed() {
    let schema = composite::schema_builder(database().await)
        .register_input_type::<composite::CompositeInputRecordKey>()
        .finish();
    let (sdl, semantic) = probes(
        schema.sdl(),
        composite::graphql_orm_semantic_catalog().clone(),
    );
    let capabilities = compile(&sdl, &semantic).unwrap();
    assert_eq!(capabilities.capabilities().count(), 3);
    assert_eq!(capabilities.diagnostics().len(), 1);
    assert_eq!(
        capabilities.diagnostics()[0].input_type,
        "CompositeInputRecordKeyInput"
    );
    assert!(
        capabilities
            .capabilities()
            .any(|capability| capability.field_name() == "keysetProbe")
    );
    for capability in capabilities.capabilities() {
        assert!(
            !capability
                .compact_argument_schema()
                .to_string()
                .to_ascii_lowercase()
                .replace('_', "")
                .contains("hiddencode")
        );
    }
}
