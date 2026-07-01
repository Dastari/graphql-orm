#![cfg(any(feature = "sqlite", feature = "postgres"))]

use graphql_orm::prelude::*;

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
schema_roots! {
    query_custom_ops: [],
    entities: [SearchArticle],
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

    let created = SearchArticle::insert(
        &db,
        CreateSearchArticleInput {
            title: "Melbourne Park".to_string(),
            body: Some("A venue for tennis finals".to_string()),
            published: true,
        },
    )
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
