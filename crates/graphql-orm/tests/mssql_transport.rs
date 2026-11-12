#![cfg(feature = "mssql")]
//! SQL Server transport security, pooled session state and pool bounds.
//!
//! The owned tests start a disposable loopback SQL Server whose certificate is
//! signed by a private root generated for that server alone. They never accept
//! an ambient application database URL.

use graphql_orm::db::mssql::{MssqlPool, MssqlPoolOptions, MssqlRow, MssqlTransportPolicy};
use graphql_orm::graphql::orm::{TransactionMode, WriteBackend};
use graphql_orm::prelude::*;
use std::time::{Duration, Instant};

#[path = "support/owned_mssql.rs"]
mod owned_mssql;
use owned_mssql::{OwnedSqlServer, execute_batch};

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "dbo.GraphqlOrmCountedRecords",
    plural = "CountedRecords",
    schema_policy = "external_writable"
)]
struct CountedRecord {
    #[primary_key]
    id: String,
    value: i64,
}

#[derive(Clone)]
struct AllowEntityPolicy;

impl EntityPolicy<MssqlBackend> for AllowEntityPolicy {
    fn can_access_entity<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<MssqlBackend>,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _kind: EntityAccessKind,
        _surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

/// The session state that pooled connections must not carry between checkouts.
#[derive(Debug, PartialEq, Eq)]
struct Session {
    /// `sys.dm_exec_connections.connection_id`, unique per physical
    /// connection. Session ids are reused as soon as a connection closes.
    connection: String,
    encrypt_option: String,
    /// `sys.dm_exec_sessions.transaction_isolation_level`: 2 is READ COMMITTED,
    /// 4 is SERIALIZABLE.
    isolation: i64,
    /// `@@OPTIONS & 512`: non-zero when `NOCOUNT` is on.
    nocount: i64,
    /// `@@TRANCOUNT`: open transactions on the session.
    trancount: i64,
}

const SESSION_SQL: &str = "SELECT CAST(c.connection_id AS NVARCHAR(36)) AS connection, \
    CAST(c.encrypt_option AS NVARCHAR(40)) AS encrypt_option, \
    CAST(s.transaction_isolation_level AS BIGINT) AS isolation, \
    CAST(@@OPTIONS & 512 AS BIGINT) AS nocount, \
    CAST(@@TRANCOUNT AS BIGINT) AS trancount \
    FROM sys.dm_exec_connections AS c \
    JOIN sys.dm_exec_sessions AS s ON s.session_id = c.session_id \
    WHERE c.session_id = @@SPID";

fn session_from(row: &MssqlRow) -> graphql_orm::Result<Session> {
    Ok(Session {
        connection: row.try_get("connection")?,
        encrypt_option: row.try_get("encrypt_option")?,
        isolation: row.try_get("isolation")?,
        nocount: row.try_get("nocount")?,
        trancount: row.try_get("trancount")?,
    })
}

async fn session(pool: &MssqlPool) -> graphql_orm::Result<Session> {
    let rows = pool.fetch_rows(SESSION_SQL, &[]).await?;
    session_from(rows.first().expect("one owned session"))
}

/// Remove one `key=value` setting from an owned connection string.
fn without(connection_string: &str, key: &str) -> String {
    connection_string
        .split(';')
        .filter(|setting| {
            !setting
                .split_once('=')
                .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case(key))
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn writable_database(pool: MssqlPool) -> Database<MssqlBackend> {
    let mut database = Database::<MssqlBackend>::builder(pool)
        .schema_policy(SchemaPolicy::ExternalWritable)
        .build();
    database.set_entity_policy(AllowEntityPolicy);
    database
}

/// `NOCOUNT` as a direct driver session sees it, bypassing the ORM pool.
async fn raw_session_nocount(connection_string: &str) -> Result<i64, Box<dyn std::error::Error>> {
    use graphql_orm::tokio_util::compat::TokioAsyncWriteCompatExt;
    let config = graphql_orm::tiberius::Config::from_ado_string(connection_string)?;
    let tcp = tokio::net::TcpStream::connect(config.get_addr()).await?;
    let mut client = graphql_orm::tiberius::Client::connect(config, tcp.compat_write()).await?;
    let row = client
        .simple_query("SELECT CAST(@@OPTIONS & 512 AS BIGINT) AS nocount")
        .await?
        .into_row()
        .await?
        .expect("one row");
    Ok(row.get::<i64, _>("nocount").expect("nocount"))
}

#[tokio::test]
#[ignore = "requires cached SQL Server Docker image and OpenSSL; owns its loopback server"]
async fn omitted_encrypt_uses_full_session_tls_with_a_validated_certificate()
-> Result<(), Box<dyn std::error::Error>> {
    let mut server = OwnedSqlServer::start().await?;
    // The owned server does not force encryption, so the client decides.
    let omitted = without(&server.connection_string, "Encrypt");
    assert!(!omitted.to_ascii_lowercase().contains("encrypt="));

    let pool = MssqlPool::connect_ado(&omitted).await?;
    assert_eq!(session(&pool).await?.encrypt_option, "TRUE");

    // The driver alone maps an omitted key to login-only TLS; the pool raises it.
    let config = graphql_orm::tiberius::Config::from_ado_string(&omitted)?;
    let pool = MssqlPool::new(config);
    assert_eq!(session(&pool).await?.encrypt_option, "TRUE");
    let writable = MssqlPool::connect_ado_external_writable(&omitted).await?;
    assert_eq!(session(&writable).await?.encrypt_option, "TRUE");

    // Certificate validation is on by default: without the private root the
    // server certificate is rejected.
    let unvalidated = without(&omitted, "TrustServerCertificateCA");
    let error = session(&MssqlPool::connect_ado(&unvalidated).await?)
        .await
        .expect_err("an untrusted certificate must be rejected");
    assert!(
        error
            .to_string()
            .to_ascii_lowercase()
            .contains("certificate"),
        "{error}"
    );

    // Weaker settings in the connection string alone are refused before connecting.
    for weaker in [
        format!("{omitted};Encrypt=false"),
        format!("{unvalidated};Encrypt=true;TrustServerCertificate=true"),
        format!("{unvalidated};TrustServerCertificate=true"),
        format!("{omitted};Encryption=true"),
        format!("{omitted};Trust Server Certificate=true"),
    ] {
        let error = MssqlPool::connect_ado(&weaker)
            .await
            .expect_err("weaker transport must be refused");
        assert!(
            matches!(error, graphql_orm::sqlx::Error::Configuration(_)),
            "{error}"
        );
        let error = MssqlPool::connect_ado_external_writable(&weaker)
            .await
            .expect_err("weaker transport must be refused");
        assert!(
            matches!(error, graphql_orm::sqlx::Error::Configuration(_)),
            "{error}"
        );
        let queue = graphql_orm::db::mssql::MssqlBrokerQueue::new("dbo", "OwnedQueue")?;
        assert!(
            graphql_orm::db::mssql::MssqlQueryNotificationConnection::connect_ado(&weaker, queue)
                .await
                .is_err()
        );
    }
    server.cleanup()?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires cached SQL Server Docker image and OpenSSL; owns its loopback server"]
async fn pooled_sessions_are_reset_between_checkouts() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = OwnedSqlServer::start().await?;
    execute_batch(
        &server.connection_string,
        "CREATE TABLE dbo.GraphqlOrmCountedRecords (\
             id NVARCHAR(64) NOT NULL PRIMARY KEY, value BIGINT NOT NULL)",
    )
    .await?;
    // A server-wide session default of NOCOUNT ON applies to every new login.
    execute_batch(
        &server.connection_string,
        "EXEC sp_configure 'user options', 512; RECONFIGURE;",
    )
    .await?;
    assert_eq!(raw_session_nocount(&server.connection_string).await?, 512);

    let config = graphql_orm::tiberius::Config::from_ado_string(&server.connection_string)?;
    let pool = MssqlPool::with_max_connections_external_writable(config, 1);
    let database = writable_database(pool.clone());

    let first = session(&pool).await?;
    assert_eq!(first.isolation, 2, "a new session reads committed data");
    assert_eq!(first.nocount, 0, "the pool turns NOCOUNT off: {first:?}");

    // Affected-row counts survive the session default.
    let raw = <MssqlBackend as WriteBackend>::execute_with_binds(
        &pool,
        "INSERT INTO dbo.GraphqlOrmCountedRecords (id, value) VALUES (@P1, @P2)",
        &[SqlValue::String("raw".to_owned()), SqlValue::Int(1)],
    )
    .await?;
    assert_eq!(<MssqlBackend as WriteBackend>::rows_affected(&raw), 1);
    let inserted = CountedRecord::insert(&database, CreateCountedRecordInput { value: 1 }).await?;
    let updated = CountedRecord::update_by_id(
        &database,
        &inserted.id,
        UpdateCountedRecordInput { value: Some(2) },
    )
    .await?
    .expect("an existing row reports its update");
    assert_eq!(updated.value, 2);
    assert!(CountedRecord::delete_by_id(&database, &inserted.id).await?);

    // A committed SERIALIZABLE transaction does not leak onto the next checkout
    // of the same physical connection.
    database
        .transaction(TransactionMode::StateMachine, |_| {
            Box::pin(async { Ok::<_, graphql_orm::graphql::errors::OrmPublicError>(()) })
        })
        .await
        .map_err(|error| format!("{error:?}"))?;
    let after_commit = session(&pool).await?;
    assert_eq!(
        after_commit.connection, first.connection,
        "the connection is pooled"
    );
    assert_eq!(
        after_commit.isolation, 2,
        "isolation leaked: {after_commit:?}"
    );

    // Nor does a rolled-back one.
    let rejected = database
        .transaction(TransactionMode::StateMachine, |_| {
            Box::pin(async {
                Err::<(), _>(graphql_orm::graphql::errors::OrmPublicError::with_message(
                    graphql_orm::graphql::errors::OrmErrorCode::Conflict,
                    "roll back",
                ))
            })
        })
        .await;
    assert!(rejected.is_err());
    let after_rollback = session(&pool).await?;
    assert_eq!(
        after_rollback.connection, first.connection,
        "the connection is pooled"
    );
    assert_eq!(
        after_rollback.isolation, 2,
        "isolation leaked: {after_rollback:?}"
    );
    assert_eq!(after_rollback.nocount, 0);

    // A server-reported error leaves the connection in a known state; it is
    // kept rather than discarded.
    assert!(
        pool.fetch_rows("SELECT 1 / 0 AS failed", &[])
            .await
            .is_err()
    );
    let after_error = session(&pool).await?;
    assert_eq!(after_error.connection, first.connection);
    assert_eq!(after_error.trancount, 0, "{after_error:?}");

    server.cleanup()?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires cached SQL Server Docker image and OpenSSL; owns its loopback server"]
async fn weaker_transport_is_an_explicit_opt_in() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = OwnedSqlServer::start().await?;
    let validated = without(&server.connection_string, "Encrypt");
    let unvalidated = without(&validated, "TrustServerCertificateCA");

    // Login-only TLS: the session itself is unencrypted on a server that does
    // not force encryption.
    let login_only = MssqlPoolOptions::new()
        .transport(MssqlTransportPolicy::verified().insecure_allow_login_only_encryption());
    let pool =
        MssqlPool::connect_ado_with(&format!("{validated};Encrypt=false"), login_only.clone())
            .await?;
    assert_eq!(session(&pool).await?.encrypt_option, "FALSE");
    // Allowing it does not weaken a connection string that omits the key.
    let pool = MssqlPool::connect_ado_with(&validated, login_only.clone()).await?;
    assert_eq!(session(&pool).await?.encrypt_option, "TRUE");
    // A caller-built configuration keeps the level its caller chose.
    let mut config = graphql_orm::tiberius::Config::from_ado_string(&validated)?;
    config.encryption(graphql_orm::tiberius::EncryptionLevel::Off);
    let pool = MssqlPool::with_options(config, login_only.clone());
    assert_eq!(session(&pool).await?.encrypt_option, "FALSE");
    // It does not admit an unvalidated certificate.
    assert!(
        MssqlPool::connect_ado_with(
            &format!("{unvalidated};TrustServerCertificate=true"),
            login_only
        )
        .await
        .is_err()
    );

    // Unvalidated certificate: still full-session TLS.
    let unverified = MssqlPoolOptions::new()
        .transport(MssqlTransportPolicy::verified().insecure_allow_unverified_certificate());
    let pool = MssqlPool::connect_ado_external_writable_with(
        &format!("{unvalidated};TrustServerCertificate=true"),
        unverified.clone(),
    )
    .await?;
    assert_eq!(session(&pool).await?.encrypt_option, "TRUE");
    // It does not admit login-only TLS.
    assert!(
        MssqlPool::connect_ado_with(
            &format!("{unvalidated};TrustServerCertificate=true;Encrypt=false"),
            unverified,
        )
        .await
        .is_err()
    );
    server.cleanup()?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires cached SQL Server Docker image and OpenSSL; owns its loopback server"]
async fn pool_bounds_acquire_and_statements_and_resets_idle_sessions()
-> Result<(), Box<dyn std::error::Error>> {
    let mut server = OwnedSqlServer::start().await?;
    let options = MssqlPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_millis(500))
        .query_timeout(Some(Duration::from_secs(2)));
    let pool = MssqlPool::connect_ado_with(&server.connection_string, options).await?;
    let first = session(&pool).await?;

    // Acquire timeout: the only connection is busy.
    let busy = pool.clone();
    let holder = tokio::spawn(async move {
        busy.fetch_rows("WAITFOR DELAY '00:00:01.500'; SELECT 1 AS done", &[])
            .await
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = Instant::now();
    let error = pool
        .fetch_rows("SELECT 1 AS waited", &[])
        .await
        .err()
        .expect("the busy pool must time out");
    assert!(
        matches!(error, graphql_orm::sqlx::Error::PoolTimedOut),
        "{error}"
    );
    assert!(
        started.elapsed() < Duration::from_millis(1200),
        "{:?}",
        started.elapsed()
    );
    holder.await??;
    assert_eq!(session(&pool).await?.connection, first.connection);

    // Statement timeout: the driver cannot cancel, so the connection closes.
    let started = Instant::now();
    let error = pool
        .fetch_rows("WAITFOR DELAY '00:00:20'; SELECT 1 AS done", &[])
        .await
        .err()
        .expect("the statement must time out");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert!(
        matches!(&error, graphql_orm::sqlx::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut),
        "{error}"
    );
    let replaced = session(&pool).await?;
    assert_ne!(
        replaced.connection, first.connection,
        "a timed-out connection is closed"
    );

    // A connection idle past the health-check threshold is reset before reuse,
    // even if raw SQL changed its session options.
    let pool = MssqlPool::connect_ado_with(
        &server.connection_string,
        MssqlPoolOptions::new()
            .max_connections(1)
            .health_check_after_idle(Some(Duration::ZERO)),
    )
    .await?;
    pool.fetch_rows(
        "SET NOCOUNT ON; SET TRANSACTION ISOLATION LEVEL SERIALIZABLE; SELECT 1 AS changed",
        &[],
    )
    .await?;
    let reset = session(&pool).await?;
    assert_eq!((reset.isolation, reset.nocount), (2, 0), "{reset:?}");

    // Expired connections are replaced rather than reused.
    let pool = MssqlPool::connect_ado_with(
        &server.connection_string,
        MssqlPoolOptions::new()
            .max_connections(1)
            .max_lifetime(Some(Duration::ZERO)),
    )
    .await?;
    let before = session(&pool).await?;
    assert_ne!(session(&pool).await?.connection, before.connection);
    server.cleanup()?;
    Ok(())
}

/// A listener that accepts connections and never answers, standing in for an
/// unresponsive server. No database is involved.
async fn silent_listener() -> std::io::Result<(std::net::SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let task = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    Ok((address, task))
}

fn silent_config(address: std::net::SocketAddr) -> graphql_orm::tiberius::Config {
    let mut config = graphql_orm::tiberius::Config::new();
    config.host(address.ip());
    config.port(address.port());
    config.authentication(graphql_orm::tiberius::AuthMethod::sql_server(
        "owned", "owned",
    ));
    config
}

#[tokio::test]
async fn connect_and_acquire_timeouts_bound_an_unresponsive_server()
-> Result<(), Box<dyn std::error::Error>> {
    let (address, listener) = silent_listener().await?;

    let pool = MssqlPool::with_options(
        silent_config(address),
        MssqlPoolOptions::new()
            .connect_timeout(Duration::from_millis(300))
            .acquire_timeout(Duration::from_secs(10)),
    );
    let started = Instant::now();
    let error = pool
        .fetch_rows("SELECT 1", &[])
        .await
        .err()
        .expect("connecting must time out");
    assert!(
        matches!(&error, graphql_orm::sqlx::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut),
        "{error}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );

    let pool = MssqlPool::with_options(
        silent_config(address),
        MssqlPoolOptions::new()
            .connect_timeout(Duration::from_secs(10))
            .acquire_timeout(Duration::from_millis(300)),
    );
    let started = Instant::now();
    let error = pool
        .fetch_rows("SELECT 1", &[])
        .await
        .err()
        .expect("acquiring must time out");
    assert!(
        matches!(error, graphql_orm::sqlx::Error::PoolTimedOut),
        "{error}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );

    // The notification connection bounds TLS and login the same way.
    let queue = graphql_orm::db::mssql::MssqlBrokerQueue::new("dbo", "OwnedQueue")?;
    let started = Instant::now();
    let error =
        graphql_orm::db::mssql::MssqlQueryNotificationConnection::connect_ado_with_transport(
            &format!(
                "server=tcp:127.0.0.1,{};user id=owned;password=owned",
                address.port()
            ),
            queue,
            MssqlTransportPolicy::verified(),
            Duration::from_millis(300),
        )
        .await
        .err()
        .expect("connecting must time out");
    assert!(
        matches!(&error, graphql_orm::sqlx::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut),
        "{error}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );

    let defaults = MssqlPoolOptions::default();
    assert_eq!(defaults.get_acquire_timeout(), Duration::from_secs(30));
    assert_eq!(defaults.get_connect_timeout(), Duration::from_secs(15));
    assert_eq!(defaults.get_transport(), MssqlTransportPolicy::verified());
    assert_eq!(
        MssqlPoolOptions::new()
            .max_connections(0)
            .get_max_connections(),
        1
    );
    listener.abort();
    Ok(())
}

/// Rows with `id` that a separate session can see, so only committed ones.
async fn committed_rows(
    connection_string: &str,
    id: &str,
) -> Result<i64, Box<dyn std::error::Error>> {
    let observer = MssqlPool::connect_ado(connection_string).await?;
    let rows = observer
        .fetch_rows(
            "SELECT CAST(COUNT_BIG(*) AS BIGINT) AS total FROM dbo.GraphqlOrmCountedRecords WHERE id = @P1",
            &[SqlValue::String(id.to_owned())],
        )
        .await?;
    Ok(rows[0].try_get("total")?)
}

async fn insert_in_transaction(
    database: &Database<MssqlBackend>,
    id: &'static str,
    nested_begin: bool,
) -> Result<(), String> {
    database
        .transaction(TransactionMode::Default, move |tx| {
            Box::pin(async move {
                let internal = |_| {
                    graphql_orm::graphql::errors::OrmPublicError::new(
                        graphql_orm::graphql::errors::OrmErrorCode::InternalError,
                    )
                };
                if nested_begin {
                    tx.execute("BEGIN TRANSACTION", &[])
                        .await
                        .map_err(internal)?;
                }
                tx.execute(
                    "INSERT INTO dbo.GraphqlOrmCountedRecords (id, value) VALUES (@P1, 1)",
                    &[SqlValue::String(id.to_owned())],
                )
                .await
                .map_err(internal)?;
                Ok(())
            })
        })
        .await
        .map_err(|error| format!("{error:?}"))
}

/// A transaction left open on a pooled connection must never let a later
/// borrower's commit be reported as successful and then rolled back.
#[tokio::test]
#[ignore = "requires cached SQL Server Docker image and OpenSSL; owns its loopback server"]
async fn an_open_transaction_never_silently_rolls_back_a_later_commit()
-> Result<(), Box<dyn std::error::Error>> {
    let mut server = OwnedSqlServer::start().await?;
    execute_batch(
        &server.connection_string,
        "CREATE TABLE dbo.GraphqlOrmCountedRecords (\
             id NVARCHAR(64) NOT NULL PRIMARY KEY, value BIGINT NOT NULL)",
    )
    .await?;
    let config = graphql_orm::tiberius::Config::from_ado_string(&server.connection_string)?;
    let pool = MssqlPool::with_max_connections_external_writable(config, 1);
    let database = writable_database(pool.clone());
    let first = session(&pool).await?;

    // A raw batch opens a transaction and then fails with a non-fatal error
    // (8134, severity 16). With XACT_ABORT OFF the transaction stays open.
    assert!(
        pool.fetch_rows(
            "BEGIN TRANSACTION; \
             INSERT INTO dbo.GraphqlOrmCountedRecords (id, value) VALUES ('leaked', 1); \
             SELECT 1 / 0 AS failed",
            &[],
        )
        .await
        .is_err()
    );
    let after_error = session(&pool).await?;
    assert_eq!(
        after_error.connection, first.connection,
        "the connection is kept"
    );
    assert_eq!(
        after_error.trancount, 0,
        "it is reset before reuse: {after_error:?}"
    );
    assert_eq!(
        committed_rows(&server.connection_string, "leaked").await?,
        0
    );

    // The next borrower's commit is durable.
    insert_in_transaction(&database, "after-error", false).await?;
    assert_eq!(
        committed_rows(&server.connection_string, "after-error").await?,
        1
    );
    assert_eq!(session(&pool).await?.trancount, 0);

    // Pooled statements run through sp_executesql, so SQL Server itself refuses
    // a batch that leaves its transaction open (error 266, severity 16). The
    // refusal takes the same reset path, so nothing leaks onto the next borrower.
    let error = pool
        .fetch_rows("BEGIN TRANSACTION; SELECT 1 AS opened", &[])
        .await
        .err()
        .expect("a batch that leaves a transaction open is refused");
    assert!(error.to_string().contains("266"), "{error}");
    let after_leak = session(&pool).await?;
    assert_eq!(
        after_leak.connection, first.connection,
        "the connection is kept"
    );
    assert_eq!(after_leak.trancount, 0, "{after_leak:?}");
    insert_in_transaction(&database, "after-leak", false).await?;
    assert_eq!(
        committed_rows(&server.connection_string, "after-leak").await?,
        1
    );

    // A transaction nested inside an ORM transaction is refused, never
    // reported as committed, and nothing from it is committed.
    let nested = insert_in_transaction(&database, "nested", true).await;
    assert!(
        nested.is_err(),
        "a nested transaction must not report a commit"
    );
    assert_eq!(
        committed_rows(&server.connection_string, "nested").await?,
        0
    );
    let after_nested = session(&pool).await?;
    assert_eq!(after_nested.trancount, 0, "{after_nested:?}");
    insert_in_transaction(&database, "after-nested", false).await?;
    assert_eq!(
        committed_rows(&server.connection_string, "after-nested").await?,
        1
    );

    server.cleanup()?;
    Ok(())
}
