use super::owned_mssql::{OwnedSqlServer, execute_batch};
use graphql_orm::db::mssql::{MssqlPool, MssqlPoolOptions};
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::graphql::orm::{BoundedMutationOutcome, DatabaseFilter, MutationLimit, SqlValue};
use graphql_orm::prelude::*;
use std::time::Duration;

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "dbo.ParameterItems",
    upsert = "id",
    schema_policy = "external_writable"
)]
struct ParameterItem {
    #[primary_key]
    #[graphql_orm(mssql_type = "INT", auto_generated = false)]
    #[filterable(type = "number")]
    id: i32,
    #[graphql_orm(mssql_type = "VARCHAR(32)")]
    #[filterable(type = "string")]
    code: String,
    #[graphql_orm(mssql_type = "VARCHAR(32)")]
    #[filterable(type = "string")]
    windows_code: Option<String>,
    #[graphql_orm(mssql_type = "VARCHAR(32)")]
    #[filterable(type = "string")]
    greek_code: Option<String>,
    #[graphql_orm(mssql_type = "NVARCHAR(40)")]
    #[filterable(type = "string")]
    label: String,
    #[graphql_orm(mssql_type = "SMALLINT")]
    score: Option<i32>,
    #[graphql_orm(mssql_type = "SMALLINT")]
    attempts: Option<u64>,
}

#[tokio::test]
#[ignore = "owns a capped disposable SQL Server container"]
async fn declared_parameter_types_preserve_comparison_and_write_semantics() {
    let server = tokio::time::timeout(Duration::from_secs(120), OwnedSqlServer::start())
        .await
        .unwrap()
        .unwrap();
    let connection = &server.connection_string;
    tokio::time::timeout(
        Duration::from_secs(30),
        execute_batch(
            connection,
            "CREATE TABLE dbo.ParameterItems (
                id int NOT NULL PRIMARY KEY,
                code varchar(32) COLLATE SQL_Latin1_General_CP1_CI_AS NOT NULL,
                windows_code varchar(32) COLLATE Latin1_General_100_CI_AS_SC NULL,
                greek_code varchar(32) COLLATE Greek_100_CI_AS NULL,
                label nvarchar(40) NOT NULL,
                score smallint NULL,
                attempts smallint NULL
             );
             WITH digits(n) AS (
                SELECT n FROM (VALUES (0),(1),(2),(3),(4),(5),(6),(7),(8),(9)) d(n)
             )
             INSERT dbo.ParameterItems (id, code, label)
             SELECT n, CONCAT('item-', n), N'東京 😀'
             FROM (SELECT a.n + 10*b.n + 100*c.n + 1000*d.n AS n
                   FROM digits a CROSS JOIN digits b CROSS JOIN digits c CROSS JOIN digits d) nums;
             INSERT dbo.ParameterItems (id, code, label) VALUES (10000, N'café', N'東京 😀');
             INSERT dbo.ParameterItems (id, code, label) VALUES (10001, REPLICATE('y', 32), N'東京 😀');
             INSERT dbo.ParameterItems (id, code, label) VALUES
                (10003, '??', N'unrepresentable'), (10004, 'abc ', N'trailing blank'),
                (10005, 'cafe', N'unaccented'), (10007, 'missing', N'null code');
             UPDATE dbo.ParameterItems SET windows_code = code;
             UPDATE dbo.ParameterItems SET greek_code = N'καφές' WHERE id = 10000;
             CREATE UNIQUE INDEX IX_ParameterItems_code ON dbo.ParameterItems(code);
             CREATE INDEX IX_ParameterItems_windows_code ON dbo.ParameterItems(windows_code);",
        ),
    )
    .await
    .unwrap()
    .unwrap();

    for (typed, identifier) in [(false, "legacy"), (true, "typed")] {
        let options = MssqlPoolOptions::new()
            .max_connections(1)
            .query_timeout(Some(Duration::from_secs(10)))
            .typed_parameters(typed);
        let pool = MssqlPool::connect_ado_with(connection, options)
            .await
            .unwrap();

        let (conditions, binds) = code_filter("item-42").to_sql_conditions();
        let marker = format!("{identifier}_parameter_plan");
        let sql = format!(
            "SELECT id, 1 AS [{marker}] FROM dbo.ParameterItems WHERE {}",
            conditions.join(" AND ")
        );
        let rows = pool.fetch_rows(&sql, &binds).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].try_get::<i32, _>("id").unwrap(), 42);
        let (plan, logical_reads) = lookup_plan(&pool, &marker).await;
        assert!(plan.contains("PhysicalOp=\"Index Scan\""), "{plan}");
        eprintln!("{identifier} IX_ParameterItems_code scan, {logical_reads} logical reads");
        assert!(
            plan.contains("Index=\"[IX_ParameterItems_code]\""),
            "{plan}"
        );
        let (conditions, binds) = ParameterItemWhereInput {
            windows_code: Some(StringFilter {
                eq: Some("CAFÉ".into()),
                ..Default::default()
            }),
            ..Default::default()
        }
        .to_sql_conditions();
        let marker = format!("{identifier}_windows_parameter_plan");
        let rows = pool
            .fetch_rows(
                &format!(
                    "SELECT id, 1 AS [{marker}] FROM dbo.ParameterItems WHERE {}",
                    conditions.join(" AND ")
                ),
                &binds,
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].try_get::<i32, _>("id").unwrap(), 10000);
        let (plan, logical_reads) = lookup_plan(&pool, &marker).await;
        assert_index_seek_with_unicode(&plan, "IX_ParameterItems_windows_code");
        eprintln!(
            "{identifier} IX_ParameterItems_windows_code seek, {logical_reads} logical reads"
        );

        assert_sql_collation_rows(&pool).await;

        let db = Database::<MssqlBackend>::new(pool);
        let matches = ParameterItem::query(&db)
            .filter(ParameterItemWhereInput {
                greek_code: Some(StringFilter {
                    eq: Some("ΚΑΦΈΣ".into()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .fetch_all()
            .await
            .unwrap();
        assert_eq!(
            matches.iter().map(|row| row.id).collect::<Vec<_>>(),
            [10000]
        );
        let (generated, _) = lookup_plan(db.pool(), "[greek_code] = @P1").await;
        assert_unicode_plan(&generated);

        let matches = ParameterItem::query(&db)
            .filter(ParameterItemWhereInput {
                id: Some(IntFilter {
                    eq: Some(10000),
                    ..Default::default()
                }),
                label: Some(StringFilter {
                    eq: Some("東京 😀".into()),
                    ..Default::default()
                }),
                ..code_filter("café")
            })
            .fetch_all()
            .await
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].code, "café");
        assert_eq!(matches[0].label, "東京 😀");
        assert!(
            ParameterItem::query(&db)
                .filter(code_filter(&"y".repeat(33)))
                .fetch_all()
                .await
                .unwrap()
                .is_empty()
        );

        assert_unicode_predicates(&db).await;

        for (sql_type, input, parameter_type) in [
            ("INT", SqlValue::Int(42), "int"),
            ("SMALLINT", SqlValue::Int(-12), "smallint"),
            ("TINYINT", SqlValue::Int(255), "tinyint"),
            ("TINYINT", SqlValue::Int(256), "bigint"),
            ("BIGINT", SqlValue::Int(i64::MAX), "bigint"),
        ] {
            let rows = db.pool().fetch_rows(
                "SELECT CONVERT(nvarchar(40), SQL_VARIANT_PROPERTY(@P1, 'BaseType')) AS bind_type,
                        CONVERT(bigint, @P1) AS value",
                &[input.clone().with_mssql_type(sql_type)],
            ).await.unwrap();
            assert_eq!(
                rows[0].try_get::<String, _>("bind_type").unwrap(),
                if typed { parameter_type } else { "bigint" }
            );
            assert_eq!(SqlValue::Int(rows[0].try_get("value").unwrap()), input);
        }
    }
    let writable = MssqlPool::connect_ado_external_writable_with(
        connection,
        MssqlPoolOptions::new()
            .max_connections(1)
            .query_timeout(Some(Duration::from_secs(10)))
            .typed_parameters(true),
    )
    .await
    .unwrap();
    let db = Database::<MssqlBackend>::builder(writable)
        .schema_policy(SchemaPolicy::ExternalWritable)
        .build();
    let (_, comparison_binds) = code_filter("item-42").to_sql_conditions();
    let bind_type = db
        .transaction(TransactionMode::Default, |context| {
            Box::pin(async move {
                let rows = context
                    .fetch_rows(
                        "SELECT CONVERT(nvarchar(40), SQL_VARIANT_PROPERTY(@P1, 'BaseType')) AS bind_type",
                        &comparison_binds,
                    )
                    .await?;
                rows[0]
                    .try_get::<String, _>("bind_type")
                    .map_err(Into::into)
            })
        })
        .await
        .unwrap();
    assert_eq!(bind_type, "nvarchar");

    let inserted = ParameterItem::insert(
        &db,
        CreateParameterItemInput {
            id: 10002,
            code: "crème".into(),
            windows_code: None,
            greek_code: None,
            label: "雪 😀".into(),
            score: Some(-12),
            attempts: Some(3),
        },
    )
    .await
    .unwrap();
    assert_eq!(inserted.code, "crème");
    assert_eq!(inserted.label, "雪 😀");
    assert_eq!(inserted.score, Some(-12));
    let updated = ParameterItem::update_by_id(
        &db,
        &inserted.id,
        UpdateParameterItemInput {
            code: Some("naïve".into()),
            score: Some(None),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(updated.code, "naïve");
    assert_eq!(updated.score, None);
    let insert_error = ParameterItem::insert(
        &db,
        CreateParameterItemInput {
            id: 10006,
            code: "overflow".into(),
            windows_code: None,
            greek_code: None,
            label: "overflow".into(),
            score: None,
            attempts: Some(u64::MAX),
        },
    )
    .await
    .err()
    .expect("out-of-range insert");
    assert_eq!(
        OrmPublicError::from(insert_error).code,
        OrmErrorCode::InvalidInput
    );
    let update_error = ParameterItem::update_by_id(
        &db,
        &inserted.id,
        UpdateParameterItemInput {
            attempts: Some(Some(u64::MAX)),
            ..Default::default()
        },
    )
    .await
    .err()
    .expect("out-of-range update");
    assert_eq!(
        OrmPublicError::from(update_error).code,
        OrmErrorCode::InvalidInput
    );
    let mut denied_db = Database::<MssqlBackend>::builder(db.pool().clone())
        .schema_policy(SchemaPolicy::ExternalWritable)
        .build();
    denied_db.set_row_policy(CodePolicy("naïve"));
    let upsert_error = ParameterItem::upsert(
        &denied_db,
        CreateParameterItemInput {
            id: inserted.id,
            code: updated.code.clone(),
            windows_code: None,
            greek_code: None,
            label: updated.label.clone(),
            score: None,
            attempts: Some(32768),
        },
    )
    .await
    .err()
    .expect("out-of-range upsert");
    // If the existing row reaches the row policy, it refuses the write.
    assert_eq!(
        OrmPublicError::from(upsert_error).code,
        OrmErrorCode::InvalidInput
    );
    let no_target = code_filter("does-not-exist");
    for (attempts, should_error) in [(32767, false), (32768, true), (u64::MAX, true)] {
        let input = UpdateParameterItemInput {
            attempts: Some(Some(attempts)),
            ..Default::default()
        };
        let by_id = ParameterItem::update_by_id(&db, &10006, input.clone())
            .await
            .map_err(OrmPublicError::from);
        let transactional_by_id = db
            .transaction(TransactionMode::StateMachine, |context| {
                let input = input.clone();
                Box::pin(async move {
                    context
                        .update_by_id::<ParameterItem>(&10006, input)
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .await
            .map_err(|error| error.public_error().clone());
        for result in [by_id, transactional_by_id] {
            if should_error {
                assert_eq!(result.err().unwrap().code, OrmErrorCode::InvalidInput);
            } else {
                assert!(result.unwrap().is_none());
            }
        }
        let standalone = ParameterItem::update_where(&db, no_target.clone(), input.clone())
            .await
            .map_err(OrmPublicError::from);
        let in_transaction = db
            .transaction(TransactionMode::StateMachine, |context| {
                let where_input = no_target.clone();
                Box::pin(async move {
                    context
                        .update_where::<ParameterItem>(where_input, input)
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .await
            .map_err(|error| error.public_error().clone());
        let bounded = ParameterItem::update_where_bounded(
            &db,
            no_target.clone(),
            UpdateParameterItemInput {
                attempts: Some(Some(attempts)),
                ..Default::default()
            },
            MutationLimit::new(1).unwrap(),
        )
        .await;
        if should_error {
            assert_eq!(
                OrmPublicError::from(bounded.unwrap_err()).code,
                OrmErrorCode::InvalidInput
            );
        } else {
            assert!(matches!(
                bounded.unwrap(),
                BoundedMutationOutcome::Applied { affected: 0 }
            ));
        }
        for result in [standalone, in_transaction] {
            if should_error {
                assert_eq!(result.unwrap_err().code, OrmErrorCode::InvalidInput);
            } else {
                assert_eq!(result.unwrap(), 0);
            }
        }
    }
    let rows = db
        .pool()
        .fetch_rows(
            "SELECT attempts FROM dbo.ParameterItems WHERE id = 10002 OR id = 10006",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].try_get::<i16, _>("attempts").unwrap(), 3);
}

struct CodePolicy(&'static str);

impl RowPolicy<MssqlBackend> for CodePolicy {
    fn read_visibility<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<MssqlBackend>,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async {
            ReadPredicate::from_filter::<MssqlBackend, _>(&code_filter(self.0))
                .map(ReadVisibility::Complete)
        })
    }

    fn can_read_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<MssqlBackend>,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: EntityAccessSurface,
        _row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }

    fn can_write_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<MssqlBackend>,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: EntityAccessSurface,
        _row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
}

async fn assert_unicode_predicates(db: &Database<MssqlBackend>) {
    assert!(
        ParameterItem::query(db)
            .filter(code_filter("東京"))
            .fetch_all()
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        ParameterItem::query(db)
            .filter(ParameterItemWhereInput {
                code: Some(StringFilter {
                    ends_with: Some("abc".into()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .fetch_all()
            .await
            .unwrap()
            .is_empty()
    );
    for (value, expected) in [("café", vec![10000]), ("東京", vec![])] {
        let mut db = db.clone();
        db.set_row_policy(CodePolicy(value));
        let rows = ParameterItem::query(&db).fetch_all().await.unwrap();
        assert_eq!(
            rows.into_iter().map(|row| row.id).collect::<Vec<_>>(),
            expected
        );
    }
}

fn code_filter(code: &str) -> ParameterItemWhereInput {
    ParameterItemWhereInput {
        code: Some(StringFilter {
            eq: Some(code.into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

async fn lookup_plan(pool: &MssqlPool, marker: &str) -> (String, i64) {
    let rows = pool
        .fetch_rows(
            "SELECT TOP (1) CONVERT(nvarchar(max), p.query_plan) AS plan_xml, s.last_logical_reads
         FROM sys.dm_exec_query_stats AS s
         CROSS APPLY sys.dm_exec_sql_text(s.sql_handle) AS t
         CROSS APPLY sys.dm_exec_query_plan(s.plan_handle) AS p
         WHERE CHARINDEX(@P1, SUBSTRING(
             t.text, (s.statement_start_offset / 2) + 1,
             (CASE s.statement_end_offset WHEN -1 THEN DATALENGTH(t.text)
                  ELSE s.statement_end_offset END - s.statement_start_offset) / 2 + 1
         )) > 0
         ORDER BY s.last_execution_time DESC",
            &[SqlValue::String(marker.into())],
        )
        .await
        .unwrap();
    let row = rows
        .first()
        .unwrap_or_else(|| panic!("no cached plan containing {marker}"));
    (
        row.try_get("plan_xml").unwrap(),
        row.try_get("last_logical_reads").unwrap(),
    )
}

fn assert_unicode_plan(plan: &str) {
    assert!(plan.contains("ParameterDataType=\"nvarchar("), "{plan}");
    assert!(
        !plan.to_ascii_lowercase().contains("convert(varchar("),
        "{plan}"
    );
}

fn assert_index_seek_with_unicode(plan: &str, index: &str) {
    assert_unicode_plan(plan);
    let index_name = format!("Index=\"[{index}]\"");
    assert!(
        plan.split("<RelOp ").any(|operator| {
            operator.split_once('>').is_some_and(|(header, body)| {
                header.contains("PhysicalOp=\"Index Seek\"") && body.contains(&index_name)
            })
        }),
        "{plan}"
    );
}

async fn assert_sql_collation_rows(pool: &MssqlPool) {
    for (filter, expected) in [
        (
            StringFilter {
                in_list: Some(vec!["café".into(), "東京".into(), "abc".into()]),
                ..Default::default()
            },
            vec![10000, 10004],
        ),
        (
            StringFilter {
                not_in: Some(vec!["café".into(), "東京".into(), "abc".into()]),
                ..Default::default()
            },
            vec![10001, 10003, 10005, 10007],
        ),
        (
            StringFilter {
                ne: Some("東京".into()),
                ..Default::default()
            },
            vec![10000, 10001, 10003, 10004, 10005, 10007],
        ),
    ] {
        let (conditions, values) = ParameterItemWhereInput {
            code: Some(filter),
            ..Default::default()
        }
        .to_sql_conditions();
        let rows = pool
            .fetch_rows(
                &format!(
                    "SELECT id FROM dbo.ParameterItems WHERE id >= 10000 AND {} ORDER BY id",
                    conditions.join(" AND "),
                ),
                &values,
            )
            .await
            .unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| row.try_get::<i32, _>("id").unwrap())
                .collect::<Vec<_>>(),
            expected,
        );
    }
}
