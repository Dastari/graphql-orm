#![cfg(feature = "sqlite")]

use graphql_orm::graphql::orm::{
    ApplyOptions, ColumnModel, MigrationRisk, SchemaAbi, SchemaDiagnosticKind, SchemaModel,
    SchemaPolicy, SchemaStage, TableModel,
};
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "readonly_notes",
    plural = "ReadOnlyNotes",
    schema_policy = "external_read_only"
)]
struct ReadOnlyNote {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub title: String,
}

schema_roots! {
    backend: "sqlite",
    schema_policy: "external_read_only",
    query_custom_ops: [],
    entities: [ReadOnlyNote],
}

fn text_column(name: &str, primary_key: bool, nullable: bool) -> ColumnModel {
    ColumnModel {
        name: name.to_string(),
        sql_type: "TEXT".to_string(),
        spatial: None,
        nullable,
        is_primary_key: primary_key,
        is_unique: false,
        default: None,
    }
}

fn users_v1(table_name: &str) -> TableModel {
    TableModel {
        entity_name: "User".to_string(),
        table_name: table_name.to_string(),
        primary_key: "id".to_string(),
        primary_keys: vec!["id".to_string()],
        default_sort: "id ASC".to_string(),
        columns: vec![
            text_column("id", true, false),
            text_column("name", false, false),
        ],
        indexes: vec![],
        composite_unique_indexes: vec![],
        foreign_keys: vec![],
        search_indexes: vec![],
        append_only: false,
        check_constraints: vec![],
    }
}

fn users_v2(table_name: &str) -> TableModel {
    TableModel {
        columns: vec![
            text_column("id", true, false),
            text_column("name", false, false),
            text_column("email", false, true),
        ],
        ..users_v1(table_name)
    }
}

#[tokio::test]
async fn database_new_does_not_mutate_schema_and_defaults_to_managed()
-> Result<(), Box<dyn std::error::Error>> {
    graphql_orm::graphql::orm::reset_query_count();
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
    let database = graphql_orm::db::Database::new(pool);

    assert_eq!(database.schema_policy(), SchemaPolicy::Managed);
    assert_eq!(graphql_orm::graphql::orm::query_count(), 0);
    Ok(())
}

#[tokio::test]
async fn external_read_only_schema_omits_generated_mutations()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
    let database = graphql_orm::db::Database::builder(pool)
        .schema_policy(SchemaPolicy::ExternalReadOnly)
        .build();
    let schema = schema_builder(database).finish();
    let sdl = schema.sdl();

    assert!(sdl.contains("type Query"));
    assert!(!sdl.contains("createReadOnlyNote"));
    assert!(!sdl.contains("updateReadOnlyNote"));
    assert!(!sdl.contains("deleteReadOnlyNote"));
    Ok(())
}

#[tokio::test]
async fn validate_reports_structured_primary_key_drift() -> Result<(), Box<dyn std::error::Error>> {
    let current = SchemaModel {
        extensions: Vec::new(),
        tables: vec![users_v1("policy_users")],
    };
    let mut target_table = users_v1("policy_users");
    target_table.primary_keys = vec!["id".to_string(), "name".to_string()];
    target_table
        .columns
        .iter_mut()
        .find(|column| column.name == "name")
        .expect("name column")
        .is_primary_key = true;
    let target = SchemaModel {
        extensions: Vec::new(),
        tables: vec![target_table],
    };
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
    let database = graphql_orm::db::Database::new(pool);

    let report = database.schema().validate(&current, &target).unwrap();
    assert!(report.has_errors());
    assert!(report.diagnostics.iter().any(|diagnostic| {
        diagnostic.kind == SchemaDiagnosticKind::PrimaryKeyMismatch
            && diagnostic.table.as_deref() == Some("policy_users")
    }));
    Ok(())
}

#[tokio::test]
async fn external_read_only_rejects_planning() -> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
    let database = graphql_orm::db::Database::builder(pool)
        .schema_policy(SchemaPolicy::ExternalReadOnly)
        .build();
    let current = SchemaModel {
        extensions: Vec::new(),
        tables: vec![],
    };
    let target = SchemaModel {
        extensions: Vec::new(),
        tables: vec![users_v1("readonly_users")],
    };

    let err = database
        .schema()
        .plan_migration("1", "create users", &current, &target)
        .expect_err("external read-only should reject planning");
    assert!(
        err.to_string()
            .contains("does not allow plan schema migration")
    );
    Ok(())
}

#[tokio::test]
async fn plan_only_rejects_application_and_classifies_risk()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
    let database = graphql_orm::db::Database::builder(pool)
        .schema_policy(SchemaPolicy::PlanOnly)
        .build();
    let current = SchemaModel {
        extensions: Vec::new(),
        tables: vec![],
    };
    let target = SchemaModel {
        extensions: Vec::new(),
        tables: vec![users_v1("plan_only_users")],
    };
    let plan = database
        .schema()
        .plan_migration("1", "create users", &current, &target)?;

    assert_eq!(plan.steps[0].risk, MigrationRisk::Additive);
    let err = database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect_err("plan-only should reject apply");
    assert!(
        err.to_string()
            .contains("does not allow apply schema migration")
    );
    Ok(())
}

#[tokio::test]
async fn managed_rejects_destructive_migration_by_default() -> Result<(), Box<dyn std::error::Error>>
{
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
    let database = graphql_orm::db::Database::new(pool);
    let current = SchemaModel {
        extensions: Vec::new(),
        tables: vec![users_v1("destructive_users")],
    };
    let target = SchemaModel {
        extensions: Vec::new(),
        tables: vec![],
    };
    let plan = database
        .schema()
        .plan_migration("2", "drop users", &current, &target)?;

    assert_eq!(plan.steps[0].risk, MigrationRisk::Destructive);
    let err = database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect_err("destructive migration should be rejected");
    assert!(err.to_string().contains("contains destructive step"));
    Ok(())
}

#[tokio::test]
async fn managed_schema_abi_applies_forward_path() -> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let table_name = format!("abi_users_{suffix}");
    let abi = SchemaAbi::new(vec![
        SchemaStage::from_schema_model(
            format!("{}_01", suffix),
            "create users",
            SchemaModel {
                extensions: Vec::new(),
                tables: vec![users_v1(&table_name)],
            },
        ),
        SchemaStage::from_schema_model(
            format!("{}_02", suffix),
            "add email",
            SchemaModel {
                extensions: Vec::new(),
                tables: vec![users_v2(&table_name)],
            },
        ),
    ])?;

    let applied = database
        .schema()
        .apply_upgrade(&abi, &format!("{}_02", suffix), ApplyOptions::default())
        .await?;

    assert_eq!(applied.applied.len(), 2);
    assert_eq!(
        database.schema().current_version().await?,
        Some(format!("{}_02", suffix))
    );
    let row = sqlx::query(
        "SELECT COUNT(*) AS count
         FROM pragma_table_info(?)
         WHERE name = 'email'",
    )
    .bind(&table_name)
    .fetch_one(&pool)
    .await?;
    assert_eq!(sqlx::Row::try_get::<i64, _>(&row, "count")?, 1);
    Ok(())
}
