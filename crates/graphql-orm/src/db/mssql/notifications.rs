//! Explicit, externally provisioned Service Broker notification capability.
//!
//! This connection is separate from entity pools. It never provisions schema or
//! enables business DML. SQL permissions remain the authoritative boundary.
use super::{MssqlClient, MssqlRow, MssqlTransportPolicy, map_tiberius_error};
use quick_xml::{Reader, events::Event};
use std::time::Duration;
use tiberius::QueryNotification;

/// An externally provisioned, dedicated Service Broker queue.
#[derive(Clone, Debug)]
pub struct MssqlBrokerQueue(String);

impl MssqlBrokerQueue {
    /// Validate and quote both identifier parts; arbitrary SQL is not accepted.
    pub fn new(schema: &str, queue: &str) -> crate::Result<Self> {
        for value in [schema, queue] {
            if value.is_empty()
                || value.encode_utf16().count() > 128
                || value.chars().any(char::is_control)
            {
                return Err(protocol("invalid Broker queue identifier"));
            }
        }
        Ok(Self(format!(
            "[{}].[{}]",
            schema.replace(']', "]]"),
            queue.replace(']', "]]")
        )))
    }
}

/// Notification reason. These are invalidation hints, never row change payloads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MssqlQueryNotificationKind {
    /// The registered query result changed.
    Change,
    /// A valid registration expired and must be renewed.
    Expired,
    /// SQL Server rejected or invalidated the registration.
    Invalid,
    /// Another Broker message requires conservative application resynchronization.
    Other,
}

/// A bounded, parsed notification. Text fields are untrusted SQL Server metadata.
#[derive(Clone, Debug)]
pub struct MssqlQueryNotificationMessage {
    /// Classification of the notification.
    pub kind: MssqlQueryNotificationKind,
    /// Caller-provided registration correlation identifier.
    pub notification_id: Option<String>,
    /// SQL Server's source attribute, bounded to 128 characters.
    pub source: Option<String>,
    /// SQL Server's info attribute, bounded to 128 characters.
    pub info: Option<String>,
}

/// Dedicated connection explicitly authorized to register and consume notifications.
///
/// It exposes no entity write methods and cannot be converted into an entity pool.
/// Registration executes trusted application SQL; prefix validation is not a SQL
/// sandbox. The database principal must be denied business DML and DDL.
/// Connecting does not create Broker objects or enable Broker. RECEIVE and END
/// CONVERSATION mutate Broker state and require external owner authorization.
///
/// Delivery is at-least-once; see [`receive`](Self::receive). After an error
/// or a cancelled call this value opens a new session on its next use.
pub struct MssqlQueryNotificationConnection {
    /// `None` after a session was disposed of: the next call connects again.
    /// Dropping a client closes its socket, and SQL Server rolls back the
    /// transaction that session left open.
    client: Option<MssqlClient>,
    queue: MssqlBrokerQueue,
    database: String,
    config: tiberius::Config,
    connect_timeout: Duration,
    /// The last message returned by `receive` is still inside its open
    /// transaction on `client`.
    unacknowledged: bool,
    uncertain_acknowledgements: u64,
    #[cfg(test)]
    acknowledgement_fault: Option<AcknowledgementFault>,
}

/// Points at which a test suspends the deferred acknowledgement so that the
/// caller's future can be dropped there.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AcknowledgementFault {
    /// Suspend before the commit is sent.
    BeforeCommit,
    /// Send the commit, let the server finish it, then suspend as if the
    /// response had not arrived yet.
    ResponsePending,
    /// Fail without sending the commit.
    Error,
}

/// Disposes of a session unless the statement it guards ran to completion.
///
/// A future dropped in the middle of a statement leaves the session with an
/// open transaction or a half-read response. Taking the client out closes
/// its socket at once, so SQL Server rolls the transaction back without
/// waiting for this value to connect again.
struct DisposeUnlessCompleted<'a> {
    client: &'a mut Option<MssqlClient>,
    completed: bool,
}

impl DisposeUnlessCompleted<'_> {
    fn client(&mut self) -> crate::Result<&mut MssqlClient> {
        self.client
            .as_mut()
            .ok_or_else(|| protocol("SQL Server notification session is not connected"))
    }
}

impl Drop for DisposeUnlessCompleted<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.client.take();
        }
    }
}

impl MssqlQueryNotificationConnection {
    /// Connect using a principal with SELECT, SUBSCRIBE QUERY NOTIFICATIONS,
    /// RECEIVE on this queue and dbo.QueryNotificationErrorsQueue, and
    /// permission to end its conversations.
    ///
    /// The connection string must meet [`MssqlTransportPolicy::verified`].
    /// Connecting, including TLS, login and session setup, is bounded by
    /// [`DEFAULT_CONNECT_TIMEOUT`](Self::DEFAULT_CONNECT_TIMEOUT).
    pub async fn connect_ado(
        connection_string: &str,
        queue: MssqlBrokerQueue,
    ) -> crate::Result<Self> {
        Self::connect_ado_with_transport(
            connection_string,
            queue,
            MssqlTransportPolicy::verified(),
            Self::DEFAULT_CONNECT_TIMEOUT,
        )
        .await
    }

    /// Bound on opening a notification connection, matching the pool default.
    pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

    /// Connect under an explicit transport policy. `connect_timeout` bounds
    /// TCP, TLS, login and session setup; exceeding it returns an
    /// `io::ErrorKind::TimedOut` error.
    pub async fn connect_ado_with_transport(
        connection_string: &str,
        queue: MssqlBrokerQueue,
        transport: MssqlTransportPolicy,
        connect_timeout: Duration,
    ) -> crate::Result<Self> {
        let config = super::transport::config_from_ado(connection_string, transport)?;
        let (client, database) = establish(config.clone(), connect_timeout).await?;
        Ok(Self {
            client: Some(client),
            queue,
            database,
            config,
            connect_timeout,
            unacknowledged: false,
            uncertain_acknowledgements: 0,
            #[cfg(test)]
            acknowledgement_fault: None,
        })
    }

    /// How many acknowledgements ended without a known outcome: the commit
    /// failed, or a call was dropped while it ran. Each such message had
    /// already been returned to the caller and may be delivered once more.
    pub fn uncertain_acknowledgements(&self) -> u64 {
        self.uncertain_acknowledgements
    }

    /// Whether the last message returned by [`receive`](Self::receive) is
    /// still unacknowledged: its transaction is open and its conversation
    /// group is locked for other receivers.
    pub fn has_unacknowledged_message(&self) -> bool {
        self.unacknowledged
    }

    /// Commit the receive of the last message returned by
    /// [`receive`](Self::receive). A no-op when nothing is unacknowledged.
    ///
    /// Call it after handling a message when the next `receive` will not
    /// follow promptly, and before dropping this value on a clean shutdown;
    /// an unacknowledged message is rolled back and delivered again.
    ///
    /// # Errors
    ///
    /// If the commit fails, its outcome is unknown: the session is disposed
    /// of, the case is counted in
    /// [`uncertain_acknowledgements`](Self::uncertain_acknowledgements) and
    /// the message is delivered again unless the commit had landed.
    ///
    /// # Cancellation
    ///
    /// Dropping the future disposes of the session. The message was already
    /// returned by `receive`, so it cannot be lost; it is delivered again
    /// unless the commit had landed.
    pub async fn acknowledge(&mut self) -> crate::Result<()> {
        if !self.unacknowledged {
            return Ok(());
        }
        // Whatever happens next, the message is no longer pending here: the
        // commit lands, or the session is disposed of and it is redelivered.
        self.unacknowledged = false;
        let result = self.commit().await;
        if let Err(error) = &result {
            self.uncertain_acknowledgements = self.uncertain_acknowledgements.saturating_add(1);
            // The message is never part of the log line.
            tracing::debug!(
                %error,
                total = self.uncertain_acknowledgements,
                "SQL Server notification acknowledgement did not complete; \
                 the message was delivered and may be delivered again"
            );
        }
        result
    }

    /// Acknowledge the last message, if any, and close the connection.
    pub async fn close(mut self) -> crate::Result<()> {
        self.acknowledge().await
    }

    /// Acknowledge the previous message before the next statement. Its
    /// failure is recorded, not returned: that message was delivered, and
    /// the statement that follows runs on a new session.
    async fn settle(&mut self) {
        let _ = self.acknowledge().await;
    }

    /// Open a session if the previous one was disposed of.
    async fn ensure_connected(&mut self) -> crate::Result<()> {
        if self.client.is_none() {
            let (client, database) = establish(self.config.clone(), self.connect_timeout).await?;
            self.client = Some(client);
            self.database = database;
        }
        Ok(())
    }

    /// Register one SELECT and fully drain its result. Successful execution alone
    /// does not prove eligibility: SQL Server reports invalid registrations through
    /// the queue. Applications must receive and handle `Invalid` immediately.
    ///
    /// The query must be one trusted application SELECT, never user input. The
    /// lexical guard requires a SELECT prefix and rejects statement separators;
    /// it is not a complete SQL parser and cannot prove absence of side effects.
    /// Database permissions must deny business writes and DDL.
    pub async fn register(
        &mut self,
        select_sql: &str,
        notification_id: &str,
        service: &str,
        timeout_seconds: u32,
    ) -> crate::Result<Vec<MssqlRow>> {
        if notification_id.is_empty()
            || notification_id.len() > 4096
            || notification_id.chars().any(char::is_control)
        {
            return Err(protocol("invalid notification correlation identifier"));
        }
        let sql = select_sql.trim().trim_end_matches(';').trim();
        if !sql
            .get(..6)
            .is_some_and(|s| s.eq_ignore_ascii_case("SELECT"))
            || !sql.as_bytes().get(6).is_some_and(u8::is_ascii_whitespace)
            || sql.contains(';')
        {
            return Err(protocol("notification registration requires one SELECT"));
        }
        for value in [service, self.database.as_str()] {
            if value.is_empty()
                || value.contains([';', '\'', '"'])
                || value.chars().any(char::is_control)
            {
                return Err(protocol("invalid notification destination"));
            }
        }
        if timeout_seconds == 0 || timeout_seconds > 86400 {
            return Err(protocol("notification timeout must be 1..86400 seconds"));
        }
        // A registration must not run inside the open receive transaction.
        self.settle().await;
        self.ensure_connected().await?;
        let options = format!("service={service};local database={}", self.database);
        let notification = QueryNotification::new(notification_id, &options, timeout_seconds)
            .map_err(map_tiberius_error)?;
        let mut session = DisposeUnlessCompleted {
            client: &mut self.client,
            completed: false,
        };
        let rows = session
            .client()?
            .simple_query_with_notification(sql, notification)
            .await
            .map_err(map_tiberius_error)?
            .into_first_result()
            .await
            .map_err(map_tiberius_error)?;
        session.completed = true;
        Ok(rows.into_iter().map(|inner| MssqlRow { inner }).collect())
    }

    /// Wait at most 60 seconds for one notification; `None` means timeout.
    /// The received conversation is ended. Malformed, oversized, and unknown
    /// messages return `Other`, requiring conservative resynchronization.
    ///
    /// # Delivery: at-least-once, hosts must be idempotent
    ///
    /// The message is returned while the transaction that received it is
    /// still open. Nothing is committed until the caller, who now holds the
    /// message, calls `receive` again, [`acknowledge`](Self::acknowledge) or
    /// [`close`](Self::close). So a message is never committed before it has
    /// been delivered, and may be delivered more than once:
    ///
    /// - Until it is acknowledged, the message's transaction stays open and
    ///   its conversation group is locked for other receivers. A host that
    ///   handles messages slowly should call `acknowledge` after handling
    ///   one, or accept the lock.
    /// - Dropping this value, losing the connection or a crash before the
    ///   acknowledgement rolls the receive back, and the message is delivered
    ///   again.
    /// - If the deferred commit fails, the message is delivered again unless
    ///   the commit had landed; the case is counted in
    ///   [`uncertain_acknowledgements`](Self::uncertain_acknowledgements) and
    ///   this call continues on a new session.
    /// - If the receive or `END CONVERSATION` fails, the transaction is
    ///   rolled back, the error is returned and the message stays queued.
    ///
    /// Messages are invalidation hints, so handling one twice only repeats a
    /// refresh.
    ///
    /// # Cancellation
    ///
    /// Dropping the future is safe at every point. While the previous
    /// message is being acknowledged, that message was already delivered and
    /// can only be delivered again. While waiting for or reading the next
    /// message, the connection is closed at once, without waiting for a new
    /// one to be established; SQL Server ends that session and rolls its
    /// receive back, and the message stays queued. No message is ever held
    /// only inside this value.
    ///
    /// # Poison-message handling
    ///
    /// SQL Server disables a queue after five consecutive rollbacks of a
    /// receive. Every unacknowledged drop, cancelled receive of a waiting
    /// message, crash or failed acknowledgement is one rollback, so a loop
    /// that keeps receiving the same message without acknowledging it
    /// (receive, drop, reconnect) disables a healthy queue. Acknowledge
    /// handled messages. To recover, re-enable the queue with
    /// `ALTER QUEUE ... WITH STATUS = ON`; a host that prefers redelivery
    /// without that stop can create the queue with
    /// `POISON_MESSAGE_HANDLING (STATUS = OFF)`.
    pub async fn receive(
        &mut self,
        wait_ms: u32,
    ) -> crate::Result<Option<MssqlQueryNotificationMessage>> {
        if wait_ms > 60_000 {
            return Err(protocol("Broker wait exceeds 60000 milliseconds"));
        }
        self.settle().await;
        self.ensure_connected().await?;
        let batch = receive_batch(&self.queue, wait_ms);
        let mut session = DisposeUnlessCompleted {
            client: &mut self.client,
            completed: false,
        };
        let row = session
            .client()?
            .simple_query(batch)
            .await
            .map_err(map_tiberius_error)?
            .into_row()
            .await
            .map_err(map_tiberius_error)?;
        let Some(row) = row else {
            // Nothing was received and the batch committed its own
            // transaction: the session is clean.
            session.completed = true;
            return Ok(None);
        };
        let message_type = row
            .try_get::<&str, _>("message_type_name")
            .map_err(map_tiberius_error)?;
        let body = row.try_get::<&str, _>("body").map_err(map_tiberius_error)?;
        let message = if message_type
            == Some("http://schemas.microsoft.com/SQL/Notifications/QueryNotification")
        {
            body.and_then(parse_notification).unwrap_or_else(other)
        } else {
            other()
        };
        // No await follows: the caller receives the message in this poll,
        // with its transaction still open.
        session.completed = true;
        self.unacknowledged = true;
        Ok(Some(message))
    }

    /// Commit the open receive transaction, disposing of the session unless
    /// the commit is known to have completed.
    async fn commit(&mut self) -> crate::Result<()> {
        #[cfg(test)]
        let fault = self.acknowledgement_fault.take();
        let mut session = DisposeUnlessCompleted {
            client: &mut self.client,
            completed: false,
        };
        #[cfg(test)]
        match fault {
            Some(AcknowledgementFault::Error) => {
                return Err(protocol("injected: the commit was never sent"));
            }
            Some(AcknowledgementFault::BeforeCommit) => std::future::pending::<()>().await,
            _ => {}
        }
        session
            .client()?
            .simple_query(ACKNOWLEDGE_BATCH)
            .await
            .map_err(map_tiberius_error)?
            .into_results()
            .await
            .map_err(map_tiberius_error)?;
        #[cfg(test)]
        if fault == Some(AcknowledgementFault::ResponsePending) {
            std::future::pending::<()>().await;
        }
        session.completed = true;
        Ok(())
    }
}

/// Open a notification session: connect, set the options query notifications
/// require and read the database name, all within `connect_timeout`.
async fn establish(
    config: tiberius::Config,
    connect_timeout: Duration,
) -> crate::Result<(MssqlClient, String)> {
    let connect = async move {
        let mut client = super::open_client(config).await?;
        let rows = client.simple_query("SET ANSI_NULLS ON; SET ANSI_PADDING ON; SET ANSI_WARNINGS ON; SET CONCAT_NULL_YIELDS_NULL ON; SET QUOTED_IDENTIFIER ON; SET NUMERIC_ROUNDABORT OFF; SET ARITHABORT ON; SET TRANSACTION ISOLATION LEVEL READ COMMITTED; SELECT DB_NAME() AS db;")
            .await.map_err(map_tiberius_error)?.into_first_result().await.map_err(map_tiberius_error)?;
        let database = rows
            .first()
            .and_then(|r| r.try_get::<&str, _>("db").ok().flatten())
            .ok_or_else(|| protocol("missing notification database"))?
            .to_owned();
        Ok::<_, sqlx::Error>((client, database))
    };
    tokio::time::timeout(connect_timeout, connect)
        .await
        .map_err(|_| {
            sqlx::Error::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "SQL Server notification connection exceeded its connect timeout",
            ))
        })?
}

/// Commits the transaction [`receive_batch`] left open around a message.
const ACKNOWLEDGE_BATCH: &str = "IF @@TRANCOUNT > 0 COMMIT TRANSACTION;";

/// One batch that receives at most one message and ends its conversation in
/// a transaction it leaves open; its only result set is the received message,
/// if any. When nothing arrives it commits its empty transaction. A
/// transaction left open on the session is rolled back first. If the receive
/// or `END CONVERSATION` fails, the transaction is rolled back and the error
/// rethrown.
fn receive_batch(queue: &MssqlBrokerQueue, wait_ms: u32) -> String {
    format!(
        "IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION; \
         DECLARE @h uniqueidentifier, @t nvarchar(256), @b nvarchar(max); \
         BEGIN TRY \
         BEGIN TRANSACTION; \
         WAITFOR (RECEIVE TOP (1) @h = conversation_handle, @t = message_type_name, \
         @b = CASE WHEN DATALENGTH(message_body) <= 16384 THEN CONVERT(nvarchar(max), message_body) ELSE NULL END \
         FROM {}), TIMEOUT {wait_ms}; \
         IF @h IS NOT NULL END CONVERSATION @h; \
         ELSE COMMIT TRANSACTION; \
         END TRY \
         BEGIN CATCH \
         IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION; \
         THROW; \
         END CATCH; \
         SELECT @h AS conversation_handle, @t AS message_type_name, @b AS body WHERE @h IS NOT NULL;",
        queue.0
    )
}

fn protocol(message: &str) -> sqlx::Error {
    sqlx::Error::Protocol(message.to_owned())
}
fn other() -> MssqlQueryNotificationMessage {
    MssqlQueryNotificationMessage {
        kind: MssqlQueryNotificationKind::Other,
        notification_id: None,
        source: None,
        info: None,
    }
}
fn parse_notification(xml: &str) -> Option<MssqlQueryNotificationMessage> {
    if xml.len() > 16384 {
        return None;
    }
    let mut reader = Reader::from_str(xml.strip_prefix('\u{feff}').unwrap_or(xml));
    let mut result = other();
    let mut notification_type = None;
    // Before root, inside root, inside Message, after Message, after root.
    let mut state = 0;
    let mut declaration = false;
    loop {
        match reader.read_event().ok()? {
            Event::Decl(_) if state == 0 && !declaration => declaration = true,
            Event::Start(element)
                if state == 0 && element.local_name().as_ref() == b"QueryNotification" =>
            {
                state = 1;
                for attr in element.attributes() {
                    let attr = attr.ok()?;
                    // Preserve the notification grammar's rejection of literal control
                    // characters. XML attribute normalization would turn tabs/newlines
                    // into spaces before the existing validation can reject them.
                    let decoded = reader.decoder().decode(&attr.value).ok()?;
                    let value = quick_xml::escape::unescape(&decoded).ok()?.into_owned();
                    if value.len() > 128 || value.chars().any(char::is_control) {
                        return None;
                    }
                    let target = match attr.key.as_ref() {
                        b"type" => &mut notification_type,
                        b"source" => &mut result.source,
                        b"info" => &mut result.info,
                        b"id" | b"database_id" | b"sid" => continue,
                        key if key == b"xmlns" || key.starts_with(b"xmlns:") => continue,
                        _ => return None,
                    };
                    if target.replace(value).is_some() {
                        return None;
                    }
                }
            }
            Event::Start(element) if state == 1 && element.local_name().as_ref() == b"Message" => {
                if element.attributes().next().is_some() {
                    return None;
                }
                state = 2;
                result.notification_id = Some(String::new());
            }
            Event::Text(text) => {
                let value = text.decode().ok()?;
                if state == 2 {
                    append_id(&mut result.notification_id, &value)?;
                } else if !value.trim().is_empty() {
                    return None;
                }
            }
            Event::CData(text) if state == 2 => {
                append_id(&mut result.notification_id, &text.decode().ok()?)?;
            }
            Event::GeneralRef(reference) if state == 2 => {
                let encoded = format!("&{};", reference.decode().ok()?);
                let value = quick_xml::escape::unescape(&encoded).ok()?;
                append_id(&mut result.notification_id, &value)?;
            }
            Event::End(element) if state == 2 && element.local_name().as_ref() == b"Message" => {
                state = 3
            }
            Event::End(element)
                if state == 3 && element.local_name().as_ref() == b"QueryNotification" =>
            {
                state = 4
            }
            Event::Eof if state == 4 => break,
            _ => return None,
        }
    }
    if result.notification_id.as_ref().is_none_or(String::is_empty) {
        return None;
    }
    result.kind = match (
        notification_type.as_deref()?,
        result.info.as_deref()?,
        result.source.as_deref()?,
    ) {
        (_, "expired", _) | (_, _, "timeout") => MssqlQueryNotificationKind::Expired,
        ("change", "insert" | "update" | "delete" | "truncate", _) => {
            MssqlQueryNotificationKind::Change
        }
        ("subscribe", _, _) | (_, "invalid" | "options" | "isolation" | "query", _) => {
            MssqlQueryNotificationKind::Invalid
        }
        _ => MssqlQueryNotificationKind::Other,
    };
    Some(result)
}

fn append_id(id: &mut Option<String>, value: &str) -> Option<()> {
    let id = id.as_mut()?;
    if id.len() + value.len() > 4096 || value.chars().any(char::is_control) {
        return None;
    }
    id.push_str(value);
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_receive_batch_leaves_its_transaction_for_the_acknowledgement() {
        let queue = MssqlBrokerQueue::new("dbo", "ChangeQueue").unwrap();
        let sql = receive_batch(&queue, 250);
        let position = |needle: &str| {
            sql.find(needle)
                .unwrap_or_else(|| panic!("{needle}: {sql}"))
        };
        let begin = position("BEGIN TRANSACTION;");
        let receive = position("WAITFOR (RECEIVE TOP (1) @h = conversation_handle");
        let end = position("IF @h IS NOT NULL END CONVERSATION @h;");
        let rollback = position("BEGIN CATCH IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION; THROW;");
        let select = position("SELECT @h AS conversation_handle");
        // A leftover transaction is rolled back before anything else.
        assert!(
            sql.starts_with("IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION;"),
            "{sql}"
        );
        assert!(begin < receive && receive < end && end < rollback, "{sql}");
        assert!(rollback < select, "{sql}");
        assert!(
            sql.contains("FROM [dbo].[ChangeQueue]), TIMEOUT 250;"),
            "{sql}"
        );
        // The batch never commits a received message: it must reach the
        // caller first, and only the acknowledgement makes the receive
        // durable. Its one commit is for the empty transaction of a timeout.
        assert_eq!(sql.matches("COMMIT").count(), 1, "{sql}");
        assert!(
            sql.contains("IF @h IS NOT NULL END CONVERSATION @h; ELSE COMMIT TRANSACTION;"),
            "{sql}"
        );
        assert_eq!(ACKNOWLEDGE_BATCH, "IF @@TRANCOUNT > 0 COMMIT TRANSACTION;");
    }

    /// Runs on a disposable SQL Server this test owns.
    #[tokio::test]
    #[ignore = "requires Docker; owns a disposable loopback SQL Server container"]
    async fn delivery_is_at_least_once_through_failures_and_cancellation()
    -> Result<(), Box<dyn std::error::Error>> {
        use super::super::owned_mssql::{OwnedSqlServer, execute_batch};

        let mut server = OwnedSqlServer::start().await?;
        let connection = server.connection_string.clone();
        execute_batch(
            &connection,
            "CREATE QUEUE dbo.RedeliveryQueue; \
             CREATE SERVICE RedeliverySource ON QUEUE dbo.RedeliveryQueue ([DEFAULT]); \
             CREATE QUEUE dbo.PoisonQueue; \
             CREATE SERVICE PoisonSource ON QUEUE dbo.PoisonQueue ([DEFAULT]); \
             CREATE QUEUE dbo.RedeliveryReplies; \
             CREATE SERVICE RedeliveryReplies ON QUEUE dbo.RedeliveryReplies ([DEFAULT]);",
        )
        .await?;
        let send_to = |service: &str| {
            format!(
                "DECLARE @h uniqueidentifier; \
                 BEGIN DIALOG @h FROM SERVICE RedeliveryReplies TO SERVICE '{service}' \
                 ON CONTRACT [DEFAULT] WITH ENCRYPTION = OFF; \
                 SEND ON CONVERSATION @h MESSAGE TYPE [DEFAULT] (N'payload');"
            )
        };
        let send = send_to("RedeliverySource");
        let queue = MssqlBrokerQueue::new("dbo", "RedeliveryQueue")?;
        let connect = || MssqlQueryNotificationConnection::connect_ado(&connection, queue.clone());
        let admin = super::super::MssqlPool::connect_ado(&connection).await?;
        let cancelled = |outcome: Result<_, tokio::time::error::Elapsed>| {
            let outcome: Result<crate::Result<Option<MssqlQueryNotificationMessage>>, _> = outcome;
            assert!(outcome.is_err(), "the call must have been cancelled");
        };

        // A message is returned with its transaction open; the next receive
        // acknowledges it first and then finds nothing more.
        execute_batch(&connection, &send).await?;
        let mut listener = connect().await?;
        assert!(listener.receive(5000).await?.is_some());
        assert!(listener.has_unacknowledged_message());
        assert!(listener.receive(200).await?.is_none());
        assert!(!listener.has_unacknowledged_message());

        // (1) The next receive is cancelled while the deferred commit's
        // response is pending, after the server committed. The message had
        // already been delivered: nothing is lost and nothing is redelivered.
        execute_batch(&connection, &send).await?;
        assert!(listener.receive(5000).await?.is_some());
        listener.acknowledgement_fault = Some(AcknowledgementFault::ResponsePending);
        cancelled(tokio::time::timeout(Duration::from_secs(3), listener.receive(5000)).await);
        assert!(listener.client.is_none(), "the session is disposed of");
        assert!(!listener.has_unacknowledged_message());
        assert!(listener.receive(200).await?.is_none(), "no redelivery");

        // (2) Cancelled before the deferred commit is sent: redelivered once.
        execute_batch(&connection, &send).await?;
        assert!(listener.receive(5000).await?.is_some());
        listener.acknowledgement_fault = Some(AcknowledgementFault::BeforeCommit);
        cancelled(tokio::time::timeout(Duration::from_secs(1), listener.receive(5000)).await);
        assert!(listener.client.is_none());
        assert!(listener.receive(5000).await?.is_some(), "redelivered");
        assert!(listener.receive(200).await?.is_none(), "and only once");

        // The deferred commit fails outright: counted, redelivered once, and
        // the receive that hit it still completes on a new session.
        execute_batch(&connection, &send).await?;
        assert!(listener.receive(5000).await?.is_some());
        listener.acknowledgement_fault = Some(AcknowledgementFault::Error);
        assert_eq!(listener.uncertain_acknowledgements(), 0);
        assert!(listener.receive(5000).await?.is_some(), "redelivered");
        assert_eq!(listener.uncertain_acknowledgements(), 1);
        // An explicit acknowledgement reports the same failure to its caller.
        listener.acknowledgement_fault = Some(AcknowledgementFault::Error);
        assert!(listener.acknowledge().await.is_err());
        assert_eq!(listener.uncertain_acknowledgements(), 2);
        assert!(listener.receive(5000).await?.is_some(), "redelivered again");
        listener.acknowledge().await?;

        // (3) The listener is dropped without acknowledging: redelivered.
        execute_batch(&connection, &send).await?;
        assert!(listener.receive(5000).await?.is_some());
        drop(listener);
        let mut listener = connect().await?;
        assert!(listener.receive(5000).await?.is_some(), "redelivered");

        // (4) An explicit acknowledgement, then a drop: not redelivered.
        listener.acknowledge().await?;
        assert!(!listener.has_unacknowledged_message());
        drop(listener);
        let mut listener = connect().await?;
        assert!(listener.receive(200).await?.is_none());
        // A clean close acknowledges too.
        execute_batch(&connection, &send).await?;
        assert!(listener.receive(5000).await?.is_some());
        listener.close().await?;
        let mut listener = connect().await?;

        // (5) Cancellation while waiting in RECEIVE closes the connection at
        // once: the server-side session, 30 seconds into nothing, is gone
        // within seconds and before this listener connects again.
        let rows = listener
            .client
            .as_mut()
            .expect("connected")
            .simple_query("SELECT CAST(@@SPID AS int) AS spid")
            .await?
            .into_first_result()
            .await?;
        let spid = rows[0].get::<i32, _>("spid").expect("spid");
        // The probe's own session exists before the listener's is freed, so
        // it cannot be given the same session id.
        let probe = admin
            .fetch_rows("SELECT CAST(@@SPID AS int) AS spid", &[])
            .await?;
        assert_ne!(probe[0].try_get::<i32, _>("spid")?, spid);
        cancelled(tokio::time::timeout(Duration::from_millis(500), listener.receive(30_000)).await);
        assert!(
            listener.client.is_none(),
            "disposed of without reconnecting"
        );
        let mut gone = false;
        for _ in 0..50 {
            let rows = admin
                .fetch_rows(
                    "SELECT COUNT_BIG(*) AS sessions FROM sys.dm_exec_sessions \
                     WHERE session_id = @P1",
                    &[crate::graphql::orm::SqlValue::Int(i64::from(spid))],
                )
                .await?;
            if rows[0].try_get::<i64, _>("sessions")? == 0 {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(gone, "the cancelled session is still open on the server");
        // A message sent afterwards is delivered once, on a new session.
        execute_batch(&connection, &send).await?;
        assert!(listener.receive(5000).await?.is_some());
        assert!(listener.receive(200).await?.is_none());

        // `END CONVERSATION` fails (an unknown handle stands in for the
        // failure): the receive is rolled back and the message stays queued.
        execute_batch(&connection, &send).await?;
        let failing = receive_batch(&queue, 5000).replace(
            "END CONVERSATION @h;",
            "END CONVERSATION 'ffffffff-ffff-ffff-ffff-ffffffffffff';",
        );
        assert_ne!(failing, receive_batch(&queue, 5000));
        let error = execute_batch(&connection, &failing)
            .await
            .expect_err("ending an unknown conversation fails");
        assert!(error.to_string().contains("conversation"), "{error}");
        assert!(
            listener.receive(5000).await?.is_some(),
            "a failed receive leaves the message on the queue"
        );
        assert!(listener.receive(200).await?.is_none());

        // Registration acknowledges the pending message first and runs
        // outside its transaction.
        execute_batch(&connection, &send).await?;
        assert!(listener.receive(5000).await?.is_some());
        execute_batch(
            &connection,
            "CREATE TABLE dbo.RedeliveryWatched (Id int NOT NULL PRIMARY KEY);",
        )
        .await?;
        execute_batch(
            &connection,
            "CREATE SERVICE RedeliveryNotify ON QUEUE dbo.RedeliveryQueue \
             ([http://schemas.microsoft.com/SQL/Notifications/PostQueryNotification]);",
        )
        .await?;
        listener
            .register(
                "SELECT Id FROM dbo.RedeliveryWatched",
                "after-receive",
                "RedeliveryNotify",
                60,
            )
            .await?;
        assert!(!listener.has_unacknowledged_message());
        drop(listener);

        // Five consecutive rolled-back receives of one message disable the
        // queue: SQL Server's poison-message handling.
        let poison = MssqlBrokerQueue::new("dbo", "PoisonQueue")?;
        execute_batch(&connection, &send_to("PoisonSource")).await?;
        let receive_enabled = || async {
            let rows = admin
                .fetch_rows(
                    "SELECT CAST(is_receive_enabled AS int) AS enabled \
                     FROM sys.service_queues WHERE name = N'PoisonQueue'",
                    &[],
                )
                .await?;
            rows[0].try_get::<i32, _>("enabled")
        };
        assert_eq!(receive_enabled().await?, 1);
        for attempt in 1..=5 {
            let mut careless =
                MssqlQueryNotificationConnection::connect_ado(&connection, poison.clone()).await?;
            assert!(
                careless.receive(5000).await?.is_some(),
                "attempt {attempt}: the unacknowledged message is redelivered"
            );
            // Dropped without acknowledging: one more rollback.
            drop(careless);
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        let mut disabled = false;
        for _ in 0..50 {
            if receive_enabled().await? == 0 {
                disabled = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(disabled, "five rolled-back receives disable the queue");
        // The remedy: re-enable the queue, then acknowledge what is received.
        execute_batch(&connection, "ALTER QUEUE dbo.PoisonQueue WITH STATUS = ON;").await?;
        let mut careful =
            MssqlQueryNotificationConnection::connect_ado(&connection, poison).await?;
        assert!(careful.receive(5000).await?.is_some());
        careful.close().await?;
        assert_eq!(receive_enabled().await?, 1);

        server.cleanup()?;
        Ok(())
    }

    #[test]
    fn parses_change_expiry_and_invalid() {
        for (xml, kind) in [
            (
                r#"<QueryNotification type="change" source="data" info="insert"><Message>id</Message></QueryNotification>"#,
                MssqlQueryNotificationKind::Change,
            ),
            (
                r#"<QueryNotification type="change" source="timeout" info="expired"><Message>id</Message></QueryNotification>"#,
                MssqlQueryNotificationKind::Expired,
            ),
            (
                r#"<QueryNotification type="subscribe" source="statement" info="invalid"><Message>id</Message></QueryNotification>"#,
                MssqlQueryNotificationKind::Invalid,
            ),
        ] {
            assert_eq!(parse_notification(xml).unwrap().kind, kind);
        }
        assert!(parse_notification("<!DOCTYPE x><QueryNotification/>").is_none());
        assert!(parse_notification(&"x".repeat(16385)).is_none());
    }
    #[test]
    fn requires_one_complete_root_and_one_bounded_correlation() {
        let sql_server = "\u{feff}<qn:QueryNotification xmlns:qn=\"http://schemas.microsoft.com/SQL/Notifications/QueryNotification\" id=\"1\" type=\"change\" source=\"data\" info=\"insert\" database_id=\"5\" sid=\"0x01\"><qn:Message>first</qn:Message></qn:QueryNotification>";
        assert_eq!(
            parse_notification(sql_server).unwrap().kind,
            MssqlQueryNotificationKind::Change
        );
        let valid = r#"<QueryNotification type="change" source="data" info="insert"><Message>A&amp;B&#x21;</Message></QueryNotification>"#;
        assert_eq!(
            parse_notification(valid)
                .unwrap()
                .notification_id
                .as_deref(),
            Some("A&B!")
        );
        for invalid in [
            valid.replace("</QueryNotification>", ""),
            format!("{valid}{valid}"),
            valid.replace("<Message>", "<Message><Unexpected>"),
            valid.replace("</Message>", "</Message><Message>second</Message>"),
            valid.replace("A&amp;B&#x21;", ""),
            valid.replace("A&amp;B&#x21;", "&unknown;"),
            valid.replace("A&amp;B&#x21;", &"a".repeat(4097)),
            valid.replace("<Message>A&amp;B&#x21;</Message>", ""),
        ] {
            assert!(
                parse_notification(&invalid).is_none(),
                "accepted malformed notification"
            );
        }
    }

    #[test]
    fn attribute_decoding_preserves_control_character_rejection() {
        let valid = r#"<QueryNotification type="change" source="d&#97;ta" info="insert" id="safe"><Message>id</Message></QueryNotification>"#;
        assert_eq!(
            parse_notification(valid).unwrap().kind,
            MssqlQueryNotificationKind::Change
        );
        // Even ignored SQL Server metadata attributes retain the same validation.
        // Normalizing literal whitespace before validation would accept these.
        for value in ["a\tb", "a\nb", "a\rb", "a&#9;b", "a&#xA;b", "a&#13;b"] {
            let invalid = valid.replace("id=\"safe\"", &format!("id=\"{value}\""));
            assert!(parse_notification(&invalid).is_none());
        }
    }

    #[test]
    fn queue_identifiers_are_quoted() {
        assert_eq!(
            MssqlBrokerQueue::new("dbo", "a]; DROP TABLE x;--")
                .unwrap()
                .0,
            "[dbo].[a]]; DROP TABLE x;--]"
        );
        assert!(MssqlBrokerQueue::new("", "q").is_err());
    }
}
