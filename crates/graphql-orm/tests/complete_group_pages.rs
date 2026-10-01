#![cfg(feature = "sqlite")]

use graphql_orm::prelude::*;
use std::sync::{Arc, Mutex};

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "sqlite",
    table = "private_history",
    plural = "PrivateHistory"
)]
struct PrivateHistory {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[filterable(type = "string")]
    tenant: String,
    #[filterable(type = "string")]
    event: String,
    units: i64,
    #[graphql_orm(private, sensitive)]
    custody: Option<String>,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "sqlite",
    table = "nullable_groups",
    plural = "NullableGroups"
)]
struct NullableGroup {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    category: Option<String>,
    units: i64,
}

#[derive(Clone)]
struct Reads(Arc<Mutex<Vec<(String, usize)>>>);
impl ReadQueryObserver for Reads {
    fn on_read(&self, sql: &str, fetched_rows: usize) {
        self.0.lock().unwrap().push((sql.into(), fetched_rows));
    }
}

fn options(context: &str, exclude_blank: bool) -> AggregateGroupPageOptions {
    AggregateGroupPageOptions {
        order: AggregateGroupOrder::SqliteNoCase,
        exclude_blank,
        context: context.into(),
    }
}
fn tenant(value: &str) -> PrivateHistoryWhereInput {
    PrivateHistoryWhereInput {
        tenant: Some(StringFilter {
            eq: Some(value.into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}
async fn database() -> graphql_orm::Result<Database<SqliteBackend>> {
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let plan = db
        .schema()
        .plan_migration_to_entities(
            "groups",
            "private fixture",
            &[PrivateHistory::metadata(), NullableGroup::metadata()],
        )
        .await?;
    db.schema()
        .apply_migration(&plan, Default::default())
        .await?;
    Ok(db)
}
async fn insert(
    db: &Database<SqliteBackend>,
    id: &str,
    partition: &str,
    event: &str,
) -> graphql_orm::Result<()> {
    PrivateHistory::insert(
        db,
        CreatePrivateHistoryInput {
            id: id.into(),
            tenant: partition.into(),
            event: event.into(),
            units: 1,
            custody: None,
        },
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn complete_native_distinct_set_beyond_cap_preserves_case_padding_and_tenants()
-> graphql_orm::Result<()> {
    let db = database().await?;
    for index in 0..230 {
        let event = format!("Event-{index:03}");
        insert(&db, &format!("a-{index}"), "alpha", &event).await?;
        insert(&db, &format!("duplicate-{index}"), "alpha", &event).await?;
    }
    for (index, event) in ["Case", "case", " CASE ", "", "   ", "'_%\\", "\t"]
        .iter()
        .enumerate()
    {
        insert(&db, &format!("special-{index}"), "alpha", event).await?;
    }
    insert(&db, "other", "beta", "other-tenant").await?;
    let observed = Reads(Arc::new(Mutex::new(vec![])));
    let db = db.with_read_query_observer(observed.clone());
    let mut cursor = None;
    let mut actual = Vec::new();
    loop {
        let page = PrivateHistory::aggregate(&db)
            .filter(tenant("alpha"))
            .group_by(PrivateHistoryAggregateField::Event)?
            .group_limit(17)?
            .fetch_group_page(options("tenant-alpha:revision-1", true), cursor.as_ref())
            .await?;
        assert!(page.rows.len() <= 17);
        for row in &page.rows {
            match &row.groups[0].value {
                AggregateValue::Text(value) => actual.push(value.clone()),
                other => panic!("unexpected group {other:?}"),
            }
            assert!(row.metrics.is_empty());
        }
        cursor = page
            .end_cursor
            .map(|cursor| AggregateGroupCursor::from_json(&cursor.to_json().unwrap()).unwrap());
        if !page.has_next_page {
            break;
        }
    }
    // Test-only SQL oracle: exact original equality and default SQLite TRIM.
    // The executable consumer example contains no query SQL.
    let expected: Vec<String> = sqlx::query_scalar("SELECT DISTINCT event FROM private_history WHERE tenant=? AND TRIM(event) <> '' ORDER BY event COLLATE NOCASE ASC, event COLLATE BINARY ASC")
        .bind("alpha").fetch_all(db.pool()).await?;
    assert_eq!(actual, expected);
    assert_eq!(actual.len(), 235);
    assert!(actual.contains(&" CASE ".into()));
    assert!(
        actual.contains(&"\t".into()),
        "SQL TRIM only removes ordinary spaces"
    );
    let reads = observed.0.lock().unwrap();
    assert_eq!(reads.len(), 14);
    assert!(reads.iter().all(|(_, count)| *count <= 18));
    assert!(
        reads
            .iter()
            .skip(1)
            .all(|(sql, _)| sql.contains(" HAVING "))
    );
    Ok(())
}

#[tokio::test]
async fn nullable_and_native_nocase_groups_keep_complete_metrics_after_boundary()
-> graphql_orm::Result<()> {
    let db = database().await?;
    // This externally supplied synthetic table has native NOCASE group equality.
    sqlx::query("DROP TABLE nullable_groups")
        .execute(db.pool())
        .await?;
    sqlx::query("CREATE TABLE nullable_groups(id TEXT PRIMARY KEY NOT NULL, category TEXT COLLATE NOCASE, units INTEGER NOT NULL)").execute(db.pool()).await?;
    for (index, category) in [
        None,
        None,
        Some(""),
        Some("Alpha"),
        Some("alpha"),
        Some("Beta"),
        Some("beta"),
    ]
    .into_iter()
    .enumerate()
    {
        NullableGroup::insert(
            &db,
            CreateNullableGroupInput {
                id: index.to_string(),
                category: category.map(str::to_owned),
                units: 1,
            },
        )
        .await?;
    }
    let mut cursor = None;
    let mut rows = Vec::new();
    loop {
        let page = NullableGroup::aggregate(&db)
            .group_by(NullableGroupAggregateField::Category)?
            .sum(NullableGroupAggregateField::Units)?
            .group_limit(1)?
            .fetch_group_page(options("nullable-owner", false), cursor.as_ref())
            .await?;
        assert_eq!(page.rows.len(), 1);
        if cursor.is_none() {
            assert_eq!(page.end_cursor.as_ref().unwrap().key(), None);
        }
        rows.extend(page.rows);
        cursor = page.end_cursor;
        if !page.has_next_page {
            break;
        }
    }
    assert_eq!(
        rows.iter()
            .map(|row| row.groups[0].value.clone())
            .collect::<Vec<_>>(),
        vec![
            AggregateValue::Null,
            AggregateValue::Text("".into()),
            AggregateValue::Text("Alpha".into()),
            AggregateValue::Text("Beta".into())
        ]
    );
    assert_eq!(
        rows.iter()
            .map(|row| row.metrics[0].value.clone())
            .collect::<Vec<_>>(),
        vec![
            AggregateValue::Integral(2),
            AggregateValue::Integral(1),
            AggregateValue::Integral(2),
            AggregateValue::Integral(2)
        ]
    );
    Ok(())
}

struct SqlVisibility {
    tenant: Arc<Mutex<String>>,
    residual: bool,
}
impl RowPolicy<SqliteBackend> for SqlVisibility {
    fn read_visibility<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<SqliteBackend>,
        _entity: &'static str,
        _policy: Option<&'static str>,
        surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        assert_eq!(surface, EntityAccessSurface::Repository);
        let filter = tenant(&self.tenant.lock().unwrap());
        Box::pin(async move {
            let predicate = ReadPredicate::from_filter::<SqliteBackend, _>(&filter)?;
            Ok(if self.residual {
                ReadVisibility::Prefilter(predicate)
            } else {
                ReadVisibility::Complete(predicate)
            })
        })
    }
    fn can_read_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<SqliteBackend>,
        _entity: &'static str,
        _policy: Option<&'static str>,
        _surface: EntityAccessSurface,
        _row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { panic!("group authorization must be complete before aggregation") })
    }
    fn can_write_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<SqliteBackend>,
        _entity: &'static str,
        _policy: Option<&'static str>,
        _surface: EntityAccessSurface,
        _row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

#[tokio::test]
async fn current_complete_visibility_applies_before_groups_and_stale_cursor_fails_before_sql()
-> graphql_orm::Result<()> {
    let mut db = database().await?;
    for (id, owner, event) in [
        ("a1", "alpha", "A"),
        ("a2", "alpha", "B"),
        ("a3", "alpha", "B"),
        ("b", "beta", "Other"),
    ] {
        insert(&db, id, owner, event).await?;
    }
    let owner = Arc::new(Mutex::new("alpha".to_string()));
    db.set_row_policy(SqlVisibility {
        tenant: owner.clone(),
        residual: false,
    });
    let observed = Reads(Arc::new(Mutex::new(vec![])));
    let mut db = db.with_read_query_observer(observed.clone());
    let page = PrivateHistory::aggregate(&db)
        .group_by(PrivateHistoryAggregateField::Event)?
        .count_rows()?
        .group_limit(1)?
        .fetch_group_page(options("current-auth", true), None)
        .await?;
    assert!(page.has_next_page);
    assert_eq!(
        page.rows[0].groups[0].value,
        AggregateValue::Text("A".into())
    );
    let cursor = page.end_cursor.unwrap();
    let page = PrivateHistory::aggregate(&db)
        .group_by(PrivateHistoryAggregateField::Event)?
        .count_rows()?
        .group_limit(1)?
        .fetch_group_page(options("current-auth", true), Some(&cursor))
        .await?;
    assert!(!page.has_next_page);
    assert_eq!(page.rows[0].metrics[0].value, AggregateValue::Count(2));
    *owner.lock().unwrap() = "beta".into();
    assert!(
        PrivateHistory::aggregate(&db)
            .group_by(PrivateHistoryAggregateField::Event)?
            .count_rows()?
            .group_limit(1)?
            .fetch_group_page(options("current-auth", true), Some(&cursor))
            .await
            .is_err()
    );
    assert_eq!(observed.0.lock().unwrap().len(), 2);
    let page = PrivateHistory::aggregate(&db)
        .group_by(PrivateHistoryAggregateField::Event)?
        .fetch_group_page(options("current-auth", true), None)
        .await?;
    assert_eq!(
        page.rows[0].groups[0].value,
        AggregateValue::Text("Other".into())
    );
    db.set_row_policy(SqlVisibility {
        tenant: owner,
        residual: true,
    });
    assert!(
        PrivateHistory::aggregate(&db)
            .group_by(PrivateHistoryAggregateField::Event)?
            .fetch_group_page(options("current-auth", true), None)
            .await
            .is_err()
    );
    assert_eq!(observed.0.lock().unwrap().len(), 3);
    Ok(())
}

#[tokio::test]
async fn cursor_shape_context_order_filter_and_limits_are_checked_before_read()
-> graphql_orm::Result<()> {
    let db = database().await?;
    insert(&db, "a", "alpha", "A").await?;
    insert(&db, "b", "alpha", "B").await?;
    let observed = Reads(Arc::new(Mutex::new(vec![])));
    let db = db.with_read_query_observer(observed.clone());
    let first = PrivateHistory::aggregate(&db)
        .filter(tenant("alpha"))
        .group_by(PrivateHistoryAggregateField::Event)?
        .group_limit(1)?
        .fetch_group_page(options("alpha:r1", true), None)
        .await?;
    let cursor = first.end_cursor.unwrap();
    for opts in [
        options("beta:r1", true),
        options("alpha:r2", true),
        options("alpha:r1", false),
        AggregateGroupPageOptions {
            order: AggregateGroupOrder::Binary,
            ..options("alpha:r1", true)
        },
    ] {
        assert!(
            PrivateHistory::aggregate(&db)
                .filter(tenant("alpha"))
                .group_by(PrivateHistoryAggregateField::Event)?
                .fetch_group_page(opts, Some(&cursor))
                .await
                .is_err()
        );
    }
    assert!(
        PrivateHistory::aggregate(&db)
            .filter(tenant("beta"))
            .group_by(PrivateHistoryAggregateField::Event)?
            .fetch_group_page(options("alpha:r1", true), Some(&cursor))
            .await
            .is_err()
    );
    let mut wire: serde_json::Value = serde_json::from_str(&cursor.to_json()?).unwrap();
    for bad in [
        serde_json::json!(3),
        serde_json::json!(true),
        serde_json::json!(["A"]),
        serde_json::json!({"unexpected":"A"}),
        serde_json::json!("x".repeat(65537)),
    ] {
        wire["key"] = bad;
        assert!(AggregateGroupCursor::from_json(&wire.to_string()).is_err());
    }
    wire["key"] = serde_json::Value::Null;
    let wrong_null = AggregateGroupCursor::from_json(&wire.to_string())?;
    assert!(
        PrivateHistory::aggregate(&db)
            .filter(tenant("alpha"))
            .group_by(PrivateHistoryAggregateField::Event)?
            .fetch_group_page(options("alpha:r1", true), Some(&wrong_null))
            .await
            .is_err()
    );
    wire.as_object_mut().unwrap().remove("key");
    assert!(AggregateGroupCursor::from_json(&wire.to_string()).is_err());
    assert!(AggregateGroupCursor::from_json(&"x".repeat(256 * 1024 + 1)).is_err());
    assert!(
        PrivateHistory::aggregate(&db)
            .group_by(PrivateHistoryAggregateField::Units)?
            .fetch_group_page(options("alpha", true), None)
            .await
            .is_err()
    );
    assert!(
        PrivateHistory::aggregate(&db)
            .group_by(PrivateHistoryAggregateField::Event)?
            .group_limit(101)
            .is_err()
    );
    let db = db.with_pagination_config(PaginationConfig::unbounded());
    assert!(
        PrivateHistory::aggregate(&db)
            .group_by(PrivateHistoryAggregateField::Event)?
            .group_limit(1001)?
            .fetch_group_page(options("alpha", true), None)
            .await
            .is_err()
    );
    assert_eq!(observed.0.lock().unwrap().len(), 1);
    Ok(())
}

struct DenyField(&'static str);
impl FieldPolicy<SqliteBackend> for DenyField {
    fn can_read_field<'a>(
        &'a self,
        _ctx: &'a async_graphql::Context<'_>,
        _db: &'a Database<SqliteBackend>,
        _entity: &'static str,
        _field: &'static str,
        _policy: Option<&'static str>,
        _record: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
    fn can_write_field<'a>(
        &'a self,
        _ctx: &'a async_graphql::Context<'_>,
        _db: &'a Database<SqliteBackend>,
        _entity: &'static str,
        _field: &'static str,
        _policy: Option<&'static str>,
        _record: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _value: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
    fn can_read_repository_field<'a>(
        &'a self,
        _access: Option<graphql_orm::graphql::auth::AccessContext<'a>>,
        _db: &'a Database<SqliteBackend>,
        _entity: &'static str,
        field: &'static str,
        _policy: Option<&'static str>,
        _record: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(field != self.0) })
    }
}

#[tokio::test]
async fn denied_group_metric_filter_and_entity_issue_no_aggregate_query() -> graphql_orm::Result<()>
{
    let mut db = database().await?;
    insert(&db, "a", "alpha", "A").await?;
    let observed = Reads(Arc::new(Mutex::new(vec![])));
    db = db.with_read_query_observer(observed.clone());
    db.set_field_policy(DenyField("event"));
    assert!(
        PrivateHistory::aggregate(&db)
            .group_by(PrivateHistoryAggregateField::Event)?
            .fetch_group_page(options("alpha", false), None)
            .await
            .is_err()
    );
    db.set_field_policy(DenyField("units"));
    assert!(
        PrivateHistory::aggregate(&db)
            .group_by(PrivateHistoryAggregateField::Event)?
            .sum(PrivateHistoryAggregateField::Units)?
            .fetch_group_page(options("alpha", false), None)
            .await
            .is_err()
    );
    db.set_field_policy(DenyField("tenant"));
    assert!(
        PrivateHistory::aggregate(&db)
            .filter(tenant("alpha"))
            .group_by(PrivateHistoryAggregateField::Event)?
            .fetch_group_page(options("alpha", false), None)
            .await
            .is_err()
    );
    db.set_entity_policy(ScopeEntityPolicy::authenticated());
    assert!(
        PrivateHistory::aggregate(&db)
            .group_by(PrivateHistoryAggregateField::Event)?
            .fetch_group_page(options("alpha", false), None)
            .await
            .is_err()
    );
    assert!(observed.0.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn thousand_group_page_uses_one_bounded_lookahead() -> graphql_orm::Result<()> {
    let db = database()
        .await?
        .with_pagination_config(PaginationConfig::legacy());
    for index in 0..1001 {
        insert(
            &db,
            &index.to_string(),
            "alpha",
            &format!("Event-{index:04}"),
        )
        .await?;
    }
    let observed = Reads(Arc::new(Mutex::new(vec![])));
    let db = db.with_read_query_observer(observed.clone());
    let first = PrivateHistory::aggregate(&db)
        .group_by(PrivateHistoryAggregateField::Event)?
        .group_limit(1000)?
        .fetch_group_page(options("alpha", true), None)
        .await?;
    assert_eq!(first.rows.len(), 1000);
    assert!(first.has_next_page);
    let last = PrivateHistory::aggregate(&db)
        .group_by(PrivateHistoryAggregateField::Event)?
        .group_limit(1000)?
        .fetch_group_page(options("alpha", true), first.end_cursor.as_ref())
        .await?;
    assert_eq!(last.rows.len(), 1);
    assert!(!last.has_next_page);
    assert_eq!(
        last.rows[0].groups[0].value,
        AggregateValue::Text("Event-1000".into())
    );
    assert_eq!(
        observed
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|(_, rows)| *rows)
            .collect::<Vec<_>>(),
        vec![1001, 1]
    );
    Ok(())
}
