use graphql_orm::graphql::orm::{
    ColumnModel, DatabaseBackend, IndexDef, Migration, MigrationRunner, SchemaModel, TableModel,
    build_migration_plan, introspect_schema,
};

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

fn leak_migration(plan: &graphql_orm::graphql::orm::MigrationPlan) -> Migration {
    let leaked_statements: &'static [&'static str] = Box::leak(
        plan.statements
            .iter()
            .map(|statement| Box::leak(statement.clone().into_boxed_str()) as &'static str)
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );
    Migration {
        version: "2026032501",
        description: "test_migration",
        statements: leaked_statements,
    }
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_migration_runner_applies_rebuild_plan() -> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;

    sqlx::query("PRAGMA foreign_keys = ON").execute(&pool).await?;
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
    database.apply_migrations(&[leak_migration(&plan)]).await?;

    let introspected = introspect_schema(&pool).await?;
    let users_table = introspected
        .tables
        .iter()
        .find(|table| table.table_name == "users")
        .expect("users table should exist after migration");
    assert!(users_table.columns.iter().any(|column| {
        column.name == "active" && column.default.as_deref() == Some("false")
    }));
    assert!(users_table
        .indexes
        .iter()
        .any(|index| index.name == "idx_users_name"));

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

    let plan = build_migration_plan(DatabaseBackend::Postgres, &SchemaModel { tables: vec![] }, &target);
    let database = graphql_orm::db::Database::new(pool.clone());
    database.apply_migrations(&[leak_migration(&plan)]).await?;

    let introspected = introspect_schema(&pool).await?;
    let table = introspected
        .tables
        .iter()
        .find(|table| table.table_name == table_name)
        .expect("migrated Postgres table should exist");
    assert_eq!(table.primary_key, "id");
    assert!(table.columns.iter().any(|column| column.name == "name"));
    assert!(table.indexes.iter().any(|index| index.name == index_name));

    sqlx::query(&format!("DROP TABLE IF EXISTS {}", table_name))
        .execute(&pool)
        .await?;

    Ok(())
}
