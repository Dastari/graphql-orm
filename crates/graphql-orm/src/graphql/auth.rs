//! Request principal and authorization-mode helpers for generated resolvers.

use std::fmt;

/// Project-agnostic request principal understood by `graphql-orm`.
///
/// Applications inject this value into an `async-graphql` request with
/// `request.data(subject)`. When only the legacy `String` user id is present,
/// [`AuthExt::auth_subject`] upgrades it to an `AuthSubject` with empty roles,
/// scopes, and optional fields.
///
/// This type must never contain raw tokens, cookies, authorization headers, or
/// other secrets. Optional claim JSON is treated as application-owned metadata
/// and is redacted in [`Debug`] output.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthSubject {
    /// Stable principal subject identifier (user id, service id, or machine id).
    pub id: String,
    /// Optional application user id when distinct from [`Self::id`].
    pub user_id: Option<String>,
    /// Application roles associated with the subject.
    pub roles: Vec<String>,
    /// Application scopes associated with the subject.
    pub scopes: Vec<String>,
    /// Optional tenant identifier for multi-tenant applications.
    pub tenant_id: Option<String>,
    /// Optional safe claim metadata (never tokens or secrets).
    pub claims: Option<serde_json::Value>,
    /// Optional token reference (`jti` or opaque token id), never the raw token.
    pub token_id: Option<String>,
    /// Optional session reference.
    pub session_id: Option<String>,
    /// Optional actor/on-behalf-of identity for delegation.
    pub actor_id: Option<String>,
}

impl fmt::Debug for AuthSubject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthSubject")
            .field("id", &self.id)
            .field("user_id", &self.user_id)
            .field("roles_len", &self.roles.len())
            .field("scopes_len", &self.scopes.len())
            .field("tenant_id", &self.tenant_id)
            .field("claims", &self.claims.as_ref().map(|_| "[redacted]"))
            .field("token_id", &self.token_id)
            .field("session_id", &self.session_id)
            .field("actor_id", &self.actor_id)
            .finish()
    }
}

impl AuthSubject {
    /// Create a subject from an identifier with no roles, scopes, or tenant.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            user_id: None,
            roles: Vec::new(),
            scopes: Vec::new(),
            tenant_id: None,
            claims: None,
            token_id: None,
            session_id: None,
            actor_id: None,
        }
    }

    /// Create a subject from the historical four-field shape.
    ///
    /// Prefer [`Self::builder`] when setting token/session/actor metadata.
    pub fn from_parts(
        id: impl Into<String>,
        roles: Vec<String>,
        scopes: Vec<String>,
        tenant_id: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            user_id: None,
            roles: dedupe_sorted(roles),
            scopes: dedupe_sorted(scopes),
            tenant_id,
            claims: None,
            token_id: None,
            session_id: None,
            actor_id: None,
        }
    }

    /// Start a builder for a fully populated subject.
    pub fn builder(id: impl Into<String>) -> AuthSubjectBuilder {
        AuthSubjectBuilder {
            subject: Self::new(id),
        }
    }

    /// Alias for [`Self::id`] used by bridge/documentation code.
    pub fn subject(&self) -> &str {
        &self.id
    }

    /// Return true when the subject has the exact scope string.
    ///
    /// Scope comparison is case-sensitive.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|candidate| candidate == scope)
    }

    /// Return true when the subject has any exact scope in `scopes`.
    pub fn has_any_scope(&self, scopes: &[&str]) -> bool {
        scopes.iter().any(|scope| self.has_scope(scope))
    }

    /// Return true when the subject has all exact scopes in `scopes`.
    pub fn has_all_scopes(&self, scopes: &[&str]) -> bool {
        scopes.iter().all(|scope| self.has_scope(scope))
    }

    /// Stable partition key for caches and batch loaders.
    ///
    /// Does not include raw claim JSON. Claims contribute only a short
    /// fingerprint so equivalent principals share cache entries without
    /// retaining sensitive claim bodies as key material.
    pub fn canonical_key(&self) -> String {
        let mut roles = self.roles.clone();
        roles.sort();
        let mut scopes = self.scopes.clone();
        scopes.sort();
        let claims_fp = self.claims.as_ref().map(fingerprint_json);
        format!(
            "id={}|user={}|tenant={}|roles={}|scopes={}|token={}|session={}|actor={}|claims_fp={}",
            self.id,
            self.user_id.as_deref().unwrap_or(""),
            self.tenant_id.as_deref().unwrap_or(""),
            roles.join(","),
            scopes.join(","),
            self.token_id.as_deref().unwrap_or(""),
            self.session_id.as_deref().unwrap_or(""),
            self.actor_id.as_deref().unwrap_or(""),
            claims_fp.unwrap_or_default(),
        )
    }
}

/// Builder for [`AuthSubject`].
#[derive(Clone, Debug)]
pub struct AuthSubjectBuilder {
    subject: AuthSubject,
}

impl AuthSubjectBuilder {
    /// Set an application user id distinct from the subject id.
    pub fn user_id(mut self, user_id: impl Into<String>) -> Self {
        self.subject.user_id = Some(user_id.into());
        self
    }

    /// Replace roles (deduplicated deterministically).
    pub fn roles(mut self, roles: Vec<String>) -> Self {
        self.subject.roles = dedupe_sorted(roles);
        self
    }

    /// Replace scopes (deduplicated deterministically).
    pub fn scopes(mut self, scopes: Vec<String>) -> Self {
        self.subject.scopes = dedupe_sorted(scopes);
        self
    }

    /// Set tenant identifier.
    pub fn tenant_id(mut self, tenant_id: impl Into<String>) -> Self {
        self.subject.tenant_id = Some(tenant_id.into());
        self
    }

    /// Attach safe claim metadata.
    pub fn claims(mut self, claims: serde_json::Value) -> Self {
        self.subject.claims = Some(claims);
        self
    }

    /// Set a token reference (never a raw secret token).
    pub fn token_id(mut self, token_id: impl Into<String>) -> Self {
        self.subject.token_id = Some(token_id.into());
        self
    }

    /// Set a session reference.
    pub fn session_id(mut self, session_id: impl Into<String>) -> Self {
        self.subject.session_id = Some(session_id.into());
        self
    }

    /// Set an actor/on-behalf-of identity.
    pub fn actor_id(mut self, actor_id: impl Into<String>) -> Self {
        self.subject.actor_id = Some(actor_id.into());
        self
    }

    /// Finish building the subject.
    pub fn build(self) -> AuthSubject {
        self.subject
    }
}

/// Explicit authorization policy enforcement mode for generated runtime checks.
///
/// # Defaults
///
/// - **Current default:** [`AuthorizationMode::LegacyPermissive`]
/// - **Secure recommended:** [`AuthorizationMode::DeclaredPoliciesRequired`]
/// - **Planned future default (next major):** [`AuthorizationMode::DeclaredPoliciesRequired`]
/// - **Removal timeline for permissive default:** next major after one release of diagnostics
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AuthorizationMode {
    /// Missing providers allow access when no policy decision is available.
    ///
    /// Schema diagnostics still report declared policies without providers so
    /// applications can migrate safely.
    #[default]
    LegacyPermissive,
    /// Entities/fields that declare a policy key require a registered provider.
    ///
    /// Missing providers fail closed for those declared policy keys.
    DeclaredPoliciesRequired,
    /// Every exposed operation requires a registered entity policy decision.
    ///
    /// Operations without a provider are denied.
    ExplicitPolicyForAllExposedOperations,
}

/// Deliberate system/internal access distinct from end-user principals.
///
/// Construct this only for trusted background jobs, migrations, and internal
/// tooling. Do not treat `None` as system authority.
#[derive(Clone, PartialEq, Eq)]
pub struct SystemAccess {
    /// Stable name for audit logs (for example `"backup-worker"`).
    pub name: String,
    /// Optional capability labels granted to this system context.
    pub capabilities: Vec<String>,
}

impl fmt::Debug for SystemAccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemAccess")
            .field("name", &self.name)
            .field("capabilities", &self.capabilities)
            .finish()
    }
}

impl SystemAccess {
    /// Create a named system access context.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            capabilities: Vec::new(),
        }
    }

    /// Attach capability labels.
    pub fn with_capabilities(mut self, capabilities: Vec<String>) -> Self {
        self.capabilities = dedupe_sorted(capabilities);
        self
    }

    /// Return true when `capability` is present.
    pub fn has_capability(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|item| item == capability)
    }
}

/// Explicit repository/database access context.
///
/// Prefer this over optional `None` arguments so system authority is never
/// accidental.
#[derive(Clone, Copy, Debug)]
pub enum AccessContext<'a> {
    /// End-user or service principal mapped into a database auth context.
    Principal(&'a crate::graphql::orm::DbAuthContext),
    /// Deliberate system/internal access.
    System(&'a SystemAccess),
}

impl<'a> AccessContext<'a> {
    /// Return the principal context when this is principal access.
    pub fn principal(self) -> Option<&'a crate::graphql::orm::DbAuthContext> {
        match self {
            Self::Principal(context) => Some(context),
            Self::System(_) => None,
        }
    }

    /// Return the system context when this is system access.
    pub fn system(self) -> Option<&'a SystemAccess> {
        match self {
            Self::Principal(_) => None,
            Self::System(system) => Some(system),
        }
    }

    /// Tenant id for principal access, if present.
    pub fn tenant_id(self) -> Option<&'a str> {
        self.principal()
            .and_then(|context| context.tenant_id.as_deref())
    }
}

/// Authentication helpers for generated resolvers and policy hooks.
pub trait AuthExt {
    /// Deprecated compatibility alias for [`AuthExt::auth_user_id`].
    #[deprecated(note = "use auth_user_id() or auth_subject()")]
    fn auth_user(&self) -> async_graphql::Result<String> {
        self.auth_user_id()
    }

    /// Return the authenticated subject id.
    fn auth_user_id(&self) -> async_graphql::Result<String>;

    /// Return the authenticated subject, upgrading a legacy `String` id when
    /// a full [`AuthSubject`] is not present in the GraphQL context.
    fn auth_subject(&self) -> async_graphql::Result<AuthSubject>;

    /// Return the authenticated subject when present.
    fn auth_subject_opt(&self) -> Option<AuthSubject>;
}

impl AuthExt for async_graphql::Context<'_> {
    fn auth_user_id(&self) -> async_graphql::Result<String> {
        Ok(self.auth_subject()?.id)
    }

    fn auth_subject(&self) -> async_graphql::Result<AuthSubject> {
        self.auth_subject_opt().ok_or_else(|| {
            crate::graphql::errors::OrmPublicError::unauthenticated().into_graphql_error()
        })
    }

    fn auth_subject_opt(&self) -> Option<AuthSubject> {
        self.data_opt::<AuthSubject>()
            .cloned()
            .or_else(|| self.data_opt::<String>().cloned().map(AuthSubject::new))
    }
}

/// Generated resolver authentication mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolverAuthMode {
    /// Require an auth subject before any generated resolver database work.
    Required,
    /// Read the auth subject when one is present and let policies decide.
    Optional,
    /// Do not read auth from the GraphQL context in generated resolvers.
    None,
}

impl ResolverAuthMode {
    /// Backward-compatible default for generated resolver auth enforcement.
    ///
    /// Generated resolvers previously called `ctx.auth_user()?` before
    /// database access. Keeping the default required preserves that fail-closed
    /// behavior unless a schema or entity explicitly opts into `optional` or
    /// `none`.
    pub const DEFAULT: Self = Self::Required;
}

/// Schema-level generated resolver auth configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolverAuthConfig {
    /// Default mode used by generated entity resolvers in the schema.
    pub mode: ResolverAuthMode,
}

impl ResolverAuthConfig {
    /// Create a schema auth configuration.
    pub const fn new(mode: ResolverAuthMode) -> Self {
        Self { mode }
    }
}

/// Apply generated resolver auth enforcement and return the request subject
/// when the selected mode reads auth.
pub fn enforce_resolver_auth(
    ctx: &async_graphql::Context<'_>,
    entity_mode: Option<ResolverAuthMode>,
) -> async_graphql::Result<Option<AuthSubject>> {
    let mode = entity_mode
        .or_else(|| {
            ctx.data_opt::<ResolverAuthConfig>()
                .map(|config| config.mode)
        })
        .unwrap_or(ResolverAuthMode::DEFAULT);

    match mode {
        ResolverAuthMode::Required => ctx.auth_subject().map(Some),
        ResolverAuthMode::Optional => Ok(ctx.auth_subject_opt()),
        ResolverAuthMode::None => Ok(None),
    }
}

fn dedupe_sorted(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

fn fingerprint_json(value: &serde_json::Value) -> String {
    // Stable, non-cryptographic fingerprint for cache partitioning only.
    let encoded = value.to_string();
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in encoded.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_claims() {
        let subject = AuthSubject::builder("user-1")
            .claims(serde_json::json!({"email": "a@example.com"}))
            .token_id("jti-1")
            .build();
        let debug = format!("{subject:?}");
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("a@example.com"));
        assert!(debug.contains("jti-1"));
    }

    #[test]
    fn scope_comparison_is_case_sensitive() {
        let subject = AuthSubject::from_parts("u", vec![], vec!["Tickets.Read".to_string()], None);
        assert!(subject.has_scope("Tickets.Read"));
        assert!(!subject.has_scope("tickets.read"));
    }

    #[test]
    fn roles_and_scopes_are_deduplicated() {
        let subject = AuthSubject::from_parts(
            "u",
            vec!["b".into(), "a".into(), "a".into()],
            vec!["s2".into(), "s1".into(), "s1".into()],
            None,
        );
        assert_eq!(subject.roles, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(subject.scopes, vec!["s1".to_string(), "s2".to_string()]);
    }
}
