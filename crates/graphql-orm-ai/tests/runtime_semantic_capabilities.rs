//! Runtime-composed GraphQL collections reach AI capability indexes through the
//! runtime semantic catalogue, with the same descriptions, classification and
//! export as their SDL, and no separate input.
#![cfg(feature = "sqlite")]
use graphql_orm::graphql::orm::*;
use graphql_orm::graphql::runtime::*;
use graphql_orm::prelude::*;
use graphql_orm_ai::*;
use std::sync::Arc;

fn field(id: &str, name: &str, kind: RuntimeValueKind) -> RuntimeField {
    RuntimeField {
        id: FieldId::new(id).unwrap(),
        api_name: name.into(),
        physical_column: name.to_ascii_lowercase(),
        value_kind: kind,
        nullable: false,
        unique: false,
        filterable: true,
        sortable: true,
        generated: false,
        default: None,
        format: None,
        description: None,
        classification: None,
        export: None,
        auto_timestamp: None,
        auto_actor: None,
    }
}

fn collection(id: &str, name: &str, fields: Vec<RuntimeField>) -> RuntimeCollection {
    RuntimeCollection {
        id: CollectionId::new(id).unwrap(),
        api_type_name: name.into(),
        api_plural_name: format!("{name}s"),
        physical_table: id.into(),
        primary_key: vec![fields[0].id.clone()],
        append_only: false,
        retention_purge: false,
        fields,
        relations: vec![],
        indexes: vec![],
        composite_unique: vec![],
        default_order: vec![],
        description: None,
        classification: None,
        label_fields: Vec::new(),
    }
}

fn relation(
    id: &str,
    name: &str,
    target: &str,
    source: &str,
    to: &str,
    many: bool,
) -> RuntimeRelation {
    RuntimeRelation {
        id: RelationId::new(id).unwrap(),
        api_name: name.into(),
        target: CollectionId::new(target).unwrap(),
        key_pairs: vec![RelationKeyPair {
            source: FieldId::new(source).unwrap(),
            target: FieldId::new(to).unwrap(),
        }],
        cardinality: if many {
            RelationCardinality::Many
        } else {
            RelationCardinality::One
        },
        enforce_foreign_key: !many,
        on_delete: (!many).then_some(DeletePolicy::Restrict),
        description: None,
        classification: None,
        export: None,
        join: None,
    }
}

fn schema() -> Arc<ValidatedRuntimeSchema> {
    let mut invoices = collection(
        "invoices",
        "Invoice",
        vec![
            field("invoices.id", "id", RuntimeValueKind::Integer),
            field("invoices.number", "number", RuntimeValueKind::String),
            field("invoices.note", "internalNote", RuntimeValueKind::String),
        ],
    );
    invoices.description = Some("Customer invoice".into());
    invoices.classification = Some(GraphqlSemanticClassification::Confidential);
    invoices.fields[1].description = Some("Invoice number shown to customers".into());
    invoices.fields[2].classification = Some(GraphqlSemanticClassification::Secret);
    invoices.relations.push(relation(
        "invoices.lines",
        "lines",
        "lines",
        "invoices.id",
        "lines.invoice",
        true,
    ));
    let mut lines = collection(
        "lines",
        "InvoiceLine",
        vec![
            field("lines.id", "id", RuntimeValueKind::Integer),
            field("lines.invoice", "invoice", RuntimeValueKind::Integer),
            field("lines.label", "label", RuntimeValueKind::String),
        ],
    );
    lines.relations.push(relation(
        "lines.owner",
        "owner",
        "invoices",
        "lines.invoice",
        "invoices.id",
        false,
    ));
    lines.relations[0].description = Some("Invoice owning the line".into());
    Arc::new(
        RuntimeSchema {
            format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
            collections: vec![invoices, lines],
        }
        .validate()
        .unwrap(),
    )
}

#[tokio::test]
async fn runtime_collections_reach_the_capability_index() {
    let schema = schema();
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    let module = RuntimeGraphqlModule::compile(
        schema.clone(),
        RuntimeGraphqlOptions::default().with_cursor_profile(RuntimeCursorProfile::Unprotected),
    )
    .unwrap();
    let (api, catalog) = RuntimeGraphqlComposer::new(database, "Query", Default::default())
        .unwrap()
        .install(module)
        .unwrap()
        .finish_with_semantic_catalog()
        .unwrap();

    // The runtime SDL and its catalogue compile as a query capability catalogue:
    // runtime lists keep their `first`/`after` connection arguments.
    let target = GraphqlExecutionTargetId::parse("runtime-application").unwrap();
    let queries = AiGraphqlQueryCapabilityCatalog::compile(
        "runtime-read",
        target.clone(),
        &api.sdl(),
        &catalog,
        AiGraphqlQueryCapabilityLimits::default(),
    )
    .unwrap();
    let invoice_query = queries
        .capabilities()
        .find(|capability| capability.field_name() == "invoices")
        .unwrap();
    let arguments = serde_json::json!({"where": {"number": {"eq": "INV-1", "notIn": ["INV-hidden"]}},
        "orderBy": [{"field": "number", "direction": "ASC", "nulls": "LAST"}]});
    let compact = invoice_query
        .compile_compact(serde_json::json!({
            "arguments": arguments, "selections": ["id", "number"], "maximumItems": 1,
            "relationshipArguments": {}, "relationshipMaximumItems": {}
        }))
        .expect("real runtime filtered and ordered compact plan");
    let nested = invoice_query.compile(serde_json::json!({
        "arguments": arguments, "fields": {"id": true, "number": true}, "relationships": {}, "maximumItems": 1
    })).expect("real runtime filtered and ordered nested plan");
    assert_eq!(
        compact.variables(),
        &serde_json::json!({"v0": 1,
        "v1": [{"field": "number", "direction": "ASC", "nulls": "LAST"}],
        "v2": {"number": {"eq": "INV-1", "notIn": ["INV-hidden"]}}})
    );
    assert_eq!(compact.variables(), nested.variables());
    for plan in [&compact, &nested] {
        assert_eq!(
            plan.descriptor().document,
            format!(
                "query AiQuery_{}($v0: Int, $v1: [InvoiceOrderInput!], $v2: InvoiceWhereInput) {{ invoices(first: $v0, orderBy: $v1, where: $v2) {{ edges {{ node {{ id number }} }} }} }}",
                &plan.plan_fingerprint()[..20],
            )
        );
    }
    let index = AiCapabilityIndex::compile(
        target,
        queries.finished_schema_fingerprint(),
        &catalog,
        Some(&queries),
        None,
        None,
        [],
        "runtime-policy-v1",
        AiCapabilityIndexLimits::default(),
    )
    .unwrap();
    let entry = |entity: &str| {
        index
            .entries()
            .find(|entry| entry.entity_name.as_deref() == Some(entity))
            .expect("runtime collection is indexed")
    };
    let invoice = entry("Invoice");
    assert_eq!(invoice.operation_name, "invoices");
    assert_eq!(invoice.description, "Page through Invoice records");
    assert_eq!(
        invoice.result_classification,
        DataClassification::Confidential
    );
    let fields = invoice
        .scalar_fields
        .iter()
        .map(|field| {
            (
                field.name.as_str(),
                field.description.as_str(),
                field.filterable,
            )
        })
        .collect::<Vec<_>>();
    // The secret field is never-export and therefore not model-facing.
    assert_eq!(
        fields,
        [
            ("id", "Id", true),
            ("number", "Invoice number shown to customers", true)
        ]
    );
    // To-many relations are connection-typed and are not advertised.
    assert!(invoice.relationships.is_empty());
    let line = entry("InvoiceLine");
    assert_eq!(line.result_classification, DataClassification::Internal);
    assert_eq!(
        line.relationships
            .iter()
            .map(|r| (
                r.name.as_str(),
                r.description.as_str(),
                r.target_entity.as_str(),
                r.to_many
            ))
            .collect::<Vec<_>>(),
        [("owner", "Invoice owning the line", "Invoice", false)]
    );
    let entity = catalog
        .entities
        .iter()
        .find(|entity| entity.entity_name == "Invoice")
        .unwrap();
    assert_eq!(entity.description, "Customer invoice");
}
