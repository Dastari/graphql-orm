#![cfg(feature = "mssql")]
//! Host-run SQL Server restores: the restore lock and the crash marker.
//!
//! SQL Server has no ORM snapshot restore, so a host's own restore may commit
//! table by table. Every ORM transaction and autocommit statement takes a
//! shared application lock; the restore holds it exclusively. A restore
//! therefore waits for in-flight ORM work and nothing starts under it. A
//! marker row keeps statements refused after a restore whose session died.

use graphql_orm::graphql::orm::{
    MutationContext, RestoreInProgress, RestoreStateUnavailable, RestoreStateUnavailableReason,
    TransactionMode, WriteBackend,
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

/// Every way ordinary ORM work reaches the database is refused, and
/// `check` is told which error each one produced.
async fn assert_refused_with(db: &Db, check: impl Fn(&graphql_orm::Error, &str)) {
    check(&count(db).await.expect_err("read"), "autocommit read");
    if db.pool().access_mode() == graphql_orm::db::mssql::MssqlAccessMode::ExternalWritable {
        check(
            &insert(db, "refused").await.expect_err("write"),
            "autocommit write",
        );
        match MutationContext::begin(db, TransactionMode::Default).await {
            Ok(_) => panic!("a transaction began while restore state was refused"),
            Err(error) => check(&error, "transaction begin"),
        }
        match MutationContext::begin(db, TransactionMode::StateMachine).await {
            Ok(_) => panic!("a serializable transaction began while restore state was refused"),
            Err(error) => check(&error, "serializable transaction begin"),
        }
    }
    check(
        &db.ensure_no_restore_in_progress()
            .await
            .expect_err("start-up check"),
        "start-up check",
    );
    check(
        &db.schema()
            .validate_against_entities(&[
                <RestoredRecord as graphql_orm::graphql::orm::Entity>::metadata(),
            ])
            .await
            .expect_err("validation"),
        "start-up validation",
    );
}

/// Refused with the typed error naming `restore_id`.
async fn assert_refused(db: &Db, restore_id: &str) {
    assert_refused_with(db, |error, what| {
        let marker = RestoreInProgress::from_error(error)
            .unwrap_or_else(|| panic!("{what}: expected RestoreInProgress, got {error}"));
        assert_eq!(marker.restore_id, restore_id, "{what}");
    })
    .await;
}

/// Refused as unavailable restore state for `reason`: never as a restore in
/// progress, with a message that names the fix and holds no SQL text.
async fn assert_unavailable(db: &Db, reason: RestoreStateUnavailableReason) {
    assert_refused_with(db, |error, what| {
        assert!(
            RestoreInProgress::from_error(error).is_none(),
            "{what}: reported a restore in progress: {error}"
        );
        let unavailable = RestoreStateUnavailable::from_error(error)
            .unwrap_or_else(|| panic!("{what}: expected RestoreStateUnavailable, got {error}"));
        assert_eq!(unavailable.reason, reason, "{what}: {error}");
        let message = error.to_string();
        assert!(!message.contains("restore started at"), "{what}: {message}");
        assert!(
            !message.to_ascii_uppercase().contains("SELECT RESTORE_ID"),
            "{what}: {message}"
        );
        match reason {
            RestoreStateUnavailableReason::MarkerNotInstalled => {
                assert!(message.contains("install_restore_marker()"), "{message}")
            }
            RestoreStateUnavailableReason::PermissionDenied => {
                assert!(message.contains("grant SELECT"), "{message}")
            }
            _ => {}
        }
        assert!(
            std::error::Error::source(unavailable).is_some(),
            "{what}: the cause is kept for diagnostics"
        );
    })
    .await;
    let error = db.restore_in_progress().await.expect_err("marker read");
    assert_eq!(
        RestoreStateUnavailable::from_error(&error).map(|value| value.reason),
        Some(reason),
        "{error}"
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
        if login == "marker_denied" {
            assert_unavailable(&db, RestoreStateUnavailableReason::PermissionDenied).await;
        } else {
            assert_refused(&db, "stale").await;
        }
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
    assert_unavailable(&clients[2], RestoreStateUnavailableReason::PermissionDenied).await;
    execute_batch(
        &server.connection_string,
        "DROP TABLE dbo.__graphql_orm_restore_marker",
    )
    .await?;
    assert_unavailable(&owner, RestoreStateUnavailableReason::MarkerNotInstalled).await;
    // A restore cannot claim a marker that is not installed either.
    let claimed = owner
        .begin_restore("no-marker", Duration::from_secs(1))
        .await;
    match claimed {
        Ok(_) => panic!("a restore started without a marker table"),
        Err(error) => assert_eq!(
            RestoreStateUnavailable::from_error(&error).map(|value| value.reason),
            Some(RestoreStateUnavailableReason::MarkerNotInstalled),
            "{error}"
        ),
    }
    assert_unavailable(&owner, RestoreStateUnavailableReason::MarkerNotInstalled).await;
    owner.pool().install_restore_marker().await?;
    assert_eq!(count(&owner).await?, 2);
    server.cleanup()?;
    Ok(())
}

/// Replicas that install the marker at the same moment must all succeed.
#[tokio::test]
#[ignore = "starts a test-owned SQL Server 2022 container"]
async fn concurrent_marker_installs_all_succeed() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = OwnedSqlServer::start().await?;
    execute_batch(
        &server.connection_string,
        "DROP TABLE dbo.__graphql_orm_restore_marker",
    )
    .await?;
    let mut installs = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let connection = server.connection_string.clone();
        installs.spawn(async move {
            let pool =
                graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(&connection)
                    .await?;
            pool.install_restore_marker().await?;
            // And again: installing twice is a no-op.
            pool.install_restore_marker().await
        });
    }
    while let Some(joined) = installs.join_next().await {
        joined?.map_err(|error| format!("concurrent install failed: {error}"))?;
    }
    let db = writable(&server.connection_string).await?;
    assert_eq!(db.restore_in_progress().await?, None);
    // The installed table is the one a restore uses.
    db.begin_restore("after-install", Duration::from_secs(10))
        .await?
        .finish()
        .await?;

    // A non-table object in the way (error 2714 on create) is not mistaken
    // for an installed marker.
    execute_batch(
        &server.connection_string,
        "DROP TABLE dbo.__graphql_orm_restore_marker",
    )
    .await?;
    execute_batch(
        &server.connection_string,
        "CREATE VIEW dbo.__graphql_orm_restore_marker AS SELECT CAST(1 AS TINYINT) AS marker_key",
    )
    .await?;
    let error = db
        .pool()
        .install_restore_marker()
        .await
        .expect_err("a view is not the marker table");
    assert!(error.to_string().contains("documented shape"), "{error}");
    execute_batch(
        &server.connection_string,
        "DROP VIEW dbo.__graphql_orm_restore_marker",
    )
    .await?;

    // A table of the wrong shape is refused at install, not at runtime.
    execute_batch(
        &server.connection_string,
        "CREATE TABLE dbo.__graphql_orm_restore_marker (restore_id NVARCHAR(10) NULL)",
    )
    .await?;
    let error = db
        .pool()
        .install_restore_marker()
        .await
        .expect_err("wrong shape");
    assert!(error.to_string().contains("documented shape"), "{error}");
    server.cleanup()?;
    Ok(())
}

/// The install writes, so a physically read-only pool is refused.
#[tokio::test]
async fn installing_the_marker_needs_a_writable_pool() -> Result<(), Box<dyn std::error::Error>> {
    let connection = "server=tcp:127.0.0.1,1;database=test;user id=test;password=test";
    let read_only = graphql_orm::db::mssql::MssqlPool::connect_ado(connection).await?;
    let error = read_only
        .install_restore_marker()
        .await
        .expect_err("a read-only pool installs nothing");
    assert!(error.to_string().contains("read-only"), "{error}");
    Ok(())
}

/// A pool that cannot reach the database cannot read restore state: that is
/// unavailable state of the connection kind, never a restore in progress.
#[tokio::test]
async fn a_connection_failure_is_not_a_restore_in_progress()
-> Result<(), Box<dyn std::error::Error>> {
    let connection = "server=tcp:127.0.0.1,1;database=test;user id=test;password=test";
    let options = graphql_orm::db::mssql::MssqlPoolOptions::new()
        .connect_timeout(Duration::from_secs(2))
        .acquire_timeout(Duration::from_secs(5));
    let pool =
        graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable_with(connection, options)
            .await?;
    let db = Database::<MssqlBackend>::builder(pool)
        .schema_policy(SchemaPolicy::ExternalWritable)
        .build();
    for error in [
        db.restore_in_progress().await.expect_err("marker read"),
        db.ensure_no_restore_in_progress()
            .await
            .expect_err("start-up check"),
        db.schema()
            .validate_against_entities(&[
                <RestoredRecord as graphql_orm::graphql::orm::Entity>::metadata(),
            ])
            .await
            .expect_err("validation"),
        db.begin_restore("never", Duration::from_secs(1))
            .await
            .map(|_| ())
            .expect_err("begin_restore"),
    ] {
        assert!(
            RestoreInProgress::from_error(&error).is_none(),
            "a connection failure was reported as a restore: {error}"
        );
        // Statements refused at the connection level are never rewritten into
        // a restore error either.
        if let Some(unavailable) = RestoreStateUnavailable::from_error(&error) {
            assert_eq!(
                unavailable.reason,
                RestoreStateUnavailableReason::ConnectionFailed,
                "{error}"
            );
            assert!(error.to_string().contains("not a restore in progress"));
        }
    }
    let marker = db.restore_in_progress().await.expect_err("marker read");
    assert_eq!(
        RestoreStateUnavailable::from_error(&marker).map(|value| value.reason),
        Some(RestoreStateUnavailableReason::ConnectionFailed),
        "{marker}"
    );
    Ok(())
}

/// Connection string for `login`, built from the owned server's.
fn as_login(connection_string: &str, login: &str, password: &str) -> String {
    connection_string
        .split(';')
        .map(|part| {
            if part.starts_with("user id=") {
                format!("user id={login}")
            } else if part.starts_with("password=") {
                format!("password={password}")
            } else {
                part.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// #744: once the gate has seen a restore (its lock held), a follow-up marker
/// read that fails does not turn the refusal into "not a restore in
/// progress". It stays `RestoreInProgress`, with the identifier unknown.
#[tokio::test]
#[ignore = "starts a test-owned SQL Server 2022 container"]
async fn a_seen_restore_stays_a_restore_when_its_marker_cannot_be_read()
-> Result<(), Box<dyn std::error::Error>> {
    let mut server = OwnedSqlServer::start().await?;
    execute_batch(
        &server.connection_string,
        "CREATE TABLE dbo.GraphqlOrmRestoredRecords (id NVARCHAR(100) NOT NULL PRIMARY KEY, value BIGINT NOT NULL);
         CREATE LOGIN [marker_blind] WITH PASSWORD = 'Owned-marker-test-93!', CHECK_POLICY = OFF;
         CREATE USER [marker_blind] FOR LOGIN [marker_blind];
         GRANT SELECT, INSERT ON dbo.GraphqlOrmRestoredRecords TO [marker_blind];
         DENY SELECT ON dbo.__graphql_orm_restore_marker TO [marker_blind];",
    )
    .await?;
    let owner = writable(&server.connection_string).await?;
    let blind = writable(&as_login(
        &server.connection_string,
        "marker_blind",
        "Owned-marker-test-93!",
    ))
    .await?;
    // No restore: the gate cannot read the marker, which is unavailable state.
    let error = count(&blind).await.expect_err("marker unreadable");
    assert_eq!(
        RestoreStateUnavailable::from_error(&error).map(|value| value.reason),
        Some(RestoreStateUnavailableReason::PermissionDenied),
        "{error}"
    );
    // A restore holds the lock: the gate refuses because of it, and the
    // marker read that would name it fails.
    let restore = owner
        .begin_restore("snapshot-hidden", Duration::from_secs(10))
        .await?;
    for (what, error) in [
        ("read", count(&blind).await.expect_err("read")),
        ("write", insert(&blind, "refused").await.expect_err("write")),
        (
            "transaction",
            MutationContext::begin(&blind, TransactionMode::Default)
                .await
                .map(|_| ())
                .expect_err("transaction"),
        ),
    ] {
        let marker = RestoreInProgress::from_error(&error)
            .unwrap_or_else(|| panic!("{what}: expected RestoreInProgress, got {error}"));
        assert_eq!(marker.restore_id, "", "{what}: the identifier is unknown");
        assert_eq!(marker.started_at, 0, "{what}");
        assert!(
            !error.to_string().contains("not a restore in progress"),
            "{what}: {error}"
        );
    }
    restore.finish().await?;
    server.cleanup()?;
    Ok(())
}

/// #744: the session reset also releases the marker install lock, so a
/// session that still held it cannot block every later install.
#[tokio::test]
#[ignore = "starts a test-owned SQL Server 2022 container"]
async fn the_session_reset_releases_the_install_lock() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = OwnedSqlServer::start().await?;
    let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable_with(
        &server.connection_string,
        graphql_orm::db::mssql::MssqlPoolOptions::new().max_connections(1),
    )
    .await?;
    // A batch that takes the install lock for its session and then fails
    // with a server error: the pool resets the session and keeps it.
    let failed = pool
        .fetch_rows(
            "EXEC sp_getapplock @Resource = N'graphql_orm_restore_marker_install', \
             @LockMode = 'Exclusive', @LockOwner = 'Session', @LockTimeout = 0; \
             SELECT 1 / 0 AS boom",
            &[],
        )
        .await;
    assert!(failed.is_err(), "the batch fails after taking the lock");
    let rows = pool
        .fetch_rows(
            "SELECT APPLOCK_MODE('public', N'graphql_orm_restore_marker_install', 'Session') AS mode, @@SPID AS spid",
            &[],
        )
        .await?;
    assert_eq!(rows[0].try_get::<String, _>("mode")?, "NoLock");
    // Another session installs at once instead of waiting 30 s for the lock.
    let other =
        graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(&server.connection_string)
            .await?;
    let started = std::time::Instant::now();
    other.install_restore_marker().await?;
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    server.cleanup()?;
    Ok(())
}

/// #744: a hand-made marker table is accepted only with the primary key on
/// `marker_key` and the `marker_key = 1` check.
#[tokio::test]
#[ignore = "starts a test-owned SQL Server 2022 container"]
async fn a_marker_table_needs_its_key_and_check() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = OwnedSqlServer::start().await?;
    let pool =
        graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(&server.connection_string)
            .await?;
    for (shape, table) in [
        (
            "primary key on another column",
            "CREATE TABLE dbo.__graphql_orm_restore_marker (marker_key TINYINT NOT NULL CHECK (marker_key = 1), \
             restore_id NVARCHAR(200) NOT NULL PRIMARY KEY, started_at BIGINT NOT NULL)",
        ),
        (
            "composite primary key",
            "CREATE TABLE dbo.__graphql_orm_restore_marker (marker_key TINYINT NOT NULL CHECK (marker_key = 1), \
             restore_id NVARCHAR(200) NOT NULL, started_at BIGINT NOT NULL, PRIMARY KEY (marker_key, started_at))",
        ),
        (
            "no check",
            "CREATE TABLE dbo.__graphql_orm_restore_marker (marker_key TINYINT NOT NULL PRIMARY KEY, \
             restore_id NVARCHAR(200) NOT NULL, started_at BIGINT NOT NULL)",
        ),
        (
            "another check",
            "CREATE TABLE dbo.__graphql_orm_restore_marker (marker_key TINYINT NOT NULL PRIMARY KEY CHECK (marker_key > 0), \
             restore_id NVARCHAR(200) NOT NULL, started_at BIGINT NOT NULL)",
        ),
    ] {
        execute_batch(
            &server.connection_string,
            &format!("DROP TABLE IF EXISTS dbo.__graphql_orm_restore_marker; {table}"),
        )
        .await?;
        let error = pool.install_restore_marker().await.expect_err(shape);
        assert!(
            error.to_string().contains("documented shape"),
            "{shape}: {error}"
        );
    }
    // The documented DDL, written by hand, is accepted.
    execute_batch(
        &server.connection_string,
        "DROP TABLE dbo.__graphql_orm_restore_marker; \
         CREATE TABLE dbo.__graphql_orm_restore_marker (marker_key TINYINT NOT NULL PRIMARY KEY CHECK (marker_key = 1), \
         restore_id NVARCHAR(200) NOT NULL, started_at BIGINT NOT NULL)",
    )
    .await?;
    pool.install_restore_marker().await?;
    server.cleanup()?;
    Ok(())
}
