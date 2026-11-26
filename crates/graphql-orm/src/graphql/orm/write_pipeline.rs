//! The authorization pipeline of generated GraphQL writes (#724, #504).
//!
//! Generated create, update, upsert, delete, bulk and nested-create resolvers
//! call these functions instead of inlining the sequence. This module is the
//! one place that fixes its order:
//!
//! 1. **Request admission** ([`__admit_generated_write_request`]): resolver
//!    authentication, the PostgreSQL RLS request context, the operation's
//!    declared scopes, then the operation's declared assurance.
//! 2. **Entity admission** ([`GeneratedWriteRequest::__admit_entity`] and its
//!    bulk and nested variants): the request's `Database`, the entity write
//!    policy on the `GraphqlMutation` surface, then the row-policy provider
//!    check. It yields a [`GeneratedWriteAdmission`].
//! 3. **Target admission** ([`__lock_generated_write_target`] for a single
//!    row; [`bulk_write_targets`](super::bulk_write_targets) for a bulk
//!    write): the locking read narrowed by the write-target prefilter, then
//!    the row read/write classification. A dependent-delete root admits its
//!    target unlocked ([`__read_generated_write_target`]) and its plan admits
//!    each row again under a lock.
//! 4. **Hook dispatch** ([`MutationContext::__run_generated_write_hook`]): a
//!    mutation hook's transaction presents the originating request only when
//!    the caller holds the [`GeneratedWriteAuthority`] of an admission for
//!    the event's entity, and the entity write decision is asked once more
//!    before each invocation that attaches it.
//!
//! Field write policies, input transforms, value binding and the statements
//! themselves stay in generated code: they are typed per entity field.
//!
//! Everything here is macro-internal and hidden from the documentation. A
//! [`GeneratedWriteAuthority`] has no public constructor: the only way to
//! obtain one is to run steps 1 and 2.
//!
//! That is not a seal against host Rust code, which is trusted. These
//! functions are `pub` because generated code compiles in the host's crate.
//! Request admission takes its authentication mode, scopes and assurance
//! coordinate from its caller. The authority is issued before target
//! admission and is not bound to the request, operation or `Database` it was
//! issued under; hook dispatch only checks that it names the event's entity
//! and asks the entity decision again for the request it is given.

use super::{
    DatabaseEntity, DatabaseRls, DbAuthContext, Entity, EntityAccessKind, EntityAccessSurface,
    EntityMetadata, EntityQuery, FromSqlRow, GraphqlOperationKind, MutationContext, OrmBackend,
    TransactionMode, WriteBackend,
};
use crate::db::Database;
use crate::graphql::auth::ResolverAuthMode;
use crate::graphql::errors::{graphql_error_from_callback, graphql_error_from_sqlx};

/// The entity a generated write names in its policy decisions and hook events.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct GeneratedWriteEntity {
    entity_name: &'static str,
    table_name: &'static str,
    metadata: fn() -> &'static EntityMetadata,
}

impl GeneratedWriteEntity {
    #[doc(hidden)]
    pub const fn new(
        entity_name: &'static str,
        table_name: &'static str,
        metadata: fn() -> &'static EntityMetadata,
    ) -> Self {
        Self {
            entity_name,
            table_name,
            metadata,
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

    /// Admit a bulk write that a repository call and a generated GraphQL root
    /// share: the entity write policy on the caller's surface, then the
    /// row-policy provider check. Returns that surface.
    #[doc(hidden)]
    pub async fn __admit_bulk<B: OrmBackend>(
        &self,
        db: &Database<B>,
        request: Option<__GeneratedWriteRequestAuthority<'_, '_>>,
    ) -> crate::Result<EntityAccessSurface> {
        let surface = if request.is_some() {
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

/// Proof that this pipeline admitted a request to write one entity on the
/// `GraphqlMutation` surface. It has no public constructor.
#[doc(hidden)]
pub struct GeneratedWriteAuthority {
    entity: GeneratedWriteEntity,
}

impl GeneratedWriteAuthority {
    /// Whether the admission was for `entity`: the same name and policy key.
    pub(super) fn covers(&self, entity: &GeneratedWriteEntity) -> bool {
        self.entity.entity_name == entity.entity_name
            && self.entity.write_policy() == entity.write_policy()
    }
}

/// The request of a generated GraphQL write and the admission it holds. A
/// repository write passes `None` where one is expected.
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
    /// The requirement declared for a named mutation, as a nested child row
    /// enforces its entity's own create root.
    Operation(&'static str),
}

/// A request that passed request admission. It has no public constructor.
#[doc(hidden)]
#[must_use]
pub struct GeneratedWriteRequest {
    _admitted: (),
}

/// Request admission of a generated write, in this order: resolver
/// authentication, the RLS request context of `T`, the operation's declared
/// scopes (`scopes`), then its declared assurance.
#[doc(hidden)]
pub fn __admit_generated_write_request<T, B>(
    ctx: &async_graphql::Context<'_>,
    auth_mode: Option<ResolverAuthMode>,
    scopes: &dyn Fn() -> async_graphql::Result<()>,
    assurance: GeneratedWriteAssurance,
) -> async_graphql::Result<GeneratedWriteRequest>
where
    T: DatabaseRls + Entity,
    B: OrmBackend,
{
    let _auth_subject = crate::graphql::auth::enforce_resolver_auth(ctx, auth_mode)?;
    super::ensure_rls_request_context::<T, B>(ctx, <T as Entity>::entity_name())?;
    scopes()?;
    match assurance {
        GeneratedWriteAssurance::Resolver => {
            crate::graphql::assurance::enforce_resolver_assurance(
                ctx,
                GraphqlOperationKind::Mutation,
            )?;
        }
        GeneratedWriteAssurance::Operation(field_name) => {
            crate::graphql::assurance::enforce_operation_assurance(
                ctx,
                GraphqlOperationKind::Mutation,
                field_name,
            )?;
        }
    }
    Ok(GeneratedWriteRequest { _admitted: () })
}

/// Which step of entity admission refused.
enum EntityRefusal {
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
        entity: GeneratedWriteEntity,
    ) -> Result<GeneratedWriteAdmission<'a, B>, EntityRefusal> {
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
            authority: GeneratedWriteAuthority { entity },
        })
    }

    /// Entity admission of a single-row write. Every refusal is the root
    /// field's error.
    #[doc(hidden)]
    pub async fn __admit_entity<'a, B: OrmBackend>(
        self,
        ctx: &async_graphql::Context<'a>,
        entity: GeneratedWriteEntity,
    ) -> async_graphql::Result<GeneratedWriteAdmission<'a, B>> {
        let db = crate::db::request_database::<B>(ctx)?;
        self.__admit_entity_on(ctx, db, entity).await
    }

    /// [`Self::__admit_entity`] on a `Database` the caller already resolved:
    /// a nested child row is admitted on its parent's database under its own
    /// field context.
    #[doc(hidden)]
    pub async fn __admit_entity_on<'a, B: OrmBackend>(
        self,
        ctx: &async_graphql::Context<'_>,
        db: &'a Database<B>,
        entity: GeneratedWriteEntity,
    ) -> async_graphql::Result<GeneratedWriteAdmission<'a, B>> {
        self.admit(ctx, db, entity)
            .await
            .map_err(|refusal| match refusal {
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
        entity: GeneratedWriteEntity,
    ) -> async_graphql::Result<Result<GeneratedWriteAdmission<'a, B>, async_graphql::Error>> {
        let db = crate::db::request_database::<B>(ctx)?;
        Ok(self
            .admit(ctx, db, entity)
            .await
            .map_err(|refusal| match refusal {
                EntityRefusal::Provider(error) => error,
                EntityRefusal::Entity(error) => graphql_error_from_callback(error),
            }))
    }
}

/// A request admitted to write one entity: its `Database`, the database auth
/// context of the request, and the authority its hooks may present.
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
}

/// Target admission of a single-row generated write: lock the row `query`
/// selects, narrowed by the write-target prefilter, then classify it.
///
/// `None` means the row is absent, filtered out, or neither readable nor
/// writable by the caller. The three are indistinguishable to the client.
#[doc(hidden)]
pub async fn __lock_generated_write_target<T, B>(
    ctx: &async_graphql::Context<'_>,
    db: &Database<B>,
    hook_ctx: &mut MutationContext<'_, B>,
    entity: GeneratedWriteEntity,
    query: EntityQuery<T, B>,
) -> async_graphql::Result<Option<T>>
where
    B: WriteBackend,
    T: Entity + DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    let row = Box::pin(super::with_write_target_prefilter(db, Some(ctx), query))
        .await
        .map_err(graphql_error_from_callback)?
        .fetch_one_for_write_in_transaction(hook_ctx)
        .await
        .map_err(graphql_error_from_sqlx)?;
    classify_write_target(ctx, db, entity, row).await
}

/// [`__lock_generated_write_target`] without a transaction or a lock, under
/// the request's database auth context. A dependent-delete root admits its
/// target this way; its plan locks and admits each row again before writing.
#[doc(hidden)]
pub async fn __read_generated_write_target<T, B>(
    ctx: &async_graphql::Context<'_>,
    db: &Database<B>,
    auth_context: Option<&DbAuthContext>,
    entity: GeneratedWriteEntity,
    query: EntityQuery<T, B>,
) -> async_graphql::Result<Option<T>>
where
    B: WriteBackend,
    T: Entity + DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    let row = Box::pin(super::with_write_target_prefilter(db, Some(ctx), query))
        .await
        .map_err(graphql_error_from_callback)?
        .fetch_one_with_auth(db, auth_context)
        .await
        .map_err(graphql_error_from_sqlx)?;
    classify_write_target(ctx, db, entity, row).await
}

async fn classify_write_target<T, B>(
    ctx: &async_graphql::Context<'_>,
    db: &Database<B>,
    entity: GeneratedWriteEntity,
    row: Option<T>,
) -> async_graphql::Result<Option<T>>
where
    B: OrmBackend,
    T: Send + Sync + 'static,
{
    let Some(row) = row else {
        return Ok(None);
    };
    let metadata = entity.metadata();
    let admitted = db
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
