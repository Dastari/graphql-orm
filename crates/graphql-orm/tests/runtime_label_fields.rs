//! Runtime collection label fields: validation, serde, identity, physical
//! neutrality and the semantic catalogue.
use graphql_orm::graphql::orm::*;
#[cfg(all(feature = "runtime-graphql", feature = "sqlite"))]
use graphql_orm::prelude::Database;

fn fid(value: &str) -> FieldId {
    FieldId::new(value).unwrap()
}

fn field(name: &str, kind: RuntimeValueKind) -> RuntimeField {
    RuntimeField {
        id: fid(&format!("f_{name}")),
        api_name: name.into(),
        physical_column: name.into(),
        value_kind: kind,
        nullable: true,
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

fn schema(labels: &[&str]) -> RuntimeSchema {
    let mut id = field("id", RuntimeValueKind::Uuid);
    id.nullable = false;
    let mut secret = field("token", RuntimeValueKind::String);
    secret.classification = Some(GraphqlSemanticClassification::Secret);
    let mut hidden = field("internal", RuntimeValueKind::String);
    hidden.export = Some(GraphqlSemanticExport::NeverExport);
    RuntimeSchema {
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![RuntimeCollection {
            id: CollectionId::new("books").unwrap(),
            api_type_name: "Book".into(),
            api_plural_name: "Books".into(),
            physical_table: "label_books".into(),
            primary_key: vec![fid("f_id")],
            append_only: false,
            retention_purge: false,
            fields: vec![
                id,
                field("title", RuntimeValueKind::String),
                field("code", RuntimeValueKind::Integer),
                field("published", RuntimeValueKind::DateTime),
                field("rating", RuntimeValueKind::Float),
                field("meta", RuntimeValueKind::Json),
                field("cover", RuntimeValueKind::Bytes),
                secret,
                hidden,
            ],
            relations: vec![],
            indexes: vec![],
            composite_unique: vec![],
            default_order: vec![],
            description: None,
            classification: None,
            label_fields: labels
                .iter()
                .map(|name| fid(&format!("f_{name}")))
                .collect(),
            search: None,
        }],
    }
}

fn codes(labels: &[&str]) -> Vec<RuntimeSchemaDiagnosticCode> {
    schema(labels)
        .validate()
        .unwrap_err()
        .into_diagnostics()
        .into_iter()
        .map(|d| d.code)
        .collect()
}

#[test]
fn label_fields_must_be_distinct_bounded_exportable_scalars_of_the_collection() {
    use RuntimeSchemaDiagnosticCode as C;
    schema(&[]).validate().unwrap();
    schema(&["title"]).validate().unwrap();
    schema(&["title", "code", "published", "rating"])
        .validate()
        .unwrap();
    assert_eq!(codes(&["missing"]), [C::UnknownFieldReference]);
    assert_eq!(codes(&["title", "title"]), [C::InvalidLabelField]);
    for ineligible in ["meta", "cover", "token", "internal"] {
        assert_eq!(codes(&[ineligible]), [C::InvalidLabelField], "{ineligible}");
    }
    assert_eq!(
        codes(&["id", "title", "code", "published", "rating"]),
        [C::InvalidLabelField]
    );
    assert_eq!(RUNTIME_MAX_LABEL_FIELDS, 4);
    assert_eq!(C::InvalidLabelField.as_str(), "invalid_label_field");

    // Removing a referenced field fails validation instead of silently clearing it.
    let mut removed = schema(&["title"]);
    removed.collections[0]
        .fields
        .retain(|f| f.api_name != "title");
    let diagnostics = removed.validate().unwrap_err().into_diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, C::UnknownFieldReference);
    assert_eq!(diagnostics[0].subject.as_deref(), Some("f_title"));
}

#[test]
fn serde_defaults_absent_labels_and_omits_empty_ones() {
    let plain = serde_json::to_value(schema(&[])).unwrap();
    assert!(plain["collections"][0].get("label_fields").is_none());
    let restored: RuntimeSchema = serde_json::from_value(plain).unwrap();
    assert!(restored.collections[0].label_fields.is_empty());

    let labelled = serde_json::to_value(schema(&["title", "code"])).unwrap();
    assert_eq!(
        labelled["collections"][0]["label_fields"],
        serde_json::json!(["f_title", "f_code"])
    );
    let restored: RuntimeSchema = serde_json::from_value(labelled).unwrap();
    assert_eq!(restored, schema(&["title", "code"]));
}

#[test]
fn labels_are_ordered_semantic_identity_but_neither_structure_nor_physical_schema() {
    let plain = schema(&[]).validate().unwrap();
    let title = schema(&["title"]).validate().unwrap();
    let both = schema(&["title", "code"]).validate().unwrap();
    let reversed = schema(&["code", "title"]).validate().unwrap();

    let bytes = |s: &ValidatedRuntimeSchema| String::from_utf8(s.canonical_bytes()).unwrap();
    assert!(!bytes(&plain).contains("|label="));
    assert!(bytes(&both).contains("|label=[\"f_title\",\"f_code\"]\n"));
    let fingerprints = [&plain, &title, &both, &reversed].map(|s| s.fingerprint());
    for (i, left) in fingerprints.iter().enumerate() {
        for right in &fingerprints[i + 1..] {
            assert_ne!(left, right);
        }
    }
    for labelled in [&title, &both, &reversed] {
        assert_eq!(
            labelled.structural_fingerprint(),
            plain.structural_fingerprint()
        );
        assert_eq!(labelled.format_fingerprint(), plain.format_fingerprint());
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    {
        #[cfg(feature = "sqlite")]
        type Backend = SqliteBackend;
        #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
        type Backend = PostgresBackend;
        let target = |schema: &ValidatedRuntimeSchema| {
            let model = schema
                .physical_schema::<Backend>(RuntimeMigrationLimits::default())
                .unwrap();
            (model.stable_hash(), format!("{model:?}"))
        };
        assert_eq!(target(&plain), target(&both));
        assert_eq!(target(&plain), target(&reversed));
    }
}

#[test]
fn label_ids_containing_commas_cannot_collide() {
    let mut schema = schema(&[]);
    for id in ["a", "b", "a,b"] {
        let mut extra = field(id, RuntimeValueKind::String);
        extra.id = fid(id);
        extra.physical_column = format!("c{}", id.replace(',', "_"));
        extra.api_name = format!("f{}", id.replace(',', "_"));
        schema.collections[0].fields.push(extra);
    }
    let labelled = |labels: &[&str]| {
        let mut schema = schema.clone();
        schema.collections[0].label_fields = labels.iter().map(|id| fid(id)).collect();
        schema.validate().unwrap()
    };
    let pair = labelled(&["a", "b"]);
    let joined = labelled(&["a,b"]);
    assert_ne!(pair.canonical_bytes(), joined.canonical_bytes());
    assert_ne!(pair.fingerprint(), joined.fingerprint());
    assert!(
        String::from_utf8(joined.canonical_bytes())
            .unwrap()
            .contains("|label=[\"a,b\"]")
    );
}

#[test]
fn semantic_catalogue_validates_label_fields() {
    let entity = |labels: &[&str]| GraphqlEntitySemanticMetadata {
        entity_name: "Book".into(),
        description: "Book".into(),
        default_classification: GraphqlSemanticClassification::Internal,
        fields: ["title", "secret", "author", "meta"]
            .map(|name| GraphqlSemanticFieldMetadata {
                field_name: name.into(),
                description: name.into(),
                type_ref: GraphqlSemanticTypeRef::named(
                    match name {
                        "author" => "Author",
                        "meta" => "Meta",
                        _ => "String",
                    },
                    if matches!(name, "author" | "meta") {
                        GraphqlSemanticTypeKind::Object
                    } else {
                        GraphqlSemanticTypeKind::Scalar
                    },
                    true,
                ),
                selectable: true,
                filter_operators: Vec::new(),
                filter_input: None,
                sortable: false,
                groupable: false,
                aggregate_operators: Vec::new(),
                aggregate_value_kind: None,
                relationship: (name == "author").then(|| GraphqlSemanticRelationshipDescriptor {
                    target: "Author".into(),
                    cardinality: GraphqlSemanticRelationshipCardinality::One,
                    arguments: Vec::new(),
                    collection_bound: None,
                }),
                classification: if name == "secret" {
                    GraphqlSemanticClassification::Secret
                } else {
                    GraphqlSemanticClassification::Internal
                },
                export: if name == "secret" {
                    GraphqlSemanticExport::NeverExport
                } else {
                    GraphqlSemanticExport::Exportable
                },
                has_field_policy: false,
            })
            .into_iter()
            .collect(),
        label_fields: labels.iter().map(|name| name.to_string()).collect(),
        record_version: None,
    };
    let author = GraphqlEntitySemanticMetadata {
        entity_name: "Author".into(),
        description: "Author".into(),
        default_classification: GraphqlSemanticClassification::Internal,
        fields: Box::new([]),
        label_fields: Vec::new(),
        record_version: None,
    };
    let compose = |labels: &[&str]| {
        GraphqlSemanticCatalog::compose_descriptors([entity(labels), author.clone()], [])
    };
    let plain = compose(&[]).unwrap();
    let labelled = compose(&["title"]).unwrap();
    assert_ne!(plain.fingerprint, labelled.fingerprint);
    // Absent labels keep earlier wire bytes.
    let wire = serde_json::to_value(&plain).unwrap();
    assert!(wire["entities"][1].get("label_fields").is_none());
    let wire = serde_json::to_value(&labelled).unwrap();
    assert_eq!(
        wire["entities"][1]["label_fields"],
        serde_json::json!(["title"])
    );
    let decoded: GraphqlSemanticCatalog = serde_json::from_value(wire).unwrap();
    decoded.validate().unwrap();
    for invalid in [
        &["missing"][..],
        &["secret"],
        &["author"],
        &["meta"],
        &["title", "title"],
        &["title", "title", "title", "title", "title"],
    ] {
        assert!(compose(invalid).is_err(), "{invalid:?}");
    }
}

#[cfg(all(feature = "runtime-graphql", feature = "sqlite"))]
mod graphql {
    use super::*;
    use graphql_orm::graphql::runtime::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn runtime_semantic_entities_publish_label_fields_by_api_name() {
        let schema = Arc::new(schema(&["title", "code"]).validate().unwrap());
        let module = RuntimeGraphqlModule::compile(
            schema.clone(),
            RuntimeGraphqlOptions::default().with_cursor_profile(RuntimeCursorProfile::Unprotected),
        )
        .unwrap();
        let entities = module.semantic_entities();
        assert_eq!(entities[0].label_fields, ["title", "code"]);
        let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
            .await
            .unwrap();
        let (_, catalog) = RuntimeGraphqlComposer::new(db, "Query", Default::default())
            .unwrap()
            .install(module)
            .unwrap()
            .finish_with_semantic_catalog()
            .unwrap();
        let book = catalog
            .entities
            .iter()
            .find(|e| e.entity_name == "Book")
            .unwrap();
        assert_eq!(book.label_fields, ["title", "code"]);

        let plain = RuntimeGraphqlModule::compile(
            Arc::new(super::schema(&[]).validate().unwrap()),
            RuntimeGraphqlOptions::default().with_cursor_profile(RuntimeCursorProfile::Unprotected),
        )
        .unwrap();
        assert!(plain.semantic_entities()[0].label_fields.is_empty());
    }

    struct Upper(RuntimeProjectionContract);
    impl RuntimeFieldProjector for Upper {
        fn contract(&self) -> &RuntimeProjectionContract {
            &self.0
        }
        fn resolve<'a>(
            &'a self,
            _: RuntimeProjectionBatch<'a>,
        ) -> futures::future::BoxFuture<'a, Result<RuntimeProjectionResults, RuntimeProjectionError>>
        {
            Box::pin(async { Err(RuntimeProjectionError::Unavailable) })
        }
    }

    #[test]
    fn labels_with_a_projector_served_member_are_not_published() {
        let namespace = RuntimeFormatNamespace::new("example.name").unwrap();
        let mut definition = schema(&["title", "code"]);
        definition.collections[0].fields[1].format = Some(
            RuntimeFieldFormat::new(namespace.clone(), 1, RuntimeFormatPayload::empty()).unwrap(),
        );
        let projector = Arc::new(Upper(RuntimeProjectionContract {
            namespace,
            versions: 1..=1,
            value_kinds: std::collections::BTreeSet::from([RuntimeValueKind::String]),
            object: RuntimeProjectedObject::new(
                "NameView",
                vec![RuntimeProjectedField::new(
                    "text",
                    RuntimeValueKind::String,
                    false,
                )],
            )
            .unwrap(),
            may_be_absent: false,
        }));
        let module = RuntimeGraphqlModule::compile(
            Arc::new(definition.validate().unwrap()),
            RuntimeGraphqlOptions::default()
                .with_cursor_profile(RuntimeCursorProfile::Unprotected)
                .project_field(fid("f_title"), projector),
        )
        .unwrap();
        // Publishing only `code` would silently change the composite label's meaning.
        assert!(module.semantic_entities()[0].label_fields.is_empty());
    }
}
