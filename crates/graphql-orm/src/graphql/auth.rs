/// Request principal data understood by `graphql-orm`.
///
/// Applications can inject this value directly into an `async-graphql`
/// request with `request.data(subject)`. When only the legacy `String` user id
/// is present, [`AuthExt::auth_subject`] upgrades it to an `AuthSubject` with
/// empty roles, scopes, and tenant id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSubject {
    /// Stable user or machine subject identifier.
    pub id: String,
    /// Application roles associated with the subject.
    pub roles: Vec<String>,
    /// Application scopes associated with the subject.
    pub scopes: Vec<String>,
    /// Optional tenant identifier for multi-tenant applications.
    pub tenant_id: Option<String>,
}

impl AuthSubject {
    /// Create a subject from an identifier with no roles, scopes, or tenant.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            roles: Vec::new(),
            scopes: Vec::new(),
            tenant_id: None,
        }
    }

    /// Create a subject from all fields.
    pub fn from_parts(
        id: impl Into<String>,
        roles: Vec<String>,
        scopes: Vec<String>,
        tenant_id: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            roles,
            scopes,
            tenant_id,
        }
    }

    /// Return true when the subject has the exact scope string.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|candidate| candidate == scope)
    }

    /// Return true when the subject has any exact scope in `scopes`.
    pub fn has_any_scope(&self, scopes: &[&str]) -> bool {
        scopes.iter().any(|scope| self.has_scope(scope))
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
        self.auth_subject_opt()
            .ok_or_else(|| async_graphql::Error::new("missing auth"))
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
