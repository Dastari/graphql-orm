#![cfg(feature = "mssql")]
use graphql_orm::db::mssql::{
    MssqlBrokerQueue, MssqlQueryNotificationConnection, MssqlQueryNotificationKind,
};

#[path = "support/owned_mssql.rs"]
mod owned_mssql;
use owned_mssql::{OwnedSqlServer, execute_batch};

#[tokio::test]
#[ignore = "requires Docker; owns a disposable loopback SQL Server container"]
async fn native_notifications_register_rearm_expire_and_reject()
-> Result<(), Box<dyn std::error::Error>> {
    let mut server = OwnedSqlServer::start().await?;
    execute_batch(&server.connection_string, "CREATE TABLE dbo.ChangeLog (RowId bigint IDENTITY PRIMARY KEY, ObjectKey int NOT NULL); CREATE QUEUE dbo.ChangeQueue; CREATE SERVICE ChangeService ON QUEUE dbo.ChangeQueue ([http://schemas.microsoft.com/SQL/Notifications/PostQueryNotification]);").await?;
    let password = format!("Ntf_{}A9!", graphql_orm::uuid::Uuid::new_v4().simple());
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
    // Registration is one-shot; re-arm after each wakeup and reconnect. The
    // handled message is acknowledged first, or it would be delivered again.
    listener.close().await?;
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
    listener.close().await?;
    server.cleanup()?;
    Ok(())
}
