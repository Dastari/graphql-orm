//! Generated root search filters over physical names shared by the native
//! search join. SQL Server uses the same filters with fallback ranking.
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]

#[cfg(feature = "mssql")]
#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
// The native helper normally uses the alias `s`.
#[cfg_attr(
    not(feature = "mssql"),
    graphql_entity(table = "s", auth = "none", default_sort = "__gom_search_pk + 0 DESC")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(table = "s", auth = "none", schema_policy = "external_writable")
)]
#[cfg_attr(
    not(feature = "mssql"),
    graphql_orm(search(index = true, fallback = "disabled"))
)]
struct SearchFilterRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: i64,
    #[graphql_orm(db_column = "__gom_search_pk")]
    #[filterable(type = "int64")]
    category: i64,
    #[graphql_orm(db_column = "__gom_search_score")]
    #[filterable(type = "int64")]
    priority: i64,
    #[graphql_orm(searchable(weight = "A"))]
    #[filterable(type = "string")]
    title: String,
}

#[cfg(not(feature = "mssql"))]
schema_roots! { query_custom_ops: [], entities: [SearchFilterRecord] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [SearchFilterRecord] }

#[test]
fn generated_search_sql_keeps_ordinary_filter_operators_and_binds() {
    let filter = SearchFilterRecordWhereInput {
        title: Some(StringFilter {
            eq: Some("ordinary".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let dialect = current_backend();
    let table = dialect.render_identifier_path(SearchFilterRecord::TABLE_NAME);
    let (clauses, values) = filter.to_sql_search_conditions(dialect, &table);
    let (expected_search, expected_non_search) = match dialect {
        DatabaseBackend::Sqlite => ("s.\"title\" = ?", "title = ?"),
        DatabaseBackend::Postgres => ("s.\"title\" = $1", "title = $1"),
        DatabaseBackend::Mssql => ("[s].[title] = @P1", "[title] = @P1"),
        DatabaseBackend::Mysql => unreachable!(),
    };
    let (non_search_clauses, non_search_values) = filter.to_sql_conditions();
    assert_eq!(clauses, [expected_search]);
    assert_eq!(non_search_clauses, [expected_non_search]);
    assert_eq!(values, non_search_values);
}

#[tokio::test]
async fn root_search_filters_bind_to_entity_columns() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    #[cfg(feature = "postgres")]
    let db = Database::new(owned_postgres::OwnedPostgres::pool("search-filter-binding", 2).await?);
    #[cfg(feature = "mssql")]
    let (db, mut owner) = {
        let owner = owned_mssql::OwnedSqlServer::start().await?;
        owned_mssql::execute_batch(
            &owner.connection_string,
            "CREATE TABLE s (id BIGINT PRIMARY KEY, __gom_search_pk BIGINT NOT NULL, \
             __gom_search_score BIGINT NOT NULL, title NVARCHAR(128) NOT NULL);",
        )
        .await?;
        let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(
            &owner.connection_string,
        )
        .await?;
        (
            Database::builder(pool)
                .schema_policy(SchemaPolicy::ExternalWritable)
                .build()
                .with_authorized_scan_config(AuthorizedScanConfig::new([17; 32], 4, 20)),
            owner,
        )
    };

    #[cfg(not(feature = "mssql"))]
    {
        let plan = db
            .schema()
            .plan_migration_to_entities(
                "search-filter-init",
                "search filter binding test",
                &[SearchFilterRecord::metadata()],
            )
            .await?;
        db.schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await?;
    }
    // The matching category is different from the document's key (id) and
    // the entity's priority is different from the document's ranking score.
    for (id, category, priority) in [(11, 7, 13), (7, 9, 13), (12, 7, 42)] {
        SearchFilterRecord::insert(
            &db,
            CreateSearchFilterRecordInput {
                id,
                category,
                priority,
                title: "identical search text".into(),
            },
        )
        .await?;
    }

    let schema = schema_builder(db.clone()).finish();
    #[cfg(not(feature = "mssql"))]
    {
        // Equal search scores leave the entity's default sort to choose the first row.
        let response = schema
            .execute(
                r#"{ searchFilterRecordsSearch(search: {query: "identical"}, page: {limit: 1}) {
                    edges { node { id } }
                    pageInfo { totalCount }
                } }"#,
            )
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let json = response.data.into_json()?;
        let result = &json["searchFilterRecordsSearch"];
        assert_eq!(result["pageInfo"]["totalCount"], 3);
        assert_eq!(result["edges"][0]["node"]["id"], 7);
    }
    let response = schema
        .execute(
            r#"{
                searchFilterRecordsSearch(
                    search: {query: "identical"}
                    where: {and: [{or: [{category: {eq: 7}}, {category: {eq: 99}}]},
                                  {not: {priority: {eq: 42}}}]}
                ) {
                    edges { score node { id category priority } }
                    pageInfo { totalCount }
                }
            }"#,
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let json = response.data.into_json()?;
    let result = &json["searchFilterRecordsSearch"];
    assert_eq!(result["pageInfo"]["totalCount"], 1);
    let edges = result["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(
        edges[0]["node"],
        graphql_orm::serde_json::json!({"id": 11, "category": 7, "priority": 13})
    );
    let score = edges[0]["score"].as_f64().unwrap();
    assert!(score > 0.0 && score != 13.0, "{score}");
    drop(schema);
    #[cfg(not(feature = "mssql"))]
    db.pool().close().await;
    #[cfg(feature = "mssql")]
    {
        drop(db);
        owner.cleanup()?;
    }
    Ok(())
}
