//! Checked intent, offline selection, and real SQL conformance on owned databases.
use graphql_orm::graphql::orm::*;
#[cfg(all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))))]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
fn id(s: &str) -> FieldId {
    FieldId::new(s).unwrap()
}
fn schema() -> ValidatedRuntimeSchema {
    RuntimeSchema {
        format_version: 1,
        collections: vec![RuntimeCollection {
            classification: None,
            description: None,
            id: CollectionId::new("samples").unwrap(),
            api_type_name: "Sample".into(),
            api_plural_name: "Samples".into(),
            physical_table: "checked_filter_samples".into(),
            primary_key: vec![id("id")],
            append_only: false,
            retention_purge: false,
            fields: [
                ("id", RuntimeValueKind::Integer),
                ("text", RuntimeValueKind::String),
                ("number", RuntimeValueKind::Integer),
                ("score", RuntimeValueKind::Float),
                ("uid", RuntimeValueKind::Uuid),
                ("bytes", RuntimeValueKind::Bytes),
                ("at", RuntimeValueKind::DateTime),
                ("flag", RuntimeValueKind::Boolean),
                ("json", RuntimeValueKind::Json),
            ]
            .into_iter()
            .map(|(name, kind)| RuntimeField {
                description: None,
                classification: None,
                export: None,
                id: id(name),
                api_name: name.into(),
                physical_column: name.into(),
                value_kind: kind,
                nullable: name != "id",
                unique: name == "id",
                filterable: true,
                sortable: kind != RuntimeValueKind::Json,
                generated: false,
                default: None,
                format: None,
                auto_timestamp: None,
                auto_actor: None,
            })
            .collect(),
            relations: vec![],
            indexes: vec![],
            composite_unique: vec![],
            default_order: vec![],
            label_fields: Vec::new(),
            search: None,
        }],
    }
    .validate()
    .unwrap()
}
fn collection(s: &ValidatedRuntimeSchema) -> RuntimeCollectionHandle {
    s.resolve_collection(&CollectionId::new("samples").unwrap())
        .unwrap()
}
fn field(s: &ValidatedRuntimeSchema, name: &str) -> RuntimeFieldHandle {
    s.resolve_field(&collection(s), &id(name)).unwrap()
}
fn builder(s: &ValidatedRuntimeSchema) -> RuntimeFilterBuilder {
    s.runtime_filter_builder(&collection(s), Default::default())
        .unwrap()
}
fn record(s: &ValidatedRuntimeSchema, values: Vec<(&str, RuntimeValue)>) -> RuntimeRecord {
    let kinds: std::collections::BTreeMap<_, _> = s.schema().collections[0]
        .fields
        .iter()
        .map(|f| (f.id.clone(), f.value_kind))
        .collect();
    let values: std::collections::BTreeMap<_, _> = values
        .into_iter()
        .map(|(name, value)| (id(name), value))
        .collect();
    RuntimeRecord::from_json(&serde_json::json!({"format_version":1,"schema_fingerprint":s.fingerprint(),"collection":collection(s).id(),"field_kinds":kinds,"values":values}).to_string()).unwrap()
}
#[derive(Default)]
struct IntentLog(Vec<String>);
impl RuntimeFilterIntentVisitor for IntentLog {
    fn enter_boolean(&mut self, k: RuntimeFilterBoolean, ignored: bool) {
        self.0.push(format!("enter:{k:?}:{ignored}"))
    }
    fn leave_boolean(&mut self, k: RuntimeFilterBoolean, ignored: bool) {
        self.0.push(format!("leave:{k:?}:{ignored}"))
    }
    fn field(&mut self, f: &RuntimeFieldHandle, op: RuntimeFilterOperator, ignored: bool) {
        self.0.push(format!("{}:{op:?}:{ignored}", f.id().as_str()))
    }
}
#[test]
fn original_empty_membership_dependency_is_checked_before_constant_shortcuts() {
    let s = schema();
    let b = builder(&s);
    let text = field(&s, "text");
    let empty = b
        .membership(&text, RuntimeListOperator::In, vec![])
        .unwrap();
    let checked = b.finish(empty).unwrap();
    assert!(checked.predicate().referenced_fields().is_empty());
    let mut log = IntentLog::default();
    checked.visit_intent(&mut log);
    assert_eq!(log.0, ["text:List(In):false"]);
    let matcher = checked
        .prepare_match(RuntimeMatchProfile::SqliteBinaryCanonical)
        .unwrap();
    assert_eq!(
        matcher
            .matches(&record(&s, vec![("id", RuntimeValue::Integer(1))]))
            .map_err(|e| e.code()),
        Err(RuntimeFilterErrorCode::MissingLoadedField)
    );
    assert!(
        !matcher
            .matches(&record(&s, vec![("text", RuntimeValue::Null)]))
            .unwrap()
    );
    let b = builder(&s);
    let all = b
        .membership(&text, RuntimeListOperator::NotIn, vec![])
        .unwrap();
    let checked = b.finish(all).unwrap();
    assert!(
        checked
            .prepare_match(RuntimeMatchProfile::PostgresCUtf8)
            .unwrap()
            .matches(&record(&s, vec![("text", RuntimeValue::Null)]))
            .unwrap()
    );
}
#[test]
fn sql_unknown_is_not_inverted_and_complete_dependencies_are_preflighted() {
    let s = schema();
    let b = builder(&s);
    let p = b
        .compare(
            &field(&s, "text"),
            RuntimeScalarOperator::Eq,
            RuntimeValue::String("secret".into()),
        )
        .unwrap();
    let p = b.not(p).unwrap();
    let c = b.finish(p).unwrap();
    let m = c
        .prepare_match(RuntimeMatchProfile::SqliteBinaryCanonical)
        .unwrap();
    assert!(
        !m.matches(&record(&s, vec![("text", RuntimeValue::Null)]))
            .unwrap()
    );
    assert!(
        m.matches(&record(
            &s,
            vec![("text", RuntimeValue::String("other".into()))]
        ))
        .unwrap()
    );
    let b = builder(&s);
    let truth = b.and(vec![]).unwrap();
    let need = b.is_null(&field(&s, "number"), true).unwrap();
    let p = b.or(vec![truth, need]).unwrap();
    let c = b.finish(p).unwrap();
    assert_eq!(
        c.prepare_match(RuntimeMatchProfile::PostgresCUtf8)
            .unwrap()
            .matches(&record(&s, vec![]))
            .map_err(|e| e.code()),
        Err(RuntimeFilterErrorCode::MissingLoadedField)
    );
    assert!(!format!("{c:?} {m:?}").contains("secret"));
}
#[test]
fn schema_bounds_foreign_nodes_and_actual_sqlite_suffix_binds_are_checked() {
    let s = schema();
    let mut other = s.clone().into_schema();
    other.collections[0].fields[1].api_name = "different".into();
    let other = other.validate().unwrap();
    let b = builder(&s);
    let foreign = builder(&other)
        .is_null(&field(&other, "text"), true)
        .unwrap();
    assert!(b.and(vec![foreign]).is_err());
    let mut limits = RuntimeQueryLimits {
        max_bind_parameters: 1,
        ..Default::default()
    };
    let b = s.runtime_filter_builder(&collection(&s), limits).unwrap();
    assert_eq!(
        b.compare(
            &field(&s, "text"),
            RuntimeScalarOperator::EndsWith,
            RuntimeValue::String("x".into())
        )
        .unwrap_err()
        .code(),
        RuntimeFilterErrorCode::LimitExceeded
    );
    assert_eq!(
        builder(&s)
            .compare(
                &field(&s, "text"),
                RuntimeScalarOperator::Eq,
                RuntimeValue::String("x".repeat(RUNTIME_FILTER_MAX_TEXT_BYTES + 1))
            )
            .unwrap_err()
            .code(),
        RuntimeFilterErrorCode::LimitExceeded
    );
    let node = builder(&s)
        .membership(
            &field(&s, "number"),
            RuntimeListOperator::In,
            vec![RuntimeValue::Integer(1), RuntimeValue::Integer(2)],
        )
        .unwrap();
    limits.max_values_per_list = 1;
    limits.max_bind_parameters = 999;
    assert_eq!(
        s.runtime_filter_builder(&collection(&s), limits)
            .unwrap()
            .finish(node)
            .unwrap_err()
            .code(),
        RuntimeFilterErrorCode::LimitExceeded
    );
}
#[test]
fn unsupported_profiles_and_nonportable_values_fail_before_record_disclosure() {
    let s = schema();
    let b = builder(&s);
    let n = b.is_null(&field(&s, "text"), true).unwrap();
    let c = b.finish(n).unwrap();
    assert_eq!(
        c.prepare_match(RuntimeMatchProfile::MssqlUnsupported)
            .unwrap_err()
            .code(),
        RuntimeFilterErrorCode::UnsupportedSemantics
    );
    let m = c
        .prepare_match(RuntimeMatchProfile::SqliteBinaryCanonical)
        .unwrap();
    for text in ["a\0b", "line\nbreak", "tab\t"] {
        assert_eq!(
            m.matches(&record(
                &s,
                vec![("text", RuntimeValue::String(text.into()))]
            ))
            .map_err(|e| e.code()),
            Err(RuntimeFilterErrorCode::UnsupportedSemantics)
        );
    }
    for raw in ["2016-12-31T23:59:60Z", "0000-01-01T00:00:00Z"] {
        let value = RuntimeDateTime::parse(raw).unwrap();
        let b = builder(&s);
        let n = b
            .compare(
                &field(&s, "at"),
                RuntimeScalarOperator::Eq,
                RuntimeValue::DateTime(value.clone()),
            )
            .unwrap();
        let c = b.finish(n).unwrap();
        assert_eq!(
            c.prepare_match(RuntimeMatchProfile::PostgresCUtf8)
                .unwrap_err()
                .code(),
            RuntimeFilterErrorCode::UnsupportedSemantics
        );
        let b = builder(&s);
        let n = b.is_null(&field(&s, "at"), false).unwrap();
        let c = b.finish(n).unwrap();
        assert_eq!(
            c.prepare_match(RuntimeMatchProfile::PostgresCUtf8)
                .unwrap()
                .matches(&record(&s, vec![("at", RuntimeValue::DateTime(value))]))
                .map_err(|e| e.code()),
            Err(RuntimeFilterErrorCode::UnsupportedSemantics)
        );
    }
}
#[test]
fn native_scalar_extremes_are_exact_without_mixed_kind_coercion() {
    let s = schema();
    for n in [i64::MIN, i64::MAX, 0] {
        let b = builder(&s);
        let p = b
            .compare(
                &field(&s, "number"),
                RuntimeScalarOperator::Eq,
                RuntimeValue::Integer(n),
            )
            .unwrap();
        let c = b.finish(p).unwrap();
        assert!(
            c.prepare_match(RuntimeMatchProfile::SqliteBinaryCanonical)
                .unwrap()
                .matches(&record(&s, vec![("number", RuntimeValue::Integer(n))]))
                .unwrap()
        );
    }
    for n in [
        f64::MIN,
        f64::MAX,
        f64::MIN_POSITIVE,
        f64::from_bits(1),
        -0.0,
    ] {
        let b = builder(&s);
        let value = RuntimeValue::Float(RuntimeFloat::new(n).unwrap());
        let p = b
            .compare(
                &field(&s, "score"),
                RuntimeScalarOperator::Eq,
                value.clone(),
            )
            .unwrap();
        let c = b.finish(p).unwrap();
        assert!(
            c.prepare_match(RuntimeMatchProfile::PostgresCUtf8)
                .unwrap()
                .matches(&record(&s, vec![("score", value)]))
                .unwrap()
        );
    }
}

#[cfg(feature = "runtime-graphql")]
#[test]
fn graphql_original_occurrences_and_null_entries_survive_constant_lowering() {
    use graphql_orm::{async_graphql::value, graphql::runtime::*};
    let s = schema();
    let c = prepare_runtime_filter(
        &s,
        &collection(&s),
        &value!({"or":[{"text":{"in":[]}},{"text":{"in":[]}}],"number":{"eq":null},"not":null}),
        Default::default(),
    )
    .unwrap();
    let mut log = IntentLog::default();
    c.visit_intent(&mut log);
    assert_eq!(
        log.0
            .iter()
            .filter(|v| v.as_str() == "text:List(In):false")
            .count(),
        2
    );
    assert!(log.0.contains(&"enter:Or:false".into()));
    assert!(log.0.contains(&"enter:Not:true".into()));
    assert!(log.0.contains(&"number:Scalar(Eq):true".into()));
    let partial = record(&s, vec![("text", RuntimeValue::Null)]);
    assert_eq!(
        c.prepare_match(RuntimeMatchProfile::SqliteBinaryCanonical)
            .unwrap()
            .matches(&partial)
            .map_err(|e| e.code()),
        Err(RuntimeFilterErrorCode::MissingLoadedField)
    );
}
#[cfg(feature = "runtime-graphql")]
#[test]
fn graphql_parser_erases_wire_duplicate_keys_before_runtime_validation() {
    use graphql_orm::async_graphql::{
        Value,
        parser::{
            parse_query,
            types::{DocumentOperations, Selection},
        },
    };
    let document =
        parse_query("{ samples(where: { text: { eq: \"first\", eq: \"last\" } }) { id } }")
            .unwrap();
    let DocumentOperations::Single(op) = document.operations else {
        panic!("single operation")
    };
    let Selection::Field(field) = &op.node.selection_set.node.items[0].node else {
        panic!("field")
    };
    let value = field.node.arguments[0]
        .1
        .node
        .clone()
        .into_const_with(|_| Err::<Value, ()>(()))
        .unwrap();
    let Value::Object(fields) = value else {
        panic!("object")
    };
    let Value::Object(ops) = &fields["text"] else {
        panic!("operators")
    };
    assert_eq!(ops.len(), 1);
    assert_eq!(ops["eq"], Value::String("last".into()));
    // JSON Variables deserialization has the same boundary; the new API cannot
    // recover erased wire occurrences. Hosts must validate BEFORE creating Value.
    let value: Value = serde_json::from_str(r#"{"eq":"first","eq":"last"}"#).unwrap();
    let Value::Object(ops) = value else {
        panic!("map")
    };
    assert_eq!(ops.len(), 1);
}

#[cfg(feature = "runtime-graphql")]
#[test]
fn implicit_and_grouping_is_preserved_without_inventing_flat_permissions() {
    use graphql_orm::{async_graphql::value, graphql::runtime::*};
    let s = schema();
    let grouped=prepare_runtime_filter(&s,&collection(&s),&value!({"or":[{"text":{"eq":"a"},"number":{"eq":"1"}},{"text":{"eq":"b"},"number":{"eq":"2"}}]}),Default::default()).unwrap();
    let flat=prepare_runtime_filter(&s,&collection(&s),&value!({"or":[{"text":{"eq":"a"}},{"number":{"eq":"1"}},{"text":{"eq":"b"}},{"number":{"eq":"2"}}]}),Default::default()).unwrap();
    let mut a = IntentLog::default();
    let mut b = IntentLog::default();
    grouped.visit_intent(&mut a);
    flat.visit_intent(&mut b);
    assert_ne!(a.0, b.0);
    assert!(a.0.iter().any(|v| v == "enter:ImplicitAnd:false"));
    let row = record(
        &s,
        vec![
            ("text", RuntimeValue::String("a".into())),
            ("number", RuntimeValue::Integer(2)),
        ],
    );
    assert!(
        !grouped
            .prepare_match(RuntimeMatchProfile::SqliteBinaryCanonical)
            .unwrap()
            .matches(&row)
            .unwrap()
    );
    assert!(
        flat.prepare_match(RuntimeMatchProfile::SqliteBinaryCanonical)
            .unwrap()
            .matches(&row)
            .unwrap()
    );
    assert_eq!(
        prepare_runtime_filter(
            &s,
            &collection(&s),
            &value!({"number":{"contains":null}}),
            Default::default()
        )
        .unwrap_err()
        .code(),
        RuntimeFilterErrorCode::UnsupportedSemantics
    );
}

#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
mod sql_parity {
    use super::*;
    use graphql_orm::db::Database;
    #[cfg(feature = "sqlite")]
    type Backend = SqliteBackend;
    #[cfg(feature = "postgres")]
    type Backend = PostgresBackend;
    #[cfg(feature = "postgres")]
    use super::owned_postgres;
    async fn read(
        db: &Database<Backend>,
        s: &ValidatedRuntimeSchema,
        p: RuntimePredicate,
    ) -> Vec<RuntimeRecord> {
        let c = collection(s);
        let fields = s.schema().collections[0]
            .fields
            .iter()
            .filter(|f| f.id != id("json"))
            .map(|f| field(s, f.id.as_str()))
            .collect::<Vec<_>>();
        let projection = s.resolve_projection(&c, &fields).unwrap();
        let limits = RuntimeQueryLimits::default();
        let request = s
            .runtime_read_request(
                &c,
                &projection,
                Some(p),
                s.runtime_order(&c, None, limits).unwrap(),
                RuntimePageRequest::first(100, None),
                false,
                limits,
            )
            .unwrap();
        db.execute_runtime_read(&request, None)
            .await
            .unwrap()
            .edges
            .into_iter()
            .map(|e| e.node)
            .collect()
    }
    #[tokio::test]
    async fn actual_sql_selected_ids_equal_offline_matches_for_canonical_storage()
    -> Result<(), Box<dyn std::error::Error>> {
        for k in [
            "DATABASE_URL",
            "TEST_DATABASE_URL",
            "MSSQL_TEST_DATABASE_URL",
        ] {
            assert!(std::env::var_os(k).is_none())
        }
        #[cfg(feature = "sqlite")]
        let db = Database::<Backend>::connect_sqlite_with_options(
            "sqlite::memory:",
            graphql_orm::db::ConnectionOptions::default().max_connections(1),
        )
        .await?;
        #[cfg(feature = "postgres")]
        let mut owned = owned_postgres::OwnedPostgres::start("checked-filter-parity")?;
        #[cfg(feature = "postgres")]
        let db = Database::<Backend>::connect_postgres(&owned.url).await?;
        #[cfg(feature = "postgres")]
        {
            let encoding: String = sqlx::query_scalar("SHOW server_encoding")
                .fetch_one(db.pool())
                .await?;
            assert_eq!(encoding, "UTF8");
            let version: String = sqlx::query_scalar("SHOW server_version")
                .fetch_one(db.pool())
                .await?;
            eprintln!("OWNED_FILTER_PG encoding={encoding} version={version}");
        }
        #[cfg(feature = "sqlite")]
        let types = ("TEXT", "BLOB", "TEXT");
        #[cfg(feature = "postgres")]
        let types = ("UUID", "BYTEA", "TIMESTAMPTZ");
        sqlx::query(&format!("CREATE TABLE checked_filter_samples(id BIGINT PRIMARY KEY, text TEXT, number BIGINT, score DOUBLE PRECISION, uid {}, bytes {}, at {}, flag BOOLEAN)",types.0,types.1,types.2)).execute(db.pool()).await?;
        let uid = graphql_orm::uuid::Uuid::parse_str("67e55044-10b1-426f-9247-bb680e5fe0c8")?;
        let texts = [
            None,
            Some(""),
            Some("a"),
            Some("alphabet"),
            Some("π雪%_"),
            Some("Ω"),
            Some("a%b_"),
            Some("longer than operand"),
        ];
        let dates = [
            "0001-01-01T00:00:00Z",
            "2026-10-15T00:00:00.000001Z",
            "2026-10-15T01:30:00+01:30",
            "2026-10-14T23:00:00-01:00",
            "2026-10-15T00:00:00.1234567Z",
            "9999-12-31T23:59:59Z",
        ];
        for (i, text) in texts.iter().enumerate() {
            let at = if i == 0 {
                None
            } else {
                Some(RuntimeDateTime::parse(dates[i % dates.len()])?)
            };
            let n = if i == 0 {
                None
            } else {
                Some([i64::MIN, i64::MAX, 0, -1, 1][i % 5])
            };
            let score = [
                f64::MIN,
                f64::MAX,
                f64::MIN_POSITIVE,
                f64::from_bits(1),
                0.0,
            ][i % 5];
            #[cfg(feature = "sqlite")]
            sqlx::query("INSERT INTO checked_filter_samples VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(i as i64)
                .bind(text)
                .bind(n)
                .bind(score)
                .bind(uid.to_string())
                .bind(vec![0u8, 255, i as u8])
                .bind(at.as_ref().map(RuntimeDateTime::as_str))
                .bind(i % 2 == 0)
                .execute(db.pool())
                .await?;
            #[cfg(feature = "postgres")]
            sqlx::query(
                "INSERT INTO checked_filter_samples VALUES ($1,$2,$3,$4,$5,$6,$7::timestamptz,$8)",
            )
            .bind(i as i64)
            .bind(text)
            .bind(n)
            .bind(score)
            .bind(uid)
            .bind(vec![0u8, 255, i as u8])
            .bind(at.as_ref().map(RuntimeDateTime::as_str))
            .bind(i % 2 == 0)
            .execute(db.pool())
            .await?;
        }
        let s = schema();
        let b = builder(&s);
        let all_filter = b.and(vec![])?;
        let all_filter = b.finish(all_filter)?;
        let all = read(&db, &s, all_filter.predicate().clone()).await;
        #[cfg(feature = "sqlite")]
        let profile = RuntimeMatchProfile::SqliteBinaryCanonical;
        #[cfg(feature = "postgres")]
        let profile = RuntimeMatchProfile::PostgresCUtf8;
        let mut checked = Vec::new();
        for op in [
            RuntimeScalarOperator::Eq,
            RuntimeScalarOperator::Ne,
            RuntimeScalarOperator::Lt,
            RuntimeScalarOperator::Lte,
            RuntimeScalarOperator::Gt,
            RuntimeScalarOperator::Gte,
            RuntimeScalarOperator::Contains,
            RuntimeScalarOperator::StartsWith,
            RuntimeScalarOperator::EndsWith,
        ] {
            for operand in ["", "a", "π", "%_", "longer than every sample here"] {
                let b = builder(&s);
                let p = b.compare(&field(&s, "text"), op, RuntimeValue::String(operand.into()))?;
                checked.push(b.finish(p)?);
            }
        }
        for op in [RuntimeListOperator::In, RuntimeListOperator::NotIn] {
            for values in [
                vec![],
                vec![
                    RuntimeValue::String("a".into()),
                    RuntimeValue::String("π雪%_".into()),
                ],
            ] {
                let b = builder(&s);
                let p = b.membership(&field(&s, "text"), op, values)?;
                checked.push(b.finish(p)?);
            }
        }
        for n in [i64::MIN, i64::MAX, 0] {
            let b = builder(&s);
            let p = b.compare(
                &field(&s, "number"),
                RuntimeScalarOperator::Gte,
                RuntimeValue::Integer(n),
            )?;
            let p = b.not(p)?;
            checked.push(b.finish(p)?);
        }
        for v in [
            f64::MIN,
            f64::MAX,
            f64::MIN_POSITIVE,
            f64::from_bits(1),
            -0.0,
        ] {
            let b = builder(&s);
            let p = b.compare(
                &field(&s, "score"),
                RuntimeScalarOperator::Eq,
                RuntimeValue::Float(RuntimeFloat::new(v)?),
            )?;
            checked.push(b.finish(p)?);
        }
        for (name, value) in [
            ("uid", RuntimeValue::Uuid(uid)),
            ("bytes", RuntimeValue::Bytes(vec![0, 255, 3])),
            (
                "at",
                RuntimeValue::DateTime(RuntimeDateTime::parse("2026-10-15T00:00:00Z")?),
            ),
            ("flag", RuntimeValue::Boolean(true)),
        ] {
            let b = builder(&s);
            let p = b.compare(&field(&s, name), RuntimeScalarOperator::Eq, value)?;
            checked.push(b.finish(p)?);
        }
        {
            let b = builder(&s);
            let p = b.between(
                &field(&s, "number"),
                RuntimeValue::Integer(-1),
                RuntimeValue::Integer(1),
            )?;
            let q = b.is_null(&field(&s, "text"), true)?;
            let p = b.and(vec![p, q])?;
            let p = b.not(p)?;
            checked.push(b.finish(p)?);
        }
        for raw in dates {
            for op in [
                RuntimeScalarOperator::Eq,
                RuntimeScalarOperator::Lt,
                RuntimeScalarOperator::Gte,
            ] {
                let b = builder(&s);
                let p = b.compare(
                    &field(&s, "at"),
                    op,
                    RuntimeValue::DateTime(RuntimeDateTime::parse(raw)?),
                )?;
                checked.push(b.finish(p)?);
            }
        }
        {
            let b = builder(&s);
            let p = b.between(
                &field(&s, "at"),
                RuntimeValue::DateTime(RuntimeDateTime::parse(dates[1])?),
                RuntimeValue::DateTime(RuntimeDateTime::parse(dates[4])?),
            )?;
            checked.push(b.finish(p)?);
        }
        for reverse in [false, true] {
            let b = builder(&s);
            let p = b.between(
                &field(&s, "number"),
                RuntimeValue::Integer(if reverse { 1 } else { -1 }),
                RuntimeValue::Integer(if reverse { -1 } else { 1 }),
            )?;
            let q = b.compare(
                &field(&s, "text"),
                RuntimeScalarOperator::Eq,
                RuntimeValue::String("a".into()),
            )?;
            let q = b.not(q)?;
            let p = b.or(vec![p, q])?;
            let p = b.not(p)?;
            checked.push(b.finish(p)?);
        }
        eprintln!(
            "OWNED_FILTER_PARITY cases={} rows={}",
            checked.len(),
            all.len()
        );
        for c in checked {
            let matcher = c.prepare_match(profile)?;
            let mut actual = read(&db, &s, c.predicate().clone())
                .await
                .into_iter()
                .map(|r| r.integer(&field(&s, "id")).unwrap())
                .collect::<Vec<_>>();
            let mut expected = Vec::new();
            for r in &all {
                if matcher.matches(r)? {
                    expected.push(r.integer(&field(&s, "id"))?)
                }
            }
            actual.sort();
            expected.sort();
            assert_eq!(actual, expected);
        }
        #[cfg(feature = "sqlite")]
        {
            // Outside the asserted profile: the row decoder erases noncanonical physical
            // spelling/classes. The matcher cannot detect this from a logical record.
            sqlx::query("INSERT INTO checked_filter_samples(id,flag,uid) VALUES (999,7,?)")
                .bind(uid.to_string().to_uppercase())
                .execute(db.pool())
                .await?;
            let b = builder(&s);
            let p = b.compare(
                &field(&s, "id"),
                RuntimeScalarOperator::Eq,
                RuntimeValue::Integer(999),
            )?;
            let p = b.finish(p)?;
            let row = read(&db, &s, p.predicate().clone()).await.remove(0);
            for (name, value) in [
                ("flag", RuntimeValue::Boolean(true)),
                ("uid", RuntimeValue::Uuid(uid)),
            ] {
                let b = builder(&s);
                let p = b.compare(&field(&s, name), RuntimeScalarOperator::Eq, value)?;
                let p = b.finish(p)?;
                assert!(p.prepare_match(profile)?.matches(&row)?);
                let b = builder(&s);
                let only = b.compare(
                    &field(&s, "id"),
                    RuntimeScalarOperator::Eq,
                    RuntimeValue::Integer(999),
                )?;
                let filter = b.and(vec![
                    only,
                    b.compare(
                        &field(&s, name),
                        RuntimeScalarOperator::Eq,
                        if name == "flag" {
                            RuntimeValue::Boolean(true)
                        } else {
                            RuntimeValue::Uuid(uid)
                        },
                    )?,
                ])?;
                let filter = b.finish(filter)?;
                assert!(read(&db, &s, filter.predicate().clone()).await.is_empty());
            }
            eprintln!(
                "OUTSIDE_PROFILE_STORAGE_COUNTEREXAMPLE confirmed; no detection/provenance claim"
            );
        }
        db.pool().close().await;
        #[cfg(feature = "postgres")]
        owned.cleanup()?;
        eprintln!("OWNED_FILTER_PARITY CLEANUP COMPLETE");
        Ok(())
    }
}

#[cfg(feature = "runtime-graphql")]
#[test]
fn checked_public_null_operators_validate_capabilities_before_artifact_creation() {
    use graphql_orm::{async_graphql::value, graphql::runtime::*};
    let mut raw = schema().into_schema();
    raw.collections[0].fields[1].filterable = false;
    let s = raw.validate().unwrap();
    assert_eq!(
        prepare_runtime_filter(
            &s,
            &collection(&s),
            &value!({"text":{"eq":null}}),
            Default::default()
        )
        .unwrap_err()
        .code(),
        RuntimeFilterErrorCode::InvalidInput
    );
    for value in [
        value!({"flag":{"lt":null}}),
        value!({"bytes":{"contains":null}}),
    ] {
        assert_eq!(
            prepare_runtime_filter(&s, &collection(&s), &value, Default::default())
                .unwrap_err()
                .code(),
            RuntimeFilterErrorCode::UnsupportedSemantics
        );
    }
    let json = prepare_runtime_filter(
        &s,
        &collection(&s),
        &value!({"json":{"isNull":true}}),
        Default::default(),
    )
    .unwrap();
    assert_eq!(
        json.prepare_match(RuntimeMatchProfile::SqliteBinaryCanonical)
            .unwrap_err()
            .code(),
        RuntimeFilterErrorCode::UnsupportedSemantics
    );
}

#[test]
fn unsupported_json_literals_are_not_reported_as_resource_exhaustion() {
    let s = schema();
    let b = builder(&s);
    assert_eq!(
        b.compare(
            &field(&s, "json"),
            RuntimeScalarOperator::Eq,
            RuntimeValue::Json(serde_json::json!({"private":"never display"}))
        )
        .unwrap_err()
        .code(),
        RuntimeFilterErrorCode::UnsupportedSemantics
    );
}
