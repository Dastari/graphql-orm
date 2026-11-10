// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "postgres"))]

#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

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
    let pool = owned_postgres::OwnedPostgres::pool("schema-stages", 5).await?;
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
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
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
        spatial: None,
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
        spatial: None,
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
        primary_keys: vec!["id".to_string()],
        default_sort: "slug ASC".to_string(),
        columns: vec![text_column("id", true), text_column("slug", false)],
        indexes: vec![IndexDef::new("idx_vocabularies_slug", &["slug"])],
        composite_unique_indexes: vec![],
        foreign_keys: vec![],
        search_indexes: vec![],
        append_only: false,
        retention_purge: false,
        check_constraints: vec![],
    }
}

#[cfg(feature = "sqlite")]
fn sqlite_vocabulary_terms_v1() -> TableModel {
    TableModel {
        entity_name: "VocabularyTerm".to_string(),
        table_name: "vocabulary_terms".to_string(),
        primary_key: "id".to_string(),
        primary_keys: vec!["id".to_string()],
        default_sort: "term ASC".to_string(),
        columns: vec![
            text_column("id", true),
            text_column("vocabulary_id", false),
            text_column("term", false),
        ],
        indexes: vec![],
        composite_unique_indexes: vec![],
        foreign_keys: vec![ForeignKeyModel::single(
            "vocabulary_id",
            "vocabularies",
            "id",
            false,
            DeletePolicy::Cascade,
        )],
        search_indexes: vec![],
        append_only: false,
        retention_purge: false,
        check_constraints: vec![],
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
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
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
                extensions: Vec::new(),
                tables: vec![sqlite_vocabularies_v1(), sqlite_vocabulary_terms_v1()],
            },
        ),
        SchemaStage::from_schema_model(
            format!("{version_prefix}_02"),
            "vocabulary_rebuild",
            SchemaModel {
                extensions: Vec::new(),
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
        foreign_key
            .source_columns()
            .eq(["vocabulary_id"].into_iter())
            && foreign_key.target_table == "vocabularies"
            && foreign_key.target_columns().eq(["id"].into_iter())
    }));

    Ok(())
}

// Issue #442: staged application runs the same step-risk gate as planned
// migrations, before any stage SQL runs.

#[cfg(feature = "sqlite")]
type GatePool = sqlx::SqlitePool;
#[cfg(feature = "postgres")]
type GatePool = sqlx::PgPool;

fn gate_column(
    name: &str,
    primary_key: bool,
    nullable: bool,
) -> graphql_orm::graphql::orm::ColumnModel {
    graphql_orm::graphql::orm::ColumnModel {
        name: name.to_string(),
        sql_type: "TEXT".to_string(),
        spatial: None,
        nullable,
        is_primary_key: primary_key,
        is_unique: false,
        default: None,
    }
}

fn gate_table(table_name: &str, columns: &[(&str, bool)]) -> graphql_orm::graphql::orm::TableModel {
    let mut all = vec![gate_column("id", true, false)];
    all.extend(
        columns
            .iter()
            .map(|(name, nullable)| gate_column(name, false, *nullable)),
    );
    graphql_orm::graphql::orm::TableModel {
        entity_name: table_name.to_string(),
        table_name: table_name.to_string(),
        primary_key: "id".to_string(),
        primary_keys: vec!["id".to_string()],
        default_sort: "id ASC".to_string(),
        columns: all,
        indexes: vec![],
        composite_unique_indexes: vec![],
        foreign_keys: vec![],
        search_indexes: vec![],
        append_only: false,
        retention_purge: false,
        check_constraints: vec![],
    }
}

fn gate_stage(
    version: &str,
    tables: Vec<graphql_orm::graphql::orm::TableModel>,
) -> graphql_orm::graphql::orm::SchemaStage {
    graphql_orm::graphql::orm::SchemaStage::from_schema_model(
        version.to_string(),
        format!("stage {version}"),
        graphql_orm::graphql::orm::SchemaModel {
            extensions: Vec::new(),
            tables,
        },
    )
}

fn gate_prefix(label: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(format!(
        "20261109_{label}_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ))
}

async fn gate_table_names(pool: &GatePool) -> Result<Vec<String>, sqlx::Error> {
    Ok(graphql_orm::graphql::orm::introspect_schema(pool)
        .await?
        .tables
        .into_iter()
        .map(|table| table.table_name)
        .collect())
}

async fn gate_column_names(pool: &GatePool, table: &str) -> Result<Vec<String>, sqlx::Error> {
    Ok(graphql_orm::graphql::orm::introspect_schema(pool)
        .await?
        .tables
        .into_iter()
        .find(|candidate| candidate.table_name == table)
        .map(|table| {
            table
                .columns
                .into_iter()
                .map(|column| column.name)
                .collect()
        })
        .unwrap_or_default())
}

async fn gate_row_count(pool: &GatePool, table: &str) -> Result<i64, sqlx::Error> {
    let row = sqlx::query(&format!("SELECT COUNT(*) AS count FROM {table}"))
        .fetch_one(pool)
        .await?;
    sqlx::Row::try_get::<i64, _>(&row, "count")
}

fn assert_refused(error: sqlx::Error, kind: &str, version: &str) {
    assert!(
        matches!(
            &error,
            sqlx::Error::Protocol(message)
                if message.starts_with(&format!("Migration {version} contains {kind} step"))
        ),
        "expected {kind} refusal for {version}, got {error:?}"
    );
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn destructive_stage_is_refused_by_default_and_applies_only_with_opt_in()
-> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::{ApplyOptions, MigrationRisk, SchemaStageRunner};

    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    let prefix = gate_prefix("destructive_gate")?;
    let v1 = format!("{prefix}_01");
    let v2 = format!("{prefix}_02");
    let stages = vec![
        gate_stage(
            &v1,
            vec![
                gate_table("gate_users", &[("name", false), ("legacy", true)]),
                gate_table("gate_notes", &[("body", false)]),
            ],
        ),
        // Drops both a table and a column that hold data.
        gate_stage(&v2, vec![gate_table("gate_users", &[("name", false)])]),
    ];

    database.apply_schema_stages(&stages[..1]).await?;
    sqlx::query("INSERT INTO gate_users (id, name, legacy) VALUES ('u1', 'ada', 'kept')")
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO gate_notes (id, body) VALUES ('n1', 'kept')")
        .execute(&pool)
        .await?;

    let planned = database.plan_schema_stages(&stages).await?;
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].highest_risk(), Some(MigrationRisk::Destructive));
    assert!(
        planned[0]
            .steps()
            .iter()
            .filter(|step| step.risk == MigrationRisk::Destructive)
            .count()
            >= 2
    );

    assert_refused(
        database.apply_schema_stages(&stages).await.unwrap_err(),
        "destructive",
        &v2,
    );
    assert_refused(
        database
            .apply_schema_stages_with_options(&stages, ApplyOptions::default())
            .await
            .unwrap_err(),
        "destructive",
        &v2,
    );
    assert_refused(
        database
            .apply_schema_stages_with_options(
                &stages,
                ApplyOptions {
                    dry_run: true,
                    ..ApplyOptions::default()
                },
            )
            .await
            .unwrap_err(),
        "destructive",
        &v2,
    );
    assert!(
        gate_table_names(&pool)
            .await?
            .contains(&"gate_notes".to_string())
    );
    assert!(
        gate_column_names(&pool, "gate_users")
            .await?
            .contains(&"legacy".to_string())
    );
    assert_eq!(gate_row_count(&pool, "gate_notes").await?, 1);
    assert_eq!(applied_stage_count(&pool, &prefix).await?, 1);

    let reports = database
        .apply_schema_stages_with_options(
            &stages,
            ApplyOptions {
                allow_destructive: true,
                ..ApplyOptions::default()
            },
        )
        .await?;
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].version, v2);
    assert!(!reports[0].dry_run);
    assert!(reports[0].statements_applied > 0);
    assert!(
        !gate_table_names(&pool)
            .await?
            .contains(&"gate_notes".to_string())
    );
    assert!(
        !gate_column_names(&pool, "gate_users")
            .await?
            .contains(&"legacy".to_string())
    );
    assert_eq!(gate_row_count(&pool, "gate_users").await?, 1);
    assert_eq!(applied_stage_count(&pool, &prefix).await?, 2);
    assert!(database.plan_schema_stages(&stages).await?.is_empty());
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn refused_later_stage_leaves_earlier_pending_stages_unapplied()
-> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::{ApplyOptions, SchemaAbi, SchemaStageRunner};

    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    let prefix = gate_prefix("atomic_gate")?;
    sqlx::query("CREATE TABLE gate_host (id TEXT PRIMARY KEY NOT NULL)")
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO gate_host (id) VALUES ('h1')")
        .execute(&pool)
        .await?;
    let v1 = format!("{prefix}_01");
    let v2 = format!("{prefix}_02");
    let host = gate_table("gate_host", &[]);
    let stages = vec![
        // Additive on its own: keeps the host table and adds a new one.
        gate_stage(
            &v1,
            vec![host.clone(), gate_table("gate_added", &[("body", true)])],
        ),
        // Destructive: drops the host table.
        gate_stage(&v2, vec![gate_table("gate_added", &[("body", true)])]),
    ];

    assert_refused(
        database.apply_schema_stages(&stages).await.unwrap_err(),
        "destructive",
        &v2,
    );
    assert!(
        !gate_table_names(&pool)
            .await?
            .contains(&"gate_added".to_string())
    );
    assert_eq!(applied_stage_count(&pool, &prefix).await?, 0);

    // The ABI upgrade path admits every stage before applying any as well.
    let abi = SchemaAbi::new(stages)?;
    assert_refused(
        database
            .schema()
            .apply_upgrade(&abi, &v2, ApplyOptions::default())
            .await
            .unwrap_err(),
        "destructive",
        &v2,
    );
    assert!(
        !gate_table_names(&pool)
            .await?
            .contains(&"gate_added".to_string())
    );
    assert_eq!(gate_row_count(&pool, "gate_host").await?, 1);
    assert_eq!(applied_stage_count(&pool, &prefix).await?, 0);
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn additive_only_refuses_non_additive_stage_steps() -> Result<(), Box<dyn std::error::Error>>
{
    use graphql_orm::graphql::orm::{ApplyOptions, SchemaStageRunner};

    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    let prefix = gate_prefix("additive_gate")?;
    let v1 = format!("{prefix}_01");
    let v2 = format!("{prefix}_02");
    let v3 = format!("{prefix}_03");
    let additive_only = ApplyOptions {
        additive_only: true,
        ..ApplyOptions::default()
    };
    let stages = vec![
        gate_stage(&v1, vec![gate_table("gate_items", &[("name", false)])]),
        // Adds a nullable column: additive.
        gate_stage(
            &v2,
            vec![gate_table("gate_items", &[("name", false), ("note", true)])],
        ),
        // Adds a required column without a default: risky, not destructive.
        gate_stage(
            &v3,
            vec![gate_table(
                "gate_items",
                &[("name", false), ("note", true), ("code", false)],
            )],
        ),
    ];

    let reports = database
        .apply_schema_stages_with_options(&stages[..2], additive_only.clone())
        .await?;
    assert_eq!(reports.len(), 2);
    assert_eq!(applied_stage_count(&pool, &prefix).await?, 2);

    let error = database
        .apply_schema_stages_with_options(&stages, additive_only.clone())
        .await
        .unwrap_err();
    assert_refused(error, "non-additive", &v3);
    assert!(
        !gate_column_names(&pool, "gate_items")
            .await?
            .contains(&"code".to_string())
    );
    assert_eq!(applied_stage_count(&pool, &prefix).await?, 2);

    // additive_only also refuses destructive steps even with allow_destructive.
    let drop = vec![
        stages[0].clone(),
        stages[1].clone(),
        gate_stage(&v3, vec![gate_table("gate_items", &[("name", false)])]),
    ];
    let error = database
        .apply_schema_stages_with_options(
            &drop,
            ApplyOptions {
                allow_destructive: true,
                ..additive_only
            },
        )
        .await
        .unwrap_err();
    assert_refused(error, "non-additive", &v3);
    assert!(
        gate_column_names(&pool, "gate_items")
            .await?
            .contains(&"note".to_string())
    );

    // The risky step is allowed by default options because it is not destructive.
    database.apply_schema_stages(&stages).await?;
    assert!(
        gate_column_names(&pool, "gate_items")
            .await?
            .contains(&"code".to_string())
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn older_binary_stage_against_newer_database_is_refused()
-> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::SchemaStageRunner;

    let pool = setup_pool().await?;
    let prefix = gate_prefix("rollback_gate")?;
    let newer = vec![gate_stage(
        &format!("{prefix}_app_2"),
        vec![
            gate_table("gate_accounts", &[("name", false), ("added_in_2", true)]),
            gate_table("gate_invoices", &[("total", false)]),
        ],
    )];
    let older = vec![gate_stage(
        &format!("{prefix}_app_1"),
        vec![gate_table("gate_accounts", &[("name", false)])],
    )];

    let newer_binary = graphql_orm::db::Database::new(pool.clone());
    newer_binary.apply_schema_stages(&newer).await?;
    sqlx::query("INSERT INTO gate_accounts (id, name, added_in_2) VALUES ('a1', 'acme', 'new')")
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO gate_invoices (id, total) VALUES ('i1', '10')")
        .execute(&pool)
        .await?;

    // Rolling back to the older binary must not drop the newer schema.
    let older_binary = graphql_orm::db::Database::new(pool.clone());
    assert_refused(
        older_binary.apply_schema_stages(&older).await.unwrap_err(),
        "destructive",
        &format!("{prefix}_app_1"),
    );
    assert_eq!(gate_row_count(&pool, "gate_invoices").await?, 1);
    assert!(
        gate_column_names(&pool, "gate_accounts")
            .await?
            .contains(&"added_in_2".to_string())
    );
    assert_eq!(
        applied_stage_count(&pool, &format!("{prefix}_app_1")).await?,
        0
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn stage_apply_options_dry_run_history_and_baseline() -> Result<(), Box<dyn std::error::Error>>
{
    use graphql_orm::graphql::orm::{ApplyOptions, SchemaStageRunner, introspect_schema};

    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    let prefix = gate_prefix("options_gate")?;
    let stages = vec![gate_stage(
        &format!("{prefix}_01"),
        vec![gate_table("gate_options", &[("name", false)])],
    )];

    let reports = database
        .apply_schema_stages_with_options(
            &stages,
            ApplyOptions {
                dry_run: true,
                ..ApplyOptions::default()
            },
        )
        .await?;
    assert_eq!(reports.len(), 1);
    assert!(reports[0].dry_run);
    assert_eq!(reports[0].statements_applied, 0);
    assert!(
        !gate_table_names(&pool)
            .await?
            .contains(&"gate_options".to_string())
    );

    let error = database
        .apply_schema_stages_with_options(
            &stages,
            ApplyOptions {
                record_history: false,
                ..ApplyOptions::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&error, sqlx::Error::Protocol(message) if message.contains("record_history")),
        "{error:?}"
    );

    let error = database
        .apply_schema_stages_with_options(
            &stages,
            ApplyOptions {
                expected_current_schema_hash: Some("not-the-live-hash".to_string()),
                ..ApplyOptions::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&error, sqlx::Error::Protocol(message) if message.contains("baseline hash mismatch")),
        "{error:?}"
    );
    assert!(
        !gate_table_names(&pool)
            .await?
            .contains(&"gate_options".to_string())
    );

    let live_hash = introspect_schema(&pool).await?.stable_hash();
    database
        .apply_schema_stages_with_options(
            &stages,
            ApplyOptions {
                expected_current_schema_hash: Some(live_hash),
                ..ApplyOptions::default()
            },
        )
        .await?;
    assert!(
        gate_table_names(&pool)
            .await?
            .contains(&"gate_options".to_string())
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn abi_upgrade_checks_the_baseline_hash_once_before_any_stage()
-> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::{ApplyOptions, SchemaAbi, introspect_schema};

    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    let prefix = gate_prefix("abi_baseline")?;
    let v1 = format!("{prefix}_01");
    let v2 = format!("{prefix}_02");
    let v3 = format!("{prefix}_03");
    let abi = SchemaAbi::new(vec![
        gate_stage(&v1, vec![gate_table("gate_abi_one", &[("name", false)])]),
        gate_stage(
            &v2,
            vec![
                gate_table("gate_abi_one", &[("name", false)]),
                gate_table("gate_abi_two", &[("name", false)]),
            ],
        ),
        gate_stage(
            &v3,
            vec![
                gate_table("gate_abi_one", &[("name", false)]),
                gate_table("gate_abi_two", &[("name", false)]),
                gate_table("gate_abi_three", &[("name", false)]),
            ],
        ),
    ])?;
    database
        .schema()
        .apply_upgrade(&abi, &v1, ApplyOptions::default())
        .await?;

    // A wrong baseline is refused before the first pending stage runs.
    let error = database
        .schema()
        .apply_upgrade(
            &abi,
            &v3,
            ApplyOptions {
                expected_current_schema_hash: Some("not-the-live-hash".to_string()),
                ..ApplyOptions::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&error, sqlx::Error::Protocol(message) if message.contains("baseline hash mismatch")),
        "{error:?}"
    );
    assert!(
        !gate_table_names(&pool)
            .await?
            .contains(&"gate_abi_two".to_string())
    );
    assert_eq!(applied_stage_count(&pool, &prefix).await?, 1);

    // The live baseline admits the whole multi-stage path, not only its first stage.
    let live_hash = introspect_schema(&pool).await?.stable_hash();
    let applied = database
        .schema()
        .apply_upgrade(
            &abi,
            &v3,
            ApplyOptions {
                expected_current_schema_hash: Some(live_hash),
                ..ApplyOptions::default()
            },
        )
        .await?;
    assert_eq!(applied.applied.len(), 2);
    let tables = gate_table_names(&pool).await?;
    assert!(tables.contains(&"gate_abi_two".to_string()));
    assert!(tables.contains(&"gate_abi_three".to_string()));
    assert_eq!(applied_stage_count(&pool, &prefix).await?, 3);
    Ok(())
}
