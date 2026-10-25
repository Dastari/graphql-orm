//! Backup and restore of a static entity with a composite primary key. Before composite-key
//! descriptors, its rows were identified by the first key column only and collided.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use graphql_orm::graphql::orm::*;
use graphql_orm::prelude::*;
use std::collections::BTreeMap;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(table = "sc_memberships", plural = "ScMemberships")]
struct ScMembership {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    team: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    member: i64,
    role: String,
}

fn entities() -> Vec<&'static EntityMetadata> {
    vec![<ScMembership as Entity>::metadata()]
}
#[cfg(feature = "sqlite")]
async fn database() -> graphql_orm::db::Database {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    schema(graphql_orm::db::Database::new(pool)).await
}
#[cfg(feature = "postgres")]
async fn database() -> graphql_orm::db::Database {
    let pool = owned_postgres::OwnedPostgres::pool("static-composite-backup", 2)
        .await
        .unwrap();
    schema(graphql_orm::db::Database::new(pool)).await
}
async fn schema(database: graphql_orm::db::Database) -> graphql_orm::db::Database {
    let plan = build_migration_plan(
        if cfg!(feature = "postgres") {
            DatabaseBackend::Postgres
        } else {
            DatabaseBackend::Sqlite
        },
        &SchemaModel {
            extensions: Vec::new(),
            tables: Vec::new(),
        },
        &SchemaModel::from_entities(&entities()),
    );
    for statement in &plan.statements {
        sqlx::query(statement)
            .execute(database.pool())
            .await
            .unwrap();
    }
    database
}
async fn insert(
    database: &graphql_orm::db::Database,
    team: &str,
    member: i64,
    role: &str,
) -> graphql_orm::Result<()> {
    ScMembership::insert(
        database,
        CreateScMembershipInput {
            team: team.into(),
            member,
            role: role.into(),
        },
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn a_static_composite_key_entity_round_trips_without_colliding_rows() {
    let source = database().await;
    for (team, member, role) in [("a", 1, "owner"), ("a", 2, "member"), ("b", 1, "member")] {
        insert(&source, team, member, role).await.unwrap();
    }
    let descriptors = source.list_backup_entities(&entities());
    assert_eq!(descriptors[0].key_columns(), ["team", "member"]);
    let mut snap = source.begin_consistent_snapshot().await.unwrap();
    let rows = source
        .export_table_rows(&mut snap, &descriptors[0])
        .await
        .unwrap();
    drop(snap);
    assert_eq!(
        rows.iter()
            .map(|r| r.primary_key.as_str())
            .collect::<Vec<_>>(),
        [r#"["a","1"]"#, r#"["a","2"]"#, r#"["b","1"]"#]
    );

    let target = database().await;
    let snapshot = source.schema_snapshot("composite", &entities());
    let rows_by_table = BTreeMap::from([("sc_memberships".to_string(), rows)]);
    let reports = target
        .restore_backup_rows(
            &snapshot,
            &snapshot,
            &rows_by_table,
            &RestoreContext::empty_database(),
        )
        .await
        .unwrap();
    assert_eq!(reports[0].rows_imported, 3);
    let restored: Vec<(String, i64, String)> =
        sqlx::query_as("SELECT team, member, role FROM sc_memberships ORDER BY team, member")
            .fetch_all(target.pool())
            .await
            .unwrap();
    assert_eq!(
        restored,
        [
            ("a".into(), 1, "owner".into()),
            ("a".into(), 2, "member".into()),
            ("b".into(), 1, "member".into())
        ]
    );
}

/// The change journal cannot name a composite key, so it refuses to record one instead of
/// producing incremental chains that resurrect or duplicate rows.
#[cfg(feature = "change-journal")]
#[tokio::test]
async fn the_change_journal_refuses_composite_key_writes() {
    let database = database().await.with_change_journal();
    database.ensure_change_journal_table().await.unwrap();
    let error = insert(&database, "a", 1, "owner").await.unwrap_err();
    assert!(
        error.to_string().contains("composite-key entity"),
        "{error}"
    );
}
