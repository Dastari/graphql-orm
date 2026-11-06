//! Implicit defaults are insert-only and depend on the Rust field type.
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
use graphql_orm::prelude::*;

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "mssql",
    repository_entity(
        backend = "mssql",
        table = "dbo.implicit_clocks",
        plural = "ImplicitClocks"
    )
)]
#[cfg_attr(
    not(feature = "mssql"),
    repository_entity(table = "implicit_clocks", plural = "ImplicitClocks")
)]
struct ImplicitClock {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[cfg_attr(feature = "mssql", graphql_orm(db_column = "[id]"))]
    id: i64,
    #[cfg_attr(feature = "mssql", graphql_orm(db_column = "[created_at]"))]
    created_at: String,
    #[cfg_attr(feature = "mssql", graphql_orm(db_column = "[updated_at]"))]
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
        .find(|c| c.name == "created_at" || c.name == "[created_at]")
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
    for legacy in [false, true] {
        sqlx::query("DROP TABLE IF EXISTS implicit_clocks")
            .execute(&pool)
            .await?;
        let mut physical = target.clone();
        if legacy {
            physical.tables[0]
                .columns
                .iter_mut()
                .find(|c| c.name == "created_at" || c.name == "[created_at]")
                .unwrap()
                .default = Some(defaults().1.into());
        }
        let plan = build_migration_plan(
            DatabaseBackend::Postgres,
            &SchemaModel {
                extensions: vec![],
                tables: vec![],
            },
            &physical,
        );
        for statement in &plan.statements {
            sqlx::query(statement).execute(&pool).await?;
        }
        sqlx::query("INSERT INTO implicit_clocks (id) VALUES (1)")
            .execute(&pool)
            .await?;
        let before: (String, i64) =
            sqlx::query_as("SELECT created_at, updated_at FROM implicit_clocks")
                .fetch_one(&pool)
                .await?;
        if legacy {
            assert_eq!(before.0.parse::<i64>()?, before.1);
        } else {
            check_values(&before.0, before.1);
        }
        let catalog: Vec<(String, String)> = sqlx::query_as("SELECT column_name::text, column_default::text FROM information_schema.columns WHERE table_name = 'implicit_clocks' AND column_default IS NOT NULL ORDER BY column_name").fetch_all(&pool).await?;
        println!("postgres legacy={legacy} catalog={catalog:?}");
        let reopened = Database::<PostgresBackend>::connect_postgres(&owned.url).await?;
        let plan = reopened
            .schema()
            .plan_migration_to_entities("reopen", "clocks", &[ImplicitClock::metadata()])
            .await?;
        assert!(plan.steps.is_empty(), "legacy={legacy}: {:?}", plan.steps);
        assert!(plan.statements.is_empty(), "{:?}", plan.statements);
        let after: (String, i64) =
            sqlx::query_as("SELECT created_at, updated_at FROM implicit_clocks")
                .fetch_one(reopened.pool())
                .await?;
        assert_eq!(before, after);
        let after_catalog: Vec<(String, String)> = sqlx::query_as("SELECT column_name::text, column_default::text FROM information_schema.columns WHERE table_name = 'implicit_clocks' AND column_default IS NOT NULL ORDER BY column_name").fetch_all(reopened.pool()).await?;
        assert_eq!(catalog, after_catalog);
        reopened.pool().close().await;
    }
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
    for legacy in [false, true] {
        client
            .simple_query("DROP TABLE IF EXISTS implicit_clocks")
            .await?
            .into_results()
            .await?;
        let mut physical = target.clone();
        if legacy {
            physical.tables[0]
                .columns
                .iter_mut()
                .find(|c| c.name == "created_at" || c.name == "[created_at]")
                .unwrap()
                .default = Some(defaults().1.into());
        }
        let plan = build_migration_plan(
            DatabaseBackend::Mssql,
            &SchemaModel {
                extensions: vec![],
                tables: vec![],
            },
            &physical,
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
        let before = (
            row.get::<&str, _>(0).unwrap().to_owned(),
            row.get::<i64, _>(1).unwrap(),
        );
        if legacy {
            assert_eq!(before.0.parse::<i64>()?, before.1);
        } else {
            check_values(&before.0, before.1);
        }
        let catalog_sql = "SELECT definition FROM sys.default_constraints WHERE parent_object_id = OBJECT_ID('implicit_clocks') ORDER BY parent_column_id";
        let rows = client
            .simple_query(catalog_sql)
            .await?
            .into_first_result()
            .await?;
        let catalog = rows
            .iter()
            .map(|row| row.get::<&str, _>(0).unwrap().to_owned())
            .collect::<Vec<_>>();
        println!("mssql legacy={legacy} catalog={catalog:?}");
        // Read-only connections may introspect; explicitly opt into planning,
        // without enabling schema application or entity writes on the pool.
        let pool = graphql_orm::db::mssql::MssqlPool::connect_ado(&owned.connection_string).await?;
        let reopened = Database::<MssqlBackend>::builder(pool)
            .schema_policy(SchemaPolicy::PlanOnly)
            .build();
        let plan = reopened
            .schema()
            .plan_migration_to_entities("reopen", "clocks", &[ImplicitClock::metadata()])
            .await?;
        assert!(plan.steps.is_empty(), "legacy={legacy}: {:?}", plan.steps);
        assert!(plan.statements.is_empty(), "{:?}", plan.statements);
        // Catalog lengths must stay meaningful: a genuinely narrower target
        // remains a type change, rather than equating bare NVARCHAR with MAX.
        use graphql_orm::graphql::orm::IntrospectionBackend;
        let live = MssqlBackend::introspect_schema(reopened.pool()).await?;
        let mut old_type_spelling = live.clone();
        old_type_spelling.tables[0]
            .columns
            .iter_mut()
            .find(|column| column.name == "[created_at]")
            .unwrap()
            .sql_type = "nvarchar".into();
        println!(
            "mssql legacy={legacy} physical hash before length correction={} after={}",
            old_type_spelling.stable_hash(),
            live.stable_hash()
        );
        assert_ne!(old_type_spelling.stable_hash(), live.stable_hash());
        let mut narrower = target.clone();
        narrower.tables[0]
            .columns
            .iter_mut()
            .find(|column| column.name == "[created_at]")
            .unwrap()
            .sql_type = "NVARCHAR(20)".into();
        assert!(
            !build_migration_plan(DatabaseBackend::Mssql, &live, &narrower)
                .steps
                .is_empty()
        );
        let row = client
            .simple_query("SELECT created_at, updated_at FROM implicit_clocks")
            .await?
            .into_row()
            .await?
            .unwrap();
        assert_eq!(
            before,
            (
                row.get::<&str, _>(0).unwrap().to_owned(),
                row.get::<i64, _>(1).unwrap()
            )
        );
        let rows = client
            .simple_query(catalog_sql)
            .await?
            .into_first_result()
            .await?;
        assert_eq!(
            catalog,
            rows.iter()
                .map(|row| row.get::<&str, _>(0).unwrap().to_owned())
                .collect::<Vec<_>>()
        );
    }
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

#[cfg(feature = "sqlite")]
#[test]
fn persisted_timestamp_default_hash_matches_base() {
    use graphql_orm::graphql::orm::{SchemaModel, canonicalize_column_default_expression};
    let defaults = [
        "now()",
        "(sysutcdatetime())",
        "(datediff_big(second,'1970-01-01',sysutcdatetime()))",
        "(datediff_big([second],'1970-01-01',sysutcdatetime()))",
    ];
    let mut schema = SchemaModel::from_entities(&[ImplicitClock::metadata()]);
    let template = schema.tables[0].columns[0].clone();
    schema.tables[0].columns = defaults
        .iter()
        .enumerate()
        .map(|(i, default)| {
            assert_eq!(
                canonicalize_column_default_expression(default),
                [
                    "now()",
                    "sysutcdatetime()",
                    "datediff_big(second,'1970-01-01',sysutcdatetime())",
                    "datediff_big([second],'1970-01-01',sysutcdatetime())"
                ][i]
            );
            let mut column = template.clone();
            column.name = ["a", "b", "c", "d"][i].into();
            column.default = Some((*default).into());
            column
        })
        .collect();
    // Computed with the base-ref canonicalizer and unchanged physical hash algorithm.
    assert_eq!(schema.stable_hash(), "f8b19d13ac384df0");
}

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(table = "optional_clocks", plural = "OptionalClocks")]
struct OptionalClock {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: i64,
    created_at: Option<String>,
    updated_at: Option<String>,
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn optional_text_defaults_and_mixed_integer_refresh() -> Result<(), Box<dyn std::error::Error>>
{
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let plan = db
        .schema()
        .plan_migration_to_entities(
            "init",
            "clocks",
            &[OptionalClock::metadata(), ImplicitClock::metadata()],
        )
        .await?;
    for sql in &plan.statements {
        sqlx::query(sql).execute(db.pool()).await?;
    }
    sqlx::query("INSERT INTO optional_clocks (id) VALUES (1)")
        .execute(db.pool())
        .await?;
    let (created, updated): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT created_at, updated_at FROM optional_clocks")
            .fetch_one(db.pool())
            .await?;
    for stamp in [created, updated] {
        assert!(chrono::DateTime::parse_from_rfc3339(&stamp.unwrap())?.timestamp() > 1_700_000_000);
    }
    sqlx::query("INSERT INTO implicit_clocks (id, updated_at) VALUES (1, 1)")
        .execute(db.pool())
        .await?;
    db.transaction(TransactionMode::Default, |tx| {
        Box::pin(async move {
            tx.update_by_id::<ImplicitClock>(&1, UpdateImplicitClockInput {})
                .await?;
            Ok::<_, OrmPublicError>(())
        })
    })
    .await?;
    let (created, updated): (String, i64) =
        sqlx::query_as("SELECT created_at, updated_at FROM implicit_clocks")
            .fetch_one(db.pool())
            .await?;
    check_values(&created, updated);
    Ok(())
}
