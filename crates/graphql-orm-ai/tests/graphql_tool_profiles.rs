#![cfg(feature = "sqlite")]

use std::collections::BTreeMap;

use graphql_orm::prelude::*;
use graphql_orm_ai::*;

mod generated_surface {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "profile_test_records",
        plural = "ProfileTestRecords",
        description = "A visible record available for reviewed application workflows"
    )]
    pub struct ProfileTestRecord {
        #[primary_key]
        #[graphql_orm(description = "Stable public record identity")]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        #[graphql_orm(description = "Human-facing record label")]
        pub label: String,
        #[graphql_orm(description = "Sensitive accounting marker")]
        pub sensitive_ledger: String,
        #[graphql_orm(description = "Internal refresh marker")]
        pub cache_marker: String,
    }

    schema_roots! {
        entities: [ProfileTestRecord],
    }
}

struct AdmitGenerated;

impl AiGeneratedGraphqlOperationPolicy for AdmitGenerated {
    fn is_application_operation(&self, operation: &GraphqlResolverOperationDescriptor) -> bool {
        operation.entity_name() == "ProfileTestRecord"
    }
}

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

fn disclosure(
    version: &str,
    root_field: &str,
    root_shape: AiDisclosureShape,
) -> AiDisclosureSchema {
    AiDisclosureSchema::new(version, object([(root_field.to_owned(), root_shape)])).unwrap()
}

const CUSTOM_SDL: &str = r#"
    schema { query: ApplicationQuery mutation: ApplicationMutation }
    type ApplicationQuery {
        records(where: RecordWhere, page: PageInput): RecordConnection!
        record(id: ID!): Record
        endpointSummary(limit: Int!): EndpointSummary!
    }
    type ApplicationMutation { renameRecord(id: ID!, label: String!): Record! }
    input RecordWhere { label: StringFilter }
    input StringFilter { contains: String }
    input PageInput { first: Int }
    type RecordConnection { nodes: [Record!]!, pageInfo: PageInfo! }
    type PageInfo { totalCount: Int! }
    type Record {
        id: ID!
        label: String!
        sensitiveLedger: String!
        cacheMarker: String!
        related(limit: Int!): [RelatedRecord!]!
    }
    type RelatedRecord { id: ID!, privateIdentity: String! }
    type EndpointSummary { connected: Int!, tenantIdentity: ID! }
"#;

fn target() -> GraphqlExecutionTargetId {
    GraphqlExecutionTargetId::parse("application-graph").unwrap()
}

fn count_profile() -> AiGraphqlToolProfile {
    AiGraphqlToolProfile::read_only(
        "count",
        "records",
        "Count visible records matching the reviewed fixed view",
        vec![AiGraphqlSelection::object(
            "pageInfo",
            [AiGraphqlSelection::scalar("totalCount")],
        )],
        disclosure(
            "record-count-v1",
            "records",
            object([(
                "pageInfo".to_owned(),
                object([("totalCount".to_owned(), scalar())]),
            )]),
        ),
        4096,
        1,
    )
    .with_arguments([AiGraphqlArgumentPlan::new(
        "page",
        AiGraphqlArgumentValue::object([("first", AiGraphqlArgumentValue::fixed(1))]),
    )])
}

fn list_profile() -> AiGraphqlToolProfile {
    AiGraphqlToolProfile::read_only(
        "bounded-list",
        "records",
        "List a bounded set of visible records by a reviewed label search",
        vec![
            AiGraphqlSelection::bounded_list(
                "nodes",
                25,
                [
                    AiGraphqlSelection::scalar("id"),
                    AiGraphqlSelection::scalar("label"),
                ],
            ),
            AiGraphqlSelection::object("pageInfo", [AiGraphqlSelection::scalar("totalCount")]),
        ],
        disclosure(
            "record-list-v1",
            "records",
            object([
                (
                    "nodes".to_owned(),
                    list(
                        25,
                        object([("id".to_owned(), scalar()), ("label".to_owned(), scalar())]),
                    ),
                ),
                (
                    "pageInfo".to_owned(),
                    object([("totalCount".to_owned(), scalar())]),
                ),
            ]),
        ),
        32 * 1024,
        25,
    )
    .with_inputs([
        AiGraphqlProfileInput::string("Query", "Label text to find", true, 1, 100),
        AiGraphqlProfileInput::integer("Limit", "Maximum records to return", true, 1, 25),
    ])
    .with_arguments([
        AiGraphqlArgumentPlan::new(
            "where",
            AiGraphqlArgumentValue::object([(
                "label",
                AiGraphqlArgumentValue::object([(
                    "contains",
                    AiGraphqlArgumentValue::input("Query"),
                )]),
            )]),
        ),
        AiGraphqlArgumentPlan::new(
            "page",
            AiGraphqlArgumentValue::object([("first", AiGraphqlArgumentValue::input("Limit"))]),
        ),
    ])
}

#[test]
fn semantic_entity_metadata_is_public_only_and_descriptive() {
    let metadata = generated_surface::ProfileTestRecord::graphql_semantic_metadata().unwrap();
    assert_eq!(
        metadata.description,
        "A visible record available for reviewed application workflows"
    );
    #[cfg(not(feature = "graphql-case-pascal"))]
    assert_eq!(metadata.fields[0].field_name, "id");
    #[cfg(feature = "graphql-case-pascal")]
    assert_eq!(metadata.fields[0].field_name, "Id");
    assert_eq!(
        metadata.fields[0].description,
        "Stable public record identity"
    );
    assert!(metadata.fields.iter().all(|field| !field.is_relationship));
}

#[test]
fn one_root_compiles_multiple_least_disclosure_profiles_without_raw_documents() {
    let mut builder =
        AiGraphqlToolManifestBuilder::new("records-service", target(), CUSTOM_SDL).unwrap();
    builder.add_custom_profile(count_profile()).unwrap();
    builder.add_custom_profile(list_profile()).unwrap();
    let manifest = builder.build().unwrap();
    assert_eq!(manifest.entries.len(), 2);
    assert_ne!(
        manifest.entries[0].descriptor.id,
        manifest.entries[1].descriptor.id
    );

    let documents = manifest
        .entries
        .iter()
        .map(|entry| entry.descriptor.document.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!documents.contains("sensitiveLedger"));
    assert!(!documents.contains("cacheMarker"));
    assert!(!documents.contains("related"));
    assert!(documents.contains("page: { first: 1 }"));
    assert!(documents.contains("contains: $Query"));
    assert!(documents.contains("first: $Limit"));

    let list = manifest
        .entries
        .iter()
        .find(|entry| entry.profile_id == "bounded-list")
        .unwrap();
    assert_eq!(
        list.descriptor.argument_schema["additionalProperties"],
        false
    );
    assert_eq!(
        list.descriptor.argument_schema["properties"]["Limit"]["maximum"],
        25
    );
    assert!(
        list.disclosure_schema
            .evaluate(&serde_json::json!({
                "records": {
                    "nodes": [{
                        "id": "record-1",
                        "label": "Visible",
                        "sensitiveLedger": "must-not-pass"
                    }],
                    "pageInfo": { "totalCount": 1 }
                }
            }))
            .is_err(),
        "a host returning an unselected sensitive field still fails disclosure"
    );
}

#[test]
fn semantic_aliases_custom_roots_and_explicit_relationships_compile_safely() {
    let record = AiGraphqlToolProfile::read_only(
        "details",
        "record",
        "Show reviewed details for one visible record",
        vec![
            AiGraphqlSelection::scalar("id"),
            AiGraphqlSelection::scalar("label"),
            AiGraphqlSelection::bounded_list("related", 3, [AiGraphqlSelection::scalar("id")])
                .with_arguments([AiGraphqlArgumentPlan::new(
                    "limit",
                    AiGraphqlArgumentValue::fixed(3),
                )]),
        ],
        disclosure(
            "record-details-v1",
            "record",
            object([
                ("id".to_owned(), scalar()),
                ("label".to_owned(), scalar()),
                (
                    "related".to_owned(),
                    list(3, object([("id".to_owned(), scalar())])),
                ),
            ]),
        ),
        16 * 1024,
        3,
    )
    .with_inputs([AiGraphqlProfileInput::string(
        "RecordNo",
        "Public record number",
        true,
        1,
        64,
    )])
    .with_arguments([AiGraphqlArgumentPlan::new(
        "id",
        AiGraphqlArgumentValue::input("RecordNo"),
    )]);
    let endpoint = AiGraphqlToolProfile::read_only(
        "connected-count",
        "endpointSummary",
        "Count currently connected managed endpoints",
        vec![AiGraphqlSelection::scalar("connected")],
        disclosure(
            "endpoint-count-v1",
            "endpointSummary",
            object([("connected".to_owned(), scalar())]),
        ),
        4096,
        1,
    )
    .with_arguments([AiGraphqlArgumentPlan::new(
        "limit",
        AiGraphqlArgumentValue::fixed(1),
    )]);
    let mut builder =
        AiGraphqlToolManifestBuilder::new("records-service", target(), CUSTOM_SDL).unwrap();
    builder.add_custom_profile(record).unwrap();
    builder.add_custom_profile(endpoint).unwrap();
    let manifest = builder.build().unwrap();
    let document = &manifest
        .entries
        .iter()
        .find(|entry| entry.field_name == "record")
        .unwrap()
        .descriptor
        .document;
    assert!(document.contains("$RecordNo: ID!"));
    assert!(document.contains("id: $RecordNo"));
    assert!(!document.contains("privateIdentity"));
    assert!(manifest.entries.iter().any(|entry| {
        entry.field_name == "endpointSummary"
            && !entry.descriptor.document.contains("tenantIdentity")
    }));
}

#[test]
fn generated_profiles_bind_the_current_operation_catalog_and_register() {
    let catalog = generated_surface::graphql_orm_operation_catalog();
    let operation = catalog
        .exposed_operations()
        .find(|operation| {
            operation.category() == GeneratedGraphqlOperationCategory::SingleRead
                && operation.entity_name() == "ProfileTestRecord"
        })
        .unwrap();
    let arguments = operation
        .arguments()
        .iter()
        .map(|argument| format!("{}: {}", argument.graphql_name(), argument.graphql_type()))
        .collect::<Vec<_>>()
        .join(", ");
    let sdl = format!(
        "type Query {{ {}({}): ProfileTestRecord }} type ProfileTestRecord {{ id: String!, label: String!, sensitiveLedger: String! }}",
        operation.field_name(),
        arguments
    );
    let id_argument = operation.arguments().first().unwrap();
    let profile = AiGraphqlToolProfile::read_only(
        "details",
        operation.field_name(),
        "Show a reviewed generated record",
        vec![AiGraphqlSelection::scalar("id")],
        disclosure(
            "generated-details-v1",
            operation.field_name(),
            object([("id".to_owned(), scalar())]),
        ),
        4096,
        1,
    )
    .with_inputs([AiGraphqlProfileInput::string(
        "RecordNo",
        "Public record number",
        true,
        1,
        64,
    )])
    .with_arguments([AiGraphqlArgumentPlan::new(
        id_argument.graphql_name(),
        AiGraphqlArgumentValue::input("RecordNo"),
    )]);
    let mut builder = AiGraphqlToolManifestBuilder::new("records-service", target(), &sdl).unwrap();
    builder
        .add_generated_profile(profile, catalog, &AdmitGenerated)
        .unwrap();
    let manifest = builder.build().unwrap();
    assert!(
        manifest.entries[0]
            .descriptor
            .graphql_contract
            .as_ref()
            .unwrap()
            .generated_operation()
            .is_some()
    );
    let mut tools = AiToolCatalog::new();
    manifest
        .register_into(&mut tools, catalog, &AdmitGenerated)
        .unwrap();
    assert_eq!(tools.descriptors().count(), 1);

    let manifests = AiGraphqlToolManifestSet::aggregate(
        [manifest],
        &BTreeMap::from([("records-service".to_owned(), sdl)]),
    )
    .unwrap();
    let mut federated_tools = AiToolCatalog::new();
    manifests.register_into(&mut federated_tools).unwrap();
    assert_eq!(federated_tools.descriptors().count(), 1);
}

#[test]
fn unknown_unbounded_mismatched_and_unselected_shapes_fail_closed() {
    let mut builder =
        AiGraphqlToolManifestBuilder::new("records-service", target(), CUSTOM_SDL).unwrap();

    let unknown = list_profile().with_arguments([AiGraphqlArgumentPlan::new(
        "unknown",
        AiGraphqlArgumentValue::fixed(1),
    )]);
    assert!(builder.add_custom_profile(unknown).is_err());

    let unbounded = list_profile().with_inputs([AiGraphqlProfileInput::string(
        "Query",
        "Label text to find",
        true,
        0,
        0,
    )]);
    assert!(builder.add_custom_profile(unbounded).is_err());

    let relationship_without_bound = AiGraphqlToolProfile::read_only(
        "unsafe-related",
        "record",
        "Show reviewed related records",
        vec![
            AiGraphqlSelection::object("related", [AiGraphqlSelection::scalar("id")])
                .with_arguments([AiGraphqlArgumentPlan::new(
                    "limit",
                    AiGraphqlArgumentValue::fixed(3),
                )]),
        ],
        disclosure(
            "unsafe-related-v1",
            "record",
            object([(
                "related".to_owned(),
                list(3, object([("id".to_owned(), scalar())])),
            )]),
        ),
        4096,
        3,
    )
    .with_arguments([AiGraphqlArgumentPlan::new(
        "id",
        AiGraphqlArgumentValue::fixed("record-1"),
    )]);
    assert!(
        builder
            .add_custom_profile(relationship_without_bound)
            .is_err()
    );

    let aliases = AiGraphqlToolProfile::read_only(
        "duplicate-aliases",
        "record",
        "Show a reviewed record with invalid aliases",
        vec![
            AiGraphqlSelection::scalar("id").with_alias("value"),
            AiGraphqlSelection::scalar("label").with_alias("value"),
        ],
        disclosure(
            "duplicate-aliases-v1",
            "record",
            object([("value".to_owned(), scalar())]),
        ),
        4096,
        1,
    )
    .with_arguments([AiGraphqlArgumentPlan::new(
        "id",
        AiGraphqlArgumentValue::fixed("record-1"),
    )]);
    assert!(builder.add_custom_profile(aliases).is_err());

    let mismatched = AiGraphqlToolProfile::read_only(
        "mismatch",
        "records",
        "Count visible records from a mismatched view",
        vec![AiGraphqlSelection::object(
            "pageInfo",
            [AiGraphqlSelection::scalar("totalCount")],
        )],
        disclosure(
            "mismatch-v1",
            "records",
            object([("sensitiveLedger".to_owned(), scalar())]),
        ),
        4096,
        1,
    )
    .with_arguments([AiGraphqlArgumentPlan::new(
        "page",
        AiGraphqlArgumentValue::object([("first", AiGraphqlArgumentValue::fixed(1))]),
    )]);
    assert!(builder.add_custom_profile(mismatched).is_err());
}

#[test]
fn manifest_wire_versions_schema_drift_and_duplicate_federated_roots_fail_closed() {
    let mut builder =
        AiGraphqlToolManifestBuilder::new("records-service", target(), CUSTOM_SDL).unwrap();
    builder.add_custom_profile(count_profile()).unwrap();
    let manifest = builder.build().unwrap();
    let round_trip =
        AiGraphqlToolManifest::from_extension_payload(manifest.extension_payload().unwrap())
            .unwrap();
    assert_eq!(round_trip, manifest);
    assert!(
        manifest
            .validate_against_finished_schema(CUSTOM_SDL)
            .is_ok()
    );
    assert!(
        manifest
            .validate_against_finished_schema(&format!("{CUSTOM_SDL}\n# drift"))
            .is_err()
    );

    let mut unsupported = manifest.extension_payload().unwrap();
    unsupported["version"] = serde_json::json!(99);
    assert!(AiGraphqlToolManifest::from_extension_payload(unsupported).is_err());

    let mut second_builder =
        AiGraphqlToolManifestBuilder::new("secondary-service", target(), CUSTOM_SDL).unwrap();
    second_builder.add_custom_profile(count_profile()).unwrap();
    let second = second_builder.build().unwrap();
    let active = BTreeMap::from([
        ("records-service".to_owned(), CUSTOM_SDL.to_owned()),
        ("secondary-service".to_owned(), CUSTOM_SDL.to_owned()),
    ]);
    assert!(AiGraphqlToolManifestSet::aggregate([manifest, second], &active).is_err());
}

#[test]
fn mutations_are_absent_by_default_and_require_supervised_construction() {
    let mutation = AiGraphqlToolProfile::supervised_mutation(
        "rename",
        "renameRecord",
        "Rename one visible record after explicit approval",
        vec![AiGraphqlSelection::scalar("id")],
        disclosure(
            "rename-record-v1",
            "renameRecord",
            object([("id".to_owned(), scalar())]),
        ),
        4096,
        1,
        AiToolRisk::LowRiskWrite,
        true,
    )
    .unwrap()
    .with_inputs([
        AiGraphqlProfileInput::string("RecordNo", "Public record number", true, 1, 64),
        AiGraphqlProfileInput::string("Label", "New visible label", true, 1, 100),
    ])
    .with_arguments([
        AiGraphqlArgumentPlan::new("id", AiGraphqlArgumentValue::input("RecordNo")),
        AiGraphqlArgumentPlan::new("label", AiGraphqlArgumentValue::input("Label")),
    ]);
    let mut forged_query = serde_json::to_value(count_profile()).unwrap();
    forged_query["idempotent"] = serde_json::json!(false);
    let forged_query: AiGraphqlToolProfile = serde_json::from_value(forged_query).unwrap();
    let mut forged_builder =
        AiGraphqlToolManifestBuilder::new("records-service", target(), CUSTOM_SDL).unwrap();
    assert!(forged_builder.add_custom_profile(forged_query).is_err());

    let mut forged_mutation = serde_json::to_value(&mutation).unwrap();
    forged_mutation["approval"] = serde_json::json!("none");
    let forged_mutation: AiGraphqlToolProfile = serde_json::from_value(forged_mutation).unwrap();
    assert!(forged_builder.add_custom_profile(forged_mutation).is_err());

    let mut builder =
        AiGraphqlToolManifestBuilder::new("records-service", target(), CUSTOM_SDL).unwrap();
    builder.add_custom_profile(mutation).unwrap();
    let manifest = builder.build().unwrap();
    let descriptor = &manifest.entries[0].descriptor;
    assert_eq!(descriptor.operation_kind, AiToolOperationKind::Mutation);
    assert_eq!(descriptor.maturity, ToolMaturity::SupervisedWrite);
    assert_eq!(descriptor.approval, AiApprovalRule::OneShot);
}
