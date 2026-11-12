//! Disclosure follows each field's own canonical coordinate, never a lexical
//! name prefix shared with another field.
//!
//! `Subscription.ownerId` is an exportable filterable scalar beside a separate
//! `NeverExport` relation `Subscription.owner` and a private scalar
//! `Subscription.ownerEmail`. The public scalar must stay available for
//! filters, ordering, selections and the provider-facing `where`/`orderBy`
//! input schemas, while the private scalar, the private relation and fields
//! selected through it stay closed. The exportable relation
//! `Subscription.plan` checks nested selections: `plan.code` stays beside a
//! private `plan.codeKey`. Relation filters (`where.plan`, `where.owner`) are
//! dropped entirely, whatever the target field's disclosure, so they prove
//! nothing about nested field privacy and are asserted only as dropped.

use std::collections::BTreeSet;

use graphql_orm_ai_tool_profiles::{
    AiGraphqlQueryCapability, AiGraphqlQueryCapabilityCatalog, AiGraphqlQueryCapabilityLimits,
    GraphqlExecutionTargetId,
};
use graphql_orm_operation_catalog::{
    GeneratedGraphqlOperationCategory, GraphqlEntitySemanticMetadata, GraphqlOperationKind,
    GraphqlSemanticArgumentDescriptor, GraphqlSemanticCatalog, GraphqlSemanticClassification,
    GraphqlSemanticExport, GraphqlSemanticFieldMetadata, GraphqlSemanticFilterInputMetadata,
    GraphqlSemanticFilterInputOperator, GraphqlSemanticFilterOperator,
    GraphqlSemanticOperationDescriptor, GraphqlSemanticRelationshipCardinality,
    GraphqlSemanticRelationshipDescriptor, GraphqlSemanticTypeKind, GraphqlSemanticTypeRef,
};
use serde_json::{Value, json};

const SDL: &str = r#"
    schema { query: Query }
    type Query {
        Subscriptions(where: SubscriptionWhereInput,
            orderBy: [SubscriptionOrderByInput!], page: PageInput): SubscriptionConnection!
    }
    type SubscriptionConnection { edges: [SubscriptionEdge!]! }
    type SubscriptionEdge { node: Subscription! }
    type Subscription {
        id: String!, name: String!, ownerId: String!, ownerEmail: String!
        owner: Owner, plan: Plan
    }
    type Owner { id: String! }
    type Plan { code: String!, codeKey: String! }
    input SubscriptionWhereInput {
        id: StringFilterInput, ownerId: StringFilterInput, ownerEmail: StringFilterInput
        owner: OwnerWhereInput, ownerSome: OwnerWhereInput, ownerIsNull: Boolean
        plan: PlanWhereInput
        ownerIdExtra: StringFilterInput, owner_id: StringFilterInput, OWNERID: StringFilterInput
        and: [SubscriptionWhereInput!], or: [SubscriptionWhereInput!], not: SubscriptionWhereInput
    }
    input OwnerWhereInput { id: StringFilterInput }
    input PlanWhereInput { code: StringFilterInput, codeKey: StringFilterInput }
    input SubscriptionOrderByInput {
        ownerId: OrderDirection, ownerEmail: OrderDirection, ownerCount: OrderDirection
    }
    input StringFilterInput { eq: String, isNull: Boolean }
    input PageInput { limit: Int, offset: Int }
    enum OrderDirection { ASC DESC }
"#;

fn object(name: &str, nullable: bool) -> GraphqlSemanticTypeRef {
    GraphqlSemanticTypeRef::named(name, GraphqlSemanticTypeKind::Object, nullable)
}

fn string_filter() -> GraphqlSemanticFilterInputMetadata {
    GraphqlSemanticFilterInputMetadata::new(
        object("StringFilterInput", true),
        GraphqlSemanticTypeRef::named("String", GraphqlSemanticTypeKind::Scalar, false),
        vec![
            GraphqlSemanticFilterInputOperator {
                graphql_name: "eq".to_owned(),
                operator: GraphqlSemanticFilterOperator::Equal,
                operand_type: GraphqlSemanticTypeRef::named(
                    "String",
                    GraphqlSemanticTypeKind::Scalar,
                    true,
                ),
            },
            GraphqlSemanticFilterInputOperator {
                graphql_name: "isNull".to_owned(),
                operator: GraphqlSemanticFilterOperator::IsNull,
                operand_type: GraphqlSemanticTypeRef::named(
                    "Boolean",
                    GraphqlSemanticTypeKind::Scalar,
                    true,
                ),
            },
        ],
        Vec::new(),
    )
    .expect("string filter contract")
}

fn field(
    name: &str,
    export: GraphqlSemanticExport,
    filterable: bool,
) -> GraphqlSemanticFieldMetadata {
    GraphqlSemanticFieldMetadata {
        field_name: name.to_owned(),
        description: format!("Reviewed {name} value."),
        type_ref: GraphqlSemanticTypeRef::named("String", GraphqlSemanticTypeKind::Scalar, false),
        selectable: true,
        filter_operators: if filterable {
            vec![
                GraphqlSemanticFilterOperator::Equal,
                GraphqlSemanticFilterOperator::IsNull,
            ]
        } else {
            Vec::new()
        },
        filter_input: filterable.then(string_filter),
        sortable: filterable,
        groupable: false,
        aggregate_operators: Vec::new(),
        aggregate_value_kind: None,
        relationship: None,
        classification: GraphqlSemanticClassification::Internal,
        export,
        has_field_policy: false,
    }
}

fn relation(
    name: &str,
    target: &str,
    export: GraphqlSemanticExport,
) -> GraphqlSemanticFieldMetadata {
    GraphqlSemanticFieldMetadata {
        field_name: name.to_owned(),
        description: format!("Reviewed {name} relationship."),
        type_ref: object(target, true),
        selectable: true,
        filter_operators: Vec::new(),
        filter_input: None,
        sortable: false,
        groupable: false,
        aggregate_operators: Vec::new(),
        aggregate_value_kind: None,
        relationship: Some(GraphqlSemanticRelationshipDescriptor {
            target: target.to_owned(),
            cardinality: GraphqlSemanticRelationshipCardinality::One,
            arguments: Vec::new(),
            collection_bound: None,
        }),
        classification: GraphqlSemanticClassification::Internal,
        export,
        has_field_policy: true,
    }
}

fn entity(name: &str, fields: Vec<GraphqlSemanticFieldMetadata>) -> GraphqlEntitySemanticMetadata {
    GraphqlEntitySemanticMetadata {
        entity_name: name.to_owned(),
        description: format!("Reviewed {name} record."),
        default_classification: GraphqlSemanticClassification::Internal,
        fields: fields.into_boxed_slice(),
        label_fields: Vec::new(),
        record_version: None,
    }
}

fn capability_catalog() -> AiGraphqlQueryCapabilityCatalog {
    use GraphqlSemanticExport::{Exportable, NeverExport};
    let argument = |name: &str, type_ref| GraphqlSemanticArgumentDescriptor {
        graphql_name: name.to_owned(),
        description: format!("Typed {name} argument."),
        type_ref,
    };
    // A generated list root: generated-kind inputs follow entity metadata
    // only when that entity's generated roots reach them.
    let operation = GraphqlSemanticOperationDescriptor::generated(
        GraphqlOperationKind::Query,
        GeneratedGraphqlOperationCategory::List,
        "Subscription",
        "Subscriptions",
        "Read one bounded subscription page.",
        vec![
            argument("where", object("SubscriptionWhereInput", true)),
            argument(
                "orderBy",
                GraphqlSemanticTypeRef::list(true, None, object("SubscriptionOrderByInput", false)),
            ),
            argument("page", object("PageInput", true)),
        ],
        object("SubscriptionConnection", false),
        true,
    )
    .expect("subscription list semantics");
    let semantics = GraphqlSemanticCatalog::compose_descriptors(
        [
            entity(
                "Subscription",
                vec![
                    field("id", Exportable, true),
                    field("name", Exportable, false),
                    field("ownerId", Exportable, true),
                    field("ownerEmail", NeverExport, true),
                    relation("owner", "Owner", NeverExport),
                    relation("plan", "Plan", Exportable),
                ],
            ),
            // The private relation's target is itself exportable: the private
            // relation, not the target's own metadata, must close owner.id.
            entity("Owner", vec![field("id", Exportable, true)]),
            entity(
                "Plan",
                vec![
                    field("code", Exportable, true),
                    field("codeKey", NeverExport, true),
                ],
            ),
        ],
        [operation],
    )
    .expect("subscription semantics");
    AiGraphqlQueryCapabilityCatalog::compile(
        "inventory",
        GraphqlExecutionTargetId::parse("inventory.graphql").expect("target"),
        SDL,
        &semantics,
        AiGraphqlQueryCapabilityLimits::default(),
    )
    .expect("subscription capabilities")
}

fn capability(catalog: &AiGraphqlQueryCapabilityCatalog) -> &AiGraphqlQueryCapability {
    let mut capabilities = catalog.capabilities();
    let capability = capabilities.next().expect("subscription capability");
    assert!(capabilities.next().is_none());
    capability
}

fn plan() -> Value {
    json!({
        "arguments": { "where": { "ownerId": { "eq": "owner-1" } } },
        "relationshipArguments": {},
        "relationshipMaximumItems": {},
        "selections": ["id", "name", "ownerId"],
        "maximumItems": 1
    })
}

fn nested(compact: &Value) -> Value {
    json!({
        "arguments": compact["arguments"],
        "fields": { "id": true, "name": true, "ownerId": true },
        "relationships": {},
        "maximumItems": compact["maximumItems"]
    })
}

/// Exact property names of the `where` and `orderBy` input objects, which are
/// identical in the compact and nested plan contracts.
fn input_properties(schema: &Value) -> (BTreeSet<&str>, BTreeSet<&str>) {
    let keys = |pointer: &str| {
        schema
            .pointer(pointer)
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("input properties at {pointer}"))
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
    };
    (
        keys("/properties/arguments/properties/where/anyOf/0/properties"),
        keys("/properties/arguments/properties/orderBy/anyOf/0/items/properties"),
    )
}

fn nested_selection(relationship: &str, field: &str) -> Value {
    json!({
        "arguments": {},
        "fields": { "id": true },
        "relationships": {
            (relationship): { "arguments": {}, "fields": { (field): true }, "relationships": {} }
        },
        "maximumItems": 1
    })
}

fn property_names(value: &Value, names: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::Object(properties)) = map.get("properties") {
                names.extend(properties.keys().cloned());
            }
            map.values().for_each(|value| property_names(value, names));
        }
        Value::Array(items) => items.iter().for_each(|value| property_names(value, names)),
        _ => {}
    }
}

#[test]
fn exportable_scalar_filter_survives_private_relation_name_prefix() {
    let catalog = capability_catalog();
    let capability = capability(&catalog);

    let compact = capability
        .compile_compact(plan())
        .expect("public ownerId filter beside private owner relation");
    let suffix = "($v0: PageInput, $v1: SubscriptionWhereInput) { Subscriptions(page: $v0, where: $v1) { edges { node { id name ownerId } } } }";
    assert_eq!(
        compact.variables(),
        &json!({ "v0": { "limit": 1 }, "v1": { "ownerId": { "eq": "owner-1" } } })
    );
    assert!(compact.descriptor().document.ends_with(suffix));
    let nested = capability
        .compile(nested(&plan()))
        .expect("same plan through the nested contract");
    assert_eq!(nested.variables(), compact.variables());
    assert!(nested.descriptor().document.ends_with(suffix));

    let mut ordered = plan();
    ordered["arguments"]["orderBy"] = json!([{ "ownerId": "ASC" }]);
    capability
        .compile_compact(ordered)
        .expect("public ownerId ordering");

    let mut related = plan();
    related["selections"] = json!(["id", "plan.code"]);
    capability
        .compile_compact(related)
        .expect("public field nested under an exportable relation");
    capability
        .compile(nested_selection("plan", "code"))
        .expect("public field nested under an exportable relation (nested contract)");

    // The provider-facing input schemas describe exactly the public filter
    // and ordering scalars: ownerId is present, and nothing else survives.
    for schema in [
        capability.compact_argument_schema(),
        capability.argument_schema(),
    ] {
        let (filters, ordering) = input_properties(schema);
        assert_eq!(filters, BTreeSet::from(["id", "ownerId"]), "where inputs");
        assert_eq!(ordering, BTreeSet::from(["ownerId"]), "orderBy inputs");
    }
    let selections = capability
        .compact_argument_schema()
        .pointer("/properties/selections/items/enum")
        .and_then(Value::as_array)
        .expect("closed selection allow-list");
    for public in ["id", "name", "ownerId", "plan.code"] {
        assert!(selections.contains(&json!(public)), "selection {public}");
    }
}

#[test]
fn private_relation_and_lookalike_fields_stay_closed() {
    let catalog = capability_catalog();
    let capability = capability(&catalog);

    let private_filters = [
        ("ownerEmail", json!({ "eq": "private-value" })),
        ("owner", json!({ "id": { "eq": "owner-1" } })),
        ("ownerSome", json!({ "id": { "eq": "owner-1" } })),
        ("ownerIsNull", json!(true)),
        ("ownerIdExtra", json!({ "eq": "owner-1" })),
        ("owner_id", json!({ "eq": "owner-1" })),
        ("OWNERID", json!({ "eq": "owner-1" })),
    ];
    for (name, value) in private_filters {
        let mut invalid = plan();
        invalid["arguments"]["where"] = json!({ (name): value });
        assert!(
            capability.compile_compact(invalid.clone()).is_err(),
            "compact filter {name}"
        );
        assert!(
            capability.compile(nested(&invalid)).is_err(),
            "nested filter {name}"
        );
    }

    // Relation filters are dropped entirely, so even a filter on the public
    // Plan.code is refused. This is not evidence that Plan.codeKey is private;
    // nested privacy is proven by the selection cases below.
    for value in [
        json!({ "code": { "eq": "public-value" } }),
        json!({ "codeKey": { "eq": "private-value" } }),
    ] {
        let mut invalid = plan();
        invalid["arguments"]["where"] = json!({ "plan": value });
        assert!(capability.compile_compact(invalid.clone()).is_err());
        assert!(capability.compile(nested(&invalid)).is_err());
    }

    for name in ["ownerEmail", "ownerCount"] {
        let mut invalid = plan();
        invalid["arguments"]["orderBy"] = json!([{ (name): "ASC" }]);
        assert!(
            capability.compile_compact(invalid).is_err(),
            "ordering {name}"
        );
    }

    for path in ["ownerEmail", "owner.id", "plan.codeKey"] {
        let mut invalid = plan();
        invalid["selections"] = json!([path]);
        assert!(
            capability.compile_compact(invalid).is_err(),
            "selection {path}"
        );
    }
    for (relationship, field) in [("owner", "id"), ("plan", "codeKey")] {
        assert!(
            capability
                .compile(nested_selection(relationship, field))
                .is_err(),
            "nested selection {relationship}.{field}"
        );
    }

    // Nothing private is described to the provider, even by a shared prefix.
    for schema in [
        capability.compact_argument_schema(),
        capability.argument_schema(),
    ] {
        let mut names = BTreeSet::new();
        property_names(schema, &mut names);
        for private in [
            "ownerEmail",
            "owner",
            "ownerSome",
            "ownerIsNull",
            "ownerCount",
            "codeKey",
            "ownerIdExtra",
            "owner_id",
            "OWNERID",
        ] {
            assert!(!names.contains(private), "described private {private}");
        }
    }
    let selections = capability
        .compact_argument_schema()
        .pointer("/properties/selections/items/enum")
        .and_then(Value::as_array)
        .expect("closed selection allow-list");
    for private in ["ownerEmail", "owner.id", "plan.codeKey"] {
        assert!(
            !selections.contains(&json!(private)),
            "selection allow-list {private}"
        );
    }
}
