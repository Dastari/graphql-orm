#![cfg(all(any(feature = "sqlite", feature = "postgres"), not(feature = "mssql")))]
//! Executable runtime string contract on databases owned exclusively by this test.

use graphql_orm::prelude::*;
#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
type Backend = PostgresBackend;

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(
        backend = "sqlite",
        table = "runtime_string_samples",
        plural = "StringSamples"
    )
)]
#[cfg_attr(
    all(feature = "postgres", not(feature = "sqlite")),
    repository_entity(
        backend = "postgres",
        table = "runtime_string_samples",
        plural = "StringSamples"
    )
)]
struct StringSample {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[filterable(type = "string")]
    text: Option<String>,
    #[filterable(type = "string")]
    tenant: String,
}

fn field(name: &str, kind: RuntimeValueKind, nullable: bool) -> RuntimeField {
    RuntimeField {
        id: FieldId::new(name).unwrap(),
        api_name: name.into(),
        physical_column: name.into(),
        value_kind: kind,
        nullable,
        unique: name == "id",
        filterable: true,
        sortable: true,
        generated: false,
        default: None,
    }
}
fn schema() -> ValidatedRuntimeSchema {
    RuntimeSchema {
        format_version: 1,
        collections: vec![RuntimeCollection {
            id: CollectionId::new("samples").unwrap(),
            api_type_name: "StringSample".into(),
            api_plural_name: "StringSamples".into(),
            physical_table: "runtime_string_samples".into(),
            primary_key: vec![FieldId::new("id").unwrap()],
            append_only: false,
            retention_purge: false,
            fields: vec![
                field("id", RuntimeValueKind::String, false),
                field("text", RuntimeValueKind::String, true),
                field("tenant", RuntimeValueKind::String, false),
            ],
            relations: vec![],
            indexes: vec![],
            composite_unique: vec![],
            default_order: vec![],
        }],
    }
    .validate()
    .unwrap()
}
struct Fixture {
    db: Database<Backend>,
    schema: ValidatedRuntimeSchema,
    rows: Vec<Option<&'static str>>,
    #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
    postgres: owned_postgres::OwnedPostgres,
}
impl Fixture {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        for variable in [
            "DATABASE_URL",
            "TEST_DATABASE_URL",
            "MSSQL_TEST_DATABASE_URL",
        ] {
            assert!(
                std::env::var_os(variable).is_none(),
                "refusing ambient database configuration"
            );
        }
        #[cfg(feature = "sqlite")]
        let db = Database::new(
            sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await?,
        );
        #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
        let postgres = owned_postgres::OwnedPostgres::start("runtime-strings")?;
        #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
        let db = Database::new(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .connect(&postgres.url)
                .await?,
        );
        let plan = db
            .schema()
            .plan_migration_to_entities(
                "fixture",
                "test-owned strings",
                &[StringSample::metadata()],
            )
            .await?;
        db.schema()
            .apply_migration(&plan, Default::default())
            .await?;
        let rows = vec![
            Some("alpha"),
            Some("ALPHA"),
            Some("πfoo"),
            Some("πfoo%"),
            Some("suffix"),
            Some("Ω"),
            Some(""),
        ];
        for (index, text) in rows.iter().enumerate() {
            StringSample::insert(
                &db,
                CreateStringSampleInput {
                    id: format!("{:03}", index + 1),
                    text: text.map(str::to_owned),
                    tenant: "visible".into(),
                },
            )
            .await?;
        }
        Ok(Self {
            db,
            schema: schema(),
            rows,
            #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
            postgres,
        })
    }
    fn collection(&self) -> RuntimeCollectionHandle {
        self.schema
            .resolve_collection(&CollectionId::new("samples").unwrap())
            .unwrap()
    }
    fn field(&self, name: &str) -> RuntimeFieldHandle {
        self.schema
            .resolve_field(&self.collection(), &FieldId::new(name).unwrap())
            .unwrap()
    }
    fn compare(
        &self,
        name: &str,
        operator: RuntimeScalarOperator,
        value: &str,
    ) -> RuntimePredicate {
        self.schema
            .runtime_compare(
                &self.collection(),
                &self.field(name),
                operator,
                RuntimeValue::String(value.into()),
                RuntimeQueryLimits::default(),
            )
            .unwrap()
    }
    async fn read(
        &self,
        predicate: RuntimePredicate,
        page: RuntimePageRequest,
        auth: Option<&DbAuthContext>,
    ) -> RuntimeConnection {
        let collection = self.collection();
        let projection = self
            .schema
            .resolve_projection(&collection, &[self.field("id"), self.field("text")])
            .unwrap();
        let limits = RuntimeQueryLimits::default();
        let order = self
            .schema
            .runtime_order(
                &collection,
                Some(vec![RuntimeOrderInput {
                    field: self.field("text"),
                    direction: RuntimeOrderDirection::Asc,
                    nulls: RuntimeNullPlacement::Last,
                }]),
                limits,
            )
            .unwrap();
        let request = self
            .schema
            .runtime_read_request(
                &collection,
                &projection,
                Some(predicate),
                order,
                page,
                true,
                limits,
            )
            .unwrap();
        self.db.execute_runtime_read(&request, auth).await.unwrap()
    }
    fn ids(&self, connection: &RuntimeConnection) -> Vec<i64> {
        connection
            .edges
            .iter()
            .map(|edge| {
                edge.node
                    .string(&self.field("id"))
                    .unwrap()
                    .parse()
                    .unwrap()
            })
            .collect()
    }
    async fn finish(self) -> Result<(), Box<dyn std::error::Error>> {
        // Verify the connection's identity before closing it; no external file is deleted.
        #[cfg(feature = "sqlite")]
        {
            let (_, _, path): (i64, String, String) = sqlx::query_as("PRAGMA database_list")
                .fetch_one(self.db.pool())
                .await?;
            assert!(
                path.is_empty(),
                "expected an exclusively owned in-memory database"
            );
        }
        #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
        {
            let (database, user): (String, String) =
                sqlx::query_as("SELECT current_database(), current_user")
                    .fetch_one(self.db.pool())
                    .await?;
            assert_eq!(
                Some(database.as_str()),
                self.postgres.url.rsplit('/').next()
            );
            assert_eq!(user, "graphql_orm_owner");
        }
        self.db.pool().close().await;
        #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
        {
            let mut postgres = self.postgres;
            postgres.cleanup()?;
        }
        Ok(())
    }
}

#[tokio::test]
#[cfg_attr(
    all(feature = "postgres", not(feature = "sqlite")),
    ignore = "owns a disposable PostgreSQL Docker container; run explicitly"
)]
async fn runtime_string_predicates_execute_and_paginate() -> Result<(), Box<dyn std::error::Error>>
{
    let mut fixture = Fixture::start().await?;
    use RuntimeScalarOperator as O;
    // The seven-row consumer reproduction, without policy lowering.
    for (operator, operand, count) in [
        (O::StartsWith, "π", 2),
        (O::EndsWith, "foo", 1),
        (O::EndsWith, "", 7),
        (O::StartsWith, "", 7),
    ] {
        let result = fixture
            .read(
                fixture.compare("text", operator, operand),
                RuntimePageRequest::first(100, None),
                None,
            )
            .await;
        assert_eq!(result.total_count, Some(count));
        assert_eq!(result.edges.len() as i64, count);
    }
    for text in [
        None,
        Some("%_\\' $1 ? @P2"),
        Some("πfoo"),
        Some("\\end"),
        Some("a_b"),
        Some("ab%"),
        Some("line\nπ"),
        Some("🦀π"),
    ] {
        StringSample::insert(
            &fixture.db,
            CreateStringSampleInput {
                id: format!("{:03}", fixture.rows.len() + 1),
                text: text.map(str::to_owned),
                tenant: "visible".into(),
            },
        )
        .await?;
        fixture.rows.push(text);
    }
    // Literal wildcards, Unicode, case, control characters, longer-than-value and null behavior.
    for operator in [O::StartsWith, O::EndsWith, O::Contains] {
        for operand in [
            "",
            "a",
            "A",
            "π",
            "foo",
            "%",
            "_",
            "\\",
            "' $1 ? @P2",
            "🦀",
            "\nπ",
            "longer-than-any-fixture-string-without-a-match",
        ] {
            let result = fixture
                .read(
                    fixture.compare("text", operator, operand),
                    RuntimePageRequest::first(100, None),
                    None,
                )
                .await;
            let mut actual = fixture.ids(&result);
            actual.sort();
            let expected: Vec<i64> = fixture
                .rows
                .iter()
                .enumerate()
                .filter_map(|(index, value)| {
                    value
                        .filter(|text| match operator {
                            O::StartsWith => text.starts_with(operand),
                            O::EndsWith => text.ends_with(operand),
                            O::Contains => text.contains(operand),
                            _ => unreachable!(),
                        })
                        .map(|_| index as i64 + 1)
                })
                .collect();
            assert_eq!(
                actual, expected,
                "literal string predicate differs from its evaluator"
            );
            assert_eq!(result.total_count, Some(expected.len() as i64));
        }
    }
    let limits = RuntimeQueryLimits::default();
    let collection = fixture.collection();
    let starts = fixture.compare("text", O::StartsWith, "π");
    let ends = fixture.compare("text", O::EndsWith, "foo");
    let either = fixture.schema.runtime_or(
        &collection,
        vec![starts.clone(), fixture.compare("text", O::StartsWith, "a")],
        limits,
    )?;
    let not_ends = fixture
        .schema
        .runtime_not(&collection, ends.clone(), limits)?;
    let composed = fixture
        .schema
        .runtime_and(&collection, vec![either, not_ends], limits)?;
    let result = fixture
        .read(composed, RuntimePageRequest::first(100, None), None)
        .await;
    let mut ids = fixture.ids(&result);
    ids.sort();
    assert_eq!(ids, vec![1, 4, 12, 13]);
    let not_empty_suffix = fixture.schema.runtime_not(
        &collection,
        fixture.compare("text", O::EndsWith, ""),
        limits,
    )?;
    assert!(
        fixture
            .read(not_empty_suffix, RuntimePageRequest::first(100, None), None)
            .await
            .edges
            .is_empty(),
        "NOT must preserve SQL null's unknown state"
    );
    let nullable = fixture.schema.runtime_or(
        &collection,
        vec![
            ends,
            fixture
                .schema
                .runtime_is_null(&collection, &fixture.field("text"), true, limits)?,
        ],
        limits,
    )?;
    assert_eq!(
        fixture
            .read(nullable, RuntimePageRequest::first(100, None), None)
            .await
            .total_count,
        Some(3)
    );

    // Independently bound authorization predicate applies before count/continuation.
    StringSample::insert(
        &fixture.db,
        CreateStringSampleInput {
            id: "100".into(),
            text: Some("πfoo".into()),
            tenant: "denied".into(),
        },
    )
    .await?;
    let authorized = fixture.schema.runtime_and(
        &collection,
        vec![starts, fixture.compare("tenant", O::Eq, "visible")],
        limits,
    )?;
    let whole = fixture
        .read(
            authorized.clone(),
            RuntimePageRequest::first(100, None),
            None,
        )
        .await;
    let mut forward = vec![];
    let mut cursor = None;
    loop {
        let page = fixture
            .read(
                authorized.clone(),
                RuntimePageRequest::first(1, cursor),
                None,
            )
            .await;
        assert_eq!(page.total_count, whole.total_count);
        assert!(
            page.edges
                .iter()
                .all(|edge| edge.node.state(&fixture.field("tenant")).unwrap()
                    == RuntimeFieldState::Unloaded)
        );
        forward.extend(fixture.ids(&page));
        if !page.page_info.has_next_page {
            break;
        }
        cursor = page.page_info.end_cursor;
    }
    assert_eq!(forward, fixture.ids(&whole));
    let mut backward = vec![];
    let mut cursor = None;
    loop {
        let page = fixture
            .read(
                authorized.clone(),
                RuntimePageRequest::last(1, cursor),
                None,
            )
            .await;
        assert_eq!(page.total_count, whole.total_count);
        backward.extend(fixture.ids(&page));
        if !page.page_info.has_previous_page {
            break;
        }
        cursor = page.page_info.start_cursor;
    }
    backward.reverse();
    assert_eq!(backward, forward);
    // Malformed cursors fail before any pool access, with safe value-free errors.
    #[cfg(feature = "sqlite")]
    let closed_pool = sqlx::sqlite::SqlitePoolOptions::new().connect_lazy("sqlite::memory:")?;
    #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
    let closed_pool = sqlx::postgres::PgPoolOptions::new().connect_lazy(&fixture.postgres.url)?;
    closed_pool.close().await;
    let closed_db = Database::<Backend>::new(closed_pool);
    let projection = fixture
        .schema
        .resolve_projection(&collection, &[fixture.field("text")])?;
    let request = fixture.schema.runtime_read_request(
        &collection,
        &projection,
        Some(authorized),
        fixture.schema.runtime_order(&collection, None, limits)?,
        RuntimePageRequest::first(1, Some("caller-value-not-for-diagnostics".into())),
        true,
        limits,
    )?;
    let error = closed_db
        .execute_runtime_read(&request, None)
        .await
        .unwrap_err();
    assert_eq!(error.code(), RuntimeQueryErrorCode::CursorInvalid);
    assert!(!format!("{error:?} {error}").contains("caller-value-not-for-diagnostics"));
    // Multiple static generated clauses and nested native fragments retain their binding semantics.
    let static_rows = StringSample::query(&fixture.db)
        .filter(StringSampleWhereInput {
            tenant: Some(StringFilter {
                eq: Some("visible".into()),
                ..Default::default()
            }),
            or: Some(vec![
                StringSampleWhereInput {
                    text: Some(StringFilter {
                        eq: Some("alpha".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                StringSampleWhereInput {
                    text: Some(StringFilter {
                        eq: Some("πfoo".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        })
        .fetch_all()
        .await?;
    let mut ids: Vec<_> = static_rows
        .into_iter()
        .map(|row| row.id.parse::<i64>().unwrap())
        .collect();
    ids.sort();
    assert_eq!(ids, vec![1, 3, 10]);
    #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
    postgres_binding_and_rls(&fixture).await?;
    fixture.finish().await
}

#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
async fn postgres_binding_and_rls(fixture: &Fixture) -> Result<(), Box<dyn std::error::Error>> {
    // Audit every backend normalization entry point, including transaction DML.
    let statement = "SELECT $2::bigint AS later, $1::bigint AS first, $2::bigint AS again";
    let values = [SqlValue::Int(17), SqlValue::Int(29)];
    let rows = PostgresBackend::fetch_rows(fixture.db.pool(), statement, &values).await?;
    assert_eq!(PostgresBackend::try_get_i64(&rows[0], "later")?, 29);
    assert_eq!(PostgresBackend::try_get_i64(&rows[0], "first")?, 17);
    assert_eq!(PostgresBackend::try_get_i64(&rows[0], "again")?, 29);
    let mut transaction = fixture.db.pool().begin().await?;
    let rows = <PostgresBackend as SqlxBackend>::fetch_rows_on(
        transaction.as_mut(),
        statement.into(),
        values.to_vec(),
    )
    .await?;
    assert_eq!(PostgresBackend::try_get_i64(&rows[0], "again")?, 29);
    <PostgresBackend as SqlxBackend>::execute_with_binds_on(
        transaction.as_mut(),
        "UPDATE runtime_string_samples SET tenant = $1 WHERE tenant = $1".into(),
        vec![SqlValue::String("visible".into())],
    )
    .await?;
    transaction.rollback().await?;
    <PostgresBackend as SqlxBackend>::execute_with_binds(
        fixture.db.pool(),
        "UPDATE runtime_string_samples SET tenant = $1 WHERE tenant = $1",
        &[SqlValue::String("visible".into())],
    )
    .await?;

    for statement in [
        "CREATE ROLE string_reader NOLOGIN NOSUPERUSER NOBYPASSRLS",
        "GRANT SELECT ON runtime_string_samples TO string_reader",
        "ALTER TABLE runtime_string_samples ENABLE ROW LEVEL SECURITY",
        "CREATE POLICY string_partition ON runtime_string_samples FOR SELECT TO string_reader USING (tenant = current_setting('app.tenant_id', true))",
    ] {
        sqlx::query(statement).execute(fixture.db.pool()).await?;
    }
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE string_reader")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&fixture.postgres.url)
        .await?;
    let db = Database::<PostgresBackend>::new(pool);
    let user: String = sqlx::query_scalar("SELECT current_user")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(user, "string_reader");
    let visible = DbAuthContext::from_parts("reader-a", vec![], vec![], Some("visible".into()));
    let denied = DbAuthContext::from_parts("reader-b", vec![], vec![], Some("denied".into()));
    let absent = DbAuthContext::from_parts("reader-c", vec![], vec![], Some("absent".into()));
    let collection = fixture.collection();
    let projection = fixture
        .schema
        .resolve_projection(&collection, &[fixture.field("id")])?;
    let limits = RuntimeQueryLimits::default();
    let order = fixture.schema.runtime_order(&collection, None, limits)?;
    for count in [false, true] {
        for operator in [
            RuntimeScalarOperator::StartsWith,
            RuntimeScalarOperator::EndsWith,
        ] {
            let filter = fixture.compare("text", operator, "");
            let request = fixture.schema.runtime_read_request(
                &collection,
                &projection,
                Some(filter.clone()),
                order.clone(),
                RuntimePageRequest::first(2, None),
                count,
                limits,
            )?;
            let first = db.execute_runtime_read(&request, Some(&visible)).await?;
            assert_eq!(first.total_count, count.then_some(14));
            assert_eq!(fixture.ids(&first), vec![1, 2]);
            let next = fixture.schema.runtime_read_request(
                &collection,
                &projection,
                Some(filter.clone()),
                order.clone(),
                RuntimePageRequest::first(2, first.page_info.end_cursor),
                count,
                limits,
            )?;
            let next = db.execute_runtime_read(&next, Some(&visible)).await?;
            assert_eq!(fixture.ids(&next), vec![3, 4]);
            assert_eq!(next.total_count, count.then_some(14));
            let other = db.execute_runtime_read(&request, Some(&denied)).await?;
            assert_eq!(fixture.ids(&other), vec![100]);
            assert_eq!(other.total_count, count.then_some(1));
            assert!(
                db.execute_runtime_read(&request, Some(&absent))
                    .await?
                    .edges
                    .is_empty()
            );
            // The previous transaction's SET LOCAL auth context must not leak.
            assert!(
                db.execute_runtime_read(&request, None)
                    .await?
                    .edges
                    .is_empty()
            );
        }
    }
    db.pool().close().await;
    Ok(())
}

#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[test]
fn postgres_complete_statement_preserves_native_parameter_identity() {
    assert_eq!(
        PostgresBackend::normalize_sql("SELECT $1, $1", 1),
        "SELECT $1, $1"
    );
    assert_eq!(
        PostgresBackend::normalize_sql("SELECT $2, $1, $2", 3),
        "SELECT $4, $3, $4"
    );
    assert_eq!(
        PostgresBackend::normalize_sql("SELECT ?, ?", 4),
        "SELECT $4, $5"
    );
    // Mixed forms retain occurrence-based compatibility; they do not promise shared identity.
    assert_eq!(
        PostgresBackend::normalize_sql("SELECT $8, ?, @P2, $8", 3),
        "SELECT $3, $4, $5, $6"
    );
    assert_eq!(
        DatabaseBackend::Postgres.normalize_sql("value = $9", 2),
        "value = $2",
        "static fragments still rebase by occurrence"
    );
    let quoted = "SELECT $2, '$1?', \"$1?\", $$ $9 ? $$, $tag$ $4 ? $tag$, E'\\\'$8?', $2 -- $9 ?\n/* $7 /* $6 ? */ */";
    assert_eq!(PostgresBackend::normalize_sql(quoted, 1), quoted);
}
