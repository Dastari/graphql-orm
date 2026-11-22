use super::owned_mssql::{OwnedSqlServer, execute_batch};
use graphql_orm::db::mssql::{MssqlPool, MssqlPoolOptions};
use graphql_orm::graphql::orm::{DatabaseFilter, SqlValue};
use graphql_orm::prelude::*;
use std::time::Duration;

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "dbo.ParameterItems",
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
async fn declared_parameter_types_preserve_values_and_allow_a_varchar_index_seek() {
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
                (10005, 'cafe', N'unaccented');
             CREATE UNIQUE INDEX IX_ParameterItems_code ON dbo.ParameterItems(code);",
        ),
    )
    .await
    .unwrap()
    .unwrap();

    for (typed, access_path, identifier) in [
        (false, "Index Scan", "legacy"),
        (true, "Index Seek", "typed"),
    ] {
        let options = MssqlPoolOptions::new()
            .max_connections(1)
            .query_timeout(Some(Duration::from_secs(10)))
            .typed_parameters(typed);
        let pool = MssqlPool::connect_ado_with(connection, options)
            .await
            .unwrap();

        let (conditions, binds) = code_filter("item-42").to_sql_conditions();
        let marker = format!("/* {identifier}_parameter_plan */");
        let sql = format!(
            "SELECT id FROM dbo.ParameterItems WHERE {} {marker}",
            conditions.join(" AND ")
        );
        let rows = pool.fetch_rows(&sql, &binds).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].try_get::<i32, _>("id").unwrap(), 42);
        let plan = lookup_plan(&pool, &format!("%{marker}%")).await;
        assert!(
            plan.contains(&format!("PhysicalOp=\"{access_path}\"")),
            "{plan}"
        );
        assert!(
            plan.contains("Index=\"[IX_ParameterItems_code]\""),
            "{plan}"
        );
        eprintln!("{identifier}: {access_path} on IX_ParameterItems_code");

        let db = Database::<MssqlBackend>::new(pool);
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
            label: "overflow".into(),
            score: None,
            attempts: Some(u64::MAX),
        },
    )
    .await
    .err()
    .expect("out-of-range insert");
    assert_eq!(
        graphql_orm::graphql::errors::OrmPublicError::from(insert_error).code,
        graphql_orm::graphql::errors::OrmErrorCode::InvalidInput
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
        graphql_orm::graphql::errors::OrmPublicError::from(update_error).code,
        graphql_orm::graphql::errors::OrmErrorCode::InvalidInput
    );
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

async fn lookup_plan(pool: &MssqlPool, marker: &str) -> String {
    let rows = pool
        .fetch_rows(
            "SELECT TOP (1) CONVERT(nvarchar(max), p.query_plan) AS plan_xml
         FROM sys.dm_exec_query_stats AS s
         CROSS APPLY sys.dm_exec_sql_text(s.sql_handle) AS t
         CROSS APPLY sys.dm_exec_query_plan(s.plan_handle) AS p
         WHERE t.text LIKE @P1
         ORDER BY s.last_execution_time DESC",
            &[SqlValue::String(marker.into())],
        )
        .await
        .unwrap();
    rows[0].try_get("plan_xml").unwrap()
}
