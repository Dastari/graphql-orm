//! Owned current-authority checks for custom release boundaries.

use std::sync::Arc;

use agql_auth::AuthResult;
use async_graphql::{Context, ErrorExtensions};
use futures::future::BoxFuture;

use super::{AgqlAcceptedRequest, AgqlCurrentAuthorityEnforcement};
use crate::graphql::errors::OrmPublicError;
use crate::graphql::orm::{EntityAccessKind, EntityAccessSurface};

/// Host-selected constants identifying one custom operation.
#[derive(Clone, Copy, Debug)]
pub struct AgqlOperationTarget {
    /// Entity whose policy is being enforced.
    pub entity_name: &'static str,
    /// Declared host policy, when applicable.
    pub policy_key: Option<&'static str>,
    /// Actual access kind.
    pub kind: EntityAccessKind,
    /// Actual execution surface.
    pub surface: EntityAccessSurface,
}

/// Mandatory owned binding and business policy for each custom release.
///
/// Retain verified transport evidence and trusted host dependencies in this
/// implementation; client arguments never establish those proofs. `T` is the
/// host's release input, such as an event/resource identity. Check current owner,
/// tenant, row and field visibility, and deny missing/deleted records unless a
/// separately authorized preimage contract applies. These methods are read-only
/// admission checks: protected effects must follow successful final admission.
/// No callback is rerun within one check, and there is no default allow.
pub trait AgqlOwnedOperationAuthorizer<T: Send + Sync>: Send + Sync {
    /// Enforces the complete original credential's bindings on this release.
    fn authorize_bindings<'a>(
        &'a self,
        accepted: &'a AgqlAcceptedRequest,
        target: AgqlOperationTarget,
        release: &'a T,
    ) -> BoxFuture<'a, AuthResult<bool>>;

    /// Enforces business/resource/row/field policy for this release.
    fn authorize_business<'a>(
        &'a self,
        accepted: &'a AgqlAcceptedRequest,
        target: AgqlOperationTarget,
        release: &'a T,
    ) -> BoxFuture<'a, AuthResult<bool>>;
}

/// Immutable operation-owned authority for custom streams or explicit effects.
///
/// This carries the trusted accepted bundle and policy dependencies, never a
/// borrowed GraphQL Context or a reusable allow permit. Call `check_release`
/// after queue/owner/signer awaits and before every initial/live/gap release.
/// Hosts retain filter, ownership, current-row, deleted-preimage and resource
/// transaction/revision fences. The final owner reads are not an atomic fence
/// against remote revocation after those reads. No data is emitted by this API.
pub struct AgqlOwnedOperation<T: Send + Sync> {
    accepted: AgqlAcceptedRequest,
    enforcement: Arc<AgqlCurrentAuthorityEnforcement>,
    authorizer: Arc<dyn AgqlOwnedOperationAuthorizer<T>>,
    target: AgqlOperationTarget,
}

impl<T: Send + Sync> AgqlOwnedOperation<T> {
    /// Captures coherent identity while a real request context is available.
    ///
    /// Construction does not authorize a release. Supply enforcement and owned
    /// policy from trusted host configuration; the owned policy must retain any
    /// verified binding evidence needed after this context has gone away.
    ///
    /// # Errors
    /// Rejects absent, inconsistent or schema-only accepted request identity.
    pub fn from_context(
        ctx: &Context<'_>,
        enforcement: Arc<AgqlCurrentAuthorityEnforcement>,
        target: AgqlOperationTarget,
        authorizer: Arc<dyn AgqlOwnedOperationAuthorizer<T>>,
    ) -> async_graphql::Result<Self> {
        Ok(Self {
            accepted: AgqlAcceptedRequest::from_context(ctx)?.clone(),
            enforcement,
            authorizer,
            target,
        })
    }

    /// Returns the immutable original bundle; it is never narrowed in place.
    pub fn accepted(&self) -> &AgqlAcceptedRequest {
        &self.accepted
    }

    /// Revalidates original status/current grants after each policy await.
    ///
    /// Every call performs new reads. A denial/error stops before the next
    /// callback; successful callbacks run exactly once. Current narrowing denies
    /// the whole release rather than mutating shared accepted request identity.
    ///
    /// # Errors
    /// Rejects expired/revoked original credentials, narrowed current authority,
    /// missing bindings, denied business policy and unavailable authority reads.
    pub async fn check_release(&self, release: &T) -> async_graphql::Result<()> {
        let deadline = self.enforcement.revalidate(&self.accepted).await?;
        let bindings = self
            .authorizer
            .authorize_bindings(&self.accepted, self.target, release)
            .await
            .map_err(|error| error.extend())?;
        self.enforcement.check_deadline(deadline)?;
        if !bindings {
            return Err(OrmPublicError::forbidden().into_graphql_error());
        }
        let deadline = deadline.min(self.enforcement.revalidate(&self.accepted).await?);
        self.enforcement.check_deadline(deadline)?;
        let business = self
            .authorizer
            .authorize_business(&self.accepted, self.target, release)
            .await
            .map_err(|error| error.extend())?;
        self.enforcement.check_deadline(deadline)?;
        if !business {
            return Err(OrmPublicError::forbidden().into_graphql_error());
        }
        let deadline = deadline.min(self.enforcement.revalidate(&self.accepted).await?);
        self.enforcement.check_deadline(deadline)
    }
}
