//! Coherent accepted request identity and immediate sensitive-operation checks.

use std::any::{Any, TypeId};
use std::fmt;
use std::sync::{Arc, OnceLock};

use agql_auth::{
    ApiTokenPrincipal, AuthError, AuthPrincipal, AuthResult, AuthUser, Clock,
    CurrentPrincipalResolver, PrincipalReference, ResolvedPrincipal, TokenStatusChecker,
    revalidate_sensitive_principal,
};
use async_graphql::{Context, Data, ErrorExtensions, Guard, Request};
use futures::future::BoxFuture;

use super::auth_bundle_from_principal;
use crate::db::Database;
use crate::graphql::auth::AuthSubject;
use crate::graphql::errors::{OrmErrorCode, OrmPublicError};
use crate::graphql::orm::{
    DbAuthContext, EntityAccessKind, EntityAccessSurface, EntityPolicy, OrmBackend,
};

/// Immutable, request-owned identity accepted by the trusted authenticator.
///
/// Construction does not authenticate a credential. The host first validates
/// its signature/secret, audience, issuer and required transport proof. The full
/// accepted principal retains binding metadata omitted from the safe database
/// projection. No bearer token, cookie or Authorization header is stored here.
/// Never install per-user identity on a globally shared schema.
#[derive(Clone, PartialEq, Eq)]
pub struct AgqlAcceptedRequest {
    principal: AuthPrincipal,
    subject: AuthSubject,
    db_context: DbAuthContext,
}

impl fmt::Debug for AgqlAcceptedRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgqlAcceptedRequest")
            .field("principal", &"[accepted principal]")
            .field("subject", &self.subject)
            .field("db_context", &self.db_context)
            .finish()
    }
}

impl AgqlAcceptedRequest {
    /// Projects an already authenticated principal without altering its grants.
    /// Tenant overrides must not be copied from client arguments into this value.
    pub fn new(principal: AuthPrincipal) -> Self {
        let (subject, db_context) = auth_bundle_from_principal(&principal);
        Self {
            principal,
            subject,
            db_context,
        }
    }

    /// Returns the complete accepted principal, including its token bindings.
    pub fn principal(&self) -> &AuthPrincipal {
        &self.principal
    }
    /// Returns the immutable safe GraphQL projection.
    pub fn subject(&self) -> &AuthSubject {
        &self.subject
    }
    /// Returns the immutable safe database projection.
    pub fn db_context(&self) -> &DbAuthContext {
        &self.db_context
    }

    /// Installs all coherent copies in one HTTP GraphQL request.
    /// Rejects preexisting conflicting copies before changing request data.
    pub fn inject(self, mut request: Request) -> async_graphql::Result<Request> {
        self.insert_into_data(&mut request.data)?;
        Ok(request)
    }

    /// Installs coherent copies in request or connection-init data.
    /// Long-lived connections still require a fresh check for every sensitive event.
    pub fn insert_into_data(self, data: &mut Data) -> async_graphql::Result<()> {
        let conflict = data_value::<Self>(data).is_some_and(|value| value != &self)
            || data_value::<AuthPrincipal>(data).is_some_and(|value| value != &self.principal)
            || data_value::<AuthSubject>(data).is_some_and(|value| value != &self.subject)
            || data_value::<DbAuthContext>(data).is_some_and(|value| value != &self.db_context)
            || match &self.principal {
                AuthPrincipal::User(user) => {
                    data_value::<ApiTokenPrincipal>(data).is_some()
                        || data_value::<AuthUser>(data).is_some_and(|value| value != user)
                }
                AuthPrincipal::ApiToken(token) => {
                    data_value::<AuthUser>(data).is_some()
                        || data_value::<ApiTokenPrincipal>(data).is_some_and(|value| value != token)
                }
            };
        if conflict {
            return Err(forbidden());
        }
        data.insert(self.principal.clone());
        match &self.principal {
            AuthPrincipal::User(user) => data.insert(user.clone()),
            AuthPrincipal::ApiToken(token) => data.insert(token.clone()),
        }
        data.insert(self.subject.clone());
        data.insert(self.db_context.clone());
        data.insert(self);
        Ok(())
    }

    /// Loads and validates an accepted request at an operation boundary.
    ///
    /// Missing copies, conflicting copies (even when shadowed by another data
    /// layer), opposite actor classes, and schema-only identity fail closed.
    pub fn from_context<'a>(ctx: &'a Context<'_>) -> async_graphql::Result<&'a Self> {
        // async-graphql exposes these data layers on Context. Inspect every
        // layer rather than data_opt's first-match view, so a shadowed identity
        // cannot silently disagree with a guard or downstream policy.
        let layers = context_layers(ctx);
        let accepted = layers[..3]
            .iter()
            .flatten()
            .find_map(|data| data_value::<Self>(data))
            .ok_or_else(|| OrmPublicError::unauthenticated().into_graphql_error())?;
        if ctx.data_opt::<AuthPrincipal>() != Some(&accepted.principal)
            || ctx.data_opt::<AuthSubject>() != Some(&accepted.subject)
            || ctx.data_opt::<DbAuthContext>() != Some(&accepted.db_context)
        {
            return Err(forbidden());
        }
        match &accepted.principal {
            AuthPrincipal::User(user) if ctx.data_opt::<AuthUser>() != Some(user) => {
                return Err(forbidden());
            }
            AuthPrincipal::ApiToken(token)
                if ctx.data_opt::<ApiTokenPrincipal>() != Some(token) =>
            {
                return Err(forbidden());
            }
            _ => {}
        }
        for data in layers.into_iter().flatten() {
            if data_value::<Self>(data).is_some_and(|value| value != accepted)
                || data_value::<AuthPrincipal>(data)
                    .is_some_and(|value| value != &accepted.principal)
                || data_value::<AuthSubject>(data).is_some_and(|value| value != &accepted.subject)
                || data_value::<DbAuthContext>(data)
                    .is_some_and(|value| value != &accepted.db_context)
            {
                return Err(forbidden());
            }
            match &accepted.principal {
                AuthPrincipal::User(user) => {
                    if data_value::<ApiTokenPrincipal>(data).is_some()
                        || data_value::<AuthUser>(data).is_some_and(|value| value != user)
                    {
                        return Err(forbidden());
                    }
                }
                AuthPrincipal::ApiToken(token) => {
                    if data_value::<AuthUser>(data).is_some()
                        || data_value::<ApiTokenPrincipal>(data).is_some_and(|value| value != token)
                    {
                        return Err(forbidden());
                    }
                }
            }
        }
        Ok(accepted)
    }
}

fn context_layers<'a>(ctx: &'a Context<'_>) -> [Option<&'a Data>; 4] {
    [
        ctx.execute_data,
        Some(ctx.query_env.query_data.as_ref()),
        Some(ctx.query_env.session_data.as_ref()),
        Some(&ctx.schema_env.data),
    ]
}
fn data_value<T: Any + Send + Sync>(data: &Data) -> Option<&T> {
    data.get(&TypeId::of::<T>())
        .and_then(|value| value.downcast_ref())
}
fn forbidden() -> async_graphql::Error {
    OrmPublicError::forbidden().into_graphql_error()
}
fn expiry_nanos(principal: &AuthPrincipal) -> Option<i128> {
    match principal {
        AuthPrincipal::User(user) => user
            .token_claims
            .expires_at
            .map(|expiry| expiry.unix_timestamp_nanos()),
        AuthPrincipal::ApiToken(token) => Some(token.expires_at.unix_timestamp_nanos()),
    }
}

/// Host-owned authorization of the original credential's operation bindings.
///
/// A callback is mandatory; there is no default allow implementation. Check
/// purpose, audience, resource, exact registered operation, tenant and sender
/// confirmation as applicable, denying when required proof is absent or a
/// restriction cannot be enforced on this surface. User JWT audience/issuer are
/// validated by the authenticator and are not retained on AuthUser; obtain any
/// needed evidence from a trusted transport contract/context, never invented
/// claim metadata. API-token audience remains on its principal.
/// Resource-row restrictions still need corresponding business/row predicates.
pub trait AgqlBindingAuthorizer: Send + Sync {
    /// Authorizes the complete accepted principal for this actual invocation.
    /// Errors use agql-auth's safe public mapping and always deny.
    fn authorize<'a>(
        &'a self,
        ctx: &'a Context<'_>,
        principal: &'a AuthPrincipal,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, AuthResult<bool>>;
}

/// Mandatory online dependencies shared by sensitive policies and custom guards.
/// All reads are performed anew per operation/event; no authorization result is cached.
pub struct AgqlCurrentAuthorityEnforcement {
    resolver: Arc<dyn CurrentPrincipalResolver>,
    checker: Arc<dyn TokenStatusChecker>,
    clock: Arc<dyn Clock>,
    bindings: Arc<dyn AgqlBindingAuthorizer>,
}

impl AgqlCurrentAuthorityEnforcement {
    /// Configures immediate authority/status plus binding enforcement.
    /// An always-active checker does not satisfy the authoritative status contract.
    pub fn new(
        resolver: Arc<dyn CurrentPrincipalResolver>,
        checker: Arc<dyn TokenStatusChecker>,
        clock: Arc<dyn Clock>,
        bindings: Arc<dyn AgqlBindingAuthorizer>,
    ) -> Self {
        Self {
            resolver,
            checker,
            clock,
            bindings,
        }
    }

    /// Checks one custom operation/event. Business, row and field authorization
    /// remain mandatory separately; this method does not grant business access.
    pub async fn enforce(
        &self,
        ctx: &Context<'_>,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> async_graphql::Result<()> {
        self.check(ctx, entity_name, policy_key, kind, surface)
            .await
            .map(|_| ())
    }

    async fn check(
        &self,
        ctx: &Context<'_>,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> async_graphql::Result<i128> {
        let accepted = AgqlAcceptedRequest::from_context(ctx)?;
        let observed = ExpiryObservingResolver {
            inner: self.resolver.as_ref(),
            expiry: OnceLock::new(),
        };
        let current = revalidate_sensitive_principal(
            &accepted.principal,
            &observed,
            self.checker.as_ref(),
            self.clock.now(),
        )
        .await
        .map_err(|error| error.extend())?;
        let deadline = expiry_nanos(&accepted.principal)
            .zip(expiry_nanos(&current))
            .zip(observed.expiry.get().copied().flatten())
            .map(|((accepted, current), resolved)| accepted.min(current).min(resolved))
            .ok_or_else(|| AuthError::AccessTokenExpired.extend())?;
        self.check_deadline(deadline)?;
        let (subject, db_context) = auth_bundle_from_principal(&current);
        if current != accepted.principal
            || subject != accepted.subject
            || db_context != accepted.db_context
        {
            // A parallel field must never observe a partially rewritten context.
            // Require token refresh/reauthentication after grant removal instead.
            return Err(forbidden());
        }
        let authorized = self
            .bindings
            .authorize(ctx, &current, entity_name, policy_key, kind, surface)
            .await
            .map_err(|error| error.extend())?;
        self.check_deadline(deadline)?;
        if !authorized {
            return Err(forbidden());
        }
        Ok(deadline)
    }

    fn check_deadline(&self, deadline: i128) -> async_graphql::Result<()> {
        if deadline <= self.clock.now().unix_timestamp_nanos() {
            return Err(AuthError::AccessTokenExpired.extend());
        }
        Ok(())
    }
}

// Local to one evaluation. Preserve the helper's checker/resolver ordering while
// retaining the authoritative parent's expiry for checks after asynchronous work.
struct ExpiryObservingResolver<'a> {
    inner: &'a dyn CurrentPrincipalResolver,
    expiry: OnceLock<Option<i128>>,
}
#[async_trait::async_trait]
impl CurrentPrincipalResolver for ExpiryObservingResolver<'_> {
    async fn resolve(&self, reference: &PrincipalReference) -> AuthResult<ResolvedPrincipal> {
        let resolved = self.inner.resolve(reference).await?;
        self.expiry
            .set(expiry_nanos(resolved.principal()))
            .map_err(|_| AuthError::AuthServiceUnavailable)?;
        Ok(resolved)
    }
}

/// Wraps every invocation of an inner business entity policy in immediate checks.
/// ctx=None repository convenience calls deny. Custom/raw access remains a trusted
/// host boundary and must use the explicit guard/enforcement plus business policy.
pub struct AgqlCurrentAuthorityPolicy<P> {
    enforcement: Arc<AgqlCurrentAuthorityEnforcement>,
    business: P,
}
impl<P> AgqlCurrentAuthorityPolicy<P> {
    /// Wraps a host business policy without adding an unauthenticated fallback.
    pub fn new(enforcement: Arc<AgqlCurrentAuthorityEnforcement>, business: P) -> Self {
        Self {
            enforcement,
            business,
        }
    }
}
impl<B: OrmBackend, P: EntityPolicy<B>> EntityPolicy<B> for AgqlCurrentAuthorityPolicy<P> {
    fn can_access_entity<'a>(
        &'a self,
        ctx: Option<&'a Context<'_>>,
        db: &'a Database<B>,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            let ctx = ctx.ok_or_else(|| OrmPublicError::unauthenticated().into_graphql_error())?;
            let deadline = self
                .enforcement
                .check(ctx, entity_name, policy_key, kind, surface)
                .await?;
            let allowed = self
                .business
                .can_access_entity(Some(ctx), db, entity_name, policy_key, kind, surface)
                .await?;
            self.enforcement.check_deadline(deadline)?;
            Ok(allowed)
        })
    }
}

/// Custom resolver guard for immediate identity/status/binding enforcement.
/// Install Arc<AgqlCurrentAuthorityEnforcement> in trusted schema configuration.
/// Custom business/row/field authorization remains a separate host requirement.
#[derive(Clone, Copy, Debug)]
pub struct AgqlCurrentAuthorityGuard {
    entity_name: &'static str,
    policy_key: Option<&'static str>,
    kind: EntityAccessKind,
    surface: EntityAccessSurface,
}
impl AgqlCurrentAuthorityGuard {
    /// Creates a guard for the actual custom operation; these are host constants.
    pub fn new(
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> Self {
        Self {
            entity_name,
            policy_key,
            kind,
            surface,
        }
    }
}
impl Guard for AgqlCurrentAuthorityGuard {
    async fn check(&self, ctx: &Context<'_>) -> async_graphql::Result<()> {
        let enforcement = ctx
            .data_opt::<Arc<AgqlCurrentAuthorityEnforcement>>()
            .ok_or_else(|| {
                OrmPublicError::new(OrmErrorCode::AuthorizationMisconfigured).into_graphql_error()
            })?;
        enforcement
            .enforce(
                ctx,
                self.entity_name,
                self.policy_key,
                self.kind,
                self.surface,
            )
            .await
    }
}
