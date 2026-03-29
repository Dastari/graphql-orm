use graphql_orm::prelude::*;

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "policy_parents",
    plural = "PolicyParents",
    default_sort = "name ASC"
)]
struct PolicyParent {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,
}

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "cascade_children",
    plural = "CascadeChildren",
    default_sort = "id ASC"
)]
struct CascadeChild {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub parent_id: String,

    #[relation(
        target = "PolicyParent",
        from = "parent_id",
        to = "id",
        on_delete = "cascade"
    )]
    pub parent: Option<PolicyParent>,
}

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "restrict_children",
    plural = "RestrictChildren",
    default_sort = "id ASC"
)]
struct RestrictChild {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub parent_id: String,

    #[relation(
        target = "PolicyParent",
        from = "parent_id",
        to = "id",
        on_delete = "restrict"
    )]
    pub parent: Option<PolicyParent>,
}

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "set_null_children",
    plural = "SetNullChildren",
    default_sort = "id ASC"
)]
struct SetNullChild {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub parent_id: Option<String>,

    #[relation(
        target = "PolicyParent",
        from = "parent_id",
        to = "id",
        on_delete = "set_null"
    )]
    pub parent: Option<PolicyParent>,
}

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "stage_policy_children",
    plural = "StagePolicyChildren",
    default_sort = "id ASC"
)]
struct StageChildRestrict {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub parent_id: String,

    #[relation(target = "StageParent", from = "parent_id", to = "id")]
    pub parent: Option<StageParent>,
}

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "stage_policy_children",
    plural = "StagePolicyChildren",
    default_sort = "id ASC"
)]
struct StageChildCascade {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub parent_id: String,

    #[relation(
        target = "StageParent",
        from = "parent_id",
        to = "id",
        on_delete = "cascade"
    )]
    pub parent: Option<StageParent>,
}

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "stage_policy_parents",
    plural = "StagePolicyParents",
    default_sort = "name ASC"
)]
struct StageParent {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,
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
    for table in [
        "cascade_children",
        "restrict_children",
        "set_null_children",
        "policy_parents",
        "stage_policy_children",
        "stage_policy_parents",
    ] {
        sqlx::query(&format!("DROP TABLE IF EXISTS {table} CASCADE"))
            .execute(&pool)
            .await?;
    }
    Ok(pool)
}

#[cfg(feature = "sqlite")]
async fn insert_parent(pool: &sqlx::SqlitePool, id: &str, name: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO policy_parents (id, name) VALUES (?, ?)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn insert_parent(pool: &sqlx::PgPool, id: &str, name: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO policy_parents (id, name) VALUES ($1, $2)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn insert_child(
    pool: &sqlx::SqlitePool,
    table: &str,
    id: &str,
    parent_id: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(&format!(
        "INSERT INTO {table} (id, parent_id) VALUES (?, ?)"
    ))
    .bind(id)
    .bind(parent_id)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn insert_child(
    pool: &sqlx::PgPool,
    table: &str,
    id: &str,
    parent_id: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(&format!(
        "INSERT INTO {table} (id, parent_id) VALUES ($1, $2)"
    ))
    .bind(id)
    .bind(parent_id)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn delete_parent(pool: &sqlx::SqlitePool, id: &str) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query("DELETE FROM policy_parents WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

#[cfg(feature = "postgres")]
async fn delete_parent(pool: &sqlx::PgPool, id: &str) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query("DELETE FROM policy_parents WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

#[cfg(feature = "sqlite")]
async fn child_exists(pool: &sqlx::SqlitePool, table: &str, id: &str) -> Result<bool, sqlx::Error> {
    let row = sqlx::query(&format!(
        "SELECT COUNT(*) AS count FROM {table} WHERE id = ?"
    ))
    .bind(id)
    .fetch_one(pool)
    .await?;
    Ok(sqlx::Row::try_get::<i64, _>(&row, "count")? > 0)
}

#[cfg(feature = "postgres")]
async fn child_exists(pool: &sqlx::PgPool, table: &str, id: &str) -> Result<bool, sqlx::Error> {
    let row = sqlx::query(&format!(
        "SELECT COUNT(*) AS count FROM {table} WHERE id = $1"
    ))
    .bind(id)
    .fetch_one(pool)
    .await?;
    Ok(sqlx::Row::try_get::<i64, _>(&row, "count")? > 0)
}

#[cfg(feature = "sqlite")]
async fn child_parent_id(
    pool: &sqlx::SqlitePool,
    table: &str,
    id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query(&format!("SELECT parent_id FROM {table} WHERE id = ?"))
        .bind(id)
        .fetch_one(pool)
        .await?;
    sqlx::Row::try_get::<Option<String>, _>(&row, "parent_id")
}

#[cfg(feature = "postgres")]
async fn child_parent_id(
    pool: &sqlx::PgPool,
    table: &str,
    id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query(&format!("SELECT parent_id FROM {table} WHERE id = $1"))
        .bind(id)
        .fetch_one(pool)
        .await?;
    sqlx::Row::try_get::<Option<String>, _>(&row, "parent_id")
}

fn leak_migration(
    plan: &graphql_orm::graphql::orm::MigrationPlan,
    version: &str,
    description: &str,
) -> graphql_orm::graphql::orm::Migration {
    let version: &'static str = Box::leak(version.to_string().into_boxed_str());
    let description: &'static str = Box::leak(description.to_string().into_boxed_str());
    let statements: &'static [&'static str] = Box::leak(
        plan.statements
            .iter()
            .cloned()
            .map(|statement| Box::leak(statement.into_boxed_str()) as &'static str)
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );
    graphql_orm::graphql::orm::Migration {
        version,
        description,
        statements,
    }
}

#[tokio::test]
async fn relation_delete_policies_apply_and_execute_correctly()
-> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::{
        DeletePolicy, Entity, SchemaStage, SchemaStageRunner, introspect_schema,
    };

    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());

    let target_schema = graphql_orm::graphql::orm::SchemaModel::from_entities(&[
        <PolicyParent as Entity>::metadata(),
        <CascadeChild as Entity>::metadata(),
        <RestrictChild as Entity>::metadata(),
        <SetNullChild as Entity>::metadata(),
    ]);
    let plan = graphql_orm::graphql::orm::build_migration_plan(
        graphql_orm::graphql::orm::current_backend(),
        &graphql_orm::graphql::orm::introspect_schema(&pool).await?,
        &target_schema,
    );
    let initial_version = format!(
        "20260329_relation_policy_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    database
        .apply_migrations(&[leak_migration(
            &plan,
            &initial_version,
            "relation_delete_policy",
        )])
        .await
        .map_err(|error| {
            format!("apply initial relation-delete-policy migration failed: {error}")
        })?;

    let schema = introspect_schema(&pool).await?;
    let cascade_fk = schema
        .tables
        .iter()
        .find(|table| table.table_name == "cascade_children")
        .and_then(|table| table.foreign_keys.first())
        .ok_or("missing cascade fk")?;
    assert_eq!(cascade_fk.on_delete, DeletePolicy::Cascade);

    let restrict_fk = schema
        .tables
        .iter()
        .find(|table| table.table_name == "restrict_children")
        .and_then(|table| table.foreign_keys.first())
        .ok_or("missing restrict fk")?;
    assert_eq!(restrict_fk.on_delete, DeletePolicy::Restrict);

    let set_null_fk = schema
        .tables
        .iter()
        .find(|table| table.table_name == "set_null_children")
        .and_then(|table| table.foreign_keys.first())
        .ok_or("missing set-null fk")?;
    assert_eq!(set_null_fk.on_delete, DeletePolicy::SetNull);

    insert_parent(&pool, "parent_cascade", "cascade")
        .await
        .map_err(|error| format!("insert parent_cascade failed: {error}"))?;
    insert_child(
        &pool,
        "cascade_children",
        "child_cascade",
        Some("parent_cascade"),
    )
    .await
    .map_err(|error| format!("insert child_cascade failed: {error}"))?;
    assert_eq!(
        delete_parent(&pool, "parent_cascade")
            .await
            .map_err(|error| format!("delete parent_cascade failed: {error}"))?,
        1
    );
    assert!(
        !child_exists(&pool, "cascade_children", "child_cascade")
            .await
            .map_err(|error| format!("check child_cascade existence failed: {error}"))?
    );

    insert_parent(&pool, "parent_restrict", "restrict")
        .await
        .map_err(|error| format!("insert parent_restrict failed: {error}"))?;
    insert_child(
        &pool,
        "restrict_children",
        "child_restrict",
        Some("parent_restrict"),
    )
    .await
    .map_err(|error| format!("insert child_restrict failed: {error}"))?;
    assert!(delete_parent(&pool, "parent_restrict").await.is_err());

    insert_parent(&pool, "parent_set_null", "set_null")
        .await
        .map_err(|error| format!("insert parent_set_null failed: {error}"))?;
    insert_child(
        &pool,
        "set_null_children",
        "child_set_null",
        Some("parent_set_null"),
    )
    .await
    .map_err(|error| format!("insert child_set_null failed: {error}"))?;
    assert_eq!(
        delete_parent(&pool, "parent_set_null")
            .await
            .map_err(|error| format!("delete parent_set_null failed: {error}"))?,
        1
    );
    assert_eq!(
        child_parent_id(&pool, "set_null_children", "child_set_null")
            .await
            .map_err(|error| format!("check child_set_null parent_id failed: {error}"))?,
        None
    );

    let stage_pool = setup_pool().await?;
    let stage_database = graphql_orm::db::Database::new(stage_pool.clone());
    let version_prefix = format!(
        "20260329_policy_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    let stages = vec![
        SchemaStage::from_entities(
            format!("{version_prefix}_01"),
            "restrict_stage",
            &[
                <StageParent as Entity>::metadata(),
                <StageChildRestrict as Entity>::metadata(),
            ],
        ),
        SchemaStage::from_entities(
            format!("{version_prefix}_02"),
            "cascade_stage",
            &[
                <StageParent as Entity>::metadata(),
                <StageChildCascade as Entity>::metadata(),
            ],
        ),
    ];

    stage_database
        .apply_schema_stages(&stages[..1])
        .await
        .map_err(|error| format!("apply staged restrict migration failed: {error}"))?;
    let before_upgrade = introspect_schema(&stage_pool).await?;
    let before_fk = before_upgrade
        .tables
        .iter()
        .find(|table| table.table_name == "stage_policy_children")
        .and_then(|table| table.foreign_keys.first())
        .ok_or("missing staged fk before upgrade")?;
    assert_eq!(before_fk.on_delete, DeletePolicy::Restrict);

    stage_database
        .apply_schema_stages(&stages)
        .await
        .map_err(|error| format!("apply staged cascade migration failed: {error}"))?;
    let after_upgrade = introspect_schema(&stage_pool).await?;
    let after_fk = after_upgrade
        .tables
        .iter()
        .find(|table| table.table_name == "stage_policy_children")
        .and_then(|table| table.foreign_keys.first())
        .ok_or("missing staged fk after upgrade")?;
    assert_eq!(after_fk.on_delete, DeletePolicy::Cascade);

    let planned = stage_database.plan_schema_stages(&stages).await?;
    assert!(planned.is_empty());

    Ok(())
}
