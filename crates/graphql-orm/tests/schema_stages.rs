use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "staged_users",
    plural = "StagedUsers",
    default_sort = "name ASC"
)]
struct StagedUser {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "staged_collections",
    plural = "StagedCollections",
    default_sort = "title ASC"
)]
struct StagedCollection {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub title: String,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "staged_records",
    plural = "StagedRecords",
    default_sort = "slug ASC"
)]
struct StagedRecord {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub slug: String,
}

fn schema_stages(version_prefix: &str) -> Vec<graphql_orm::graphql::orm::SchemaStage> {
    use graphql_orm::graphql::orm::{Entity, SchemaStage};

    vec![
        SchemaStage::from_entities(
            format!("{version_prefix}_01"),
            "auth_foundation",
            &[<StagedUser as Entity>::metadata()],
        ),
        SchemaStage::from_entities(
            format!("{version_prefix}_02"),
            "collection_foundation",
            &[
                <StagedUser as Entity>::metadata(),
                <StagedCollection as Entity>::metadata(),
            ],
        ),
        SchemaStage::from_entities(
            format!("{version_prefix}_03"),
            "record_foundation",
            &[
                <StagedUser as Entity>::metadata(),
                <StagedCollection as Entity>::metadata(),
                <StagedRecord as Entity>::metadata(),
            ],
        ),
    ]
}

#[cfg(feature = "sqlite")]
async fn setup_pool() -> Result<sqlx::SqlitePool, Box<dyn std::error::Error>> {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await?;
    Ok(pool)
}

#[cfg(feature = "postgres")]
async fn setup_pool() -> Result<sqlx::PgPool, Box<dyn std::error::Error>> {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:55432/postgres".to_string());
    let pool = sqlx::PgPool::connect(&database_url).await?;
    sqlx::query("DROP TABLE IF EXISTS staged_records")
        .execute(&pool)
        .await?;
    sqlx::query("DROP TABLE IF EXISTS staged_collections")
        .execute(&pool)
        .await?;
    sqlx::query("DROP TABLE IF EXISTS staged_users")
        .execute(&pool)
        .await?;
    Ok(pool)
}

#[cfg(feature = "sqlite")]
async fn applied_stage_count(
    pool: &sqlx::SqlitePool,
    version_prefix: &str,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query(
        "SELECT COUNT(*) AS count
         FROM __graphql_orm_migrations
         WHERE version LIKE ?",
    )
    .bind(format!("{version_prefix}%"))
    .fetch_one(pool)
    .await?;
    sqlx::Row::try_get::<i64, _>(&row, "count")
}

#[cfg(feature = "postgres")]
async fn applied_stage_count(
    pool: &sqlx::PgPool,
    version_prefix: &str,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query(
        "SELECT COUNT(*) AS count
         FROM __graphql_orm_migrations
         WHERE version LIKE $1",
    )
    .bind(format!("{version_prefix}%"))
    .fetch_one(pool)
    .await?;
    sqlx::Row::try_get::<i64, _>(&row, "count")
}

#[tokio::test]
async fn schema_stages_apply_incrementally_and_rerun_cleanly()
-> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::{SchemaStageRunner, introspect_schema};

    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    let version_prefix = format!(
        "20260328_staged_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    let stages = schema_stages(&version_prefix);

    database.apply_schema_stages(&stages[..2]).await?;

    let mid_schema = introspect_schema(&pool).await?;
    assert!(
        mid_schema
            .tables
            .iter()
            .any(|table| table.table_name == "staged_users")
    );
    assert!(
        mid_schema
            .tables
            .iter()
            .any(|table| table.table_name == "staged_collections")
    );
    assert!(
        !mid_schema
            .tables
            .iter()
            .any(|table| table.table_name == "staged_records")
    );
    assert_eq!(applied_stage_count(&pool, &version_prefix).await?, 2);

    database.apply_schema_stages(&stages).await?;

    let final_schema = introspect_schema(&pool).await?;
    assert!(
        final_schema
            .tables
            .iter()
            .any(|table| table.table_name == "staged_users")
    );
    assert!(
        final_schema
            .tables
            .iter()
            .any(|table| table.table_name == "staged_collections")
    );
    assert!(
        final_schema
            .tables
            .iter()
            .any(|table| table.table_name == "staged_records")
    );
    assert_eq!(applied_stage_count(&pool, &version_prefix).await?, 3);

    let planned = database.plan_schema_stages(&stages).await?;
    assert!(planned.is_empty());

    database.apply_schema_stages(&stages).await?;
    assert_eq!(applied_stage_count(&pool, &version_prefix).await?, 3);

    Ok(())
}
