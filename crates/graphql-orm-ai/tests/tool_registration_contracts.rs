//! Static tool documents and generated capabilities are admitted only for
//! exact application operations outside the AI control plane.

use graphql_orm::graphql::orm::{
    AiMutationExecutionPolicy, GraphqlEntitySemanticMetadata, GraphqlOperationCatalog,
    GraphqlOperationKind, GraphqlSemanticArgumentDescriptor, GraphqlSemanticCatalog,
    GraphqlSemanticClassification, GraphqlSemanticExport, GraphqlSemanticFieldMetadata,
    GraphqlSemanticOperationDescriptor, GraphqlSemanticTypeKind, GraphqlSemanticTypeRef,
    GraphqlSubscriptionConditionField, GraphqlSubscriptionConditionOperator,
    GraphqlSubscriptionObservationDescriptor, GraphqlSubscriptionReplayMode,
};
use graphql_orm_ai::*;
use serde_json::json;

fn disclosure_schema() -> AiDisclosureSchema {
    AiDisclosureSchema::new(
        "records-v1",
        AiDisclosureShape::object(
            AiDisclosureRule::exportable(DataClassification::Internal),
            [(
                "records".to_owned(),
                AiDisclosureShape::list(
                    AiDisclosureRule::exportable(DataClassification::Internal),
                    100,
                    AiDisclosureShape::object(
                        AiDisclosureRule::exportable(DataClassification::Internal),
                        [(
                            "id".to_owned(),
                            AiDisclosureShape::scalar(AiDisclosureRule::exportable(
                                DataClassification::Internal,
                            )),
                        )],
                    ),
                ),
            )],
        ),
    )
    .expect("disclosure schema should validate")
}

fn register(kind: AiToolOperationKind, document: &str) -> Result<(), AiError> {
    let disclosure = disclosure_schema();
    let contract = GraphqlOperationContract::new(
        GraphqlExecutionTargetId::parse("application").expect("target ID"),
        "schema-v1",
        "Search",
        document,
        "records-projection-v1",
        disclosure.fingerprint.clone(),
    )
    .expect("operation contract should validate");
    let descriptor = AiToolDescriptor::new(
        "records.search",
        "Search records",
        kind,
        document,
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object"
        }),
    )
    .expect("descriptor construction is separate from catalog admission")
    .with_result_projection("records-projection-v1")
    .with_graphql_contract(contract);
    AiToolCatalog::new().register_with_disclosure(descriptor, disclosure)
}

#[test]
fn static_registration_requires_one_operation_of_the_declared_kind_and_name() {
    register(
        AiToolOperationKind::Query,
        "query Search { records { id } }",
    )
    .expect("exact query registers");
    register(
        AiToolOperationKind::Mutation,
        "mutation Search($id: ID!) { updateRecord(id: $id) { id } }",
    )
    .expect("exact mutation registers");

    for (kind, document) in [
        // A mutation labelled as a query would skip mutation supervision.
        (
            AiToolOperationKind::Query,
            "mutation Search($id: ID!) { deleteRecord(id: $id) { id } }",
        ),
        (
            AiToolOperationKind::Query,
            "subscription Search { records { id } }",
        ),
        (
            AiToolOperationKind::Mutation,
            "query Search { records { id } }",
        ),
        // Exactly one operation, with the contract's name.
        (
            AiToolOperationKind::Query,
            "query Search { records { id } } mutation Other { deleteRecord(id: \"1\") { id } }",
        ),
        (AiToolOperationKind::Query, "query Other { records { id } }"),
        (AiToolOperationKind::Query, "{ records { id } }"),
        (AiToolOperationKind::Query, "query Search { records { id }"),
    ] {
        assert!(
            matches!(
                register(kind, document),
                Err(AiError::InvalidConfiguration(_))
            ),
            "{kind:?} {document}"
        );
    }
}

const KNOWN_CONTROL_PLANE_ROOTS: &[&str] = &[
    "aiSessions",
    "decideAiApproval",
    "reviewAiProposal",
    "upsertAiSkill",
    "publishAiSkillVersion",
    "setAiRulePolicy",
    "aiBudgetScopeCapacity",
    "reclaimAiBudgetReservation",
    "aiUsage",
    "aiSessionEvents",
];

fn assert_root_refused(root: &str) {
    for (kind, operation) in [
        (AiToolOperationKind::Query, "query"),
        (AiToolOperationKind::Mutation, "mutation"),
        (AiToolOperationKind::Subscription, "subscription"),
    ] {
        for document in [
            format!("{operation} Search {{ {root} {{ id }} }}"),
            format!("{operation} Search {{ alias: {root} }}"),
            format!("{operation} Search {{ ... on Root {{ {root} }} }}"),
            format!("{operation} Search {{ ...Roots }} fragment Roots on Root {{ {root} }}"),
        ] {
            assert!(
                matches!(
                    register(kind, &document),
                    Err(AiError::InvalidConfiguration(_))
                ),
                "{document}"
            );
        }
    }
}

#[test]
fn static_registration_refuses_ai_control_plane_roots_in_every_case() {
    for root in KNOWN_CONTROL_PLANE_ROOTS {
        assert_root_refused(root);
        let mut pascal = root.to_string().into_bytes();
        pascal[0] = pascal[0].to_ascii_uppercase();
        assert_root_refused(&String::from_utf8(pascal).expect("ASCII root"));
    }
    assert!(
        register(
            AiToolOperationKind::Query,
            "query Search { __schema { queryType { name } } }"
        )
        .is_err()
    );
    // A control-plane root nested under another field, and federation
    // service fields, are refused like introspection.
    for document in [
        "query Search { ai { aiSessions { id } } }",
        "query Search { records { id ...Nested } } fragment Nested on Record { ai { decideAiApproval } }",
        "query Search { _service { sdl } }",
        "query Search { _entities(representations: []) { __typename } }",
    ] {
        assert!(
            matches!(
                register(AiToolOperationKind::Query, document),
                Err(AiError::InvalidConfiguration(_))
            ),
            "{document}"
        );
    }
    assert!(
        register(
            AiToolOperationKind::Query,
            "query Search { records { id ...Meta } } fragment Meta on Record { __type(name: \"Record\") { name } }"
        )
        .is_err()
    );
}

#[test]
fn control_plane_roots_are_derived_from_every_composable_ai_root() {
    let roots = ai_control_plane_root_fields();
    for expected in KNOWN_CONTROL_PLANE_ROOTS {
        assert!(
            roots.iter().any(|root| root.eq_ignore_ascii_case(expected)),
            "{expected} is missing from {roots:?}"
        );
    }
    for root in &roots {
        assert_root_refused(root);
    }
}

fn record_entity() -> GraphqlEntitySemanticMetadata {
    let scalar = |field_name: &str, scalar_name: &str| GraphqlSemanticFieldMetadata {
        field_name: field_name.to_owned(),
        description: format!("Reviewed public {field_name}."),
        type_ref: GraphqlSemanticTypeRef::named(
            scalar_name,
            GraphqlSemanticTypeKind::Scalar,
            false,
        ),
        selectable: true,
        filter_operators: Vec::new(),
        filter_input: None,
        sortable: false,
        groupable: false,
        aggregate_operators: Vec::new(),
        aggregate_value_kind: None,
        relationship: None,
        classification: GraphqlSemanticClassification::Internal,
        export: GraphqlSemanticExport::Exportable,
        has_field_policy: false,
    };
    GraphqlEntitySemanticMetadata {
        entity_name: "Record".to_owned(),
        description: "A reviewed application record.".to_owned(),
        default_classification: GraphqlSemanticClassification::Internal,
        fields: vec![scalar("recordId", "ID"), scalar("subject", "String")].into_boxed_slice(),
        label_fields: Vec::new(),
        record_version: None,
    }
}

fn record_operation(
    kind: GraphqlOperationKind,
    field_name: &str,
) -> GraphqlSemanticOperationDescriptor {
    let operation = GraphqlSemanticOperationDescriptor::custom(
        kind,
        field_name,
        "Operate on one reviewed application record.",
        vec![GraphqlSemanticArgumentDescriptor {
            graphql_name: "recordId".to_owned(),
            description: "Reviewed record identifier.".to_owned(),
            type_ref: GraphqlSemanticTypeRef::named("ID", GraphqlSemanticTypeKind::Scalar, false),
        }],
        GraphqlSemanticTypeRef::named("Record", GraphqlSemanticTypeKind::Object, false),
        true,
    )
    .expect("test operation semantics should validate");
    if kind == GraphqlOperationKind::Mutation {
        operation
            .with_ai_mutation_execution(AiMutationExecutionPolicy::Automatic)
            .expect("test mutation policy should validate")
    } else {
        operation
    }
}

fn target() -> GraphqlExecutionTargetId {
    GraphqlExecutionTargetId::parse("private-router").expect("target should validate")
}

fn query_catalog(root: &str) -> AiGraphqlQueryCapabilityCatalog {
    let sdl = format!(
        "schema {{ query: Query }} type Query {{ {root}(recordId: ID!): Record! }} type Record {{ recordId: ID!, subject: String! }}"
    );
    let semantics = GraphqlSemanticCatalog::compose_with_custom(
        [record_entity()],
        &GraphqlOperationCatalog::compose(std::iter::empty()),
        [record_operation(GraphqlOperationKind::Query, root)],
    )
    .expect("query catalogue should validate");
    AiGraphqlQueryCapabilityCatalog::compile(
        "generated-read",
        target(),
        &sdl,
        &semantics,
        AiGraphqlQueryCapabilityLimits::default(),
    )
    .expect("query capabilities compile without AI knowledge")
}

fn mutation_catalog(root: &str) -> AiGraphqlMutationCapabilityCatalog {
    let sdl = format!(
        "schema {{ query: Query, mutation: Mutation }} type Query {{ GeneratedRecord(recordId: ID!): Record! }} type Mutation {{ {root}(recordId: ID!): Record! }} type Record {{ recordId: ID!, subject: String! }}"
    );
    let semantics = GraphqlSemanticCatalog::compose_with_custom(
        [record_entity()],
        &GraphqlOperationCatalog::compose(std::iter::empty()),
        [
            record_operation(GraphqlOperationKind::Query, "GeneratedRecord"),
            record_operation(GraphqlOperationKind::Mutation, root),
        ],
    )
    .expect("mutation catalogue should validate");
    AiGraphqlMutationCapabilityCatalog::compile(
        "generated-write",
        target(),
        &sdl,
        &semantics,
        AiGraphqlQueryCapabilityLimits::default(),
    )
    .expect("mutation capabilities compile without AI knowledge")
}

#[test]
fn generated_capabilities_refuse_ai_control_plane_roots_all_or_nothing() {
    let mut catalog = AiToolCatalog::new();
    catalog
        .register_query_capability_catalog(&query_catalog("GeneratedRecord"))
        .expect("application query capability registers");
    for root in ["aiSessions", "AiApprovals", "ai_skills"] {
        let mut catalog = AiToolCatalog::new();
        assert!(
            matches!(
                catalog.register_query_capability_catalog(&query_catalog(root)),
                Err(AiError::InvalidConfiguration(_))
            ),
            "{root}"
        );
        assert_eq!(catalog.query_capabilities().count(), 0);
    }

    let mut catalog = AiToolCatalog::new();
    catalog
        .register_mutation_capability_catalog(&mutation_catalog("UpdateRecord"))
        .expect("application mutation capability registers");
    for root in [
        "decideAiApproval",
        "ReviewAiProposal",
        "reclaim_ai_budget_reservation",
    ] {
        let mut catalog = AiToolCatalog::new();
        assert!(
            matches!(
                catalog.register_mutation_capability_catalog(&mutation_catalog(root)),
                Err(AiError::InvalidConfiguration(_))
            ),
            "{root}"
        );
        assert_eq!(catalog.mutation_capabilities().count(), 0);
    }
}

fn subscription_catalog(root: &str) -> AiGraphqlSubscriptionCapabilityCatalog {
    let sdl = format!(
        "schema {{ query: Query, subscription: Subscription }} type Query {{ health: Boolean! }} type Subscription {{ {root}: Record! }} type Record {{ recordId: ID!, subject: String! }}"
    );
    let operation = GraphqlSemanticOperationDescriptor::custom(
        GraphqlOperationKind::Subscription,
        root,
        "Observe reviewed record changes.",
        Vec::new(),
        GraphqlSemanticTypeRef::named("Record", GraphqlSemanticTypeKind::Object, false),
        true,
    )
    .expect("test subscription semantics should validate")
    .with_subscription_observation(GraphqlSubscriptionObservationDescriptor {
        replay_mode: GraphqlSubscriptionReplayMode::ReplayThenLive,
        maximum_duration_seconds: Some(120),
        maximum_events: Some(20),
        condition_fields: vec![GraphqlSubscriptionConditionField {
            field_name: "recordId".to_owned(),
            operators: vec![GraphqlSubscriptionConditionOperator::Equal],
        }],
    })
    .expect("test replayable subscription should validate");
    let semantics = GraphqlSemanticCatalog::compose_with_custom(
        [record_entity()],
        &GraphqlOperationCatalog::compose(std::iter::empty()),
        [operation],
    )
    .expect("subscription catalogue should validate");
    AiGraphqlSubscriptionCapabilityCatalog::compile(
        "generated-subscription",
        target(),
        &sdl,
        &semantics,
        AiGraphqlSubscriptionCapabilityLimits::default(),
    )
    .expect("subscription capabilities compile without AI knowledge")
}

#[test]
fn generated_subscription_capabilities_refuse_ai_control_plane_roots() {
    AiToolCatalog::new()
        .register_subscription_capability_catalog(&subscription_catalog("RecordChanged"))
        .expect("application subscription capability registers");
    for root in ["aiSessionEvents", "AiSessionEvents"] {
        let mut catalog = AiToolCatalog::new();
        assert!(
            matches!(
                catalog.register_subscription_capability_catalog(&subscription_catalog(root)),
                Err(AiError::InvalidConfiguration(_))
            ),
            "{root}"
        );
        assert_eq!(catalog.subscription_capabilities().count(), 0);
    }
}
