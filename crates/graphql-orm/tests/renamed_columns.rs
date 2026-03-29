use graphql_orm::prelude::*;

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "renamed_accounts",
    plural = "RenamedAccounts",
    default_sort = "email ASC"
)]
struct RenamedAccountV1 {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub email: String,

    #[graphql_orm(json, filter = false, order = false, subscribe = false)]
    pub roles_json: Vec<String>,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "renamed_accounts",
    plural = "RenamedAccounts",
    default_sort = "email ASC"
)]
struct RenamedAccountV2 {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub email: String,

    #[graphql_orm(
        json,
        db_column = "roles_json",
        filter = false,
        order = false,
        subscribe = false
    )]
    pub roles: Vec<String>,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,
}

schema_roots! {
    query_custom_ops: [],
    entities: [RenamedAccountV2],
}

fn staged_versions(prefix: &str) -> Vec<graphql_orm::graphql::orm::SchemaStage> {
    use graphql_orm::graphql::orm::{Entity, SchemaStage};

    vec![
        SchemaStage::from_entities(
            format!("{prefix}_01"),
            "legacy_roles_json",
            &[<RenamedAccountV1 as Entity>::metadata()],
        ),
        SchemaStage::from_entities(
            format!("{prefix}_02"),
            "semantic_roles_field",
            &[<RenamedAccountV2 as Entity>::metadata()],
        ),
    ]
}

#[cfg(feature = "sqlite")]
type TestPool = sqlx::SqlitePool;
#[cfg(feature = "postgres")]
type TestPool = sqlx::PgPool;

#[cfg(feature = "sqlite")]
async fn setup_pool() -> Result<TestPool, Box<dyn std::error::Error>> {
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
async fn setup_pool() -> Result<TestPool, Box<dyn std::error::Error>> {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:55432/postgres".to_string());
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await?;
    sqlx::query("DROP TABLE IF EXISTS renamed_accounts")
        .execute(&pool)
        .await?;
    Ok(pool)
}

#[cfg(feature = "sqlite")]
async fn select_roles_json(
    pool: &sqlx::SqlitePool,
    id: graphql_orm::uuid::Uuid,
) -> Result<String, sqlx::Error> {
    let row = sqlx::query("SELECT roles_json FROM renamed_accounts WHERE id = ?")
        .bind(id.to_string())
        .fetch_one(pool)
        .await?;
    sqlx::Row::try_get::<String, _>(&row, "roles_json")
}

#[cfg(feature = "postgres")]
async fn select_roles_json(
    pool: &sqlx::PgPool,
    id: graphql_orm::uuid::Uuid,
) -> Result<serde_json::Value, sqlx::Error> {
    let row = sqlx::query("SELECT roles_json FROM renamed_accounts WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await?;
    let json: sqlx::types::Json<serde_json::Value> = sqlx::Row::try_get(&row, "roles_json")?;
    Ok(json.0)
}

#[tokio::test]
async fn staged_field_rename_keeps_legacy_db_column_and_preserves_data()
-> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::{SchemaStageRunner, introspect_schema};

    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    let version_prefix = format!(
        "20260329_rename_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    let stages = staged_versions(&version_prefix);

    database.apply_schema_stages(&stages[..1]).await?;

    let created_id = graphql_orm::uuid::Uuid::new_v4();
    #[cfg(feature = "sqlite")]
    sqlx::query("INSERT INTO renamed_accounts (id, email, roles_json, created_at, updated_at) VALUES (?, ?, ?, unixepoch(), unixepoch())")
        .bind(created_id.to_string())
        .bind("owner@example.com")
        .bind("[\"owner\"]")
        .execute(database.pool())
        .await?;
    #[cfg(feature = "postgres")]
    sqlx::query("INSERT INTO renamed_accounts (id, email, roles_json, created_at, updated_at) VALUES ($1, $2, $3, EXTRACT(EPOCH FROM NOW())::bigint, EXTRACT(EPOCH FROM NOW())::bigint)")
        .bind(created_id)
        .bind("owner@example.com")
        .bind(sqlx::types::Json(serde_json::json!(["owner"])))
        .execute(database.pool())
        .await?;

    database.apply_schema_stages(&stages).await?;

    let schema = introspect_schema(&pool).await?;
    let table = schema
        .tables
        .iter()
        .find(|table| table.table_name == "renamed_accounts")
        .expect("renamed_accounts table should exist");
    assert!(
        table
            .columns
            .iter()
            .any(|column| column.name == "roles_json")
    );
    assert!(!table.columns.iter().any(|column| column.name == "roles"));

    let loaded = RenamedAccountV2::get(database.pool(), &created_id)
        .await?
        .expect("renamed account should load");
    assert_eq!(loaded.roles, vec!["owner".to_string()]);

    let updated = RenamedAccountV2::update_by_id(
        &database,
        &created_id,
        UpdateRenamedAccountV2Input {
            roles: Some(vec!["editor".to_string(), "reviewer".to_string()]),
            ..Default::default()
        },
    )
    .await?
    .expect("renamed account should update");
    assert_eq!(
        updated.roles,
        vec!["editor".to_string(), "reviewer".to_string()]
    );

    #[cfg(feature = "sqlite")]
    {
        let raw = select_roles_json(&pool, created_id).await?;
        assert_eq!(raw, "[\"editor\",\"reviewer\"]");
    }

    #[cfg(feature = "postgres")]
    {
        let raw = select_roles_json(&pool, created_id).await?;
        assert_eq!(raw, serde_json::json!(["editor", "reviewer"]));
    }

    let schema = schema_builder(database.clone())
        .data("test-user".to_string())
        .finish();
    let sdl = schema.sdl();
    assert!(sdl.contains("roles: [String!]!"));
    assert!(!sdl.contains("rolesJson"));

    Ok(())
}
