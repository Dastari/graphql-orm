#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))),
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))),
))]

#[cfg(feature = "mssql")]
#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::prelude::*;
use std::time::Duration;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(backend = "sqlite", table = "snapshot_rows")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(backend = "postgres", table = "snapshot_rows")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "snapshot_rows",
        schema_policy = "external_writable"
    )
)]
struct SnapshotRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    value: String,
}

#[cfg(feature = "sqlite")]
schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [SnapshotRow] }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [SnapshotRow] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [SnapshotRow] }

async fn write(db: &Database, sql: &str) -> graphql_orm::Result<()> {
    <DefaultBackend as WriteBackend>::execute_with_binds(db.pool(), sql, &[]).await?;
    Ok(())
}

async fn setup(db: &Database) {
    write(
        db,
        "CREATE TABLE snapshot_rows (id VARCHAR(80) PRIMARY KEY, value VARCHAR(80) NOT NULL)",
    )
    .await
    .unwrap();
    write(
        db,
        "INSERT INTO snapshot_rows VALUES ('visible', 'before'), ('hidden', 'secret')",
    )
    .await
    .unwrap();
}

async fn check_snapshot(reader: &Database, writer: &Database, auth: Option<&DbAuthContext>) {
    let writer = writer.clone();
    let writer_after = writer.clone();
    let reader = reader.clone().with_change_journal();
    reader
        .read_snapshot_transaction_with_auth(auth, |tx| {
            Box::pin(async move {
                let key = [SqlValue::String("visible".into())];
                assert_eq!(
                    tx.find_by_key::<SnapshotRow>(&key).await?.unwrap().value,
                    "before"
                );
                #[cfg(feature = "postgres")]
                assert!(
                    tx.find_by_key::<SnapshotRow>(&[SqlValue::String("hidden".into())])
                        .await?
                        .is_none()
                );

                // A separate task owns this write. Require its commit before
                // continuing the callback, so a blocked writer fails the test.
                let committed = tokio::spawn(async move {
                    write(
                        &writer,
                        "UPDATE snapshot_rows SET value = 'after' WHERE id = 'visible'",
                    )
                    .await
                });
                tokio::time::timeout(Duration::from_secs(5), committed)
                    .await
                    .expect("snapshot reader blocked a concurrent commit")
                    .expect("writer task failed")?;

                assert_eq!(
                    tx.find_by_key::<SnapshotRow>(&key).await?.unwrap().value,
                    "before"
                );
                Ok(())
            })
        })
        .await
        .unwrap();
    reader
        .read_snapshot_transaction_with_auth(auth, |tx| {
            Box::pin(async move {
                let rows = tx.query::<SnapshotRow>().fetch_all().await?;
                assert!(
                    rows.iter()
                        .any(|row| row.id == "visible" && row.value == "after")
                );
                Ok(())
            })
        })
        .await
        .unwrap();

    let db = writer_after.clone();
    let result = reader
        .read_snapshot_transaction(|_| {
            Box::pin(async move {
                let error = SnapshotRow::insert(
                    &db,
                    CreateSnapshotRowInput {
                        id: "forbidden".into(),
                        value: "write".into(),
                    },
                )
                .await
                .unwrap_err();
                assert_eq!(OrmPublicError::from(error).code, OrmErrorCode::Forbidden);
                Ok(())
            })
        })
        .await;
    assert!(matches!(result, Err(ReadSnapshotError::WriteRefused)));

    let db = writer_after.clone();
    let result = reader
        .read_snapshot_transaction(|_| {
            Box::pin(async move {
                write(&db, "UPDATE snapshot_rows SET value = 'forbidden'").await?;
                Ok(())
            })
        })
        .await;
    assert!(matches!(result, Err(ReadSnapshotError::WriteRefused)));

    let schema = schema_builder(writer_after.clone())
        .data(AuthSubject::new("snapshot-user"))
        .finish();
    let result = reader.read_snapshot_transaction(|_| {
        Box::pin(async move {
            let response = schema.execute(
                r#"mutation { createSnapshotRow(input: {id: "graphql", value: "write"}) { success } }"#,
            ).await;
            assert_eq!(response.errors.len(), 1);
            assert_eq!(
                response.errors[0].extensions.as_ref().and_then(|e| e.get("code")),
                Some(&async_graphql::Value::from("FORBIDDEN")),
            );
            Ok(())
        })
    }).await;
    assert!(matches!(result, Err(ReadSnapshotError::WriteRefused)));

    let db = writer_after.clone();
    reader
        .read_snapshot_transaction(|_| {
            Box::pin(async move {
                assert!(matches!(
                    db.read_snapshot_transaction(|_| Box::pin(async { Ok(()) }))
                        .await,
                    Err(ReadSnapshotError::NestedTransaction),
                ));
                Ok(())
            })
        })
        .await
        .unwrap();

    let db = writer_after.clone();
    let result = reader
        .read_snapshot_transaction(|_| {
            Box::pin(async move {
                let nested = db
                    .transaction::<(), _>(TransactionMode::Default, |_| {
                        Box::pin(async { panic!("nested callback entered") })
                    })
                    .await
                    .unwrap_err();
                assert_eq!(nested.public_error().code, OrmErrorCode::Forbidden);
                Ok(())
            })
        })
        .await;
    assert!(matches!(result, Err(ReadSnapshotError::WriteRefused)));

    let db = writer_after.clone();
    let result = reader
        .read_snapshot_transaction(|_| {
            Box::pin(async move {
                let nested = db
                    .retention_transaction_with_auth::<(), _>(None, |_| {
                        Box::pin(async { panic!("nested retention callback entered") })
                    })
                    .await
                    .unwrap_err();
                assert_eq!(nested.public_error().code, OrmErrorCode::Forbidden);
                Ok(())
            })
        })
        .await;
    assert!(matches!(result, Err(ReadSnapshotError::WriteRefused)));

    assert_eq!(SnapshotRow::count_all(&writer_after).await.unwrap(), 2);
    assert_eq!(
        SnapshotRow::query(writer_after.pool())
            .fetch_all()
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.id == "visible")
            .unwrap()
            .value,
        "after",
    );
}

#[cfg(feature = "sqlite")]
struct SnapshotDirectory(std::path::PathBuf);

#[cfg(feature = "sqlite")]
impl SnapshotDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "graphql-orm-snapshot-{}",
            graphql_orm::uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

#[cfg(feature = "sqlite")]
impl Drop for SnapshotDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_checks_file_backed_attached_databases() -> graphql_orm::Result<()> {
    use graphql_orm::sqlx::{self, sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions}};

    let directory = SnapshotDirectory::new();
    let pool = SqlitePoolOptions::new().max_connections(1).connect_with(
        SqliteConnectOptions::new().filename(directory.0.join("main.sqlite"))
            .create_if_missing(true).journal_mode(SqliteJournalMode::Wal),
    ).await?;
    let db = Database::new(pool);
    sqlx::query(r#"ATTACH DATABASE ? AS "archive.""copy""#)
        .bind(directory.0.join("archive.sqlite").to_string_lossy().as_ref())
        .execute(db.pool()).await?;
    sqlx::query(r#"PRAGMA "archive.""copy".journal_mode = delete"#)
        .execute(db.pool()).await?;

    assert!(matches!(
        db.read_snapshot_transaction(|_| Box::pin(async { Ok(()) })).await,
        Err(ReadSnapshotError::SqliteWalRequired),
    ));

    sqlx::query(r#"PRAGMA "archive.""copy".journal_mode = wal"#)
        .execute(db.pool()).await?;
    sqlx::query(r#"PRAGMA "archive.""copy".locking_mode = exclusive"#)
        .execute(db.pool()).await?;
    assert!(matches!(
        db.read_snapshot_transaction(|_| Box::pin(async { Ok(()) })).await,
        Err(ReadSnapshotError::SqliteNormalLockingRequired),
    ));

    sqlx::query(r#"PRAGMA "archive.""copy".locking_mode = normal"#)
        .execute(db.pool()).await?;
    sqlx::query("ATTACH DATABASE ':memory:' AS scratch")
        .execute(db.pool()).await?;
    sqlx::query("CREATE TEMP TABLE scratch_rows (id INTEGER)")
        .execute(db.pool()).await?;
    assert_eq!(
        db.read_snapshot_transaction(|_| Box::pin(async { Ok("entered") })).await.unwrap(),
        "entered",
    );
    db.pool().close().await;
    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_requires_wal_and_allows_concurrent_commits() {
    let memory = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    assert!(matches!(
        memory
            .read_snapshot_transaction(|_| Box::pin(async { panic!("callback entered") }))
            .await,
        Err::<(), _>(ReadSnapshotError::SqliteWalRequired),
    ));

    use graphql_orm::sqlx::sqlite::{
        SqliteConnectOptions, SqliteJournalMode, SqliteLockingMode, SqlitePoolOptions,
    };
    let directory = SnapshotDirectory::new();
    let options = SqliteConnectOptions::new()
        .filename(directory.0.join("snapshot.sqlite"))
        .create_if_missing(true);
    let rollback = Database::new(
        SqlitePoolOptions::new()
            .connect_with(options.clone().journal_mode(SqliteJournalMode::Delete))
            .await
            .unwrap(),
    );
    assert!(matches!(
        rollback
            .read_snapshot_transaction(|_| Box::pin(async { panic!("callback entered") }))
            .await,
        Err::<(), _>(ReadSnapshotError::SqliteWalRequired),
    ));
    rollback.pool().close().await;
    let exclusive = Database::new(
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                options
                    .clone()
                    .journal_mode(SqliteJournalMode::Wal)
                    .locking_mode(SqliteLockingMode::Exclusive),
            )
            .await
            .unwrap(),
    );
    assert!(matches!(
        exclusive
            .read_snapshot_transaction(|_| Box::pin(async { panic!("callback entered") }))
            .await,
        Err::<(), _>(ReadSnapshotError::SqliteNormalLockingRequired),
    ));
    exclusive.pool().close().await;
    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(options.journal_mode(SqliteJournalMode::Wal))
        .await
        .unwrap();
    let db = Database::new(pool);
    setup(&db).await;
    let writer = db.clone();
    db.read_snapshot_transaction(|tx| {
        Box::pin(async move {
            let commit = tokio::spawn(async move {
                write(
                    &writer,
                    "UPDATE snapshot_rows SET value = 'first-read' WHERE id = 'visible'",
                )
                .await
            });
            tokio::time::timeout(Duration::from_secs(5), commit)
                .await
                .expect("writer did not finish before the first read")
                .expect("writer task failed")?;
            let key = [SqlValue::String("visible".into())];
            assert_eq!(
                tx.find_by_key::<SnapshotRow>(&key).await?.unwrap().value,
                "first-read"
            );
            Ok(())
        })
    })
    .await
    .unwrap();
    write(
        &db,
        "UPDATE snapshot_rows SET value = 'before' WHERE id = 'visible'",
    )
    .await
    .unwrap();
    check_snapshot(&db, &db, None).await;
    db.pool().close().await;
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires Docker for an owned PostgreSQL server"]
async fn postgres_snapshot_preserves_rls_and_concurrent_commits() {
    let owned = owned_postgres::OwnedPostgres::start("read-snapshot").unwrap();
    let writer = Database::<PostgresBackend>::connect_postgres(&owned.url)
        .await
        .unwrap();
    setup(&writer).await;
    for sql in [
        "CREATE ROLE snapshot_reader",
        "GRANT SELECT ON snapshot_rows TO snapshot_reader",
        "ALTER TABLE snapshot_rows ENABLE ROW LEVEL SECURITY",
        "CREATE POLICY snapshot_visibility ON snapshot_rows FOR SELECT TO snapshot_reader USING (id = current_setting('app.user_id', true) AND current_setting('transaction_read_only') = 'on' AND current_setting('graphql_orm.change_journal', true) IS DISTINCT FROM 'on')",
    ] {
        write(&writer, sql).await.unwrap();
    }
    let reader = Database::new(
        graphql_orm::sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(|connection, _| {
                Box::pin(async move {
                    graphql_orm::sqlx::query("SET ROLE snapshot_reader")
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&owned.url)
            .await
            .unwrap(),
    );
    let auth = DbAuthContext::from_parts("visible", vec![], vec![], None);
    check_snapshot(&reader, &writer, Some(&auth)).await;
    reader
        .read_snapshot_transaction(|tx| {
            Box::pin(async move {
                assert_eq!(tx.query::<SnapshotRow>().count().await?, 0);
                Ok(())
            })
        })
        .await
        .unwrap();
}

#[cfg(feature = "mssql")]
#[tokio::test]
#[ignore = "requires Docker for an owned SQL Server"]
async fn mssql_requires_snapshot_isolation_and_allows_concurrent_commits() {
    let mut owned = owned_mssql::OwnedSqlServer::start().await.unwrap();
    let writer_pool =
        graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(&owned.connection_string)
            .await
            .unwrap();
    let writer = Database::builder(writer_pool)
        .schema_policy(SchemaPolicy::ExternalWritable)
        .build();
    let reader = Database::new(
        graphql_orm::db::mssql::MssqlPool::connect_ado_with(
            &owned.connection_string,
            graphql_orm::db::mssql::MssqlPoolOptions::default()
                .max_connections(1)
                .health_check_after_idle(None),
        )
        .await
        .unwrap(),
    );
    assert!(matches!(
        reader
            .read_snapshot_transaction(|_| Box::pin(async { panic!("callback entered") }))
            .await,
        Err::<(), _>(ReadSnapshotError::MssqlSnapshotIsolationRequired),
    ));
    owned_mssql::execute_batch(
        &owned.connection_string,
        "ALTER DATABASE CURRENT SET ALLOW_SNAPSHOT_ISOLATION ON",
    )
    .await
    .unwrap();
    setup(&writer).await;
    let connection_state = async || {
        let rows = reader
            .pool()
            .fetch_rows(
                "SELECT CAST(s.session_id AS int) AS session_id,
                        CONVERT(varchar(33), c.connect_time, 126) AS connect_time,
                        CAST(s.transaction_isolation_level AS int) AS isolation_level
                 FROM sys.dm_exec_sessions AS s
                 JOIN sys.dm_exec_connections AS c ON c.session_id = s.session_id
                 WHERE s.session_id = @@SPID AND c.parent_connection_id IS NULL",
                &[],
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "expected the physical connection for the session");
        (
            rows[0].try_get::<i32, _>("session_id").unwrap(),
            rows[0].try_get::<String, _>("connect_time").unwrap(),
            rows[0].try_get::<i32, _>("isolation_level").unwrap(),
        )
    };
    let (session_id, connect_time, isolation) = connection_state().await;
    assert_eq!(isolation, 2, "reader starts at READ COMMITTED");
    check_snapshot(&reader, &writer, None).await;
    assert_eq!(
        connection_state().await,
        (session_id, connect_time, 2),
        "the same pooled connection returned to READ COMMITTED",
    );
    owned.cleanup().unwrap();
}
