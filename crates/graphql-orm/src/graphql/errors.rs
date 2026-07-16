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
        if let sqlx::Error::Protocol(message) = error {
            if let Some(code) = message.strip_prefix("graphql-orm-public:") {
                let code = match code {
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
                };
                if let Some(code) = code {
                    return Self::new(code);
                }
            }
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
                    || db.is_check_violation() =>
            {
                Self::new(OrmErrorCode::ConstraintViolation).with_internal(error.to_string())
            }
            sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed => {
                Self::new(OrmErrorCode::ServiceUnavailable).with_internal(error.to_string())
            }
            other => Self::internal(other.to_string()),
        }
    }

    /// Convert into an `async-graphql` error with safe extensions only.
    pub fn into_graphql_error(self) -> async_graphql::Error {
        let mut error = async_graphql::Error::new(self.message.clone());
        error = error.extend_with(|_, extensions| {
            extensions.set("code", self.code.as_str());
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

/// Preserve a safe public category through legacy generated SQLx-result paths.
///
/// The protocol payload contains only the stable public code; internal policy
/// diagnostics, identifiers, and protected values are deliberately discarded.
#[doc(hidden)]
pub fn sqlx_error_from_public(error: OrmPublicError) -> sqlx::Error {
    if let Some(internal) = &error.internal {
        tracing_log_internal(&error.code, internal);
    }
    sqlx::Error::Protocol(format!("graphql-orm-public:{}", error.code.as_str()))
}

/// Preserve a safe public category emitted by an internal policy callback.
#[doc(hidden)]
pub fn sqlx_error_from_graphql(error: async_graphql::Error) -> sqlx::Error {
    let code = error
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.get("code"))
        .and_then(|value| match value {
            async_graphql::Value::String(value) => Some(value.as_str()),
            _ => None,
        });
    let public = match code {
        Some("INVALID_INPUT") => OrmPublicError::new(OrmErrorCode::InvalidInput),
        Some("UNAUTHENTICATED") => OrmPublicError::new(OrmErrorCode::Unauthenticated),
        Some("FORBIDDEN") => OrmPublicError::new(OrmErrorCode::Forbidden),
        Some("NOT_FOUND") => OrmPublicError::new(OrmErrorCode::NotFound),
        Some("CONFLICT") => OrmPublicError::new(OrmErrorCode::Conflict),
        Some("CONSTRAINT_VIOLATION") => OrmPublicError::new(OrmErrorCode::ConstraintViolation),
        Some("CURSOR_INVALID") => OrmPublicError::new(OrmErrorCode::CursorInvalid),
        Some("PAGE_LIMIT_EXCEEDED") => OrmPublicError::new(OrmErrorCode::PageLimitExceeded),
        Some("SERVICE_UNAVAILABLE") => OrmPublicError::new(OrmErrorCode::ServiceUnavailable),
        Some("AUTHORIZATION_MISCONFIGURED") => {
            OrmPublicError::new(OrmErrorCode::AuthorizationMisconfigured)
        }
        _ => OrmPublicError::new(OrmErrorCode::InternalError),
    };
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
        assert!(
            sqlx.to_string()
                .contains("graphql-orm-public:AUTHORIZATION_MISCONFIGURED")
        );
        assert!(!sqlx.to_string().contains("secret policy"));
        let public = OrmPublicError::from(sqlx);
        assert_eq!(public.code, OrmErrorCode::AuthorizationMisconfigured);
        assert!(!format!("{public:?}").contains("secret policy"));

        let graphql = OrmPublicError::forbidden().into_graphql_error();
        let public = OrmPublicError::from(sqlx_error_from_graphql(graphql));
        assert_eq!(public.code, OrmErrorCode::Forbidden);
    }

    #[test]
    fn debug_redacts_internal() {
        let error = OrmPublicError::internal("password=super-secret");
        let debug = format!("{error:?}");
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("super-secret"));
    }
}
