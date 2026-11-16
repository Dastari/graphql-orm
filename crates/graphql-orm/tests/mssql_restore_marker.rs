#![cfg(feature = "mssql")]
//! Host-run SQL Server restores: the restore lock and the crash marker.
//!
//! SQL Server has no ORM snapshot restore, so a host's own restore may commit
//! table by table. Every ORM transaction and autocommit statement takes a
//! shared application lock; the restore holds it exclusively. A restore
//! therefore waits for in-flight ORM work and nothing starts under it. A
//! marker row keeps statements refused after a restore whose session died.

use graphql_orm::graphql::orm::{
    MutationContext, RestoreInProgress, TransactionMode, WriteBackend,
};
use graphql_orm::prelude::*;
use std::time::Duration;

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

const COUNT: &str = "SELECT COUNT_BIG(*) AS n FROM dbo.GraphqlOrmRestoredRecords";
const INSERT: &str = "INSERT INTO dbo.GraphqlOrmRestoredRecords (id, value) VALUES (@P1, 1)";

type Db = Database<MssqlBackend>;

async fn writable(connection_string: &str) -> Result<Db, Box<dyn std::error::Error>> {
    let pool =
        graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(connection_string).await?;
    Ok(Database::<MssqlBackend>::builder(pool)
        .schema_policy(SchemaPolicy::ExternalWritable)
        .build())
}

async fn count(db: &Db) -> graphql_orm::Result<i64> {
    let rows = <MssqlBackend as OrmBackend>::fetch_rows(db.pool(), COUNT, &[]).await?;
    <MssqlBackend as OrmBackend>::try_get_i64(&rows[0], "n")
}

async fn insert(db: &Db, id: &str) -> graphql_orm::Result<u64> {
    let written = <MssqlBackend as WriteBackend>::execute_with_binds(
        db.pool(),
        INSERT,
        &[SqlValue::String(id.to_string())],
    )
    .await?;
    Ok(<MssqlBackend as WriteBackend>::rows_affected(&written))
}

/// Every way ordinary ORM work reaches the database is refused with the
/// typed error naming `restore_id`: an autocommit read and write, the start
/// of a transaction, and start-up validation.
async fn assert_refused(db: &Db, restore_id: &str) {
    let named = |error: &graphql_orm::Error, what: &str| {
        let marker = RestoreInProgress::from_error(error)
            .unwrap_or_else(|| panic!("{what}: expected RestoreInProgress, got {error}"));
        assert_eq!(marker.restore_id, restore_id, "{what}");
    };
    named(&count(db).await.expect_err("read"), "autocommit read");
    if db.pool().access_mode() == graphql_orm::db::mssql::MssqlAccessMode::ExternalWritable {
        named(
            &insert(db, "refused").await.expect_err("write"),
            "autocommit write",
        );
        match MutationContext::begin(db, TransactionMode::Default).await {
            Ok(_) => panic!("a transaction began during a restore"),
            Err(error) => named(&error, "transaction begin"),
        }
        match MutationContext::begin(db, TransactionMode::StateMachine).await {
            Ok(_) => panic!("a serializable transaction began during a restore"),
            Err(error) => named(&error, "serializable transaction begin"),
        }
    }
    named(
        &db.ensure_no_restore_in_progress()
            .await
            .expect_err("start-up check"),
        "start-up check",
    );
    named(
        &db.schema()
            .validate_against_entities(&[
                <RestoredRecord as graphql_orm::graphql::orm::Entity>::metadata(),
            ])
            .await
            .expect_err("validation"),
        "start-up validation",
    );
}

#[tokio::test]
#[ignore = "starts a test-owned SQL Server 2022 container"]
async fn a_restore_excludes_orm_readers_and_writers_until_it_finishes()
-> Result<(), Box<dyn std::error::Error>> {
    let mut server = OwnedSqlServer::start().await?;
    execute_batch(
        &server.connection_string,
        "CREATE TABLE dbo.GraphqlOrmRestoredRecords (id NVARCHAR(100) NOT NULL PRIMARY KEY, value BIGINT NOT NULL)",
    )
    .await?;
    let db = writable(&server.connection_string).await?;
    // A second process, with its own pool, and a read-only one.
    let other = writable(&server.connection_string).await?;
    let reader = Database::<MssqlBackend>::builder(
        graphql_orm::db::mssql::MssqlPool::connect_ado(&server.connection_string).await?,
    )
    .schema_policy(SchemaPolicy::ExternalReadOnly)
    .build();
    let entities = [<RestoredRecord as graphql_orm::graphql::orm::Entity>::metadata()];

    // No restore: everything works, and a write reports its own row count.
    assert_eq!(db.restore_in_progress().await?, None);
    db.ensure_no_restore_in_progress().await?;
    db.schema().validate_against_entities(&entities).await?;
    assert_eq!(insert(&db, "before").await?, 1);
    assert_eq!(count(&reader).await?, 1);

    // A transaction in flight holds the shared lock: the restore cannot start
    // under it, and the transaction is not disturbed.
    let mut in_flight = MutationContext::begin(&other, TransactionMode::Default).await?;
    in_flight
        .execute(INSERT, &[SqlValue::String("in-flight".into())])
        .await?;
    let blocked = db
        .begin_restore("snapshot-1", Duration::from_millis(300))
        .await;
    match blocked {
        Ok(_) => panic!("a restore started under an open transaction"),
        Err(error) => assert!(
            RestoreInProgress::from_error(&error).is_some(),
            "expected the typed refusal, got {error}"
        ),
    }
    assert_eq!(db.restore_in_progress().await?, None, "no marker was left");
    in_flight.commit_and_emit().await?;
    assert_eq!(count(&db).await?, 2);

    // The restore holds the lock. A transaction or statement started now, in
    // any process, is refused before it reads or writes.
    let mut restore = db
        .begin_restore("snapshot-1", Duration::from_secs(10))
        .await?;
    assert_eq!(restore.restore_id(), "snapshot-1");
    let marker = db.restore_in_progress().await?.expect("marker");
    assert_eq!(marker.restore_id, "snapshot-1");
    assert!(marker.started_at > 0);
    assert_refused(&db, "snapshot-1").await;
    assert_refused(&other, "snapshot-1").await;
    assert_refused(&reader, "snapshot-1").await;
    let second = other
        .begin_restore("snapshot-2", Duration::from_millis(300))
        .await;
    assert!(
        matches!(&second, Err(error) if RestoreInProgress::from_error(error).is_some()),
        "one restore at a time"
    );

    // The restore itself reads and writes through its guard.
    assert_eq!(
        restore
            .execute("DELETE FROM dbo.GraphqlOrmRestoredRecords", &[])
            .await?,
        2
    );
    restore
        .execute(INSERT, &[SqlValue::String("restored".into())])
        .await?;
    assert_eq!(restore.fetch_rows(COUNT, &[]).await?.len(), 1);
    restore.finish().await?;

    // Finished: no marker, no lock, ordinary work resumes everywhere.
    assert_eq!(db.restore_in_progress().await?, None);
    db.schema().validate_against_entities(&entities).await?;
    assert_eq!(count(&reader).await?, 1);
    assert_eq!(insert(&other, "after").await?, 1);
    let mut tx = MutationContext::begin(&db, TransactionMode::StateMachine).await?;
    tx.execute(INSERT, &[SqlValue::String("after-tx".into())])
        .await?;
    tx.commit_and_emit().await?;
    assert_eq!(count(&db).await?, 3);

    server.cleanup()?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a test-owned SQL Server 2022 container"]
async fn a_crashed_restore_leaves_a_marker_that_keeps_statements_refused()
-> Result<(), Box<dyn std::error::Error>> {
    let mut server = OwnedSqlServer::start().await?;
    execute_batch(
        &server.connection_string,
        "CREATE TABLE dbo.GraphqlOrmRestoredRecords (id NVARCHAR(100) NOT NULL PRIMARY KEY, value BIGINT NOT NULL)",
    )
    .await?;
    let db = writable(&server.connection_string).await?;

    // The restore commits a partial row and its process dies: the guard is
    // dropped without `finish`, which closes its connection.
    let mut restore = db
        .begin_restore("snapshot-1", Duration::from_secs(10))
        .await?;
    restore
        .execute(INSERT, &[SqlValue::String("partial".into())])
        .await?;
    drop(restore);

    // A process started afterwards has no lock to run into, only the marker.
    let restarted = writable(&server.connection_string).await?;
    assert_refused(&restarted, "snapshot-1").await;
    assert_refused(&db, "snapshot-1").await;

    // Another restore cannot start over the stale marker, and only the
    // restore it names can be resumed.
    for attempt in [
        restarted
            .begin_restore("snapshot-2", Duration::from_secs(10))
            .await,
        restarted
            .resume_restore("snapshot-2", Duration::from_secs(10))
            .await,
    ] {
        match attempt {
            Ok(_) => panic!("a different restore adopted the stale marker"),
            Err(error) => assert_eq!(
                RestoreInProgress::from_error(&error).map(|marker| marker.restore_id.as_str()),
                Some("snapshot-1"),
                "{error}"
            ),
        }
    }
    assert_refused(&restarted, "snapshot-1").await;

    // Resuming adopts the marker under the lock; the host removes the partial
    // rows (or completes the restore) and finishes.
    let mut resumed = restarted
        .resume_restore("snapshot-1", Duration::from_secs(10))
        .await?;
    assert_eq!(
        resumed
            .execute("DELETE FROM dbo.GraphqlOrmRestoredRecords", &[])
            .await?,
        1
    );
    // `finish` never discards an open transaction on the restore connection:
    // it fails and the marker stays.
    // (SQL Server reports the unbalanced batch; the transaction stays open.)
    let _ = resumed.execute("BEGIN TRANSACTION", &[]).await;
    resumed
        .finish()
        .await
        .expect_err("finish is refused with an open transaction");
    assert_refused(&restarted, "snapshot-1").await;
    let resumed = restarted
        .resume_restore("snapshot-1", Duration::from_secs(10))
        .await?;
    resumed.finish().await?;

    assert_eq!(restarted.restore_in_progress().await?, None);
    assert_eq!(count(&restarted).await?, 0);
    assert_eq!(insert(&db, "after").await?, 1);
    restarted
        .resume_restore("snapshot-1", Duration::from_secs(1))
        .await
        .err()
        .expect("no marker is left to resume");
    assert_eq!(
        count(&db).await?,
        1,
        "the refused resume left no marker behind"
    );
    assert_eq!(db.restore_in_progress().await?, None);

    server.cleanup()?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a test-owned SQL Server 2022 container"]
async fn marker_visibility_is_independent_of_principal_and_permission()
-> Result<(), Box<dyn std::error::Error>> {
    let mut server = OwnedSqlServer::start().await?;
    execute_batch(&server.connection_string,
        "CREATE TABLE dbo.GraphqlOrmRestoredRecords (id NVARCHAR(100) NOT NULL PRIMARY KEY, value BIGINT NOT NULL);
         EXEC(N'CREATE SCHEMA other_schema');
         CREATE TABLE other_schema.__graphql_orm_restore_marker (marker_key TINYINT NOT NULL PRIMARY KEY, restore_id NVARCHAR(200) NOT NULL, started_at BIGINT NOT NULL);"
    ).await?;
    for (login, schema) in [
        ("marker_a", "dbo"),
        ("marker_b", "other_schema"),
        ("marker_denied", "other_schema"),
    ] {
        execute_batch(
            &server.connection_string,
            &format!(
            "CREATE LOGIN [{login}] WITH PASSWORD = 'Owned-marker-test-93!', CHECK_POLICY = OFF;
             CREATE USER [{login}] FOR LOGIN [{login}] WITH DEFAULT_SCHEMA = [{schema}];
             GRANT SELECT, INSERT ON dbo.GraphqlOrmRestoredRecords TO [{login}];"
        ),
        )
        .await?;
    }
    let owner = writable(&server.connection_string).await?;
    let restore = owner
        .begin_restore("stale", Duration::from_secs(10))
        .await?;
    drop(restore);
    execute_batch(
        &server.connection_string,
        "GRANT SELECT ON dbo.__graphql_orm_restore_marker TO marker_a;
         GRANT SELECT ON dbo.__graphql_orm_restore_marker TO marker_b;
         GRANT SELECT ON other_schema.__graphql_orm_restore_marker TO marker_b;
         DENY SELECT ON dbo.__graphql_orm_restore_marker TO marker_denied;",
    )
    .await?;
    let mut clients = Vec::new();
    for login in ["marker_a", "marker_b", "marker_denied"] {
        let connection = server
            .connection_string
            .split(';')
            .map(|part| {
                if part.starts_with("user id=") {
                    format!("user id={login}")
                } else if part.starts_with("password=") {
                    "password=Owned-marker-test-93!".into()
                } else {
                    part.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(";");
        let db = writable(&connection).await?;
        assert_refused(
            &db,
            if login == "marker_denied" {
                ""
            } else {
                "stale"
            },
        )
        .await;
        clients.push(db);
    }
    owner
        .resume_restore("stale", Duration::from_secs(10))
        .await?
        .finish()
        .await?;
    for (expected, db) in clients[..2].iter().enumerate() {
        assert_eq!(count(db).await?, expected as i64);
        insert(db, &graphql_orm::uuid::Uuid::new_v4().to_string()).await?;
        let tx = MutationContext::begin(db, TransactionMode::Default).await?;
        tx.commit_and_emit().await?;
    }
    assert_refused(&clients[2], "").await;
    execute_batch(
        &server.connection_string,
        "DROP TABLE dbo.__graphql_orm_restore_marker",
    )
    .await?;
    assert_refused(&owner, "").await;
    owner.pool().install_restore_marker().await?;
    assert_eq!(count(&owner).await?, 2);
    server.cleanup()?;
    Ok(())
}
