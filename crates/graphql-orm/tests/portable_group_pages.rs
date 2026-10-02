#![cfg(all(any(feature = "sqlite", feature = "postgres"), not(feature = "mssql")))]

use graphql_orm::prelude::*;
use std::sync::{Arc, Mutex};
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
type Backend = PostgresBackend;

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(
        backend = "sqlite",
        table = "portable_history",
        plural = "PortableHistory"
    )
)]
#[cfg_attr(
    all(feature = "postgres", not(feature = "sqlite")),
    repository_entity(
        backend = "postgres",
        table = "portable_history",
        plural = "PortableHistory"
    )
)]
struct History {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[filterable(type = "string")]
    tenant: String,
    #[filterable(type = "string")]
    event: Option<String>,
    units: i64,
}

struct Owned {
    db: Database<Backend>,
    #[cfg(feature = "postgres")]
    postgres: owned_postgres::OwnedPostgres,
}
impl Owned {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        #[cfg(feature = "postgres")]
        let postgres = owned_postgres::OwnedPostgres::start("portable-groups")?;
        #[cfg(feature = "sqlite")]
        let db = Database::new(
            sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await?,
        );
        #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
        let db = Database::new(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .connect(&postgres.url)
                .await?,
        );
        let db = db.with_pagination_config(PaginationConfig::legacy());
        let plan = db
            .schema()
            .plan_migration_to_entities("fixture", "owned portable groups", &[History::metadata()])
            .await?;
        db.schema()
            .apply_migration(&plan, Default::default())
            .await?;
        Ok(Self {
            db,
            #[cfg(feature = "postgres")]
            postgres,
        })
    }
    async fn finish(self) -> Result<(), Box<dyn std::error::Error>> {
        self.db.pool().close().await;
        #[cfg(feature = "postgres")]
        {
            let mut postgres = self.postgres;
            postgres.cleanup()?;
        }
        Ok(())
    }
}
fn options(exclude_blank: bool) -> AggregateGroupPageOptions {
    AggregateGroupPageOptions {
        order: AggregateGroupOrder::SqliteNoCase,
        exclude_blank,
        context: "verified-alpha:policy-r1".into(),
    }
}
fn tenant(owner: &str) -> HistoryWhereInput {
    HistoryWhereInput {
        tenant: Some(StringFilter {
            eq: Some(owner.into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}
async fn insert(
    db: &Database<Backend>,
    id: &str,
    owner: &str,
    event: Option<&str>,
    units: i64,
) -> graphql_orm::Result<()> {
    History::insert(
        db,
        CreateHistoryInput {
            id: id.into(),
            tenant: owner.into(),
            event: event.map(str::to_owned),
            units,
        },
    )
    .await?;
    Ok(())
}
#[derive(Clone, Default)]
struct Reads(Arc<Mutex<Vec<usize>>>);
impl ReadQueryObserver for Reads {
    fn on_read(&self, _: &str, rows: usize) {
        self.0.lock().unwrap().push(rows);
    }
}
#[derive(Clone)]
struct Visibility {
    owner: Arc<Mutex<String>>,
    mode: u8,
}
impl RowPolicy<Backend> for Visibility {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            Ok(match self.mode {
                0 => ReadVisibility::Unrestricted,
                1 => ReadVisibility::Complete(ReadPredicate::from_filter::<Backend, _>(&tenant(
                    &self.owner.lock().unwrap(),
                ))?),
                2 => ReadVisibility::Prefilter(ReadPredicate::from_filter::<Backend, _>(&tenant(
                    &self.owner.lock().unwrap(),
                ))?),
                _ => ReadVisibility::CallbackOnly,
            })
        })
    }
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { panic!("residual evaluation must never run after aggregation") })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "owns disposable loopback PostgreSQL 17"
)]
async fn complete_original_groups_and_metrics_match_ascii_byte_order_across_pages()
-> Result<(), Box<dyn std::error::Error>> {
    let mut owned = Owned::start().await?;
    let mut keys: Vec<_> = (0..1305).map(|i| format!("Event-{i:04}")).collect();
    keys.extend(
        [
            "event-0000",
            "EVENT-0000",
            " Case ",
            "Case",
            "case",
            "é",
            "É",
            "İ",
            "i",
            "ß",
            "SS",
            "中",
            "e\u{301}",
            "\t",
            "\n",
            "😀",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    for (i, key) in keys.iter().enumerate() {
        insert(&owned.db, &format!("a-{i}"), "alpha", Some(key), 1).await?;
        insert(&owned.db, &format!("duplicate-{i}"), "alpha", Some(key), 2).await?;
        insert(&owned.db, &format!("b-{i}"), "beta", Some(key), 1000).await?;
    }
    for (i, value) in [None, Some(""), Some(" "), Some("   ")]
        .into_iter()
        .enumerate()
    {
        insert(&owned.db, &format!("blank-{i}"), "alpha", value, 4).await?;
    }
    keys.sort_by(|a, b| {
        a.to_ascii_lowercase()
            .as_bytes()
            .cmp(b.to_ascii_lowercase().as_bytes())
            .then_with(|| a.as_bytes().cmp(b.as_bytes()))
    });
    let observer = Reads::default();
    owned.db = owned.db.with_read_query_observer(observer.clone());
    owned.db.set_row_policy(Visibility {
        owner: Arc::new(Mutex::new("alpha".into())),
        mode: 1,
    });
    for (order, limit) in [
        (AggregateGroupOrder::SqliteNoCase, 100),
        (AggregateGroupOrder::Binary, 100),
        (AggregateGroupOrder::SqliteNoCase, 1000),
    ] {
        let mut expected = keys.clone();
        if order == AggregateGroupOrder::Binary {
            expected.sort();
        }
        let mut cursor = None;
        let mut actual = Vec::new();
        let mut pages = 0;
        loop {
            let page = History::aggregate(&owned.db)
                .group_by(HistoryAggregateField::Event)?
                .count_rows()?
                .count(HistoryAggregateField::Event)?
                .sum(HistoryAggregateField::Units)?
                .min(HistoryAggregateField::Units)?
                .max(HistoryAggregateField::Units)?
                .group_limit(limit)?
                .fetch_group_page(
                    AggregateGroupPageOptions {
                        order,
                        ..options(true)
                    },
                    cursor.as_ref(),
                )
                .await?;
            assert!(page.rows.len() <= limit as usize);
            for row in page.rows {
                let AggregateValue::Text(key) = &row.groups[0].value else {
                    panic!("excluded null group")
                };
                actual.push(key.clone());
                assert_eq!(
                    row.metrics
                        .iter()
                        .map(|m| m.value.clone())
                        .collect::<Vec<_>>(),
                    vec![
                        AggregateValue::Count(2),
                        AggregateValue::Count(2),
                        AggregateValue::Integral(3),
                        AggregateValue::Integral(1),
                        AggregateValue::Integral(2)
                    ]
                );
            }
            pages += 1;
            cursor = page
                .end_cursor
                .map(|c| AggregateGroupCursor::from_json(&c.to_json()?))
                .transpose()?;
            if !page.has_next_page {
                break;
            }
            assert!(cursor.is_some());
        }
        assert_eq!(actual, expected);
        assert_eq!(pages, keys.len().div_ceil(limit as usize));
    }
    assert!(observer.0.lock().unwrap().iter().all(|rows| *rows <= 1001));
    owned.finish().await
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "owns disposable loopback PostgreSQL 17"
)]
async fn nullable_groups_and_policy_aware_ordinary_metrics_are_exact()
-> Result<(), Box<dyn std::error::Error>> {
    let mut owned = Owned::start().await?;
    for (id, owner, value, units) in [
        ("a", "alpha", None, 2),
        ("b", "alpha", None, 3),
        ("c", "alpha", Some(""), 4),
        ("d", "alpha", Some("X"), 5),
        ("e", "beta", Some("Hidden"), 100),
    ] {
        insert(&owned.db, id, owner, value, units).await?;
    }
    let owner = Arc::new(Mutex::new("alpha".into()));
    owned.db.set_row_policy(Visibility {
        owner: owner.clone(),
        mode: 1,
    });
    let mut cursor = None;
    let mut values = vec![];
    loop {
        let page = History::aggregate(&owned.db)
            .group_by(HistoryAggregateField::Event)?
            .count_rows()?
            .sum(HistoryAggregateField::Units)?
            .group_limit(1)?
            .fetch_group_page(options(false), cursor.as_ref())
            .await?;
        values.push((
            page.rows[0].groups[0].value.clone(),
            page.rows[0].metrics[0].value.clone(),
        ));
        cursor = page.end_cursor;
        if !page.has_next_page {
            break;
        }
    }
    assert_eq!(
        values,
        vec![
            (AggregateValue::Null, AggregateValue::Count(2)),
            (AggregateValue::Text("".into()), AggregateValue::Count(1)),
            (AggregateValue::Text("X".into()), AggregateValue::Count(1))
        ]
    );
    let rows = History::aggregate(&owned.db)
        .count_rows()?
        .sum(HistoryAggregateField::Units)?
        .fetch()
        .await?;
    assert_eq!(rows[0].metrics[0].value, AggregateValue::Count(4));
    assert_eq!(rows[0].metrics[1].value, AggregateValue::Integral(14));
    // Caller and policy filters are intersected, never substituted.
    let rows = History::aggregate(&owned.db)
        .filter(tenant("beta"))
        .count_rows()?
        .sum(HistoryAggregateField::Units)?
        .fetch()
        .await?;
    assert_eq!(rows[0].metrics[0].value, AggregateValue::Count(0));
    assert_eq!(rows[0].metrics[1].value, AggregateValue::Null);
    for mode in [2, 3] {
        owned.db.set_row_policy(Visibility {
            owner: owner.clone(),
            mode,
        });
        assert!(
            History::aggregate(&owned.db)
                .count_rows()?
                .fetch()
                .await
                .is_err()
        );
        assert!(
            History::aggregate(&owned.db)
                .group_by(HistoryAggregateField::Event)?
                .fetch_group_page(options(false), None)
                .await
                .is_err()
        );
    }
    owned.db.set_row_policy(Visibility { owner, mode: 0 });
    let rows = History::aggregate(&owned.db)
        .count_rows()?
        .sum(HistoryAggregateField::Units)?
        .fetch()
        .await?;
    assert_eq!(rows[0].metrics[0].value, AggregateValue::Count(5));
    assert_eq!(rows[0].metrics[1].value, AggregateValue::Integral(114));
    owned.finish().await
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "owns disposable loopback PostgreSQL 17"
)]
async fn cursor_bindings_and_current_policy_reject_before_query()
-> Result<(), Box<dyn std::error::Error>> {
    let mut owned = Owned::start().await?;
    for (id, owner, value) in [
        ("a", "alpha", "A"),
        ("b", "alpha", "a"),
        ("c", "beta", "Hidden"),
    ] {
        insert(&owned.db, id, owner, Some(value), 1).await?;
    }
    let owner = Arc::new(Mutex::new("alpha".into()));
    owned.db.set_row_policy(Visibility {
        owner: owner.clone(),
        mode: 1,
    });
    let observer = Reads::default();
    owned.db = owned.db.with_read_query_observer(observer.clone());
    let cursor = History::aggregate(&owned.db)
        .group_by(HistoryAggregateField::Event)?
        .group_limit(1)?
        .fetch_group_page(options(true), None)
        .await?
        .end_cursor
        .unwrap();
    let mut changed = options(true);
    changed.context = "alpha:r2".into();
    for opts in [
        changed,
        options(false),
        AggregateGroupPageOptions {
            order: AggregateGroupOrder::Binary,
            ..options(true)
        },
    ] {
        assert!(
            History::aggregate(&owned.db)
                .group_by(HistoryAggregateField::Event)?
                .fetch_group_page(opts, Some(&cursor))
                .await
                .is_err()
        );
    }
    assert!(
        History::aggregate(&owned.db)
            .group_by(HistoryAggregateField::Event)?
            .count_rows()?
            .fetch_group_page(options(true), Some(&cursor))
            .await
            .is_err()
    );
    assert!(
        History::aggregate(&owned.db)
            .filter(tenant("beta"))
            .group_by(HistoryAggregateField::Event)?
            .fetch_group_page(options(true), Some(&cursor))
            .await
            .is_err()
    );
    *owner.lock().unwrap() = "beta".into();
    assert!(
        History::aggregate(&owned.db)
            .group_by(HistoryAggregateField::Event)?
            .fetch_group_page(options(true), Some(&cursor))
            .await
            .is_err()
    );
    for wire in ["{}".into(), "null".into(), "x".repeat(256 * 1024 + 1)] {
        assert!(AggregateGroupCursor::from_json(&wire).is_err());
    }
    let mut wire: serde_json::Value = serde_json::from_str(&cursor.to_json()?)?;
    for key in [
        serde_json::json!(42),
        serde_json::json!(true),
        serde_json::json!("x".repeat(65537)),
    ] {
        wire["key"] = key;
        assert!(AggregateGroupCursor::from_json(&wire.to_string()).is_err());
    }
    assert!(
        History::aggregate(&owned.db)
            .group_by(HistoryAggregateField::Units)?
            .fetch_group_page(options(true), None)
            .await
            .is_err()
    );
    assert!(
        History::aggregate(&owned.db)
            .group_by(HistoryAggregateField::Event)?
            .group_by(HistoryAggregateField::Tenant)?
            .fetch_group_page(options(true), None)
            .await
            .is_err()
    );
    assert_eq!(observer.0.lock().unwrap().len(), 1);
    let page = History::aggregate(&owned.db)
        .group_by(HistoryAggregateField::Event)?
        .fetch_group_page(options(true), None)
        .await?;
    assert_eq!(
        page.rows[0].groups[0].value,
        AggregateValue::Text("Hidden".into())
    );
    owned.finish().await
}

struct DenyField(&'static str);
impl FieldPolicy<Backend> for DenyField {
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database<Backend>,
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
        _: &'a Database<Backend>,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
    fn can_read_repository_field<'a>(
        &'a self,
        _: Option<graphql_orm::graphql::auth::AccessContext<'a>>,
        _: &'a Database<Backend>,
        _: &'static str,
        field: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(field != self.0) })
    }
}
struct Root;
#[async_graphql::Object]
impl Root {
    async fn visible_count(&self, ctx: &async_graphql::Context<'_>) -> async_graphql::Result<i64> {
        let db = ctx.data::<Database<Backend>>()?;
        let rows = History::aggregate(db)
            .count_rows()?
            .sum(HistoryAggregateField::Units)?
            .fetch_with_graphql_context(ctx)
            .await?;
        match rows[0].metrics[0].value {
            AggregateValue::Count(count) => Ok(count),
            _ => Err("unexpected count".into()),
        }
    }
}
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "owns disposable loopback PostgreSQL 17"
)]
async fn group_metric_filter_and_entity_authority_remain_fail_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let mut owned = Owned::start().await?;
    insert(&owned.db, "a", "alpha", Some("A"), 1).await?;
    insert(&owned.db, "b", "beta", Some("B"), 2).await?;
    let observer = Reads::default();
    owned.db = owned.db.with_read_query_observer(observer.clone());
    let visibility = Visibility {
        owner: Arc::new(Mutex::new("alpha".into())),
        mode: 1,
    };
    owned.db.set_row_policy(visibility);
    for field in ["event", "units", "tenant"] {
        owned.db.set_field_policy(DenyField(field));
        assert!(
            History::aggregate(&owned.db)
                .filter(tenant("alpha"))
                .group_by(HistoryAggregateField::Event)?
                .sum(HistoryAggregateField::Units)?
                .fetch_group_page(options(true), None)
                .await
                .is_err()
        );
        assert!(
            History::aggregate(&owned.db)
                .filter(tenant("alpha"))
                .group_by(HistoryAggregateField::Event)?
                .sum(HistoryAggregateField::Units)?
                .fetch()
                .await
                .is_err()
        );
    }
    assert!(observer.0.lock().unwrap().is_empty());
    owned.db.set_field_policy(DenyField("unrelated"));
    let schema = async_graphql::Schema::build(
        Root,
        async_graphql::EmptyMutation,
        async_graphql::EmptySubscription,
    )
    .data(owned.db.clone())
    .finish();
    let response = schema.execute("{ visibleCount }").await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(response.data, async_graphql::value!({"visibleCount": 1}));
    drop(schema);
    owned.db.set_field_policy(DenyField("units"));
    let schema = async_graphql::Schema::build(
        Root,
        async_graphql::EmptyMutation,
        async_graphql::EmptySubscription,
    )
    .data(owned.db.clone())
    .finish();
    assert!(!schema.execute("{ visibleCount }").await.errors.is_empty());
    drop(schema);
    assert_eq!(observer.0.lock().unwrap().len(), 1);
    owned
        .db
        .set_entity_policy(ScopeEntityPolicy::authenticated());
    assert!(
        History::aggregate(&owned.db)
            .count_rows()?
            .fetch()
            .await
            .is_err()
    );
    assert!(
        History::aggregate(&owned.db)
            .group_by(HistoryAggregateField::Event)?
            .fetch_group_page(options(true), None)
            .await
            .is_err()
    );
    assert_eq!(observer.0.lock().unwrap().len(), 1);
    owned.finish().await
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "owns disposable loopback PostgreSQL 17"
)]
async fn concurrent_new_groups_are_not_a_cross_request_snapshot()
-> Result<(), Box<dyn std::error::Error>> {
    let owned = Owned::start().await?;
    insert(&owned.db, "b", "alpha", Some("B"), 1).await?;
    insert(&owned.db, "d", "alpha", Some("D"), 1).await?;
    let first = History::aggregate(&owned.db)
        .group_by(HistoryAggregateField::Event)?
        .group_limit(1)?
        .fetch_group_page(options(true), None)
        .await?;
    insert(&owned.db, "a", "alpha", Some("A"), 1).await?;
    insert(&owned.db, "c", "alpha", Some("C"), 1).await?;
    let next = History::aggregate(&owned.db)
        .group_by(HistoryAggregateField::Event)?
        .fetch_group_page(options(true), first.end_cursor.as_ref())
        .await?;
    assert_eq!(
        next.rows
            .iter()
            .map(|r| r.groups[0].value.clone())
            .collect::<Vec<_>>(),
        vec![
            AggregateValue::Text("C".into()),
            AggregateValue::Text("D".into())
        ]
    );
    // A inserted behind the boundary is skipped; C ahead is seen. Re-enumerate
    // from None for a refreshed dropdown instead of treating cursors as snapshots.
    owned.finish().await
}

#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[tokio::test]
#[ignore = "owns disposable loopback PostgreSQL 17 with ICU"]
async fn postgres_nondeterministic_source_collation_does_not_merge_original_groups()
-> Result<(), Box<dyn std::error::Error>> {
    let owned = Owned::start().await?;
    sqlx::query("CREATE COLLATION fixture_case_insensitive (provider = icu, locale = 'und-u-ks-level2', deterministic = false)").execute(owned.db.pool()).await?;
    sqlx::query("ALTER TABLE portable_history ALTER COLUMN event TYPE TEXT COLLATE fixture_case_insensitive").execute(owned.db.pool()).await?;
    for (id, key) in [("a", "A"), ("b", "a"), ("c", "É"), ("d", "é")] {
        insert(&owned.db, id, "alpha", Some(key), 1).await?;
    }
    let mut cursor = None;
    let mut keys = vec![];
    loop {
        let page = History::aggregate(&owned.db)
            .group_by(HistoryAggregateField::Event)?
            .count_rows()?
            .group_limit(1)?
            .fetch_group_page(options(true), cursor.as_ref())
            .await?;
        assert_eq!(page.rows[0].metrics[0].value, AggregateValue::Count(1));
        keys.push(page.rows[0].groups[0].value.clone());
        cursor = page.end_cursor;
        if !page.has_next_page {
            break;
        }
    }
    assert_eq!(
        keys,
        ["A", "a", "É", "é"].map(|s| AggregateValue::Text(s.into()))
    );
    owned.finish().await
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct HandwrittenField;
impl TypedAggregateField<History> for HandwrittenField {
    fn aggregate_field(self) -> &'static AggregateFieldRef {
        HistoryAggregateField::Event.aggregate_field()
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct WrongIdentityField;
impl TypedAggregateField<History> for WrongIdentityField {
    fn aggregate_field(self) -> &'static AggregateFieldRef {
        HistoryAggregateField::Event.aggregate_field()
    }
    fn entity_type_id() -> Option<std::any::TypeId> {
        Some(std::any::TypeId::of::<()>())
    }
}
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "owns disposable loopback PostgreSQL 17"
)]
async fn handwritten_aggregate_identity_remains_compatible_and_complete_visibility_is_bound()
-> Result<(), Box<dyn std::error::Error>> {
    let mut owned = Owned::start().await?;
    insert(&owned.db, "a", "alpha", Some("A"), 1).await?;
    let owner = Arc::new(Mutex::new("alpha".into()));
    owned.db.set_row_policy(Visibility {
        owner: owner.clone(),
        mode: 1,
    });
    assert!(
        GroupedAggregateQuery::<History, HistoryWhereInput, HandwrittenField, Backend>::new(
            &owned.db
        )
        .count_rows()?
        .fetch()
        .await
        .is_err()
    );
    assert!(
        GroupedAggregateQuery::<History, HistoryWhereInput, WrongIdentityField, Backend>::new(
            &owned.db
        )
        .count_rows()?
        .fetch()
        .await
        .is_err()
    );
    owned.db.set_row_policy(Visibility { owner, mode: 0 });
    let rows = GroupedAggregateQuery::<History, HistoryWhereInput, HandwrittenField, Backend>::new(
        &owned.db,
    )
    .count_rows()?
    .fetch()
    .await?;
    assert_eq!(rows[0].metrics[0].value, AggregateValue::Count(1));
    owned.finish().await
}

#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[tokio::test]
#[ignore = "owns disposable loopback PostgreSQL 17 with nonowner RLS role"]
async fn postgres_db_auth_context_filters_before_pages_and_ordinary_aggregation()
-> Result<(), Box<dyn std::error::Error>> {
    let owned = Owned::start().await?;
    for (id, owner, key, units) in [
        ("a", "alpha", "A", 1),
        ("b", "alpha", "B", 2),
        ("c", "beta", "Other", 100),
    ] {
        insert(&owned.db, id, owner, Some(key), units).await?;
    }
    for statement in [
        "CREATE ROLE fixture_reader NOLOGIN NOSUPERUSER NOBYPASSRLS",
        "GRANT SELECT ON portable_history TO fixture_reader",
        "ALTER TABLE portable_history ENABLE ROW LEVEL SECURITY",
        "CREATE POLICY tenant_read ON portable_history FOR SELECT TO fixture_reader USING (tenant = current_setting('app.tenant_id', true))",
    ] {
        sqlx::query(statement).execute(owned.db.pool()).await?;
    }
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE fixture_reader")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&owned.postgres.url)
        .await?;
    let db = Database::<PostgresBackend>::new(pool);
    let user: String = sqlx::query_scalar("SELECT current_user")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(user, "fixture_reader");
    let alpha = DbAuthContext::from_parts("reader-a", vec![], vec![], Some("alpha".into()));
    let beta = DbAuthContext::from_parts("reader-b", vec![], vec![], Some("beta".into()));
    let first = History::aggregate(&db)
        .with_auth(Some(&alpha))
        .group_by(HistoryAggregateField::Event)?
        .group_limit(1)?
        .fetch_group_page(options(true), None)
        .await?;
    assert_eq!(
        first.rows[0].groups[0].value,
        AggregateValue::Text("A".into())
    );
    assert!(first.has_next_page);
    assert!(
        History::aggregate(&db)
            .with_auth(Some(&beta))
            .group_by(HistoryAggregateField::Event)?
            .fetch_group_page(options(true), first.end_cursor.as_ref())
            .await
            .is_err()
    );
    let next = History::aggregate(&db)
        .with_auth(Some(&alpha))
        .group_by(HistoryAggregateField::Event)?
        .fetch_group_page(options(true), first.end_cursor.as_ref())
        .await?;
    assert_eq!(
        next.rows[0].groups[0].value,
        AggregateValue::Text("B".into())
    );
    assert!(!next.has_next_page);
    for (auth, count, sum) in [
        (Some(&alpha), 2, AggregateValue::Integral(3)),
        (Some(&beta), 1, AggregateValue::Integral(100)),
        (None, 0, AggregateValue::Null),
    ] {
        let rows = History::aggregate(&db)
            .with_auth(auth)
            .count_rows()?
            .sum(HistoryAggregateField::Units)?
            .fetch()
            .await?;
        assert_eq!(rows[0].metrics[0].value, AggregateValue::Count(count));
        assert_eq!(rows[0].metrics[1].value, sum);
    }
    db.pool().close().await;
    drop(db);
    owned.finish().await
}
