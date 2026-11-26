//! The order of policy, transform and hook callbacks of every generated write
//! shape, and what each refusal answers (#724).
//!
//! `tests/goldens/write_pipeline_sequence.txt` was captured on main before the
//! generated write admission moved into the runtime pipeline
//! (`graphql_orm::graphql::orm::write_pipeline`). The transcript must not
//! change: each mutation shape asks the same decisions in the same order,
//! refuses with the same error, and leaves the same rows and events behind.
//!
//! Regenerate with `GOM_UPDATE_WRITE_PIPELINE_GOLDEN=1` only for a deliberate
//! change to that order, and review the diff line by line.

#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::async_graphql::ErrorExtensions;
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::prelude::*;
use std::any::Any;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

#[derive(
    GraphQLEntity,
    GraphQLOperations,
    GraphQLRelations,
    graphql_orm::async_graphql::SimpleObject,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
)]
#[graphql_entity(
    backend = "sqlite",
    table = "seq_orders",
    plural = "SeqOrders",
    read_policy = "order.read",
    write_policy = "order.write"
)]
#[graphql_orm(operation_authorization(
    categories = ["create", "update", "update_many", "delete", "delete_many"],
    all_scopes = ["orders.write"]
))]
#[graphql(complex)]
struct SeqOrder {
    #[primary_key]
    #[sortable]
    #[filterable(type = "string")]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[filterable(type = "string")]
    tenant: String,
    #[graphql_orm(write_policy = "guard")]
    note: Option<String>,
    #[relation(target = "SeqLine", from = ["id", "tenant"], to = ["order_id", "tenant"], multiple, nested_create, emit_fk = false)]
    #[graphql(skip)]
    #[serde(default)]
    lines: Vec<SeqLine>,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "sqlite",
    table = "seq_lines",
    plural = "SeqLines",
    nested_create_keys = ["order_id", "tenant"],
    read_policy = "line.read",
    write_policy = "line.write"
)]
#[graphql_orm(operation_authorization(categories = ["create"], all_scopes = ["lines.write"]))]
struct SeqLine {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    id: String,
    order_id: String,
    #[graphql_orm(write_policy = "guard")]
    note: Option<String>,
    tenant: String,
    #[graphql_orm(version = "managed", default = "0")]
    version: i64,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "sqlite",
    table = "seq_devices",
    plural = "SeqDevices",
    read_policy = "device.read",
    write_policy = "device.write",
    upsert = "mac",
    upsert_acknowledge = true
)]
#[graphql_orm(operation_authorization(categories = ["upsert"], all_scopes = ["devices.write"]))]
struct SeqDevice {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[unique]
    #[filterable(type = "string")]
    mac: String,
    #[graphql_orm(write_policy = "guard")]
    name: String,
}

schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [SeqOrder, SeqLine, SeqDevice] }

/// What one scenario refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refuse {
    Nothing,
    /// The subject holds no scope.
    Scope,
    /// The resolver-level assurance requirement.
    Assurance,
    /// The entity write decision of the request.
    Entity,
    /// The entity write decision asked again for a hook's authority.
    EntityAtHook,
    /// The child entity of a nested create.
    ChildEntity,
    /// Every row write decision.
    RowWrite,
    /// Every row read and write decision.
    RowReadWrite,
    /// The guarded field.
    Field,
    /// The `Before` hook.
    HookBefore,
    /// The `After` hook.
    HookAfter,
    /// The input transform.
    Transform,
}

const REFUSALS: [Refuse; 12] = [
    Refuse::Nothing,
    Refuse::Scope,
    Refuse::Assurance,
    Refuse::Entity,
    Refuse::EntityAtHook,
    Refuse::ChildEntity,
    Refuse::RowWrite,
    Refuse::RowReadWrite,
    Refuse::Field,
    Refuse::HookBefore,
    Refuse::HookAfter,
    Refuse::Transform,
];

#[derive(Clone)]
struct Log(Arc<Mutex<Vec<String>>>);
impl Log {
    fn push(&self, line: String) {
        self.0.lock().unwrap().push(line);
    }
}

fn field(ctx: Option<&async_graphql::Context<'_>>) -> String {
    ctx.map_or_else(|| "-".to_owned(), |ctx| ctx.field().name().to_owned())
}

fn refusal(ctx: Option<&async_graphql::Context<'_>>) -> Refuse {
    ctx.and_then(|ctx| ctx.data_opt::<Refuse>().copied())
        .unwrap_or(Refuse::Nothing)
}

fn row_id(row: &(dyn Any + Send + Sync)) -> String {
    if let Some(row) = row.downcast_ref::<SeqOrder>() {
        format!("order:{}", row.id)
    } else if let Some(row) = row.downcast_ref::<SeqLine>() {
        format!("line:{}", row.id)
    } else if let Some(row) = row.downcast_ref::<SeqDevice>() {
        format!("device:{}", row.id)
    } else {
        "other".to_owned()
    }
}

/// The refusal the scenario selected, kept where callbacks without a request
/// (a hook's nested decision) can read it.
#[derive(Clone)]
struct Recorder {
    log: Log,
    active: Arc<Mutex<Refuse>>,
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
        entity: &'static str,
        key: Option<&'static str>,
        kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            // The hook's own probe: records whether the originating request
            // is presented to the hook's transaction.
            if ctx.is_none() && surface == EntityAccessSurface::Repository {
                self.log.push(format!(
                    "  entity {entity} {key:?} {kind:?} {surface:?} hook_request={}",
                    authority
                        .hook_request()
                        .and_then(|request| request.subject())
                        .map_or_else(|| "none".to_owned(), |subject| subject.id.clone()),
                ));
                return Ok(true);
            }
            let mode = refusal(ctx);
            // The dispatcher asks on the root field after the first decision
            // of the request; count the decisions to tell them apart.
            let asked = self
                .log
                .0
                .lock()
                .unwrap()
                .iter()
                .filter(|line| line.starts_with(&format!("entity {entity} ")))
                .count();
            self.log.push(format!(
                "entity {entity} {key:?} {kind:?} {surface:?} field={}",
                field(ctx)
            ));
            let child = entity == "SeqLine" && field(ctx) == "createSeqLine";
            Ok(match mode {
                Refuse::Entity => kind != EntityAccessKind::Write,
                Refuse::ChildEntity => !(child && kind == EntityAccessKind::Write),
                // Bulk writes ask twice before the first hook.
                Refuse::EntityAtHook => {
                    let before_hooks = if field(ctx).ends_with("SeqOrders") { 2 } else { 1 };
                    !(kind == EntityAccessKind::Write && asked >= before_hooks)
                }
                _ => true,
            })
        })
    }
}

impl RowPolicy for Recorder {
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { panic!("the ORM calls the with_authority method") })
    }

    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { panic!("the ORM calls the with_authority method") })
    }

    fn can_read_deleted_preimage<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        key: Option<&'static str>,
        surface: EntityAccessSurface,
        row: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            self.log.push(format!(
                "row-preimage {entity} {key:?} {surface:?} {} field={}",
                row_id(row),
                field(ctx)
            ));
            Ok(true)
        })
    }

    fn read_visibility_with_authority<'a>(
        &'a self,
        _: PolicyAuthority<'a>,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        key: Option<&'static str>,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            self.log.push(format!(
                "row-visibility {entity} {key:?} {surface:?} field={}",
                field(ctx)
            ));
            Ok(ReadVisibility::CallbackOnly)
        })
    }

    fn can_read_row_with_authority<'a>(
        &'a self,
        _: PolicyAuthority<'a>,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        key: Option<&'static str>,
        surface: EntityAccessSurface,
        row: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            self.log.push(format!(
                "row-read {entity} {key:?} {surface:?} {} field={}",
                row_id(row),
                field(ctx)
            ));
            Ok(*self.active.lock().unwrap() != Refuse::RowReadWrite)
        })
    }

    fn can_write_row_with_authority<'a>(
        &'a self,
        _: PolicyAuthority<'a>,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        key: Option<&'static str>,
        surface: EntityAccessSurface,
        row: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            self.log.push(format!(
                "row-write {entity} {key:?} {surface:?} {} field={}",
                row_id(row),
                field(ctx)
            ));
            Ok(!matches!(
                *self.active.lock().unwrap(),
                Refuse::RowWrite | Refuse::RowReadWrite
            ))
        })
    }

    fn write_target_prefilter<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        key: Option<&'static str>,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<Option<ReadPredicate>>> {
        Box::pin(async move {
            self.log.push(format!(
                "row-prefilter {entity} {key:?} {surface:?} field={}",
                field(ctx)
            ));
            Ok(None)
        })
    }
}

impl FieldPolicy for Recorder {
    fn can_read_field<'a>(
        &'a self,
        ctx: &'a async_graphql::Context<'_>,
        _: &'a Database,
        entity: &'static str,
        name: &'static str,
        key: Option<&'static str>,
        record: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            self.log.push(format!(
                "field-read {entity}.{name} {key:?} record={} field={}",
                record.map_or_else(|| "-".to_owned(), row_id),
                field(Some(ctx))
            ));
            Ok(true)
        })
    }

    fn can_write_field<'a>(
        &'a self,
        ctx: &'a async_graphql::Context<'_>,
        _: &'a Database,
        entity: &'static str,
        name: &'static str,
        key: Option<&'static str>,
        record: Option<&'a (dyn Any + Send + Sync)>,
        value: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            self.log.push(format!(
                "field-write {entity}.{name} {key:?} record={} value={} field={}",
                record.map_or_else(|| "-".to_owned(), row_id),
                value.is_some(),
                field(Some(ctx))
            ));
            Ok(refusal(Some(ctx)) != Refuse::Field)
        })
    }
}

impl WriteInputTransform for Recorder {
    fn before_create_with_context<'a>(
        &'a self,
        write_ctx: &'a mut WriteInputContext<'_, '_>,
        _: &'a mut (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move { self.transform("create", write_ctx, false) })
    }

    fn before_update_with_context<'a>(
        &'a self,
        write_ctx: &'a mut WriteInputContext<'_, '_>,
        existing: Option<&'a (dyn Any + Send + Sync)>,
        _: &'a mut (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move { self.transform("update", write_ctx, existing.is_some()) })
    }

    fn before_upsert_with_context<'a>(
        &'a self,
        write_ctx: &'a mut WriteInputContext<'_, '_>,
        _: &'a mut (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move { self.transform("upsert", write_ctx, false) })
    }
}

impl Recorder {
    fn transform(
        &self,
        kind: &str,
        write_ctx: &WriteInputContext<'_, '_>,
        existing: bool,
    ) -> async_graphql::Result<()> {
        self.log.push(format!(
            "transform {kind} {} {:?} existing={existing} field={}",
            write_ctx.entity_name(),
            write_ctx.origin(),
            field(write_ctx.graphql_ctx()),
        ));
        if *self.active.lock().unwrap() == Refuse::Transform {
            return Err(async_graphql::Error::new("transform refused")
                .extend_with(|_, extensions| extensions.set("code", "TRANSFORM_REFUSED")));
        }
        Ok(())
    }
}

impl MutationHook for Recorder {
    fn on_mutation<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        tx: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            self.log.push(format!(
                "hook {:?} {:?} {} table={} id={} changes=[{}] before={} after={} field={}",
                event.phase,
                event.action,
                event.entity_name,
                event.table_name,
                event.id,
                event
                    .changes
                    .iter()
                    .map(|change| change.field.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                event.before_state.is_some(),
                event.after_state.is_some(),
                field(ctx),
            ));
            // What authority the hook's own transaction presents.
            tx.__policy()
                .ensure_entity_access(
                    None,
                    event.entity_name,
                    event.metadata.write_policy,
                    EntityAccessKind::Write,
                    EntityAccessSurface::Repository,
                )
                .await?;
            let mode = *self.active.lock().unwrap();
            let refuse = match event.phase {
                MutationPhase::Before => mode == Refuse::HookBefore,
                MutationPhase::After => mode == Refuse::HookAfter,
            };
            if refuse {
                return Err(async_graphql::Error::new("hook refused")
                    .extend_with(|_, extensions| extensions.set("code", "HOOK_REFUSED")));
            }
            if event.entity_name == "SeqDevice"
                && event.phase == MutationPhase::Before
                && event.id == "ack"
                && tx.can_acknowledge_without_write()
            {
                self.log.push("  acknowledge".to_owned());
                tx.acknowledge_without_write()?;
            }
            if event.phase == MutationPhase::After {
                let log = self.log.clone();
                let id = event.id.clone();
                tx.defer(move |_| async move {
                    log.push(format!("deferred {id}"));
                    Ok::<(), std::convert::Infallible>(())
                });
            }
            Ok(())
        })
    }
}

#[derive(Debug)]
struct Assurance;
impl AssuranceRequirementEvaluator for Assurance {
    fn enforce(
        &self,
        check: &AssuranceCheck<'_>,
        _: AssuranceActorClass,
        policy_id: &str,
    ) -> async_graphql::Result<()> {
        if let Some(log) = check.data_opt::<Log>() {
            log.push(format!(
                "assurance {:?} {} {policy_id}",
                check.kind(),
                check.field_name()
            ));
        }
        if check.data_opt::<Refuse>() == Some(&Refuse::Assurance) {
            return Err(
                async_graphql::Error::new("additional authentication is required")
                    .extend_with(|_, extensions| extensions.set("code", "STEP_UP_REQUIRED")),
            );
        }
        Ok(())
    }
}

const MUTATIONS: [(&str, &str); 11] = [
    (
        "create",
        r#"mutation { createSeqOrder(input: { id: "new", tenant: "west", note: "n" }) { success error } }"#,
    ),
    (
        "create with children",
        r#"mutation { createSeqOrder(input: { id: "new", tenant: "west", note: "n", lines: [{ id: "c1", note: "x" }, { id: "c2" }] }) { success error } }"#,
    ),
    (
        "update",
        r#"mutation { updateSeqOrder(id: "o1", input: { note: "changed" }) { success error } }"#,
    ),
    (
        "update missing",
        r#"mutation { updateSeqOrder(id: "absent", input: { note: "changed" }) { success error } }"#,
    ),
    (
        "update many",
        r#"mutation { updateSeqOrders(where: { tenant: { eq: "west" } }, input: { note: "bulk" }) { success error affectedCount } }"#,
    ),
    (
        "delete",
        r#"mutation { deleteSeqOrder(id: "o1") { success error } }"#,
    ),
    (
        "delete many",
        r#"mutation { deleteSeqOrders(where: { tenant: { eq: "west" } }) { success error deletedCount } }"#,
    ),
    (
        "delete many without a filter",
        r#"mutation { deleteSeqOrders { success error deletedCount } }"#,
    ),
    (
        "upsert new",
        r#"mutation { upsertSeqDevice(input: { id: "d9", mac: "m9", name: "fresh" }) { success error action } }"#,
    ),
    (
        "upsert existing",
        r#"mutation { upsertSeqDevice(input: { id: "d1", mac: "m1", name: "renamed" }) { success error action } }"#,
    ),
    (
        "upsert acknowledged",
        r#"mutation { upsertSeqDevice(input: { id: "ack", mac: "mack", name: "renamed" }) { success error action } }"#,
    ),
];

async fn dump(db: &Database, table: &str, columns: &str) -> String {
    use graphql_orm::sqlx::Row;
    let rows = graphql_orm::sqlx::query(&format!(
        "SELECT {columns} AS line FROM {table} ORDER BY id"
    ))
    .fetch_all(db.pool())
    .await
    .unwrap();
    rows.iter()
        .map(|row| row.get::<String, _>("line"))
        .collect::<Vec<_>>()
        .join(" ")
}

async fn scenario(out: &mut String, name: &str, query: &str, mode: Refuse) {
    let mut db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    for sql in [
        "CREATE TABLE seq_orders (id TEXT PRIMARY KEY, tenant TEXT NOT NULL, note TEXT NULL)",
        "CREATE TABLE seq_lines (id TEXT PRIMARY KEY, order_id TEXT NOT NULL, tenant TEXT NOT NULL, note TEXT NULL, version BIGINT NOT NULL DEFAULT 0)",
        "CREATE TABLE seq_devices (id TEXT PRIMARY KEY, mac TEXT NOT NULL UNIQUE, name TEXT NOT NULL)",
        "INSERT INTO seq_orders VALUES ('o1', 'west', NULL), ('o2', 'west', 'kept'), ('o3', 'east', NULL)",
        "INSERT INTO seq_devices VALUES ('d1', 'm1', 'first'), ('ack', 'mack', 'second')",
    ] {
        <DefaultBackend as WriteBackend>::execute_with_binds(db.pool(), sql, &[])
            .await
            .unwrap();
    }
    db = db.with_request_authority_propagated_to_hooks(true);
    let log = Log(Arc::new(Mutex::new(Vec::new())));
    let recorder = Recorder {
        log: log.clone(),
        active: Arc::new(Mutex::new(mode)),
    };
    db.set_mutation_hook(recorder.clone());
    db.set_entity_policy(recorder.clone());
    db.set_field_policy(recorder.clone());
    db.set_row_policy(recorder.clone());
    db.set_write_input_transform(recorder);
    let mut assurance = OperationAssuranceRegistry::builder(
        graphql_orm_operation_catalog(),
        AssuranceSchemaConfig::legacy(),
    );
    for (root, policy) in [
        ("createSeqOrder", "order.assured"),
        ("createSeqLine", "line.assured"),
        ("updateSeqOrder", "order.assured"),
        ("updateSeqOrders", "order.assured"),
        ("deleteSeqOrder", "order.assured"),
        ("deleteSeqOrders", "order.assured"),
        ("upsertSeqDevice", "device.assured"),
    ] {
        assurance
            .require(GraphqlOperationKind::Mutation, root, policy)
            .unwrap();
    }
    let schema = schema_builder(db.clone())
        .data(AssuranceEnforcement::new(
            Arc::new(assurance.build().unwrap()),
            Arc::new(Assurance),
        ))
        .finish();
    let mut orders = db.ensure_event_sender::<SeqOrderChangedEvent>().subscribe();
    let mut lines = db.ensure_event_sender::<SeqLineChangedEvent>().subscribe();
    let mut devices = db
        .ensure_event_sender::<SeqDeviceChangedEvent>()
        .subscribe();

    let scopes = if mode == Refuse::Scope {
        Vec::new()
    } else {
        vec![
            "orders.write".to_owned(),
            "lines.write".to_owned(),
            "devices.write".to_owned(),
        ]
    };
    let request = async_graphql::Request::new(query)
        .data(AuthSubject::builder("operator").scopes(scopes).build())
        .data(mode)
        .data(log.clone());
    let response = schema.execute(request).await;

    writeln!(out, "== {name} / {mode:?}").unwrap();
    for line in log.0.lock().unwrap().iter() {
        writeln!(out, "{line}").unwrap();
    }
    writeln!(
        out,
        "data {}",
        serde_json::to_string(&response.data.into_json().unwrap()).unwrap()
    )
    .unwrap();
    for error in &response.errors {
        writeln!(
            out,
            "error {:?} path={:?} extensions={}",
            error.message,
            error.path,
            error.extensions.as_ref().map_or_else(
                || "-".to_owned(),
                |extensions| serde_json::to_string(extensions).unwrap()
            ),
        )
        .unwrap();
    }
    while let Ok(event) = orders.try_recv() {
        writeln!(out, "changed SeqOrder {:?} {}", event.action, event.id).unwrap();
    }
    while let Ok(event) = lines.try_recv() {
        writeln!(out, "changed SeqLine {:?} {}", event.action, event.id).unwrap();
    }
    while let Ok(event) = devices.try_recv() {
        writeln!(out, "changed SeqDevice {:?} {}", event.action, event.id).unwrap();
    }
    writeln!(
        out,
        "rows orders[{}] lines[{}] devices[{}]",
        dump(&db, "seq_orders", "id || ':' || tenant || ':' || COALESCE(note, '-')").await,
        dump(
            &db,
            "seq_lines",
            "id || ':' || order_id || ':' || tenant || ':' || COALESCE(note, '-') || ':' || version"
        )
        .await,
        dump(&db, "seq_devices", "id || ':' || mac || ':' || name").await,
    )
    .unwrap();
}

#[tokio::test]
async fn generated_write_callback_sequence_is_unchanged() {
    let mut transcript = String::new();
    for (name, query) in MUTATIONS {
        for mode in REFUSALS {
            scenario(&mut transcript, name, query, mode).await;
        }
    }
    let path = format!(
        "{}/tests/goldens/write_pipeline_sequence.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    if std::env::var_os("GOM_UPDATE_WRITE_PIPELINE_GOLDEN").is_some() {
        std::fs::write(&path, &transcript).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap();
    if transcript != expected {
        let line = transcript
            .lines()
            .zip(expected.lines())
            .position(|(actual, expected)| actual != expected)
            .unwrap_or_else(|| transcript.lines().count().min(expected.lines().count()));
        panic!(
            "write pipeline transcript differs from the golden at line {}:\n  actual:   {:?}\n  expected: {:?}",
            line + 1,
            transcript.lines().nth(line),
            expected.lines().nth(line),
        );
    }
}
