//! Empty membership and boolean predicates must never silently widen SQL.
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(backend = "sqlite", table = "empty_filter_rows", auth = "none")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(backend = "postgres", table = "empty_filter_rows", auth = "none")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "empty_filter_rows",
        schema_policy = "external_writable",
        auth = "none"
    )
)]
struct EmptyFilterRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    text: String,
    #[filterable(type = "number")]
    number: i32,
    #[filterable(type = "int64")]
    wide: i64,
    #[filterable(type = "uuid")]
    uid: graphql_orm::uuid::Uuid,
    #[filterable(type = "bytes")]
    bytes: Vec<u8>,
}
fn row() -> EmptyFilterRow {
    EmptyFilterRow {
        id: "row".into(),
        text: "visible".into(),
        number: 1,
        wide: 1,
        uid: graphql_orm::uuid::Uuid::nil(),
        bytes: vec![1],
    }
}
fn filters() -> Vec<EmptyFilterRowWhereInput> {
    vec![
        EmptyFilterRowWhereInput {
            text: Some(StringFilter {
                in_list: Some(vec![]),
                ..Default::default()
            }),
            ..Default::default()
        },
        EmptyFilterRowWhereInput {
            number: Some(IntFilter {
                in_list: Some(vec![]),
                ..Default::default()
            }),
            ..Default::default()
        },
        EmptyFilterRowWhereInput {
            wide: Some(Int64Filter {
                in_list: Some(vec![]),
                ..Default::default()
            }),
            ..Default::default()
        },
        EmptyFilterRowWhereInput {
            uid: Some(UuidFilter {
                in_list: Some(vec![]),
                ..Default::default()
            }),
            ..Default::default()
        },
        EmptyFilterRowWhereInput {
            bytes: Some(BytesFilter {
                in_list: Some(vec![]),
                ..Default::default()
            }),
            ..Default::default()
        },
        EmptyFilterRowWhereInput {
            or: Some(vec![]),
            ..Default::default()
        },
    ]
}
#[test]
fn empty_inclusion_is_false_for_every_list_filter() {
    for filter in filters() {
        let (sql, binds) = filter.to_sql_conditions();
        assert_eq!(sql, ["1 = 0"]);
        assert!(binds.is_empty());
        assert!(!filter.is_empty());
        assert!(!filter.matches_entity(&row()).unwrap());
        // In particular, another field must not make an authorization predicate
        // silently lose the empty-list restriction.
        let filter = EmptyFilterRowWhereInput {
            id: Some(StringFilter {
                eq: Some("row".into()),
                ..Default::default()
            }),
            ..filter
        };
        assert!(
            filter
                .to_sql_conditions()
                .0
                .iter()
                .any(|sql| sql == "1 = 0")
        );
        assert!(ReadPredicate::from_filter::<DefaultBackend, _>(&filter).is_ok());
        assert!(!filter.matches_entity(&row()).unwrap());
    }
}
#[test]
fn empty_exclusion_is_a_noop_for_every_list_filter() {
    let filters = [
        EmptyFilterRowWhereInput {
            text: Some(StringFilter {
                not_in: Some(vec![]),
                ..Default::default()
            }),
            ..Default::default()
        },
        EmptyFilterRowWhereInput {
            number: Some(IntFilter {
                not_in: Some(vec![]),
                ..Default::default()
            }),
            ..Default::default()
        },
        EmptyFilterRowWhereInput {
            wide: Some(Int64Filter {
                not_in: Some(vec![]),
                ..Default::default()
            }),
            ..Default::default()
        },
        EmptyFilterRowWhereInput {
            uid: Some(UuidFilter {
                not_in: Some(vec![]),
                ..Default::default()
            }),
            ..Default::default()
        },
        EmptyFilterRowWhereInput {
            bytes: Some(BytesFilter {
                not_in: Some(vec![]),
                ..Default::default()
            }),
            ..Default::default()
        },
    ];
    for filter in filters {
        assert!(filter.to_sql_conditions().0.is_empty());
        assert!(filter.matches_entity(&row()).unwrap());
    }
}
#[test]
fn an_empty_or_branch_is_true_and_keeps_bind_positions() {
    let filter = EmptyFilterRowWhereInput {
        or: Some(vec![
            EmptyFilterRowWhereInput {
                text: Some(StringFilter {
                    eq: Some("other".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Default::default(),
        ]),
        ..Default::default()
    };
    let (sql, binds) = filter.to_sql_conditions();
    assert!(sql.join(" ").contains(" OR 1 = 1"));
    assert_eq!(binds.len(), 1);
    assert!(filter.matches_entity(&row()).unwrap());
    let all_empty = EmptyFilterRowWhereInput {
        or: Some(vec![Default::default(), Default::default()]),
        ..Default::default()
    };
    assert_eq!(all_empty.to_sql_conditions().0, ["(1 = 1 OR 1 = 1)"]);
    assert!(all_empty.matches_entity(&row()).unwrap());
}
#[cfg(feature = "sqlite")]
#[tokio::test]
async fn bounded_delete_with_empty_inclusion_leaves_rows_intact()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = graphql_orm::sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    graphql_orm::sqlx::query("CREATE TABLE empty_filter_rows (id TEXT PRIMARY KEY, text TEXT, number INTEGER, wide INTEGER, uid TEXT, bytes BLOB)").execute(&pool).await?;
    graphql_orm::sqlx::query("INSERT INTO empty_filter_rows VALUES ('row', 'visible', 1, 1, '00000000-0000-0000-0000-000000000000', X'01')").execute(&pool).await?;
    let db = Database::new(pool.clone());
    for filter in filters() {
        let result =
            EmptyFilterRow::delete_where_bounded(&db, filter, MutationLimit::new(1)?).await?;
        assert_eq!(result, BoundedMutationOutcome::Applied { affected: 0 });
    }
    let count: i64 = graphql_orm::sqlx::query_scalar("SELECT COUNT(*) FROM empty_filter_rows")
        .fetch_one(&pool)
        .await?;
    assert_eq!(count, 1);
    Ok(())
}

#[cfg(feature = "mssql")]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

#[cfg(any(feature = "postgres", feature = "mssql"))]
async fn verify_owned_database(db: Database) -> Result<(), Box<dyn std::error::Error>> {
    for filter in filters() {
        assert_eq!(
            EmptyFilterRow::count_query(db.pool())
                .filter(&filter)
                .count()
                .await?,
            0
        );
        assert!(!filter.matches_entity(&row())?);
        assert_eq!(
            EmptyFilterRow::delete_where_bounded(&db, filter, MutationLimit::new(1)?).await?,
            BoundedMutationOutcome::Applied { affected: 0 }
        );
    }
    assert_eq!(EmptyFilterRow::count_query(db.pool()).count().await?, 1);
    Ok(())
}
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn postgres_empty_lists_preserve_rows() -> Result<(), Box<dyn std::error::Error>> {
    let mut owned = owned_postgres::OwnedPostgres::start("empty-filters")?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&owned.url)
        .await?;
    sqlx::query("CREATE TABLE empty_filter_rows (id TEXT PRIMARY KEY, text TEXT NOT NULL, number INTEGER NOT NULL, wide BIGINT NOT NULL, uid UUID NOT NULL, bytes BYTEA NOT NULL)").execute(&pool).await?;
    sqlx::query("INSERT INTO empty_filter_rows VALUES ('row', 'visible', 1, 1, '00000000-0000-0000-0000-000000000000', '\\x01')").execute(&pool).await?;
    let result = verify_owned_database(Database::new(pool)).await;
    owned.cleanup()?;
    result
}
#[cfg(feature = "mssql")]
#[tokio::test]
#[ignore = "requires test-owned SQL Server container"]
async fn mssql_empty_lists_preserve_rows() -> Result<(), Box<dyn std::error::Error>> {
    let mut owner = owned_mssql::OwnedSqlServer::start().await?;
    owned_mssql::execute_batch(&owner.connection_string, "CREATE TABLE empty_filter_rows (id NVARCHAR(64) PRIMARY KEY, text NVARCHAR(64) NOT NULL, number INT NOT NULL, wide BIGINT NOT NULL, uid UNIQUEIDENTIFIER NOT NULL, bytes VARBINARY(64) NOT NULL); INSERT INTO empty_filter_rows VALUES ('row', 'visible', 1, 1, '00000000-0000-0000-0000-000000000000', 0x01);").await?;
    let pool =
        graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(&owner.connection_string)
            .await?;
    let db = Database::builder(pool)
        .schema_policy(SchemaPolicy::ExternalWritable)
        .build();
    let result = verify_owned_database(db).await;
    owner.cleanup()?;
    result
}
