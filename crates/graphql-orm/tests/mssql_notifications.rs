#![cfg(feature = "mssql")]
use graphql_orm::db::mssql::{
    MssqlBrokerQueue, MssqlQueryNotificationConnection, MssqlQueryNotificationKind,
};
use std::process::Command;
const SQL_SERVER_IMAGE: &str = "mcr.microsoft.com/mssql/server@sha256:ba4c8329f48fb8f02e1416be6a930ebfd71268caee78aa985f3af4315e457c89";
struct OwnedSqlServer {
    name: String,
    container_id: String,
    owner_token: String,
    connection_string: String,
    cleaned: bool,
}

impl Drop for OwnedSqlServer {
    fn drop(&mut self) {
        if !self.cleaned && self.has_exact_owned_identity() {
            let _ = Command::new("docker")
                .args(["rm", "--force", "--volumes", &self.container_id])
                .output();
        }
    }
}

impl OwnedSqlServer {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let token = graphql_orm::uuid::Uuid::new_v4().simple().to_string();
        let name = format!("graphql-orm-mssql-notifications-{token}");
        let password = format!("Gom_{token}A9!");
        let output = Command::new("docker")
            .args([
                "run",
                "--detach",
                "--cpus",
                "2",
                "--memory",
                "3g",
                "--memory-swap",
                "3g",
                "--rm",
                "--name",
                &name,
                "--label",
                &format!("graphql-orm.test-owner={token}"),
                "--publish",
                "127.0.0.1::1433",
                "--env",
                "ACCEPT_EULA=Y",
                "--env",
                &format!("MSSQL_SA_PASSWORD={password}"),
                SQL_SERVER_IMAGE,
            ])
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "failed to start owned SQL Server: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let container_id = String::from_utf8(output.stdout)?.trim().to_owned();
        if container_id.is_empty() {
            return Err("docker did not return the owned SQL Server container ID".into());
        }
        let mut owned = Self {
            name,
            container_id,
            owner_token: token,
            connection_string: String::new(),
            cleaned: false,
        };
        let port = Command::new("docker")
            .args(["port", &owned.name, "1433/tcp"])
            .output()?;
        let published = String::from_utf8(port.stdout)?;
        let port = published
            .lines()
            .find_map(|line| line.strip_prefix("127.0.0.1:"))
            .ok_or("owned SQL Server was not loopback-published")?;
        let master_connection = format!(
            "server=tcp:127.0.0.1,{port};database=master;user id=sa;password={password};TrustServerCertificate=true"
        );
        for _ in 0..180 {
            if let Ok(pool) =
                graphql_orm::db::mssql::MssqlPool::connect_ado(&master_connection).await
                && pool.fetch_rows("SELECT 1 AS ready", &[]).await.is_ok()
            {
                let database_name = format!("graphql_orm_{}", owned.owner_token);
                execute_batch(
                    &master_connection,
                    &format!("CREATE DATABASE [{database_name}]"),
                )
                .await?;
                owned.connection_string = format!(
                    "server=tcp:127.0.0.1,{port};database={database_name};user id=sa;password={password};TrustServerCertificate=true"
                );
                return Ok(owned);
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        Err("owned SQL Server did not become ready".into())
    }

    fn has_exact_owned_identity(&self) -> bool {
        let identity = Command::new("docker")
            .args([
                "inspect",
                "--format",
                "{{.Id}} {{ index .Config.Labels \"graphql-orm.test-owner\" }}",
                &self.container_id,
            ])
            .output();
        identity
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| {
                String::from_utf8_lossy(&output.stdout).trim()
                    == format!("{} {}", self.container_id, self.owner_token)
            })
    }

    fn cleanup(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if !self.has_exact_owned_identity() {
            return Err("refusing to clean up a SQL Server without exact owned identity".into());
        }
        let removed = Command::new("docker")
            .args(["rm", "--force", "--volumes", &self.container_id])
            .output()?;
        if !removed.status.success() {
            return Err(format!(
                "failed to remove owned SQL Server: {}",
                String::from_utf8_lossy(&removed.stderr)
            )
            .into());
        }
        let absent = Command::new("docker")
            .args(["inspect", &self.container_id])
            .output()?;
        if absent.status.success() {
            return Err("owned SQL Server remains after cleanup".into());
        }
        self.cleaned = true;
        Ok(())
    }
}

async fn execute_batch(connection_string: &str, sql: &str) -> graphql_orm::Result<()> {
    use graphql_orm::tokio_util::compat::TokioAsyncWriteCompatExt;
    let config = graphql_orm::tiberius::Config::from_ado_string(connection_string)
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
    let tcp = tokio::net::TcpStream::connect(config.get_addr()).await?;
    tcp.set_nodelay(true)?;
    let mut client = graphql_orm::tiberius::Client::connect(config, tcp.compat_write())
        .await
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
    client
        .simple_query(sql)
        .await
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))?
        .into_results()
        .await
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker; owns a disposable loopback SQL Server container"]
async fn native_notifications_register_rearm_expire_and_reject()
-> Result<(), Box<dyn std::error::Error>> {
    let mut server = OwnedSqlServer::start().await?;
    execute_batch(&server.connection_string, "CREATE TABLE dbo.ChangeLog (RowId bigint IDENTITY PRIMARY KEY, ObjectKey int NOT NULL); CREATE QUEUE dbo.ChangeQueue; CREATE SERVICE ChangeService ON QUEUE dbo.ChangeQueue ([http://schemas.microsoft.com/SQL/Notifications/PostQueryNotification]);").await?;
    let password = format!("Ntf_{}A9!", server.owner_token);
    execute_batch(&server.connection_string, &format!("CREATE LOGIN NotificationReader WITH PASSWORD='{password}'; CREATE USER NotificationReader FOR LOGIN NotificationReader; GRANT SELECT ON dbo.ChangeLog TO NotificationReader; GRANT SUBSCRIBE QUERY NOTIFICATIONS TO NotificationReader; GRANT RECEIVE ON dbo.ChangeQueue TO NotificationReader; GRANT RECEIVE ON dbo.QueryNotificationErrorsQueue TO NotificationReader; GRANT SEND ON SERVICE::ChangeService TO NotificationReader; DENY INSERT, UPDATE, DELETE ON dbo.ChangeLog TO NotificationReader; DENY CREATE TABLE TO NotificationReader;")).await?;
    let listener_connection = server
        .connection_string
        .replace("user id=sa", "user id=NotificationReader");
    let start = listener_connection.find("password=").unwrap();
    let end = listener_connection[start..].find(';').unwrap() + start;
    let listener_connection = format!(
        "{}password={password}{}",
        &listener_connection[..start],
        &listener_connection[end..]
    );
    assert!(
        execute_batch(
            &listener_connection,
            "INSERT dbo.ChangeLog(ObjectKey) VALUES (0)"
        )
        .await
        .is_err()
    );
    assert!(
        execute_batch(&listener_connection, "CREATE TABLE dbo.Forbidden (id int)")
            .await
            .is_err()
    );
    let queue = MssqlBrokerQueue::new("dbo", "ChangeQueue")?;
    let mut listener =
        MssqlQueryNotificationConnection::connect_ado(&listener_connection, queue.clone()).await?;
    let query = "SELECT COUNT_BIG(*) AS Total FROM dbo.ChangeLog";
    let rows = listener
        .register(query, "first", "ChangeService", 60)
        .await?;
    assert_eq!(rows[0].try_get::<i64, _>("Total")?, 0);
    assert!(
        listener.receive(100).await?.is_none(),
        "registration must be valid and idle"
    );
    execute_batch(
        &server.connection_string,
        "INSERT dbo.ChangeLog(ObjectKey) VALUES (42)",
    )
    .await?;
    let message = listener.receive(5000).await?.expect("insert notification");
    assert_eq!(
        message.kind,
        MssqlQueryNotificationKind::Change,
        "{message:?}"
    );
    assert_eq!(message.notification_id.as_deref(), Some("first"));
    // Registration is one-shot; re-arm after each wakeup and reconnect.
    drop(listener);
    let mut listener =
        MssqlQueryNotificationConnection::connect_ado(&listener_connection, queue).await?;
    assert_eq!(
        listener
            .register(query, "second", "ChangeService", 60)
            .await?[0]
            .try_get::<i64, _>("Total")?,
        1
    );
    execute_batch(
        &server.connection_string,
        "INSERT dbo.ChangeLog(ObjectKey) VALUES (43)",
    )
    .await?;
    let message = listener.receive(5000).await?.expect("rearmed notification");
    assert_eq!(
        message.kind,
        MssqlQueryNotificationKind::Change,
        "{message:?}"
    );
    assert_eq!(message.notification_id.as_deref(), Some("second"));
    listener
        .register(
            "SELECT * FROM dbo.ChangeLog",
            "invalid",
            "ChangeService",
            60,
        )
        .await?;
    let message = listener
        .receive(5000)
        .await?
        .expect("invalid registration notification");
    assert_eq!(
        message.kind,
        MssqlQueryNotificationKind::Invalid,
        "{message:?}"
    );
    listener
        .register(query, "expiry", "ChangeService", 1)
        .await?;
    let mut expired = false;
    for _ in 0..20 {
        if let Some(message) = listener.receive(5000).await? {
            assert_eq!(
                message.kind,
                MssqlQueryNotificationKind::Expired,
                "{message:?}"
            );
            expired = true;
            break;
        }
    }
    assert!(expired, "SQL Server must deliver registration expiry");
    drop(listener);
    server.cleanup()?;
    Ok(())
}
