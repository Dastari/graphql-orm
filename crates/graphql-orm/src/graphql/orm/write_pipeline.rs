//! The authorization pipeline of generated GraphQL writes (#724, #504, #921).
//!
//! Generated create, update, upsert, delete, bulk and nested-create resolvers
//! call these functions instead of inlining the sequence. This module is the
//! one place that fixes its order:
//!
//! 1. **Request admission** ([`__admit_generated_write_request`]): resolver
//!    authentication, the PostgreSQL RLS request context, the operation's
//!    declared scopes, then the operation's declared assurance. The
//!    authentication mode, the scope requirement and the operation's own
//!    coordinate are read from the entity's generated
//!    [`GeneratedWriteConfig`]; the caller names only the operation
//!    ([`GeneratedWriteKind`]).
//! 2. **Entity admission** ([`GeneratedWriteRequest::__admit_entity`] and its
//!    bulk variant): the request's `Database`, the entity write policy on the
//!    `GraphqlMutation` surface, then the row-policy provider check. It
//!    yields a [`GeneratedWriteAdmission`].
//! 3. **Target admission**: the locking read narrowed by the write-target
//!    prefilter, then the row read/write classification
//!    ([`GeneratedWriteAdmission::__lock_target`] for a single row;
//!    [`bulk_write_targets`](super::bulk_write_targets) for a bulk write), or
//!    the row write decision on the row an upsert found
//!    ([`GeneratedWriteAdmission::__admit_upsert_target`]). A dependent-delete
//!    root admits its target unlocked
//!    ([`GeneratedWriteAdmission::__read_target`]) and its plan admits each
//!    row again under a lock.
//! 4. **Hook dispatch** ([`MutationContext::__run_generated_write_hook`]): a
//!    mutation hook's transaction presents the originating request only when
//!    the caller holds the issued [`GeneratedWriteAuthority`] of this
//!    dispatch, and the entity write decision is asked once more before each
//!    invocation that attaches it.
//!
//! Field write policies, input transforms, value binding and the statements
//! themselves stay in generated code: they are typed per entity field.
//!
//! # The authority
//!
//! A [`GeneratedWriteAuthority`] has no public constructor: the only way to
//! obtain one is to run steps 1 and 2. It is **issued** only when the
//! admission its operation needs is complete: at entity admission for a
//! create, which has no target, and at target admission (step 3) for an
//! update, a delete, an upsert and a bulk write. The unlocked read of a
//! dependent-delete root issues nothing.
//!
//! An authority is bound to what it was earned under:
//!
//! - **one request**: the request's query environment and the root field
//!   (by response key) being resolved. The synthetic child-field contexts of
//!   a nested create share both with their root;
//! - **one operation**: the operation kind, the entity, and the field whose
//!   context was admitted (the root field, or a nested child's synthetic
//!   create field). The authority of a single-row operation covers one row:
//!   one `Before` and one `After` dispatch;
//! - **one `Database`**: the handle the entity was admitted on, which must be
//!   the handle of the transaction that dispatches the hook.
//!
//! Target admission, the bulk entry points and hook dispatch **refuse** a
//! request whose authority does not match: the call fails before any hook
//! runs. Generated code always presents the authority of its own admission,
//! so a refusal means the dispatch was not written by the macros.
//!
//! Everything here is macro-internal and hidden from the documentation.
//! These functions are `pub` because generated code compiles in the host's
//! crate, so this is not a seal against host Rust code, which is trusted:
//! host code can still run the whole pipeline itself, in order, for an entity
//! and operation the macros generated. It cannot choose the authentication
//! mode, scopes or assurance coordinate of that admission, obtain an
//! authority before the admission is complete, or present one anywhere else.

use super::{
    ChangeAction, DatabaseEntity, DatabaseRls, DbAuthContext, Entity, EntityAccessKind,
    EntityAccessSurface, EntityMetadata, EntityQuery, FromSqlRow, GraphqlOperationKind,
    MutationContext, MutationPhase, OrmBackend, TransactionMode, WriteBackend,
};
use crate::db::Database;
use crate::graphql::auth::ResolverAuthMode;
use crate::graphql::errors::{graphql_error_from_callback, graphql_error_from_sqlx};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// The refusal of a request, admission or authority that does not match the
/// call it is presented to.
fn mismatch() -> async_graphql::Error {
    async_graphql::Error::new(
        "generated write admission does not match this request, operation or database",
    )
}

/// A generated write operation of an entity.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeneratedWriteKind {
    Create,
    Update,
    UpdateMany,
    Delete,
    DeleteMany,
    Upsert,
}

impl GeneratedWriteKind {
    /// Whether the operation writes any number of rows.
    fn is_bulk(self) -> bool {
        matches!(self, Self::UpdateMany | Self::DeleteMany)
    }

    /// Whether the operation admits a target before it may dispatch a hook.
    fn admits_target(self) -> bool {
        self != Self::Create
    }

    /// Whether a hook event of this operation may report `action`.
    fn reports(self, action: ChangeAction) -> bool {
        match self {
            Self::Create => action == ChangeAction::Created,
            Self::Update | Self::UpdateMany => action == ChangeAction::Updated,
            Self::Delete | Self::DeleteMany => action == ChangeAction::Deleted,
            Self::Upsert => action != ChangeAction::Deleted,
        }
    }
}

/// The scope requirement an entity declares for one generated write.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub enum GeneratedWriteScopes {
    /// No `operation_authorization` covers the operation.
    None,
    /// Any-of alternatives of all-of scopes.
    Fixed(&'static [&'static [&'static str]]),
    /// Any-of alternatives of all-of scope templates over the operation's
    /// scalar arguments.
    Templates(&'static [&'static [&'static str]]),
}

impl GeneratedWriteScopes {
    fn enforce(
        self,
        ctx: &async_graphql::Context<'_>,
        arguments: &[(&str, &str)],
    ) -> async_graphql::Result<()> {
        match self {
            Self::None => {}
            Self::Fixed(alternatives) => {
                crate::graphql::auth::enforce_resolver_scopes(ctx, alternatives)?;
            }
            Self::Templates(alternatives) => {
                crate::graphql::auth::enforce_resolver_scope_templates(
                    ctx,
                    alternatives,
                    arguments,
                )?;
            }
        }
        Ok(())
    }
}

/// What an entity's declaration fixes for one generated write: the root
/// field, the resolver authentication mode and the scope requirement.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct GeneratedWriteOperation {
    field_name: &'static str,
    auth_mode: Option<ResolverAuthMode>,
    scopes: GeneratedWriteScopes,
}

impl GeneratedWriteOperation {
    #[doc(hidden)]
    pub const fn __new(
        field_name: &'static str,
        auth_mode: Option<ResolverAuthMode>,
        scopes: GeneratedWriteScopes,
    ) -> Self {
        Self {
            field_name,
            auth_mode,
            scopes,
        }
    }
}

/// The generated write configuration of an entity. `GraphQLOperations`
/// implements it; request admission reads it instead of taking the
/// authentication mode, scopes and coordinate from its caller.
#[doc(hidden)]
pub trait GeneratedWriteConfig {
    #[doc(hidden)]
    fn __gom_write_operation(kind: GeneratedWriteKind) -> GeneratedWriteOperation;
}

/// The entity a generated write names in its policy decisions and hook events.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct GeneratedWriteEntity {
    type_id: std::any::TypeId,
    entity_name: &'static str,
    table_name: &'static str,
    metadata: fn() -> &'static EntityMetadata,
}

impl GeneratedWriteEntity {
    /// The entity `T`, as its own declaration names it.
    #[doc(hidden)]
    pub fn of<T: Entity + 'static>() -> Self {
        Self {
            type_id: std::any::TypeId::of::<T>(),
            entity_name: T::entity_name(),
            table_name: <T as DatabaseEntity>::TABLE_NAME,
            metadata: T::metadata,
        }
    }

    pub(super) fn entity_name(&self) -> &'static str {
        self.entity_name
    }

    pub(super) fn table_name(&self) -> &'static str {
        self.table_name
    }

    pub(super) fn metadata(&self) -> &'static EntityMetadata {
        (self.metadata)()
    }

    fn write_policy(&self) -> Option<&'static str> {
        self.metadata().write_policy
    }

    fn is(&self, other: &Self) -> bool {
        self.type_id == other.type_id
    }

    fn is_type<T: 'static>(&self) -> bool {
        self.type_id == std::any::TypeId::of::<T>()
    }

    /// Admit a bulk write that a repository call and a generated GraphQL root
    /// share: the entity write policy on the caller's surface, then the
    /// row-policy provider check. Returns that surface.
    ///
    /// A request is refused unless its authority is the one a bulk admission
    /// of this entity earned under that request on `db`.
    #[doc(hidden)]
    pub async fn __admit_bulk<B: OrmBackend>(
        &self,
        db: &Database<B>,
        request: Option<__GeneratedWriteRequestAuthority<'_, '_>>,
    ) -> crate::Result<EntityAccessSurface> {
        let surface = if let Some((ctx, authority)) = request {
            authority
                .require_bulk(self, ctx, db)
                .map_err(crate::graphql::errors::sqlx_error_from_graphql)?;
            EntityAccessSurface::GraphqlMutation
        } else {
            EntityAccessSurface::Repository
        };
        db.ensure_entity_access(
            request.map(|(ctx, _)| ctx),
            self.entity_name,
            self.write_policy(),
            EntityAccessKind::Write,
            surface,
        )
        .await
        .map_err(crate::graphql::errors::sqlx_error_from_graphql)?;
        db.ensure_row_write_provider(self.entity_name, self.write_policy())
            .map_err(crate::graphql::errors::sqlx_error_from_graphql)?;
        Ok(surface)
    }
}

/// The request, root field and field context an admission ran under.
struct RequestBinding {
    /// Kept alive so its address identifies the request for as long as the
    /// binding exists.
    env: async_graphql::context::QueryEnv,
    /// The response key of the root field being resolved.
    root: Option<String>,
    /// The name of the admitted field: the root field, or the synthetic
    /// create field of a nested child row.
    field: String,
}

fn root_key<'a>(ctx: &'a async_graphql::Context<'_>) -> Option<&'a str> {
    let mut node = ctx.path_node.as_ref()?;
    while let Some(parent) = node.parent {
        node = parent;
    }
    match node.segment {
        async_graphql::QueryPathSegment::Name(name) => Some(name),
        async_graphql::QueryPathSegment::Index(_) => None,
    }
}

impl RequestBinding {
    fn of(ctx: &async_graphql::Context<'_>) -> Self {
        Self {
            env: ctx.query_env.clone(),
            root: root_key(ctx).map(str::to_owned),
            field: ctx.field().name().to_owned(),
        }
    }

    fn is(&self, ctx: &async_graphql::Context<'_>) -> bool {
        std::ptr::eq::<async_graphql::context::QueryEnvInner>(&*self.env, &**ctx.query_env)
            && self.root.as_deref() == root_key(ctx)
            && self.field == ctx.field().name()
    }
}

/// Proof that this pipeline admitted one request to run one generated write
/// of one entity on one `Database`, on the `GraphqlMutation` surface. It has
/// no public constructor, and hook dispatch honours it only once it is
/// issued (see the module documentation).
#[doc(hidden)]
pub struct GeneratedWriteAuthority {
    entity: GeneratedWriteEntity,
    kind: GeneratedWriteKind,
    request: RequestBinding,
    /// The address of the `Database` handle the entity was admitted on. It is
    /// compared, never dereferenced.
    database: usize,
    issued: AtomicBool,
    /// The phases a single-row authority has dispatched.
    dispatched: AtomicU8,
}

fn database_address<B: OrmBackend>(db: &Database<B>) -> usize {
    std::ptr::from_ref(db) as usize
}

impl GeneratedWriteAuthority {
    fn issue(&self) {
        self.issued.store(true, Ordering::Release);
    }

    /// Whether this authority was earned for `entity` under the request and
    /// field of `ctx`, on `db`.
    fn binds<B: OrmBackend>(
        &self,
        entity: &GeneratedWriteEntity,
        ctx: &async_graphql::Context<'_>,
        db: &Database<B>,
    ) -> bool {
        self.entity.is(entity) && self.request.is(ctx) && self.database == database_address(db)
    }

    fn require_bulk<B: OrmBackend>(
        &self,
        entity: &GeneratedWriteEntity,
        ctx: &async_graphql::Context<'_>,
        db: &Database<B>,
    ) -> async_graphql::Result<()> {
        if self.kind.is_bulk() && self.binds(entity, ctx, db) {
            Ok(())
        } else {
            Err(mismatch())
        }
    }

    /// Before a bulk write selects its targets: the authority must be the
    /// bulk admission of `T` under the request of `ctx`, on `db`, which must
    /// be the transaction's `Database`.
    pub(super) fn __require_bulk_targets<T: Entity + 'static, B: OrmBackend>(
        &self,
        ctx: &async_graphql::Context<'_>,
        db: &Database<B>,
        transaction_db: &Database<B>,
    ) -> async_graphql::Result<()> {
        if database_address(db) != database_address(transaction_db) {
            return Err(mismatch());
        }
        self.require_bulk(&GeneratedWriteEntity::of::<T>(), ctx, db)
    }

    /// Target admission of a bulk write succeeded: issue the authority
    /// [`Self::__require_bulk_targets`] accepted.
    pub(super) fn __issue_bulk_targets(&self) {
        self.issue();
    }

    /// Admit one hook dispatch: the authority must be issued, and earned for
    /// `entity` and an operation that reports `action`, under the request and
    /// field of `ctx`, on the transaction's `Database`. A single-row
    /// authority dispatches each phase once.
    pub(super) fn __require_dispatch<B: OrmBackend>(
        &self,
        entity: &GeneratedWriteEntity,
        ctx: &async_graphql::Context<'_>,
        transaction_db: &Database<B>,
        phase: &MutationPhase,
        action: ChangeAction,
    ) -> async_graphql::Result<()> {
        if !(self.issued.load(Ordering::Acquire)
            && self.kind.reports(action)
            && self.binds(entity, ctx, transaction_db))
        {
            return Err(mismatch());
        }
        if !self.kind.is_bulk() {
            let phase = match phase {
                MutationPhase::Before => 1,
                MutationPhase::After => 2,
            };
            if self.dispatched.fetch_or(phase, Ordering::AcqRel) & phase != 0 {
                return Err(mismatch());
            }
        }
        Ok(())
    }
}

/// The request of a generated GraphQL write and the authority of its
/// admission. A repository write passes `None` where one is expected.
#[doc(hidden)]
pub type __GeneratedWriteRequestAuthority<'a, 'ctx> = (
    &'a async_graphql::Context<'ctx>,
    &'a GeneratedWriteAuthority,
);

/// Which assurance requirement a generated write enforces.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub enum GeneratedWriteAssurance {
    /// The requirement declared for the root field being resolved.
    Resolver,
    /// The requirement declared for the operation's own root field, as a
    /// nested child row enforces its entity's create root and a
    /// dependent-delete step its entity's delete or update root.
    Operation,
}

/// A request that passed request admission for one operation of one entity.
/// It has no public constructor.
#[doc(hidden)]
#[must_use]
pub struct GeneratedWriteRequest {
    entity: GeneratedWriteEntity,
    kind: GeneratedWriteKind,
    request: RequestBinding,
}

/// Request admission of the generated write `kind` of `T`, in this order:
/// resolver authentication, the RLS request context of `T`, the operation's
/// declared scopes, then its declared assurance.
///
/// The authentication mode, the scope requirement and the operation's
/// coordinate come from `T`'s [`GeneratedWriteConfig`]. `scope_arguments` are
/// the request's values of the scalar arguments a scope template names.
#[doc(hidden)]
pub fn __admit_generated_write_request<T, B>(
    ctx: &async_graphql::Context<'_>,
    kind: GeneratedWriteKind,
    assurance: GeneratedWriteAssurance,
    scope_arguments: &[(&str, &str)],
) -> async_graphql::Result<GeneratedWriteRequest>
where
    T: DatabaseRls + Entity + GeneratedWriteConfig + 'static,
    B: OrmBackend,
{
    let operation = T::__gom_write_operation(kind);
    let _auth_subject = crate::graphql::auth::enforce_resolver_auth(ctx, operation.auth_mode)?;
    super::ensure_rls_request_context::<T, B>(ctx, <T as Entity>::entity_name())?;
    operation.scopes.enforce(ctx, scope_arguments)?;
    match assurance {
        GeneratedWriteAssurance::Resolver => {
            crate::graphql::assurance::enforce_resolver_assurance(
                ctx,
                GraphqlOperationKind::Mutation,
            )?;
        }
        GeneratedWriteAssurance::Operation => {
            crate::graphql::assurance::enforce_operation_assurance(
                ctx,
                GraphqlOperationKind::Mutation,
                operation.field_name,
            )?;
        }
    }
    Ok(GeneratedWriteRequest {
        entity: GeneratedWriteEntity::of::<T>(),
        kind,
        request: RequestBinding::of(ctx),
    })
}

/// Which step of entity admission refused.
enum EntityRefusal {
    /// The request is not the one request admission ran under.
    Request(async_graphql::Error),
    /// The entity write policy.
    Entity(async_graphql::Error),
    /// The row-policy provider check.
    Provider(async_graphql::Error),
}

impl GeneratedWriteRequest {
    async fn admit<'a, B: OrmBackend>(
        self,
        ctx: &async_graphql::Context<'_>,
        db: &'a Database<B>,
    ) -> Result<GeneratedWriteAdmission<'a, B>, EntityRefusal> {
        let Self {
            entity,
            kind,
            request,
        } = self;
        if !request.is(ctx) {
            return Err(EntityRefusal::Request(mismatch()));
        }
        let auth_context = ctx.data_opt::<DbAuthContext>().cloned();
        db.ensure_entity_access(
            Some(ctx),
            entity.entity_name,
            entity.write_policy(),
            EntityAccessKind::Write,
            EntityAccessSurface::GraphqlMutation,
        )
        .await
        .map_err(EntityRefusal::Entity)?;
        db.ensure_row_write_provider(entity.entity_name, entity.write_policy())
            .map_err(EntityRefusal::Provider)?;
        Ok(GeneratedWriteAdmission {
            db,
            auth_context,
            authority: GeneratedWriteAuthority {
                entity,
                kind,
                request,
                database: database_address(db),
                // A create has no target: its admission is complete here.
                issued: AtomicBool::new(!kind.admits_target()),
                dispatched: AtomicU8::new(0),
            },
        })
    }

    /// Entity admission of a single-row write, on the request's `Database`
    /// and under the field context request admission ran under. Every
    /// refusal is the root field's error.
    ///
    /// A nested child row is admitted this way under its own synthetic field
    /// context, which carries the `Database` of its parent's request.
    #[doc(hidden)]
    pub async fn __admit_entity<'a, B: OrmBackend>(
        self,
        ctx: &async_graphql::Context<'a>,
    ) -> async_graphql::Result<GeneratedWriteAdmission<'a, B>> {
        if self.kind.is_bulk() {
            return Err(mismatch());
        }
        let db = crate::db::request_database::<B>(ctx)?;
        self.admit(ctx, db).await.map_err(|refusal| match refusal {
            EntityRefusal::Request(error) => error,
            EntityRefusal::Entity(error) | EntityRefusal::Provider(error) => {
                graphql_error_from_callback(error)
            }
        })
    }

    /// Entity admission of a bulk write. A policy refusal is the inner error,
    /// which the root answers as a `success: false` payload; a request with
    /// no `Database` is the root field's error.
    #[doc(hidden)]
    pub async fn __admit_entity_bulk<'a, B: OrmBackend>(
        self,
        ctx: &async_graphql::Context<'a>,
    ) -> async_graphql::Result<Result<GeneratedWriteAdmission<'a, B>, async_graphql::Error>> {
        if !self.kind.is_bulk() {
            return Err(mismatch());
        }
        let db = crate::db::request_database::<B>(ctx)?;
        match self.admit(ctx, db).await {
            Ok(admission) => Ok(Ok(admission)),
            Err(EntityRefusal::Request(error)) => Err(error),
            Err(EntityRefusal::Provider(error)) => Ok(Err(error)),
            Err(EntityRefusal::Entity(error)) => Ok(Err(graphql_error_from_callback(error))),
        }
    }
}

/// A request admitted to write one entity: its `Database`, the database auth
/// context of the request, and the authority its hooks may present once it
/// is issued.
#[doc(hidden)]
pub struct GeneratedWriteAdmission<'a, B: OrmBackend> {
    db: &'a Database<B>,
    auth_context: Option<DbAuthContext>,
    authority: GeneratedWriteAuthority,
}

impl<'a, B: OrmBackend> GeneratedWriteAdmission<'a, B> {
    #[doc(hidden)]
    pub fn db(&self) -> &'a Database<B> {
        self.db
    }

    #[doc(hidden)]
    pub fn auth_context(&self) -> Option<&DbAuthContext> {
        self.auth_context.as_ref()
    }

    /// The admission's authority. Hook dispatch refuses it until it is
    /// issued: at once for a create, by target admission otherwise.
    #[doc(hidden)]
    pub fn authority(&self) -> &GeneratedWriteAuthority {
        &self.authority
    }

    /// The authority alone, for a row that is written after its admission
    /// goes out of scope (a prepared nested child).
    #[doc(hidden)]
    pub fn into_authority(self) -> GeneratedWriteAuthority {
        self.authority
    }

    /// Begin the write transaction under the request's database auth context.
    #[doc(hidden)]
    pub async fn begin(
        &self,
        mode: TransactionMode,
    ) -> async_graphql::Result<MutationContext<'a, B>>
    where
        B: WriteBackend,
    {
        MutationContext::begin_with_auth(self.db, mode, self.auth_context.as_ref())
            .await
            .map_err(graphql_error_from_sqlx)
    }

    /// The single-row target admission `kind` runs for the entity `T` under
    /// the request and field of `ctx`, or a refusal.
    fn require_target<T: 'static>(
        &self,
        ctx: &async_graphql::Context<'_>,
        kinds: &[GeneratedWriteKind],
    ) -> async_graphql::Result<()> {
        let authority = &self.authority;
        if kinds.contains(&authority.kind)
            && authority.entity.is_type::<T>()
            && authority.request.is(ctx)
        {
            Ok(())
        } else {
            Err(mismatch())
        }
    }

    /// Target admission of a single-row update or delete: lock the row
    /// `query` selects, narrowed by the write-target prefilter, then classify
    /// it. An admitted row issues the authority.
    ///
    /// `None` means the row is absent, filtered out, or neither readable nor
    /// writable by the caller. The three are indistinguishable to the client.
    #[doc(hidden)]
    pub async fn __lock_target<T>(
        &self,
        ctx: &async_graphql::Context<'_>,
        hook_ctx: &mut MutationContext<'_, B>,
        query: EntityQuery<T, B>,
    ) -> async_graphql::Result<Option<T>>
    where
        B: WriteBackend,
        T: Entity + DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
    {
        self.require_target::<T>(
            ctx,
            &[GeneratedWriteKind::Update, GeneratedWriteKind::Delete],
        )?;
        if database_address(hook_ctx.__base_database()) != self.authority.database {
            return Err(mismatch());
        }
        let row = Box::pin(super::with_write_target_prefilter(
            self.db,
            Some(ctx),
            query,
        ))
        .await
        .map_err(graphql_error_from_callback)?
        .fetch_one_for_write_in_transaction(hook_ctx)
        .await
        .map_err(graphql_error_from_sqlx)?;
        let row = self.classify(ctx, row).await?;
        if row.is_some() {
            self.authority.issue();
        }
        Ok(row)
    }

    /// [`Self::__lock_target`] without a transaction or a lock, under the
    /// request's database auth context. A dependent-delete root admits its
    /// target this way; its plan locks and admits each row again before
    /// writing. It issues no authority.
    #[doc(hidden)]
    pub async fn __read_target<T>(
        &self,
        ctx: &async_graphql::Context<'_>,
        query: EntityQuery<T, B>,
    ) -> async_graphql::Result<Option<T>>
    where
        B: WriteBackend,
        T: Entity + DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
    {
        self.require_target::<T>(
            ctx,
            &[GeneratedWriteKind::Update, GeneratedWriteKind::Delete],
        )?;
        let row = Box::pin(super::with_write_target_prefilter(
            self.db,
            Some(ctx),
            query,
        ))
        .await
        .map_err(graphql_error_from_callback)?
        .fetch_one_with_auth(self.db, self.auth_context.as_ref())
        .await
        .map_err(graphql_error_from_sqlx)?;
        self.classify(ctx, row).await
    }

    /// Target admission of an upsert: the row write decision on the row the
    /// upsert key found in `hook_ctx`, if any. A key with no row has no
    /// target. Either way, success issues the authority.
    #[doc(hidden)]
    pub async fn __admit_upsert_target<T>(
        &self,
        ctx: &async_graphql::Context<'_>,
        hook_ctx: &mut MutationContext<'_, B>,
        existing: Option<&T>,
    ) -> async_graphql::Result<()>
    where
        B: WriteBackend,
        T: Send + Sync + 'static,
    {
        self.require_target::<T>(ctx, &[GeneratedWriteKind::Upsert])?;
        if database_address(hook_ctx.__base_database()) != self.authority.database {
            return Err(mismatch());
        }
        if let Some(existing) = existing {
            let entity = &self.authority.entity;
            self.db
                .ensure_writable_row(
                    Some(ctx),
                    entity.entity_name,
                    entity.write_policy(),
                    EntityAccessSurface::GraphqlMutation,
                    existing as &(dyn std::any::Any + Send + Sync),
                )
                .await
                .map_err(graphql_error_from_callback)?;
        }
        self.authority.issue();
        Ok(())
    }

    async fn classify<T>(
        &self,
        ctx: &async_graphql::Context<'_>,
        row: Option<T>,
    ) -> async_graphql::Result<Option<T>>
    where
        T: Send + Sync + 'static,
    {
        let Some(row) = row else {
            return Ok(None);
        };
        let entity = &self.authority.entity;
        let metadata = entity.metadata();
        let admitted = self
            .db
            .authorize_write_target(
                Some(ctx),
                entity.entity_name,
                metadata.read_policy,
                metadata.write_policy,
                EntityAccessSurface::GraphqlMutation,
                &row as &(dyn std::any::Any + Send + Sync),
            )
            .await
            .map_err(graphql_error_from_callback)?;
        Ok(admitted.then_some(row))
    }
}
