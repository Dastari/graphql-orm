#![cfg(feature = "mssql")]
//! The restore-in-progress marker for host-run SQL Server restores.
//!
//! SQL Server has no ORM snapshot restore, so a host's own restore may commit
//! table by table. The marker makes a process that starts between the first
//! and the last restore write refuse to serve the partly restored database.

use graphql_orm::graphql::orm::RestoreInProgress;
use graphql_orm::prelude::*;

#[path = "support/owned_mssql.rs"]
mod owned_mssql;
use owned_mssql::{OwnedSqlServer, execute_batch};

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "dbo.GraphqlOrmRestoredRecords",
    plural = "RestoredRecords",
    schema_policy = "external_writable"
)]
struct RestoredRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    value: i64,
}

#[tokio::test]
#[ignore = "starts a test-owned SQL Server 2022 container"]
async fn a_restore_marker_blocks_start_until_the_restore_finishes()
-> Result<(), Box<dyn std::error::Error>> {
    let mut server = OwnedSqlServer::start().await?;
    execute_batch(
        &server.connection_string,
        "CREATE TABLE dbo.GraphqlOrmRestoredRecords (id NVARCHAR(100) NOT NULL PRIMARY KEY, value BIGINT NOT NULL)",
    )
    .await?;
    let pool =
        graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(&server.connection_string)
            .await?;
    let db = Database::<MssqlBackend>::builder(pool)
        .schema_policy(SchemaPolicy::ExternalWritable)
        .build();
    let entities = [<RestoredRecord as graphql_orm::graphql::orm::Entity>::metadata()];

    // No marker table, no restore: start-up checks pass.
    assert_eq!(db.restore_in_progress().await?, None);
    db.ensure_no_restore_in_progress().await?;
    db.schema().validate_against_entities(&entities).await?;

    // A restore that commits its first table and then fails.
    db.begin_restore_marker("snapshot-1").await?;
    execute_batch(
        &server.connection_string,
        "INSERT INTO dbo.GraphqlOrmRestoredRecords (id, value) VALUES (N'partial', 1)",
    )
    .await?;

    let marker = db.restore_in_progress().await?.expect("marker");
    assert_eq!(marker.restore_id, "snapshot-1");
    assert!(marker.started_at > 0);
    let refused = db
        .ensure_no_restore_in_progress()
        .await
        .expect_err("start is refused during a restore");
    assert_eq!(RestoreInProgress::from_error(&refused), Some(&marker));
    let refused = db
        .schema()
        .validate_against_entities(&entities)
        .await
        .expect_err("start-up validation is refused during a restore");
    assert_eq!(RestoreInProgress::from_error(&refused), Some(&marker));
    // A second process observes the same marker.
    let other = Database::<MssqlBackend>::builder(
        graphql_orm::db::mssql::MssqlPool::connect_ado(&server.connection_string).await?,
    )
    .schema_policy(SchemaPolicy::ExternalReadOnly)
    .build();
    assert_eq!(other.restore_in_progress().await?, Some(marker.clone()));

    // Another restore cannot start over it, and only the owner clears it.
    let second = db
        .begin_restore_marker("snapshot-2")
        .await
        .expect_err("one restore at a time");
    assert_eq!(RestoreInProgress::from_error(&second), Some(&marker));
    db.finish_restore_marker("snapshot-2")
        .await
        .expect_err("another restore's marker is never cleared");
    assert_eq!(db.restore_in_progress().await?, Some(marker));

    // The host removes the partial rows (or completes the restore) and clears
    // the marker; start-up checks pass again and a new restore can begin.
    execute_batch(
        &server.connection_string,
        "DELETE FROM dbo.GraphqlOrmRestoredRecords",
    )
    .await?;
    db.finish_restore_marker("snapshot-1").await?;
    assert_eq!(db.restore_in_progress().await?, None);
    db.ensure_no_restore_in_progress().await?;
    db.schema().validate_against_entities(&entities).await?;
    db.finish_restore_marker("snapshot-1")
        .await
        .expect_err("no marker is present");
    db.begin_restore_marker("snapshot-2").await?;
    db.finish_restore_marker("snapshot-2").await?;

    server.cleanup()?;
    Ok(())
}
