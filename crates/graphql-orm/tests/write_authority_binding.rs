//! A generated write's authority is derived from the entity's generated
//! configuration, issued only once the admission its operation needs is
//! complete, and bound to the request, operation and `Database` it was earned
//! under (#921).
//!
//! Host Rust code is trusted and can run the macro-internal write pipeline
//! itself, as the resolvers below do. Every step that does not match the
//! admission it presents is refused before a hook runs.

#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::async_graphql::{Context, Object, Value};
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError, graphql_error_from_sqlx};
use graphql_orm::graphql::input_validation::__nested_create_field;
use graphql_orm::graphql::orm::{
    __admit_generated_write_request, EntityQuery, GeneratedWriteAssurance, GeneratedWriteAuthority,
    GeneratedWriteEntity, GeneratedWriteKind, GeneratedWriteRequest, SqlValue, bulk_write_targets,
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

/// The message of every binding refusal.
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
/// repository errors. The binding refusal is uncoded, so its text stays
/// server-side and callers see `INTERNAL_ERROR`; each case also runs the
/// same step with the matching authority, which succeeds.
fn bulk_outcome<T>(result: graphql_orm::Result<T>) -> String {
    match result.map_err(|error| OrmPublicError::from_sqlx(&error).code) {
        Ok(_) => "ok".to_owned(),
        Err(OrmErrorCode::InternalError) => "refused".to_owned(),
        Err(code) => format!("error: {code:?}"),
    }
}

/// Request admission of `kind` for `T` under `ctx`, with no scope arguments.
fn admit<T>(
    ctx: &Context<'_>,
    kind: GeneratedWriteKind,
    assurance: GeneratedWriteAssurance,
) -> graphql_orm::async_graphql::Result<GeneratedWriteRequest>
where
    T: DatabaseRls + Entity + graphql_orm::graphql::orm::GeneratedWriteConfig + 'static,
{
    __admit_generated_write_request::<T, Sqlite>(ctx, kind, assurance, &[])
}

/// One `Before` or `After` dispatch of `entity` presenting `authority`.
async fn dispatch(
    tx: &mut MutationContext<'_>,
    entity: GeneratedWriteEntity,
    ctx: &Context<'_>,
    authority: &GeneratedWriteAuthority,
    phase: MutationPhase,
    action: ChangeAction,
) -> String {
    outcome(
        tx.__run_generated_write_hook(
            &entity,
            Some((ctx, authority)),
            phase,
            action,
            "row".to_owned(),
            Vec::new(),
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

async fn begin<'a>(db: &'a Database) -> graphql_orm::async_graphql::Result<MutationContext<'a>> {
    MutationContext::begin(db, TransactionMode::Default)
        .await
        .map_err(graphql_error_from_sqlx)
}

/// An authority one request leaves for another to present.
#[derive(Default)]
struct Stash(Mutex<Option<GeneratedWriteAuthority>>);

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
        use GeneratedWriteAssurance::{Operation, Resolver};
        use GeneratedWriteKind::{Create, Delete, DeleteMany, Update};
        use MutationPhase::{After, Before};

        let db = ctx.data::<Database>()?;
        let orders = GeneratedWriteEntity::of::<BindOrder>();
        let lines = GeneratedWriteEntity::of::<BindLine>();
        let mut out = Vec::new();
        match case.as_str() {
            // Item 1: the scopes and the authentication mode are the entity's.
            "config" => {
                out.push(outcome(admit::<BindOrder>(ctx, Delete, Resolver)));
                for id in ["o1", "o2"] {
                    out.push(outcome(
                        __admit_generated_write_request::<BindOrder, Sqlite>(
                            ctx,
                            Update,
                            Resolver,
                            &[("id", id)],
                        ),
                    ));
                }
                out.push(outcome(admit::<BindOrder>(ctx, Create, Resolver)));
                out.push(outcome(admit::<BindLine>(ctx, Create, Resolver)));
            }
            // Item 2: nothing is issued before target admission succeeds.
            "target" => {
                let admission = admit::<BindOrder>(ctx, Delete, Resolver)?
                    .__admit_entity::<Sqlite>(ctx)
                    .await?;
                let authority = admission.authority();
                // The unlocked read of a dependent-delete root issues nothing.
                let read = admission
                    .__read_target(ctx, by_id::<BindOrder>("o1"))
                    .await?;
                assert!(read.is_some());
                let mut tx = begin(db).await?;
                out.push(dispatch(&mut tx, orders, ctx, authority, Before, Deleted).await);
                let absent = admission
                    .__lock_target(ctx, &mut tx, by_id::<BindOrder>("absent"))
                    .await?;
                assert!(absent.is_none());
                out.push(dispatch(&mut tx, orders, ctx, authority, Before, Deleted).await);
                let locked = admission
                    .__lock_target(ctx, &mut tx, by_id::<BindOrder>("o1"))
                    .await?;
                assert!(locked.is_some());
                // A different operation: a delete admission reports no update.
                out.push(dispatch(&mut tx, orders, ctx, authority, Before, Updated).await);
                out.push(dispatch(&mut tx, orders, ctx, authority, Before, Deleted).await);
                // One row: each phase once.
                out.push(dispatch(&mut tx, orders, ctx, authority, Before, Deleted).await);
                out.push(dispatch(&mut tx, orders, ctx, authority, After, Deleted).await);
                out.push(dispatch(&mut tx, orders, ctx, authority, After, Deleted).await);
                tx.rollback().await.map_err(graphql_error_from_sqlx)?;
            }
            // Item 4: a different entity.
            "entity" => {
                let mut tx = begin(db).await?;
                let authority = admit::<BindLine>(ctx, Create, Resolver)?
                    .__admit_entity::<Sqlite>(ctx)
                    .await?
                    .into_authority();
                out.push(dispatch(&mut tx, orders, ctx, &authority, Before, Created).await);
                out.push(dispatch(&mut tx, lines, ctx, &authority, Before, Created).await);
                // A delete admission of orders at the target admission of lines.
                let admission = admit::<BindOrder>(ctx, Delete, Resolver)?
                    .__admit_entity::<Sqlite>(ctx)
                    .await?;
                out.push(outcome(
                    admission
                        .__lock_target(ctx, &mut tx, by_id::<BindLine>("l1"))
                        .await,
                ));
                tx.rollback().await.map_err(graphql_error_from_sqlx)?;
            }
            // Item 4: a different operation.
            "operation" => {
                let mut tx = begin(db).await?;
                // A single-row authority at the bulk entry points.
                let single = admit::<BindLine>(ctx, Create, Resolver)?
                    .__admit_entity::<Sqlite>(ctx)
                    .await?
                    .into_authority();
                out.push(bulk_outcome(
                    lines.__admit_bulk(db, Some((ctx, &single))).await,
                ));
                out.push(bulk_outcome(
                    bulk_write_targets::<BindLine, Sqlite>(
                        db,
                        Some((ctx, &single)),
                        &mut tx,
                        EntityQuery::new(),
                        |line| SqlValue::String(line.id.clone()),
                    )
                    .await,
                ));
                // Each admission at the other's entity admission.
                out.push(outcome(
                    admit::<BindLine>(ctx, DeleteMany, Resolver)?
                        .__admit_entity::<Sqlite>(ctx)
                        .await
                        .map(drop),
                ));
                out.push(outcome(
                    admit::<BindLine>(ctx, Delete, Resolver)?
                        .__admit_entity_bulk::<Sqlite>(ctx)
                        .await
                        .map(drop),
                ));
                // A bulk admission: unissued, and no single-row target.
                let bulk = admit::<BindLine>(ctx, DeleteMany, Resolver)?
                    .__admit_entity_bulk::<Sqlite>(ctx)
                    .await??;
                let authority = bulk.authority();
                out.push(dispatch(&mut tx, lines, ctx, authority, Before, Deleted).await);
                out.push(outcome(
                    bulk.__lock_target(ctx, &mut tx, by_id::<BindLine>("l1"))
                        .await,
                ));
                out.push(outcome(
                    bulk.__admit_upsert_target::<BindLine>(ctx, &mut tx, None)
                        .await,
                ));
                out.push(dispatch(&mut tx, lines, ctx, authority, Before, Deleted).await);
                // Its own bulk admission and target admission, which issues
                // it for every row.
                out.push(bulk_outcome(
                    lines.__admit_bulk(db, Some((ctx, authority))).await,
                ));
                let targets = bulk_write_targets::<BindLine, Sqlite>(
                    db,
                    Some((ctx, authority)),
                    &mut tx,
                    EntityQuery::new(),
                    |line| SqlValue::String(line.id.clone()),
                )
                .await
                .map_err(graphql_error_from_sqlx)?;
                assert_eq!(targets.len(), 2);
                out.push(dispatch(&mut tx, lines, ctx, authority, Before, Updated).await);
                out.push(dispatch(&mut tx, lines, ctx, authority, Before, Deleted).await);
                out.push(dispatch(&mut tx, lines, ctx, authority, Before, Deleted).await);
                tx.rollback().await.map_err(graphql_error_from_sqlx)?;
            }
            // Item 4: a different `Database`.
            "database" => {
                let other = db.clone();
                let mut tx = begin(&other).await?;
                let created = admit::<BindLine>(ctx, Create, Resolver)?
                    .__admit_entity::<Sqlite>(ctx)
                    .await?
                    .into_authority();
                out.push(dispatch(&mut tx, lines, ctx, &created, Before, Created).await);
                let delete = admit::<BindOrder>(ctx, Delete, Resolver)?
                    .__admit_entity::<Sqlite>(ctx)
                    .await?;
                out.push(outcome(
                    delete
                        .__lock_target(ctx, &mut tx, by_id::<BindOrder>("o1"))
                        .await,
                ));
                let bulk = admit::<BindLine>(ctx, DeleteMany, Resolver)?
                    .__admit_entity_bulk::<Sqlite>(ctx)
                    .await??;
                for handle in [db, &other] {
                    out.push(bulk_outcome(
                        bulk_write_targets::<BindLine, Sqlite>(
                            handle,
                            Some((ctx, bulk.authority())),
                            &mut tx,
                            EntityQuery::new(),
                            |line| SqlValue::String(line.id.clone()),
                        )
                        .await,
                    ));
                }
                out.push(bulk_outcome(
                    lines
                        .__admit_bulk(&other, Some((ctx, bulk.authority())))
                        .await,
                ));
                tx.rollback().await.map_err(graphql_error_from_sqlx)?;
                // The same authority on the request's own handle.
                let mut tx = begin(db).await?;
                out.push(dispatch(&mut tx, lines, ctx, &created, Before, Created).await);
                tx.rollback().await.map_err(graphql_error_from_sqlx)?;
            }
            // Item 4: a different request, or another root field of this one.
            "stash" => {
                let authority = admit::<BindLine>(ctx, Create, Resolver)?
                    .__admit_entity::<Sqlite>(ctx)
                    .await?
                    .into_authority();
                *ctx.data::<Stash>()?.0.lock().unwrap() = Some(authority);
                out.push("stashed".to_owned());
            }
            "stashed" => {
                let authority = ctx.data::<Stash>()?.0.lock().unwrap().take().unwrap();
                let mut tx = begin(db).await?;
                out.push(dispatch(&mut tx, lines, ctx, &authority, Before, Created).await);
                tx.rollback().await.map_err(graphql_error_from_sqlx)?;
            }
            // Items 3 and 4: the synthetic child contexts of a nested create.
            "nested" => {
                let mut tx = begin(db).await?;
                let child_field = |name| {
                    __nested_create_field(ctx, name, "input", Value::Object(Default::default()))
                };
                let field = child_field("createBindLine");
                let child = ctx.with_field(&field);
                // Request admission under the child, entity admission under
                // the root.
                out.push(outcome(
                    admit::<BindLine>(&child, Create, Operation)?
                        .__admit_entity::<Sqlite>(ctx)
                        .await
                        .map(drop),
                ));
                let authority = admit::<BindLine>(&child, Create, Operation)?
                    .__admit_entity::<Sqlite>(&child)
                    .await?
                    .into_authority();
                // The child's authority under its parent's field context.
                out.push(dispatch(&mut tx, lines, ctx, &authority, Before, Created).await);
                // The parent's authority under the child's.
                let parent = admit::<BindOrder>(ctx, Create, Resolver)?
                    .__admit_entity::<Sqlite>(ctx)
                    .await?
                    .into_authority();
                out.push(dispatch(&mut tx, orders, &child, &parent, Before, Created).await);
                // A sibling child of another entity, under its own field.
                let sibling_field = child_field("createBindOrder");
                let sibling = ctx.with_field(&sibling_field);
                out.push(dispatch(&mut tx, orders, &sibling, &authority, Before, Created).await);
                out.push(dispatch(&mut tx, lines, &sibling, &authority, Before, Created).await);
                // The write step rebuilds the child's context around the
                // field the prepared row carries: the binding is the
                // request's, not one context value's.
                let moved = field.clone();
                let rebuilt = ctx.with_field(&moved);
                out.push(dispatch(&mut tx, lines, &rebuilt, &authority, Before, Created).await);
                out.push(dispatch(&mut tx, lines, &rebuilt, &authority, After, Created).await);
                // A second child row needs its own admission.
                out.push(dispatch(&mut tx, lines, &rebuilt, &authority, Before, Created).await);
                out.push(dispatch(&mut tx, lines, &rebuilt, &authority, After, Created).await);
                // The parent's own row, under its own field.
                out.push(dispatch(&mut tx, orders, ctx, &parent, Before, Created).await);
                tx.rollback().await.map_err(graphql_error_from_sqlx)?;
            }
            other => panic!("unknown case {other}"),
        }
        Ok(out)
    }
}

schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [BindOrder, BindLine], extra_mutation_types: [HostMutation] }

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
        "INSERT INTO bind_orders VALUES ('o1', 'first')",
        "INSERT INTO bind_lines VALUES ('l1', 'o1'), ('l2', 'o1')",
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
        ["ok", "ok", "error: forbidden", "ok", "ok"]
    );
    assert_eq!(
        run(Some(operator(&["orders.o2.update"]))).await,
        ["error: forbidden", "error: forbidden", "ok", "ok", "ok"]
    );
    // `auth = "required"` is the order entity's own declaration.
    let anonymous = run(None).await;
    assert!(
        anonymous[..4]
            .iter()
            .all(|step| step.starts_with("error: ")),
        "{anonymous:?}"
    );
    assert_eq!(anonymous[4], "ok");
    assert!(fixture.hooks().is_empty());
}

#[tokio::test]
async fn authority_is_issued_only_after_target_admission() {
    let fixture = fixture().await;
    let steps = fixture.probe("target").await;
    assert_hooks(
        &fixture,
        &steps,
        &[
            // After the unlocked read alone, and after a missing target.
            "refused", "refused",
            // Locked and admitted: not an update; the delete's `Before`,
            // once; its `After`, once.
            "refused", "ok", "refused", "ok", "refused",
        ],
        2,
    );
}

#[tokio::test]
async fn an_authority_is_refused_for_a_different_entity() {
    let fixture = fixture().await;
    let steps = fixture.probe("entity").await;
    assert_hooks(&fixture, &steps, &["refused", "ok", "refused"], 1);
}

#[tokio::test]
async fn an_authority_is_refused_for_a_different_operation() {
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
            // A bulk authority: unissued; no single-row or upsert target;
            // still unissued.
            "refused", "refused", "refused", "refused",
            // Its own bulk admission. Issued by its own targets: not an
            // update; every row's `Before`.
            "ok", "refused", "ok", "ok",
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
            // Dispatch and single-row target admission on another handle's
            // transaction.
            "refused", "refused",
            // Bulk targets on the request's handle with another handle's
            // transaction, and on the other handle; bulk admission there.
            "refused", "refused", "refused", // The request's own handle.
            "ok",
        ],
        1,
    );
}

#[tokio::test]
async fn an_authority_is_refused_in_a_different_request() {
    let fixture = fixture().await;
    let schema = fixture.schema();
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
}

#[tokio::test]
async fn nested_child_contexts_share_one_binding_and_nothing_more() {
    let fixture = fixture().await;
    let steps = fixture.probe("nested").await;
    assert_hooks(
        &fixture,
        &steps,
        &[
            // Request admission under the child, entity admission under the
            // root.
            "refused",
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
