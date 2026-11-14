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
/// After a failed or cancelled [`receive`](Self::receive), keep using this
/// value: it re-establishes its own connection before the next call.
pub struct MssqlQueryNotificationConnection {
    client: MssqlClient,
    queue: MssqlBrokerQueue,
    database: String,
    config: tiberius::Config,
    connect_timeout: Duration,
    /// The session may hold an open receive transaction or a half-read
    /// response: connect again before the next statement. Closing the old
    /// session makes SQL Server roll its transaction back.
    reconnect: bool,
    /// A message already read whose acknowledgement has not finished. It is
    /// handed to the caller whatever happens to that acknowledgement.
    held: Option<MssqlQueryNotificationMessage>,
    uncertain_acknowledgements: u64,
    #[cfg(test)]
    acknowledgement_fault: Option<AcknowledgementFault>,
}

/// Failures a test injects into the acknowledgement of a received message.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AcknowledgementFault {
    /// The commit never reaches the server.
    BeforeCommit,
    /// The server commits and the response is lost.
    ResponseLost,
    /// The acknowledgement never completes, so the caller's future is dropped.
    Never,
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
            client,
            queue,
            database,
            config,
            connect_timeout,
            reconnect: false,
            held: None,
            uncertain_acknowledgements: 0,
            #[cfg(test)]
            acknowledgement_fault: None,
        })
    }

    /// How many acknowledgements ended without a known outcome: the commit
    /// failed, its response was lost, or a `receive` future was dropped while
    /// it ran. Each such message was still returned to the caller and may be
    /// delivered once more.
    pub fn uncertain_acknowledgements(&self) -> u64 {
        self.uncertain_acknowledgements
    }

    /// Replace a session whose state is unknown. Dropping the old client
    /// closes its socket, and SQL Server rolls back what it left open.
    async fn ensure_connected(&mut self) -> crate::Result<()> {
        if self.reconnect {
            let (client, database) = establish(self.config.clone(), self.connect_timeout).await?;
            self.client = client;
            self.database = database;
            self.reconnect = false;
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
        self.ensure_connected().await?;
        let options = format!("service={service};local database={}", self.database);
        let notification = QueryNotification::new(notification_id, &options, timeout_seconds)
            .map_err(map_tiberius_error)?;
        let rows = self
            .client
            .simple_query_with_notification(sql, notification)
            .await
            .map_err(map_tiberius_error)?
            .into_first_result()
            .await
            .map_err(map_tiberius_error)?;
        Ok(rows.into_iter().map(|inner| MssqlRow { inner }).collect())
    }

    /// Wait at most 60 seconds for one notification; `None` means timeout.
    /// The received conversation is ended. Malformed, oversized, and unknown
    /// messages return `Other`, requiring conservative resynchronization.
    ///
    /// Delivery is **at-least-once; consumers must be idempotent.** The
    /// message is received and its conversation ended inside a transaction
    /// that commits only after the message has been read by this process.
    ///
    /// - If the receive or `END CONVERSATION` fails, the transaction rolls
    ///   back, the error is returned and the message stays on the queue.
    /// - Once the message has been read it is returned as `Ok` whatever
    ///   happens to the commit. A commit that fails, or whose response is
    ///   lost, is counted in
    ///   [`uncertain_acknowledgements`](Self::uncertain_acknowledgements) and
    ///   the connection is re-established before the next call. If the commit
    ///   did not happen, the message is delivered again later.
    /// - **Cancellation.** Dropping the future before the message has been
    ///   read rolls the receive back: the next call connects again and the
    ///   message is redelivered. Dropping it while the commit is in progress
    ///   keeps the message in this value: the next `receive` returns it at
    ///   once, and it may be delivered once more if the commit had not
    ///   happened. Dropping this value itself during that commit is the one
    ///   case that can lose the message, so keep the connection and call
    ///   `receive` again after a cancelled call.
    ///
    /// A message may therefore be delivered more than once and never less.
    /// Messages are invalidation hints, so handling one twice only repeats a
    /// refresh.
    ///
    /// SQL Server disables a queue after five consecutive rolled-back
    /// receives (poison-message handling), so a persistent failure stops
    /// delivery visibly instead of dropping notifications.
    pub async fn receive(
        &mut self,
        wait_ms: u32,
    ) -> crate::Result<Option<MssqlQueryNotificationMessage>> {
        if wait_ms > 60_000 {
            return Err(protocol("Broker wait exceeds 60000 milliseconds"));
        }
        // A call that was dropped while it acknowledged left its message here.
        if let Some(message) = self.held.take() {
            self.note_uncertain_acknowledgement("the receive was cancelled while acknowledging");
            return Ok(Some(message));
        }
        self.ensure_connected().await?;
        // Until the acknowledgement is known to have finished, the session is
        // not reusable: an error or a dropped future leaves it marked.
        self.reconnect = true;
        let message = self.receive_unacknowledged(wait_ms).await?;
        self.held = message;
        let acknowledged = self.acknowledge().await;
        let message = self.held.take();
        match acknowledged {
            Ok(()) => self.reconnect = false,
            // Nothing was received, so nothing can be lost; reconnect anyway.
            Err(_) if message.is_none() => {}
            Err(error) => self.note_uncertain_acknowledgement(&error.to_string()),
        }
        Ok(message)
    }

    /// Record an acknowledgement whose outcome is unknown. The message is
    /// never part of the log line.
    fn note_uncertain_acknowledgement(&mut self, reason: &str) {
        self.reconnect = true;
        self.uncertain_acknowledgements = self.uncertain_acknowledgements.saturating_add(1);
        tracing::debug!(
            reason,
            total = self.uncertain_acknowledgements,
            "SQL Server notification acknowledgement did not complete; \
             the message was delivered and may be delivered again"
        );
    }

    /// Receive at most one message and end its conversation, leaving the
    /// transaction open. Nothing is durable until [`Self::acknowledge`].
    async fn receive_unacknowledged(
        &mut self,
        wait_ms: u32,
    ) -> crate::Result<Option<MssqlQueryNotificationMessage>> {
        let row = self
            .client
            .simple_query(receive_batch(&self.queue, wait_ms))
            .await
            .map_err(map_tiberius_error)?
            .into_row()
            .await
            .map_err(map_tiberius_error)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let message_type = row
            .try_get::<&str, _>("message_type_name")
            .map_err(map_tiberius_error)?;
        let body = row.try_get::<&str, _>("body").map_err(map_tiberius_error)?;
        Ok(Some(
            if message_type
                == Some("http://schemas.microsoft.com/SQL/Notifications/QueryNotification")
            {
                body.and_then(parse_notification).unwrap_or_else(other)
            } else {
                other()
            },
        ))
    }

    /// Commit the receive whose message this process now holds.
    async fn acknowledge(&mut self) -> crate::Result<()> {
        #[cfg(test)]
        match self.acknowledgement_fault.take() {
            Some(AcknowledgementFault::BeforeCommit) => {
                return Err(protocol("injected: the commit was never sent"));
            }
            Some(AcknowledgementFault::Never) => std::future::pending::<()>().await,
            Some(AcknowledgementFault::ResponseLost) => {
                self.commit().await?;
                return Err(protocol("injected: the commit response was lost"));
            }
            None => {}
        }
        self.commit().await
    }

    async fn commit(&mut self) -> crate::Result<()> {
        self.client
            .simple_query(ACKNOWLEDGE_BATCH)
            .await
            .map_err(map_tiberius_error)?
            .into_results()
            .await
            .map_err(map_tiberius_error)?;
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

/// Commits the transaction [`receive_batch`] left open.
const ACKNOWLEDGE_BATCH: &str = "IF @@TRANCOUNT > 0 COMMIT TRANSACTION;";

/// One batch that receives at most one message and ends its conversation in
/// a transaction it leaves open; its only result set is the received message,
/// if any. A transaction an interrupted earlier receive left open is rolled
/// back first, which returns that message to the queue. If the receive or
/// `END CONVERSATION` fails, the transaction is rolled back and the error
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
        // The batch itself never commits: the message must reach this
        // process first, and only the acknowledgement makes the receive durable.
        assert!(!sql.contains("COMMIT"), "{sql}");
        assert_eq!(ACKNOWLEDGE_BATCH, "IF @@TRANCOUNT > 0 COMMIT TRANSACTION;");
    }

    /// Runs on a disposable SQL Server this test owns.
    #[tokio::test]
    #[ignore = "requires Docker; owns a disposable loopback SQL Server container"]
    async fn unacknowledged_and_failed_receives_are_redelivered()
    -> Result<(), Box<dyn std::error::Error>> {
        use super::super::owned_mssql::{OwnedSqlServer, execute_batch};

        let mut server = OwnedSqlServer::start().await?;
        let connection = server.connection_string.clone();
        execute_batch(
            &connection,
            "CREATE QUEUE dbo.RedeliveryQueue; \
             CREATE SERVICE RedeliverySource ON QUEUE dbo.RedeliveryQueue ([DEFAULT]); \
             CREATE QUEUE dbo.RedeliveryReplies; \
             CREATE SERVICE RedeliveryReplies ON QUEUE dbo.RedeliveryReplies ([DEFAULT]);",
        )
        .await?;
        let send = "DECLARE @h uniqueidentifier; \
             BEGIN DIALOG @h FROM SERVICE RedeliveryReplies TO SERVICE 'RedeliverySource' \
             ON CONTRACT [DEFAULT] WITH ENCRYPTION = OFF; \
             SEND ON CONVERSATION @h MESSAGE TYPE [DEFAULT] (N'payload');";
        let queue = MssqlBrokerQueue::new("dbo", "RedeliveryQueue")?;
        let connect = || MssqlQueryNotificationConnection::connect_ado(&connection, queue.clone());

        // The message reaches this process, then the connection is lost
        // before the acknowledgement: it must be delivered again.
        execute_batch(&connection, send).await?;
        let mut interrupted = connect().await?;
        assert!(
            interrupted.receive_unacknowledged(5000).await?.is_some(),
            "the message is delivered"
        );
        drop(interrupted);
        let mut listener = connect().await?;
        assert!(
            listener.receive(5000).await?.is_some(),
            "an unacknowledged message is redelivered"
        );
        assert!(
            listener.receive(200).await?.is_none(),
            "an acknowledged message is delivered once"
        );
        assert_eq!(listener.uncertain_acknowledgements(), 0);

        // The server commits and the response is lost: the message is still
        // returned, and because the commit happened it is not delivered again.
        execute_batch(&connection, send).await?;
        listener.acknowledgement_fault = Some(AcknowledgementFault::ResponseLost);
        assert!(
            listener.receive(5000).await?.is_some(),
            "a lost commit response must not lose the message"
        );
        assert_eq!(listener.uncertain_acknowledgements(), 1);
        assert!(listener.reconnect, "the session is replaced before reuse");
        assert!(
            listener.receive(200).await?.is_none(),
            "a committed receive is not redelivered"
        );
        assert!(!listener.reconnect);

        // The acknowledgement fails before the commit: the message is still
        // returned, and it is delivered exactly once more.
        execute_batch(&connection, send).await?;
        listener.acknowledgement_fault = Some(AcknowledgementFault::BeforeCommit);
        assert!(
            listener.receive(5000).await?.is_some(),
            "a failed acknowledgement must not lose the message"
        );
        assert_eq!(listener.uncertain_acknowledgements(), 2);
        assert!(
            listener.receive(5000).await?.is_some(),
            "an uncommitted receive is redelivered"
        );
        assert!(listener.receive(200).await?.is_none(), "and only once");
        assert_eq!(listener.uncertain_acknowledgements(), 2);

        // The future is dropped between reading the message and finishing its
        // acknowledgement: the next call returns the message, and because
        // nothing was committed it is delivered once more.
        execute_batch(&connection, send).await?;
        listener.acknowledgement_fault = Some(AcknowledgementFault::Never);
        assert!(
            tokio::time::timeout(Duration::from_secs(8), listener.receive(5000))
                .await
                .is_err(),
            "the receive is cancelled while acknowledging"
        );
        assert!(listener.held.is_some(), "the read message is kept");
        assert!(
            listener.receive(5000).await?.is_some(),
            "a cancelled receive hands its message to the next call"
        );
        assert_eq!(listener.uncertain_acknowledgements(), 3);
        assert!(
            listener.receive(5000).await?.is_some(),
            "the rolled-back receive is redelivered"
        );
        assert!(listener.receive(200).await?.is_none(), "and only once");

        // The future is dropped before any message was read: nothing is lost
        // and nothing is duplicated.
        assert!(
            tokio::time::timeout(Duration::from_millis(300), listener.receive(5000))
                .await
                .is_err()
        );
        assert!(listener.reconnect, "a cancelled wait replaces the session");
        execute_batch(&connection, send).await?;
        assert!(listener.receive(5000).await?.is_some());
        assert!(listener.receive(200).await?.is_none());
        assert_eq!(listener.uncertain_acknowledgements(), 3);

        // `END CONVERSATION` fails (an unknown handle stands in for the
        // failure): the receive is rolled back and the message stays queued.
        execute_batch(&connection, send).await?;
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

        // Registration works on a session that had to be replaced.
        execute_batch(&connection, send).await?;
        listener.acknowledgement_fault = Some(AcknowledgementFault::ResponseLost);
        assert!(listener.receive(5000).await?.is_some());
        assert!(listener.reconnect);
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
                "after-reconnect",
                "RedeliveryNotify",
                60,
            )
            .await?;
        assert!(!listener.reconnect);

        drop(listener);
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
