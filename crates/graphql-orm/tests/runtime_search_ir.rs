//! Runtime collection full-text search declarations: validation, identity,
//! serialization and conversion from static search entities.
use graphql_orm::graphql::orm::*;

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

fn entry(name: &str, weight: RuntimeSearchWeight, path: Option<&str>) -> RuntimeSearchField {
    RuntimeSearchField {
        field: fid(&format!("f_{name}")),
        weight,
        json_path: path.map(str::to_string),
    }
}

fn search(fields: Vec<RuntimeSearchField>) -> RuntimeSearch {
    RuntimeSearch {
        fields,
        language: None,
        tokenizer: None,
        remove_diacritics: false,
        min_token_len: None,
    }
}

fn schema(search: Option<RuntimeSearch>) -> RuntimeSchema {
    let mut id = field("id", RuntimeValueKind::Uuid);
    id.nullable = false;
    RuntimeSchema {
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![RuntimeCollection {
            id: CollectionId::new("records").unwrap(),
            api_type_name: "Record".into(),
            api_plural_name: "Records".into(),
            physical_table: "search_records".into(),
            primary_key: vec![fid("f_id")],
            append_only: false,
            retention_purge: false,
            fields: vec![
                id,
                field("title", RuntimeValueKind::String),
                field("body", RuntimeValueKind::String),
                field("meta", RuntimeValueKind::Json),
                field("code", RuntimeValueKind::Integer),
            ],
            relations: vec![],
            indexes: vec![],
            composite_unique: vec![],
            default_order: vec![],
            description: None,
            classification: None,
            label_fields: vec![],
            search,
        }],
    }
}

fn valid() -> RuntimeSearch {
    search(vec![
        entry("title", RuntimeSearchWeight::A, None),
        entry("body", RuntimeSearchWeight::C, None),
        entry("meta", RuntimeSearchWeight::B, Some("$.summary")),
    ])
}

fn codes(search: RuntimeSearch) -> Vec<RuntimeSchemaDiagnosticCode> {
    schema(Some(search))
        .validate()
        .unwrap_err()
        .into_diagnostics()
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn search_declarations_are_validated_with_structured_diagnostics() {
    use RuntimeSchemaDiagnosticCode as C;
    use RuntimeSearchWeight::{A, B};
    schema(Some(valid())).validate().unwrap();

    assert_eq!(
        codes(search(vec![entry("missing", A, None)])),
        [C::UnknownFieldReference]
    );
    assert_eq!(
        codes(search(vec![entry("code", A, None)])),
        [C::InvalidSearchField]
    );
    assert_eq!(
        codes(search(vec![entry("meta", A, None)])),
        [C::InvalidSearchField]
    );
    assert_eq!(
        codes(search(vec![entry("title", A, Some("$.a"))])),
        [C::InvalidSearchField]
    );
    assert_eq!(
        codes(search(vec![entry("meta", A, Some("summary"))])),
        [C::InvalidSearchField]
    );
    assert_eq!(
        codes(search(vec![
            entry("title", A, None),
            entry("title", B, None)
        ])),
        [C::InvalidSearchField]
    );
    assert_eq!(codes(search(vec![])), [C::InvalidSearchConfiguration]);
    let too_many = (0..=RUNTIME_MAX_SEARCH_FIELDS)
        .map(|index| entry("meta", A, Some(&format!("$.part{index}"))))
        .collect();
    assert_eq!(codes(search(too_many)), [C::InvalidSearchConfiguration]);

    for language in ["English", "english;drop", "", &"a".repeat(64), "1english"] {
        let mut declaration = valid();
        declaration.language = Some(language.to_string());
        assert_eq!(
            codes(declaration),
            [C::InvalidSearchConfiguration],
            "{language:?}"
        );
    }
    let mut stemmed = valid();
    stemmed.tokenizer = Some(RuntimeSearchTokenizer::Porter);
    stemmed.remove_diacritics = true;
    assert_eq!(codes(stemmed), [C::InvalidSearchConfiguration]);
    for length in [0, 17] {
        let mut declaration = valid();
        declaration.min_token_len = Some(length);
        assert_eq!(
            codes(declaration),
            [C::InvalidSearchConfiguration],
            "{length}"
        );
    }

    let mut folded = valid();
    folded.remove_diacritics = true;
    folded.language = Some("simple".into());
    folded.min_token_len = Some(1);
    schema(Some(folded)).validate().unwrap();
    assert_eq!(C::InvalidSearchField.as_str(), "invalid_search_field");
    assert_eq!(
        C::InvalidSearchConfiguration.as_str(),
        "invalid_search_configuration"
    );
}

#[test]
fn search_is_structural_and_absent_search_keeps_earlier_bytes() {
    let plain = schema(None).validate().unwrap();
    let searched = schema(Some(valid())).validate().unwrap();
    assert_ne!(plain.fingerprint(), searched.fingerprint());
    assert_ne!(
        plain.structural_fingerprint(),
        searched.structural_fingerprint()
    );
    let plain_bytes = String::from_utf8_lossy(&plain.canonical_bytes()).into_owned();
    assert!(!plain_bytes.contains("  search|") && !plain_bytes.contains("  search_field|"));
    let searched_bytes = String::from_utf8_lossy(&searched.canonical_bytes()).into_owned();
    assert!(searched_bytes.contains("  search|language=english|tokenizer=unicode61"));

    // Every member is part of the structure.
    let mut reweighted = valid();
    reweighted.fields[0].weight = RuntimeSearchWeight::B;
    let mut repathed = valid();
    repathed.fields[2].json_path = Some("$.other".into());
    let mut folded = valid();
    folded.remove_diacritics = true;
    for changed in [reweighted, repathed, folded] {
        let changed = schema(Some(changed)).validate().unwrap();
        assert_ne!(
            changed.structural_fingerprint(),
            searched.structural_fingerprint()
        );
    }
    // Explicit defaults are the same structure as omitted ones.
    let mut explicit = valid();
    explicit.language = Some("english".into());
    explicit.tokenizer = Some(RuntimeSearchTokenizer::Unicode61);
    explicit.min_token_len = Some(2);
    assert_eq!(
        schema(Some(explicit))
            .validate()
            .unwrap()
            .structural_fingerprint(),
        searched.structural_fingerprint()
    );
}

#[test]
fn search_round_trips_and_is_omitted_when_absent() {
    let plain = serde_json::to_value(schema(None)).unwrap();
    assert!(plain["collections"][0].get("search").is_none());

    let declared = schema(Some(valid()));
    let value = serde_json::to_value(&declared).unwrap();
    assert_eq!(
        value["collections"][0]["search"],
        serde_json::json!({
            "fields": [
                {"field": "f_title", "weight": "A"},
                {"field": "f_body", "weight": "C"},
                {"field": "f_meta", "weight": "B", "json_path": "$.summary"}
            ]
        })
    );
    let decoded: RuntimeSchema = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(decoded, declared);

    let mut unknown = value;
    unknown["collections"][0]["search"]["stemmer"] = serde_json::json!("x");
    assert!(serde_json::from_value::<RuntimeSchema>(unknown).is_err());
}

#[cfg(feature = "sqlite")]
mod from_static {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "static_articles",
        plural = "StaticArticles",
        backend = "sqlite"
    )]
    #[graphql_orm(search(
        index = true,
        tokenizer = "unicode61 remove_diacritics 2",
        min_token_len = 3,
        language = "simple"
    ))]
    struct StaticArticle {
        #[primary_key]
        #[filterable(type = "string")]
        pub id: String,
        #[graphql_orm(searchable(weight = "A"))]
        #[sortable]
        pub title: String,
        #[graphql_orm(searchable(weight = "B"))]
        pub body: Option<String>,
    }

    #[test]
    fn a_static_search_index_maps_to_the_runtime_declaration() {
        let converted =
            RuntimeSchema::from_static_entities(&[StaticArticle::metadata()]).expect("converted");
        let search = converted.collections[0]
            .search
            .clone()
            .expect("search is represented");
        let field = |column: &str| FieldId::new(format!("static_articles.{column}")).unwrap();
        assert_eq!(
            search,
            RuntimeSearch {
                fields: vec![
                    RuntimeSearchField {
                        field: field("title"),
                        weight: RuntimeSearchWeight::A,
                        json_path: None,
                    },
                    RuntimeSearchField {
                        field: field("body"),
                        weight: RuntimeSearchWeight::B,
                        json_path: None,
                    },
                ],
                language: Some("simple".into()),
                tokenizer: None,
                remove_diacritics: true,
                min_token_len: Some(3),
            }
        );
        converted
            .validate()
            .expect("the converted declaration is valid");
    }
}
