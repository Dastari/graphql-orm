use graphql_orm_ai_tool_profiles::{
    AI_GRAPHQL_TOOL_MANIFEST_EXTENSION_NAME, AI_GRAPHQL_TOOL_MANIFEST_VERSION, AiDisclosureRule,
    AiDisclosureSchema, AiDisclosureShape, AiGraphqlArgumentPlan, AiGraphqlArgumentValue,
    AiGraphqlProfileInput, AiGraphqlSelection, AiGraphqlToolManifest, AiGraphqlToolManifestBuilder,
    AiGraphqlToolProfile, DataClassification, GraphqlExecutionTargetId,
};
use graphql_orm_router_protocol::{
    AuthorizationRequirement, DescriptorExtension, Fingerprint, OperationDescriptor,
    RootOperationType, SubgraphDescriptor, SubgraphDescriptorBuilder,
};
use serde_json::{Map, Value};

const FINISHED_SDL: &str = r#"
    schema { query: Query }
    type Query {
        tickets(filter: TicketFilter!, page: PageInput!): TicketConnection!
    }
    input TicketFilter { summary: StringFilter, customer: CustomerFilter }
    input CustomerFilter { name: StringFilter }
    input StringFilter { contains: String }
    input PageInput { first: Int! }
    type TicketConnection { nodes: [Ticket!]!, pageInfo: PageInfo! }
    type Ticket { id: ID!, summary: String!, customer: Customer! }
    type Customer { id: ID!, name: String! }
    type PageInfo { totalCount: Int!, hasNextPage: Boolean! }
"#;

fn rule() -> AiDisclosureRule {
    AiDisclosureRule::exportable(DataClassification::Confidential)
}

fn scalar() -> AiDisclosureShape {
    AiDisclosureShape::scalar(rule())
}

fn object(fields: impl IntoIterator<Item = (String, AiDisclosureShape)>) -> AiDisclosureShape {
    AiDisclosureShape::object(rule(), fields)
}

fn list(maximum_items: u32, item: AiDisclosureShape) -> AiDisclosureShape {
    AiDisclosureShape::list(rule(), maximum_items, item)
}

fn complex_manifest() -> AiGraphqlToolManifest {
    let disclosure = AiDisclosureSchema::new(
        "tickets-browser-v1",
        object([(
            "tickets".to_owned(),
            object([
                (
                    "nodes".to_owned(),
                    list(
                        5,
                        object([
                            ("id".to_owned(), scalar()),
                            ("summary".to_owned(), scalar()),
                            (
                                "customer".to_owned(),
                                object([
                                    ("id".to_owned(), scalar()),
                                    ("name".to_owned(), scalar()),
                                ]),
                            ),
                        ]),
                    ),
                ),
                (
                    "pageInfo".to_owned(),
                    object([
                        ("totalCount".to_owned(), scalar()),
                        ("hasNextPage".to_owned(), scalar()),
                    ]),
                ),
            ]),
        )]),
    )
    .expect("complex disclosure should validate");
    let profile = AiGraphqlToolProfile::read_only(
        "bounded-browser-list",
        "tickets",
        "Find a bounded ticket list with reviewed customer details",
        vec![
            AiGraphqlSelection::bounded_list(
                "nodes",
                5,
                [
                    AiGraphqlSelection::scalar("id"),
                    AiGraphqlSelection::scalar("summary"),
                    AiGraphqlSelection::object(
                        "customer",
                        [
                            AiGraphqlSelection::scalar("id"),
                            AiGraphqlSelection::scalar("name"),
                        ],
                    ),
                ],
            ),
            AiGraphqlSelection::object(
                "pageInfo",
                [
                    AiGraphqlSelection::scalar("totalCount"),
                    AiGraphqlSelection::scalar("hasNextPage"),
                ],
            ),
        ],
        disclosure,
        32 * 1024,
        12,
    )
    .with_inputs([
        AiGraphqlProfileInput::string("Query", "Ticket text to find", true, 1, 120),
        AiGraphqlProfileInput::integer("Limit", "Maximum tickets to return", true, 1, 5),
    ])
    .with_arguments([
        AiGraphqlArgumentPlan::new(
            "filter",
            AiGraphqlArgumentValue::object([
                (
                    "summary",
                    AiGraphqlArgumentValue::object([(
                        "contains",
                        AiGraphqlArgumentValue::input("Query"),
                    )]),
                ),
                (
                    "customer",
                    AiGraphqlArgumentValue::object([(
                        "name",
                        AiGraphqlArgumentValue::object([(
                            "contains",
                            AiGraphqlArgumentValue::input("Query"),
                        )]),
                    )]),
                ),
            ]),
        ),
        AiGraphqlArgumentPlan::new(
            "page",
            AiGraphqlArgumentValue::object([("first", AiGraphqlArgumentValue::input("Limit"))]),
        ),
    ]);
    let mut builder = AiGraphqlToolManifestBuilder::new(
        "support-service",
        GraphqlExecutionTargetId::parse("application-graph")
            .expect("target identity should validate"),
        FINISHED_SDL,
    )
    .expect("finished schema should validate");
    builder
        .add_custom_profile(profile)
        .expect("complex custom profile should compile");
    builder.build().expect("manifest should build")
}

fn reverse_object_keys(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries = object.into_iter().collect::<Vec<_>>();
            entries.reverse();
            let mut reversed = Map::new();
            for (key, value) in entries {
                reversed.insert(key, reverse_object_keys(value));
            }
            Value::Object(reversed)
        }
        Value::Array(values) => Value::Array(values.into_iter().map(reverse_object_keys).collect()),
        scalar => scalar,
    }
}

#[test]
fn complex_manifest_survives_canonical_descriptor_extension_roundtrip() {
    let manifest = complex_manifest();
    let payload = manifest
        .extension_payload()
        .expect("manifest should encode");
    let extension = DescriptorExtension::new(
        AI_GRAPHQL_TOOL_MANIFEST_EXTENSION_NAME,
        AI_GRAPHQL_TOOL_MANIFEST_VERSION,
        payload,
    )
    .expect("extension should validate");
    let descriptor = SubgraphDescriptorBuilder::new(
        "support-service",
        "Support",
        "http://support.internal/graphql",
        "http://support.internal/.well-known/sdl",
        Fingerprint::sha256(FINISHED_SDL),
    )
    .expect("descriptor identity should validate")
    .operation(OperationDescriptor {
        root_type: RootOperationType::Query,
        field_name: "tickets".to_owned(),
        arguments: Vec::new(),
        authorization: AuthorizationRequirement::Authenticated,
    })
    .extension(extension)
    .build()
    .expect("descriptor should build");
    let encoded = serde_json::to_string(&descriptor).expect("descriptor should encode");
    let decoded = SubgraphDescriptor::from_json_compatible(&encoded)
        .expect("complete descriptor should decode and validate");
    let transported = decoded
        .extensions
        .iter()
        .find(|extension| extension.name == AI_GRAPHQL_TOOL_MANIFEST_EXTENSION_NAME)
        .expect("tool manifest extension should be present");
    let decoded_manifest =
        AiGraphqlToolManifest::from_extension_payload(transported.payload.clone())
            .expect("canonical router transport must preserve the manifest");
    assert_eq!(decoded_manifest, manifest);
}

#[test]
fn arbitrary_recursive_object_key_reordering_preserves_fingerprints() {
    let manifest = complex_manifest();
    let payload = manifest
        .extension_payload()
        .expect("manifest should encode");
    let reordered = reverse_object_keys(payload.clone());
    assert_ne!(
        serde_json::to_vec(&payload).expect("payload should encode"),
        serde_json::to_vec(&reordered).expect("reordered payload should encode"),
        "preserve_order must make this a meaningful transport-order test",
    );
    let decoded = AiGraphqlToolManifest::from_extension_payload(reordered)
        .expect("object-key order must not change validation");
    assert_eq!(decoded.fingerprint, manifest.fingerprint);
    assert_eq!(
        decoded.entries[0].descriptor.fingerprint,
        manifest.entries[0].descriptor.fingerprint
    );

    let mut tampered = payload;
    let mut legacy = tampered.clone();
    legacy["version"] = Value::from(1);
    assert!(AiGraphqlToolManifest::from_extension_payload(legacy).is_err());

    tampered["entries"][0]["descriptor"]["maximum_result_bytes"] = Value::from(1);
    assert!(AiGraphqlToolManifest::from_extension_payload(tampered).is_err());
}
