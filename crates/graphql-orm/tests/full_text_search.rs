// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "postgres"))]

use graphql_orm::prelude::*;

#[cfg(feature = "sqlite")]
mod rebuild_visibility {
    use super::*;

    struct PublishedRows;

    impl RowPolicy for PublishedRows {
        fn read_visibility<'a>(
            &'a self,
            _: Option<&'a async_graphql::Context<'_>>,
            _: &'a Database,
            _: &'static str,
            _: Option<&'static str>,
            surface: EntityAccessSurface,
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>>
        {
            Box::pin(async move {
                assert_eq!(surface, EntityAccessSurface::Repository);
                let filter = SearchArticleWhereInput {
                    published: Some(BoolFilter {
                        eq: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                };
                Ok(ReadVisibility::Complete(ReadPredicate::from_filter::<
                    SqliteBackend,
                    _,
                >(&filter)?))
            })
        }

        fn can_read_row<'a>(
            &'a self,
            _: Option<&'a async_graphql::Context<'_>>,
            db: &'a Database,
            _: &'static str,
            _: Option<&'static str>,
            _: EntityAccessSurface,
            _: &'a (dyn std::any::Any + Send + Sync),
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
            // This provider relies on its SQL predicate for visibility.
            Box::pin(async move {
                graphql_orm::sqlx::query("SELECT 1")
                    .execute(db.pool())
                    .await?;
                Ok(true)
            })
        }

        fn can_write_row<'a>(
            &'a self,
            _: Option<&'a async_graphql::Context<'_>>,
            _: &'a Database,
            _: &'static str,
            _: Option<&'static str>,
            _: EntityAccessSurface,
            _: &'a (dyn std::any::Any + Send + Sync),
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async { Ok(true) })
        }
    }

    #[tokio::test]
    async fn single_document_rebuild_preserves_repository_read_visibility()
    -> Result<(), Box<dyn std::error::Error>> {
        let pool = graphql_orm::sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await?;
        let plan = build_migration_plan(
            DatabaseBackend::Sqlite,
            &SchemaModel {
                extensions: Vec::new(),
                tables: Vec::new(),
            },
            &SchemaModel::from_entities(&[SearchArticle::metadata()]),
        );
        for statement in &plan.statements {
            graphql_orm::sqlx::query(statement).execute(&pool).await?;
        }
        graphql_orm::sqlx::query(
            "INSERT INTO search_articles (id, title, body, published) VALUES
             ('hidden', 'Private draft', NULL, 0), ('visible', 'Published note', NULL, 1)",
        )
        .execute(&pool)
        .await?;
        let db = Database::<SqliteBackend>::with_row_policy(pool, PublishedRows);
        let count_sql = format!(
            "SELECT COUNT(*) FROM \"{}\"",
            graphql_orm::graphql::orm::sqlite_fts_table_name("search_articles")
        );
        SearchArticle::rebuild_search_document(&db, &"hidden".into()).await?;
        let hidden_count: i64 = graphql_orm::sqlx::query_scalar(&count_sql)
            .fetch_one(db.pool())
            .await?;
        assert_eq!(hidden_count, 0, "a hidden row must not be indexed");

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            SearchArticle::rebuild_search_document(&db, &"visible".into()),
        )
        .await
        .expect("host callbacks must run without holding the pool connection")?;
        let visible_count: i64 = graphql_orm::sqlx::query_scalar(&count_sql)
            .fetch_one(db.pool())
            .await?;
        assert_eq!(visible_count, 1);
        Ok(())
    }
}

#[cfg(feature = "sqlite")]
#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "search_articles",
    plural = "SearchArticles",
    backend = "sqlite"
)]
#[graphql_orm(search(index = true, tokenizer = "unicode61", min_token_len = 2))]
struct SearchArticle {
    #[primary_key]
    #[filterable(type = "string")]
    pub id: String,

    #[graphql_orm(searchable(weight = "A"))]
    #[filterable(type = "string")]
    #[sortable]
    pub title: String,

    #[graphql_orm(searchable(weight = "B"))]
    pub body: Option<String>,

    #[graphql_orm(boolean_field)]
    #[filterable(type = "boolean")]
    pub published: bool,
}

#[cfg(feature = "sqlite")]
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Default)]
struct JsonSearchSummary {
    pub summary: Option<String>,
}

#[cfg(feature = "sqlite")]
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Default)]
struct JsonSearchLabel {
    pub label: Option<String>,
}

#[cfg(feature = "sqlite")]
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Default)]
struct JsonSearchClassification {
    pub primary: Option<JsonSearchLabel>,
    pub secondary: Option<JsonSearchLabel>,
    pub keywords: Vec<JsonSearchLabel>,
}

#[cfg(feature = "sqlite")]
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Default)]
struct JsonSearchContent {
    pub description: Option<JsonSearchSummary>,
    pub historical: Option<JsonSearchSummary>,
    pub classification: JsonSearchClassification,
}

#[cfg(feature = "sqlite")]
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Default)]
struct JsonSearchTag {
    pub value: Option<String>,
}

// Searchable JSON is readable; these test values have explicit output scalars.
#[cfg(feature = "sqlite")]
graphql_orm::async_graphql::scalar!(JsonSearchContent);
#[cfg(feature = "sqlite")]
graphql_orm::async_graphql::scalar!(JsonSearchTag);

#[cfg(feature = "sqlite")]
#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "json_search_records",
    plural = "JsonSearchRecords",
    backend = "sqlite"
)]
#[graphql_orm(search(index = true, tokenizer = "unicode61", min_token_len = 2))]
struct JsonSearchRecord {
    #[primary_key]
    #[filterable(type = "string")]
    pub id: String,

    #[graphql_orm(searchable(weight = "A"))]
    #[sortable]
    pub display_title: String,

    #[graphql_orm(json, filter = false, order = false, subscribe = false)]
    #[graphql_orm(search_json(path = "$.description.summary", weight = "C"))]
    #[graphql_orm(search_json(path = "$.historical.summary", weight = "C"))]
    #[graphql_orm(search_json(path = "$.classification.primary.label", weight = "B"))]
    #[graphql_orm(search_json(path = "$.classification.secondary.label", weight = "B"))]
    #[graphql_orm(search_json(path = "$.classification.keywords[*].label", weight = "C"))]
    pub content: JsonSearchContent,

    #[graphql_orm(json, filter = false, order = false, subscribe = false)]
    #[graphql_orm(search_json(path = "$[*].value", weight = "C"))]
    pub tags: Vec<JsonSearchTag>,
}

#[cfg(feature = "sqlite")]
schema_roots! {
    query_custom_ops: [],
    entities: [SearchArticle, JsonSearchRecord],
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn generated_sdl_includes_search_resolver_and_types() {
    let pool =
        graphql_orm::sqlx::SqlitePool::connect_lazy("sqlite::memory:").expect("lazy sqlite pool");
    let database = graphql_orm::db::Database::<graphql_orm::SqliteBackend>::new(pool);
    let schema = schema_builder(database).finish();
    let sdl = schema.sdl();

    assert!(sdl.contains("input SearchInput"));
    assert!(sdl.contains("enum SearchMode"));
    assert!(sdl.contains("type SearchArticleSearchEdge"));
    assert!(sdl.contains("score: Float!"));
    assert!(sdl.contains("searchArticlesSearch("));
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn portable_search_helper_scores_and_filters() -> Result<(), Box<dyn std::error::Error>> {
    let pool = graphql_orm::sqlx::SqlitePool::connect("sqlite::memory:").await?;
    graphql_orm::sqlx::query(
        "CREATE TABLE search_articles (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            body TEXT,
            published INTEGER NOT NULL
        )",
    )
    .execute(&pool)
    .await?;
    graphql_orm::sqlx::query(
        "INSERT INTO search_articles (id, title, body, published) VALUES
            ('1', 'Melbourne Park', 'Tennis and concerts beside the city', 1),
            ('2', 'Park planning notes', 'Melbourne transport draft', 0),
            ('3', 'Sydney Harbour', 'Ferry and bridge guide', 1)",
    )
    .execute(&pool)
    .await?;

    let hits = SearchArticle::search(
        &pool,
        SearchInput {
            query: "melbourne park".to_string(),
            mode: Some(SearchMode::Plain),
            min_score: None,
        },
    )
    .filter(SearchArticleWhereInput {
        published: Some(BoolFilter {
            eq: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    })
    .fetch_all()
    .await?;

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entity.id, "1");
    assert!(hits[0].score > 1.0);
    Ok(())
}

#[cfg(feature = "sqlite")]
fn json_record_content(description: &str, historical: Option<&str>) -> JsonSearchContent {
    JsonSearchContent {
        description: Some(JsonSearchSummary {
            summary: Some(description.to_string()),
        }),
        historical: Some(JsonSearchSummary {
            summary: historical.map(str::to_string),
        }),
        classification: JsonSearchClassification {
            primary: Some(JsonSearchLabel {
                label: Some("Architecture".to_string()),
            }),
            secondary: Some(JsonSearchLabel {
                label: Some("Civic".to_string()),
            }),
            keywords: vec![
                JsonSearchLabel {
                    label: Some("heritage".to_string()),
                },
                JsonSearchLabel {
                    label: Some("landmark".to_string()),
                },
                JsonSearchLabel { label: None },
            ],
        },
    }
}

#[cfg(feature = "sqlite")]
#[test]
fn search_document_includes_json_path_chunks() {
    let record = JsonSearchRecord {
        id: "rec-1".to_string(),
        display_title: "Town Hall".to_string(),
        content: json_record_content("Federation-era civic chambers", Some("Opened in 1908")),
        tags: vec![
            JsonSearchTag {
                value: Some("council".to_string()),
            },
            JsonSearchTag {
                value: Some("archive".to_string()),
            },
        ],
    };

    let index = JsonSearchRecord::search_index().expect("json search index");
    assert_eq!(index.fields.len(), 1);
    assert_eq!(index.json_paths.len(), 6);
    assert_eq!(index.json_paths[0].path, "$.description.summary");

    let document = record.search_document();
    assert!(document.chunks.iter().any(|chunk| {
        chunk.source
            == SearchDocumentSource::Field {
                field_name: "display_title",
            }
            && chunk.weight == SearchWeight::A
            && chunk.text == "Town Hall"
    }));
    assert!(document.chunks.iter().any(|chunk| {
        chunk.source
            == SearchDocumentSource::JsonPath {
                field_name: "content",
                path: "$.classification.primary.label",
            }
            && chunk.weight == SearchWeight::B
            && chunk.text == "Architecture"
    }));
    assert!(document.chunks.iter().any(|chunk| {
        chunk.source
            == SearchDocumentSource::JsonPath {
                field_name: "content",
                path: "$.classification.keywords[*].label",
            }
            && chunk.weight == SearchWeight::C
            && chunk.text == "heritage landmark"
    }));
    assert!(document.chunks.iter().any(|chunk| {
        chunk.source
            == SearchDocumentSource::JsonPath {
                field_name: "tags",
                path: "$[*].value",
            }
            && chunk.weight == SearchWeight::C
            && chunk.text == "council archive"
    }));
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_json_search_paths_are_maintained_by_writes_and_rebuilds()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = graphql_orm::sqlx::SqlitePool::connect("sqlite::memory:").await?;
    let target = SchemaModel::from_entities(&[JsonSearchRecord::metadata()]);
    let plan = build_migration_plan(
        DatabaseBackend::Sqlite,
        &SchemaModel {
            extensions: Vec::new(),
            tables: Vec::new(),
        },
        &target,
    );
    for statement in &plan.statements {
        graphql_orm::sqlx::query(statement).execute(&pool).await?;
    }
    let table = target
        .tables
        .iter()
        .find(|table| table.table_name == "json_search_records")
        .expect("json search table metadata");
    assert_eq!(table.search_indexes.len(), 1);
    assert_eq!(table.search_indexes[0].json_paths.len(), 6);

    let db = graphql_orm::db::Database::<graphql_orm::SqliteBackend>::new(pool.clone());
    let created = JsonSearchRecord::insert(
        &db,
        CreateJsonSearchRecordInput {
            display_title: "Town Hall".to_string(),
            content: json_record_content("Federation-era civic chambers", None),
            tags: vec![JsonSearchTag {
                value: Some("council".to_string()),
            }],
        },
    )
    .await?;
    JsonSearchRecord::insert(
        &db,
        CreateJsonSearchRecordInput {
            display_title: "Harbour Shed".to_string(),
            content: JsonSearchContent {
                description: Some(JsonSearchSummary {
                    summary: Some("Working waterfront storage".to_string()),
                }),
                historical: None,
                classification: JsonSearchClassification {
                    primary: Some(JsonSearchLabel {
                        label: Some("Industrial".to_string()),
                    }),
                    secondary: None,
                    keywords: vec![JsonSearchLabel {
                        label: Some("maritime".to_string()),
                    }],
                },
            },
            tags: vec![JsonSearchTag {
                value: Some("dock".to_string()),
            }],
        },
    )
    .await?;

    let hits = JsonSearchRecord::search(
        db.pool(),
        SearchInput {
            query: "federation".to_string(),
            mode: Some(SearchMode::Plain),
            min_score: None,
        },
    )
    .fetch_all()
    .await?;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entity.id, created.id);

    let tag_hits = JsonSearchRecord::search(
        db.pool(),
        SearchInput {
            query: "council".to_string(),
            mode: Some(SearchMode::Plain),
            min_score: None,
        },
    )
    .fetch_all()
    .await?;
    assert_eq!(tag_hits.len(), 1);
    assert_eq!(tag_hits[0].entity.id, created.id);

    JsonSearchRecord::update_by_id(
        &db,
        &created.id,
        UpdateJsonSearchRecordInput {
            display_title: None,
            content: Some(json_record_content(
                "Art deco council rooms",
                Some("Restored archive"),
            )),
            tags: Some(vec![JsonSearchTag {
                value: Some("restored".to_string()),
            }]),
        },
    )
    .await?;

    let old_hits = JsonSearchRecord::search(
        db.pool(),
        SearchInput {
            query: "federation".to_string(),
            mode: Some(SearchMode::Plain),
            min_score: None,
        },
    )
    .fetch_all()
    .await?;
    assert!(old_hits.is_empty());

    JsonSearchRecord::rebuild_search_index(&db).await?;
    let rebuilt_hits = JsonSearchRecord::search(
        db.pool(),
        SearchInput {
            query: "archive restored".to_string(),
            mode: Some(SearchMode::Plain),
            min_score: None,
        },
    )
    .fetch_all()
    .await?;
    assert_eq!(rebuilt_hits.len(), 1);
    assert_eq!(rebuilt_hits[0].entity.id, created.id);
    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_managed_fts_is_maintained_by_orm_writes() -> Result<(), Box<dyn std::error::Error>>
{
    let pool = graphql_orm::sqlx::SqlitePool::connect("sqlite::memory:").await?;
    let target = SchemaModel::from_entities(&[SearchArticle::metadata()]);
    let plan = build_migration_plan(
        DatabaseBackend::Sqlite,
        &SchemaModel {
            extensions: Vec::new(),
            tables: Vec::new(),
        },
        &target,
    );
    for statement in &plan.statements {
        graphql_orm::sqlx::query(statement).execute(&pool).await?;
    }
    let introspected =
        graphql_orm::graphql::orm::introspect_schema::<graphql_orm::SqliteBackend, _>(&pool)
            .await?;
    let introspected_table = introspected
        .tables
        .iter()
        .find(|table| table.table_name == "search_articles")
        .expect("introspected search_articles table");
    assert_eq!(introspected_table.search_indexes.len(), 1);
    assert_eq!(
        introspected_table.search_indexes[0].strategy,
        SearchIndexStrategy::SqliteFts5
    );

    let db = graphql_orm::db::Database::<graphql_orm::SqliteBackend>::new(pool.clone());

    let created = db
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                tx.insert::<SearchArticle>(CreateSearchArticleInput {
                    title: "Melbourne Park".to_string(),
                    body: Some("A venue for tennis finals".to_string()),
                    published: true,
                })
                .await
                .map_err(Into::into)
            })
        })
        .await?;
    SearchArticle::insert(
        &db,
        CreateSearchArticleInput {
            title: "Sydney Harbour".to_string(),
            body: Some("Ferries and bridge walks".to_string()),
            published: true,
        },
    )
    .await?;

    let hits = SearchArticle::search(
        db.pool(),
        SearchInput {
            query: "melbourne".to_string(),
            mode: Some(SearchMode::Plain),
            min_score: None,
        },
    )
    .fetch_all()
    .await?;

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entity.id, created.id);

    SearchArticle::insert(
        &db,
        CreateSearchArticleInput {
            title: "Melbourne Markets".to_string(),
            body: Some("Food and events".to_string()),
            published: true,
        },
    )
    .await?;
    let limited_hits = SearchArticle::search(
        db.pool(),
        SearchInput {
            query: "melbourne".to_string(),
            mode: Some(SearchMode::Plain),
            min_score: None,
        },
    )
    .limit(1)
    .fetch_all()
    .await?;
    assert_eq!(limited_hits.len(), 1);

    SearchArticle::rebuild_search_index(&db).await?;
    let rebuilt_hits = SearchArticle::search(
        db.pool(),
        SearchInput {
            query: "tennis".to_string(),
            mode: Some(SearchMode::Plain),
            min_score: None,
        },
    )
    .fetch_all()
    .await?;
    assert_eq!(rebuilt_hits.len(), 1);
    assert_eq!(rebuilt_hits[0].entity.id, created.id);
    Ok(())
}

#[cfg(feature = "sqlite")]
#[test]
fn sqlite_search_schema_plans_fts5_table() {
    let target = SchemaModel::from_entities(&[SearchArticle::metadata()]);
    let table = target
        .tables
        .iter()
        .find(|table| table.table_name == "search_articles")
        .expect("search table metadata");
    assert_eq!(table.search_indexes.len(), 1);
    assert_eq!(
        table.search_indexes[0].strategy,
        SearchIndexStrategy::SqliteFts5
    );

    let plan = build_migration_plan(
        DatabaseBackend::Sqlite,
        &SchemaModel {
            extensions: Vec::new(),
            tables: Vec::new(),
        },
        &target,
    );
    assert!(plan.statements.iter().any(|statement| {
        statement.contains("CREATE VIRTUAL TABLE __graphql_orm_fts_search_articles")
            && statement.contains("USING fts5")
    }));
    assert!(
        plan.statements
            .iter()
            .any(|statement| statement.contains("__graphql_orm_search_metadata"))
    );
}

#[cfg(feature = "postgres")]
#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "pg_search_articles",
    plural = "PgSearchArticles",
    backend = "postgres"
)]
#[graphql_orm(search(index = true, language = "english"))]
struct PgSearchArticle {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[graphql_orm(searchable(weight = "A"))]
    #[sortable]
    pub title: String,

    #[graphql_orm(searchable(weight = "B"))]
    pub body: Option<String>,
}

#[cfg(feature = "postgres")]
#[test]
fn postgres_search_schema_plans_shadow_table_and_gin_index() {
    let target = SchemaModel::from_entities(&[PgSearchArticle::metadata()]);
    let table = target
        .tables
        .iter()
        .find(|table| table.table_name == "pg_search_articles")
        .expect("search table metadata");
    assert_eq!(table.search_indexes.len(), 1);
    assert_eq!(
        table.search_indexes[0].strategy,
        SearchIndexStrategy::PostgresTsvector
    );

    let plan = build_migration_plan(
        DatabaseBackend::Postgres,
        &SchemaModel {
            extensions: Vec::new(),
            tables: Vec::new(),
        },
        &target,
    );
    assert!(plan.statements.iter().any(|statement| {
        statement.contains("CREATE TABLE __graphql_orm_search_pg_search_articles")
            && statement.contains("document_vector TSVECTOR")
    }));
    assert!(plan.statements.iter().any(|statement| {
        statement.contains("USING GIN (document_vector)")
            && statement.contains("idx_gom_search_pg_search_articles_vector")
    }));
}

#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        table = "sort_collisions",
        plural = "SortCollisions",
        auth = "none",
        backend = "sqlite",
        default_sort = "rank ASC"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        table = "sort_collisions",
        plural = "SortCollisions",
        auth = "none",
        backend = "postgres",
        default_sort = "entity_pk ASC"
    )
)]
#[graphql_orm(search(index = true))]
struct SortCollision {
    #[primary_key]
    pub id: String,
    #[sortable]
    #[filterable(type = "int64")]
    pub rank: i64,
    #[sortable]
    #[filterable(type = "int64")]
    pub entity_pk: i64,
    #[graphql_orm(searchable(weight = "A"))]
    pub title: String,
}

mod collision_schema {
    use super::*;
    schema_roots! { query_custom_ops: [], entities: [SortCollision], }
}

#[tokio::test]
#[cfg_attr(feature = "postgres", ignore = "owned PostgreSQL lane")]
async fn native_search_sort_collisions_repository() -> Result<(), Box<dyn std::error::Error>> {
    native_search_sort_collisions(false).await
}

#[tokio::test]
#[cfg_attr(feature = "postgres", ignore = "owned PostgreSQL lane")]
async fn native_search_sort_collisions_graphql() -> Result<(), Box<dyn std::error::Error>> {
    native_search_sort_collisions(true).await
}

async fn native_search_sort_collisions(graphql: bool) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    let pool = graphql_orm::sqlx::SqlitePool::connect("sqlite::memory:").await?;
    #[cfg(feature = "postgres")]
    let owned = owned_postgres::OwnedPostgres::start("search-sort-collision")?;
    #[cfg(feature = "postgres")]
    let pool = graphql_orm::sqlx::PgPool::connect(&owned.url).await?;
    #[cfg(feature = "sqlite")]
    type Backend = graphql_orm::SqliteBackend;
    #[cfg(feature = "postgres")]
    type Backend = graphql_orm::PostgresBackend;
    let target = SchemaModel::from_entities(&[SortCollision::metadata()]);
    let plan = build_migration_plan(
        Backend::DIALECT,
        &SchemaModel {
            extensions: vec![],
            tables: vec![],
        },
        &target,
    );
    for statement in &plan.statements {
        graphql_orm::sqlx::query(statement).execute(&pool).await?;
    }
    let db = graphql_orm::db::Database::<Backend>::new(pool.clone());
    for value in [2, 1, 1] {
        SortCollision::insert(
            &db,
            CreateSortCollisionInput {
                rank: value,
                entity_pk: value,
                title: "identical search text".into(),
            },
        )
        .await?;
    }
    if graphql {
        let schema = collision_schema::schema_builder(db).finish();
        let response = schema.execute("{ sortCollisionsSearch(search: {query: \"identical\"}) { edges { node { id rank entityPk } } pageInfo { totalCount } } }").await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let json = response.data.into_json()?;
        let edges = json["sortCollisionsSearch"]["edges"].as_array().unwrap();
        assert_eq!(edges.len(), 3);
        assert_eq!(edges[0]["node"]["rank"], 1);
        assert_eq!(edges[2]["node"]["rank"], 2);
        assert!(edges[0]["node"]["id"].as_str() < edges[1]["node"]["id"].as_str());
        let response = schema.execute("{ sortCollisionsSearch(search: {query: \"identical\"}, where: {entityPk: {eq: 2}}, page: {limit: 1}) { edges { node { rank } } pageInfo { totalCount } } }").await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let json = response.data.into_json()?;
        assert_eq!(json["sortCollisionsSearch"]["edges"][0]["node"]["rank"], 2);
        assert_eq!(json["sortCollisionsSearch"]["pageInfo"]["totalCount"], 1);
    } else {
        let input = SearchInput {
            query: "identical".into(),
            mode: Some(SearchMode::Plain),
            min_score: None,
        };
        let hits = SortCollision::search_db(&db, input.clone())
            .limit(4)
            .fetch_all()
            .await?;
        assert_eq!(
            hits.iter().map(|hit| hit.entity.rank).collect::<Vec<_>>(),
            vec![1, 1, 2]
        );
        assert!(hits[0].entity.id < hits[1].entity.id);
        let filtered = SortCollision::search_db(&db, input)
            .filter(SortCollisionWhereInput {
                entity_pk: Some(Int64Filter {
                    eq: Some(2),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .fetch_all()
            .await?;
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].entity.rank, 2);
    }
    pool.close().await;
    Ok(())
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        table = "expression_articles",
        backend = "sqlite",
        auth = "none",
        default_sort = "(SELECT name FROM sort_authors WHERE sort_authors.id = author_id) ASC, date('2024-01-01') ASC"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        table = "expression_articles",
        backend = "postgres",
        auth = "none",
        default_sort = "(SELECT name FROM sort_authors WHERE sort_authors.id = author_id) ASC, date '2024-01-01' ASC"
    )
)]
#[graphql_orm(search(index = true, fallback = "disabled"))]
struct ExpressionArticle {
    #[primary_key]
    id: String,
    #[sortable]
    name: String,
    date: String,
    author_id: i64,
    #[graphql_orm(searchable(weight = "A"))]
    title: String,
}

#[cfg(feature = "sqlite")]
struct SearchFixtureOwner;
#[cfg(feature = "postgres")]
type SearchFixtureOwner = owned_postgres::OwnedPostgres;

async fn search_fixture() -> Result<(Database, SearchFixtureOwner), Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    let (pool, owner) = (
        sqlx::SqlitePool::connect("sqlite::memory:").await?,
        SearchFixtureOwner,
    );
    #[cfg(feature = "postgres")]
    let (pool, owner) = {
        let owner = SearchFixtureOwner::start("native-search-expressions")?;
        (sqlx::PgPool::connect(&owner.url).await?, owner)
    };
    let target =
        SchemaModel::from_entities(&[ExpressionArticle::metadata(), SortCollision::metadata()]);
    let plan = build_migration_plan(
        DefaultBackend::DIALECT,
        &SchemaModel {
            extensions: vec![],
            tables: vec![],
        },
        &target,
    );
    for statement in &plan.statements {
        sqlx::query(statement).execute(&pool).await?;
    }
    Ok((Database::new(pool), owner))
}

#[tokio::test]
#[cfg_attr(feature = "postgres", ignore = "owned PostgreSQL lane")]
async fn native_search_preserves_default_sort_expressions() -> Result<(), Box<dyn std::error::Error>>
{
    let (db, _owner) = search_fixture().await?;
    sqlx::query("CREATE TABLE sort_authors (id BIGINT PRIMARY KEY, name TEXT NOT NULL)")
        .execute(db.pool())
        .await?;
    sqlx::query("INSERT INTO sort_authors VALUES (1, 'Zulu'), (2, 'Alpha')")
        .execute(db.pool())
        .await?;
    for (name, author_id) in [("Alpha", 1), ("Zulu", 2)] {
        ExpressionArticle::insert(
            &db,
            CreateExpressionArticleInput {
                name: name.into(),
                date: "2020-01-01".into(),
                author_id,
                title: "identical search text".into(),
            },
        )
        .await?;
    }
    let listed = EntityQuery::<ExpressionArticle>::new()
        .default_order()
        .fetch_all(&db)
        .await?;
    assert_eq!(
        listed
            .iter()
            .map(|article| article.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Zulu", "Alpha"]
    );
    let searched = ExpressionArticle::search_db(
        &db,
        SearchInput {
            query: "identical".into(),
            mode: Some(SearchMode::Plain),
            min_score: None,
        },
    )
    .limit(4)
    .fetch_all()
    .await?;
    assert_eq!(
        searched
            .iter()
            .map(|hit| &hit.entity.id)
            .collect::<Vec<_>>(),
        listed.iter().map(|article| &article.id).collect::<Vec<_>>()
    );
    Ok(())
}

struct SearchVisibility;
impl RowPolicy for SearchVisibility {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async {
            Ok(ReadVisibility::Complete(ReadPredicate::column::<
                DefaultBackend,
                SortCollision,
            >(
                "entity_pk",
                ReadPredicateOp::Eq,
                SqlValue::Int(7),
            )?))
        })
    }
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { panic!("complete SQL visibility must not use the row callback") })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

#[tokio::test]
#[cfg_attr(feature = "postgres", ignore = "owned PostgreSQL lane")]
async fn native_search_binds_visibility_filter_and_limit() -> Result<(), Box<dyn std::error::Error>>
{
    let (mut db, _owner) = search_fixture().await?;
    let mut eligible = Vec::new();
    for (rank, entity_pk, title) in [
        (0, 7, "needle text"),
        (16, 7, "needle text"),
        (17, 9, "needle text"),
        (17, 7, "needle text"),
        (18, 7, "needle text"),
        (19, 7, "unmatched text"),
    ] {
        let record = SortCollision::insert(
            &db,
            CreateSortCollisionInput {
                rank,
                entity_pk,
                title: title.into(),
            },
        )
        .await?;
        if entity_pk == 7 && rank >= 17 && title == "needle text" {
            eligible.push(record);
        }
    }
    #[cfg(feature = "sqlite")]
    eligible.sort_by_key(|record| record.rank);
    #[cfg(feature = "postgres")]
    eligible.sort_by(|a, b| a.id.cmp(&b.id));
    db.set_row_policy(SearchVisibility);
    // A portable/callback scan is unavailable, so success requires native SQL.
    assert!(db.authorized_scan_config().is_none());
    let schema = collision_schema::schema_builder(db).finish();
    let response = schema.execute("{ sortCollisionsSearch(search: {query: \"needle\"}, where: {rank: {gte: 17}}, page: {limit: 1}) { edges { score node { id rank } } pageInfo { totalCount hasNextPage } } }").await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let json = response.data.into_json()?;
    let connection = &json["sortCollisionsSearch"];
    assert_eq!(connection["pageInfo"]["totalCount"], 2);
    assert_eq!(connection["pageInfo"]["hasNextPage"], true);
    assert_eq!(connection["edges"].as_array().unwrap().len(), 1);
    assert_eq!(connection["edges"][0]["node"]["id"], eligible[0].id);
    assert!(connection["edges"][0]["score"].as_f64().unwrap() > 0.0);
    Ok(())
}
