//! Generated nested create on the three writable backends.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]

#[cfg(feature = "mssql")]
#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use graphql_orm::futures::future::BoxFuture;
use graphql_orm::prelude::*;
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
#[cfg_attr(
    not(feature = "mssql"),
    graphql_entity(table = "nested_orders", plural = "Orders")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        table = "nested_orders",
        plural = "Orders",
        schema_policy = "external_writable"
    )
)]
#[graphql(complex)]
struct Order {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    id: String,
    tenant: String,
    #[relation(target = "Line", from = ["id", "tenant"], to = ["order_id", "tenant"], multiple, nested_create, emit_fk = false)]
    #[graphql(skip)]
    #[serde(default)]
    lines: Vec<Line>,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(not(feature = "mssql"), graphql_entity(table = "nested_lines", plural = "Lines", nested_create_keys = ["order_id", "tenant"]))]
#[cfg_attr(feature = "mssql", graphql_entity(table = "nested_lines", plural = "Lines", nested_create_keys = ["order_id", "tenant"], schema_policy = "external_writable"))]
#[graphql_orm(operation_authorization(categories = ["create"], all_scopes = ["lines.write"]))]
struct Line {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    id: String,
    order_id: String,
    note: Option<String>,
    tenant: String,
    #[graphql_orm(version = "managed", default = "0")]
    version: i64,
}

#[cfg(not(feature = "mssql"))]
schema_roots! { query_custom_ops: [], entities: [Order, Line] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [Order, Line] }

struct DenyOneLine;
impl RowPolicy for DenyOneLine {
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_write_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            let Some(line) = row.downcast_ref::<Line>() else {
                return Ok(true);
            };
            let ctx = ctx.expect("nested row policy receives the request context");
            assert_eq!(ctx.field().name(), "createLine");
            let input = ctx
                .field()
                .arguments()
                .unwrap()
                .into_iter()
                .find_map(|(name, value)| (name == "input").then_some(value))
                .unwrap();
            let async_graphql::Value::Object(fields) = input else {
                panic!("child policy expects typed create fields");
            };
            assert_eq!(
                fields.get("tenant"),
                Some(&async_graphql::Value::from("west"))
            );
            Ok(line.id != "denied")
        })
    }
}

struct Trace(Arc<Mutex<Vec<String>>>);
impl MutationHook for Trace {
    fn on_mutation<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        tx: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            tx.emit_queued_events();
            self.0
                .lock()
                .unwrap()
                .push(format!("{:?}:{}", event.phase, event.id));
            if event.phase == MutationPhase::After {
                let trace = self.0.clone();
                let id = event.id.clone();
                tx.defer(move |_| async move {
                    trace.lock().unwrap().push(format!("committed:{id}"));
                    Ok::<(), std::convert::Infallible>(())
                });
            }
            Ok(())
        })
    }
}

struct TransformLine;
impl WriteInputTransform for TransformLine {
    fn before_create<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if let Some(line) = input.downcast_mut::<CreateLineInput>() {
                if line.id == "rewritten" {
                    line.tenant = "other".to_owned();
                }
                line.note = Some("normalized".to_owned());
            }
            Ok(())
        })
    }
}

async fn rows(db: &Database, table: &str) -> usize {
    DefaultBackend::fetch_rows(db.pool(), &format!("SELECT id FROM {table}"), &[])
        .await
        .unwrap()
        .len()
}

#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "owns a disposable database"
)]
async fn nested_create_is_bounded_authorized_and_atomic() {
    #[cfg(feature = "sqlite")]
    let mut db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    #[cfg(feature = "postgres")]
    let mut db = Database::new(
        owned_postgres::OwnedPostgres::pool("nested-create", 2)
            .await
            .unwrap(),
    );
    #[cfg(feature = "mssql")]
    let (mut db, mut owner) = {
        let owner = owned_mssql::OwnedSqlServer::start().await.unwrap();
        let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(
            &owner.connection_string,
        )
        .await
        .unwrap();
        (
            Database::builder(pool)
                .schema_policy(SchemaPolicy::ExternalWritable)
                .build(),
            owner,
        )
    };
    for sql in [
        "CREATE TABLE nested_orders (id VARCHAR(80) PRIMARY KEY, tenant VARCHAR(80) NOT NULL)",
        "CREATE TABLE nested_lines (id VARCHAR(80) PRIMARY KEY, order_id VARCHAR(80) NOT NULL REFERENCES nested_orders(id), tenant VARCHAR(80) NOT NULL, note VARCHAR(80) NULL, version BIGINT NOT NULL DEFAULT 0)",
    ] {
        <DefaultBackend as WriteBackend>::execute_with_binds(db.pool(), sql, &[])
            .await
            .unwrap();
    }
    let trace = Arc::new(Mutex::new(Vec::new()));
    db.set_mutation_hook(Trace(trace.clone()));
    db.set_row_policy(DenyOneLine);
    db.set_write_input_transform(TransformLine);
    let schema = schema_builder(db.clone())
        .data(
            AuthSubject::builder("operator")
                .scopes(vec!["lines.write".to_owned()])
                .build(),
        )
        .finish();
    let mut parents = db.ensure_event_sender::<OrderChangedEvent>().subscribe();
    let mut children_events = db.ensure_event_sender::<LineChangedEvent>().subscribe();
    let sdl = schema.sdl();
    assert!(sdl.contains("input CreateOrderWithChildrenInput"));
    assert!(sdl.contains("lines: [NestedCreateLineInput!]"));
    let operation = graphql_orm_operation_catalog()
        .operations()
        .iter()
        .find(|operation| operation.field_name() == "createOrder")
        .unwrap();
    assert_eq!(
        operation.arguments()[0].graphql_type(),
        "CreateOrderWithChildrenInput!"
    );
    let semantic = graphql_orm_semantic_catalog()
        .operations
        .iter()
        .find(|operation| operation.field_name == "createOrder")
        .unwrap();
    assert!(
        semantic.arguments[0]
            .description
            .contains("lines [NestedCreateLineInput!]")
    );

    let missing_child_scope = async_graphql::Request::new(
        r#"mutation { createOrder(input: { id: "order", tenant: "west", lines: [{ id: "one" }] }) { success } }"#,
    )
    .data(AuthSubject::new("operator"));
    let response = schema.execute(missing_child_scope).await;
    assert!(
        response
            .errors
            .iter()
            .any(|error| error.message.contains("forbidden")),
        "{response:?}"
    );
    assert_eq!(rows(&db, "nested_orders").await, 0);
    assert!(trace.lock().unwrap().is_empty());

    for (children, message) in [
        (r#"{ id: "one" }, { id: "denied" }"#.to_owned(), "forbidden"),
        (r#"{ id: "same" }, { id: "same" }"#.to_owned(), "duplicate"),
        (
            r#"{ id: "injected", orderId: "another" }"#.to_owned(),
            "orderId",
        ),
        (r#"{ id: "rewritten" }"#.to_owned(), "forbidden"),
        (
            (0..51)
                .map(|i| format!(r#"{{ id: "line-{i}" }}"#))
                .collect::<Vec<_>>()
                .join(", "),
            "50",
        ),
    ] {
        trace.lock().unwrap().clear();
        let response = schema.execute(format!(r#"mutation {{ createOrder(input: {{ id: "order", tenant: "west", lines: [{children}] }}) {{ success }} }}"#)).await;
        assert!(
            response
                .errors
                .iter()
                .any(|error| error.message.contains(message)),
            "{:?}",
            response.errors
        );
        assert_eq!(rows(&db, "nested_orders").await, 0);
        assert_eq!(rows(&db, "nested_lines").await, 0);
        assert!(parents.try_recv().is_err());
        assert!(children_events.try_recv().is_err());
        let log = trace.lock().unwrap();
        assert!(log.iter().all(|entry| !entry.starts_with("committed:")));
        if message != "forbidden" {
            assert!(log.is_empty(), "{log:?}");
        }
    }

    trace.lock().unwrap().clear();
    let injected = async_graphql::Request::new(
        "mutation ($data: CreateOrderWithChildrenInput!) { createOrder(input: $data) { success } }",
    )
    .variables(async_graphql::Variables::from_json(serde_json::json!({
        "data": {
            "id": "order", "tenant": "west", "lines": [{ "id": "line", "orderId": "foreign" }]
        }
    })));
    let response = schema.execute(injected).await;
    assert!(!response.errors.is_empty(), "{response:?}");
    assert_eq!(rows(&db, "nested_orders").await, 0);
    assert!(trace.lock().unwrap().is_empty());

    let response = schema.execute(r#"mutation { createOrder(input: { id: "order", tenant: "west", lines: [{ id: "one" }, { id: "two" }] }) { success } }"#).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(rows(&db, "nested_orders").await, 1);
    assert_eq!(rows(&db, "nested_lines").await, 2);
    assert_eq!(rows(&db, "nested_lines WHERE order_id = 'order' AND tenant = 'west' AND version = 0 AND note = 'normalized'").await, 2);
    assert_eq!(
        *trace.lock().unwrap(),
        [
            "Before:order",
            "After:order",
            "Before:one",
            "After:one",
            "Before:two",
            "After:two",
            "committed:order",
            "committed:one",
            "committed:two",
        ]
    );
    assert_eq!(parents.try_recv().unwrap().id, "order");
    assert_eq!(children_events.try_recv().unwrap().id, "one");
    assert_eq!(children_events.try_recv().unwrap().id, "two");
    assert!(parents.try_recv().is_err());
    assert!(children_events.try_recv().is_err());
    #[cfg(feature = "mssql")]
    owner.cleanup().unwrap();
}
