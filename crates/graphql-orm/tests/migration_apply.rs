use graphql_orm::graphql::orm::{
    ColumnModel, DatabaseBackend, IndexDef, Migration, MigrationRunner, SchemaModel, TableModel,
    build_migration_plan, introspect_schema,
};

const HISTORY_TABLE: &str = "__graphql_orm_migrations";

fn leaked_index(name: &str, columns: &[&str]) -> IndexDef {
    let leaked_name: &'static str = Box::leak(name.to_string().into_boxed_str());
    let leaked_columns: &'static [&'static str] = Box::leak(
        columns
            .iter()
            .map(|column| Box::leak((*column).to_string().into_boxed_str()) as &'static str)
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );
    IndexDef::new(leaked_name, leaked_columns)
}

#[cfg(feature = "sqlite")]
async fn sqlite_history_count(pool: &sqlx::SqlitePool) -> Result<i64, sqlx::Error> {
    let row = sqlx::query(&format!("SELECT COUNT(*) AS count FROM {}", HISTORY_TABLE))
        .fetch_one(pool)
        .await?;
    sqlx::Row::try_get::<i64, _>(&row, "count")
}

#[cfg(feature = "postgres")]
async fn postgres_history_count(
    pool: &sqlx::PgPool,
    version_prefix: &str,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query(&format!(
        "SELECT COUNT(*) AS count FROM {} WHERE version LIKE $1",
        HISTORY_TABLE
    ))
    .bind(format!("{version_prefix}%"))
    .fetch_one(pool)
    .await?;
    sqlx::Row::try_get::<i64, _>(&row, "count")
}

fn leak_migration(
    plan: &graphql_orm::graphql::orm::MigrationPlan,
    version: &str,
    description: &str,
) -> Migration {
    let leaked_statements: &'static [&'static str] = Box::leak(
        plan.statements
            .iter()
            .map(|statement| Box::leak(statement.clone().into_boxed_str()) as &'static str)
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );
    Migration {
        version: Box::leak(version.to_string().into_boxed_str()),
        description: Box::leak(description.to_string().into_boxed_str()),
        statements: leaked_statements,
    }
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_migration_runner_applies_rebuild_plan() -> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;

    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await?;
    sqlx::query("CREATE TABLE users (id TEXT PRIMARY KEY, name TEXT NOT NULL)")
        .execute(&pool)
        .await?;

    let current = SchemaModel {
        tables: vec![TableModel {
            entity_name: "User".to_string(),
            table_name: "users".to_string(),
            primary_key: "id".to_string(),
            default_sort: "name ASC".to_string(),
            columns: vec![
                ColumnModel {
                    name: "id".to_string(),
                    sql_type: "TEXT".to_string(),
                    nullable: false,
                    is_primary_key: true,
                    is_unique: false,
                    default: None,
                },
                ColumnModel {
                    name: "name".to_string(),
                    sql_type: "TEXT".to_string(),
                    nullable: false,
                    is_primary_key: false,
                    is_unique: false,
                    default: None,
                },
            ],
            indexes: vec![],
            composite_unique_indexes: vec![],
            foreign_keys: vec![],
        }],
    };
    let target = SchemaModel {
        tables: vec![TableModel {
            entity_name: "User".to_string(),
            table_name: "users".to_string(),
            primary_key: "id".to_string(),
            default_sort: "name ASC".to_string(),
            columns: vec![
                ColumnModel {
                    name: "id".to_string(),
                    sql_type: "TEXT".to_string(),
                    nullable: false,
                    is_primary_key: true,
                    is_unique: false,
                    default: None,
                },
                ColumnModel {
                    name: "name".to_string(),
                    sql_type: "VARCHAR(255)".to_string(),
                    nullable: false,
                    is_primary_key: false,
                    is_unique: false,
                    default: None,
                },
                ColumnModel {
                    name: "active".to_string(),
                    sql_type: "BOOLEAN".to_string(),
                    nullable: false,
                    is_primary_key: false,
                    is_unique: false,
                    default: Some("false".to_string()),
                },
            ],
            indexes: vec![leaked_index("idx_users_name", &["name"])],
            composite_unique_indexes: vec![],
            foreign_keys: vec![],
        }],
    };

    let plan = build_migration_plan(DatabaseBackend::Sqlite, &current, &target);
    let database = graphql_orm::db::Database::new(pool.clone());
    database
        .apply_migrations(&[leak_migration(&plan, "2026032501", "sqlite_rebuild_plan")])
        .await?;
    database
        .apply_migrations(&[leak_migration(&plan, "2026032501", "sqlite_rebuild_plan")])
        .await?;

    let introspected = introspect_schema(&pool).await?;
    let users_table = introspected
        .tables
        .iter()
        .find(|table| table.table_name == "users")
        .expect("users table should exist after migration");
    assert!(
        users_table.columns.iter().any(|column| {
            column.name == "active" && column.default.as_deref() == Some("false")
        })
    );
    assert!(
        users_table
            .indexes
            .iter()
            .any(|index| index.name == "idx_users_name")
    );
    assert_eq!(sqlite_history_count(&pool).await?, 1);

    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_migration_runner_recovers_from_stale_rewrite_table()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await?;
    sqlx::query("CREATE TABLE users (id TEXT PRIMARY KEY, name TEXT NOT NULL)")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE __graphql_orm_users_new (id TEXT PRIMARY KEY, name VARCHAR(255) NOT NULL, active BOOLEAN NOT NULL DEFAULT false)",
    )
    .execute(&pool)
    .await?;

    let plan = build_migration_plan(
        DatabaseBackend::Sqlite,
        &SchemaModel {
            tables: vec![users_v1_like()],
        },
        &SchemaModel {
            tables: vec![users_v2_like()],
        },
    );
    let database = graphql_orm::db::Database::new(pool.clone());
    database
        .apply_migrations(&[leak_migration(
            &plan,
            "2026032504",
            "sqlite_recover_stale_rewrite_table",
        )])
        .await?;

    let row = sqlx::query(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = '__graphql_orm_users_new'",
    )
    .fetch_optional(&pool)
    .await?;
    assert!(row.is_none());
    assert_eq!(sqlite_history_count(&pool).await?, 1);

    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_migration_runner_rolls_back_failed_rewrite()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
    sqlx::query("CREATE TABLE users (id TEXT PRIMARY KEY, name TEXT NOT NULL)")
        .execute(&pool)
        .await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    let failing = Migration {
        version: "2026032502",
        description: "sqlite_rewrite_failure",
        statements: &[
            "CREATE TABLE __graphql_orm_users_new (id TEXT PRIMARY KEY, name TEXT NOT NULL, active BOOLEAN NOT NULL DEFAULT false)",
            "INSERT INTO __graphql_orm_users_new (id, missing_column) SELECT id, missing_column FROM users",
            "DROP TABLE users",
            "ALTER TABLE __graphql_orm_users_new RENAME TO users",
        ],
    };

    assert!(database.apply_migrations(&[failing]).await.is_err());

    let users =
        sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'users'")
            .fetch_optional(&pool)
            .await?;
    assert!(users.is_some());
    let temp = sqlx::query(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = '__graphql_orm_users_new'",
    )
    .fetch_optional(&pool)
    .await?;
    assert!(temp.is_none());

    Ok(())
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn postgres_migration_runner_applies_plan() -> Result<(), Box<dyn std::error::Error>> {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:55432/postgres".to_string());
    let pool = sqlx::PgPool::connect(&database_url).await?;
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let version_prefix = format!("2026032501_pg_{}", suffix);
    let table_name = format!("migration_users_{}", suffix);
    let index_name = format!("idx_{}_name", table_name);

    let target = SchemaModel {
        tables: vec![TableModel {
            entity_name: "User".to_string(),
            table_name: table_name.clone(),
            primary_key: "id".to_string(),
            default_sort: "name ASC".to_string(),
            columns: vec![
                ColumnModel {
                    name: "id".to_string(),
                    sql_type: "TEXT".to_string(),
                    nullable: false,
                    is_primary_key: true,
                    is_unique: false,
                    default: None,
                },
                ColumnModel {
                    name: "name".to_string(),
                    sql_type: "TEXT".to_string(),
                    nullable: false,
                    is_primary_key: false,
                    is_unique: false,
                    default: None,
                },
            ],
            indexes: vec![leaked_index(&index_name, &["name"])],
            composite_unique_indexes: vec![],
            foreign_keys: vec![],
        }],
    };

    let plan = build_migration_plan(
        DatabaseBackend::Postgres,
        &SchemaModel { tables: vec![] },
        &target,
    );
    let database = graphql_orm::db::Database::new(pool.clone());
    database
        .apply_migrations(&[leak_migration(
            &plan,
            &version_prefix,
            "postgres_apply_plan",
        )])
        .await?;
    database
        .apply_migrations(&[leak_migration(
            &plan,
            &version_prefix,
            "postgres_apply_plan",
        )])
        .await?;

    let introspected = introspect_schema(&pool).await?;
    let table = introspected
        .tables
        .iter()
        .find(|table| table.table_name == table_name)
        .expect("migrated Postgres table should exist");
    assert_eq!(table.primary_key, "id");
    assert!(table.columns.iter().any(|column| column.name == "name"));
    assert!(table.indexes.iter().any(|index| index.name == index_name));
    assert_eq!(postgres_history_count(&pool, &version_prefix).await?, 1);

    sqlx::query(&format!("DROP TABLE IF EXISTS {}", table_name))
        .execute(&pool)
        .await?;

    Ok(())
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn postgres_migration_runner_rolls_back_failed_migration()
-> Result<(), Box<dyn std::error::Error>> {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:55432/postgres".to_string());
    let pool = sqlx::PgPool::connect(&database_url).await?;
    let table_name = format!(
        "rollback_users_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    let version_prefix = format!("2026032503_pg_{}", table_name);
    let database = graphql_orm::db::Database::new(pool.clone());
    let create_sql: &'static str =
        Box::leak(format!("CREATE TABLE {} (id TEXT PRIMARY KEY)", table_name).into_boxed_str());
    let statements: &'static [&'static str] =
        Box::leak(vec![create_sql, "THIS IS NOT VALID SQL"].into_boxed_slice());
    let failing = Migration {
        version: Box::leak(version_prefix.clone().into_boxed_str()),
        description: "postgres_rollback_failure",
        statements,
    };

    assert!(database.apply_migrations(&[failing]).await.is_err());

    let row = sqlx::query(
        "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' AND table_name = $1",
    )
    .bind(&table_name)
    .fetch_optional(&pool)
    .await?;
    assert!(row.is_none());
    assert_eq!(postgres_history_count(&pool, &version_prefix).await?, 0);

    Ok(())
}

#[cfg(feature = "sqlite")]
fn users_v1_like() -> TableModel {
    TableModel {
        entity_name: "User".to_string(),
        table_name: "users".to_string(),
        primary_key: "id".to_string(),
        default_sort: "name ASC".to_string(),
        columns: vec![
            ColumnModel {
                name: "id".to_string(),
                sql_type: "TEXT".to_string(),
                nullable: false,
                is_primary_key: true,
                is_unique: false,
                default: None,
            },
            ColumnModel {
                name: "name".to_string(),
                sql_type: "TEXT".to_string(),
                nullable: false,
                is_primary_key: false,
                is_unique: false,
                default: None,
            },
        ],
        indexes: vec![],
        composite_unique_indexes: vec![],
        foreign_keys: vec![],
    }
}

#[cfg(feature = "sqlite")]
fn users_v2_like() -> TableModel {
    TableModel {
        columns: vec![
            ColumnModel {
                name: "id".to_string(),
                sql_type: "TEXT".to_string(),
                nullable: false,
                is_primary_key: true,
                is_unique: false,
                default: None,
            },
            ColumnModel {
                name: "name".to_string(),
                sql_type: "VARCHAR(255)".to_string(),
                nullable: false,
                is_primary_key: false,
                is_unique: false,
                default: None,
            },
            ColumnModel {
                name: "active".to_string(),
                sql_type: "BOOLEAN".to_string(),
                nullable: false,
                is_primary_key: false,
                is_unique: false,
                default: Some("false".to_string()),
            },
        ],
        indexes: vec![leaked_index("idx_users_name", &["name"])],
        ..users_v1_like()
    }
}
