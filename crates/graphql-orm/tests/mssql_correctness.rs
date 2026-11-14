//! SQL Server behaviour that only a server can confirm, run on the disposable
//! container these tests own. Rendered text is asserted in the unit tests and
//! `mssql_ddl_rendering.rs`; here the same statements execute.
#![cfg(feature = "mssql")]

#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;

use graphql_orm::db::mssql::{MssqlPool, MssqlServerError};
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::graphql::loaders::{
    CompositeRelationQueryKey, RelationKey, RelationKeyPartKind, RelationLoader,
    relation_key_projection,
};
use graphql_orm::graphql::orm::{
    ColumnModel, DatabaseBackend, DatabaseEntity, DeletePolicy, ForeignKeyModel, MigrationStep,
    PaginationRequest, SelectQuery, SortExpression, SqlDialect, SqlValue, TransactionError,
    TransactionMode, contains_like_pattern, introspect_mssql_schema, render_migration_step,
    render_select_query, render_upsert_sql,
};
use graphql_orm::prelude::*;
use graphql_orm::rust_decimal::Decimal;
use owned_mssql::{OwnedSqlServer, execute_batch};

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.GomBatchChildren",
    plural = "GomBatchChildren",
    default_sort = "[ChildId] ASC"
)]
pub struct GomBatchChild {
    #[primary_key]
    #[graphql_orm(db_column = "ChildId", write = false)]
    #[filterable(type = "number")]
    #[sortable]
    pub id: i64,

    #[graphql_orm(db_column = "PartA", write = false)]
    #[filterable(type = "number")]
    pub part_a: i64,

    #[graphql_orm(db_column = "PartB", write = false)]
    #[filterable(type = "number")]
    pub part_b: i64,

    #[graphql_orm(db_column = "PartC", write = false)]
    #[filterable(type = "number")]
    pub part_c: i64,
}

impl graphql_orm::graphql::loaders::BatchLoadEntity<MssqlBackend> for GomBatchChild {
    fn batch_column() -> &'static str {
        "[ChildId]"
    }

    fn batch_key_from_row(row: &graphql_orm::db::mssql::MssqlRow) -> Result<String, sqlx::Error> {
        row.try_get::<i64, _>("ChildId")
            .map(|value| value.to_string())
    }
}

schema_roots! {
    backend: "mssql",
    schema_policy: "external_read_only",
    query_custom_ops: [],
    entities: [GomBatchChild],
}

/// An explicit multi-column default sort written without whitespace (#430).
#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(backend = "mssql", table = "dbo.GomSorted", default_sort = "KeyA,KeyB")]
struct GomSorted {
    #[primary_key]
    #[graphql_orm(db_column = "KeyA")]
    key_a: String,
    #[primary_key]
    #[graphql_orm(db_column = "KeyB")]
    key_b: String,
}

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn server_error(error: &sqlx::Error) -> &MssqlServerError {
    error
        .as_database_error()
        .and_then(|error| error.try_downcast_ref::<MssqlServerError>())
        .unwrap_or_else(|| panic!("expected a SQL Server error, got {error:?}"))
}

#[tokio::test]
#[ignore = "requires test-owned SQL Server container"]
async fn sql_server_correctness_batch() -> TestResult {
    let mut server = OwnedSqlServer::start().await?;
    let connection = server.connection_string.clone();
    let pool = MssqlPool::connect_ado(&connection).await?;
    let writable = graphql_orm::db::Database::<MssqlBackend>::new(
        MssqlPool::connect_ado_external_writable(&connection).await?,
    );

    errors_keep_their_classification(&connection, &pool, &writable).await?;
    decimals_and_dates_decode(&pool).await?;
    empty_pages_and_bracket_patterns(&connection, &pool).await?;
    text_conversions_are_lossless(&connection, &pool).await?;
    wide_batches_stay_under_the_parameter_limit(&connection).await?;
    upserts_are_atomic_outside_a_transaction(&connection, &pool).await?;
    rendered_ddl_executes(&connection, &pool).await?;

    server.cleanup()?;
    Ok(())
}

/// #451: the server's error number survives, and a failed statement's own
/// error is what the transaction runner returns.
async fn errors_keep_their_classification(
    connection: &str,
    pool: &MssqlPool,
    writable: &graphql_orm::db::Database<MssqlBackend>,
) -> TestResult {
    execute_batch(
        connection,
        "CREATE TABLE dbo.GomUnique (Id int NOT NULL PRIMARY KEY, Parent int NULL, \
         Amount int NOT NULL CONSTRAINT GomUniqueAmount CHECK (Amount >= 0) DEFAULT 0); \
         CREATE TABLE dbo.GomChild (Id int NOT NULL PRIMARY KEY, \
         UniqueId int NOT NULL REFERENCES dbo.GomUnique(Id));",
    )
    .await?;

    let divide = pool
        .fetch_rows("SELECT 1 / 0 AS boom", &[])
        .await
        .err()
        .expect("division by zero");
    assert_eq!(server_error(&divide).number(), 8134);
    assert_eq!(
        divide.as_database_error().unwrap().code().as_deref(),
        Some("8134")
    );
    // The connection is reusable after a server error.
    assert_eq!(pool.fetch_rows("SELECT 1 AS one", &[]).await?.len(), 1);

    let duplicate = writable
        .transaction(TransactionMode::Default, |context| {
            Box::pin(async move {
                context
                    .execute("INSERT INTO dbo.GomUnique (Id) VALUES (1)", &[])
                    .await?;
                context
                    .execute("INSERT INTO dbo.GomUnique (Id) VALUES (1)", &[])
                    .await?;
                Ok::<(), OrmPublicError>(())
            })
        })
        .await;
    // Not an INTERNAL_ERROR from a rollback on a discarded connection.
    match duplicate {
        Err(TransactionError::Rejected(error)) => {
            assert_eq!(error.code, OrmErrorCode::ConstraintViolation, "{error:?}")
        }
        other => panic!("expected a rejected constraint violation, got {other:?}"),
    }
    // The first insert was rolled back with the failed transaction.
    assert!(
        pool.fetch_rows("SELECT Id FROM dbo.GomUnique", &[])
            .await?
            .is_empty()
    );

    for (sql, number, check) in [
        (
            "INSERT INTO dbo.GomChild (Id, UniqueId) VALUES (1, 404)",
            547,
            false,
        ),
        (
            "INSERT INTO dbo.GomUnique (Id, Amount) VALUES (2, -1)",
            547,
            true,
        ),
        (
            "INSERT INTO dbo.GomUnique (Id, Amount) VALUES (3, NULL)",
            515,
            false,
        ),
    ] {
        let error = writable
            .transaction(TransactionMode::Default, |context| {
                Box::pin(async move {
                    context.execute(sql, &[]).await.map_err(|error| {
                        let server = server_error(&error);
                        assert_eq!(server.number(), number, "{sql}");
                        let database = error.as_database_error().unwrap();
                        assert_eq!(database.is_check_violation(), check, "{sql}");
                        assert_eq!(
                            database.is_foreign_key_violation(),
                            number == 547 && !check,
                            "{sql}"
                        );
                        OrmPublicError::from_sqlx(&error)
                    })?;
                    Ok::<(), OrmPublicError>(())
                })
            })
            .await
            .expect_err(sql);
        assert!(
            matches!(error, TransactionError::Rejected(_)),
            "{sql}: {error:?}"
        );
    }

    // A failed transaction refuses further statements and never commits the
    // statements that ran before the failure.
    let continued = writable
        .transaction(TransactionMode::Default, |context| {
            Box::pin(async move {
                context
                    .execute("INSERT INTO dbo.GomUnique (Id) VALUES (10)", &[])
                    .await?;
                let _ = context
                    .execute("INSERT INTO dbo.GomUnique (Id) VALUES (10)", &[])
                    .await;
                let after = context
                    .execute("INSERT INTO dbo.GomUnique (Id) VALUES (11)", &[])
                    .await;
                assert!(
                    after.is_err(),
                    "a failed transaction must refuse statements"
                );
                Ok::<(), OrmPublicError>(())
            })
        })
        .await;
    assert!(continued.is_err(), "{continued:?}");
    assert!(
        pool.fetch_rows("SELECT Id FROM dbo.GomUnique", &[])
            .await?
            .is_empty()
    );
    // The pool still serves writes afterwards.
    writable
        .transaction(TransactionMode::Default, |context| {
            Box::pin(async move {
                context
                    .execute("INSERT INTO dbo.GomUnique (Id) VALUES (20)", &[])
                    .await?;
                Ok::<(), OrmPublicError>(())
            })
        })
        .await
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(
        pool.fetch_rows("SELECT Id FROM dbo.GomUnique", &[])
            .await?
            .len(),
        1
    );
    Ok(())
}

/// #452 and #521.
async fn decimals_and_dates_decode(pool: &MssqlPool) -> TestResult {
    let rows = pool
        .fetch_rows(
            "SELECT CAST(123.45 AS DECIMAL(10, 2)) AS small, \
             CAST(0.5 AS DECIMAL(38, 30)) AS deep, \
             CAST('99999999999999999999999999999999999999' AS DECIMAL(38, 0)) AS wide",
            &[],
        )
        .await?;
    assert_eq!(
        rows[0].try_get::<Decimal, _>("small")?.to_string(),
        "123.45"
    );
    // Scale 30 and a 127-bit mantissa are beyond `Decimal`: a decode error,
    // where the driver's own conversion panics.
    for column in ["deep", "wide"] {
        let error = rows[0]
            .try_get::<Decimal, _>(column)
            .expect_err("out of range");
        assert!(
            matches!(error, sqlx::Error::ColumnDecode { .. }),
            "{column}: {error:?}"
        );
    }

    let rows = pool
        .fetch_rows(
            "SELECT CAST('2026-11-09T12:34:56.1234567' AS datetime2) AS precise, \
             CAST('2026-11-09T12:34:56' AS datetime2(0)) AS whole, \
             CAST('2026-11-09T12:34:56' AS datetime) AS legacy, \
             CAST('2026-11-09T12:34:00' AS smalldatetime) AS small, \
             CAST('2026-11-09T12:34:56.5+02:00' AS datetimeoffset) AS shifted, \
             CAST('2026-11-09T12:34:56+00:00' AS datetimeoffset) AS utc, \
             CAST('2026-11-09' AS date) AS day, \
             CAST(NULL AS datetimeoffset) AS missing",
            &[],
        )
        .await?;
    let text = |column: &str| rows[0].try_get::<String, _>(column);
    assert_eq!(text("precise")?, "2026-11-09T12:34:56.123456700Z");
    assert_eq!(text("whole")?, "2026-11-09T12:34:56Z");
    assert_eq!(text("legacy")?, "2026-11-09T12:34:56Z");
    assert_eq!(text("small")?, "2026-11-09T12:34:00Z");
    assert_eq!(text("shifted")?, "2026-11-09T12:34:56.500+02:00");
    assert_eq!(text("utc")?, "2026-11-09T12:34:56Z");
    assert_eq!(text("day")?, "2026-11-09");
    assert_eq!(rows[0].try_get::<Option<String>, _>("missing")?, None);
    Ok(())
}

/// #456 and #457.
async fn empty_pages_and_bracket_patterns(connection: &str, pool: &MssqlPool) -> TestResult {
    execute_batch(
        connection,
        "CREATE TABLE dbo.GomNames (Id int NOT NULL PRIMARY KEY, Name nvarchar(50) NOT NULL); \
         INSERT INTO dbo.GomNames (Id, Name) VALUES (1, N'[draft] plan'), (2, N'd'), \
         (3, N'raft'), (4, N'50%_off'), (5, N'500 off');",
    )
    .await?;
    let page = |limit| SelectQuery {
        table: "dbo.GomNames",
        columns: vec!["[Id]".to_string()],
        filter: None,
        sorts: vec![SortExpression {
            clause: "[Id] ASC".to_string(),
            values: Vec::new(),
        }],
        pagination: Some(PaginationRequest { limit, offset: 1 }),
        count_only: false,
    };
    let empty = render_select_query(DatabaseBackend::Mssql, &page(Some(0)));
    assert!(pool.fetch_rows(&empty.sql, &empty.values).await?.is_empty());
    let two = render_select_query(DatabaseBackend::Mssql, &page(Some(2)));
    assert_eq!(pool.fetch_rows(&two.sql, &two.values).await?.len(), 2);
    // The dialect clause on its own is valid SQL for a zero limit too.
    let clause = DatabaseBackend::Mssql.render_pagination(Some(0), 0);
    let sql = format!("SELECT [Id] FROM dbo.GomNames ORDER BY [Id] ASC{clause}");
    assert!(pool.fetch_rows(&sql, &[]).await?.is_empty());

    let like = format!(
        "SELECT [Id] FROM dbo.GomNames WHERE {} ORDER BY [Id] ASC",
        DatabaseBackend::Mssql.ci_like("[Name]", "@P1")
    );
    let matching = |pattern: &str| {
        let like = like.clone();
        let pattern = contains_like_pattern(pattern);
        async move {
            let rows = pool.fetch_rows(&like, &[SqlValue::String(pattern)]).await?;
            rows.iter()
                .map(|row| row.try_get::<i32, _>("Id"))
                .collect::<Result<Vec<_>, _>>()
        }
    };
    // Unescaped, `[draft]` is a character class and matches rows 1, 2 and 3.
    assert_eq!(matching("[draft]").await?, [1]);
    assert_eq!(matching("50%_off").await?, [4]);
    assert_eq!(matching("raft").await?, [1, 3]);
    Ok(())
}

/// #519: the conversions the renderers emit lose nothing on the server.
async fn text_conversions_are_lossless(connection: &str, pool: &MssqlPool) -> TestResult {
    execute_batch(
        connection,
        "CREATE TABLE dbo.GomValues (Id int NOT NULL PRIMARY KEY, Ratio float NOT NULL, \
         Small real NOT NULL, SeenAt datetime NOT NULL, Note nvarchar(max) NOT NULL); \
         INSERT INTO dbo.GomValues (Id, Ratio, Small, SeenAt, Note) VALUES \
         (1, 1000000, 0.1, '2026-11-09T12:34:56', REPLICATE(CAST(N'x' AS nvarchar(max)), 5000)), \
         (2, 0.1, 2.5, '2026-11-09T12:34:57', N'short');",
    )
    .await?;
    let sql = format!(
        "SELECT {} AS ratio_key, {} AS small_key, \
         CONVERT(NVARCHAR(MAX), [SeenAt], 126) AS seen_at, \
         CAST([Ratio] AS NVARCHAR(4000)) AS lossy \
         FROM dbo.GomValues ORDER BY [Id] ASC",
        relation_key_projection(
            DatabaseBackend::Mssql,
            "[Ratio]",
            RelationKeyPartKind::Float
        ),
        relation_key_projection(
            DatabaseBackend::Mssql,
            "[Small]",
            RelationKeyPartKind::Float
        ),
    );
    let rows = pool.fetch_rows(&sql, &[]).await?;
    let float_key = |row: usize, column: &str| -> Result<RelationKey, sqlx::Error> {
        Ok(RelationKey::from_projected_parts(
            vec![rows[row].try_get::<String, _>(column)?],
            &[RelationKeyPartKind::Float],
        ))
    };
    let parent = |value: f64| RelationKey::from_sql_values(&[SqlValue::Float(value)]);
    // The default conversion this replaces renders `1e+006`.
    assert_eq!(rows[0].try_get::<String, _>("lossy")?, "1e+006");
    assert_eq!(float_key(0, "ratio_key")?, parent(1_000_000.0));
    assert_eq!(float_key(1, "ratio_key")?, parent(0.1));
    assert_eq!(float_key(0, "small_key")?, parent(f64::from(0.1_f32)));
    assert_eq!(float_key(1, "small_key")?, parent(2.5));
    // Seconds survive: the default conversion renders `Nov  9 2026 12:34PM`.
    assert_eq!(
        rows[0].try_get::<String, _>("seen_at")?,
        "2026-11-09T12:34:56"
    );
    assert_eq!(
        rows[1].try_get::<String, _>("seen_at")?,
        "2026-11-09T12:34:57"
    );

    let longest = pool
        .fetch_rows(
            "SELECT CONVERT(NVARCHAR(MAX), MAX([Note]), 126) AS longest FROM dbo.GomValues",
            &[],
        )
        .await?;
    assert_eq!(longest[0].try_get::<String, _>("longest")?.len(), 5000);
    Ok(())
}

/// #520: 800 parents of a three-column relation bind 2,400 values.
async fn wide_batches_stay_under_the_parameter_limit(connection: &str) -> TestResult {
    use graphql_orm::async_graphql::dataloader::Loader;

    execute_batch(
        connection,
        "CREATE TABLE dbo.GomBatchChildren (ChildId bigint NOT NULL PRIMARY KEY, \
         PartA bigint NOT NULL, PartB bigint NOT NULL, PartC bigint NOT NULL); \
         INSERT INTO dbo.GomBatchChildren (ChildId, PartA, PartB, PartC) \
         SELECT n, n, n + 1, n + 2 FROM (SELECT TOP (800) ROW_NUMBER() OVER (ORDER BY (SELECT 1)) AS n \
         FROM sys.all_objects) numbers;",
    )
    .await?;
    let database =
        graphql_orm::db::Database::<MssqlBackend>::new(MssqlPool::connect_ado(connection).await?);
    let keys = |paged: bool| {
        (1..=800_i64)
            .map(|n| {
                let values = vec![SqlValue::Int(n), SqlValue::Int(n + 1), SqlValue::Int(n + 2)];
                CompositeRelationQueryKey {
                    relation: "wide_children",
                    parent_key: RelationKey::from_sql_values(&values),
                    parent_values: values,
                    fk_columns: vec!["[PartA]", "[PartB]", "[PartC]"],
                    key_part_kinds: vec![RelationKeyPartKind::Int; 3],
                    where_signature: None,
                    order_signature: None,
                    page_signature: paged.then(|| "first".into()),
                    filter: None,
                    sorts: vec![],
                    pagination: paged.then_some(PaginationRequest {
                        limit: Some(1),
                        offset: 0,
                    }),
                    auth_context: None,
                }
            })
            .collect::<Vec<_>>()
    };
    let loader = RelationLoader::<GomBatchChild, MssqlBackend>::new(database);
    for paged in [false, true] {
        let keys = keys(paged);
        let results = loader.load(&keys).await?;
        assert_eq!(results.len(), 800, "paged: {paged}");
        for (index, key) in keys.iter().enumerate() {
            let page = &results[key];
            assert_eq!(page.total_count, 1, "paged: {paged}, parent {index}");
            assert_eq!(page.entities.len(), 1);
            assert_eq!(page.entities[0].id, index as i64 + 1);
        }
    }
    Ok(())
}

/// #523: the rendered upsert is atomic when no transaction is open, and a
/// failed update is the error the caller sees.
async fn upserts_are_atomic_outside_a_transaction(
    connection: &str,
    pool: &MssqlPool,
) -> TestResult {
    execute_batch(
        connection,
        "CREATE TABLE dbo.GomUpsert (Id int NOT NULL PRIMARY KEY, \
         Amount int NOT NULL CONSTRAINT GomUpsertAmount CHECK (Amount < 100));",
    )
    .await?;
    let upsert = |amount: &str| {
        render_upsert_sql(
            DatabaseBackend::Mssql,
            "dbo.GomUpsert",
            &["Id", "Amount"],
            &["1", amount],
            &["Id"],
            &["Amount"],
            false,
        )
    };
    let amount = || async {
        let rows = pool
            .fetch_rows("SELECT Amount FROM dbo.GomUpsert WHERE Id = 1", &[])
            .await?;
        rows[0].try_get::<i32, _>("Amount")
    };
    execute_batch(connection, &upsert("5")).await?;
    assert_eq!(amount().await?, 5);
    execute_batch(connection, &upsert("7")).await?;
    assert_eq!(amount().await?, 7);

    let error = execute_batch(connection, &upsert("500"))
        .await
        .expect_err("the update violates the check constraint");
    assert!(error.to_string().contains("GomUpsertAmount"), "{error}");
    assert_eq!(amount().await?, 7);

    // After a failure the batch leaves no transaction and no temporary table
    // on its connection.
    let recovered = format!(
        "BEGIN TRY {} END TRY BEGIN CATCH END CATCH; \
         IF @@TRANCOUNT <> 0 THROW 50001, N'transaction left open', 1; \
         IF OBJECT_ID(N'tempdb..#graphql_orm_upsert_source') IS NOT NULL \
         THROW 50003, N'temporary table left behind', 1;",
        upsert("500")
    );
    execute_batch(connection, &recovered).await?;
    assert_eq!(amount().await?, 7);

    // Inside a caller's transaction the batch neither commits nor rolls back.
    let nested = format!(
        "BEGIN TRANSACTION; {}; IF @@TRANCOUNT <> 1 THROW 50002, N'transaction count changed', 1; \
         ROLLBACK TRANSACTION;",
        upsert("11")
    );
    execute_batch(connection, &nested).await?;
    assert_eq!(amount().await?, 7);
    Ok(())
}

fn column(name: &str, sql_type: &str, nullable: bool) -> ColumnModel {
    ColumnModel {
        name: name.into(),
        sql_type: sql_type.into(),
        spatial: None,
        nullable,
        is_primary_key: false,
        is_unique: false,
        default: None,
    }
}

/// #537, #546, #524 and #430.
async fn rendered_ddl_executes(connection: &str, pool: &MssqlPool) -> TestResult {
    execute_batch(
        connection,
        "CREATE TABLE dbo.GomParents (Id bigint NOT NULL PRIMARY KEY); \
         CREATE TABLE dbo.GomKids (Id bigint NOT NULL PRIMARY KEY, ParentId bigint NOT NULL, \
         OwnerId int NOT NULL, Name nvarchar(50) NOT NULL, Slug nvarchar(50) NOT NULL, \
         Price decimal(19, 4) NOT NULL, Payload varbinary(max) NULL, Code varchar(12) NOT NULL, \
         UNIQUE (OwnerId, Name)); \
         CREATE TABLE dbo.GomOthers (Id bigint NOT NULL PRIMARY KEY, OwnerId int NOT NULL, \
         Name nvarchar(50) NOT NULL, Slug nvarchar(50) NOT NULL, UNIQUE (OwnerId, Name)); \
         CREATE TABLE dbo.[Gom.Dotted] (Id bigint NOT NULL PRIMARY KEY); \
         CREATE TABLE dbo.GomSorted (KeyA nvarchar(20) NOT NULL, KeyB nvarchar(20) NOT NULL, \
         PRIMARY KEY (KeyA, KeyB)); \
         INSERT INTO dbo.GomSorted (KeyA, KeyB) VALUES (N'b', N'1'), (N'a', N'2'), (N'a', N'1');",
    )
    .await?;

    // SQL Server rejects `ON DELETE RESTRICT`.
    let foreign_key = MigrationStep::AddForeignKey {
        table_name: "dbo.GomKids".into(),
        foreign_key: ForeignKeyModel::single(
            "ParentId",
            "dbo.GomParents",
            "Id",
            true,
            DeletePolicy::Restrict,
        ),
    };
    for statement in render_migration_step(DatabaseBackend::Mssql, &foreign_key) {
        execute_batch(connection, &statement).await?;
    }
    execute_batch(
        connection,
        "INSERT INTO dbo.GomParents (Id) VALUES (1); \
         INSERT INTO dbo.GomKids (Id, ParentId, OwnerId, Name, Slug, Price, Code) \
         VALUES (1, 1, 1, N'n', N's', 1, 'c');",
    )
    .await?;
    assert!(
        execute_batch(connection, "DELETE FROM dbo.GomParents WHERE Id = 1")
            .await
            .is_err(),
        "NO ACTION must refuse deleting a referenced parent"
    );

    // Two composite-unique changes compose into one batch.
    let uniques = |table: &str| MigrationStep::SetCompositeUniques {
        table_name: table.into(),
        before: vec![vec!["OwnerId".into(), "Name".into()]],
        after: vec![vec!["OwnerId".into(), "Slug".into()]],
    };
    let plan = [uniques("dbo.GomKids"), uniques("dbo.GomOthers")]
        .iter()
        .flat_map(|step| render_migration_step(DatabaseBackend::Mssql, step))
        .collect::<Vec<_>>()
        .join("; ");
    execute_batch(
        connection,
        &format!("BEGIN TRANSACTION; {plan}; COMMIT TRANSACTION;"),
    )
    .await?;

    // Column changes render as T-SQL and keep the declared nullability.
    for step in [
        MigrationStep::AddColumn {
            table_name: "dbo.GomOthers".into(),
            column: column("Note", "NVARCHAR(20)", false),
        },
        MigrationStep::AlterColumn {
            table_name: "dbo.GomOthers".into(),
            before: column("Note", "NVARCHAR(20)", false),
            after: column("Note", "NVARCHAR(40)", false),
        },
    ] {
        for statement in render_migration_step(DatabaseBackend::Mssql, &step) {
            execute_batch(connection, &statement).await?;
        }
    }

    let schema = introspect_mssql_schema(pool).await?;
    let table = |name: &str| {
        schema
            .tables
            .iter()
            .find(|table| table.table_name == name)
            .unwrap_or_else(|| {
                panic!(
                    "{name} not in {:?}",
                    schema
                        .tables
                        .iter()
                        .map(|table| &table.table_name)
                        .collect::<Vec<_>>()
                )
            })
    };
    let kids = table("[dbo].[GomKids]");
    let sql_type = |name: &str| {
        kids.columns
            .iter()
            .find(|column| column.name == name)
            .map(|column| column.sql_type.to_ascii_uppercase())
            .unwrap_or_else(|| panic!("{name}"))
    };
    // Compared without their parameters, these always read as changed.
    assert_eq!(sql_type("[Price]"), "DECIMAL(19,4)");
    assert_eq!(sql_type("[Payload]"), "VARBINARY(MAX)");
    assert_eq!(sql_type("[Code]"), "VARCHAR(12)");
    assert_eq!(sql_type("[Name]"), "NVARCHAR(50)");
    assert_eq!(sql_type("[Id]"), "BIGINT");
    assert_eq!(
        kids.composite_unique_indexes,
        [["[OwnerId]".to_string(), "[Slug]".to_string()]]
    );
    // A `.` inside a table name is not a schema separator.
    table("[dbo].[Gom.Dotted]");
    let note = table("[dbo].[GomOthers]")
        .columns
        .iter()
        .find(|column| column.name == "[Note]")
        .expect("added column");
    assert_eq!(note.sql_type.to_ascii_uppercase(), "NVARCHAR(40)");
    assert!(!note.nullable, "ALTER COLUMN must keep NOT NULL");

    // `default_sort = "KeyA,KeyB"` orders by both columns.
    assert_eq!(
        <GomSorted as DatabaseEntity>::DEFAULT_SORT,
        "[KeyA], [KeyB]"
    );
    let rows = pool
        .fetch_rows(
            &format!(
                "SELECT KeyA + KeyB AS k FROM dbo.GomSorted ORDER BY {}",
                <GomSorted as DatabaseEntity>::DEFAULT_SORT
            ),
            &[],
        )
        .await?;
    let order = rows
        .iter()
        .map(|row| row.try_get::<String, _>("k"))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(order, ["a1", "a2", "b1"]);
    Ok(())
}
