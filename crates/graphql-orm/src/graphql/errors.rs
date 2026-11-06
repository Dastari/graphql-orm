//! Stable public ORM error codes for GraphQL and repository surfaces.
//!
//! Internal database, migration, filesystem, and configuration details stay on
//! the server side. GraphQL clients receive only a stable code and a safe
//! message unless an application deliberately maps additional extensions.

use async_graphql::ErrorExtensions;
use std::fmt;

/// Stable public error codes exposed to GraphQL clients.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OrmErrorCode {
    /// Client input failed validation.
    InvalidInput,
    /// No authenticated principal was present.
    Unauthenticated,
    /// The principal is authenticated but not authorized.
    Forbidden,
    /// The requested resource was not found or is not visible.
    NotFound,
    /// The operation conflicts with current state.
    Conflict,
    /// A database constraint was violated without leaking schema names.
    ConstraintViolation,
    /// A pagination cursor was invalid, tampered, or version-mismatched.
    CursorInvalid,
    /// A requested page size exceeded the configured maximum.
    PageLimitExceeded,
    /// A temporary dependency failure occurred.
    ServiceUnavailable,
    /// An unexpected internal error occurred.
    InternalError,
    /// Authorization policy configuration is incomplete under strict mode.
    AuthorizationMisconfigured,
}

impl OrmErrorCode {
    /// Wire representation used in GraphQL `extensions.code`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "INVALID_INPUT",
            Self::Unauthenticated => "UNAUTHENTICATED",
            Self::Forbidden => "FORBIDDEN",
            Self::NotFound => "NOT_FOUND",
            Self::Conflict => "CONFLICT",
            Self::ConstraintViolation => "CONSTRAINT_VIOLATION",
            Self::CursorInvalid => "CURSOR_INVALID",
            Self::PageLimitExceeded => "PAGE_LIMIT_EXCEEDED",
            Self::ServiceUnavailable => "SERVICE_UNAVAILABLE",
            Self::InternalError => "INTERNAL_ERROR",
            Self::AuthorizationMisconfigured => "AUTHORIZATION_MISCONFIGURED",
        }
    }

    /// Safe default public message for this code.
    pub const fn default_message(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid input",
            Self::Unauthenticated => "unauthenticated",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not found",
            Self::Conflict => "conflict",
            Self::ConstraintViolation => "constraint violation",
            Self::CursorInvalid => "invalid cursor",
            Self::PageLimitExceeded => "page limit exceeded",
            Self::ServiceUnavailable => "service unavailable",
            Self::InternalError => "internal error",
            Self::AuthorizationMisconfigured => "authorization is misconfigured",
        }
    }
}

impl fmt::Display for OrmErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Public-facing ORM error with an optional internal diagnostic source.
#[derive(Clone)]
pub struct OrmPublicError {
    /// Stable public code.
    pub code: OrmErrorCode,
    /// Safe client-visible message.
    pub message: String,
    /// Optional correlation / audit id.
    pub correlation_id: Option<String>,
    /// Internal diagnostic text never exposed by default GraphQL extensions.
    pub(crate) internal: Option<String>,
    pub(crate) retryable: bool,
    denied_write_field: Option<Box<str>>,
    application_error: Option<Box<async_graphql::Error>>,
    pub(crate) runtime_mutation_code: Option<&'static str>,
}

impl fmt::Debug for OrmPublicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OrmPublicError")
            .field("code", &self.code)
            .field("message", &self.message)
            .field("correlation_id", &self.correlation_id)
            .field("internal", &self.internal.as_ref().map(|_| "[redacted]"))
            .field("retryable", &self.retryable)
            .finish()
    }
}

impl fmt::Display for OrmPublicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for OrmPublicError {}

impl From<sqlx::Error> for OrmPublicError {
    fn from(error: sqlx::Error) -> Self {
        Self::from_sqlx(&error)
    }
}

impl OrmPublicError {
    /// Create a public error with the default safe message for `code`.
    pub fn new(code: OrmErrorCode) -> Self {
        Self {
            code,
            message: code.default_message().to_string(),
            correlation_id: None,
            internal: None,
            retryable: false,
            runtime_mutation_code: None,
            denied_write_field: None,
            application_error: None,
        }
    }

    /// Create a public error with an explicit safe message.
    pub fn with_message(code: OrmErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            correlation_id: None,
            internal: None,
            retryable: false,
            runtime_mutation_code: None,
            denied_write_field: None,
            application_error: None,
        }
    }

    /// Attach an internal diagnostic string for server-side tracing only.
    pub fn with_internal(mut self, internal: impl Into<String>) -> Self {
        self.internal = Some(internal.into());
        self
    }

    /// Attach a correlation id that may be returned to clients.
    pub fn with_correlation_id(mut self, correlation_id: impl Into<String>) -> Self {
        self.correlation_id = Some(correlation_id.into());
        self
    }

    /// Safe runtime mutation code retained through the transaction runner, when applicable.
    pub fn runtime_mutation_code(&self) -> Option<&'static str> {
        self.runtime_mutation_code
    }

    /// Whether retrying the complete transaction is safe and recommended.
    pub const fn is_retryable(&self) -> bool {
        self.retryable
    }

    /// Convenience constructors.
    pub fn unauthenticated() -> Self {
        Self::new(OrmErrorCode::Unauthenticated)
    }

    /// Forbidden decision with the default message.
    pub fn forbidden() -> Self {
        Self::new(OrmErrorCode::Forbidden)
    }

    /// Forbidden write naming only the public GraphQL field that was refused.
    pub fn field_write_denied(field: &str) -> Self {
        let mut error = Self::with_message(
            OrmErrorCode::Forbidden,
            format!("write denied for field {field}"),
        );
        error.denied_write_field = Some(field.into());
        error
    }

    /// Not-found decision that does not reveal cross-tenant existence.
    pub fn not_found() -> Self {
        Self::new(OrmErrorCode::NotFound)
    }

    /// Internal failure that hides infrastructure details from clients.
    pub fn internal(internal: impl Into<String>) -> Self {
        Self::new(OrmErrorCode::InternalError).with_internal(internal)
    }

    /// Map a backend SQL error into a public error without leaking SQL text.
    pub fn from_sqlx(error: &sqlx::Error) -> Self {
        if let sqlx::Error::Io(error) = error
            && let Some(carrier) = error
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<PublicErrorCarrier>())
        {
            return carrier.0.clone();
        }
        let retryable = error
            .as_database_error()
            .and_then(|error| error.code())
            .is_some_and(|code| {
                matches!(
                    code.as_ref(),
                    "5" | "6" | "261" | "262" | "517" | "40001" | "40P01" | "55P03"
                )
            });
        if retryable {
            return Self {
                retryable: true,
                ..Self::new(OrmErrorCode::ServiceUnavailable).with_internal(error.to_string())
            };
        }
        match error {
            sqlx::Error::RowNotFound => Self::not_found(),
            sqlx::Error::Database(db)
                if db.is_unique_violation()
                    || db.is_foreign_key_violation()
                    || db.is_check_violation()
                    || is_sqlite_restrict_foreign_key_refusal(db.as_ref()) =>
            {
                Self::new(OrmErrorCode::ConstraintViolation).with_internal(error.to_string())
            }
            sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed => {
                Self::new(OrmErrorCode::ServiceUnavailable).with_internal(error.to_string())
            }
            other => Self::internal(other.to_string()),
        }
    }

    /// Convert into a GraphQL error, preserving explicit application callback extensions.
    /// Infrastructure failures expose only safe ORM extensions.
    pub fn into_graphql_error(self) -> async_graphql::Error {
        if let Some(error) = self.application_error {
            return *error;
        }
        let mut error = async_graphql::Error::new(self.message.clone());
        error = error.extend_with(|_, extensions| {
            extensions.set("code", self.code.as_str());
            if let Some(field) = &self.denied_write_field {
                extensions.set("deniedWriteField", field.to_string());
            }
            if let Some(correlation_id) = &self.correlation_id {
                extensions.set("correlationId", correlation_id.clone());
            }
        });
        // Keep internal diagnostics on the server-side Debug path only.
        if let Some(internal) = self.internal {
            tracing_log_internal(&self.code, &internal);
        }
        error
    }
}

/// Recognize a delete or key update refused by an SQLite `ON DELETE/UPDATE RESTRICT` action.
///
/// SQLite implements `RESTRICT` as an internal trigger program that raises
/// `RAISE(ABORT, 'FOREIGN KEY constraint failed')`, so the failure carries the
/// extended code `SQLITE_CONSTRAINT_TRIGGER` (1811) rather than
/// `SQLITE_CONSTRAINT_FOREIGNKEY` (787), and sqlx reports it as
/// `ErrorKind::Other`. The rule requires both structured signals available:
/// the SQLite driver error type with extended code 1811, and SQLite's exact
/// foreign-key message. Any other trigger failure, including an application
/// `RAISE(ABORT, ...)` with a different message, remains unclassified. An
/// application trigger that raises the identical foreign-key message is, by
/// construction, indistinguishable and is treated as a foreign-key refusal.
#[cfg(feature = "sqlite")]
fn is_sqlite_restrict_foreign_key_refusal(error: &dyn sqlx::error::DatabaseError) -> bool {
    /// SQLite extended result code `SQLITE_CONSTRAINT_TRIGGER`.
    const SQLITE_CONSTRAINT_TRIGGER: &str = "1811";
    /// The fixed message SQLite's foreign-key engine raises for a `RESTRICT` action.
    const SQLITE_FOREIGN_KEY_MESSAGE: &str = "FOREIGN KEY constraint failed";
    error
        .try_downcast_ref::<sqlx::sqlite::SqliteError>()
        .is_some()
        && error.code().as_deref() == Some(SQLITE_CONSTRAINT_TRIGGER)
        && error.message() == SQLITE_FOREIGN_KEY_MESSAGE
}

#[cfg(not(feature = "sqlite"))]
fn is_sqlite_restrict_foreign_key_refusal(_: &dyn sqlx::error::DatabaseError) -> bool {
    false
}

// Private type identity is the trust boundary: SQL/driver strings cannot create it.
struct PublicErrorCarrier(OrmPublicError);
impl fmt::Debug for PublicErrorCarrier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PublicErrorCarrier([redacted])")
    }
}
impl fmt::Display for PublicErrorCarrier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ORM public refusal")
    }
}
impl std::error::Error for PublicErrorCarrier {}

/// Preserve a trusted public error through generated SQLx-result paths.
/// The private typed carrier cannot be constructed by database or driver text.
#[doc(hidden)]
pub fn sqlx_error_from_public(error: OrmPublicError) -> sqlx::Error {
    sqlx::Error::Io(std::io::Error::other(PublicErrorCarrier(error)))
}

fn public_error_transport(error: OrmPublicError) -> String {
    if let Some(internal) = &error.internal {
        tracing_log_internal(&error.code, internal);
    }
    if let Some(field) = &error.denied_write_field {
        return format!("graphql-orm-public:FIELD_WRITE_DENIED:{field}");
    }
    format!("graphql-orm-public:{}", error.code.as_str())
}

fn public_code(code: &str) -> Option<OrmErrorCode> {
    match code {
        "INVALID_INPUT" => Some(OrmErrorCode::InvalidInput),
        "UNAUTHENTICATED" => Some(OrmErrorCode::Unauthenticated),
        "FORBIDDEN" => Some(OrmErrorCode::Forbidden),
        "NOT_FOUND" => Some(OrmErrorCode::NotFound),
        "CONFLICT" => Some(OrmErrorCode::Conflict),
        "CONSTRAINT_VIOLATION" => Some(OrmErrorCode::ConstraintViolation),
        "CURSOR_INVALID" => Some(OrmErrorCode::CursorInvalid),
        "PAGE_LIMIT_EXCEEDED" => Some(OrmErrorCode::PageLimitExceeded),
        "SERVICE_UNAVAILABLE" => Some(OrmErrorCode::ServiceUnavailable),
        "AUTHORIZATION_MISCONFIGURED" => Some(OrmErrorCode::AuthorizationMisconfigured),
        "INTERNAL_ERROR" => Some(OrmErrorCode::InternalError),
        _ => None,
    }
}

fn public_error_from_loader_transport(message: &str) -> OrmPublicError {
    if let Some(field) = message.strip_prefix("graphql-orm-public:FIELD_WRITE_DENIED:") {
        return OrmPublicError::field_write_denied(field);
    }
    let code = message
        .strip_prefix("graphql-orm-public:")
        .and_then(public_code)
        .unwrap_or(OrmErrorCode::InternalError);
    OrmPublicError::new(code)
}

/// Preserve only the safe public category through a generated loader's String error.
/// Internal diagnostics follow the same server-only path as GraphQL errors.
#[doc(hidden)]
pub fn loader_error_from_sqlx(error: sqlx::Error) -> String {
    public_error_transport(OrmPublicError::from(error))
}

/// Decode a generated loader's safe category, sanitizing unknown host failures.
#[doc(hidden)]
pub fn graphql_error_from_loader(error: String) -> async_graphql::Error {
    if error.starts_with("graphql-orm-public:") {
        public_error_from_loader_transport(&error).into_graphql_error()
    } else {
        graphql_internal_error(error)
    }
}

/// Preserve an application callback's explicit message and extensions through SQLx.
/// Only call this at a trusted GraphQL callback boundary; never with driver text.
#[doc(hidden)]
pub fn sqlx_error_from_graphql(error: async_graphql::Error) -> sqlx::Error {
    let category = error
        .extensions
        .as_ref()
        .and_then(|ext| ext.get("code"))
        .and_then(|value| match value {
            async_graphql::Value::String(code) => Some(code.as_str()),
            _ => None,
        });
    let mut public = OrmPublicError::new(
        category
            .and_then(public_code)
            .unwrap_or(OrmErrorCode::InternalError),
    );
    public.message = error.message.clone();
    public.application_error = Some(Box::new(error));
    sqlx_error_from_public(public)
}

/// Map a repository/SQL error into a GraphQL error with safe public fields.
pub fn graphql_error_from_sqlx(error: sqlx::Error) -> async_graphql::Error {
    OrmPublicError::from_sqlx(&error).into_graphql_error()
}

/// Map any displayable internal failure into a safe GraphQL internal error.
pub fn graphql_internal_error(error: impl fmt::Display) -> async_graphql::Error {
    OrmPublicError::internal(error.to_string()).into_graphql_error()
}

fn tracing_log_internal(code: &OrmErrorCode, internal: &str) {
    // Avoid pulling a tracing dependency into the public crate surface.
    // Hosts can install a custom panic/log hook; we emit to stderr only when
    // the `GRAPHQL_ORM_LOG_INTERNAL_ERRORS` environment variable is set.
    if std::env::var_os("GRAPHQL_ORM_LOG_INTERNAL_ERRORS").is_some() {
        eprintln!("graphql-orm internal error code={code} detail={internal}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_refusal_roundtrips_but_prefix_text_is_not_trusted() {
        let error = async_graphql::Error::new("application refusal")
            .extend_with(|_, ext| ext.set("code", "CUSTOM_REFUSAL"));
        let restored = graphql_error_from_sqlx(sqlx_error_from_graphql(error));
        assert_eq!(restored.message, "application refusal");
        assert_eq!(
            restored.extensions.unwrap().get("code"),
            Some(&async_graphql::Value::from("CUSTOM_REFUSAL"))
        );
        for text in [
            "graphql-orm-public:FIELD_WRITE_DENIED:secret",
            "graphql-orm-public:FORBIDDEN",
        ] {
            for source in [
                sqlx::Error::Protocol(text.into()),
                sqlx::Error::Io(std::io::Error::other(text.to_owned())),
            ] {
                let error = graphql_error_from_sqlx(source);
                assert_eq!(error.message, "internal error");
                assert_eq!(
                    error.extensions.unwrap().get("code"),
                    Some(&async_graphql::Value::from("INTERNAL_ERROR"))
                );
            }
        }
    }

    #[test]
    fn graphql_extensions_are_safe() {
        let error = OrmPublicError::internal("relation \"secret_table\" does not exist")
            .with_correlation_id("corr-1")
            .into_graphql_error();
        assert_eq!(error.message, "internal error");
        let extensions = error.extensions.expect("extensions");
        assert_eq!(
            extensions.get("code").map(|value| value.to_string()),
            Some("\"INTERNAL_ERROR\"".to_string())
        );
        assert_eq!(
            extensions
                .get("correlationId")
                .map(|value| value.to_string()),
            Some("\"corr-1\"".to_string())
        );
        let rendered = format!("{extensions:?}");
        assert!(!rendered.contains("secret_table"));
    }

    #[test]
    fn repository_policy_category_survives_legacy_sqlx_result_without_detail() {
        let sqlx = sqlx_error_from_public(
            OrmPublicError::new(OrmErrorCode::AuthorizationMisconfigured)
                .with_internal("secret policy and table detail"),
        );
        assert!(matches!(sqlx, sqlx::Error::Io(_)));
        assert!(!sqlx.to_string().contains("secret policy"));
        let public = OrmPublicError::from(sqlx);
        assert_eq!(public.code, OrmErrorCode::AuthorizationMisconfigured);
        assert!(!format!("{public:?}").contains("secret policy"));

        let graphql = OrmPublicError::forbidden().into_graphql_error();
        let public = OrmPublicError::from(sqlx_error_from_graphql(graphql));
        assert_eq!(public.code, OrmErrorCode::Forbidden);
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_restrict_refusal_is_a_constraint_violation_but_other_triggers_are_not() {
        use sqlx::Connection;
        let mut conn = sqlx::SqliteConnection::connect("sqlite::memory:")
            .await
            .unwrap();
        for statement in [
            "PRAGMA foreign_keys = ON",
            "CREATE TABLE parent (id TEXT PRIMARY KEY)",
            "CREATE TABLE restricted (id TEXT PRIMARY KEY, parent TEXT REFERENCES parent(id) ON DELETE RESTRICT)",
            "CREATE TABLE no_action (id TEXT PRIMARY KEY, parent TEXT REFERENCES parent(id))",
            "CREATE TABLE guarded (id TEXT PRIMARY KEY)",
            "CREATE TRIGGER guarded_abort BEFORE DELETE ON guarded BEGIN SELECT RAISE(ABORT, 'guarded rows are immutable'); END",
            "INSERT INTO parent (id) VALUES ('r'), ('n')",
            "INSERT INTO restricted (id, parent) VALUES ('c', 'r')",
            "INSERT INTO no_action (id, parent) VALUES ('c', 'n')",
            "INSERT INTO guarded (id) VALUES ('g')",
        ] {
            sqlx::query(statement).execute(&mut conn).await.unwrap();
        }
        let classify = |error: sqlx::Error| {
            let code = error
                .as_database_error()
                .and_then(|db| db.code().map(|code| code.into_owned()));
            (code, OrmPublicError::from_sqlx(&error).code)
        };

        let restrict = sqlx::query("DELETE FROM parent WHERE id = 'r' RETURNING id")
            .fetch_all(&mut conn)
            .await
            .err()
            .expect("RESTRICT refusal");
        assert_eq!(
            classify(restrict),
            (Some("1811".into()), OrmErrorCode::ConstraintViolation)
        );

        let no_action = sqlx::query("DELETE FROM parent WHERE id = 'n'")
            .execute(&mut conn)
            .await
            .unwrap_err();
        assert_eq!(
            classify(no_action),
            (Some("787".into()), OrmErrorCode::ConstraintViolation)
        );

        let trigger = sqlx::query("DELETE FROM guarded WHERE id = 'g'")
            .execute(&mut conn)
            .await
            .unwrap_err();
        assert_eq!(
            classify(trigger),
            (Some("1811".into()), OrmErrorCode::InternalError)
        );

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM parent")
            .fetch_one(&mut conn)
            .await
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn debug_redacts_internal() {
        let error = OrmPublicError::internal("password=super-secret");
        let debug = format!("{error:?}");
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("super-secret"));
    }
}
