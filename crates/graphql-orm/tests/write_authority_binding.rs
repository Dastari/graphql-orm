//! A generated write's authority is derived from the entity's generated
//! configuration, issued only once the admission its operation needs is
//! complete, and bound to the request, invocation, operation, `Database`,
//! transaction and rows it was earned for (#921).
//!
//! Host Rust code is trusted and can run the macro-internal write pipeline
//! itself, as the resolvers below do. Every step that does not match the
//! admission it presents is refused before a hook runs.

#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::async_graphql::{Context, ErrorExtensions, Object, Value};
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::graphql::errors::{OrmPublicError, graphql_error_from_sqlx};
use graphql_orm::graphql::input_validation::__nested_create_field;
use graphql_orm::graphql::orm::{
    __admit_generated_nested_create_request, __admit_generated_write_request, EntityQuery,
    GeneratedWriteAuthority, GeneratedWriteEntity, GeneratedWriteKind, GeneratedWriteRequest,
    SqlValue, bulk_write_targets,
};
use graphql_orm::prelude::*;
use std::sync::{Arc, Mutex};

type Sqlite = graphql_orm::SqliteBackend;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "sqlite",
    table = "bind_orders",
    plural = "BindOrders",
    auth = "required"
)]
#[graphql_orm(operation_authorization(categories = ["delete"], all_scopes = ["orders.delete"]))]
#[graphql_orm(operation_authorization(
    categories = ["update"],
    all_scope_templates = ["orders.{id}.update"]
))]
struct BindOrder {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    label: String,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "sqlite",
    table = "bind_lines",
    plural = "BindLines",
    auth = "optional"
)]
struct BindLine {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    order_id: String,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "sqlite",
    table = "bind_devices",
    plural = "BindDevices",
    auth = "optional",
    upsert = "mac"
)]
struct BindDevice {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    #[unique]
    #[filterable(type = "string")]
    mac: String,
    name: String,
}

fn device(id: &str, mac: &str) -> CreateBindDeviceInput {
    CreateBindDeviceInput {
        id: id.to_owned(),
        mac: mac.to_owned(),
        name: "probe".to_owned(),
    }
}

/// A row policy that admits every row, after waiting at the gate while it is
/// closed. It counts the decisions it was asked.
#[derive(Default)]
struct Gate {
    closed: std::sync::atomic::AtomicBool,
    asked: std::sync::atomic::AtomicUsize,
    open: tokio::sync::Notify,
}

impl Gate {
    fn close(&self) {
        self.asked.store(0, std::sync::atomic::Ordering::SeqCst);
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn open(&self) {
        self.closed
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.open.notify_waiters();
    }

    fn asked(&self) -> usize {
        self.asked.load(std::sync::atomic::Ordering::SeqCst)
    }

    async fn pass(&self) -> async_graphql::Result<bool> {
        self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        while self.closed.load(std::sync::atomic::Ordering::SeqCst) {
            let opened = self.open.notified();
            if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
                opened.await;
            }
        }
        Ok(true)
    }
}

struct GatedRows(Arc<Gate>);
impl RowPolicy for GatedRows {
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(self.0.pass())
    }

    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(self.0.pass())
    }
}

/// The text of every binding refusal.
const REFUSED: &str =
    "generated write admission does not match this request, operation or database";

fn outcome<T>(result: graphql_orm::async_graphql::Result<T>) -> String {
    match result {
        Ok(_) => "ok".to_owned(),
        Err(error) if error.message == REFUSED => "refused".to_owned(),
        Err(error) => format!("error: {}", error.message),
    }
}

/// [`outcome`] of a step on the shared bulk write path, whose refusals are
/// repository errors: the binding refusal is the protocol error with that
/// text, which clients see as an internal error.
fn bulk_outcome<T>(result: graphql_orm::Result<T>) -> String {
    match result {
        Ok(_) => "ok".to_owned(),
        Err(graphql_orm::sqlx::Error::Protocol(text)) if text == REFUSED => {
            let public = OrmPublicError::from_sqlx(&graphql_orm::sqlx::Error::Protocol(text));
            assert!(!public.message.contains(REFUSED), "{public:?}");
            "refused".to_owned()
        }
        Err(error) => format!("error: {:?}", OrmPublicError::from_sqlx(&error).code),
    }
}

/// Request admission of `kind` for `T` under `ctx`, with no scope arguments.
fn admit<T>(
    ctx: &Context<'_>,
    kind: GeneratedWriteKind,
) -> graphql_orm::async_graphql::Result<GeneratedWriteRequest<Sqlite>>
where
    T: DatabaseRls
        + Entity
        + graphql_orm::graphql::orm::GeneratedWriteConfig<Backend = Sqlite>
        + 'static,
{
    __admit_generated_write_request::<T>(ctx, kind, &[])
}

/// One `Before` or `After` dispatch of the row `id` of `entity`, presenting
/// `authority`, with no changes.
async fn dispatch(
    tx: &mut MutationContext<'_>,
    entity: GeneratedWriteEntity,
    ctx: &Context<'_>,
    authority: &GeneratedWriteAuthority,
    phase: MutationPhase,
    action: ChangeAction,
    id: &str,
) -> String {
    dispatch_changes(tx, entity, ctx, authority, phase, action, id, &[]).await
}

/// [`dispatch`] of an event whose changes give these columns these values.
#[allow(clippy::too_many_arguments)]
async fn dispatch_changes(
    tx: &mut MutationContext<'_>,
    entity: GeneratedWriteEntity,
    ctx: &Context<'_>,
    authority: &GeneratedWriteAuthority,
    phase: MutationPhase,
    action: ChangeAction,
    id: &str,
    changes: &[(&str, &str)],
) -> String {
    outcome(
        tx.__run_generated_write_hook(
            &entity,
            Some((ctx, authority)),
            phase,
            action,
            id.to_owned(),
            changes
                .iter()
                .map(|(field, value)| MutationFieldValue {
                    field: (*field).to_owned(),
                    value: SqlValue::String((*value).to_owned()),
                })
                .collect(),
            None,
            None,
        )
        .await,
    )
}

fn by_id<T>(id: &str) -> EntityQuery<T, Sqlite>
where
    T: DatabaseEntity + graphql_orm::graphql::orm::FromSqlRow + Clone + Send + Sync + 'static,
{
    EntityQuery::<T, Sqlite>::new().where_clause("id = ?", SqlValue::String(id.to_owned()))
}

fn lines_of(order: &str) -> EntityQuery<BindLine, Sqlite> {
    EntityQuery::<BindLine, Sqlite>::new()
        .where_clause("order_id = ?", SqlValue::String(order.to_owned()))
}

fn line_key(line: &BindLine) -> SqlValue {
    SqlValue::String(line.id.clone())
}

/// A transaction that no admission began.
async fn begin<'a>(db: &'a Database) -> graphql_orm::async_graphql::Result<MutationContext<'a>> {
    MutationContext::begin(db, TransactionMode::Default)
        .await
        .map_err(graphql_error_from_sqlx)
}

async fn rollback(tx: MutationContext<'_>) -> graphql_orm::async_graphql::Result<()> {
    tx.rollback().await.map_err(graphql_error_from_sqlx)
}

/// An authority one invocation leaves for another to present.
#[derive(Default)]
struct Stash(Mutex<Option<GeneratedWriteAuthority>>);

/// What the invocations of a repeated selection did, in order.
#[derive(Clone, Default)]
struct Repeats(Arc<Mutex<Vec<String>>>);

/// Host code that drives the write pipeline itself, as a custom resolver can.
#[derive(Default)]
struct HostMutation;
#[graphql_orm_root_fields]
#[Object]
impl HostMutation {
    /// Run the steps of `case` and report each as `ok`, `refused` (the
    /// binding refusal) or another error. Nothing is committed.
    async fn probe(
        &self,
        ctx: &Context<'_>,
        case: String,
    ) -> graphql_orm::async_graphql::Result<Vec<String>> {
        use ChangeAction::{Created, Deleted, Updated};
        use GeneratedWriteKind::{Create, Delete, DeleteMany, Update, Upsert};
        type Devices = BindDevice;
        use MutationPhase::{After, Before};

        let db = ctx.data::<Database>()?;
        let orders = GeneratedWriteEntity::of::<BindOrder>();
        let lines = GeneratedWriteEntity::of::<BindLine>();
        let devices = GeneratedWriteEntity::of::<BindDevice>();
        let mut out = Vec::new();
        match case.as_str() {
            // The scopes and the authentication mode are the entity's.
            "config" => {
                out.push(outcome(admit::<BindOrder>(ctx, Delete)));
                for id in ["o1", "o2"] {
                    out.push(outcome(__admit_generated_write_request::<BindOrder>(
                        ctx,
                        Update,
                        &[("id", id)],
                    )));
                }
                out.push(outcome(admit::<BindOrder>(ctx, Create)));
                out.push(outcome(admit::<BindLine>(ctx, Create)));
                // Lines generate no upsert root: there is none to admit, and
                // so no lookup a caller could supply for one.
                out.push(outcome(admit::<BindLine>(ctx, Upsert)));
            }
            // The assurance coordinate is the operation's, not this field's.
            "assurance" => {
                out.push(outcome(admit::<BindOrder>(ctx, Delete)));
                out.push(outcome(admit::<BindLine>(ctx, Create)));
            }
            // Nothing is issued before target admission succeeds, and then
            // only for the admitted row in the admitting transaction.
            "target" => {
                let admission = admit::<BindOrder>(ctx, Delete)?.__admit_entity(ctx).await?;
                let authority = admission.authority();
                // The unlocked read of a dependent-delete root issues nothing.
                let read = admission
                    .__read_target(ctx, by_id::<BindOrder>("o1"))
                    .await?;
                assert!(read.is_some());
                let mut tx = admission.begin(TransactionMode::Default).await?;
                out.push(dispatch(&mut tx, orders, ctx, authority, Before, Deleted, "o1").await);
                let absent = admission
                    .__lock_target(ctx, &mut tx, by_id::<BindOrder>("absent"))
                    .await?;
                assert!(absent.is_none());
                out.push(dispatch(&mut tx, orders, ctx, authority, Before, Deleted, "o1").await);
                // One target admission per admission, whatever it found.
                out.push(outcome(
                    admission
                        .__lock_target(ctx, &mut tx, by_id::<BindOrder>("o1"))
                        .await,
                ));
                rollback(tx).await?;
                let admission = admit::<BindOrder>(ctx, Delete)?.__admit_entity(ctx).await?;
                let authority = admission.authority();
                let mut tx = admission.begin(TransactionMode::Default).await?;
                let locked = admission
                    .__lock_target(ctx, &mut tx, by_id::<BindOrder>("o1"))
                    .await?;
                assert!(locked.is_some());
                // A different operation: a delete admission reports no update.
                out.push(dispatch(&mut tx, orders, ctx, authority, Before, Updated, "o1").await);
                // Another row.
                out.push(dispatch(&mut tx, orders, ctx, authority, Before, Deleted, "o2").await);
                out.push(dispatch(&mut tx, orders, ctx, authority, Before, Deleted, "o1").await);
                // Each phase once, and `After` for the admitted row only.
                out.push(dispatch(&mut tx, orders, ctx, authority, Before, Deleted, "o1").await);
                out.push(dispatch(&mut tx, orders, ctx, authority, After, Deleted, "o2").await);
                out.push(dispatch(&mut tx, orders, ctx, authority, After, Deleted, "o1").await);
                out.push(dispatch(&mut tx, orders, ctx, authority, After, Deleted, "o1").await);
                // One target per admission, and one transaction per invocation.
                out.push(outcome(
                    admission
                        .__lock_target(ctx, &mut tx, by_id::<BindOrder>("o2"))
                        .await,
                ));
                rollback(tx).await?;
                out.push(outcome(
                    admission.begin(TransactionMode::Default).await.map(drop),
                ));
                // A row locked in one transaction, dispatched in a fresh one
                // on the same handle.
                let admission = admit::<BindOrder>(ctx, Delete)?.__admit_entity(ctx).await?;
                let mut tx = admission.begin(TransactionMode::Default).await?;
                let locked = admission
                    .__lock_target(ctx, &mut tx, by_id::<BindOrder>("o1"))
                    .await?;
                assert!(locked.is_some());
                rollback(tx).await?;
                let mut tx = begin(db).await?;
                out.push(
                    dispatch(
                        &mut tx,
                        orders,
                        ctx,
                        admission.authority(),
                        Before,
                        Deleted,
                        "o1",
                    )
                    .await,
                );
                // An authority detached before target admission stays inert.
                let detached = admit::<BindOrder>(ctx, Delete)?
                    .__admit_entity(ctx)
                    .await?
                    .into_authority();
                out.push(dispatch(&mut tx, orders, ctx, &detached, Before, Deleted, "o1").await);
                rollback(tx).await?;
            }
            // An upsert's authority is the branch the pipeline's own lookup
            // decided: the caller reports no row.
            "upsert" => {
                // The key selects an existing row, whatever the caller thinks.
                let admission = admit::<Devices>(ctx, Upsert)?.__admit_entity(ctx).await?;
                let authority = admission.authority();
                let mut tx = admission.begin(TransactionMode::Default).await?;
                out.push(dispatch(&mut tx, devices, ctx, authority, Before, Created, "d9").await);
                // No decision before the lookup.
                out.push(outcome(
                    admission.__admit_upsert_target::<Devices>(ctx).await,
                ));
                let found = admission
                    .__lookup_upsert_target::<Devices>(ctx, &mut tx, &device("d9", "m1"))
                    .await?;
                assert_eq!(found.map(|row| row.id).as_deref(), Some("d1"));
                // Looked up, not yet admitted; and looked up once.
                out.push(dispatch(&mut tx, devices, ctx, authority, Before, Updated, "d1").await);
                out.push(outcome(
                    admission
                        .__lookup_upsert_target::<Devices>(ctx, &mut tx, &device("d9", "m9"))
                        .await,
                ));
                out.push(outcome(
                    admission.__admit_upsert_target::<Devices>(ctx).await,
                ));
                // An update of the row that was found, and nothing else.
                out.push(dispatch(&mut tx, devices, ctx, authority, Before, Created, "d9").await);
                out.push(dispatch(&mut tx, devices, ctx, authority, Before, Updated, "d2").await);
                out.push(dispatch(&mut tx, devices, ctx, authority, Before, Updated, "d1").await);
                // Decided once.
                out.push(outcome(
                    admission.__admit_upsert_target::<Devices>(ctx).await,
                ));
                rollback(tx).await?;
                // The key selects no row: a create, and nothing else.
                let admission = admit::<Devices>(ctx, Upsert)?.__admit_entity(ctx).await?;
                let authority = admission.authority();
                let mut tx = admission.begin(TransactionMode::Default).await?;
                let found = admission
                    .__lookup_upsert_target::<Devices>(ctx, &mut tx, &device("d9", "m9"))
                    .await?;
                assert!(found.is_none());
                out.push(outcome(
                    admission.__admit_upsert_target::<Devices>(ctx).await,
                ));
                out.push(dispatch(&mut tx, devices, ctx, authority, Before, Updated, "d1").await);
                let (looked_up, other) = ([("mac", "m9")], [("mac", "m7"), ("name", "m9")]);
                let mut created = async |phase, changes: &[(&str, &str)]| {
                    dispatch_changes(
                        &mut tx, devices, ctx, authority, phase, Created, "d9", changes,
                    )
                    .await
                };
                out.push(created(After, &looked_up).await);
                // The created row must carry the conflict key that was
                // looked up: not none, and not another device's.
                out.push(created(Before, &[]).await);
                out.push(created(Before, &other).await);
                out.push(created(Before, &looked_up).await);
                out.push(created(After, &other).await);
                out.push(created(After, &looked_up).await);
                rollback(tx).await?;
            }
            // Two callers share an admission: one target admission wins,
            // before either finishes its policy decision.
            "race" => {
                let gate = ctx.data::<Arc<Gate>>()?;
                // Upsert: both ask for the decision on the looked-up row.
                let admission = admit::<Devices>(ctx, Upsert)?.__admit_entity(ctx).await?;
                let mut tx = admission.begin(TransactionMode::Default).await?;
                let found = admission
                    .__lookup_upsert_target::<Devices>(ctx, &mut tx, &device("d9", "m1"))
                    .await?;
                assert!(found.is_some());
                gate.close();
                let (first, second, ()) = graphql_orm::futures::join!(
                    admission.__admit_upsert_target::<Devices>(ctx),
                    admission.__admit_upsert_target::<Devices>(ctx),
                    async { gate.open() },
                );
                out.push(outcome(first));
                out.push(outcome(second));
                out.push(format!("asked {}", gate.asked()));
                out.push(
                    dispatch(
                        &mut tx,
                        devices,
                        ctx,
                        admission.authority(),
                        Before,
                        Updated,
                        "d1",
                    )
                    .await,
                );
                rollback(tx).await?;
                // Single row: the lock waits on the row decision; a second
                // target admission of the same admission arrives meanwhile.
                let admission = admit::<BindOrder>(ctx, Delete)?.__admit_entity(ctx).await?;
                let mut tx = admission.begin(TransactionMode::Default).await?;
                gate.close();
                let (first, second, ()) = graphql_orm::futures::join!(
                    admission.__lock_target(ctx, &mut tx, by_id::<BindOrder>("o1")),
                    admission.__read_target(ctx, by_id::<BindOrder>("o2")),
                    async { gate.open() },
                );
                out.push(outcome(first));
                out.push(outcome(second));
                out.push(
                    dispatch(
                        &mut tx,
                        orders,
                        ctx,
                        admission.authority(),
                        Before,
                        Deleted,
                        "o1",
                    )
                    .await,
                );
                out.push(
                    dispatch(
                        &mut tx,
                        orders,
                        ctx,
                        admission.authority(),
                        Before,
                        Deleted,
                        "o2",
                    )
                    .await,
                );
                // Bulk: target selection waits on a row decision; the bulk
                // admission of the same authority arrives meanwhile.
                let bulk = admit::<BindLine>(ctx, DeleteMany)?
                    .__admit_entity_bulk(ctx)
                    .await??;
                let authority = bulk.authority();
                gate.close();
                let (first, second, ()) = graphql_orm::futures::join!(
                    bulk_write_targets(
                        db,
                        Some((ctx, authority)),
                        &mut tx,
                        lines_of("o1"),
                        line_key,
                    ),
                    lines.__admit_bulk(db, Some((ctx, authority))),
                    async { gate.open() },
                );
                out.push(bulk_outcome(first));
                out.push(bulk_outcome(second));
                out.push(dispatch(&mut tx, lines, ctx, authority, Before, Deleted, "l1").await);
                rollback(tx).await?;
            }
            // A different entity.
            "entity" => {
                let admission = admit::<BindLine>(ctx, Create)?.__admit_entity(ctx).await?;
                let authority = admission.authority();
                let mut tx = admission.begin(TransactionMode::Default).await?;
                out.push(dispatch(&mut tx, orders, ctx, authority, Before, Created, "x").await);
                out.push(dispatch(&mut tx, lines, ctx, authority, Before, Created, "x").await);
                rollback(tx).await?;
                // A delete admission of orders at the target admission of lines.
                let admission = admit::<BindOrder>(ctx, Delete)?.__admit_entity(ctx).await?;
                let mut tx = admission.begin(TransactionMode::Default).await?;
                out.push(outcome(
                    admission
                        .__lock_target(ctx, &mut tx, by_id::<BindLine>("l1"))
                        .await,
                ));
                rollback(tx).await?;
            }
            // A different operation, and the rows of a bulk write.
            "operation" => {
                // A single-row authority at the bulk entry points.
                let single = admit::<BindLine>(ctx, Create)?.__admit_entity(ctx).await?;
                let mut tx = single.begin(TransactionMode::Default).await?;
                out.push(bulk_outcome(
                    lines
                        .__admit_bulk(db, Some((ctx, single.authority())))
                        .await,
                ));
                out.push(bulk_outcome(
                    bulk_write_targets(
                        db,
                        Some((ctx, single.authority())),
                        &mut tx,
                        lines_of("o1"),
                        line_key,
                    )
                    .await,
                ));
                // Each admission at the other's entity admission.
                out.push(outcome(
                    admit::<BindLine>(ctx, DeleteMany)?
                        .__admit_entity(ctx)
                        .await
                        .map(drop),
                ));
                out.push(outcome(
                    admit::<BindLine>(ctx, Delete)?
                        .__admit_entity_bulk(ctx)
                        .await
                        .map(drop),
                ));
                // A bulk admission: unissued, and no single-row target.
                let bulk = admit::<BindLine>(ctx, DeleteMany)?
                    .__admit_entity_bulk(ctx)
                    .await??;
                let authority = bulk.authority();
                out.push(dispatch(&mut tx, lines, ctx, authority, Before, Deleted, "l1").await);
                out.push(outcome(
                    bulk.__lock_target(ctx, &mut tx, by_id::<BindLine>("l1"))
                        .await,
                ));
                out.push(outcome(bulk.__admit_upsert_target::<BindLine>(ctx).await));
                // Its own bulk admission.
                out.push(bulk_outcome(
                    lines.__admit_bulk(db, Some((ctx, authority))).await,
                ));
                // A filter that admits no row issues nothing, for good.
                let targets = bulk_write_targets(
                    db,
                    Some((ctx, authority)),
                    &mut tx,
                    lines_of("none"),
                    line_key,
                )
                .await
                .map_err(graphql_error_from_sqlx)?;
                assert!(targets.is_empty());
                out.push(dispatch(&mut tx, lines, ctx, authority, Before, Deleted, "l1").await);
                out.push(bulk_outcome(
                    bulk_write_targets(
                        db,
                        Some((ctx, authority)),
                        &mut tx,
                        lines_of("o1"),
                        line_key,
                    )
                    .await,
                ));
                // Two rows: the authority is theirs alone.
                let bulk = admit::<BindLine>(ctx, DeleteMany)?
                    .__admit_entity_bulk(ctx)
                    .await??;
                let authority = bulk.authority();
                let targets = bulk_write_targets(
                    db,
                    Some((ctx, authority)),
                    &mut tx,
                    lines_of("o1"),
                    line_key,
                )
                .await
                .map_err(graphql_error_from_sqlx)?;
                assert_eq!(targets.len(), 2);
                out.push(dispatch(&mut tx, lines, ctx, authority, Before, Updated, "l1").await);
                out.push(dispatch(&mut tx, lines, ctx, authority, Before, Deleted, "l3").await);
                out.push(dispatch(&mut tx, lines, ctx, authority, Before, Deleted, "l1").await);
                out.push(dispatch(&mut tx, lines, ctx, authority, Before, Deleted, "l2").await);
                out.push(dispatch(&mut tx, lines, ctx, authority, Before, Deleted, "l1").await);
                // Issued once: neither bulk entry point accepts it again.
                out.push(bulk_outcome(
                    lines.__admit_bulk(db, Some((ctx, authority))).await,
                ));
                out.push(bulk_outcome(
                    bulk_write_targets(
                        db,
                        Some((ctx, authority)),
                        &mut tx,
                        lines_of("o3"),
                        line_key,
                    )
                    .await,
                ));
                out.push(dispatch(&mut tx, lines, ctx, authority, Before, Deleted, "l3").await);
                rollback(tx).await?;
            }
            // A different `Database`.
            "database" => {
                let other = db.clone();
                let created = admit::<BindLine>(ctx, Create)?.__admit_entity(ctx).await?;
                let mut tx = begin(&other).await?;
                out.push(
                    dispatch(
                        &mut tx,
                        lines,
                        ctx,
                        created.authority(),
                        Before,
                        Created,
                        "x",
                    )
                    .await,
                );
                let delete = admit::<BindOrder>(ctx, Delete)?.__admit_entity(ctx).await?;
                out.push(outcome(
                    delete
                        .__lock_target(ctx, &mut tx, by_id::<BindOrder>("o1"))
                        .await,
                ));
                let bulk = admit::<BindLine>(ctx, DeleteMany)?
                    .__admit_entity_bulk(ctx)
                    .await??;
                for handle in [db, &other] {
                    out.push(bulk_outcome(
                        bulk_write_targets(
                            handle,
                            Some((ctx, bulk.authority())),
                            &mut tx,
                            lines_of("o1"),
                            line_key,
                        )
                        .await,
                    ));
                }
                out.push(bulk_outcome(
                    lines
                        .__admit_bulk(&other, Some((ctx, bulk.authority())))
                        .await,
                ));
                rollback(tx).await?;
                // The same authority on the request's own handle.
                let mut tx = created.begin(TransactionMode::Default).await?;
                out.push(
                    dispatch(
                        &mut tx,
                        lines,
                        ctx,
                        created.authority(),
                        Before,
                        Created,
                        "x",
                    )
                    .await,
                );
                rollback(tx).await?;
            }
            // A different request, or another root field of this one.
            "stash" => {
                let authority = admit::<BindLine>(ctx, Create)?
                    .__admit_entity(ctx)
                    .await?
                    .into_authority();
                *ctx.data::<Stash>()?.0.lock().unwrap() = Some(authority);
                out.push("stashed".to_owned());
            }
            "stashed" => {
                let stashed = ctx.data::<Stash>()?.0.lock().unwrap().take().unwrap();
                let own = admit::<BindLine>(ctx, Create)?.__admit_entity(ctx).await?;
                let mut tx = own.begin(TransactionMode::Default).await?;
                out.push(dispatch(&mut tx, lines, ctx, &stashed, Before, Created, "x").await);
                rollback(tx).await?;
            }
            // The same selection, repeated: each invocation is its own.
            "repeat" => {
                let own = admit::<BindLine>(ctx, Create)?.__admit_entity(ctx).await?;
                let stashed = ctx.data::<Stash>()?.0.lock().unwrap().take();
                let step = match stashed {
                    None => {
                        *ctx.data::<Stash>()?.0.lock().unwrap() = Some(own.into_authority());
                        "stashed".to_owned()
                    }
                    Some(stashed) => {
                        let mut tx = own.begin(TransactionMode::Default).await?;
                        let presented =
                            dispatch(&mut tx, lines, ctx, &stashed, Before, Created, "x").await;
                        let control =
                            dispatch(&mut tx, lines, ctx, own.authority(), Before, Created, "x")
                                .await;
                        rollback(tx).await?;
                        format!("first invocation's: {presented}; own: {control}")
                    }
                };
                ctx.data::<Repeats>()?.0.lock().unwrap().push(step);
            }
            // The synthetic child contexts of a nested create.
            "nested" => {
                let child_field = |name| {
                    __nested_create_field(ctx, name, "input", Value::Object(Default::default()))
                };
                let nested = |child: &Context<'_>, parent| {
                    __admit_generated_nested_create_request::<BindLine>(child, parent)
                };
                let parent = admit::<BindOrder>(ctx, Create)?.__admit_entity(ctx).await?;
                let field = child_field("createBindLine");
                let child = ctx.with_field(&field);
                // Request admission under the child, entity admission under
                // the root.
                out.push(outcome(
                    nested(&child, &parent)?.__admit_entity(ctx).await.map(drop),
                ));
                // A parent that is not a create.
                let delete = admit::<BindOrder>(ctx, Delete)?.__admit_entity(ctx).await?;
                out.push(outcome(nested(&child, &delete).map(drop)));
                let authority = nested(&child, &parent)?
                    .__admit_entity(&child)
                    .await?
                    .into_authority();
                // A child row admitted on its own, outside the invocation.
                let orphan = admit::<BindLine>(&child, Create)?
                    .__admit_entity(&child)
                    .await?
                    .into_authority();
                // The transaction of another invocation of the same root.
                let other = admit::<BindOrder>(ctx, Create)?.__admit_entity(ctx).await?;
                let mut tx = other.begin(TransactionMode::Default).await?;
                out.push(dispatch(&mut tx, lines, &child, &authority, Before, Created, "c1").await);
                rollback(tx).await?;
                let mut tx = parent.begin(TransactionMode::Default).await?;
                out.push(dispatch(&mut tx, lines, &child, &orphan, Before, Created, "c0").await);
                // A repeated selection of the root, on this same transaction:
                // the same response key and the same synthetic field name,
                // placed at another selection of the document.
                let mut elsewhere = field.clone();
                elsewhere.pos.line += 1;
                let repeated = ctx.with_field(&elsewhere);
                out.push(
                    dispatch(&mut tx, lines, &repeated, &authority, Before, Created, "c1").await,
                );
                out.push(outcome(nested(&repeated, &parent).map(drop)));
                // The child's authority under its parent's field context.
                out.push(dispatch(&mut tx, lines, ctx, &authority, Before, Created, "c1").await);
                // The parent's authority under the child's.
                out.push(
                    dispatch(
                        &mut tx,
                        orders,
                        &child,
                        parent.authority(),
                        Before,
                        Created,
                        "p",
                    )
                    .await,
                );
                // A sibling child of another entity, under its own field.
                let sibling_field = child_field("createBindOrder");
                let sibling = ctx.with_field(&sibling_field);
                out.push(
                    dispatch(&mut tx, orders, &sibling, &authority, Before, Created, "c1").await,
                );
                out.push(
                    dispatch(&mut tx, lines, &sibling, &authority, Before, Created, "c1").await,
                );
                // The write step rebuilds the child's context around the
                // field the prepared row carries: the binding is the
                // invocation's, not one context value's.
                let moved = field.clone();
                let rebuilt = ctx.with_field(&moved);
                out.push(
                    dispatch(&mut tx, lines, &rebuilt, &authority, Before, Created, "c1").await,
                );
                out.push(
                    dispatch(&mut tx, lines, &rebuilt, &authority, After, Created, "c1").await,
                );
                // A second child row needs its own admission.
                out.push(
                    dispatch(&mut tx, lines, &rebuilt, &authority, Before, Created, "c2").await,
                );
                out.push(
                    dispatch(&mut tx, lines, &rebuilt, &authority, After, Created, "c2").await,
                );
                // The parent's own row, under its own field.
                out.push(
                    dispatch(
                        &mut tx,
                        orders,
                        ctx,
                        parent.authority(),
                        Before,
                        Created,
                        "p",
                    )
                    .await,
                );
                rollback(tx).await?;
            }
            other => panic!("unknown case {other}"),
        }
        Ok(out)
    }
}

schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [BindOrder, BindLine, BindDevice], extra_mutation_types: [HostMutation] }

/// Records each hook invocation with the subject of the originating request
/// its transaction presents.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<String>>>);

impl MutationHook for Recorder {
    fn on_mutation<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        tx: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(format!(
                "{:?} {:?} {}",
                event.phase, event.action, event.entity_name
            ));
            tx.__policy()
                .ensure_entity_access(
                    None,
                    event.entity_name,
                    event.metadata.write_policy,
                    EntityAccessKind::Write,
                    EntityAccessSurface::Repository,
                )
                .await
        })
    }
}

impl EntityPolicy for Recorder {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { panic!("the ORM calls the with_authority method") })
    }

    fn can_access_entity_with_authority<'a>(
        &'a self,
        authority: PolicyAuthority<'a>,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            // The hook's own probe.
            if ctx.is_none() && surface == EntityAccessSurface::Repository {
                self.0.lock().unwrap().push(format!(
                    "  presents {}",
                    authority
                        .hook_request()
                        .and_then(|request| request.subject())
                        .map_or_else(|| "none".to_owned(), |subject| subject.id.clone()),
                ));
            }
            Ok(true)
        })
    }
}

struct Fixture {
    db: Database,
    hooks: Recorder,
}

async fn fixture() -> Fixture {
    let mut db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    for sql in [
        "CREATE TABLE bind_orders (id TEXT PRIMARY KEY, label TEXT NOT NULL)",
        "CREATE TABLE bind_lines (id TEXT PRIMARY KEY, order_id TEXT NOT NULL)",
        "CREATE TABLE bind_devices (id TEXT PRIMARY KEY, mac TEXT NOT NULL UNIQUE, name TEXT NOT NULL)",
        "INSERT INTO bind_devices VALUES ('d1', 'm1', 'first'), ('d2', 'm2', 'second')",
        "INSERT INTO bind_orders VALUES ('o1', 'first'), ('o2', 'second')",
        "INSERT INTO bind_lines VALUES ('l1', 'o1'), ('l2', 'o1'), ('l3', 'o3')",
    ] {
        graphql_orm::sqlx::query(sql)
            .execute(db.pool())
            .await
            .unwrap();
    }
    db = db.with_request_authority_propagated_to_hooks(true);
    let hooks = Recorder::default();
    db.set_mutation_hook(hooks.clone());
    db.set_entity_policy(hooks.clone());
    Fixture { db, hooks }
}

fn operator(scopes: &[&str]) -> AuthSubject {
    AuthSubject::builder("operator")
        .scopes(scopes.iter().map(|scope| (*scope).to_owned()).collect())
        .build()
}

const ALL_SCOPES: [&str; 2] = ["orders.delete", "orders.o1.update"];

impl Fixture {
    fn schema(&self) -> AppSchema {
        schema_builder(self.db.clone())
            .data(Stash::default())
            .data(Repeats::default())
            .finish()
    }

    /// The outcomes of `probe(case:)`, one list per root field of `query`.
    async fn run(
        &self,
        schema: &AppSchema,
        query: &str,
        subject: Option<AuthSubject>,
    ) -> Vec<Vec<String>> {
        let mut request = async_graphql::Request::new(query);
        if let Some(subject) = subject {
            request = request.data(subject);
        }
        let response = schema.execute(request).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let serde_json::Value::Object(fields) = response.data.into_json().unwrap() else {
            panic!("no data");
        };
        fields
            .into_iter()
            .map(|(_, steps)| serde_json::from_value(steps).unwrap())
            .collect()
    }

    async fn probe(&self, case: &str) -> Vec<String> {
        self.run(
            &self.schema(),
            &format!("mutation {{ probe(case: \"{case}\") }}"),
            Some(operator(&ALL_SCOPES)),
        )
        .await
        .remove(0)
    }

    /// What the hook recorded, and clear it.
    fn hooks(&self) -> Vec<String> {
        std::mem::take(&mut *self.hooks.0.lock().unwrap())
    }
}

/// The steps answered as `expected`, and the hook ran for the `dispatched`
/// admitted dispatches only, presenting the operator's request each time: a
/// refused dispatch never reached it.
fn assert_hooks(fixture: &Fixture, steps: &[String], expected: &[&str], dispatched: usize) {
    assert_eq!(steps, expected);
    let hooks = fixture.hooks();
    assert_eq!(hooks.len(), dispatched * 2, "{hooks:?}");
    for presented in hooks.iter().skip(1).step_by(2) {
        assert_eq!(presented, "  presents operator", "{hooks:?}");
    }
}

#[tokio::test]
async fn admission_inputs_come_from_the_entity_configuration() {
    // The host resolver names an entity and an operation and nothing else:
    // the authentication mode and the scope requirement are the ones the
    // entity declares, with the request's own argument values in a template.
    let fixture = fixture().await;
    let schema = fixture.schema();
    let config = "mutation { probe(case: \"config\") }";
    let run = |subject| async { fixture.run(&schema, config, subject).await.remove(0) };
    // delete (fixed scope), update o1 and o2 (template), create order (no
    // scopes declared), create line (`auth = "optional"`).
    assert_eq!(
        run(Some(operator(&ALL_SCOPES))).await,
        ["ok", "ok", "error: forbidden", "ok", "ok", "refused"]
    );
    assert_eq!(
        run(Some(operator(&["orders.o2.update"]))).await,
        [
            "error: forbidden",
            "error: forbidden",
            "ok",
            "ok",
            "ok",
            "refused"
        ]
    );
    // `auth = "required"` is the order entity's own declaration.
    let anonymous = run(None).await;
    assert!(
        anonymous[..4]
            .iter()
            .all(|step| step.starts_with("error: ")),
        "{anonymous:?}"
    );
    assert_eq!(anonymous[4..], ["ok", "refused"]);
    assert!(fixture.hooks().is_empty());
}

/// Refuses every requirement it is asked to enforce and records its field.
#[derive(Debug, Default)]
struct RefusingAssurance(Mutex<Vec<String>>);
impl AssuranceRequirementEvaluator for RefusingAssurance {
    fn enforce(
        &self,
        check: &AssuranceCheck<'_>,
        _: AssuranceActorClass,
        _: &str,
    ) -> async_graphql::Result<()> {
        self.0.lock().unwrap().push(check.field_name().to_owned());
        Err(async_graphql::Error::new("step up")
            .extend_with(|_, extensions| extensions.set("code", "STEP_UP_REQUIRED")))
    }
}

#[tokio::test]
async fn admission_enforces_the_operations_own_assurance_coordinate() {
    // The delete root of orders requires assurance; the host's `probe` field
    // and the create root of lines require none. A host resolver that asks
    // for the delete's admission is held to the delete's requirement, not to
    // its own field's.
    let fixture = fixture().await;
    let mut registry = OperationAssuranceRegistry::builder(
        graphql_orm_operation_catalog(),
        AssuranceSchemaConfig::legacy(),
    );
    registry
        .require(
            GraphqlOperationKind::Mutation,
            "deleteBindOrder",
            "order.assured",
        )
        .unwrap();
    let evaluator = Arc::new(RefusingAssurance::default());
    let schema = schema_builder(fixture.db.clone())
        .data(AssuranceEnforcement::new(
            Arc::new(registry.build().unwrap()),
            evaluator.clone(),
        ))
        .finish();
    let steps = fixture
        .run(
            &schema,
            "mutation { probe(case: \"assurance\") }",
            Some(operator(&ALL_SCOPES)),
        )
        .await
        .remove(0);
    assert_eq!(steps, ["error: step up", "ok"]);
    assert_eq!(*evaluator.0.lock().unwrap(), ["deleteBindOrder"]);
}

#[tokio::test]
async fn authority_is_issued_only_for_the_admitted_row_and_transaction() {
    let fixture = fixture().await;
    let steps = fixture.probe("target").await;
    assert_hooks(
        &fixture,
        &steps,
        &[
            // After the unlocked read alone; after a missing target; no
            // second attempt.
            "refused", "refused", "refused",
            // Locked and admitted: not an update; not another row; the
            // row's `Before`, once.
            "refused", "refused", "ok", "refused",
            // `After`: not another row; the row's, once.
            "refused", "ok", "refused",
            // A second target; a second transaction of the invocation.
            "refused", "refused",
            // A fresh transaction on the same handle; a detached authority.
            "refused", "refused",
        ],
        2,
    );
}

#[tokio::test]
async fn an_upsert_authority_is_the_branch_its_target_admission_decided() {
    let fixture = fixture().await;
    let steps = fixture.probe("upsert").await;
    assert_hooks(
        &fixture,
        &steps,
        &[
            // Before the lookup: no dispatch, no decision.
            "refused", "refused",
            // Looked up: no dispatch yet; no second lookup; the decision.
            "refused", "refused", "ok",
            // The row that was found: not a create; not another row; its
            // update; decided once.
            "refused", "refused", "ok", "refused",
            // No row: the decision; not an update; no `After` before
            // `Before`.
            "ok", "refused", "refused",
            // `Before`: without the conflict key; with another device's;
            // with the key that was looked up.
            "refused", "refused", "ok",
            // `After`: with another device's key; with the looked-up key.
            "refused", "ok",
        ],
        3,
    );
}

#[tokio::test]
async fn concurrent_target_admissions_of_one_admission_do_not_both_win() {
    // The row policy holds its decision until the second caller has been
    // answered. The first caller's claim stands, the second is refused
    // without reaching the policy, and nothing the second did replaced the
    // first's binding.
    let mut fixture = fixture().await;
    let gate = Arc::new(Gate::default());
    fixture.db.set_row_policy(GatedRows(gate.clone()));
    let schema = schema_builder(fixture.db.clone())
        .data(Stash::default())
        .data(Repeats::default())
        .data(gate)
        .finish();
    let steps = fixture
        .run(
            &schema,
            "mutation { probe(case: \"race\") }",
            Some(operator(&ALL_SCOPES)),
        )
        .await
        .remove(0);
    assert_hooks(
        &fixture,
        &steps,
        &[
            // Upsert: the first decision, the second refused; one policy
            // decision was asked; the first's row dispatches.
            "ok", "refused", "asked 1", "ok",
            // Single row: the lock, the concurrent admission refused; the
            // locked row dispatches, the other does not.
            "ok", "refused", "ok", "refused",
            // Bulk: the targets, the concurrent bulk admission refused; an
            // admitted row dispatches.
            "ok", "refused", "ok",
        ],
        3,
    );
}

#[tokio::test]
async fn an_authority_is_refused_for_a_different_entity() {
    let fixture = fixture().await;
    let steps = fixture.probe("entity").await;
    assert_hooks(&fixture, &steps, &["refused", "ok", "refused"], 1);
}

#[tokio::test]
async fn an_authority_is_refused_for_a_different_operation_or_bulk_row() {
    let fixture = fixture().await;
    let steps = fixture.probe("operation").await;
    assert_hooks(
        &fixture,
        &steps,
        &[
            // A single-row authority at the bulk admission and bulk targets.
            "refused", "refused",
            // A bulk request at single-row entity admission, and the reverse.
            "refused", "refused",
            // A bulk authority: unissued; no single-row or upsert target.
            "refused", "refused", "refused",
            // Its own bulk admission; unissued after an empty result, and no
            // second target admission.
            "ok", "refused", "refused",
            // Issued for two rows: not an update; not a row outside them;
            // each row's `Before`, once.
            "refused", "refused", "ok", "ok", "refused",
            // Issued once; the set does not grow.
            "refused", "refused", "refused",
        ],
        2,
    );
}

#[tokio::test]
async fn an_authority_is_refused_on_a_different_database() {
    let fixture = fixture().await;
    let steps = fixture.probe("database").await;
    assert_hooks(
        &fixture,
        &steps,
        &[
            // Dispatch and single-row target admission on a clone's
            // transaction.
            "refused", "refused",
            // Bulk targets on the request's handle with the clone's
            // transaction, and on the clone; bulk admission there.
            "refused", "refused", "refused", // The request's own handle.
            "ok",
        ],
        1,
    );
}

#[tokio::test]
async fn an_authority_is_refused_in_a_different_request_or_invocation() {
    let fixture = fixture().await;
    let repeats = Repeats::default();
    let schema = schema_builder(fixture.db.clone())
        .data(Stash::default())
        .data(repeats.clone())
        .finish();
    let subject = || Some(operator(&ALL_SCOPES));
    // Another request on the same schema, by the same subject.
    fixture
        .run(&schema, "mutation { probe(case: \"stash\") }", subject())
        .await;
    let steps = fixture
        .run(&schema, "mutation { probe(case: \"stashed\") }", subject())
        .await
        .remove(0);
    assert_hooks(&fixture, &steps, &["refused"], 0);
    // Another root field of the same request.
    let steps = fixture
        .run(
            &schema,
            "mutation { first: probe(case: \"stash\") second: probe(case: \"stashed\") }",
            subject(),
        )
        .await
        .remove(1);
    assert_hooks(&fixture, &steps, &["refused"], 0);
    // The same selection twice: one response key, two invocations.
    let response = schema
        .execute(
            async_graphql::Request::new(
                "mutation { probe(case: \"repeat\") probe(case: \"repeat\") }",
            )
            .data(operator(&ALL_SCOPES)),
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        *repeats.0.lock().unwrap(),
        ["stashed", "first invocation's: refused; own: ok"]
    );
    assert_eq!(fixture.hooks().len(), 2);
}

#[tokio::test]
async fn nested_child_contexts_share_one_invocation_and_nothing_more() {
    let fixture = fixture().await;
    let steps = fixture.probe("nested").await;
    assert_hooks(
        &fixture,
        &steps,
        &[
            // Request admission under the child, entity admission under the
            // root; a parent that is not a create.
            "refused", "refused",
            // The child's authority on the transaction of another invocation
            // of the same root.
            "refused", // A child admitted outside the invocation.
            "refused",
            // A repeated selection of the root on the same transaction: the
            // child's dispatch, and a child admission through this parent.
            "refused", "refused",
            // The child's authority under the parent's field; the parent's
            // under the child's.
            "refused", "refused",
            // Under a sibling's field, for the sibling's entity and its own.
            "refused", "refused",
            // Under the child's rebuilt context: its row's two phases.
            "ok", "ok", // A second row.
            "refused", "refused", // The parent's own row.
            "ok",
        ],
        3,
    );
}

#[tokio::test]
async fn generated_mutations_present_their_own_authority() {
    // The generated resolvers of the same entities, end to end: each hook
    // invocation presents the operator's request.
    let fixture = fixture().await;
    let schema = fixture.schema();
    for (query, hooks) in [
        (
            r#"mutation { createBindLine(input: { id: "l9", orderId: "o1" }) { success } }"#,
            2,
        ),
        (
            r#"mutation { updateBindOrder(id: "o1", input: { label: "x" }) { success } }"#,
            2,
        ),
        (
            r#"mutation { updateBindLines(where: { orderId: { eq: "o1" } }, input: { orderId: "o2" }) { success } }"#,
            6,
        ),
        (
            r#"mutation { deleteBindLines(where: { orderId: { eq: "o2" } }) { success } }"#,
            6,
        ),
        (r#"mutation { deleteBindOrder(id: "o1") { success } }"#, 2),
    ] {
        let response = schema
            .execute(async_graphql::Request::new(query).data(operator(&ALL_SCOPES)))
            .await;
        assert!(response.errors.is_empty(), "{query}: {:?}", response.errors);
        let recorded = fixture.hooks();
        assert_eq!(recorded.len(), hooks * 2, "{query}: {recorded:?}");
        for presented in recorded.iter().skip(1).step_by(2) {
            assert_eq!(presented, "  presents operator", "{query}: {recorded:?}");
        }
    }
}
