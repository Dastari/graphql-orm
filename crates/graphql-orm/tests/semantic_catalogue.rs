#![cfg(any(feature = "sqlite", feature = "postgres"))]

use graphql_orm::async_graphql::{Schema, SimpleObject};
use graphql_orm::prelude::*;
use graphql_orm::sqlx::Row;

fn field_name(camel: &'static str, pascal: &'static str) -> &'static str {
    if cfg!(feature = "field-case-pascal") {
        pascal
    } else {
        camel
    }
}

/// A reviewed public record used to validate semantic emission.
#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
)]
#[graphql_entity(
    table = "semantic_records",
    plural = "SemanticRecords",
    classification = "confidential",
    ai_mutations(create = "automatic", update = "approval_required")
)]
#[graphql(complex)]
struct SemanticRecord {
    /// Stable public record identity.
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    id: String,

    /// Human-readable record label.
    #[graphql_orm(description = "Human-readable record label.")]
    #[filterable(type = "string")]
    #[sortable]
    label: String,

    /// Protected credential material.
    #[graphql_orm(sensitive)]
    credential: String,

    #[graphql(skip)]
    #[graphql_orm(private)]
    implementation_marker: String,

    /// Bounded child records.
    #[graphql(skip)]
    #[relation(target = "SemanticRecord", from = "id", to = "parent_id", multiple)]
    children: Vec<SemanticRecord>,

    /// Optional parent record identity.
    #[filterable(type = "string")]
    parent_id: Option<String>,
}

impl graphql_orm::graphql::loaders::BatchLoadEntity for SemanticRecord {
    fn batch_column() -> &'static str {
        "id"
    }

    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, graphql_orm::sqlx::Error> {
        row.try_get("id")
    }
}

#[derive(Default)]
struct ReviewedQueries;

#[derive(Default)]
struct ReviewedMutations;

#[derive(Default)]
struct ReviewedSubscriptions;

/// Reviewed status result.
#[derive(SimpleObject, GraphQLSemanticObject)]
#[graphql_orm(classification = "internal")]
struct ReviewedStatus {
    /// Bounded public status messages.
    #[graphql_orm(maximum_items = 4)]
    messages: Vec<String>,
    /// Provider credential used internally.
    #[graphql_orm(sensitive)]
    provider_credential: String,
}

#[graphql_orm_custom_operations(kind = "query", authorization = true)]
#[graphql_orm::async_graphql::Object]
impl ReviewedQueries {
    /// Returns a bounded service status value.
    #[graphql(name = "ReviewedStatus")]
    async fn reviewed_status(&self, limit: i32) -> ReviewedStatus {
        ReviewedStatus {
            messages: vec![format!("ok:{limit}")],
            provider_credential: "not executed in schema tests".to_owned(),
        }
    }

    /// Returns a public scalar status.
    #[graphql_orm(result_classification = "public", result_export = "exportable")]
    async fn public_scalar_status(&self) -> String {
        "ok".to_owned()
    }

    /// Returns a bounded restricted scalar list.
    #[graphql_orm(
        result_classification = "restricted",
        result_export = "exportable",
        result_maximum_items = 3
    )]
    async fn restricted_status_codes(&self) -> Vec<i32> {
        vec![1]
    }

    /// Returns a secret that is structurally unavailable to providers.
    #[graphql_orm(result_classification = "secret", result_export = "never_export")]
    async fn secret_scalar_status(&self) -> String {
        "hidden".to_owned()
    }

    /// Returns a non-secret value that remains structurally non-exportable.
    #[graphql_orm(result_classification = "restricted", result_export = "never_export")]
    async fn internal_only_scalar_status(&self) -> String {
        "internal".to_owned()
    }
}

#[graphql_orm_custom_operations(
    kind = "mutation",
    authorization = true,
    ai_execution = "approval_required"
)]
#[graphql_orm::async_graphql::Object]
impl ReviewedMutations {
    /// Applies one reviewed bounded status change.
    #[graphql_orm(result_classification = "internal", result_export = "exportable")]
    async fn apply_reviewed_status(&self, enabled: bool) -> bool {
        enabled
    }

    /// Applies and returns bounded restricted status codes.
    #[graphql_orm(
        result_classification = "restricted",
        result_export = "exportable",
        result_maximum_items = 2
    )]
    async fn apply_restricted_codes(&self, enabled: bool) -> Vec<i32> {
        if enabled { vec![1] } else { Vec::new() }
    }
}

#[graphql_orm_custom_operations(
    kind = "subscription",
    authorization = true,
    observation = "replay_then_live",
    maximum_duration_seconds = 300,
    maximum_events = 16
)]
#[graphql_orm::async_graphql::Subscription]
impl ReviewedSubscriptions {
    /// Observes replayable reviewed status values.
    async fn reviewed_status_changed(
        &self,
    ) -> impl graphql_orm::futures::Stream<Item = ReviewedStatus> {
        graphql_orm::futures::stream::empty()
    }

    /// Observes bounded restricted scalar-list events.
    #[graphql_orm(
        result_classification = "restricted",
        result_export = "exportable",
        result_maximum_items = 2
    )]
    async fn restricted_codes_changed(&self) -> impl graphql_orm::futures::Stream<Item = Vec<i32>> {
        graphql_orm::futures::stream::empty()
    }
}

schema_roots! {
    entities: [SemanticRecord],
    described_query_types: [ReviewedQueries],
    described_mutation_types: [ReviewedMutations],
    described_subscription_types: [ReviewedSubscriptions],
}

#[test]
fn entity_and_custom_root_semantics_are_canonical_and_safe() {
    let metadata = SemanticRecord::graphql_semantic_metadata().expect("entity semantics");
    assert_eq!(
        metadata.description,
        "A reviewed public record used to validate semantic emission."
    );
    assert_eq!(
        metadata.default_classification,
        GraphqlSemanticClassification::Confidential
    );
    assert!(metadata.fields.iter().all(
        |field| field.field_name != field_name("implementationMarker", "ImplementationMarker")
    ));

    let credential = metadata
        .fields
        .iter()
        .find(|field| field.field_name == field_name("credential", "Credential"))
        .expect("sensitive public field remains documented");
    assert_eq!(
        credential.classification,
        GraphqlSemanticClassification::Secret
    );
    assert_eq!(credential.export, GraphqlSemanticExport::NeverExport);

    let children = metadata
        .fields
        .iter()
        .find(|field| field.field_name == field_name("children", "Children"))
        .expect("relationship semantics");
    let relationship = children
        .relationship
        .as_ref()
        .expect("relationship descriptor");
    assert_eq!(
        relationship.cardinality,
        GraphqlSemanticRelationshipCardinality::Many
    );
    assert!(matches!(
        children.type_ref,
        GraphqlSemanticTypeRef::List { maximum_items: Some(limit), .. }
            if limit == PaginationConfig::DEFAULT_MAX_LIMIT as u32
    ));

    let catalog = graphql_orm_semantic_catalog();
    catalog.validate().expect("generated catalogue validates");
    assert_eq!(catalog.fingerprint.len(), 64);
    let custom = catalog
        .operations
        .iter()
        .find(|operation| operation.field_name == "ReviewedStatus")
        .expect("custom root semantics");
    assert_eq!(custom.source, GraphqlSemanticOperationSource::Custom);
    assert_eq!(custom.arguments[0].graphql_name, "limit");
    assert_eq!(custom.arguments[0].description, "Limit");
    assert_eq!(
        custom.description,
        "Returns a bounded service status value."
    );
    assert!(custom.result_disclosure.is_none());
    assert_eq!(custom.generated_entity_name, None);
    let public_scalar = catalog
        .operations
        .iter()
        .find(|operation| operation.field_name == "publicScalarStatus")
        .expect("public scalar semantics");
    assert_eq!(
        public_scalar.result_disclosure,
        Some(GraphqlSemanticResultDisclosure::new(
            GraphqlSemanticClassification::Public,
            GraphqlSemanticExport::Exportable,
        ))
    );
    let restricted_list = catalog
        .operations
        .iter()
        .find(|operation| operation.field_name == "restrictedStatusCodes")
        .expect("restricted list semantics");
    assert_eq!(
        restricted_list.result_disclosure,
        Some(
            GraphqlSemanticResultDisclosure::new(
                GraphqlSemanticClassification::Restricted,
                GraphqlSemanticExport::Exportable,
            )
            .with_maximum_items(3)
        )
    );
    for field_name in ["secretScalarStatus", "internalOnlyScalarStatus"] {
        let operation = catalog
            .operations
            .iter()
            .find(|operation| operation.field_name == field_name)
            .expect("non-exportable scalar semantics");
        assert_eq!(
            operation
                .result_disclosure
                .expect("scalar result disclosure")
                .export,
            GraphqlSemanticExport::NeverExport
        );
    }
    let restricted_subscription = catalog
        .operations
        .iter()
        .find(|operation| operation.field_name == "restrictedCodesChanged")
        .expect("restricted subscription semantics");
    assert_eq!(
        restricted_subscription
            .result_disclosure
            .expect("subscription result disclosure")
            .maximum_items,
        Some(2)
    );
    let status = catalog
        .entities
        .iter()
        .find(|entity| entity.entity_name == "ReviewedStatus")
        .expect("custom result semantics");
    let messages = status
        .fields
        .iter()
        .find(|field| field.field_name == "messages")
        .expect("bounded custom list");
    assert!(matches!(
        messages.type_ref,
        GraphqlSemanticTypeRef::List {
            maximum_items: Some(4),
            ..
        }
    ));
    let provider_credential = status
        .fields
        .iter()
        .find(|field| field.field_name == "providerCredential")
        .expect("sensitive custom field semantics");
    assert_eq!(
        provider_credential.classification,
        GraphqlSemanticClassification::Secret
    );
    assert_eq!(
        provider_credential.export,
        GraphqlSemanticExport::NeverExport
    );

    let subscription = catalog
        .operations
        .iter()
        .find(|operation| operation.field_name == "reviewedStatusChanged")
        .expect("custom subscription semantics");
    let observation = subscription
        .subscription_observation
        .as_ref()
        .expect("subscription observation semantics");
    assert_eq!(
        observation.replay_mode,
        GraphqlSubscriptionReplayMode::ReplayThenLive
    );
    assert_eq!(observation.maximum_duration_seconds, Some(300));
    assert_eq!(observation.maximum_events, Some(16));

    let automatic_create = catalog
        .operations
        .iter()
        .find(|operation| {
            operation.kind == GraphqlOperationKind::Mutation
                && operation.generated_category == Some(GeneratedGraphqlOperationCategory::Create)
        })
        .expect("generated create mutation semantics");
    assert_eq!(
        automatic_create.ai_mutation_execution,
        Some(AiMutationExecutionPolicy::Automatic)
    );
    assert_eq!(
        automatic_create.generated_entity_name.as_deref(),
        Some("SemanticRecord")
    );
    let approval_update = catalog
        .operations
        .iter()
        .find(|operation| {
            operation.kind == GraphqlOperationKind::Mutation
                && operation.generated_category == Some(GeneratedGraphqlOperationCategory::Update)
        })
        .expect("generated update mutation semantics");
    assert_eq!(
        approval_update.ai_mutation_execution,
        Some(AiMutationExecutionPolicy::ApprovalRequired)
    );
    let prohibited_delete = catalog
        .operations
        .iter()
        .find(|operation| {
            operation.kind == GraphqlOperationKind::Mutation
                && operation.generated_category == Some(GeneratedGraphqlOperationCategory::Delete)
        })
        .expect("generated delete mutation semantics");
    assert_eq!(
        prohibited_delete.ai_mutation_execution,
        Some(AiMutationExecutionPolicy::Prohibited)
    );
    let custom_mutation = catalog
        .operations
        .iter()
        .find(|operation| operation.field_name == "applyReviewedStatus")
        .expect("custom mutation semantics");
    assert_eq!(
        custom_mutation.ai_mutation_execution,
        Some(AiMutationExecutionPolicy::ApprovalRequired)
    );
    let restricted_mutation = catalog
        .operations
        .iter()
        .find(|operation| operation.field_name == "applyRestrictedCodes")
        .expect("restricted mutation semantics");
    assert_eq!(
        restricted_mutation
            .result_disclosure
            .expect("mutation result disclosure")
            .maximum_items,
        Some(2)
    );

    let payload = catalog.extension_payload().expect("extension payload");
    let decoded = GraphqlSemanticCatalog::from_extension_payload(payload).expect("payload decodes");
    assert_eq!(decoded.fingerprint, catalog.fingerprint);
    let encoded = serde_json::to_string(&decoded).expect("semantic catalogue serializes");
    assert!(!encoded.contains("semantic_records"));
    assert!(!encoded.contains("implementation_marker"));
}

#[test]
fn semantic_descriptions_match_finished_schema_documentation() {
    let schema = Schema::build(
        QueryRoot::default(),
        MutationRoot::default(),
        SubscriptionRoot::default(),
    )
    .finish();
    let sdl = schema.sdl();
    assert!(sdl.contains("A reviewed public record used to validate semantic emission."));
    assert!(sdl.contains("Human-readable record label"));
    assert!(sdl.contains("Returns a bounded service status value."));
    assert!(sdl.contains("Observes replayable reviewed status values."));
    assert!(sdl.contains("\t\tLimit\n\t\t\"\"\"\n\t\tlimit: Int!"));
    assert!(!sdl.contains("implementationMarker"));
    assert!(sdl.contains("List SemanticRecord records"));
}

#[test]
fn extra_or_stale_payload_metadata_fails_closed() {
    let catalog = graphql_orm_semantic_catalog();
    let mut payload = catalog.extension_payload().expect("extension payload");
    payload
        .as_object_mut()
        .expect("catalog payload object")
        .insert("unknown".to_owned(), serde_json::json!(true));
    assert_eq!(
        GraphqlSemanticCatalog::from_extension_payload(payload)
            .expect_err("unknown wire fields must fail")
            .message(),
        "semantic catalogue payload is invalid"
    );

    let mut stale = catalog.extension_payload().expect("extension payload");
    stale["entities"][0]["description"] = serde_json::json!("Changed description");
    assert_eq!(
        GraphqlSemanticCatalog::from_extension_payload(stale)
            .expect_err("fingerprint drift must fail")
            .message(),
        "semantic catalogue fingerprint is stale"
    );

    let mut missing_evidence = catalog.extension_payload().expect("extension payload");
    missing_evidence["fingerprint"] = serde_json::json!("");
    assert_eq!(
        GraphqlSemanticCatalog::from_extension_payload(missing_evidence)
            .expect_err("empty fingerprint evidence must fail")
            .message(),
        "semantic catalogue fingerprint is stale"
    );

    let mut changed_result = catalog.extension_payload().expect("extension payload");
    let operations = changed_result["operations"]
        .as_array_mut()
        .expect("operation array");
    let public_scalar = operations
        .iter_mut()
        .find(|operation| operation["field_name"] == "publicScalarStatus")
        .expect("public scalar operation");
    public_scalar["result_disclosure"]["classification"] = serde_json::json!("restricted");
    assert_eq!(
        GraphqlSemanticCatalog::from_extension_payload(changed_result)
            .expect_err("result disclosure drift must fail")
            .message(),
        "semantic operation fingerprint is stale"
    );
}

#[cfg(feature = "router-protocol")]
#[test]
fn semantic_catalogue_survives_complete_router_descriptor_transport() {
    use graphql_orm::graphql::orm::GRAPHQL_SEMANTIC_CATALOG_EXTENSION_NAME;
    use graphql_orm_router_protocol::{
        AuthorizationRequirement, Fingerprint, OperationDescriptor, RootOperationType,
        SubgraphDescriptor, SubgraphDescriptorBuilder,
    };

    let catalog = graphql_orm_semantic_catalog();
    let extension = catalog
        .router_protocol_extension()
        .expect("semantic extension validates");
    let descriptor = SubgraphDescriptorBuilder::new(
        "reviewed_service",
        "ReviewedService",
        "http://reviewed.internal/graphql",
        "http://reviewed.internal/.well-known/sdl",
        Fingerprint::sha256("reviewed-schema"),
    )
    .expect("subgraph identity validates")
    .operation(OperationDescriptor {
        root_type: RootOperationType::Query,
        field_name: "ReviewedStatus".to_owned(),
        arguments: Vec::new(),
        authorization: AuthorizationRequirement::Authenticated,
    })
    .extension(extension)
    .build()
    .expect("descriptor validates");
    let encoded = serde_json::to_string(&descriptor).expect("descriptor serializes");
    let decoded =
        SubgraphDescriptor::from_json_compatible(&encoded).expect("descriptor transport validates");
    let payload = decoded
        .extensions
        .iter()
        .find(|extension| extension.name == GRAPHQL_SEMANTIC_CATALOG_EXTENSION_NAME)
        .expect("semantic extension survives")
        .payload
        .clone();
    let transported = GraphqlSemanticCatalog::from_extension_payload(payload)
        .expect("transported semantics validate");
    assert_eq!(transported.fingerprint, catalog.fingerprint);
}
