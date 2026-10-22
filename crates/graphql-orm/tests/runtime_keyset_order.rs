#![cfg(feature = "sqlite")]

use graphql_orm::prelude::*;
use serde_json::json;
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "runtime_order_rows", plural = "RuntimeOrderRows", keyset = "rank asc, id asc")]
struct RuntimeOrderRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[sortable]
    #[graphql_orm(db_column = "source_title", read_policy = "title.read")]
    title: Option<String>,
    #[sortable]
    rank: i64,
    payload: String,
}

schema_roots! { query_custom_ops: [], entities: [RuntimeOrderRow], }

async fn setup() -> Database<SqliteBackend> {
    let db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    let plan = db.schema().plan_migration_to_entities("runtime-order", "owned test", &[RuntimeOrderRow::metadata()]).await.unwrap();
    db.schema().apply_migration(&plan, ApplyOptions::default()).await.unwrap();
    for (id, title, rank) in [("b", Some("same"), 1), ("a", Some("same"), 4), ("c", Some("z"), 2), ("d", None, 3)] {
        RuntimeOrderRow::insert(&db, CreateRuntimeOrderRowInput {
            id: id.into(), title: title.map(str::to_owned), rank, payload: "public".into(),
        }).await.unwrap();
    }
    db
}
fn title_order(direction: OrderDirection) -> Option<Vec<RuntimeOrderRowKeysetOrderByInput>> {
    Some(vec![RuntimeOrderRowKeysetOrderByInput { title: Some(direction), ..Default::default() }])
}
async fn forward(db: &Database<SqliteBackend>, after: Option<String>, direction: OrderDirection) -> async_graphql::Result<Connection<RuntimeOrderRow>> {
    RuntimeOrderRow::keyset_page_ordered(db, Default::default(), title_order(direction), KeysetPageInput { after, limit: Some(2), include_total_count: true }).await
}

#[tokio::test]
async fn collisions_nulls_and_primary_suffix_traverse_in_both_directions() {
    let db = setup().await;
    let first = forward(&db, None, OrderDirection::Asc).await.unwrap();
    assert_eq!(first.edges.iter().map(|e| e.node.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(first.page_info.total_count, Some(4));
    let second = forward(&db, first.page_info.end_cursor.clone(), OrderDirection::Asc).await.unwrap();
    assert_eq!(second.edges.iter().map(|e| e.node.id.as_str()).collect::<Vec<_>>(), ["c", "d"]);
    assert!(!second.page_info.has_next_page);
    let previous = RuntimeOrderRow::keyset_connection_page_ordered(&db, Default::default(), title_order(OrderDirection::Asc), KeysetConnectionInput {
        before: second.page_info.start_cursor, last: Some(2), ..Default::default()
    }).await.unwrap();
    assert_eq!(previous.edges.iter().map(|e| e.node.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    let descending = forward(&db, None, OrderDirection::Desc).await.unwrap();
    assert_eq!(descending.edges.iter().map(|e| e.node.id.as_str()).collect::<Vec<_>>(), ["c", "a"]);
    let remaining = forward(&db, descending.page_info.end_cursor, OrderDirection::Desc).await.unwrap();
    assert_eq!(remaining.edges.iter().map(|e| e.node.id.as_str()).collect::<Vec<_>>(), ["b", "d"]);
}

#[tokio::test]
async fn changed_order_and_ambiguous_or_duplicate_fields_fail_before_select() {
    let db = setup().await;
    let first = forward(&db, None, OrderDirection::Asc).await.unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let db = db.with_read_query_observer(Observe(reads.clone()));
    assert!(forward(&db, first.page_info.end_cursor.clone(), OrderDirection::Desc).await.is_err());
    assert!(RuntimeOrderRow::keyset_page(&db, Default::default(), KeysetPageInput { after: first.page_info.end_cursor, ..Default::default() }).await.is_err());
    for orders in [vec![], vec![Default::default()], vec![RuntimeOrderRowKeysetOrderByInput { title: Some(OrderDirection::Asc), rank: Some(OrderDirection::Asc), ..Default::default() }], vec![RuntimeOrderRowKeysetOrderByInput { title: Some(OrderDirection::Asc), ..Default::default() }; 2]] {
        assert!(RuntimeOrderRow::keyset_page_ordered(&db, Default::default(), Some(orders), Default::default()).await.is_err());
    }
    assert_eq!(reads.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn omitted_order_preserves_configured_cursor_and_nodes() {
    let db = setup().await;
    let configured = RuntimeOrderRow::keyset_page(&db, Default::default(), Default::default()).await.unwrap();
    let omitted = RuntimeOrderRow::keyset_page_ordered(&db, Default::default(), None, Default::default()).await.unwrap();
    assert_eq!(configured.edges.iter().map(|e| (&e.cursor, &e.node.id)).collect::<Vec<_>>(), omitted.edges.iter().map(|e| (&e.cursor, &e.node.id)).collect::<Vec<_>>());
}

struct Observe(Arc<AtomicUsize>);
impl ReadQueryObserver for Observe {
    fn on_read(&self, _: &str, _: usize) { self.0.fetch_add(1, Ordering::Relaxed); }
}
struct DenyTitle;
impl FieldPolicy for DenyTitle {
    fn can_read_field<'a>(&'a self, _: &'a async_graphql::Context<'_>, _: &'a Database, _: &'static str, field: &'static str, _: Option<&'static str>, _: Option<&'a (dyn std::any::Any + Send + Sync)>) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(field != "title") })
    }
    fn can_write_field<'a>(&'a self, _: &'a async_graphql::Context<'_>, _: &'a Database, _: &'static str, _: &'static str, _: Option<&'static str>, _: Option<&'a (dyn std::any::Any + Send + Sync)>, _: Option<&'a (dyn std::any::Any + Send + Sync)>) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
}

#[tokio::test]
async fn unselected_sort_field_denial_prevents_order_leak_and_database_select() {
    let reads = Arc::new(AtomicUsize::new(0));
    let mut db = setup().await;
    db.set_field_policy(DenyTitle);
    let db = db.with_read_query_observer(Observe(reads.clone()));
    let schema = schema_builder(db).data("owned-user".to_string()).finish();
    let response = schema.execute("{ runtimeOrderRowsKeyset(orderBy: [{title: ASC}], page: {limit: 2}) { edges { node { id } } } }").await;
    assert!(!response.errors.is_empty());
    assert_eq!(reads.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn generated_sdl_catalogue_and_typed_order_agree() {
    let schema = schema_builder(setup().await).data("owned-user".to_string()).finish();
    let sdl = schema.sdl();
    assert!(sdl.contains("orderBy: [RuntimeOrderRowKeysetOrderByInput!]"));
    let catalogue = graphql_orm_operation_catalog();
    let operation = catalogue.operations().iter().find(|operation| operation.field_name() == "runtimeOrderRowsKeyset").unwrap();
    assert!(operation.arguments().iter().any(|arg| arg.graphql_name() == "orderBy"));
    let response = schema.execute("{ runtimeOrderRowsKeyset(orderBy: [{rank: DESC}], page: {limit: 2}) { edges { node { id } } } }").await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(response.data.into_json().unwrap()["runtimeOrderRowsKeyset"]["edges"], json!([{"node":{"id":"a"}}, {"node":{"id":"d"}}]));
    let invalid = schema.execute("{ runtimeOrderRowsKeyset(orderBy: [{payload: ASC}], page: {limit: 2}) { edges { node { id } } } }").await;
    assert!(!invalid.errors.is_empty());
}


mod private_key_compatibility {
    use graphql_orm::prelude::*;
    #[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
    #[graphql_entity(table = "private_fixed_order", plural = "PrivateFixedOrderRows", keyset = "digest asc")]
    struct PrivateFixedOrderRow {
        #[primary_key]
        #[graphql_orm(private, auto_generated = false)]
        digest: String,
        #[sortable]
        payload: String,
    }
    schema_roots! { query_custom_ops: [], entities: [PrivateFixedOrderRow], }
    #[tokio::test]
    async fn fixed_keyset_with_private_primary_key_keeps_valid_schema_without_runtime_order() {
        let schema=schema_builder(Database::connect_sqlite("sqlite::memory:").await.unwrap()).finish();
        let sdl=schema.sdl();
        assert!(sdl.contains("privateFixedOrderRowsKeyset"));
        assert!(!sdl.contains("PrivateFixedOrderRowKeysetOrderByInput"));
        let catalog=graphql_orm_operation_catalog();
        let keyset=catalog.operations().iter().find(|operation|operation.field_name()=="privateFixedOrderRowsKeyset").unwrap();
        assert!(!keyset.arguments().iter().any(|argument|argument.graphql_name()=="orderBy"));
    }
}
