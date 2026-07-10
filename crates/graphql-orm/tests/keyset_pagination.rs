#![cfg(feature = "sqlite")]

use graphql_orm::prelude::*;
use std::collections::HashSet;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "keyset_items",
    plural = "KeysetItems",
    keyset = "priority asc nulls last, rank asc, id asc"
)]
struct KeysetItem {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "number")]
    #[sortable]
    priority: Option<i64>,
    #[filterable(type = "number")]
    #[sortable]
    rank: i64,
    label: String,
}

schema_roots! {
    query_custom_ops: [],
    entities: [KeysetItem],
}

fn input(priority: Option<i64>, rank: i64, label: &str) -> CreateKeysetItemInput {
    CreateKeysetItemInput {
        priority,
        rank,
        label: label.to_string(),
    }
}

async fn setup() -> graphql_orm::Result<Database<SqliteBackend>> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let plan = database
        .schema()
        .plan_migration_to_entities("keyset-init", "keyset test", &[KeysetItem::metadata()])
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    Ok(database)
}

#[tokio::test]
async fn bounded_composite_keyset_has_no_duplicates_when_rows_arrive_before_cursor()
-> graphql_orm::Result<()> {
    let database = setup().await?;
    for (priority, rank, label) in [
        (Some(1), 1, "a"),
        (Some(1), 2, "b"),
        (Some(2), 1, "c"),
        (Some(2), 2, "d"),
        (None, 1, "nullable-last"),
    ] {
        KeysetItem::insert(&database, input(priority, rank, label)).await?;
    }

    let first = KeysetItem::keyset_page(
        &database,
        KeysetItemWhereInput::default(),
        KeysetPageInput {
            limit: Some(2),
            ..Default::default()
        },
    )
    .await
    .expect("first keyset page");
    assert_eq!(first.edges.len(), 2);
    assert!(first.page_info.has_next_page);
    assert!(first.page_info.total_count.is_none());
    let cursor = first.page_info.end_cursor.clone().expect("end cursor");

    KeysetItem::insert(&database, input(Some(0), 1, "inserted-before-cursor")).await?;
    let mut ids = first
        .edges
        .iter()
        .map(|edge| edge.node.id)
        .collect::<HashSet<_>>();
    let mut after = Some(cursor);
    let mut labels = first
        .edges
        .into_iter()
        .map(|edge| edge.node.label)
        .collect::<Vec<_>>();
    while let Some(cursor) = after {
        let page = KeysetItem::keyset_page(
            &database,
            KeysetItemWhereInput::default(),
            KeysetPageInput {
                after: Some(cursor),
                limit: Some(2),
                include_total_count: false,
            },
        )
        .await
        .expect("next keyset page");
        for edge in &page.edges {
            assert!(ids.insert(edge.node.id), "duplicate row across pages");
            labels.push(edge.node.label.clone());
        }
        after = page.page_info.has_next_page.then(|| {
            page.page_info
                .end_cursor
                .clone()
                .expect("look-ahead implies cursor")
        });
    }
    assert!(!labels.iter().any(|label| label == "inserted-before-cursor"));
    assert_eq!(labels.last().map(String::as_str), Some("nullable-last"));

    let invalid = KeysetItem::keyset_page(
        &database,
        KeysetItemWhereInput::default(),
        KeysetPageInput {
            after: Some("2".to_string()),
            limit: Some(2),
            include_total_count: false,
        },
    )
    .await
    .expect_err("legacy offset cursor is rejected");
    assert_eq!(invalid.code, OrmErrorCode::CursorInvalid);
    Ok(())
}

#[tokio::test]
async fn keyset_applies_max_limit_and_counts_only_when_requested() -> graphql_orm::Result<()> {
    let database = setup()
        .await?
        .with_pagination_config(PaginationConfig::secure().with_max_limit(Some(2)));
    for rank in 0..5 {
        KeysetItem::insert(&database, input(Some(1), rank, "item")).await?;
    }
    let page = KeysetItem::keyset_page(
        &database,
        KeysetItemWhereInput::default(),
        KeysetPageInput {
            limit: Some(500),
            include_total_count: true,
            ..Default::default()
        },
    )
    .await
    .expect("bounded page");
    assert_eq!(page.edges.len(), 2);
    assert_eq!(page.page_info.total_count, Some(5));
    Ok(())
}

#[tokio::test]
async fn generated_graphql_keyset_connection_uses_opaque_cursors() -> graphql_orm::Result<()> {
    let database = setup().await?;
    for rank in 0..3 {
        KeysetItem::insert(&database, input(Some(1), rank, "item")).await?;
    }
    let schema = schema_builder(database)
        .data("test-user".to_string())
        .finish();
    let response = schema
        .execute(
            "{ keysetItemsKeyset(page: { limit: 2 }) {
                edges { cursor node { rank } }
                pageInfo { hasNextPage totalCount endCursor }
            } }",
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let json = response.data.into_json().expect("GraphQL JSON");
    let cursor = json["keysetItemsKeyset"]["edges"][0]["cursor"]
        .as_str()
        .expect("cursor string");
    assert!(cursor.starts_with("gomk1."));
    assert_eq!(json["keysetItemsKeyset"]["pageInfo"]["hasNextPage"], true);
    assert!(json["keysetItemsKeyset"]["pageInfo"]["totalCount"].is_null());
    Ok(())
}
