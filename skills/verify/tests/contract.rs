//! Pins each library behaviour `skills/graphql-orm-usage/SKILL.md` states. A
//! failure here means the library changed and the skill needs updating.

use async_graphql::{Context, Object, Request, SimpleObject};
use graphql_orm::futures::StreamExt;
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::prelude::*;
use graphql_orm::sqlx::Row;
use graphql_orm::uuid::Uuid;
use std::any::Any;
use std::sync::{Arc, Mutex};
use std::time::Duration;

macro_rules! batch_load {
    ($($entity:ty),*) => {$(
        impl BatchLoadEntity for $entity {
            fn batch_column() -> &'static str {
                "id"
            }
            fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
                row.try_get("id")
            }
        }
    )*};
}

// Relation entities: FK cascade, default restrict and upward propagation.
#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_entity(table = "customers", plural = "Customers", default_sort = "name ASC")]
struct Customer {
    #[primary_key]
    id: Uuid,
    #[filterable(type = "string")]
    #[sortable]
    #[graphql_orm(min_length = 2)]
    name: String,
    #[graphql(skip)]
    #[relation(target = "Order", from = "id", to = "customer_id", multiple, filter)]
    orders: Vec<Order>,
}

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_entity(table = "orders", plural = "Orders", default_sort = "reference ASC")]
struct Order {
    #[primary_key]
    id: Uuid,
    #[filterable(type = "uuid")]
    customer_id: Uuid,
    #[sortable]
    reference: String,
    #[graphql_orm(version = "managed", default = "0")]
    row_version: i64,
    #[graphql(skip)]
    #[relation(
        target = "Customer",
        from = "customer_id",
        to = "id",
        on_delete = "cascade"
    )]
    customer: Option<Customer>,
    #[graphql(skip)]
    #[relation(target = "OrderLine", from = "id", to = "order_id", multiple)]
    lines: Vec<OrderLine>,
}

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_entity(table = "order_lines", plural = "OrderLines", default_sort = "sku ASC")]
struct OrderLine {
    #[primary_key]
    id: Uuid,
    #[filterable(type = "uuid")]
    order_id: Uuid,
    #[filterable(type = "string")]
    #[sortable]
    sku: String,
    #[graphql(skip)]
    #[relation(
        target = "Order",
        from = "order_id",
        to = "id",
        on_delete = "cascade",
        propagate_change = "up"
    )]
    order: Option<Order>,
}

// A parent whose child uses the default delete policy.
#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_entity(table = "warehouses", plural = "Warehouses", default_sort = "name ASC")]
struct Warehouse {
    #[primary_key]
    id: Uuid,
    #[sortable]
    name: String,
    #[graphql(skip)]
    #[relation(target = "Bin", from = "id", to = "warehouse_id", multiple)]
    bins: Vec<Bin>,
}

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_entity(table = "bins", plural = "Bins", default_sort = "label ASC")]
struct Bin {
    #[primary_key]
    id: Uuid,
    #[filterable(type = "uuid")]
    warehouse_id: Uuid,
    #[sortable]
    label: String,
    #[graphql(skip)]
    #[relation(target = "Warehouse", from = "warehouse_id", to = "id")]
    warehouse: Option<Warehouse>,
}

batch_load!(Customer, Order, OrderLine, Warehouse, Bin, Account);

// A generated object: no relation fields, so it can serve NodeId and RecordVersion.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "tickets",
    plural = "Tickets",
    default_sort = "title ASC",
    node_id,
    upsert = "title",
    aggregate = true,
    keyset = "title asc, id asc"
)]
#[graphql_orm(search(index = true))]
struct Ticket {
    #[primary_key]
    id: Uuid,
    #[filterable(type = "string")]
    #[sortable]
    #[unique]
    #[graphql_orm(searchable(weight = "A"))]
    title: String,
    #[filterable(type = "string")]
    note: String,
    #[filterable(type = "int64")]
    #[sortable]
    weight: i64,
    #[sortable]
    rank: i64,
    #[graphql_orm(version = "managed", default = "0")]
    row_version: i64,
}

// Narrowed generated surface.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "audit_notes",
    plural = "AuditNotes",
    default_sort = "body ASC",
    query_shapes = "single"
)]
struct AuditNote {
    #[primary_key]
    id: Uuid,
    #[filterable(type = "string")]
    #[sortable]
    body: String,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "ledger_entries",
    plural = "LedgerEntries",
    default_sort = "memo ASC",
    append_only = true
)]
struct LedgerEntry {
    #[primary_key]
    id: Uuid,
    #[filterable(type = "string")]
    #[sortable]
    memo: String,
}

// Private and sensitive fields on the two object forms.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "profiles", plural = "Profiles", default_sort = "name ASC")]
struct Profile {
    #[primary_key]
    id: Uuid,
    #[sortable]
    name: String,
    #[graphql_orm(private)]
    hidden_note: String,
    #[graphql_orm(sensitive)]
    flagged_note: String,
}

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_entity(table = "accounts", plural = "Accounts", default_sort = "name ASC")]
struct Account {
    #[primary_key]
    id: Uuid,
    #[sortable]
    name: String,
    #[graphql_orm(private)]
    private_note: String,
    #[filterable(type = "uuid")]
    customer_id: Uuid,
    #[graphql(skip)]
    #[relation(target = "Customer", from = "customer_id", to = "id")]
    customer: Option<Customer>,
}

// Typed persistence with no GraphQL surface.
#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "sqlite",
    table = "credentials",
    plural = "Credentials",
    default_sort = "username ASC"
)]
struct Credential {
    #[primary_key]
    id: Uuid,
    #[sortable]
    username: String,
}

// Declares policies, so it needs the matching providers.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "secured",
    plural = "Secureds",
    default_sort = "name ASC",
    read_policy = "secured.read",
    write_policy = "secured.write"
)]
struct Secured {
    #[primary_key]
    id: Uuid,
    #[filterable(type = "string")]
    #[sortable]
    name: String,
}

/// Same names as generated operations, to show which resolver runs.
#[derive(Default)]
struct TwinMutations;

#[graphql_orm_custom_operations(kind = "mutation", authorization = true)]
#[Object]
impl TwinMutations {
    /// Collides with the generated `deleteOrder`.
    async fn delete_order(&self, id: async_graphql::ID) -> String {
        format!("custom:{}", id.as_str())
    }

    /// A custom resolver calling a repository helper.
    async fn rename_secured(&self, ctx: &Context<'_>, id: Uuid) -> async_graphql::Result<bool> {
        let db = ctx.data::<Database<SqliteBackend>>()?;
        Ok(Secured::update_by_id(
            db,
            &id,
            UpdateSecuredInput {
                name: Some("renamed".into()),
            },
        )
        .await
        .map_err(|error| async_graphql::Error::new(error.to_string()))?
        .is_some())
    }
}

#[derive(Default)]
struct TwinQueries;

#[graphql_orm_custom_operations(kind = "query", authorization = true)]
#[Object]
impl TwinQueries {
    /// Collides with the generated `orders`.
    async fn orders(&self) -> String {
        "custom".into()
    }
}

schema_roots! {
    backend: "sqlite",
    schema_policy: "managed",
    generated_mutations: "denylist",
    generated_mutation_denylist: [Warehouse],
    query_custom_ops: [],
    described_query_types: [TwinQueries],
    described_mutation_types: [TwinMutations],
    entities: [Customer, Order, OrderLine, Warehouse, Bin, Ticket, AuditNote, LedgerEntry, Profile, Account, Secured],
}

type Log = Arc<Mutex<Vec<String>>>;

fn take(log: &Log) -> Vec<String> {
    std::mem::take(&mut log.lock().unwrap())
}

#[derive(Clone, Default)]
struct RecordingHook {
    log: Log,
    fail_deferred: bool,
    delete_lines_in_hook: bool,
}

impl MutationHook for RecordingHook {
    fn on_mutation<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            self.log.lock().unwrap().push(format!(
                "{:?} {:?} {} ctx={}",
                event.phase,
                event.action,
                event.entity_name,
                ctx.is_some()
            ));
            if self.delete_lines_in_hook
                && event.entity_name == "Order"
                && event.action == ChangeAction::Deleted
                && event.phase == MutationPhase::Before
            {
                let order = event.before::<Order>()?.expect("preimage");
                hook_ctx
                    .delete_where::<OrderLine>(OrderLineWhereInput {
                        order_id: Some(UuidFilter {
                            eq: Some(order.id),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .await
                    .map_err(|error| async_graphql::Error::new(error.to_string()))?;
            }
            if event.phase == MutationPhase::After {
                let log = self.log.clone();
                let fail = self.fail_deferred;
                hook_ctx.defer(move |_db| async move {
                    log.lock().unwrap().push("deferred".into());
                    if fail { Err("deferred failed") } else { Ok(()) }
                });
            }
            Ok(())
        })
    }
}

#[derive(Clone, Default)]
struct RecordingTransform(Log);

impl RecordingTransform {
    fn note(&self, what: String) {
        self.0.lock().unwrap().push(what);
    }
}

impl WriteInputTransform for RecordingTransform {
    fn before_create_with_context<'a>(
        &'a self,
        write_ctx: &'a mut WriteInputContext<'_, '_>,
        _input: &'a mut (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            self.note(format!("create {}", write_ctx.entity_name()));
            Ok(())
        })
    }

    fn before_update_with_context<'a>(
        &'a self,
        write_ctx: &'a mut WriteInputContext<'_, '_>,
        _existing: Option<&'a (dyn Any + Send + Sync)>,
        _input: &'a mut (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            self.note(format!("update {}", write_ctx.entity_name()));
            Ok(())
        })
    }
}

#[derive(Clone, Default)]
struct RecordingEntityPolicy(Log);

impl EntityPolicy for RecordingEntityPolicy {
    fn can_access_entity<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database,
        entity_name: &'static str,
        _policy_key: Option<&'static str>,
        kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(format!(
                "{entity_name} {kind:?} {surface:?} ctx={}",
                ctx.is_some()
            ));
            Ok(true)
        })
    }
}

fn entities() -> [&'static EntityMetadata; 12] {
    [
        Customer::metadata(),
        Order::metadata(),
        OrderLine::metadata(),
        Warehouse::metadata(),
        Bin::metadata(),
        Ticket::metadata(),
        AuditNote::metadata(),
        LedgerEntry::metadata(),
        Profile::metadata(),
        Account::metadata(),
        Credential::metadata(),
        Secured::metadata(),
    ]
}

async fn database() -> Database<SqliteBackend> {
    let pool = graphql_orm::sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    graphql_orm::sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();
    let db = Database::<SqliteBackend>::new(pool).with_schema_policy(SchemaPolicy::Managed);
    let plan = db
        .schema()
        .plan_migration_to_entities("contract", "contract", &entities())
        .await
        .unwrap();
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .unwrap();
    db
}

fn as_user(query: impl Into<String>) -> Request {
    Request::new(query.into()).data(AuthSubject::new("user-1"))
}

async fn seed(db: &Database<SqliteBackend>) -> (Customer, Order, OrderLine) {
    let customer = Customer::insert(
        db,
        CreateCustomerInput {
            name: "Acme".into(),
        },
    )
    .await
    .unwrap();
    let order = Order::insert(
        db,
        CreateOrderInput {
            customer_id: customer.id,
            reference: "A-1".into(),
        },
    )
    .await
    .unwrap();
    let line = OrderLine::insert(
        db,
        CreateOrderLineInput {
            order_id: order.id,
            sku: "sku-1".into(),
        },
    )
    .await
    .unwrap();
    (customer, order, line)
}

#[tokio::test]
async fn every_generated_write_path_runs_hooks_and_only_graphql_has_a_context() {
    let hook = RecordingHook::default();
    let log = hook.log.clone();
    let mut db = database().await;
    db.set_mutation_hook(hook);
    let schema = schema_builder(db.clone()).finish();
    let (_, order, line) = seed(&db).await;
    take(&log);

    let response = schema
        .execute(as_user(format!(
            r#"mutation {{ updateOrderLine(id: "{}", input: {{ sku: "sku-2" }}) {{ success }} }}"#,
            line.id
        )))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        take(&log),
        [
            "Before Updated OrderLine ctx=true",
            "After Updated OrderLine ctx=true",
            "deferred"
        ]
    );

    let bulk = schema
        .execute(as_user(
            r#"mutation { updateOrderLines(where: { sku: { eq: "sku-2" } }, input: { sku: "sku-3" }) { success } }"#,
        ))
        .await;
    assert!(bulk.errors.is_empty(), "{:?}", bulk.errors);
    assert_eq!(
        take(&log),
        [
            "Before Updated OrderLine ctx=true",
            "After Updated OrderLine ctx=true",
            "deferred"
        ]
    );

    OrderLine::update_by_id(
        &db,
        &line.id,
        UpdateOrderLineInput {
            sku: Some("sku-4".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        take(&log),
        [
            "Before Updated OrderLine ctx=false",
            "After Updated OrderLine ctx=false",
            "deferred"
        ]
    );

    let order_id = order.id;
    db.transaction(TransactionMode::Default, |tx| {
        Box::pin(async move {
            tx.update_by_id::<Order>(
                &order_id,
                UpdateOrderInput {
                    reference: Some("A-2".into()),
                    ..Default::default()
                },
            )
            .await
            .map_err(Into::into)
        })
    })
    .await
    .map_err(|error| format!("{error:?}"))
    .unwrap();
    assert_eq!(
        take(&log),
        [
            "Before Updated Order ctx=false",
            "After Updated Order ctx=false",
            "deferred"
        ]
    );

    let ticket = Ticket::upsert(
        &db,
        CreateTicketInput {
            title: "t".into(),
            note: "n".into(),
            weight: 1,
            rank: 1,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        take(&log),
        [
            "Before Created Ticket ctx=false",
            "After Created Ticket ctx=false",
            "deferred"
        ]
    );
    let swapped = Ticket::compare_and_swap(
        &db,
        &ticket.entity.id,
        0,
        TicketWhereInput::default(),
        UpdateTicketInput {
            note: Some("cas".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(matches!(swapped, ConditionalUpdateOutcome::Updated(_)));
    assert_eq!(
        take(&log),
        [
            "Before Updated Ticket ctx=false",
            "After Updated Ticket ctx=false",
            "deferred"
        ]
    );
}

#[tokio::test]
async fn repository_bulk_helpers_run_hooks_per_row() {
    let hook = RecordingHook::default();
    let log = hook.log.clone();
    let mut db = database().await;
    db.set_mutation_hook(hook);
    let ticket = |title: &str| CreateTicketInput {
        title: title.into(),
        note: "n".into(),
        weight: 1,
        rank: 1,
    };
    let count = |log: &Log, entry: &str| take(log).iter().filter(|line| *line == entry).count();

    Ticket::insert_many(&db, vec![ticket("a"), ticket("b")])
        .await
        .unwrap();
    assert_eq!(count(&log, "After Created Ticket ctx=false"), 2);

    Ticket::update_where(
        &db,
        TicketWhereInput {
            note: Some(StringFilter {
                eq: Some("n".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        UpdateTicketInput {
            note: Some("m".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(count(&log, "After Updated Ticket ctx=false"), 2);

    Ticket::delete_where(
        &db,
        TicketWhereInput {
            title: Some(StringFilter {
                eq: Some("a".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(count(&log, "After Deleted Ticket ctx=false"), 1);

    Ticket::replace_all(&db, vec![ticket("c")]).await.unwrap();
    let replaced = take(&log);
    assert!(replaced.contains(&"After Deleted Ticket ctx=false".to_string()));
    assert!(replaced.contains(&"After Created Ticket ctx=false".to_string()));

    Ticket::delete_all(&db).await.unwrap();
    assert_eq!(count(&log, "After Deleted Ticket ctx=false"), 1);
}

#[tokio::test]
async fn writes_from_a_hook_run_hooks_and_deferred_work_precedes_the_response() {
    let hook = RecordingHook {
        delete_lines_in_hook: true,
        ..Default::default()
    };
    let log = hook.log.clone();
    let mut db = database().await;
    db.set_mutation_hook(hook);
    let schema = schema_builder(db.clone()).finish();
    let (_, order, _) = seed(&db).await;
    take(&log);

    let response = schema
        .execute(as_user(format!(
            r#"mutation {{ deleteOrder(id: "{}") {{ success }} }}"#,
            order.id
        )))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    // Deferred actions have already run when the mutation returns, and the
    // nested delete reports no GraphQL context.
    assert_eq!(
        take(&log),
        [
            "Before Deleted Order ctx=true",
            "Before Deleted OrderLine ctx=false",
            "After Deleted OrderLine ctx=false",
            "After Deleted Order ctx=true",
            "deferred",
            "deferred"
        ]
    );
}

#[tokio::test]
async fn a_failed_deferred_action_does_not_fail_the_committed_mutation() {
    let hook = RecordingHook {
        fail_deferred: true,
        ..Default::default()
    };
    let mut db = database().await;
    db.set_mutation_hook(hook);
    let schema = schema_builder(db.clone()).finish();
    let (_, order, _) = seed(&db).await;

    let response = schema
        .execute(as_user(format!(
            r#"mutation {{ deleteOrder(id: "{}") {{ success }} }}"#,
            order.id
        )))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(Order::count_all(&db).await.unwrap(), 0);
}

#[tokio::test]
async fn database_cascades_run_no_dependent_hooks_or_changed_events() {
    let hook = RecordingHook::default();
    let log = hook.log.clone();
    let mut db = database().await;
    db.set_mutation_hook(hook);
    let schema = schema_builder(db.clone()).finish();
    let (customer, _, _) = seed(&db).await;
    take(&log);
    let mut orders = db.ensure_event_sender::<OrderChangedEvent>().subscribe();
    let mut lines = db
        .ensure_event_sender::<OrderLineChangedEvent>()
        .subscribe();

    let response = schema
        .execute(as_user(format!(
            r#"mutation {{ deleteCustomer(id: "{}") {{ success }} }}"#,
            customer.id
        )))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(Order::count_all(&db).await.unwrap(), 0);
    assert_eq!(OrderLine::count_all(&db).await.unwrap(), 0);
    assert_eq!(
        take(&log),
        [
            "Before Deleted Customer ctx=true",
            "After Deleted Customer ctx=true",
            "deferred"
        ]
    );
    assert!(orders.try_recv().is_err());
    assert!(lines.try_recv().is_err());
}

#[tokio::test]
async fn the_default_delete_policy_refuses_a_parent_with_children() {
    let db = database().await;
    let warehouse = Warehouse::insert(&db, CreateWarehouseInput { name: "w".into() })
        .await
        .unwrap();
    Bin::insert(
        &db,
        CreateBinInput {
            warehouse_id: warehouse.id,
            label: "b".into(),
        },
    )
    .await
    .unwrap();
    assert!(Warehouse::delete_by_id(&db, &warehouse.id).await.is_err());
    assert_eq!(Warehouse::count_all(&db).await.unwrap(), 1);
}

#[tokio::test]
async fn a_custom_root_with_a_generated_name_never_runs() {
    let db = database().await;
    let schema = schema_builder(db.clone()).finish();
    let (_, order, _) = seed(&db).await;

    // The custom resolvers return String, but the schema exposes the generated types.
    let query = schema
        .execute(as_user("query { orders { edges { node { reference } } } }"))
        .await;
    assert!(query.errors.is_empty(), "{:?}", query.errors);
    let mutation = schema
        .execute(as_user(format!(
            r#"mutation {{ deleteOrder(id: "{}") {{ success }} }}"#,
            order.id
        )))
        .await;
    assert!(mutation.errors.is_empty(), "{:?}", mutation.errors);
    assert_eq!(Order::count_all(&db).await.unwrap(), 0);

    // The only signal is a panic from the semantic catalogue.
    assert!(std::panic::catch_unwind(graphql_orm_semantic_catalog).is_err());
}

#[tokio::test]
async fn transforms_run_twice_on_repository_insert_and_once_for_a_bulk_update() {
    let transform = RecordingTransform::default();
    let log = transform.0.clone();
    let mut db = database().await;
    db.set_write_input_transform(transform);
    let schema = schema_builder(db.clone()).finish();

    let customer = Customer::insert(
        &db,
        CreateCustomerInput {
            name: "Acme".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(take(&log), ["create Customer", "create Customer"]);

    let order = Order::insert(
        &db,
        CreateOrderInput {
            customer_id: customer.id,
            reference: "A-1".into(),
        },
    )
    .await
    .unwrap();
    take(&log);
    for sku in ["same", "same"] {
        OrderLine::insert(
            &db,
            CreateOrderLineInput {
                order_id: order.id,
                sku: sku.into(),
            },
        )
        .await
        .unwrap();
    }
    take(&log);

    let bulk = schema
        .execute(as_user(
            r#"mutation { updateOrderLines(where: { sku: { eq: "same" } }, input: { sku: "x" }) { success } }"#,
        ))
        .await;
    assert!(bulk.errors.is_empty(), "{:?}", bulk.errors);
    assert_eq!(take(&log), ["update OrderLine"]);

    let graphql = schema
        .execute(as_user(format!(
            r#"mutation {{ createOrderLine(input: {{ orderId: "{}", sku: "y" }}) {{ success }} }}"#,
            order.id
        )))
        .await;
    assert!(graphql.errors.is_empty(), "{:?}", graphql.errors);
    assert_eq!(take(&log), ["create OrderLine"]);

    assert!(Order::delete_by_id(&db, &order.id).await.unwrap());
    assert!(take(&log).is_empty(), "transforms do not run for deletes");
}

#[tokio::test]
async fn declared_field_validation_applies_to_graphql_and_repository_writes() {
    let db = database().await;
    let schema = schema_builder(db.clone()).finish();
    let graphql = schema
        .execute(as_user(
            r#"mutation { createCustomer(input: { name: "x" }) { success error } }"#,
        ))
        .await;
    let rejected =
        !graphql.errors.is_empty() || graphql.data.to_string().contains("success: false");
    assert!(rejected, "{:?}", graphql.data);
    assert!(
        Customer::insert(&db, CreateCustomerInput { name: "x".into() })
            .await
            .is_err()
    );
    assert_eq!(Customer::count_all(&db).await.unwrap(), 0);
}

#[tokio::test]
async fn an_entity_without_policies_is_open_to_any_authenticated_subject() {
    let db = database().await;
    let schema = schema_builder(db.clone()).finish();
    let create = r#"mutation { createCustomer(input: { name: "Acme" }) { success } }"#;

    let anonymous = schema.execute(create).await;
    let code = anonymous.errors[0]
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.get("code").cloned());
    assert_eq!(code, Some(async_graphql::Value::from("UNAUTHENTICATED")));

    let authenticated = schema.execute(as_user(create)).await;
    assert!(
        authenticated.errors.is_empty(),
        "{:?}",
        authenticated.errors
    );
    assert_eq!(Customer::count_all(&db).await.unwrap(), 1);
}

#[tokio::test]
async fn a_declared_policy_key_needs_entity_and_row_providers() {
    let db = database().await;
    let schema = schema_builder(db.clone()).finish();
    let create = r#"mutation { createSecured(input: { name: "a" }) { success } }"#;

    let no_provider = schema.execute(as_user(create)).await;
    assert_eq!(
        no_provider.errors[0].message,
        "authorization is misconfigured"
    );

    let mut with_entity_policy = db.clone();
    with_entity_policy.set_entity_policy(RecordingEntityPolicy::default());
    let secured = Secured::insert(&with_entity_policy, CreateSecuredInput { name: "a".into() })
        .await
        .unwrap();
    let error = Secured::update_by_id(
        &with_entity_policy,
        &secured.id,
        UpdateSecuredInput {
            name: Some("b".into()),
        },
    )
    .await
    .unwrap_err();
    assert!(
        error.to_string().contains("AUTHORIZATION_MISCONFIGURED"),
        "{error}"
    );
}

#[tokio::test]
async fn custom_resolvers_reach_policies_with_no_request_context() {
    let policy = RecordingEntityPolicy::default();
    let log = policy.0.clone();
    let mut db = database().await;
    db.set_entity_policy(policy);
    let schema = schema_builder(db.clone()).finish();
    let secured = Secured::insert(&db, CreateSecuredInput { name: "a".into() })
        .await
        .unwrap();
    take(&log);

    let _ = schema
        .execute(as_user(format!(
            r#"mutation {{ renameSecured(id: "{}") }}"#,
            secured.id
        )))
        .await;
    assert_eq!(take(&log), ["Secured Write Repository ctx=false"]);
}

#[tokio::test]
async fn the_generated_mutation_denylist_keeps_repository_writes_and_changed() {
    let db = database().await;
    let sdl = schema_builder(db.clone()).finish().sdl();
    assert!(!sdl.contains("createWarehouse("));
    assert!(!sdl.contains("deleteWarehouse("));
    assert!(sdl.contains("warehouseChanged("));
    assert!(sdl.contains("createBin("));
    assert!(
        Warehouse::insert(&db, CreateWarehouseInput { name: "w".into() })
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn filterable_fields_get_an_index_and_sortable_fields_do_not() {
    let db = database().await;
    let plan = Database::<SqliteBackend>::new(
        graphql_orm::sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap(),
    )
    .with_schema_policy(SchemaPolicy::Managed)
    .schema()
    .plan_migration_to_entities("indexes", "indexes", &entities())
    .await
    .unwrap();
    drop(db);
    let indexed = |index: &str| plan.statements.iter().any(|sql| sql.contains(index));
    assert!(indexed("idx_customers_name"));
    assert!(indexed("idx_tickets_note"));
    assert!(!indexed("idx_orders_reference"));
    assert!(!indexed("idx_tickets_rank"));
}

#[tokio::test]
async fn generated_names_and_surface_match_the_skill() {
    let db = database().await;
    let sdl = schema_builder(db).finish().sdl();
    for name in [
        "orders(",
        "order(",
        "createOrder(",
        "updateOrder(",
        "updateOrders(",
        "deleteOrder(",
        "deleteOrders(",
        "orderChanged(",
        "upsertTicket(",
        "ticketsKeyset(",
        "ticketsAggregate(",
        "ticketsSearch(",
        "expectedVersion: Int",
        "keys: [",
    ] {
        assert!(sdl.contains(name), "{name} missing from the SDL");
    }
    assert!(sdl.contains("createLedgerEntry("));
    assert!(
        !sdl.contains("updateLedgerEntry("),
        "append_only removes updates"
    );
    assert!(
        !sdl.contains("deleteLedgerEntry("),
        "append_only removes deletes"
    );
    assert!(
        sdl.contains("auditNote("),
        "query_shapes keeps the single read"
    );
    assert!(
        !sdl.contains("auditNotes("),
        "query_shapes removes the list"
    );
}

#[tokio::test]
async fn record_version_and_node_id_need_a_generated_object() {
    let db = database().await;
    let sdl = schema_builder(db).finish().sdl();
    let object = |name: &str| {
        let start = sdl.find(&format!("type {name} ")).unwrap();
        let end = sdl[start..].find("\n}").unwrap();
        sdl[start..start + end].to_string()
    };
    assert!(object("Ticket").contains("RecordVersion: Int!"));
    assert!(object("Ticket").contains("NodeId: ID!"));
    assert!(!object("Order").contains("RecordVersion"));
    assert!(!object("Order").contains("NodeId"));
    assert_eq!(Ticket::managed_version_field(), Some("rowVersion"));
    assert_eq!(Order::managed_version_field(), None);
}

#[tokio::test]
async fn propagation_marks_the_parent_changed_and_payloads_are_persisted_scalars() {
    let db = database().await;
    let schema = schema_builder(db.clone()).finish();
    let (_, order, line) = seed(&db).await;

    let mut parent = schema.execute_stream(as_user(format!(
        r#"subscription {{ orderChanged(key: "{}") {{ action changeKind id sourceEntity }} }}"#,
        order.id
    )));
    let write = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        OrderLine::update_by_id(
            &db,
            &line.id,
            UpdateOrderLineInput {
                sku: Some("sku-2".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    };
    let (event, ()) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(5), parent.next()),
        write
    );
    let event = event.unwrap().unwrap();
    assert!(event.errors.is_empty(), "{:?}", event.errors);
    let data = event.data.into_json().unwrap();
    assert_eq!(data["orderChanged"]["changeKind"], "PROPAGATED");
    assert_eq!(data["orderChanged"]["id"], order.id.to_string());
    assert_eq!(data["orderChanged"]["sourceEntity"], "OrderLine");

    let mut nested = schema.execute_stream(as_user(
        "subscription { orderChanged { order { lines { edges { node { sku } } } } } }",
    ));
    let rejected = tokio::time::timeout(Duration::from_secs(5), nested.next())
        .await
        .unwrap()
        .unwrap();
    assert!(!rejected.errors.is_empty());
    let code = rejected.errors[0]
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.get("code").cloned());
    assert_eq!(code, Some(async_graphql::Value::from("INVALID_INPUT")));
}

#[derive(Clone)]
struct DeletedPreimageRows;

impl RowPolicy for DeletedPreimageRows {
    fn can_read_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: EntityAccessSurface,
        _row: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }

    fn can_write_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: EntityAccessSurface,
        _row: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }

    fn can_read_deleted_preimage<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: EntityAccessSurface,
        _row: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

async fn next_after_delete(
    db: &Database<SqliteBackend>,
    subscription: &str,
    line: Uuid,
) -> Option<serde_json::Value> {
    let schema = schema_builder(db.clone()).finish();
    let mut stream = schema.execute_stream(as_user(subscription));
    let write = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(OrderLine::delete_by_id(db, &line).await.unwrap());
    };
    let (event, ()) = tokio::join!(
        tokio::time::timeout(Duration::from_millis(400), stream.next()),
        write
    );
    event
        .ok()
        .flatten()
        .map(|event| event.data.into_json().unwrap())
}

#[tokio::test]
async fn a_deleted_row_reaches_subscribers_only_with_the_preimage_opt_in() {
    let db = database().await;
    let (_, _, line) = seed(&db).await;
    let subscription = "subscription { orderLineChanged { action id } }";
    assert_eq!(next_after_delete(&db, subscription, line.id).await, None);

    let mut opted_in = database().await;
    opted_in.set_row_policy(DeletedPreimageRows);
    let (_, _, line) = seed(&opted_in).await;
    let event = next_after_delete(&opted_in, subscription, line.id).await;
    assert_eq!(event.unwrap()["orderLineChanged"]["action"], "DELETED");
}

#[tokio::test]
async fn a_deleted_child_does_not_propagate_to_its_parent() {
    let mut db = database().await;
    db.set_row_policy(DeletedPreimageRows);
    let (_, order, line) = seed(&db).await;
    let subscription = format!(
        r#"subscription {{ orderChanged(key: "{}") {{ action changeKind }} }}"#,
        order.id
    );
    assert_eq!(next_after_delete(&db, &subscription, line.id).await, None);
}

#[tokio::test]
async fn touching_a_parent_in_a_child_hook_notifies_its_subscribers() {
    #[derive(Clone)]
    struct TouchParent;

    impl MutationHook for TouchParent {
        fn on_mutation<'a>(
            &'a self,
            _ctx: Option<&'a async_graphql::Context<'_>>,
            hook_ctx: &'a mut MutationContext<'_>,
            event: &'a MutationEvent,
        ) -> BoxFuture<'a, async_graphql::Result<()>> {
            Box::pin(async move {
                let line = event.before::<OrderLine>().ok().flatten();
                if let (MutationPhase::After, ChangeAction::Deleted, Some(line)) =
                    (&event.phase, event.action, line)
                {
                    hook_ctx
                        .update_by_id::<Order>(&line.order_id, UpdateOrderInput::default())
                        .await
                        .map_err(|error| async_graphql::Error::new(error.to_string()))?;
                }
                Ok(())
            })
        }
    }

    let mut db = database().await;
    db.set_mutation_hook(TouchParent);
    let (_, order, line) = seed(&db).await;
    let subscription = format!(
        r#"subscription {{ orderChanged(key: "{}") {{ action }} }}"#,
        order.id
    );
    let event = next_after_delete(&db, &subscription, line.id).await;
    assert_eq!(event.unwrap()["orderChanged"]["action"], "UPDATED");
}

#[tokio::test]
async fn callback_only_row_policies_need_a_scan_budget_for_paged_reads() {
    let mut db = database().await;
    db.set_row_policy(DeletedPreimageRows);
    seed(&db).await;
    let query = "query { orders { edges { node { reference } } } }";

    let schema = schema_builder(db.clone()).finish();
    let refused = schema.execute(as_user(query)).await;
    assert_eq!(refused.errors[0].message, "authorization is misconfigured");

    let budgeted = db.with_authorized_scan_config(AuthorizedScanConfig::new([7; 32], 50, 1_000));
    let schema = schema_builder(budgeted).finish();
    let allowed = schema.execute(as_user(query)).await;
    assert!(allowed.errors.is_empty(), "{:?}", allowed.errors);
}

#[tokio::test]
async fn private_hides_fields_only_on_generated_objects_and_sensitive_hides_nothing() {
    let db = database().await;
    let sdl = schema_builder(db).finish().sdl();
    let object = |name: &str| {
        let start = sdl.find(&format!("type {name} ")).unwrap();
        let end = sdl[start..].find("\n}").unwrap();
        sdl[start..start + end].to_string()
    };
    assert!(!object("Profile").contains("hiddenNote"));
    assert!(object("Profile").contains("flaggedNote"));
    assert!(object("Account").contains("privateNote"));
}

#[tokio::test]
async fn repository_entities_persist_without_a_graphql_surface() {
    let db = database().await;
    let credential = Credential::insert(
        &db,
        CreateCredentialInput {
            username: "svc".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(credential.username, "svc");
    let sdl = schema_builder(db).finish().sdl();
    assert!(!sdl.to_lowercase().contains("credential"));
}

#[tokio::test]
async fn a_migration_plan_drops_entities_left_out_of_it() {
    let db = database().await;
    let partial = db
        .schema()
        .plan_migration_to_entities("partial", "partial", &[Customer::metadata()])
        .await
        .unwrap();
    assert!(
        partial
            .statements
            .iter()
            .any(|sql| sql.contains("DROP TABLE")),
        "{:?}",
        partial.statements
    );
    assert!(
        db.schema()
            .apply_migration(&partial, ApplyOptions::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn additive_only_rejects_a_plan_with_a_non_additive_step() {
    let db = database().await;
    let partial = db
        .schema()
        .plan_migration_to_entities("partial", "partial", &[Customer::metadata()])
        .await
        .unwrap();
    let options = ApplyOptions {
        additive_only: true,
        allow_destructive: true,
        ..Default::default()
    };
    assert!(
        db.schema()
            .apply_migration(&partial, options)
            .await
            .is_err()
    );
    assert_eq!(Order::count_all(&db).await.unwrap(), 0);
}

#[tokio::test]
async fn scope_policies_refuse_hook_writes_and_apply_to_every_entity() {
    let hook = RecordingHook {
        delete_lines_in_hook: true,
        ..Default::default()
    };
    let mut db = database().await;
    let (customer, order, _) = seed(&db).await;
    db.set_mutation_hook(hook);
    db.set_entity_policy(ScopeEntityPolicy::new(&["all.read"], &["all.write"]));
    let schema = schema_builder(db.clone()).finish();
    let scoped = |query: String, scopes: Vec<String>| {
        Request::new(query).data(AuthSubject::builder("user-1").scopes(scopes).build())
    };

    let delete = format!(
        r#"mutation {{ deleteOrder(id: "{}") {{ success }} }}"#,
        order.id
    );
    let refused = schema
        .execute(scoped(delete, vec!["all.write".into()]))
        .await;
    let text = format!("{:?} {}", refused.errors, refused.data);
    assert!(text.contains("unauthenticated"), "{text}");

    // Customer declares no policy key, and the scope still applies.
    let read = format!(r#"query {{ customer(id: "{}") {{ name }} }}"#, customer.id);
    let denied = schema.execute(scoped(read, Vec::new())).await;
    assert!(!denied.errors.is_empty());
}

#[tokio::test]
async fn an_upsert_needs_a_state_machine_transaction() {
    let db = database().await;
    let input = || CreateTicketInput {
        title: "t".into(),
        note: "n".into(),
        weight: 1,
        rank: 1,
    };
    let default = db
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move { tx.upsert::<Ticket>(input()).await.map_err(Into::into) })
        })
        .await;
    assert!(default.is_err());
    let machine = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move { tx.upsert::<Ticket>(input()).await.map_err(Into::into) })
        })
        .await;
    assert!(machine.is_ok());
}

#[test]
fn collection_filter_opt_in_is_typed_and_read_only() {
    let filter = CustomerWhereInput {
        orders: Some(vec![]),
        ..Default::default()
    };
    assert!(!filter.is_empty());
    assert!(filter.validate().is_err());
    let entity = Customer {
        id: Uuid::new_v4(),
        name: "Reader".into(),
        orders: vec![],
    };
    assert!(!filter.matches_entity(&entity).unwrap());
    assert!(filter.__gom_require_scalar_write_filter().is_err());
}
