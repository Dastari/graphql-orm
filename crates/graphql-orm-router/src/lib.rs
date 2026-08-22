#![forbid(unsafe_code)]
// The pinned Hive/Ntex service type exceeds rustc 1.90's default query depth
// when the maintained loopback test server is monomorphized.
#![recursion_limit = "256"]
//! Project-neutral federated GraphQL router.
//!
//! The public surface uses router-owned configuration, error, and graph
//! identity types. Federation composition and execution types stay private.

mod admin;
#[cfg(feature = "auth-agql")]
mod agql;
mod auth;
mod config;
mod federation;
mod file_config;
mod jwt;
mod lifecycle;
mod metrics;
mod network;
mod server;
mod subscriptions;

pub use config::{
    AdminConfig, RequestLimits, RouterBuilder, RouterConfig, RouterLogLevel, RouterTelemetryConfig,
    StaticSubgraph, SubscriptionConfig, TrustedSubgraph,
};
pub use file_config::RouterFileConfig;
pub use server::{ActiveGraphIdentity, PreparedRouter, run};

use thiserror::Error;

/// Stable category for a public router failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RouterErrorKind {
    /// Programmatic or file configuration failed validation.
    InvalidConfiguration,
    /// An SDL endpoint could not provide a valid bounded response.
    SchemaFetch,
    /// Federation composition rejected the complete candidate graph.
    Composition,
    /// The federation executor could not construct the candidate runtime.
    Runtime,
    /// Protocol authorization metadata is missing, stale, or ambiguous.
    AuthorizationMetadata,
    /// A dynamic destination violated its outbound network trust policy.
    NetworkPolicy,
    /// Dynamic registration identity, metadata, or admission was rejected.
    Registration,
    /// The public listener or server lifecycle failed.
    Server,
}

/// Router-owned error that does not expose federation-engine types.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct RouterError {
    kind: RouterErrorKind,
    message: String,
}

impl RouterError {
    pub(crate) fn new(kind: RouterErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Returns the stable failure category.
    pub fn kind(&self) -> RouterErrorKind {
        self.kind
    }
}
#[cfg(feature = "auth-agql")]
pub use agql::{AgqlAuthenticationProvider, AgqlScopeMatcher};
pub use auth::{
    AuthenticatedPrincipal, AuthenticationError, AuthenticationErrorKind, AuthenticationProvider,
    ExactScopeMatcher, ScopeMatcher,
};
pub use jwt::{
    AuthenticationClock, JwksAuthenticationConfig, JwksAuthenticationProvider, LegacyScopeClaims,
    RoleScopeCatalogueConfig, SystemAuthenticationClock,
};
pub use lifecycle::{
    RouterHandle, RouterStatus, SchemaRefreshOutcome, SubgraphRuntimeState, SubgraphSourceKind,
    SubgraphStatus,
};
pub use metrics::RouterMetricsSnapshot;
pub use network::{HostResolver, NetworkCidr, NetworkPolicy, SystemHostResolver};
