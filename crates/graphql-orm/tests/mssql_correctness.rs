//! SQL Server behaviour that only a server can confirm, run on the disposable
//! container these tests own. Rendered text is asserted in the unit tests and
//! `mssql_ddl_rendering.rs`; here the same statements execute.
#![cfg(feature = "mssql")]

#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;

#[allow(dead_code)]
#[path = "support/like_matrix.rs"]
mod like_matrix;

use graphql_orm::db::mssql::{MssqlPool, MssqlServerError};
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::graphql::loaders::{
    CompositeRelationQueryKey, RelationKey, RelationKeyPartKind, RelationLoader,
    relation_key_projection,
};
use graphql_orm::graphql::orm::{
    ColumnModel, DatabaseBackend, DatabaseEntity, DeletePolicy, FilterExpression, ForeignKeyModel,
    MigrationStep, PaginationRequest, SelectQuery, SortExpression, SqlDialect, SqlValue,
    TransactionError, TransactionMode, contains_like_pattern, introspect_mssql_schema,
    render_migration_step, render_select_query, render_upsert_sql,
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
    entities: [GomBatchChild, GomLongKeyChild],
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

/// A quoted column whose name contains `]` and `,` in a bare default sort.
#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "dbo.GomEscapedSort",
    default_sort = "[a]],b],KeyB"
)]
struct GomEscapedSort {
    #[primary_key]
    #[graphql_orm(db_column = "a],b")]
    odd: String,
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
    long_string_keys_stay_distinct(&connection).await?;
    like_matrix_matches_literally(&pool).await?;
    dates_decode_at_their_boundaries_in_any_language(&connection).await?;
    upsert_failures_keep_their_error_in_a_caller_transaction(&connection).await?;
    upserts_are_atomic_outside_a_transaction(&connection, &pool).await?;
    rendered_ddl_executes(&connection, &pool).await?;
    rendered_schema_round_trips(&connection, &pool).await?;

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

/// #520: 800 parents of a three-column relation bind 2,400 values, so every
/// load below runs as more than one statement. Each parent has three children
/// (ranks 1 to 3); the rank 2 child is the one a visibility filter hides.
async fn wide_batches_stay_under_the_parameter_limit(connection: &str) -> TestResult {
    use graphql_orm::async_graphql::dataloader::Loader;
    use graphql_orm::graphql::loaders::{FederationEntityKey, FederationKeyLoader};

    execute_batch(
        connection,
        "CREATE TABLE dbo.GomBatchChildren (ChildId bigint NOT NULL PRIMARY KEY, \
         PartA bigint NOT NULL, PartB bigint NOT NULL, PartC bigint NOT NULL, \
         Rank int NOT NULL, Hidden bit NOT NULL); \
         INSERT INTO dbo.GomBatchChildren (ChildId, PartA, PartB, PartC, Rank, Hidden) \
         SELECT n * 10 + r, n, n + 1, n + 2, r, CASE WHEN r = 2 THEN 1 ELSE 0 END \
         FROM (SELECT TOP (800) ROW_NUMBER() OVER (ORDER BY (SELECT 1)) AS n \
         FROM sys.all_objects) numbers CROSS JOIN (VALUES (1), (2), (3)) ranks(r);",
    )
    .await?;
    let database =
        graphql_orm::db::Database::<MssqlBackend>::new(MssqlPool::connect_ado(connection).await?);
    let key = |n: i64,
               filter: Option<FilterExpression>,
               sorts: Vec<SortExpression>,
               pagination: Option<PaginationRequest>| {
        let values = vec![SqlValue::Int(n), SqlValue::Int(n + 1), SqlValue::Int(n + 2)];
        CompositeRelationQueryKey {
            relation: "wide_children",
            parent_key: RelationKey::from_sql_values(&values),
            parent_values: values,
            fk_columns: vec!["[PartA]", "[PartB]", "[PartC]"],
            key_part_kinds: vec![RelationKeyPartKind::Int; 3],
            where_signature: filter.as_ref().map(|_| "visible".into()),
            order_signature: (!sorts.is_empty()).then(|| "ranked".into()),
            page_signature: pagination.as_ref().map(|_| "first".into()),
            filter,
            sorts,
            pagination,
            auth_context: None,
        }
    };
    let ids = |page: &graphql_orm::graphql::loaders::RelationLoadResult<GomBatchChild>| {
        page.entities
            .iter()
            .map(|child| child.id)
            .collect::<Vec<_>>()
    };
    let loader = RelationLoader::<GomBatchChild, MssqlBackend>::new(database.clone());

    // Every child of every parent, unpaged and as a first page.
    for paged in [false, true] {
        let page = paged.then_some(PaginationRequest {
            limit: Some(1),
            offset: 0,
        });
        let keys = (1..=800)
            .map(|n| key(n, None, Vec::new(), page.clone()))
            .collect::<Vec<_>>();
        let results = loader
            .load(&keys)
            .await
            .map_err(|error| format!("load 1: {error}"))?;
        assert_eq!(results.len(), 800, "paged: {paged}");
        for (key, n) in keys.iter().zip(1..=800_i64) {
            let page = &results[key];
            assert_eq!(page.total_count, 3, "paged: {paged}, parent {n}");
            let expected = if paged {
                vec![n * 10 + 1]
            } else {
                vec![n * 10 + 1, n * 10 + 2, n * 10 + 3]
            };
            assert_eq!(ids(page), expected, "paged: {paged}, parent {n}");
            assert_eq!(page.has_next_page, paged);
        }
    }

    // The visibility filter binds a value and applies in every chunk: the
    // hidden child of a parent in the last chunk stays hidden like the first.
    let visible =
        || FilterExpression::trusted_fragment("[Hidden] = ?", vec![SqlValue::Bool(false)]);
    // A bound sort value: the child nearest rank 3 first.
    let nearest = || {
        vec![SortExpression {
            clause: "ABS([Rank] - ?) ASC".to_string(),
            values: vec![SqlValue::Int(3)],
        }]
    };
    let mut keys = (1..=800)
        .map(|n| key(n, Some(visible()), nearest(), None))
        .collect::<Vec<_>>();
    // Duplicate input keys load once and resolve for every caller.
    keys.extend((1..=10).map(|n| key(n, Some(visible()), nearest(), None)));
    let results = loader
        .load(&keys)
        .await
        .map_err(|error| format!("load 2: {error}"))?;
    assert_eq!(results.len(), 800);
    for (key, n) in keys.iter().zip((1..=800_i64).chain(1..=10)) {
        let page = &results[key];
        assert_eq!(ids(page), [n * 10 + 3, n * 10 + 1], "parent {n}");
        assert_eq!(page.total_count, 2, "parent {n}");
    }

    // Per-parent limits and ordering across chunks, with the filter applied
    // to the counts as well as the rows.
    let keys = (1..=800)
        .map(|n| {
            key(
                n,
                Some(visible()),
                vec![SortExpression::unbound("[ChildId] DESC".to_string())],
                Some(PaginationRequest {
                    limit: Some(1),
                    offset: 0,
                }),
            )
        })
        .collect::<Vec<_>>();
    let results = loader
        .load(&keys)
        .await
        .map_err(|error| format!("load 3: {error}"))?;
    for (key, n) in keys.iter().zip(1..=800_i64) {
        let page = &results[key];
        assert_eq!(ids(page), [n * 10 + 3], "parent {n}");
        assert_eq!(page.total_count, 2, "parent {n}");
        assert!(page.has_next_page, "parent {n}");
    }

    // A filter that fills the statement on its own is refused before it is
    // sent, with the public code for a request the client must reduce.
    let oversized = FilterExpression::trusted_fragment(
        format!("[Rank] IN ({})", vec!["?"; 1_999].join(", ")),
        vec![SqlValue::Int(1); 1_999],
    );
    let error = loader
        .load(&[key(1, Some(oversized), Vec::new(), None)])
        .await
        .expect_err("no room for one key");
    assert_eq!(error, "graphql-orm-public:INVALID_INPUT");

    // An empty batch runs nothing.
    assert!(
        Loader::<CompositeRelationQueryKey>::load(&loader, &[])
            .await?
            .is_empty()
    );

    // Federation keys: 800 three-column representations, and one that
    // matches no row.
    let representation = |n: i64, rank: i64| {
        let values = vec![
            SqlValue::Int(n * 10 + rank),
            SqlValue::Int(n),
            SqlValue::Int(n + 1),
        ];
        FederationEntityKey {
            key: "child",
            columns: vec!["[ChildId]", "[PartA]", "[PartB]"],
            key_part_kinds: vec![RelationKeyPartKind::Int; 3],
            representation: RelationKey::from_sql_values(&values),
            values,
            auth_context: None,
        }
    };
    let federation = FederationKeyLoader::<GomBatchChild, MssqlBackend>::new(database.clone());
    let mut representations = (1..=800).map(|n| representation(n, 3)).collect::<Vec<_>>();
    representations.push(representation(801, 3));
    let resolved = federation
        .load(&representations)
        .await
        .map_err(|error| format!("load 4: {error}"))?;
    assert_eq!(resolved.len(), 801);
    for (representation, n) in representations.iter().zip(1..=800_i64) {
        assert_eq!(
            resolved[representation].as_ref().map(|child| child.id),
            Some(n * 10 + 3)
        );
    }
    assert!(resolved[&representations[800]].is_none());

    // The single-column loader: 2,500 keys in one `IN` list.
    let singles = (1..=2_500).map(|id| id.to_string()).collect::<Vec<_>>();
    let loaded = Loader::<String>::load(&loader, &singles)
        .await
        .map_err(|error| format!("load 5: {error}"))?;
    assert_eq!(loaded.len(), 2_500);
    let found = loaded.values().filter(|rows| !rows.is_empty()).count();
    // Children 11, 12, 13, 21, ... up to 2493.
    assert_eq!(found, 249 * 3);
    assert_eq!(loaded["2493"][0].id, 2493);
    assert!(loaded["2500"].is_empty());
    Ok(())
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.GomLongKeyChildren",
    plural = "GomLongKeyChildren",
    default_sort = "[ChildId] ASC"
)]
pub struct GomLongKeyChild {
    #[primary_key]
    #[graphql_orm(db_column = "ChildId", write = false)]
    #[filterable(type = "number")]
    #[sortable]
    pub id: i64,

    #[graphql_orm(db_column = "ParentKey", write = false)]
    pub parent_key: String,
}

impl graphql_orm::graphql::loaders::BatchLoadEntity<MssqlBackend> for GomLongKeyChild {
    fn batch_column() -> &'static str {
        "[ChildId]"
    }

    fn batch_key_from_row(row: &graphql_orm::db::mssql::MssqlRow) -> Result<String, sqlx::Error> {
        row.try_get::<i64, _>("ChildId")
            .map(|value| value.to_string())
    }
}

/// #519: string relation keys longer than 4,000 characters, sharing their
/// first 4,000, stay distinct.
async fn long_string_keys_stay_distinct(connection: &str) -> TestResult {
    use graphql_orm::async_graphql::dataloader::Loader;

    execute_batch(
        connection,
        "CREATE TABLE dbo.GomLongKeyChildren (ChildId bigint NOT NULL PRIMARY KEY, \
         ParentKey nvarchar(max) NOT NULL); \
         INSERT INTO dbo.GomLongKeyChildren (ChildId, ParentKey) VALUES \
         (1, REPLICATE(CAST(N'k' AS nvarchar(max)), 4000) + N'-first'), \
         (2, REPLICATE(CAST(N'k' AS nvarchar(max)), 4000) + N'-second'), \
         (3, REPLICATE(CAST(N'k' AS nvarchar(max)), 4000) + N'-second');",
    )
    .await?;
    let database =
        graphql_orm::db::Database::<MssqlBackend>::new(MssqlPool::connect_ado(connection).await?);
    let loader = RelationLoader::<GomLongKeyChild, MssqlBackend>::new(database);
    let key = |suffix: &str, paged: bool| {
        let values = vec![SqlValue::String(format!("{}{suffix}", "k".repeat(4000)))];
        CompositeRelationQueryKey {
            relation: "long_children",
            parent_key: RelationKey::from_sql_values(&values),
            parent_values: values,
            fk_columns: vec!["[ParentKey]"],
            key_part_kinds: vec![RelationKeyPartKind::String],
            where_signature: None,
            order_signature: None,
            page_signature: paged.then(|| "first".into()),
            filter: None,
            sorts: vec![],
            pagination: paged.then_some(PaginationRequest {
                limit: Some(5),
                offset: 0,
            }),
            auth_context: None,
        }
    };
    for paged in [false, true] {
        let keys = [
            key("-first", paged),
            key("-second", paged),
            key("-none", paged),
        ];
        let results = loader.load(&keys).await?;
        let ids = |index: usize| {
            results[&keys[index]]
                .entities
                .iter()
                .map(|child| child.id)
                .collect::<Vec<_>>()
        };
        // Cut at 4,000 characters, both parents would read as one key and
        // neither would match its own, longer value.
        assert_eq!(ids(0), [1], "paged: {paged}");
        assert_eq!(ids(1), [2, 3], "paged: {paged}");
        assert!(ids(2).is_empty(), "paged: {paged}");
        assert_eq!(results[&keys[0]].total_count, 1, "paged: {paged}");
        assert_eq!(results[&keys[1]].total_count, 2, "paged: {paged}");
    }
    Ok(())
}

/// #457: every pattern character as literal search text.
async fn like_matrix_matches_literally(pool: &MssqlPool) -> TestResult {
    let values = like_matrix::haystacks();
    let rows = (1..=values.len())
        .map(|index| format!("({index}, @P{index})"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT value FROM (VALUES {rows}) haystack(position, value) WHERE {} ORDER BY position",
        DatabaseBackend::Mssql.ci_like("value", &format!("@P{}", values.len() + 1))
    );
    for needle in like_matrix::NEEDLES {
        let mut binds = values
            .iter()
            .cloned()
            .map(SqlValue::String)
            .collect::<Vec<_>>();
        binds.push(SqlValue::String(contains_like_pattern(needle)));
        let found = pool
            .fetch_rows(&sql, &binds)
            .await?
            .iter()
            .map(|row| row.try_get::<String, _>("value"))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(found, like_matrix::expected(needle), "needle {needle:?}");
    }
    Ok(())
}

/// #521: range boundaries decode, and a session language or date format
/// that changes how SQL Server reads date text does not change what is read.
async fn dates_decode_at_their_boundaries_in_any_language(connection: &str) -> TestResult {
    // A pool of its own: the session settings must not reach other tests.
    let pool = MssqlPool::connect_ado_with(
        connection,
        graphql_orm::db::mssql::MssqlPoolOptions::new().max_connections(1),
    )
    .await?;
    let select = "SELECT CONVERT(datetime2, '0001-01-01T00:00:00', 126) AS first2, \
         CONVERT(datetime2, '9999-12-31T23:59:59.9999999', 126) AS last2, \
         CONVERT(datetime, '1753-01-01T00:00:00', 126) AS first1, \
         CONVERT(datetime, '9999-12-31T23:59:59.997', 126) AS last1, \
         CONVERT(smalldatetime, '1900-01-01T00:00:00', 126) AS first_small, \
         CONVERT(smalldatetime, '2079-06-06T23:59:00', 126) AS last_small, \
         CAST('0001-01-01T00:00:00+00:00' AS datetimeoffset) AS first_offset, \
         CAST('9999-12-31T09:59:59.9999999-14:00' AS datetimeoffset) AS west, \
         CAST('2026-03-04T05:06:07+14:00' AS datetimeoffset) AS east, \
         CONVERT(date, '0001-01-01', 23) AS first_day, \
         CONVERT(date, '9999-12-31', 23) AS last_day, \
         CONVERT(datetime2, '2026-03-04T05:06:07', 126) AS ambiguous";
    let expected = [
        ("first2", "0001-01-01T00:00:00Z"),
        ("last2", "9999-12-31T23:59:59.999999900Z"),
        ("first1", "1753-01-01T00:00:00Z"),
        ("last1", "9999-12-31T23:59:59.997Z"),
        ("first_small", "1900-01-01T00:00:00Z"),
        ("last_small", "2079-06-06T23:59:00Z"),
        ("first_offset", "0001-01-01T00:00:00Z"),
        ("west", "9999-12-31T09:59:59.999999900-14:00"),
        ("east", "2026-03-04T05:06:07+14:00"),
        ("first_day", "0001-01-01"),
        ("last_day", "9999-12-31"),
        // 4 March in every language: never 3 April.
        ("ambiguous", "2026-03-04T05:06:07Z"),
    ];
    for settings in [
        "SET LANGUAGE us_english; SET DATEFORMAT mdy;",
        "SET LANGUAGE Deutsch;",
        "SET LANGUAGE British; SET DATEFORMAT dmy;",
        "SET LANGUAGE us_english; SET DATEFORMAT ydm;",
    ] {
        let rows = pool
            .fetch_rows(&format!("{settings} {select}"), &[])
            .await?;
        for (column, text) in expected {
            assert_eq!(
                rows[0].try_get::<String, _>(column)?,
                text,
                "{column} under {settings}"
            );
        }
    }
    Ok(())
}

/// #523 with #451: when the server ends a caller's transaction during the
/// update, the upsert reports that error, not one about its own cleanup.
async fn upsert_failures_keep_their_error_in_a_caller_transaction(connection: &str) -> TestResult {
    execute_batch(
        connection,
        "CREATE TABLE dbo.GomVictim (Id int NOT NULL PRIMARY KEY, \
         Amount int NOT NULL CONSTRAINT GomVictimAmount CHECK (Amount < 100)); \
         INSERT INTO dbo.GomVictim (Id, Amount) VALUES (1, 1), (2, 1);",
    )
    .await?;
    // A real deadlock needs two sessions and timing. This trigger does what
    // the deadlock monitor does to its victim: it rolls the whole transaction
    // back, the upsert's source table with it, and raises an error.
    execute_batch(
        connection,
        "CREATE TRIGGER dbo.GomVictimTrigger ON dbo.GomVictim AFTER UPDATE AS \
         IF EXISTS (SELECT 1 FROM inserted WHERE Id = 1) BEGIN \
         ROLLBACK TRANSACTION; THROW 51205, N'rolled back as the victim', 1; END;",
    )
    .await?;
    let upsert = |id: &str, amount: &str| {
        render_upsert_sql(
            DatabaseBackend::Mssql,
            "dbo.GomVictim",
            &["Id", "Amount"],
            &[id, amount],
            &["Id"],
            &["Amount"],
            false,
        )
    };
    let number_in_transaction = |pool: MssqlPool, sql: String| async move {
        let database = graphql_orm::db::Database::<MssqlBackend>::new(pool);
        let number = std::sync::Arc::new(std::sync::Mutex::new(None));
        let seen = number.clone();
        let result = database
            .transaction(TransactionMode::Default, |context| {
                Box::pin(async move {
                    if let Err(error) = context.execute(&sql, &[]).await {
                        *seen.lock().unwrap() = Some(server_error(&error).number());
                        return Err(OrmPublicError::from_sqlx(&error));
                    }
                    Ok::<(), OrmPublicError>(())
                })
            })
            .await;
        assert!(result.is_err(), "{result:?}");
        number.lock().unwrap().expect("a server error")
    };

    // The transaction and the source table are gone when CATCH runs. Reading
    // the table there raises error 208 in place of the trigger's error.
    let pool = MssqlPool::connect_ado_external_writable(connection).await?;
    assert_eq!(
        number_in_transaction(pool.clone(), upsert("1", "5")).await,
        51205
    );
    let amount = pool
        .fetch_rows("SELECT Amount FROM dbo.GomVictim WHERE Id = 1", &[])
        .await?;
    assert_eq!(amount[0].try_get::<i32, _>("Amount")?, 1);

    // An uncommittable transaction (`XACT_ABORT ON`): the source table can
    // be read but not dropped, and the constraint error must come through.
    let doomed = MssqlPool::connect_ado_external_writable_with(
        connection,
        graphql_orm::db::mssql::MssqlPoolOptions::new().max_connections(1),
    )
    .await?;
    assert_eq!(
        number_in_transaction(doomed, format!("SET XACT_ABORT ON; {}", upsert("2", "500"))).await,
        547
    );
    let amount = pool
        .fetch_rows("SELECT Amount FROM dbo.GomVictim WHERE Id = 2", &[])
        .await?;
    assert_eq!(amount[0].try_get::<i32, _>("Amount")?, 1);
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

    // `[a]],b]` is one quoted column named `a],b`, not two terms.
    assert_eq!(
        <GomEscapedSort as DatabaseEntity>::DEFAULT_SORT,
        "[a]],b], [KeyB]"
    );
    execute_batch(
        connection,
        "CREATE TABLE dbo.GomEscapedSort ([a]],b] nvarchar(20) NOT NULL, KeyB nvarchar(20) NOT NULL, \
         PRIMARY KEY ([a]],b], KeyB)); \
         INSERT INTO dbo.GomEscapedSort ([a]],b], KeyB) VALUES (N'b', N'1'), (N'a', N'2'), (N'a', N'1');",
    )
    .await?;
    let rows = pool
        .fetch_rows(
            &format!(
                "SELECT [a]],b] + KeyB AS k FROM dbo.GomEscapedSort ORDER BY {}",
                <GomEscapedSort as DatabaseEntity>::DEFAULT_SORT
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

/// #524: a rendered schema, applied and read back, plans no change.
async fn rendered_schema_round_trips(connection: &str, pool: &MssqlPool) -> TestResult {
    use graphql_orm::graphql::orm::{SchemaModel, TableModel, diff_schema_models_for_backend};

    let key = |name: &str, sql_type: &str| ColumnModel {
        is_primary_key: true,
        ..column(name, sql_type, false)
    };
    let with_default = |name: &str, sql_type: &str, default: &str| ColumnModel {
        default: Some(default.into()),
        ..column(name, sql_type, false)
    };
    let table = |name: &str, keys: &[&str], columns: Vec<ColumnModel>| TableModel {
        entity_name: name.into(),
        table_name: format!("[dbo].[{name}]"),
        primary_key: keys[0].to_string(),
        primary_keys: keys.iter().map(|key| key.to_string()).collect(),
        default_sort: format!("{} ASC", keys[0]),
        columns,
        indexes: Vec::new(),
        composite_unique_indexes: Vec::new(),
        foreign_keys: Vec::new(),
        search_indexes: Vec::new(),
        append_only: false,
        retention_purge: false,
        check_constraints: Vec::new(),
    };
    let reference = |source: &str, target: &str, column: &str, on_delete| {
        ForeignKeyModel::single(source, format!("[dbo].[{target}]"), column, true, on_delete)
    };

    let accounts = table(
        "RtAccounts",
        &["[Region]", "[Number]"],
        vec![
            key("[Region]", "NVARCHAR(20)"),
            key("[Number]", "BIGINT"),
            ColumnModel {
                is_unique: true,
                ..column("[Code]", "VARCHAR(40)", false)
            },
        ],
    );
    let owners = table("RtOwners", &["[Id]"], vec![key("[Id]", "UNIQUEIDENTIFIER")]);
    let mut entries = table(
        "RtEntries",
        &["[Id]"],
        vec![
            key("[Id]", "BIGINT"),
            column("[Region]", "NVARCHAR(20)", false),
            column("[Number]", "BIGINT", false),
            column("[OwnerId]", "UNIQUEIDENTIFIER", false),
            column("[ReviewerId]", "UNIQUEIDENTIFIER", true),
            column("[ApproverId]", "UNIQUEIDENTIFIER", false),
            column("[Label]", "NVARCHAR(100)", false),
            column("[Slug]", "NVARCHAR(100)", false),
            column("[Amount]", "DECIMAL(19,4)", false),
            column("[Price]", "MONEY", true),
            column("[Ratio]", "FLOAT", true),
            column("[OccurredAt]", "DATETIME2", false),
            column("[BusinessDate]", "DATE", true),
            column("[Payload]", "VARBINARY(MAX)", true),
            column("[Document]", "NVARCHAR(MAX)", true),
            column("[Enabled]", "BIT", false),
            with_default("[Attempts]", "INT", "0"),
            with_default("[State]", "NVARCHAR(20)", "'new'"),
        ],
    );
    entries.composite_unique_indexes = vec![vec!["[OwnerId]".into(), "[Slug]".into()]];
    entries.foreign_keys = vec![
        ForeignKeyModel {
            constraint_name: None,
            column_pairs: vec![
                graphql_orm::graphql::orm::ForeignKeyColumnPairModel {
                    source_column: "[Region]".into(),
                    target_column: "[Region]".into(),
                },
                graphql_orm::graphql::orm::ForeignKeyColumnPairModel {
                    source_column: "[Number]".into(),
                    target_column: "[Number]".into(),
                },
            ],
            target_table: "[dbo].[RtAccounts]".into(),
            is_multiple: true,
            on_delete: DeletePolicy::Cascade,
        },
        reference("[OwnerId]", "RtOwners", "[Id]", DeletePolicy::Restrict),
        reference("[ReviewerId]", "RtOwners", "[Id]", DeletePolicy::SetNull),
    ];
    let target = SchemaModel {
        extensions: Vec::new(),
        tables: vec![accounts, owners, entries],
    };

    let ours = |schema: &SchemaModel| SchemaModel {
        extensions: Vec::new(),
        tables: schema
            .tables
            .iter()
            .filter(|table| table.table_name.starts_with("[dbo].[Rt"))
            .cloned()
            .collect(),
    };
    let empty = SchemaModel {
        extensions: Vec::new(),
        tables: Vec::new(),
    };
    let create = diff_schema_models_for_backend(DatabaseBackend::Mssql, &empty, &target);
    for step in &create.steps {
        for statement in render_migration_step(DatabaseBackend::Mssql, step) {
            execute_batch(connection, &statement)
                .await
                .map_err(|error| format!("{statement}: {error}"))?;
        }
    }
    let live = ours(&introspect_mssql_schema(pool).await?);
    assert_eq!(live.tables.len(), 3);
    let replan = diff_schema_models_for_backend(DatabaseBackend::Mssql, &live, &target);
    assert!(
        replan.steps.is_empty(),
        "an applied schema must plan no change: {:#?}",
        replan.steps
    );

    // Each referential action was applied as declared.
    let entries = live
        .tables
        .iter()
        .find(|table| table.table_name == "[dbo].[RtEntries]")
        .expect("entries");
    let mut actions = entries
        .foreign_keys
        .iter()
        .map(|foreign_key| {
            (
                foreign_key
                    .column_pairs
                    .iter()
                    .map(|pair| pair.source_column.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                foreign_key.on_delete.clone(),
            )
        })
        .collect::<Vec<_>>();
    actions.sort();
    assert_eq!(
        actions,
        [
            ("[OwnerId]".to_string(), DeletePolicy::Restrict),
            ("[Region],[Number]".to_string(), DeletePolicy::Cascade),
            ("[ReviewerId]".to_string(), DeletePolicy::SetNull),
        ]
    );

    // Nullability and type changes plan, apply and settle.
    let mut changed = target.clone();
    for column in &mut changed.tables[2].columns {
        match column.name.as_str() {
            "[Ratio]" => column.nullable = false,
            "[Label]" => column.sql_type = "NVARCHAR(200)".into(),
            "[Amount]" => column.sql_type = "DECIMAL(28,6)".into(),
            "[ApproverId]" => column.nullable = true,
            _ => {}
        }
    }
    changed.tables[2]
        .columns
        .push(column("[Note]", "NVARCHAR(50)", true));
    let alter = diff_schema_models_for_backend(DatabaseBackend::Mssql, &live, &changed);
    assert!(!alter.steps.is_empty());
    for step in &alter.steps {
        for statement in render_migration_step(DatabaseBackend::Mssql, step) {
            execute_batch(connection, &statement)
                .await
                .map_err(|error| format!("{statement}: {error}"))?;
        }
    }
    let live = ours(&introspect_mssql_schema(pool).await?);
    let settled = diff_schema_models_for_backend(DatabaseBackend::Mssql, &live, &changed);
    assert!(
        settled.steps.is_empty(),
        "an applied change must plan no further change: {:#?}",
        settled.steps
    );
    Ok(())
}
