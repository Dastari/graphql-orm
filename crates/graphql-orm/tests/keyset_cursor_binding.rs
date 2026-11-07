// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]

//! What a keyset cursor is bound to, on every generated path that decodes one.
//!
//! A cursor resumes by order *values*, so it is bound to the entity, the
//! order, and (when the caller filtered) a hash of the caller-visible filter.
//! It is deliberately not bound to row contents: value drift is documented in
//! `docs/reference/graphql-orm/pagination-migration.md`.
#![cfg(feature = "sqlite")]

use graphql_orm::graphql::errors::OrmErrorCode;
use graphql_orm::prelude::*;
use serde_json::{Value, json};

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "binding_items",
    plural = "BindingItems",
    keyset = "rank asc, id asc"
)]
struct BindingItem {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    group_name: String,
    #[filterable(type = "number")]
    #[sortable]
    rank: i64,
}

/// Same key columns and order as `BindingItem`; only the entity differs.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "binding_others",
    plural = "BindingOthers",
    keyset = "rank asc, id asc"
)]
struct BindingOther {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    group_name: String,
    #[filterable(type = "number")]
    #[sortable]
    rank: i64,
}

schema_roots! { query_custom_ops: [], entities: [BindingItem, BindingOther], }

fn group(name: &str) -> BindingItemWhereInput {
    BindingItemWhereInput {
        group_name: Some(StringFilter {
            eq: Some(name.to_string()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn other_group(name: &str) -> BindingOtherWhereInput {
    BindingOtherWhereInput {
        group_name: Some(StringFilter {
            eq: Some(name.to_string()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

async fn setup() -> Database<SqliteBackend> {
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    let plan = db
        .schema()
        .plan_migration_to_entities(
            "binding",
            "cursor binding",
            &[BindingItem::metadata(), BindingOther::metadata()],
        )
        .await
        .unwrap();
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .unwrap();
    // Four rows in two groups, interleaved so each group spans two pages.
    for (id, group_name, rank) in [("a", "x", 1), ("b", "y", 2), ("c", "x", 3), ("d", "y", 4)] {
        BindingItem::insert(
            &db,
            CreateBindingItemInput {
                id: id.into(),
                group_name: group_name.into(),
                rank,
            },
        )
        .await
        .unwrap();
        BindingOther::insert(
            &db,
            CreateBindingOtherInput {
                id: id.into(),
                group_name: group_name.into(),
                rank,
            },
        )
        .await
        .unwrap();
    }
    db
}

fn page(after: Option<String>) -> KeysetPageInput {
    KeysetPageInput {
        after,
        limit: Some(1),
        include_total_count: false,
    }
}

fn ids<T>(connection: &Connection<T>, id: impl Fn(&T) -> &str) -> Vec<String> {
    connection
        .edges
        .iter()
        .map(|edge| id(&edge.node).to_string())
        .collect()
}

fn refused<T: std::fmt::Debug>(result: async_graphql::Result<T>) {
    let error = result.expect_err("cursor must be refused");
    let code = error
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.get("code"))
        .map(|code| code.to_string());
    assert_eq!(
        code.as_deref(),
        Some("\"CURSOR_INVALID\""),
        "expected CURSOR_INVALID, got {error:?}"
    );
}

fn refused_public<T: std::fmt::Debug>(
    result: Result<T, graphql_orm::graphql::errors::OrmPublicError>,
) {
    let error = result.expect_err("cursor must be refused");
    assert_eq!(error.code, OrmErrorCode::CursorInvalid, "{error:?}");
}

async fn first(
    db: &Database<SqliteBackend>,
    filter: BindingItemWhereInput,
) -> Connection<BindingItem> {
    BindingItem::keyset_page(db, filter, page(None))
        .await
        .unwrap()
}

async fn resume(
    db: &Database<SqliteBackend>,
    filter: BindingItemWhereInput,
    cursor: &str,
) -> async_graphql::Result<Connection<BindingItem>> {
    BindingItem::keyset_page(db, filter, page(Some(cursor.to_string())))
        .await
        .map_err(|error| error.into_graphql_error())
}

#[tokio::test]
async fn same_filter_resumes_and_a_different_filter_is_refused() {
    let db = setup().await;
    let issued = first(&db, group("x")).await;
    assert_eq!(ids(&issued, |n| &n.id), ["a"]);
    let cursor = issued.page_info.end_cursor.unwrap();
    let next = resume(&db, group("x"), &cursor).await.unwrap();
    assert_eq!(ids(&next, |n| &n.id), ["c"]);
    refused(resume(&db, group("y"), &cursor).await);
}

#[tokio::test]
async fn a_filtered_cursor_is_refused_without_a_filter_and_the_reverse() {
    let db = setup().await;
    let filtered = first(&db, group("x")).await.page_info.end_cursor.unwrap();
    refused(resume(&db, Default::default(), &filtered).await);
    let unfiltered = first(&db, Default::default())
        .await
        .page_info
        .end_cursor
        .unwrap();
    // `filter: None` to `filter: Some(..)`: an empty filter has no identity,
    // a populated one does, and the two must not be interchangeable.
    refused(resume(&db, group("x"), &unfiltered).await);
    refused(resume(&db, group("y"), &unfiltered).await);
    // An all-default filter is the same query as no filter, so it still resumes.
    assert!(resume(&db, Default::default(), &unfiltered).await.is_ok());
}

#[tokio::test]
async fn a_cursor_is_refused_on_another_entity_with_the_same_order() {
    let db = setup().await;
    let item = first(&db, Default::default())
        .await
        .page_info
        .end_cursor
        .unwrap();
    refused(
        BindingOther::keyset_page(&db, Default::default(), page(Some(item)))
            .await
            .map_err(|error| error.into_graphql_error()),
    );
    let filtered = first(&db, group("x")).await.page_info.end_cursor.unwrap();
    refused(
        BindingOther::keyset_page(&db, other_group("x"), page(Some(filtered)))
            .await
            .map_err(|error| error.into_graphql_error()),
    );
}

#[tokio::test]
async fn a_cursor_is_refused_under_a_different_order() {
    let db = setup().await;
    let cursor = first(&db, Default::default())
        .await
        .page_info
        .end_cursor
        .unwrap();
    let by_group = Some(vec![BindingItemKeysetOrderByInput {
        group_name: Some(OrderDirection::Asc),
        ..Default::default()
    }]);
    refused(
        BindingItem::keyset_page_ordered(&db, Default::default(), by_group, page(Some(cursor)))
            .await,
    );
}

#[tokio::test]
async fn the_cursor_and_error_never_contain_the_filter() {
    let db = setup().await;
    let secret = "needle-filter-value";
    let cursor = first(&db, group(secret)).await.page_info.end_cursor;
    // No rows match, so seed one and take its cursor under the secret filter.
    drop(cursor);
    BindingItem::insert(
        &db,
        CreateBindingItemInput {
            id: "z".into(),
            group_name: secret.into(),
            rank: 9,
        },
    )
    .await
    .unwrap();
    BindingItem::insert(
        &db,
        CreateBindingItemInput {
            id: "zz".into(),
            group_name: secret.into(),
            rank: 10,
        },
    )
    .await
    .unwrap();
    let cursor = first(&db, group(secret))
        .await
        .page_info
        .end_cursor
        .unwrap();
    let hex = cursor.strip_prefix("gomk1.").unwrap();
    let decoded: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    let decoded = String::from_utf8(decoded).unwrap();
    assert!(!cursor.contains(secret) && !decoded.contains(secret));
    assert!(decoded.contains(":filter-v1:"));
    let error = resume(&db, group("other"), &cursor).await.unwrap_err();
    assert!(!format!("{error:?}").contains(secret));
    assert!(!format!("{error:?}").contains("other"));
}

#[tokio::test]
async fn bidirectional_connection_binds_the_filter_both_ways() {
    let db = setup().await;
    let forward = BindingItem::keyset_connection_page(
        &db,
        group("x"),
        KeysetConnectionInput {
            first: Some(1),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let cursor = forward.page_info.end_cursor.unwrap();
    for (filter, backwards) in [
        (group("y"), false),
        (Default::default(), false),
        (group("y"), true),
    ] {
        let input = if backwards {
            KeysetConnectionInput {
                last: Some(1),
                before: Some(cursor.clone()),
                ..Default::default()
            }
        } else {
            KeysetConnectionInput {
                first: Some(1),
                after: Some(cursor.clone()),
                ..Default::default()
            }
        };
        refused_public(BindingItem::keyset_connection_page(&db, filter, input).await);
    }
}

#[tokio::test]
async fn graphql_keyset_root_binds_filter_and_entity() {
    let db = setup().await;
    let schema = async_graphql::Schema::build(
        QueryRoot::default(),
        MutationRoot::default(),
        SubscriptionRoot::default(),
    )
    .data(db)
    .data(AuthSubject::builder("reader").build())
    .finish();
    let run = |root: &'static str, filter: Value, after: Value| {
        let schema = schema.clone();
        async move {
            schema
                .execute(
                    async_graphql::Request::new(format!(
                        "query($w: {0}WhereInput, $after: String) {{ rows: {1}(where: $w, page: {{limit: 1, after: $after}}) {{ edges {{ node {{ id }} cursor }} pageInfo {{ endCursor }} }} }}",
                        if root == "bindingItemsKeyset" { "BindingItem" } else { "BindingOther" },
                        root
                    ))
                    .variables(async_graphql::Variables::from_json(
                        json!({"w": filter, "after": after}),
                    )),
                )
                .await
        }
    };
    let ok = |response: async_graphql::Response| {
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        response.data.into_json().unwrap()["rows"].clone()
    };
    let cursor = ok(run(
        "bindingItemsKeyset",
        json!({"groupName": {"eq": "x"}}),
        Value::Null,
    )
    .await)["pageInfo"]["endCursor"]
        .clone();
    assert_eq!(
        ok(run(
            "bindingItemsKeyset",
            json!({"groupName": {"eq": "x"}}),
            cursor.clone()
        )
        .await)["edges"][0]["node"]["id"],
        "c"
    );
    for (root, filter) in [
        ("bindingItemsKeyset", json!({"groupName": {"eq": "y"}})),
        ("bindingItemsKeyset", json!({})),
        ("bindingOthersKeyset", json!({"groupName": {"eq": "x"}})),
    ] {
        let response = run(root, filter, cursor.clone()).await;
        assert_eq!(response.errors.len(), 1, "{root}");
        assert_eq!(
            response.errors[0].extensions.as_ref().unwrap().get("code"),
            Some(&async_graphql::Value::from("CURSOR_INVALID")),
            "{root}"
        );
    }
    let unfiltered =
        ok(run("bindingItemsKeyset", json!({}), Value::Null).await)["pageInfo"]["endCursor"]
            .clone();
    let response = run(
        "bindingItemsKeyset",
        json!({"groupName": {"eq": "x"}}),
        unfiltered,
    )
    .await;
    assert_eq!(response.errors.len(), 1);
}

#[tokio::test]
async fn mutation_context_keyset_binds_filter_and_entity() {
    let db = setup().await;
    let mut ctx = MutationContext::begin(&db, TransactionMode::Default)
        .await
        .unwrap();
    let issued = ctx
        .keyset_page::<BindingItem>(group("x"), page(None))
        .await
        .unwrap();
    let cursor = issued.page_info.end_cursor.unwrap();
    let next = ctx
        .keyset_page::<BindingItem>(group("x"), page(Some(cursor.clone())))
        .await
        .unwrap();
    assert_eq!(ids(&next, |n| &n.id), ["c"]);
    refused_public(
        ctx.keyset_page::<BindingItem>(group("y"), page(Some(cursor.clone())))
            .await,
    );
    refused_public(
        ctx.keyset_page::<BindingItem>(Default::default(), page(Some(cursor.clone())))
            .await,
    );
    refused_public(
        ctx.keyset_page::<BindingOther>(other_group("x"), page(Some(cursor.clone())))
            .await,
    );
    let unfiltered = ctx
        .keyset_page::<BindingItem>(Default::default(), page(None))
        .await
        .unwrap()
        .page_info
        .end_cursor
        .unwrap();
    refused_public(
        ctx.keyset_page::<BindingItem>(group("x"), page(Some(unfiltered)))
            .await,
    );
    // The connection form binds the same way.
    let connection = ctx
        .keyset_connection_page::<BindingItem>(
            group("x"),
            KeysetConnectionInput {
                first: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let cursor = connection.page_info.end_cursor.unwrap();
    refused_public(
        ctx.keyset_connection_page::<BindingItem>(
            group("y"),
            KeysetConnectionInput {
                first: Some(1),
                after: Some(cursor),
                ..Default::default()
            },
        )
        .await,
    );
}

#[tokio::test]
async fn scan_continuation_binds_filter_and_entity() {
    let db = setup()
        .await
        .with_authorized_scan_config(AuthorizedScanConfig::new([7; 32], 1, 1));
    let scan = |filter: BindingItemWhereInput, after: Option<String>| {
        let db = &db;
        async move {
            BindingItem::authorized_scan(
                db,
                filter,
                AuthorizedScanInput {
                    after,
                    limit: Some(1),
                    include_total_count: false,
                },
            )
            .await
        }
    };
    let first = scan(group("x"), None).await.unwrap();
    let cursor = first.page_info.continuation.clone().expect("continuation");
    assert!(scan(group("x"), Some(cursor.clone())).await.is_ok());
    refused(scan(group("y"), Some(cursor.clone())).await);
    refused(scan(Default::default(), Some(cursor.clone())).await);
    refused(
        BindingOther::authorized_scan(
            &db,
            other_group("x"),
            AuthorizedScanInput {
                after: Some(cursor),
                limit: Some(1),
                include_total_count: false,
            },
        )
        .await,
    );
}

/// Keyset resumes by order *values*, never by row identity. A cursor taken
/// before a row's order values change is accepted, and the page after it
/// reflects the new values. This pins the documented behaviour; do not "fix" it.
#[tokio::test]
async fn value_drift_after_the_cursor_is_accepted_and_resumes_by_value() {
    let db = setup().await;
    let issued = first(&db, Default::default()).await;
    assert_eq!(ids(&issued, |n| &n.id), ["a"]);
    let cursor = issued.page_info.end_cursor.unwrap();

    // Move the cursor's own row past every other row, and pull `d` forward.
    BindingItem::update_by_id(
        &db,
        &"a".to_string(),
        UpdateBindingItemInput {
            rank: Some(100),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    BindingItem::update_by_id(
        &db,
        &"d".to_string(),
        UpdateBindingItemInput {
            rank: Some(0),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Accepted, not CURSOR_INVALID: the cursor holds (rank=1, id="a").
    let resumed = BindingItem::keyset_page(
        &db,
        Default::default(),
        KeysetPageInput {
            after: Some(cursor),
            limit: Some(10),
            include_total_count: false,
        },
    )
    .await
    .unwrap();
    // "d" (now rank 0) sorts before the cursor and is skipped; "a" (now rank
    // 100) sorts after it and is returned again.
    assert_eq!(ids(&resumed, |n| &n.id), ["b", "c", "a"]);
}
