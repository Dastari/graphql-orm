//! Opt-in, authorized dynamic GraphQL registration for owned runtime schemas.
//!
//! The host owns authority and cursor encryption. This module does not publish
//! schemas, persist catalogs, own transport, or alter static GraphQL behavior.

mod cursor;
mod scalar;
pub use cursor::*;
pub use scalar::RuntimeScalarLimits;

use std::fmt;

/// A bounded, value-free dynamic GraphQL error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeGraphqlError {
    code: &'static str,
}
impl RuntimeGraphqlError {
    /// Stable code; no identifiers, query text, cursor or caller values.
    pub const fn code(self) -> &'static str {
        self.code
    }
    /// Explicit host denial without exposing policy internals.
    pub const fn denied() -> Self {
        Self {
            code: "authorization_denied",
        }
    }
    pub(crate) const fn new(code: &'static str) -> Self {
        Self { code }
    }
}
impl fmt::Display for RuntimeGraphqlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code)
    }
}
impl std::error::Error for RuntimeGraphqlError {}
mod execution;
mod module;
mod plan;
mod schema;
pub use execution::{
    RuntimeGraphqlRequest, RuntimeReadAuthority, RuntimeReadCheck, RuntimeReadGrant,
};
pub use module::*;
