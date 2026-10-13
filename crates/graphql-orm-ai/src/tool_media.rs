//! Typed protected tool-result media and current source-disclosure boundaries.
//! Output references and hashes are descriptive; only the owning resolver/current policy can
//! admit a resource. A retained result never authorizes replay of its original application effect.

#[cfg(any(feature = "sqlite", feature = "postgres"))]
mod boundary;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
mod context;
mod reference;
mod resolved;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub(crate) use boundary::evaluate as evaluate_tool_source_boundaries;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub use boundary::*;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub use context::*;
pub use reference::*;
pub use resolved::*;

#[cfg(any(feature = "sqlite", feature = "postgres"))]
use crate::{AiError, AiToolDescriptor};
#[cfg(any(feature = "sqlite", feature = "postgres"))]
use agql_auth::ResolvedPrincipal;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
use async_trait::async_trait;

/// Trusted owning boundary for already retained output images. It must reopen original protected
/// resource records, verify current actor/tenant/grant and exact original call/content identities,
/// and read bounded protected bytes. It must never execute the tool, accept a URL/storage path,
/// refresh an action deadline, or treat output JSON/hash values as authorization.
#[cfg(any(feature = "sqlite", feature = "postgres"))]
#[async_trait]
pub trait AiToolResultMediaResolver: Send + Sync {
    /// Opens exact retained bytes for current disclosure, under the provided freshly rehydrated
    /// principal and immutable descriptor. Original execution and current disclosure fences differ.
    ///
    /// # Errors
    /// Returns a safe error for cancellation, expired/revoked consent, wrong origin/resource,
    /// unavailable or ambiguous custody, changed bytes, or exceeded time/size/decode bounds.
    async fn resolve_for_provider(
        &self,
        principal: &ResolvedPrincipal,
        descriptor: &AiToolDescriptor,
        context: &AiToolResultDisclosureContext,
        reference: &AiToolImageReference,
    ) -> Result<AiResolvedToolImage, AiError>;
}
