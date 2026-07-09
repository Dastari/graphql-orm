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
}

impl fmt::Debug for OrmPublicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OrmPublicError")
            .field("code", &self.code)
            .field("message", &self.message)
            .field("correlation_id", &self.correlation_id)
            .field("internal", &self.internal.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

impl fmt::Display for OrmPublicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for OrmPublicError {}

impl OrmPublicError {
    /// Create a public error with the default safe message for `code`.
    pub fn new(code: OrmErrorCode) -> Self {
        Self {
            code,
            message: code.default_message().to_string(),
            correlation_id: None,
            internal: None,
        }
    }

    /// Create a public error with an explicit safe message.
    pub fn with_message(code: OrmErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            correlation_id: None,
            internal: None,
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
        match error {
            sqlx::Error::RowNotFound => Self::not_found(),
            sqlx::Error::Database(db)
                if db.is_unique_violation() || db.is_foreign_key_violation() =>
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
    fn debug_redacts_internal() {
        let error = OrmPublicError::internal("password=super-secret");
        let debug = format!("{error:?}");
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("super-secret"));
    }
}
