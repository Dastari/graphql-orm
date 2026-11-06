//! Implicit defaults are insert-only and depend on the Rust field type.
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
use graphql_orm::prelude::*;

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(table = "implicit_clocks", plural = "ImplicitClocks")]
struct ImplicitClock {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: i64,
    created_at: String,
    updated_at: i64,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "text_update_clocks", plural = "TextUpdateClocks")]
struct TextUpdateClock {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    label: String,
    updated_at: String,
}

fn defaults() -> (&'static str, &'static str) {
    let columns = &ImplicitClock::metadata().fields;
    (
        columns
            .iter()
            .find(|c| c.rust_name == "created_at")
            .unwrap()
            .default
            .unwrap(),
        columns
            .iter()
            .find(|c| c.rust_name == "updated_at")
            .unwrap()
            .default
            .unwrap(),
    )
}

fn check_values(text: &str, epoch: i64) {
    let parsed =
        chrono::DateTime::parse_from_rfc3339(text).expect("String default must be RFC 3339");
    assert_eq!(parsed.offset().local_minus_utc(), 0);
    assert!((parsed.timestamp() - epoch).abs() <= 1);
    assert!(epoch > 1_700_000_000);
}

#[test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "included in the owned backend runner"
)]
fn generated_metadata_retains_legacy_text_defaults_without_a_plan() {
    use graphql_orm::graphql::orm::{DatabaseBackend, SchemaModel, build_migration_plan};
    #[cfg(feature = "sqlite")]
    let backend = DatabaseBackend::Sqlite;
    #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
    let backend = DatabaseBackend::Postgres;
    #[cfg(all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))))]
    let backend = DatabaseBackend::Mssql;
    let target = SchemaModel::from_entities(&[ImplicitClock::metadata()]);
    let mut legacy = target.clone();
    legacy.tables[0]
        .columns
        .iter_mut()
        .find(|c| c.name == "created_at")
        .unwrap()
        .default = Some(defaults().1.to_owned());
    let plan = build_migration_plan(backend, &legacy, &target);
    assert!(plan.steps.is_empty(), "{:?}", plan.steps);
    assert!(plan.statements.is_empty());
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_defaults_and_reopen_preserve_new_and_legacy_rows()
-> Result<(), Box<dyn std::error::Error>> {
    let (text, epoch) = defaults();
    let file = std::env::temp_dir().join(format!(
        "implicit-clocks-{}.sqlite",
        graphql_orm::uuid::Uuid::new_v4()
    ));
    std::fs::File::create(&file)?;
    let url = format!("sqlite://{}", file.display());
    for legacy in [false, true] {
        let db = Database::<SqliteBackend>::connect_sqlite(&url).await?;
        sqlx::query("DROP TABLE IF EXISTS implicit_clocks")
            .execute(db.pool())
            .await?;
        // Build through the real planner for the new default; replace just the
        // default for a pre-upgrade physical schema.
        let plan = db
            .schema()
            .plan_migration_to_entities("init", "clocks", &[ImplicitClock::metadata()])
            .await?;
        for statement in &plan.statements {
            let statement = if legacy {
                statement.replace(text, epoch)
            } else {
                statement.clone()
            };
            sqlx::query(&statement).execute(db.pool()).await?;
        }
        sqlx::query("INSERT INTO implicit_clocks (id) VALUES (1)")
            .execute(db.pool())
            .await?;
        let before: (String, i64) =
            sqlx::query_as("SELECT created_at, updated_at FROM implicit_clocks")
                .fetch_one(db.pool())
                .await?;
        if legacy {
            assert_eq!(before.0.parse::<i64>()?, before.1);
        } else {
            check_values(&before.0, before.1);
        }
        // Defaults do not act as ON UPDATE expressions.
        sqlx::query("UPDATE implicit_clocks SET id = 2")
            .execute(db.pool())
            .await?;
        db.pool().close().await;
        let reopened = Database::<SqliteBackend>::connect_sqlite(&url).await?;
        let plan = reopened
            .schema()
            .plan_migration_to_entities("reopen", "clocks", &[ImplicitClock::metadata()])
            .await?;
        assert!(
            plan.statements.is_empty(),
            "legacy={legacy}: {:?}",
            plan.statements
        );
        assert!(plan.steps.is_empty(), "legacy={legacy}: {:?}", plan.steps);
        let after: (String, i64) =
            sqlx::query_as("SELECT created_at, updated_at FROM implicit_clocks")
                .fetch_one(reopened.pool())
                .await?;
        assert_eq!(before, after);
        reopened.pool().close().await;
    }
    std::fs::remove_file(file)?;
    Ok(())
}

#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "run only through scripts/run-owned-database-lanes.sh postgres"]
async fn postgres_defaults_parse() -> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::{DatabaseBackend, SchemaModel, build_migration_plan};
    let mut owned = owned_postgres::OwnedPostgres::start("implicit-clocks")?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&owned.url)
        .await?;
    let target = SchemaModel::from_entities(&[ImplicitClock::metadata()]);
    let plan = build_migration_plan(
        DatabaseBackend::Postgres,
        &SchemaModel {
            extensions: vec![],
            tables: vec![],
        },
        &target,
    );
    for statement in &plan.statements {
        sqlx::query(statement).execute(&pool).await?;
    }
    sqlx::query("INSERT INTO implicit_clocks (id) VALUES (1)")
        .execute(&pool)
        .await?;
    let row: (String, i64) = sqlx::query_as("SELECT created_at, updated_at FROM implicit_clocks")
        .fetch_one(&pool)
        .await?;
    check_values(&row.0, row.1);
    pool.close().await;
    owned.cleanup()?;
    Ok(())
}

#[cfg(feature = "mssql")]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;

#[cfg(feature = "mssql")]
#[tokio::test]
#[ignore = "run only through scripts/run-owned-database-lanes.sh mssql"]
async fn mssql_defaults_parse() -> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::{DatabaseBackend, SchemaModel, build_migration_plan};
    use graphql_orm::tokio_util::compat::TokioAsyncWriteCompatExt;
    let mut owned = owned_mssql::OwnedSqlServer::start().await?;
    let config = graphql_orm::tiberius::Config::from_ado_string(&owned.connection_string)?;
    let tcp = tokio::net::TcpStream::connect(config.get_addr()).await?;
    let mut client = graphql_orm::tiberius::Client::connect(config, tcp.compat_write()).await?;
    let target = SchemaModel::from_entities(&[ImplicitClock::metadata()]);
    let plan = build_migration_plan(
        DatabaseBackend::Mssql,
        &SchemaModel {
            extensions: vec![],
            tables: vec![],
        },
        &target,
    );
    for statement in &plan.statements {
        client.simple_query(statement).await?.into_results().await?;
    }
    client
        .simple_query("INSERT INTO implicit_clocks (id) VALUES (1)")
        .await?
        .into_results()
        .await?;
    let row = client
        .simple_query("SELECT created_at, updated_at FROM implicit_clocks")
        .await?
        .into_row()
        .await?
        .unwrap();
    check_values(
        row.get::<&str, _>(0).unwrap(),
        row.get::<i64, _>(1).unwrap(),
    );
    drop(client);
    owned.cleanup()?;
    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn omitted_updated_at_is_refreshed_by_generated_writes()
-> Result<(), Box<dyn std::error::Error>> {
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    sqlx::query("CREATE TABLE text_update_clocks (id TEXT PRIMARY KEY, label TEXT NOT NULL, updated_at TEXT NOT NULL)").execute(db.pool()).await?;
    sqlx::query("INSERT INTO text_update_clocks VALUES ('one', 'before', '2000-01-01T00:00:00Z')")
        .execute(db.pool())
        .await?;
    db.transaction(TransactionMode::Default, |tx| {
        Box::pin(async move {
            tx.update_by_id::<TextUpdateClock>(
                &"one".to_string(),
                UpdateTextUpdateClockInput {
                    label: Some("after".into()),
                },
            )
            .await?;
            Ok::<_, OrmPublicError>(())
        })
    })
    .await?;
    let (label, stamp): (String, String) =
        sqlx::query_as("SELECT label, updated_at FROM text_update_clocks")
            .fetch_one(db.pool())
            .await?;
    assert_eq!(label, "after");
    assert!(chrono::DateTime::parse_from_rfc3339(&stamp)?.timestamp() > 1_700_000_000);
    Ok(())
}
