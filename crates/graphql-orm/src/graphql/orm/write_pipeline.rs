//! The authorization pipeline of generated GraphQL writes (#724, #504, #921).
//!
//! Generated create, update, upsert, delete, bulk and nested-create resolvers
//! call these functions instead of inlining the sequence. This module is the
//! one place that fixes its order:
//!
//! 1. **Request admission** ([`__admit_generated_write_request`]): resolver
//!    authentication, the PostgreSQL RLS request context, the operation's
//!    declared scopes, then the assurance declared for the operation's own
//!    root field. The authentication mode, the scope declarations and that
//!    coordinate are read from the entity's generated
//!    [`GeneratedWriteConfig`]; the caller names only the operation
//!    ([`GeneratedWriteKind`]) and passes the request's values of the scalar
//!    arguments a scope template names. The backend is the entity's own,
//!    from the same configuration, and the admitted request is typed by it.
//! 2. **Entity admission** ([`GeneratedWriteRequest::__admit_entity`] and its
//!    bulk variant): the request's `Database`, the entity write policy on the
//!    `GraphqlMutation` surface, then the row-policy provider check. It
//!    yields a [`GeneratedWriteAdmission`].
//! 3. **Target admission**: the locking read narrowed by the write-target
//!    prefilter, then the row read/write classification
//!    ([`GeneratedWriteAdmission::__lock_target`] for a single row;
//!    [`bulk_write_targets`](super::bulk_write_targets) for a bulk write), or
//!    the locked lookup of an upsert's conflict key
//!    ([`GeneratedWriteAdmission::__lookup_upsert_target`]) and then the row
//!    write decision on the row it found
//!    ([`GeneratedWriteAdmission::__admit_upsert_target`]). An admission
//!    admits a target once: the attempt is claimed before its first await. A dependent-delete
//!    root admits its target unlocked
//!    ([`GeneratedWriteAdmission::__read_target`]) and its plan admits each
//!    row again under a lock.
//! 4. **Hook dispatch** ([`MutationContext::__run_generated_write_hook`]): a
//!    mutation hook's transaction presents the originating request only when
//!    the caller holds the issued [`GeneratedWriteAuthority`] of this
//!    dispatch, and the entity write decision is asked once more before each
//!    invocation that attaches it.
//!
//! Generated GraphQL upsert and its entity's create mutation share the field
//! policy walk. Their resolver bodies retain the placement of the field
//! checks, input transforms, value binding and statements.
//!
//! # The authority
//!
//! A [`GeneratedWriteAuthority`] has no public constructor: the only way to
//! obtain one is to run steps 1 and 2. It is **issued** only when the
//! admission its operation needs is complete: at entity admission for a
//! create, which has no target, and at target admission (step 3) for an
//! update, a delete, an upsert and a bulk write. A target that is absent or
//! refused, a bulk filter that admits no row, and the unlocked read of a
//! dependent-delete root issue nothing.
//!
//! An authority is bound to what it was earned under:
//!
//! - **one request and one invocation**: the request's query environment,
//!   the root field (by response key and by the position of its selection in
//!   the document) and one invocation of that root field. Each request
//!   admission of a root starts a new invocation, and the transaction its
//!   admission begins carries it. A nested child is admitted through its
//!   parent's admission and inherits the parent's invocation, so the
//!   synthetic child-field contexts of a nested create share it with their
//!   root, and a repeated selection of the same root field does not;
//! - **one operation**: the operation kind, the entity, and the field whose
//!   context was admitted (the root field, or a nested child's synthetic
//!   create field);
//! - **one `Database`**: the handle the entity was admitted on, by an
//!   identity the handle keeps for its lifetime (a clone is another handle),
//!   which must be the handle of the transaction that dispatches the hook;
//! - **one transaction**: the transaction target admission ran in, or for a
//!   create the transaction that carries its invocation;
//! - **the admitted rows**: the key of the row a single-row target admission
//!   locked, the branch an upsert's own lookup decided (a new row that
//!   carries the conflict key it looked up, or the key of the row it found),
//!   or the keys of the rows a bulk target admission
//!   selected. Each row dispatches one `Before` and one `After`, and a new
//!   row its `After` only after its `Before`.
//!
//! Entity admission, target admission, the bulk entry points and hook
//! dispatch **refuse** a request whose admission or authority does not match:
//! the call fails before any hook runs. Generated code always presents the
//! authority of its own admission, so a refusal means the dispatch was not
//! written by the macros.
//!
//! Everything here is macro-internal and hidden from the documentation.
//! These functions are `pub` because generated code compiles in the host's
//! crate, so this is not a seal against host Rust code, which is trusted:
//! host code can still run the whole pipeline itself, in order, for an entity
//! and operation the macros generated. It cannot choose the authentication
//! mode, scope declarations, assurance coordinate or backend of that
//! admission, obtain an authority before the admission is complete, or
//! present one anywhere else.

use super::{
    ChangeAction, DatabaseEntity, DatabaseRls, DbAuthContext, Entity, EntityAccessKind,
    EntityAccessSurface, EntityMetadata, EntityQuery, FromSqlRow, GraphqlOperationKind,
    MutationContext, MutationPhase, OrmBackend, TransactionMode, WriteBackend,
};
use crate::db::Database;
use crate::graphql::auth::ResolverAuthMode;
use crate::graphql::errors::{
    OrmPublicError, graphql_error_from_callback, graphql_error_from_sqlx,
};
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// When a generated create-input field needs a write decision.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub enum GeneratedCreateFieldCheck {
    Never,
    WhenSupplied,
    Always,
}

impl GeneratedCreateFieldCheck {
    fn applies(self, name: &str, supplied: &HashSet<&'static str>) -> bool {
        match self {
            Self::Never => false,
            Self::WhenSupplied => supplied.contains(name),
            Self::Always => true,
        }
    }
}

/// Typed access to a field in the input shared by generated create and upsert.
#[doc(hidden)]
pub struct GeneratedCreateField<I> {
    pub name: &'static str,
    pub policy: Option<&'static str>,
    pub column: &'static str,
    pub conflict_key: bool,
    pub create_check: GeneratedCreateFieldCheck,
    pub update_check: GeneratedCreateFieldCheck,
    pub value: fn(&I) -> &(dyn Any + Send + Sync),
    pub clear: Option<fn(&mut I)>,
}

/// The resolver branch that is about to write the checked input.
#[doc(hidden)]
pub enum GeneratedCreateFieldMode<'a> {
    Create {
        nested: bool,
    },
    Upsert {
        current: Option<&'a (dyn Any + Send + Sync)>,
    },
}

/// Check the fields in declaration order under the resolver's request.
///
/// An ignored denial on the update branch removes the column from both the
/// SQL assignment list and the hook's change list.
#[doc(hidden)]
pub async fn __apply_generated_create_fields<B: OrmBackend, I>(
    db: &Database<B>,
    ctx: &async_graphql::Context<'_>,
    entity: &'static str,
    input: &mut I,
    supplied: &HashSet<&'static str>,
    fields: &[GeneratedCreateField<I>],
    mode: GeneratedCreateFieldMode<'_>,
) -> async_graphql::Result<Vec<&'static str>> {
    let (current, upsert_insert, nested) = match mode {
        GeneratedCreateFieldMode::Create { nested } => (None, false, nested),
        GeneratedCreateFieldMode::Upsert { current } => (current, current.is_none(), false),
    };
    let mut denied_columns = Vec::new();
    for field in fields {
        let check = if current.is_some() {
            field.update_check
        } else {
            field.create_check
        };
        if !check.applies(field.name, supplied) {
            continue;
        }
        let value = Some((field.value)(input));
        // An upsert insert must use the same conflict key as its lookup.
        if let Some(clear) = field
            .clear
            .filter(|_| !(upsert_insert && field.conflict_key))
        {
            let allowed = db
                .check_field_write(ctx, entity, field.name, field.policy, current, value)
                .await
                .map_err(graphql_error_from_callback)?;
            if !allowed {
                if nested {
                    return Err(OrmPublicError::field_write_denied(field.name).into_graphql_error());
                }
                clear(input);
                if current.is_some() {
                    denied_columns.push(field.column);
                }
            }
        } else {
            db.ensure_writable_field(ctx, entity, field.name, field.policy, current, value)
                .await
                .map_err(graphql_error_from_callback)?;
        }
    }
    Ok(denied_columns)
}

/// The text of every binding refusal.
const MISMATCH: &str =
    "generated write admission does not match this request, operation or database";

/// The refusal of a request, admission or authority that does not match the
/// call it is presented to.
fn mismatch() -> async_graphql::Error {
    async_graphql::Error::new(MISMATCH)
}

/// [`mismatch`] on the shared bulk write path, whose errors are repository
/// errors. Clients see an internal error either way.
pub(super) fn mismatch_error() -> sqlx::Error {
    sqlx::Error::Protocol(MISMATCH.to_owned())
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

    /// The action a hook event of an admitted existing row reports.
    fn existing_row_action(self) -> Option<ChangeAction> {
        match self {
            Self::Create => None,
            Self::Update | Self::UpdateMany | Self::Upsert => Some(ChangeAction::Updated),
            Self::Delete | Self::DeleteMany => Some(ChangeAction::Deleted),
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
/// implements it, once; the pipeline reads it instead of taking the backend,
/// authentication mode, scopes, coordinate or row key from its caller.
#[doc(hidden)]
pub trait GeneratedWriteConfig {
    /// The entity's backend. Admission, target admission and an upsert's
    /// lookup all run on it.
    #[doc(hidden)]
    type Backend: OrmBackend;

    /// The operation `kind` as the entity declares it. Upsert returns
    /// `None` unless the entity generates that root.
    #[doc(hidden)]
    fn __gom_write_operation(kind: GeneratedWriteKind) -> Option<GeneratedWriteOperation>;

    /// The id a hook event of this row carries.
    #[doc(hidden)]
    fn __gom_write_key(&self) -> String;
}

/// An upsert's conflict key: each key column with the value an input gives
/// it, as a hook event's changes name and carry them.
type UpsertKey = Vec<(&'static str, super::SqlValue)>;
type UpsertLookup<T> = (Option<T>, UpsertKey);

/// The locked lookup of an upsert's conflict key, on the entity's own
/// backend. `GraphQLOperations` implements it for an entity with a generated
/// upsert, once; target admission runs it instead of trusting a row its
/// caller reports.
#[doc(hidden)]
pub trait GeneratedUpsertLookup: GeneratedWriteConfig + Sized
where
    Self::Backend: WriteBackend,
{
    /// The upsert's input, which carries the conflict key.
    #[doc(hidden)]
    type Input;

    /// Lock and return the row selected in `hook_ctx`, together with the
    /// conflict key actually bound by the lookup.
    #[doc(hidden)]
    fn __gom_upsert_lookup<'a, 'tx: 'a>(
        hook_ctx: &'a mut MutationContext<'tx, Self::Backend>,
        input: &'a Self::Input,
    ) -> futures::future::BoxFuture<'a, crate::Result<UpsertLookup<Self>>>;
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
            if !authority.is_pending_bulk(self, ctx, db) {
                return Err(mismatch_error());
            }
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
    /// Where the root field's selection is in the request document. A
    /// nested child's synthetic field is placed at its root's selection, so
    /// two selections of one root field, each with its own children, differ.
    selection: async_graphql::Pos,
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
            selection: ctx.item.pos,
            field: ctx.field().name().to_owned(),
        }
    }

    /// Whether `ctx` belongs to the same request and root field selection.
    fn shares_root(&self, ctx: &async_graphql::Context<'_>) -> bool {
        std::ptr::eq::<async_graphql::context::QueryEnvInner>(&*self.env, &**ctx.query_env)
            && self.root.as_deref() == root_key(ctx)
            && self.selection == ctx.item.pos
    }

    fn is(&self, ctx: &async_graphql::Context<'_>) -> bool {
        self.shares_root(ctx) && self.field == ctx.field().name()
    }
}

/// One invocation of a root field: what a root's request admission starts,
/// its nested children inherit and the transaction its admission begins
/// carries. Its identity is its allocation, which every admission, authority
/// and transaction of the invocation keeps alive.
#[derive(Default)]
pub(super) struct Invocation {
    /// Whether the root's admission has begun the invocation's transaction.
    begun: AtomicBool,
}

/// What an authority has been issued for.
enum Admitted {
    /// No target admission has been attempted.
    Pending,
    /// A target admission claimed the authority and has not finished.
    Claimed,
    /// An upsert looked its conflict key up in `transaction` and found
    /// `row`; the row write decision is still to be asked.
    Looked {
        transaction: u64,
        key: UpsertKey,
        row: Option<Box<dyn Any + Send + Sync>>,
    },
    /// The target admission admitted nothing, failed or was abandoned, or
    /// the authority was detached before one: it can no longer be issued.
    Inert,
    /// A row the write creates: a create, or an upsert whose key found no
    /// row. `transaction` is the admitting transaction; a create has none
    /// and dispatches in the transaction that carries its invocation. `key`
    /// is the conflict key an upsert looked up: its events must carry it.
    NewRow {
        transaction: Option<u64>,
        key: Option<UpsertKey>,
        phases: u8,
    },
    /// Existing rows admitted in `transaction`, by hook event id, with the
    /// phases each has dispatched.
    Rows {
        transaction: u64,
        rows: HashMap<String, u8>,
    },
}

/// One target admission attempt of an authority, claimed before the
/// attempt's first await. Until it completes, and for good unless it does,
/// no other attempt and no dispatch is accepted.
pub(super) struct Claim<'a> {
    authority: &'a GeneratedWriteAuthority,
    completed: bool,
}

impl Claim<'_> {
    fn complete(mut self, admitted: Admitted) {
        *self.authority.admitted() = admitted;
        self.completed = true;
    }

    /// Target admission of a bulk write succeeded in `transaction` for the
    /// rows with these hook event ids: issue the authority for exactly those
    /// rows. No row issues nothing.
    pub(super) fn __complete_bulk_targets(
        self,
        transaction: u64,
        rows: impl Iterator<Item = String>,
    ) {
        let rows: HashMap<String, u8> = rows.map(|row| (row, 0)).collect();
        if !rows.is_empty() {
            self.complete(Admitted::Rows { transaction, rows });
        }
    }
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        if !self.completed {
            *self.authority.admitted() = Admitted::Inert;
        }
    }
}

const BEFORE: u8 = 1;
const AFTER: u8 = 2;

/// Proof that this pipeline admitted one invocation of one request to run
/// one generated write of one entity on one `Database`, on the
/// `GraphqlMutation` surface. It has no public constructor, and hook
/// dispatch honours it only once it is issued, in the transaction and for
/// the rows it was issued for (see the module documentation).
#[doc(hidden)]
pub struct GeneratedWriteAuthority {
    entity: GeneratedWriteEntity,
    kind: GeneratedWriteKind,
    request: RequestBinding,
    invocation: Arc<Invocation>,
    /// The `Database` handle the entity was admitted on.
    database: crate::db::HandleIdentity,
    admitted: Mutex<Admitted>,
}

impl GeneratedWriteAuthority {
    fn admitted(&self) -> std::sync::MutexGuard<'_, Admitted> {
        self.admitted.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether this authority was earned for `entity` under the request and
    /// field of `ctx`, on `db`.
    fn binds<B: OrmBackend>(
        &self,
        entity: &GeneratedWriteEntity,
        ctx: &async_graphql::Context<'_>,
        db: &Database<B>,
    ) -> bool {
        self.entity.is(entity) && self.request.is(ctx) && self.database == db.handle_identity()
    }

    /// Whether this is the bulk admission of `entity` under the request of
    /// `ctx`, on `db`, before its target admission.
    fn is_pending_bulk<B: OrmBackend>(
        &self,
        entity: &GeneratedWriteEntity,
        ctx: &async_graphql::Context<'_>,
        db: &Database<B>,
    ) -> bool {
        self.kind.is_bulk()
            && self.binds(entity, ctx, db)
            && matches!(*self.admitted(), Admitted::Pending)
    }

    /// Claim the authority for the attempt `accepts` admits, or refuse: only
    /// a state `accepts` takes is claimed, and the check and the claim are
    /// one step, so of two concurrent attempts one is refused.
    fn claim<T>(
        &self,
        accepts: impl FnOnce(Admitted) -> Result<T, Admitted>,
    ) -> Option<(Claim<'_>, T)> {
        let mut admitted = self.admitted();
        match accepts(std::mem::replace(&mut *admitted, Admitted::Claimed)) {
            Ok(taken) => Some((
                Claim {
                    authority: self,
                    completed: false,
                },
                taken,
            )),
            Err(unchanged) => {
                *admitted = unchanged;
                None
            }
        }
    }

    /// [`Self::claim`] of an authority no target admission has attempted.
    fn claim_pending(&self) -> Option<Claim<'_>> {
        self.claim(|admitted| match admitted {
            Admitted::Pending => Ok(()),
            other => Err(other),
        })
        .map(|(claim, ())| claim)
    }

    /// Before a bulk write selects its targets: claim the pending bulk
    /// admission of `T` under the request of `ctx`, on `db`, which must be
    /// the transaction's `Database`.
    pub(super) fn __claim_bulk_targets<T: Entity + 'static, B: OrmBackend>(
        &self,
        ctx: &async_graphql::Context<'_>,
        db: &Database<B>,
        transaction_db: &Database<B>,
    ) -> crate::Result<Claim<'_>> {
        (self.kind.is_bulk()
            && db.handle_identity() == transaction_db.handle_identity()
            && self.binds(&GeneratedWriteEntity::of::<T>(), ctx, db))
        .then(|| self.claim_pending())
        .flatten()
        .ok_or_else(mismatch_error)
    }

    /// Admit one hook dispatch of the row `id`: the authority must be earned
    /// for `entity` under the request and field of `ctx`, on the
    /// transaction's `Database`, and issued in this transaction for that row
    /// and an event that reports `action`; a create must dispatch in the
    /// transaction that carries its invocation, and the row an upsert
    /// creates must carry, in `changes`, the conflict key its lookup used.
    /// Each row dispatches each phase once.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn __require_dispatch<B: OrmBackend>(
        &self,
        entity: &GeneratedWriteEntity,
        ctx: &async_graphql::Context<'_>,
        transaction_db: &Database<B>,
        transaction: u64,
        transaction_invocation: Option<&Arc<Invocation>>,
        phase: &MutationPhase,
        action: ChangeAction,
        id: &str,
        changes: &[super::MutationFieldValue],
    ) -> async_graphql::Result<()> {
        if !self.binds(entity, ctx, transaction_db) {
            return Err(mismatch());
        }
        let phase = match phase {
            MutationPhase::Before => BEFORE,
            MutationPhase::After => AFTER,
        };
        let dispatched = match &mut *self.admitted() {
            Admitted::Pending | Admitted::Claimed | Admitted::Looked { .. } | Admitted::Inert => {
                None
            }
            Admitted::NewRow {
                transaction: admitting,
                key,
                phases,
            } => {
                let admitted = match admitting {
                    Some(admitting) => *admitting == transaction,
                    // A create: the transaction must carry this invocation.
                    None => transaction_invocation
                        .is_some_and(|carried| Arc::ptr_eq(carried, &self.invocation)),
                };
                // The row an upsert creates carries the conflict key its
                // lookup missed on.
                let carries_key = key.as_ref().is_none_or(|key| {
                    key.iter().all(|(column, value)| {
                        let mut values = changes.iter().filter(|change| change.field == *column);
                        values
                            .next()
                            .is_some_and(|change| match (&change.value, value) {
                                (super::SqlValue::Float(actual), super::SqlValue::Float(bound)) => {
                                    actual.to_bits() == bound.to_bits()
                                }
                                (super::SqlValue::Json(actual), super::SqlValue::Json(bound)) => {
                                    // SQLite and SQL Server bind the serialized text.
                                    // JSON equality can hide differences such as -0.0.
                                    matches!(
                                        (serde_json::to_vec(actual), serde_json::to_vec(bound)),
                                        (Ok(actual_bytes), Ok(bound_bytes)) if actual_bytes == bound_bytes
                                    )
                                }
                                (actual, bound) => actual == bound,
                            })
                            && values.next().is_none()
                    })
                });
                // A new row's After follows its Before.
                (action == ChangeAction::Created
                    && admitted
                    && carries_key
                    && (phase == BEFORE || *phases & BEFORE != 0))
                    .then_some(phases)
            }
            Admitted::Rows {
                transaction: admitting,
                rows,
            } => (self.kind.existing_row_action() == Some(action) && *admitting == transaction)
                .then(|| rows.get_mut(id))
                .flatten(),
        }
        .filter(|phases| **phases & phase == 0)
        .map(|phases| *phases |= phase);
        dispatched.ok_or_else(mismatch)
    }
}

/// The request of a generated GraphQL write and the authority of its
/// admission. A repository write passes `None` where one is expected.
#[doc(hidden)]
pub type __GeneratedWriteRequestAuthority<'a, 'ctx> = (
    &'a async_graphql::Context<'ctx>,
    &'a GeneratedWriteAuthority,
);

/// A request that passed request admission for one operation of one entity
/// on the backend `B`. It has no public constructor.
#[doc(hidden)]
#[must_use]
pub struct GeneratedWriteRequest<B: OrmBackend> {
    entity: GeneratedWriteEntity,
    kind: GeneratedWriteKind,
    request: RequestBinding,
    invocation: Arc<Invocation>,
    /// The backend whose RLS request context was checked.
    _backend: PhantomData<fn() -> B>,
}

fn admit_request<T>(
    ctx: &async_graphql::Context<'_>,
    kind: GeneratedWriteKind,
    scope_arguments: &[(&str, &str)],
    invocation: Arc<Invocation>,
) -> async_graphql::Result<GeneratedWriteRequest<T::Backend>>
where
    T: DatabaseRls + Entity + GeneratedWriteConfig + 'static,
{
    let operation = T::__gom_write_operation(kind).ok_or_else(mismatch)?;
    let _auth_subject = crate::graphql::auth::enforce_resolver_auth(ctx, operation.auth_mode)?;
    super::ensure_rls_request_context::<T, T::Backend>(ctx, <T as Entity>::entity_name())?;
    operation.scopes.enforce(ctx, scope_arguments)?;
    // The operation's own coordinate, whatever field is being resolved: a
    // nested child row enforces its entity's create root, a dependent-delete
    // step its entity's delete or update root, and a resolver that is not
    // the operation's root cannot substitute its own requirement.
    crate::graphql::assurance::enforce_operation_assurance(
        ctx,
        GraphqlOperationKind::Mutation,
        operation.field_name,
    )?;
    Ok(GeneratedWriteRequest {
        entity: GeneratedWriteEntity::of::<T>(),
        kind,
        request: RequestBinding::of(ctx),
        invocation,
        _backend: PhantomData,
    })
}

/// Request admission of the generated write `kind` of `T`, on `T`'s own
/// backend, in this order: resolver authentication, the RLS request context
/// of `T`, the operation's declared scopes, then the assurance declared for
/// the operation's own root field. It starts a new invocation.
///
/// The backend, the authentication mode, the scope declarations and the
/// operation's coordinate come from `T`'s [`GeneratedWriteConfig`]. Upsert
/// requires a generated upsert root. The caller supplies `scope_arguments`,
/// the request's values of the scalar arguments a scope template names.
#[doc(hidden)]
pub fn __admit_generated_write_request<T>(
    ctx: &async_graphql::Context<'_>,
    kind: GeneratedWriteKind,
    scope_arguments: &[(&str, &str)],
) -> async_graphql::Result<GeneratedWriteRequest<T::Backend>>
where
    T: DatabaseRls + Entity + GeneratedWriteConfig + 'static,
{
    admit_request::<T>(ctx, kind, scope_arguments, Arc::default())
}

/// [`__admit_generated_write_request`] for a child row of a nested create:
/// the create of `T` under the child's own synthetic field context `ctx`,
/// admitted through the admission of its parent's create. The child inherits
/// the parent's invocation, shares its backend, and must belong to the
/// parent's request and root field.
#[doc(hidden)]
pub fn __admit_generated_nested_create_request<T>(
    ctx: &async_graphql::Context<'_>,
    parent: &GeneratedWriteAdmission<'_, T::Backend>,
) -> async_graphql::Result<GeneratedWriteRequest<T::Backend>>
where
    T: DatabaseRls + Entity + GeneratedWriteConfig + 'static,
{
    let parent = &parent.authority;
    if parent.kind != GeneratedWriteKind::Create || !parent.request.shares_root(ctx) {
        return Err(mismatch());
    }
    admit_request::<T>(
        ctx,
        GeneratedWriteKind::Create,
        &[],
        parent.invocation.clone(),
    )
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

impl<B: OrmBackend> GeneratedWriteRequest<B> {
    async fn admit<'a>(
        self,
        ctx: &async_graphql::Context<'_>,
        db: &'a Database<B>,
    ) -> Result<GeneratedWriteAdmission<'a, B>, EntityRefusal> {
        let Self {
            entity,
            kind,
            request,
            invocation,
            _backend,
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
                invocation,
                database: db.handle_identity(),
                admitted: Mutex::new(if kind == GeneratedWriteKind::Create {
                    // A create has no target: its admission is complete here.
                    Admitted::NewRow {
                        transaction: None,
                        key: None,
                        phases: 0,
                    }
                } else {
                    Admitted::Pending
                }),
            },
        })
    }

    /// Entity admission of a single-row write, on the request's `Database`
    /// of the admitted backend and under the field context request admission
    /// ran under. Every refusal is the root field's error.
    ///
    /// A nested child row is admitted this way under its own synthetic field
    /// context, which carries the `Database` of its parent's request.
    #[doc(hidden)]
    pub async fn __admit_entity<'a>(
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
    pub async fn __admit_entity_bulk<'a>(
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
    /// goes out of scope (a prepared nested child). Target admission needs
    /// the admission, so an authority detached before it can never be
    /// issued.
    #[doc(hidden)]
    pub fn into_authority(self) -> GeneratedWriteAuthority {
        {
            let mut admitted = self.authority.admitted();
            if !matches!(*admitted, Admitted::NewRow { .. } | Admitted::Rows { .. }) {
                *admitted = Admitted::Inert;
            }
        }
        self.authority
    }

    /// Begin the write transaction of this admission's invocation under the
    /// request's database auth context. An invocation begins one, which
    /// carries it: the creates of the invocation, its nested children
    /// included, dispatch in it.
    #[doc(hidden)]
    pub async fn begin(
        &self,
        mode: TransactionMode,
    ) -> async_graphql::Result<MutationContext<'a, B>>
    where
        B: WriteBackend,
    {
        let invocation = &self.authority.invocation;
        if invocation.begun.swap(true, Ordering::AcqRel) {
            return Err(mismatch());
        }
        let mut transaction =
            MutationContext::begin_with_auth(self.db, mode, self.auth_context.as_ref())
                .await
                .map_err(graphql_error_from_sqlx)?;
        transaction.__carry_write_invocation(invocation.clone());
        Ok(transaction)
    }

    /// Whether this is an admission of one of `kinds` for the entity `T`
    /// under the request and field of `ctx`, and `transaction_db`, when
    /// given, is the admitted `Database`.
    fn admits<T: 'static>(
        &self,
        ctx: &async_graphql::Context<'_>,
        transaction_db: Option<&Database<B>>,
        kinds: &[GeneratedWriteKind],
    ) -> bool {
        let authority = &self.authority;
        kinds.contains(&authority.kind)
            && authority.entity.is_type::<T>()
            && authority.request.is(ctx)
            && transaction_db.is_none_or(|db| db.handle_identity() == authority.database)
    }

    /// Claim this admission's one target admission, of one of `kinds` for
    /// the entity `T` under the request and field of `ctx`, in a transaction
    /// on the admitted `Database`; or a refusal. The claim is taken before
    /// the caller's first await.
    fn claim_target<T: 'static>(
        &self,
        ctx: &async_graphql::Context<'_>,
        transaction_db: &Database<B>,
        kinds: &[GeneratedWriteKind],
    ) -> async_graphql::Result<Claim<'_>> {
        self.admits::<T>(ctx, Some(transaction_db), kinds)
            .then(|| self.authority.claim_pending())
            .flatten()
            .ok_or_else(mismatch)
    }

    /// The authority of the existing row `row`, admitted in `transaction`.
    fn row<T: GeneratedWriteConfig>(transaction: u64, row: &T) -> Admitted {
        Admitted::Rows {
            transaction,
            rows: HashMap::from([(row.__gom_write_key(), 0)]),
        }
    }

    /// Target admission of a single-row update or delete: lock the row
    /// `query` selects, narrowed by the write-target prefilter, then classify
    /// it. An admitted row issues the authority for that row in this
    /// transaction. An admission admits one target, once.
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
        T: Entity
            + DatabaseEntity
            + GeneratedWriteConfig<Backend = B>
            + FromSqlRow<B>
            + Clone
            + Send
            + Sync
            + 'static,
    {
        let claim = self.claim_target::<T>(
            ctx,
            hook_ctx.__base_database(),
            &[GeneratedWriteKind::Update, GeneratedWriteKind::Delete],
        )?;
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
        if let Some(row) = &row {
            claim.complete(Self::row(hook_ctx.__identity(), row));
        }
        Ok(row)
    }

    /// [`Self::__lock_target`] without a transaction or a lock, under the
    /// request's database auth context. A dependent-delete root admits its
    /// target this way; its plan locks and admits each row again before
    /// writing. It claims and issues nothing.
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
        if !(self.admits::<T>(
            ctx,
            None,
            &[GeneratedWriteKind::Update, GeneratedWriteKind::Delete],
        ) && matches!(*self.authority.admitted(), Admitted::Pending))
        {
            return Err(mismatch());
        }
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

    /// The first step of an upsert's target admission: the locked lookup of
    /// the conflict key of `input` in `hook_ctx`, which the pipeline runs
    /// itself, through the lookup the entity generates for its own backend.
    /// It returns the row the key found and keeps it for
    /// [`Self::__admit_upsert_target`], which decides the branch from it.
    /// An admission looks its key up once.
    #[doc(hidden)]
    pub async fn __lookup_upsert_target<T>(
        &self,
        ctx: &async_graphql::Context<'_>,
        hook_ctx: &mut MutationContext<'_, B>,
        input: &T::Input,
    ) -> async_graphql::Result<Option<T>>
    where
        B: WriteBackend,
        T: GeneratedUpsertLookup<Backend = B> + Clone + Send + Sync + 'static,
    {
        let claim = self.claim_target::<T>(
            ctx,
            hook_ctx.__base_database(),
            &[GeneratedWriteKind::Upsert],
        )?;
        let transaction = hook_ctx.__identity();
        let (row, key) = T::__gom_upsert_lookup(hook_ctx, input)
            .await
            .map_err(graphql_error_from_sqlx)?;
        claim.complete(Admitted::Looked {
            transaction,
            key,
            row: row
                .clone()
                .map(|row| Box::new(row) as Box<dyn Any + Send + Sync>),
        });
        Ok(row)
    }

    /// The second step of an upsert's target admission: the row write
    /// decision on the row [`Self::__lookup_upsert_target`] found, if any.
    /// Success issues the authority for the branch the lookup decided, in
    /// the lookup's transaction: an update of that row, or, when the key
    /// found no row, the create of a row that carries that key.
    #[doc(hidden)]
    pub async fn __admit_upsert_target<T>(
        &self,
        ctx: &async_graphql::Context<'_>,
    ) -> async_graphql::Result<()>
    where
        T: GeneratedWriteConfig<Backend = B> + Send + Sync + 'static,
    {
        let (claim, (transaction, key, row)) = self
            .admits::<T>(ctx, None, &[GeneratedWriteKind::Upsert])
            .then(|| {
                self.authority.claim(|admitted| match admitted {
                    Admitted::Looked {
                        transaction,
                        key,
                        row,
                    } => Ok((transaction, key, row)),
                    other => Err(other),
                })
            })
            .flatten()
            .ok_or_else(mismatch)?;
        let Some(row) = row else {
            claim.complete(Admitted::NewRow {
                transaction: Some(transaction),
                key: Some(key),
                phases: 0,
            });
            return Ok(());
        };
        let row = row.downcast_ref::<T>().ok_or_else(mismatch)?;
        let entity = &self.authority.entity;
        self.db
            .ensure_writable_row(
                Some(ctx),
                entity.entity_name,
                entity.write_policy(),
                EntityAccessSurface::GraphqlMutation,
                row as &(dyn Any + Send + Sync),
            )
            .await
            .map_err(graphql_error_from_callback)?;
        claim.complete(Self::row(transaction, row));
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
