#[cfg(feature = "sqlite")]
use graphql_orm::graphql::orm::{
    ColumnModel, DeletePolicy, ForeignKeyModel, IndexDef, SchemaModel, SchemaStage, TableModel,
};
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

#[cfg(feature = "sqlite")]
fn text_column(name: &str, primary_key: bool) -> ColumnModel {
    ColumnModel {
        name: name.to_string(),
        sql_type: "TEXT".to_string(),
        nullable: false,
        is_primary_key: primary_key,
        is_unique: false,
        default: None,
    }
}

#[cfg(feature = "sqlite")]
fn varchar_column(name: &str, primary_key: bool) -> ColumnModel {
    ColumnModel {
        name: name.to_string(),
        sql_type: if primary_key {
            "TEXT".to_string()
        } else {
            "VARCHAR(255)".to_string()
        },
        nullable: false,
        is_primary_key: primary_key,
        is_unique: false,
        default: None,
    }
}

#[cfg(feature = "sqlite")]
fn sqlite_vocabularies_v1() -> TableModel {
    TableModel {
        entity_name: "Vocabulary".to_string(),
        table_name: "vocabularies".to_string(),
        primary_key: "id".to_string(),
        default_sort: "slug ASC".to_string(),
        columns: vec![text_column("id", true), text_column("slug", false)],
        indexes: vec![IndexDef::new("idx_vocabularies_slug", &["slug"])],
        composite_unique_indexes: vec![],
        foreign_keys: vec![],
    }
}

#[cfg(feature = "sqlite")]
fn sqlite_vocabulary_terms_v1() -> TableModel {
    TableModel {
        entity_name: "VocabularyTerm".to_string(),
        table_name: "vocabulary_terms".to_string(),
        primary_key: "id".to_string(),
        default_sort: "term ASC".to_string(),
        columns: vec![
            text_column("id", true),
            text_column("vocabulary_id", false),
            text_column("term", false),
        ],
        indexes: vec![],
        composite_unique_indexes: vec![],
        foreign_keys: vec![ForeignKeyModel {
            source_column: "vocabulary_id".to_string(),
            target_table: "vocabularies".to_string(),
            target_column: "id".to_string(),
            is_multiple: false,
            on_delete: DeletePolicy::Cascade,
        }],
    }
}

#[cfg(feature = "sqlite")]
fn sqlite_vocabularies_v2() -> TableModel {
    TableModel {
        columns: vec![varchar_column("id", true), varchar_column("slug", false)],
        ..sqlite_vocabularies_v1()
    }
}

#[cfg(feature = "sqlite")]
fn sqlite_vocabulary_terms_v2() -> TableModel {
    TableModel {
        columns: vec![
            varchar_column("id", true),
            varchar_column("vocabulary_id", false),
            varchar_column("term", false),
        ],
        ..sqlite_vocabulary_terms_v1()
    }
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_schema_stage_rebuilds_related_tables_in_one_stage()
-> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::{SchemaStageRunner, introspect_schema};

    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    let version_prefix = format!(
        "20260402_fk_stage_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );

    let stages = vec![
        SchemaStage::from_schema_model(
            format!("{version_prefix}_01"),
            "vocabulary_foundation",
            SchemaModel {
                tables: vec![sqlite_vocabularies_v1(), sqlite_vocabulary_terms_v1()],
            },
        ),
        SchemaStage::from_schema_model(
            format!("{version_prefix}_02"),
            "vocabulary_rebuild",
            SchemaModel {
                tables: vec![sqlite_vocabularies_v2(), sqlite_vocabulary_terms_v2()],
            },
        ),
    ];

    database.apply_schema_stages(&stages[..1]).await?;
    sqlx::query("INSERT INTO vocabularies (id, slug) VALUES ('v1', 'core')")
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO vocabulary_terms (id, vocabulary_id, term) VALUES ('t1', 'v1', 'alpha')",
    )
    .execute(&pool)
    .await?;

    database.apply_schema_stages(&stages).await?;

    let row = sqlx::query(
        "SELECT vocabularies.slug AS slug, vocabulary_terms.term AS term
         FROM vocabularies
         JOIN vocabulary_terms ON vocabulary_terms.vocabulary_id = vocabularies.id
         WHERE vocabularies.id = 'v1'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(sqlx::Row::try_get::<String, _>(&row, "slug")?, "core");
    assert_eq!(sqlx::Row::try_get::<String, _>(&row, "term")?, "alpha");

    let schema = introspect_schema(&pool).await?;
    let terms_table = schema
        .tables
        .iter()
        .find(|table| table.table_name == "vocabulary_terms")
        .expect("vocabulary_terms table should exist");
    assert!(
        terms_table
            .columns
            .iter()
            .any(|column| { column.name == "term" && column.sql_type == "VARCHAR(255)" })
    );
    assert!(terms_table.foreign_keys.iter().any(|foreign_key| {
        foreign_key.source_column == "vocabulary_id"
            && foreign_key.target_table == "vocabularies"
            && foreign_key.target_column == "id"
    }));

    Ok(())
}
