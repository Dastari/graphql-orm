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
    // An authored refusal's own message and extensions, rendered only at the
    // GraphQL boundary. Not the whole `async_graphql::Error`, whose type-erased
    // source would make this public type lose its auto traits.
    application_refusal: Option<Box<ApplicationRefusal>>,
    pub(crate) runtime_mutation_code: Option<&'static str>,
}

#[derive(Clone)]
struct ApplicationRefusal {
    message: String,
    extensions: Option<async_graphql::ErrorExtensionValues>,
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
            application_refusal: None,
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
            application_refusal: None,
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
        if let sqlx::Error::Decode(error) = error
            && let Some(carrier) = error.downcast_ref::<PublicErrorCarrier>()
        {
            return carrier.0.clone();
        }
        // SQL Server error numbers share the text space of SQLite result
        // codes and SQLSTATEs (262 is a permission error there), so a SQL
        // Server error is classified by its own type only.
        let mssql = mssql_server_error(error);
        let retryable = match mssql {
            Some(retryable) => retryable,
            None => error
                .as_database_error()
                .and_then(|error| error.code())
                .is_some_and(|code| {
                    matches!(
                        code.as_ref(),
                        "5" | "6" | "261" | "262" | "517" | "40001" | "40P01" | "55P03"
                    )
                }),
        };
        if retryable {
            return Self {
                retryable: true,
                ..Self::new(OrmErrorCode::ServiceUnavailable).with_internal(error.to_string())
            };
        }
        match error {
            sqlx::Error::RowNotFound => Self::not_found(),
            // PostgreSQL `insufficient_privilege`: a row-level security
            // `WITH CHECK` refusal or a missing privilege.
            sqlx::Error::Database(db)
                if mssql.is_none() && db.code().as_deref() == Some("42501") =>
            {
                Self::new(OrmErrorCode::Forbidden).with_internal(error.to_string())
            }
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

    /// Classify a host policy callback failure where no authored text may
    /// reach a caller unless the callback marked it as a public refusal.
    ///
    /// A refusal with an ORM public `code` extension other than
    /// `INTERNAL_ERROR`, or a typed [`OrmPublicError`] propagated with `?`,
    /// keeps its code and message. Every other failure, including a plain
    /// `async_graphql::Error::new(text)` without a code, reads as
    /// `INTERNAL_ERROR` with the default message on every surface; its text is
    /// kept as a server-side diagnostic only.
    pub(crate) fn from_callback(error: async_graphql::Error) -> Self {
        let mut public = Self::from(sqlx_error_from_graphql(error));
        if public.code == OrmErrorCode::InternalError {
            // The general classifier lets the GraphQL boundary render an
            // uncoded authored message as written. These callbacks get no
            // such allowance: only a known public code marks text as safe.
            public.application_refusal = None;
        }
        public
    }

    /// Convert into a GraphQL error, preserving explicit application callback extensions.
    /// Infrastructure failures expose only safe ORM extensions.
    pub fn into_graphql_error(self) -> async_graphql::Error {
        if let Some(refusal) = self.application_refusal {
            if let Some(internal) = &self.internal {
                tracing_log_internal(&self.code, internal);
            }
            let mut error = async_graphql::Error::new(refusal.message);
            error.extensions = refusal.extensions;
            return error;
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

/// `Some(retryable)` when `error` is an error SQL Server reported.
#[cfg(feature = "mssql")]
fn mssql_server_error(error: &sqlx::Error) -> Option<bool> {
    error
        .as_database_error()
        .and_then(|error| error.try_downcast_ref::<crate::db::mssql::MssqlServerError>())
        .map(crate::db::mssql::MssqlServerError::is_retryable)
}

#[cfg(not(feature = "mssql"))]
fn mssql_server_error(_error: &sqlx::Error) -> Option<bool> {
    None
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
// It travels as `Decode`, not `Io`, because consumers commonly treat `Io` as a
// transient connection failure and retry it, and a refusal must never be retried.
struct PublicErrorCarrier(OrmPublicError);
impl fmt::Debug for PublicErrorCarrier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PublicErrorCarrier([redacted])")
    }
}
impl fmt::Display for PublicErrorCarrier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Hosts commonly turn a repository error into a GraphQL message with
        // `to_string()`, so this text may reach clients. It carries only the
        // code's default message and the stable code: never authored text or
        // the transport token.
        let code = self.0.code;
        write!(f, "{} ({})", code.default_message(), code.as_str())
    }
}
impl std::error::Error for PublicErrorCarrier {}

/// Preserve a trusted public error through generated SQLx-result paths.
/// The private typed carrier cannot be constructed by database or driver text.
#[doc(hidden)]
pub fn sqlx_error_from_public(error: OrmPublicError) -> sqlx::Error {
    sqlx::Error::Decode(Box::new(PublicErrorCarrier(error)))
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

pub(crate) fn public_code(code: &str) -> Option<OrmErrorCode> {
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
///
/// A callback's code and message are kept when it carries an allow-listed ORM
/// public `code` extension, or is a typed [`OrmPublicError`] propagated with `?`,
/// on both repository and GraphQL surfaces. `INTERNAL_ERROR`, by either route,
/// always has the default message. Any other authored error reads as
/// `INTERNAL_ERROR` with the default message for repository callers, while
/// [`OrmPublicError::into_graphql_error`] still renders it as the callback wrote
/// it, matching generated GraphQL mutations.
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
    // `?` in a callback converts through `From<T: Display>`, which records a source and
    // takes the failure's own text as the message. That is not an authored refusal,
    // unless the source is itself a typed public error the host constructed.
    if error.source.is_some() && category.is_none() {
        let source = error.source.as_ref();
        if let Some(public) = source.and_then(|source| source.downcast_ref::<OrmPublicError>()) {
            let mut public = public.clone();
            if public.code == OrmErrorCode::InternalError {
                // As for a code extension, `INTERNAL_ERROR` never marks its text as safe.
                let message = std::mem::replace(
                    &mut public.message,
                    OrmErrorCode::InternalError.default_message().to_string(),
                );
                if public.internal.is_none() {
                    public.internal = Some(message);
                }
            }
            return sqlx_error_from_public(public);
        }
        return sqlx_error_from_public(
            source
                .and_then(|source| source.downcast_ref::<sqlx::Error>())
                .map(OrmPublicError::from_sqlx)
                .unwrap_or_else(|| OrmPublicError::internal(error.message.clone())),
        );
    }
    let known = category.and_then(public_code);
    let code = known.unwrap_or(OrmErrorCode::InternalError);
    let mut public = OrmPublicError::new(code);
    if code == OrmErrorCode::InternalError {
        // Only an allow-listed public code marks the message as safe for
        // repository callers; any other message stays a server-side diagnostic.
        public.internal = Some(error.message.clone());
        if known.is_some() {
            // An explicit `INTERNAL_ERROR` is internal on every surface.
            return sqlx_error_from_public(public);
        }
    } else {
        public.message = error.message.clone();
    }
    // The GraphQL boundary still renders an authored refusal as written.
    public.application_refusal = Some(Box::new(ApplicationRefusal {
        message: error.message,
        extensions: error.extensions,
    }));
    sqlx_error_from_public(public)
}

/// Apply the bulk callback refusal classifier at a GraphQL callback boundary.
///
/// Source-bearing failures without a string `code` extension receive a safe ORM
/// category. Authored refusals retain their message and all extensions. Internal
/// diagnostics use the same opt-in server reporting path as repository failures.
#[doc(hidden)]
pub fn graphql_error_from_callback(error: impl Into<async_graphql::Error>) -> async_graphql::Error {
    graphql_error_from_sqlx(sqlx_error_from_graphql(error.into()))
}

/// Map a repository/SQL error into a GraphQL error with safe public fields.
pub fn graphql_error_from_sqlx(error: sqlx::Error) -> async_graphql::Error {
    OrmPublicError::from_sqlx(&error).into_graphql_error()
}

/// Map any displayable internal failure into a safe GraphQL internal error.
pub fn graphql_internal_error(error: impl fmt::Display) -> async_graphql::Error {
    OrmPublicError::internal(error.to_string()).into_graphql_error()
}

/// Report an internal diagnostic through `tracing`.
///
/// Every report is an event on the `graphql_orm::internal_error` target that
/// carries the public code. Failures (`INTERNAL_ERROR`, `SERVICE_UNAVAILABLE`
/// and `AUTHORIZATION_MISCONFIGURED`) are `ERROR` events and refusals are
/// `DEBUG` events. The diagnostic text can quote driver or host callback
/// messages, so it is attached as the `detail` field only when the operator
/// has set `GRAPHQL_ORM_LOG_INTERNAL_ERRORS`. The ORM never formats a row or
/// a bound value into it.
fn tracing_log_internal(code: &OrmErrorCode, internal: &str) {
    let detail = std::env::var_os("GRAPHQL_ORM_LOG_INTERNAL_ERRORS").map(|_| internal);
    let code = code.as_str();
    if matches!(
        code,
        "INTERNAL_ERROR" | "SERVICE_UNAVAILABLE" | "AUTHORIZATION_MISCONFIGURED"
    ) {
        tracing::error!(
            target: "graphql_orm::internal_error",
            code,
            detail,
            "graphql-orm internal error"
        );
    } else {
        tracing::debug!(
            target: "graphql_orm::internal_error",
            code,
            detail,
            "graphql-orm refusal"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carried_refusals_read_as_their_code_and_never_as_a_connection_failure() {
        let error = sqlx_error_from_public(OrmPublicError::new(
            OrmErrorCode::AuthorizationMisconfigured,
        ));
        assert!(!matches!(error, sqlx::Error::Io(_)));
        assert!(error.to_string().contains("AUTHORIZATION_MISCONFIGURED"));
        assert!(!error.to_string().contains("graphql-orm-public"));
        let refusal = async_graphql::Error::new("private application detail")
            .extend_with(|_, ext| ext.set("code", "FORBIDDEN"));
        let error = sqlx_error_from_graphql(refusal);
        assert!(!error.to_string().contains("private application detail"));
        assert!(!format!("{error:?}").contains("private application detail"));
    }

    #[test]
    fn converted_callback_failures_stay_masked_but_authored_refusals_do_not() {
        // `?` inside a callback converts through `From<T: Display>`, so the message is
        // the failure's own text, which can hold SQL or driver detail.
        let text = "SELECT secret FROM private_table";
        for converted in [
            async_graphql::Error::from(sqlx::Error::Protocol(text.into())),
            async_graphql::Error::from(std::io::Error::other(text)),
        ] {
            let error = graphql_error_from_sqlx(sqlx_error_from_graphql(converted));
            assert_eq!(error.message, "internal error");
            assert_eq!(
                error.extensions.unwrap().get("code"),
                Some(&async_graphql::Value::from("INTERNAL_ERROR"))
            );
        }
        let plain = graphql_error_from_sqlx(sqlx_error_from_graphql(async_graphql::Error::new(
            "uncoded refusal",
        )));
        assert_eq!(plain.message, "uncoded refusal");
        let coded = async_graphql::Error::from(std::io::Error::other("explicit refusal"))
            .extend_with(|_, ext| ext.set("code", "CUSTOM_REFUSAL"));
        let coded = graphql_error_from_sqlx(sqlx_error_from_graphql(coded));
        assert_eq!(coded.message, "explicit refusal");
    }

    #[test]
    fn callback_sources_mean_the_graphql_field_even_for_leaf_errors() {
        // A leaf error has no nested std::error::Error source; From still stores it.
        let leaf = std::io::Error::other("private leaf diagnostic");
        assert!(std::error::Error::source(&leaf).is_none());
        let converted = async_graphql::Error::from(leaf);
        assert!(converted.source.is_some());
        let public = OrmPublicError::from(sqlx_error_from_graphql(converted));
        assert_eq!(public.code, OrmErrorCode::InternalError);
        assert_eq!(public.internal.as_deref(), Some("private leaf diagnostic"));

        struct HostRefusal;
        impl From<HostRefusal> for async_graphql::Error {
            fn from(_: HostRefusal) -> Self {
                Self::new("host refusal").extend_with(|_, ext| ext.set("detail", "preserved"))
            }
        }
        let converted: async_graphql::Result<()> = (|| {
            Err(HostRefusal)?;
            Ok(())
        })();
        let converted = converted.unwrap_err();
        assert!(converted.source.is_none());
        let extensions = converted.extensions.clone();
        let public = graphql_error_from_callback(converted);
        assert_eq!(public.message, "host refusal");
        assert_eq!(public.extensions, extensions);

        let sqlx = sqlx_error_from_graphql(async_graphql::Error::from(sqlx::Error::PoolClosed));
        let public = OrmPublicError::from(sqlx);
        assert_eq!(public.code, OrmErrorCode::ServiceUnavailable);
        assert!(public.internal.is_some());
    }

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
                sqlx::Error::Decode(text.into()),
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
        assert!(matches!(sqlx, sqlx::Error::Decode(_)));
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
