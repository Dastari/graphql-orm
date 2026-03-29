use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "schema_only_parents",
    plural = "SchemaOnlyParents",
    default_sort = "name ASC"
)]
struct SchemaOnlyParentV1 {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,
}

#[derive(GraphQLSchemaEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "schema_only_children",
    plural = "SchemaOnlyChildren",
    default_sort = "id ASC"
)]
struct SchemaOnlyChildV1 {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub parent_id: String,

    #[graphql(skip)]
    #[relation(target = "SchemaOnlyParentV1", from = "parent_id", to = "id")]
    pub parent_relation: Option<String>,
}

#[derive(GraphQLSchemaEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "schema_only_parents",
    plural = "SchemaOnlyParents",
    default_sort = "name ASC"
)]
struct SchemaOnlyParentV2 {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,
}

#[derive(GraphQLSchemaEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "schema_only_children",
    plural = "SchemaOnlyChildren",
    default_sort = "id ASC"
)]
struct SchemaOnlyChildV2 {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub parent_id: String,

    #[graphql(skip)]
    #[relation(
        target = "SchemaOnlyParentV2",
        from = "parent_id",
        to = "id",
        on_delete = "cascade"
    )]
    pub parent_relation: Option<String>,
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
    for table in ["schema_only_children", "schema_only_parents"] {
        sqlx::query(&format!("DROP TABLE IF EXISTS {table} CASCADE"))
            .execute(&pool)
            .await?;
    }
    Ok(pool)
}

#[cfg(feature = "sqlite")]
async fn insert_parent(pool: &sqlx::SqlitePool, id: &str, name: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO schema_only_parents (id, name) VALUES (?, ?)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn insert_parent(pool: &sqlx::PgPool, id: &str, name: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO schema_only_parents (id, name) VALUES ($1, $2)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn insert_child(
    pool: &sqlx::SqlitePool,
    id: &str,
    parent_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO schema_only_children (id, parent_id) VALUES (?, ?)")
        .bind(id)
        .bind(parent_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn insert_child(pool: &sqlx::PgPool, id: &str, parent_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO schema_only_children (id, parent_id) VALUES ($1, $2)")
        .bind(id)
        .bind(parent_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn delete_parent(pool: &sqlx::SqlitePool, id: &str) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query("DELETE FROM schema_only_parents WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

#[cfg(feature = "postgres")]
async fn delete_parent(pool: &sqlx::PgPool, id: &str) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query("DELETE FROM schema_only_parents WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

#[cfg(feature = "sqlite")]
async fn child_exists(pool: &sqlx::SqlitePool, id: &str) -> Result<bool, sqlx::Error> {
    let row = sqlx::query("SELECT COUNT(*) AS count FROM schema_only_children WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await?;
    Ok(sqlx::Row::try_get::<i64, _>(&row, "count")? > 0)
}

#[cfg(feature = "postgres")]
async fn child_exists(pool: &sqlx::PgPool, id: &str) -> Result<bool, sqlx::Error> {
    let row = sqlx::query("SELECT COUNT(*) AS count FROM schema_only_children WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await?;
    Ok(sqlx::Row::try_get::<i64, _>(&row, "count")? > 0)
}

#[tokio::test]
async fn schema_only_entities_drive_staged_relation_delete_policy_migrations()
-> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::{
        DeletePolicy, Entity, SchemaStage, SchemaStageRunner, introspect_schema,
    };

    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    let version_prefix = format!(
        "20260329_schema_only_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );

    let stages = vec![
        SchemaStage::from_entities(
            format!("{version_prefix}_01"),
            "restrict_fk",
            &[
                <SchemaOnlyParentV1 as Entity>::metadata(),
                <SchemaOnlyChildV1 as Entity>::metadata(),
            ],
        ),
        SchemaStage::from_entities(
            format!("{version_prefix}_02"),
            "cascade_fk",
            &[
                <SchemaOnlyParentV2 as Entity>::metadata(),
                <SchemaOnlyChildV2 as Entity>::metadata(),
            ],
        ),
    ];

    database.apply_schema_stages(&stages[..1]).await?;
    let before = introspect_schema(&pool).await?;
    let before_fk = before
        .tables
        .iter()
        .find(|table| table.table_name == "schema_only_children")
        .and_then(|table| table.foreign_keys.first())
        .ok_or("missing schema-only fk before upgrade")?;
    assert_eq!(before_fk.on_delete, DeletePolicy::Restrict);

    database.apply_schema_stages(&stages).await?;
    let after = introspect_schema(&pool).await?;
    let after_fk = after
        .tables
        .iter()
        .find(|table| table.table_name == "schema_only_children")
        .and_then(|table| table.foreign_keys.first())
        .ok_or("missing schema-only fk after upgrade")?;
    assert_eq!(after_fk.on_delete, DeletePolicy::Cascade);

    insert_parent(&pool, "parent_1", "Parent").await?;
    insert_child(&pool, "child_1", "parent_1").await?;
    assert_eq!(delete_parent(&pool, "parent_1").await?, 1);
    assert!(!child_exists(&pool, "child_1").await?);

    let planned = database.plan_schema_stages(&stages).await?;
    assert!(planned.is_empty());

    Ok(())
}
