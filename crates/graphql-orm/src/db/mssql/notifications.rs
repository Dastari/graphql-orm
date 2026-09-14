//! Explicit, externally provisioned Service Broker notification capability.
//!
//! This connection is separate from entity pools. It never provisions schema or
//! enables business DML. SQL permissions remain the authoritative boundary.
use super::{MssqlClient, MssqlRow, map_tiberius_error};
use quick_xml::{Reader, events::Event};
use tiberius::{Config, QueryNotification};
use tokio::net::TcpStream;
use tokio_util::compat::TokioAsyncWriteCompatExt;

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
/// After cancellation or a protocol error, discard and reconnect this connection.
pub struct MssqlQueryNotificationConnection {
    client: MssqlClient,
    queue: MssqlBrokerQueue,
    database: String,
}

impl MssqlQueryNotificationConnection {
    /// Connect using a principal with SELECT, SUBSCRIBE QUERY NOTIFICATIONS,
    /// RECEIVE on this queue and dbo.QueryNotificationErrorsQueue, and
    /// permission to end its conversations.
    pub async fn connect_ado(
        connection_string: &str,
        queue: MssqlBrokerQueue,
    ) -> crate::Result<Self> {
        let config = Config::from_ado_string(connection_string).map_err(map_tiberius_error)?;
        let tcp = TcpStream::connect(config.get_addr()).await?;
        tcp.set_nodelay(true)?;
        let mut client = MssqlClient::connect(config, tcp.compat_write())
            .await
            .map_err(map_tiberius_error)?;
        let rows = client.simple_query("SET ANSI_NULLS ON; SET ANSI_PADDING ON; SET ANSI_WARNINGS ON; SET CONCAT_NULL_YIELDS_NULL ON; SET QUOTED_IDENTIFIER ON; SET NUMERIC_ROUNDABORT OFF; SET ARITHABORT ON; SET TRANSACTION ISOLATION LEVEL READ COMMITTED; SELECT DB_NAME() AS db;")
            .await.map_err(map_tiberius_error)?.into_first_result().await.map_err(map_tiberius_error)?;
        let database = rows
            .first()
            .and_then(|r| r.get::<&str, _>("db"))
            .ok_or_else(|| protocol("missing notification database"))?
            .to_owned();
        Ok(Self {
            client,
            queue,
            database,
        })
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
    pub async fn receive(
        &mut self,
        wait_ms: u32,
    ) -> crate::Result<Option<MssqlQueryNotificationMessage>> {
        if wait_ms > 60_000 {
            return Err(protocol("Broker wait exceeds 60000 milliseconds"));
        }
        let sql = format!(
            "WAITFOR (RECEIVE TOP (1) conversation_handle, message_type_name, CASE WHEN DATALENGTH(message_body) <= 16384 THEN CONVERT(nvarchar(max), message_body) ELSE NULL END AS body FROM {}), TIMEOUT {wait_ms};",
            self.queue.0
        );
        let row = self
            .client
            .simple_query(sql)
            .await
            .map_err(map_tiberius_error)?
            .into_row()
            .await
            .map_err(map_tiberius_error)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let handle = row
            .try_get::<uuid::Uuid, _>("conversation_handle")
            .map_err(map_tiberius_error)?
            .ok_or_else(|| protocol("Broker message without conversation"))?;
        let message_type = row
            .try_get::<&str, _>("message_type_name")
            .map_err(map_tiberius_error)?;
        let body = row.try_get::<&str, _>("body").map_err(map_tiberius_error)?;
        let result = if message_type
            == Some("http://schemas.microsoft.com/SQL/Notifications/QueryNotification")
        {
            body.and_then(parse_notification).unwrap_or_else(other)
        } else {
            other()
        };
        // UUID formatting is generated from a decoded UUID, never SQL input.
        self.client
            .simple_query(format!("END CONVERSATION '{handle}';"))
            .await
            .map_err(map_tiberius_error)?
            .into_results()
            .await
            .map_err(map_tiberius_error)?;
        Ok(Some(result))
    }
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
                    let value = attr
                        .decode_and_unescape_value(reader.decoder())
                        .ok()?
                        .into_owned();
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
