#![cfg(feature = "sqlite")]

use graphql_orm::prelude::*;
use std::collections::BTreeSet;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "composite_keyset_records",
    plural = "CompositeKeysetRecords",
    keyset = "rank desc nulls last, partition_key asc, item_no asc"
)]
struct CompositeRecord {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false, db_column = "partition")]
    #[filterable(type = "string")]
    partition_key: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "number")]
    item_no: i64,
    #[filterable(type = "number")]
    rank: Option<i64>,
}

schema_roots! {
    query_custom_ops: [],
    generated_mutations: "none",
    entities: [CompositeRecord],
}

async fn setup() -> graphql_orm::Result<Database<SqliteBackend>> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    // This database and schema are disposable fixture-owned state, never an external source.
    sqlx::query("CREATE TABLE composite_keyset_records (\"partition\" TEXT NOT NULL, item_no INTEGER NOT NULL, rank INTEGER, PRIMARY KEY (\"partition\", item_no))")
        .execute(database.pool()).await?;
    for (partition, item, rank) in [
        ("a", 1, Some(3_i64)),
        ("a", 2, Some(3)),
        ("b", 1, Some(3)),
        ("b", 2, Some(2)),
        ("a", 3, None),
        ("b", 3, None),
    ] {
        sqlx::query(
            "INSERT INTO composite_keyset_records (\"partition\", item_no, rank) VALUES (?, ?, ?)",
        )
        .bind(partition)
        .bind(item)
        .bind(rank)
        .execute(database.pool())
        .await?;
    }
    Ok(database)
}

#[tokio::test]
async fn forward_pages_keep_complete_composite_identity_and_nullable_order()
-> graphql_orm::Result<()> {
    let database = setup().await?;
    let mut after = None;
    let mut keys = Vec::new();
    for page_index in 0..4 {
        let page = CompositeRecord::keyset_page(
            &database,
            CompositeRecordWhereInput::default(),
            KeysetPageInput {
                after,
                limit: Some(2),
                include_total_count: false,
            },
        )
        .await
        .expect("composite keyset page");
        keys.extend(
            page.edges
                .into_iter()
                .map(|edge| (edge.node.partition_key, edge.node.item_no)),
        );
        if page_index == 0 {
            sqlx::query("INSERT INTO composite_keyset_records (\"partition\", item_no, rank) VALUES ('new-before-cursor', 1, 99)")
                .execute(database.pool()).await?;
        }
        if !page.page_info.has_next_page {
            break;
        }
        after = page.page_info.end_cursor;
    }
    assert_eq!(
        keys,
        vec![
            ("a".into(), 1),
            ("a".into(), 2),
            ("b".into(), 1),
            ("b".into(), 2),
            ("a".into(), 3),
            ("b".into(), 3)
        ]
    );
    assert_eq!(keys.iter().collect::<BTreeSet<_>>().len(), 6);
    Ok(())
}

#[tokio::test]
async fn backward_pages_keep_the_complete_key_tiebreaker() -> graphql_orm::Result<()> {
    let database = setup().await?;
    let tail = CompositeRecord::keyset_connection_page(
        &database,
        CompositeRecordWhereInput::default(),
        graphql_orm::graphql::pagination::KeysetConnectionInput {
            last: Some(2),
            ..Default::default()
        },
    )
    .await
    .expect("composite tail");
    assert_eq!(
        tail.edges
            .iter()
            .map(|edge| (edge.node.partition_key.as_str(), edge.node.item_no))
            .collect::<Vec<_>>(),
        [("a", 3), ("b", 3)]
    );
    let older = CompositeRecord::keyset_connection_page(
        &database,
        CompositeRecordWhereInput::default(),
        graphql_orm::graphql::pagination::KeysetConnectionInput {
            last: Some(2),
            before: tail.page_info.start_cursor,
            ..Default::default()
        },
    )
    .await
    .expect("composite older window");
    assert_eq!(
        older
            .edges
            .iter()
            .map(|edge| (edge.node.partition_key.as_str(), edge.node.item_no))
            .collect::<Vec<_>>(),
        [("b", 1), ("b", 2)]
    );
    Ok(())
}

#[tokio::test]
async fn generated_composite_keyset_is_admitted_without_graphql_writes() -> graphql_orm::Result<()>
{
    let schema = schema_builder(setup().await?)
        .data("test-user".to_string())
        .finish();
    let sdl = schema.sdl();
    assert!(sdl.contains("compositeKeysetRecordsKeyset("));
    assert!(sdl.contains("compositeKeysetRecordsScan("));
    assert!(!sdl.contains("createCompositeRecord("));
    let response = schema.execute("{ compositeKeysetRecordsKeyset(page: {limit: 2}) { edges { node { partitionKey itemNo } } } }").await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["compositeKeysetRecordsKeyset"]["edges"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    Ok(())
}

struct CompositeCursorAuthority(&'static str);
impl FieldPolicy for CompositeCursorAuthority {
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        field: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(field != self.0) })
    }
    fn can_write_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}
struct CompositeReadCounter(std::sync::Arc<std::sync::atomic::AtomicUsize>);
impl ReadQueryObserver for CompositeReadCounter {
    fn on_read(&self, _: &str, _: usize) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}
#[tokio::test]
async fn cursor_only_composite_reads_require_rank_and_both_key_components_before_select() {
    for denied in ["rank", "partitionKey", "itemNo"] {
        let mut database = setup().await.unwrap();
        database.set_field_policy(CompositeCursorAuthority(denied));
        let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let database = database.with_read_query_observer(CompositeReadCounter(reads.clone()));
        let schema = schema_builder(database)
            .data(AuthSubject::new("fixture-principal"))
            .finish();
        let response = schema
            .execute("{ compositeKeysetRecordsKeyset(page: {limit: 2}) { edges { cursor } } }")
            .await;
        assert!(
            !response.errors.is_empty(),
            "unselected cursor component {denied} was not checked"
        );
        assert_eq!(
            reads.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "denied cursor field reached SELECT"
        );
        assert!(response.data.into_json().unwrap()["compositeKeysetRecordsKeyset"].is_null());
    }
}
