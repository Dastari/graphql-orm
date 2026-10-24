//! Runtime descriptions, classification and export: IR validation and
//! identity, SDL and introspection, and the runtime semantic catalogue.
use graphql_orm::graphql::orm::*;

fn cid(value: &str) -> CollectionId {
    CollectionId::new(value).unwrap()
}
fn fid(value: &str) -> FieldId {
    FieldId::new(value).unwrap()
}

fn field(id: &str, name: &str, kind: RuntimeValueKind) -> RuntimeField {
    RuntimeField {
        id: fid(id),
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
    }
}

fn collection(id: &str, name: &str, plural: &str, fields: Vec<RuntimeField>) -> RuntimeCollection {
    RuntimeCollection {
        id: cid(id),
        api_type_name: name.into(),
        api_plural_name: plural.into(),
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
    target_field: &str,
    cardinality: RelationCardinality,
) -> RuntimeRelation {
    let one = cardinality == RelationCardinality::One;
    RuntimeRelation {
        id: RelationId::new(id).unwrap(),
        api_name: name.into(),
        target: cid(target),
        key_pairs: vec![RelationKeyPair {
            source: fid(source),
            target: fid(target_field),
        }],
        cardinality,
        enforce_foreign_key: one,
        on_delete: one.then_some(DeletePolicy::Restrict),
        description: None,
        classification: None,
        export: None,
        join: None,
    }
}

/// Invoices and their lines; `described` adds every new member.
fn schema(described: bool) -> RuntimeSchema {
    let mut invoices = collection(
        "invoices",
        "Invoice",
        "Invoices",
        vec![
            field("invoices.id", "id", RuntimeValueKind::Integer),
            field("invoices.number", "number", RuntimeValueKind::String),
            field("invoices.total", "total", RuntimeValueKind::Float),
            field("invoices.note", "internalNote", RuntimeValueKind::String),
        ],
    );
    invoices.relations.push(relation(
        "invoices.lines",
        "lines",
        "invoice_lines",
        "invoices.id",
        "invoice_lines.invoice",
        RelationCardinality::Many,
    ));
    let mut lines = collection(
        "invoice_lines",
        "InvoiceLine",
        "InvoiceLines",
        vec![
            field("invoice_lines.id", "id", RuntimeValueKind::Integer),
            field(
                "invoice_lines.invoice",
                "invoice",
                RuntimeValueKind::Integer,
            ),
            field("invoice_lines.label", "label", RuntimeValueKind::String),
        ],
    );
    lines.relations.push(relation(
        "invoice_lines.owner",
        "owner",
        "invoices",
        "invoice_lines.invoice",
        "invoices.id",
        RelationCardinality::One,
    ));
    if described {
        invoices.description = Some("Customer invoice".into());
        invoices.classification = Some(GraphqlSemanticClassification::Confidential);
        invoices.fields[1].description = Some("Invoice number shown to customers".into());
        invoices.fields[2].classification = Some(GraphqlSemanticClassification::Restricted);
        invoices.fields[2].export = Some(GraphqlSemanticExport::NeverExport);
        invoices.fields[3].classification = Some(GraphqlSemanticClassification::Secret);
        invoices.relations[0].description = Some("Lines of the invoice".into());
        lines.description = Some("One invoice line".into());
        lines.relations[0].description = Some("Invoice owning the line".into());
    }
    RuntimeSchema {
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![invoices, lines],
    }
}

fn codes(schema: RuntimeSchema) -> Vec<(RuntimeSchemaDiagnosticCode, Option<String>, String)> {
    let Err(diagnostics) = schema.validate() else {
        return Vec::new();
    };
    diagnostics
        .into_diagnostics()
        .into_iter()
        .map(|d| (d.code, d.subject, d.message))
        .collect()
}

#[test]
fn absent_members_are_omitted_and_present_members_are_semantic_identity() {
    let plain = schema(false);
    let json = serde_json::to_string(&plain).unwrap();
    for member in ["description", "classification", "export"] {
        assert!(!json.contains(member), "absent `{member}` is serialized");
    }
    let plain = plain.validate().unwrap();
    let described = schema(true);
    let reloaded: RuntimeSchema =
        serde_json::from_str(&serde_json::to_string(&described).unwrap()).unwrap();
    assert_eq!(reloaded, described);
    let described = described.validate().unwrap();

    let canonical = String::from_utf8(described.canonical_bytes()).unwrap();
    assert!(canonical.contains(
        "|type=Invoice|plural=Invoices|table=invoices|append_only=false\
         |description=\"Customer invoice\"|classification=confidential\n"
    ));
    assert!(canonical.contains("|classification=restricted|export=never_export\n"));
    assert!(canonical.contains("|description=\"Lines of the invoice\"\n"));
    assert!(
        !String::from_utf8(plain.canonical_bytes())
            .unwrap()
            .contains("|description=")
    );
    assert_ne!(plain.fingerprint(), described.fingerprint());
    assert_eq!(
        plain.structural_fingerprint(),
        described.structural_fingerprint()
    );
    assert_eq!(plain.format_fingerprint(), described.format_fingerprint());
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    {
        #[cfg(feature = "sqlite")]
        type Backend = SqliteBackend;
        #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
        type Backend = PostgresBackend;
        let target = |schema: &ValidatedRuntimeSchema| {
            schema
                .physical_schema::<Backend>(RuntimeMigrationLimits::default())
                .unwrap()
                .stable_hash()
        };
        assert_eq!(target(&plain), target(&described));
    }

    // Typos in the new members still fail closed.
    let mut value = serde_json::to_value(schema(true)).unwrap();
    value["collections"][0]["descriptions"] = serde_json::json!("typo");
    assert!(serde_json::from_value::<RuntimeSchema>(value).is_err());
    let mut value = serde_json::to_value(schema(true)).unwrap();
    value["collections"][0]["classification"] = serde_json::json!("top_secret");
    assert!(serde_json::from_value::<RuntimeSchema>(value).is_err());
}

#[test]
fn descriptions_use_the_static_rule_and_never_echo_text() {
    let longest = "x".repeat(1024);
    let mut accepted = schema(true);
    accepted.collections[0].description = Some(longest.clone());
    accepted.collections[0].fields[0].description = Some("Ünïcode description".into());
    accepted.validate().unwrap();

    for bad in [
        String::new(),
        "   ".into(),
        " padded".into(),
        "trailing ".into(),
        "bell\u{7}".into(),
        "line\nbreak".into(),
        "right\u{202e}left".into(),
        "zero\u{200b}width".into(),
        "isolate\u{2066}x".into(),
        "bom\u{feff}".into(),
        format!("{longest}x"),
    ] {
        let mut schema = schema(true);
        schema.collections[0].description = Some(bad.clone());
        schema.collections[0].fields[2].description = Some(bad.clone());
        schema.collections[1].relations[0].description = Some(bad.clone());
        let diagnostics = codes(schema);
        let subjects = diagnostics
            .iter()
            .filter(|(code, ..)| *code == RuntimeSchemaDiagnosticCode::InvalidDescription)
            .map(|(_, subject, _)| subject.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            subjects,
            [
                None,
                Some("invoices.total".into()),
                Some("invoice_lines.owner".into())
            ],
            "{bad:?}"
        );
        if !bad.trim().is_empty() {
            assert!(
                diagnostics
                    .iter()
                    .all(|(_, _, message)| !message.contains(bad.trim()))
            );
        }
    }
}

#[test]
fn classification_inherits_and_only_strengthens() {
    let validated = schema(true).validate().unwrap();
    let invoices = &validated.schema().collections[0];
    let [id, number, total, note] = &invoices.fields[..] else {
        unreachable!()
    };
    use GraphqlSemanticClassification::*;
    use GraphqlSemanticExport::*;
    assert_eq!(invoices.effective_classification(), Confidential);
    assert_eq!(id.effective_classification(invoices), Confidential);
    assert_eq!(id.effective_export(invoices), Exportable);
    assert_eq!(id.effective_description(), "Id");
    assert_eq!(
        number.effective_description(),
        "Invoice number shown to customers"
    );
    assert_eq!(note.effective_description(), "Internal Note");
    assert_eq!(total.effective_classification(invoices), Restricted);
    assert_eq!(total.effective_export(invoices), NeverExport);
    assert_eq!(note.effective_classification(invoices), Secret);
    assert_eq!(note.effective_export(invoices), NeverExport);
    let lines = &validated.schema().collections[1];
    assert_eq!(lines.effective_classification(), Internal);
    assert_eq!(lines.relations[0].effective_classification(lines), Internal);
    assert_eq!(
        validated.schema().collections[0].relations[0].effective_classification(invoices),
        Confidential
    );

    let weaker = |mutate: fn(&mut RuntimeSchema)| {
        let mut schema = schema(true);
        mutate(&mut schema);
        codes(schema)
            .into_iter()
            .filter(|(code, ..)| *code == RuntimeSchemaDiagnosticCode::InvalidClassification)
            .map(|(_, subject, _)| subject)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        weaker(|s| s.collections[0].fields[0].classification = Some(Public)),
        [Some("invoices.id".to_string())]
    );
    assert_eq!(
        weaker(|s| s.collections[0].relations[0].classification = Some(Internal)),
        [Some("invoices.lines".to_string())]
    );
    assert_eq!(
        weaker(|s| s.collections[0].fields[3].export = Some(Exportable)),
        [Some("invoices.note".to_string())]
    );
    assert_eq!(
        weaker(|s| s.collections[1].relations[0].classification = Some(Secret)),
        Vec::<Option<String>>::new()
    );
}

#[cfg(all(feature = "runtime-graphql", feature = "sqlite"))]
mod graphql {
    use super::*;
    use futures::future::BoxFuture;
    use graphql_orm::async_graphql::{Request, dynamic::Schema};
    use graphql_orm::graphql::runtime::*;
    use graphql_orm::prelude::*;
    use serde_json::{Value, json};
    use std::sync::Arc;

    struct Allow;
    impl RuntimeReadAuthority for Allow {
        fn authorize<'a>(
            &'a self,
            _check: RuntimeReadCheck<'a>,
        ) -> BoxFuture<'a, Result<RuntimeReadGrant, RuntimeGraphqlError>> {
            Box::pin(async { Ok(RuntimeReadGrant::new(None)) })
        }
    }

    async fn database() -> Database<SqliteBackend> {
        Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
            .await
            .unwrap()
    }

    fn module(schema: &Arc<ValidatedRuntimeSchema>) -> RuntimeGraphqlModule {
        RuntimeGraphqlModule::compile(
            schema.clone(),
            RuntimeGraphqlOptions::default()
                .with_cursor_profile(RuntimeCursorProfile::Unprotected)
                .with_mutations(Default::default()),
        )
        .unwrap()
    }

    async fn composed() -> (Arc<ValidatedRuntimeSchema>, Schema, GraphqlSemanticCatalog) {
        let schema = Arc::new(schema(true).validate().unwrap());
        let (api, catalog) =
            RuntimeGraphqlComposer::new(database().await, "Query", Default::default())
                .unwrap()
                .mutation_root("Mutation")
                .unwrap()
                .install(module(&schema))
                .unwrap()
                .finish_with_semantic_catalog()
                .unwrap();
        (schema, api, catalog)
    }

    async fn introspect(api: &Schema, schema: &ValidatedRuntimeSchema, query: &str) -> Value {
        let response = api
            .execute(
                Request::new(query).data(RuntimeGraphqlRequest::<SqliteBackend>::new(
                    schema.fingerprint(),
                    Arc::new(Allow),
                    None,
                    Default::default(),
                )),
            )
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        response.data.into_json().unwrap()
    }

    fn described(value: &Value, list: &str) -> Vec<(String, String)> {
        value[list]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                (
                    item["name"].as_str().unwrap().to_owned(),
                    item["description"].as_str().unwrap_or_default().to_owned(),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn introspection_returns_every_description() {
        let (schema, api, _) = composed().await;
        let data = introspect(
            &api,
            &schema,
            r#"{
                invoice: __type(name: "Invoice") { description fields { name description } }
                line: __type(name: "InvoiceLine") { description fields { name description } }
                query: __type(name: "Query") { fields { name description args { name description } } }
                mutation: __type(name: "Mutation") { fields { name description args { name description } } }
                create: __type(name: "CreateInvoiceInput") { description inputFields { name description } }
                filter: __type(name: "InvoiceWhereInput") { description inputFields { name description } }
                order: __type(name: "InvoiceOrderField") { description enumValues { name description } }
                connection: __type(name: "InvoiceConnection") { description }
                int64: __type(name: "RuntimeInt64") { description }
            }"#,
        )
        .await;
        assert_eq!(data["invoice"]["description"], "Customer invoice");
        assert_eq!(
            described(&data["invoice"], "fields"),
            [
                ("id".into(), "Id".into()),
                ("number".into(), "Invoice number shown to customers".into()),
                ("total".into(), "Total".into()),
                ("internalNote".into(), "Internal Note".into()),
                ("lines".into(), "Lines of the invoice".into()),
            ]
        );
        assert_eq!(data["line"]["description"], "One invoice line");
        assert_eq!(
            described(&data["line"], "fields")[3],
            ("owner".into(), "Invoice owning the line".into())
        );
        let roots = &data["query"]["fields"];
        let invoices = roots
            .as_array()
            .unwrap()
            .iter()
            .find(|root| root["name"] == "invoices")
            .unwrap();
        assert_eq!(invoices["description"], "Page through Invoice records");
        assert_eq!(
            described(invoices, "args"),
            [
                ("where".into(), "Where".into()),
                ("orderBy".into(), "Order by".into()),
                ("first".into(), "First".into()),
                ("after".into(), "After".into()),
                ("last".into(), "Last".into()),
                ("before".into(), "Before".into()),
            ]
        );
        let mutations = described(&data["mutation"], "fields");
        assert!(mutations.contains(&("createInvoice".into(), "Create one Invoice records".into())));
        assert!(mutations.contains(&(
            "deleteInvoiceLine".into(),
            "Delete one InvoiceLine records".into()
        )));
        let update = data["mutation"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|root| root["name"] == "updateInvoice")
            .unwrap();
        assert_eq!(
            described(update, "args"),
            [
                ("expectedSchema".into(), "Expected schema".into()),
                ("key".into(), "Key".into()),
                ("expected".into(), "Expected".into()),
                ("input".into(), "Input".into()),
            ]
        );
        assert_eq!(
            data["create"]["description"],
            "Values of a new Invoice record"
        );
        assert!(
            described(&data["create"], "inputFields")
                .contains(&("number".into(), "Invoice number shown to customers".into()))
        );
        assert!(
            described(&data["filter"], "inputFields")
                .contains(&("number".into(), "Invoice number shown to customers".into()))
        );
        assert!(
            described(&data["order"], "enumValues")
                .contains(&("number".into(), "Invoice number shown to customers".into()))
        );
        assert_eq!(
            data["connection"]["description"],
            "A page of Invoice records"
        );
        assert_eq!(
            data["int64"]["description"],
            "Signed 64-bit integer encoded as a decimal string"
        );
        // Every generated type is described; the host names the root objects.
        let sdl = api.sdl();
        for block in sdl.split("\n\n") {
            let block = block.trim();
            if ["schema", "directive", "type Query", "type Mutation"]
                .iter()
                .any(|prefix| block.starts_with(prefix))
                || block.is_empty()
            {
                continue;
            }
            assert!(
                block.starts_with("\"\"\""),
                "undescribed SDL block: {block}"
            );
        }
    }

    #[tokio::test]
    async fn semantic_catalog_carries_the_same_text_and_dispositions() {
        let (_, _, catalog) = composed().await;
        catalog.validate().unwrap();
        let invoice = catalog
            .entities
            .iter()
            .find(|entity| entity.entity_name == "Invoice")
            .unwrap();
        assert_eq!(invoice.description, "Customer invoice");
        assert_eq!(
            invoice.default_classification,
            GraphqlSemanticClassification::Confidential
        );
        let field = |name: &str| {
            invoice
                .fields
                .iter()
                .find(|field| field.field_name == name)
                .unwrap()
        };
        assert_eq!(
            field("number").description,
            "Invoice number shown to customers"
        );
        assert_eq!(
            field("number").classification,
            GraphqlSemanticClassification::Confidential
        );
        assert_eq!(field("number").export, GraphqlSemanticExport::Exportable);
        assert_eq!(
            field("number").type_ref,
            GraphqlSemanticTypeRef::named("String", GraphqlSemanticTypeKind::Scalar, false)
        );
        assert_eq!(field("number").filter_operators, {
            use GraphqlSemanticFilterOperator::*;
            vec![
                Equal,
                NotEqual,
                In,
                LessThan,
                LessThanOrEqual,
                GreaterThan,
                GreaterThanOrEqual,
                Contains,
                StartsWith,
                EndsWith,
                IsNull,
            ]
        });
        assert!(field("number").sortable && !field("number").groupable);
        assert!(field("number").aggregate_operators.is_empty());
        assert_eq!(field("total").export, GraphqlSemanticExport::NeverExport);
        assert_eq!(
            field("internalNote").classification,
            GraphqlSemanticClassification::Secret
        );
        assert_eq!(
            field("internalNote").export,
            GraphqlSemanticExport::NeverExport
        );
        // To-many relations are connection-typed and are not advertised.
        assert!(
            invoice
                .fields
                .iter()
                .all(|field| field.field_name != "lines")
        );
        let line = catalog
            .entities
            .iter()
            .find(|entity| entity.entity_name == "InvoiceLine")
            .unwrap();
        let owner = line
            .fields
            .iter()
            .find(|f| f.field_name == "owner")
            .unwrap();
        assert_eq!(owner.description, "Invoice owning the line");
        assert_eq!(
            owner.relationship.as_ref().unwrap().target,
            "Invoice".to_string()
        );

        let operation = |kind, name: &str| {
            catalog
                .operations
                .iter()
                .find(|op| op.kind == kind && op.field_name == name)
                .unwrap()
        };
        let list = operation(GraphqlOperationKind::Query, "invoices");
        assert_eq!(list.description, "Page through Invoice records");
        assert_eq!(
            list.generated_category,
            Some(GeneratedGraphqlOperationCategory::KeysetList)
        );
        assert_eq!(list.generated_entity_name.as_deref(), Some("Invoice"));
        assert_eq!(list.arguments[1].description, "Order by");
        let create = operation(GraphqlOperationKind::Mutation, "createInvoice");
        assert_eq!(create.description, "Create one Invoice records");
        assert_eq!(
            create.ai_mutation_execution,
            Some(AiMutationExecutionPolicy::Prohibited)
        );
        assert_eq!(catalog.operations.len(), 2 * 4);
        let reloaded =
            GraphqlSemanticCatalog::from_extension_payload(catalog.extension_payload().unwrap())
                .unwrap();
        assert_eq!(reloaded, catalog);
    }

    #[tokio::test]
    async fn host_roots_must_be_described_for_the_catalog() {
        let schema = Arc::new(schema(false).validate().unwrap());
        let host = || {
            RuntimeHostField::new(
                "serverTime",
                graphql_orm::async_graphql::dynamic::TypeRef::named_nn("String"),
                |_| {
                    graphql_orm::async_graphql::dynamic::FieldFuture::new(async {
                        Ok(Some(graphql_orm::async_graphql::Value::from("now")))
                    })
                },
            )
            .argument(
                "zone",
                graphql_orm::async_graphql::dynamic::TypeRef::named("String"),
                None,
            )
            .unwrap()
        };
        let descriptor = |name: &str, kind| {
            GraphqlSemanticOperationDescriptor::custom(
                kind,
                name,
                "Current server time",
                vec![GraphqlSemanticArgumentDescriptor {
                    graphql_name: "zone".into(),
                    description: "Time zone name".into(),
                    type_ref: parse_graphql_type("String").unwrap(),
                }],
                parse_graphql_type("String!").unwrap(),
                false,
            )
            .unwrap()
        };
        let composer = || async {
            RuntimeGraphqlComposer::new(database().await, "Query", Default::default())
                .unwrap()
                .install(
                    RuntimeGraphqlModule::compile(
                        schema.clone(),
                        RuntimeGraphqlOptions::default()
                            .with_cursor_profile(RuntimeCursorProfile::Unprotected),
                    )
                    .unwrap(),
                )
                .unwrap()
        };
        let undescribed = composer().await.query_field(host()).unwrap();
        assert_eq!(
            undescribed
                .finish_with_semantic_catalog()
                .err()
                .unwrap()
                .code(),
            "semantic_descriptor_missing"
        );
        composer()
            .await
            .query_field(host())
            .unwrap()
            .finish()
            .unwrap();

        assert_eq!(
            host()
                .semantic(descriptor("otherTime", GraphqlOperationKind::Query))
                .err()
                .unwrap()
                .code(),
            "invalid_semantic_catalog"
        );
        let wrong_kind = host()
            .semantic(descriptor("serverTime", GraphqlOperationKind::Subscription))
            .unwrap();
        assert_eq!(
            composer()
                .await
                .query_field(wrong_kind)
                .err()
                .unwrap()
                .code(),
            "invalid_semantic_catalog"
        );

        let (api, catalog) = composer()
            .await
            .query_field(
                host()
                    .semantic(descriptor("serverTime", GraphqlOperationKind::Query))
                    .unwrap(),
            )
            .unwrap()
            .finish_with_semantic_catalog()
            .unwrap();
        assert!(
            catalog
                .operations
                .iter()
                .any(|op| op.field_name == "serverTime" && op.description == "Current server time")
        );
        let data = introspect(
            &api,
            &schema,
            r#"{ __type(name: "Query") { fields { name description args { name description } } } }"#,
        )
        .await;
        let root = data["__type"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|root| root["name"] == "serverTime")
            .unwrap()
            .clone();
        assert_eq!(
            root,
            json!({
                "name": "serverTime",
                "description": "Current server time",
                "args": [{"name": "zone", "description": "Time zone name"}]
            })
        );
    }
}
