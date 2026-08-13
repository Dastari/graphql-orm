#![cfg(feature = "sqlite")]

use graphql_orm::prelude::*;
use graphql_orm::rust_decimal::Decimal;

/// Work records used to prove database-side grouped aggregation.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "aggregate_work_records",
    plural = "AggregateWorkRecords",
    default_sort = "id ASC",
    aggregate = true,
    auth = "none"
)]
struct AggregateWorkRecord {
    /// Stable work-record identity.
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    /// Optional team used as a nullable grouping key.
    #[filterable(type = "string")]
    #[sortable]
    team: Option<String>,
    /// Work category used as the second grouping key.
    #[filterable(type = "string")]
    #[sortable]
    category: String,
    /// Integral work units.
    #[filterable(type = "number")]
    units: i64,
    /// Floating work duration.
    hours: f64,
    /// Exact fixed-precision cost.
    #[filterable(type = "decimal")]
    #[sortable]
    #[graphql_orm(decimal(precision = 12, scale = 2), default = "1.25")]
    amount: Decimal,
    /// Optional exact adjustment used to cover SQL NULL round trips.
    #[filterable(type = "decimal")]
    #[sortable]
    #[graphql_orm(decimal(precision = 12, scale = 2))]
    adjustment: Option<Decimal>,
}

schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [AggregateWorkRecord],
}

struct ApplicationRowPolicy;

impl RowPolicy<SqliteBackend> for ApplicationRowPolicy {
    fn can_read_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<SqliteBackend>,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: EntityAccessSurface,
        _row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }

    fn can_write_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<SqliteBackend>,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: EntityAccessSurface,
        _row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

async fn database() -> graphql_orm::Result<Database<SqliteBackend>> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "grouped-aggregate-init",
            "grouped aggregate test schema",
            &[AggregateWorkRecord::metadata()],
        )
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    Ok(database)
}

#[tokio::test]
async fn grouped_multi_sum_uses_every_source_row_before_group_limit() -> graphql_orm::Result<()> {
    let database = database().await?;
    for id in 1..=300_i64 {
        let team = if id <= 100 {
            Some("alpha")
        } else if id <= 200 {
            Some("beta")
        } else {
            None
        };
        let category = if id % 2 == 0 { "field" } else { "shop" };
        graphql_orm::sqlx::query(
            "INSERT INTO aggregate_work_records (id, team, category, units, hours, amount, adjustment) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(format!("record-{id:03}"))
        .bind(team)
        .bind(category)
        .bind(1_i64)
        .bind(0.5_f64)
        .bind(125_i64)
        .bind(Option::<i64>::None)
        .execute(database.pool())
        .await?;
    }

    let rows = AggregateWorkRecord::aggregate(&database)
        .group_by(AggregateWorkRecordAggregateField::Team)?
        .group_by(AggregateWorkRecordAggregateField::Category)?
        .count_rows()?
        .sum(AggregateWorkRecordAggregateField::Units)?
        .sum(AggregateWorkRecordAggregateField::Hours)?
        .sum(AggregateWorkRecordAggregateField::Amount)?
        .sum(AggregateWorkRecordAggregateField::Adjustment)?
        .group_limit(10)?
        .fetch()
        .await?;

    assert_eq!(rows.len(), 6);
    assert_eq!(rows[0].groups[0].value, AggregateValue::Null);
    for row in rows {
        assert_eq!(row.metrics[0].value, AggregateValue::Count(50));
        assert_eq!(row.metrics[1].value, AggregateValue::Integral(50));
        assert_eq!(row.metrics[2].value, AggregateValue::Floating(25.0));
        assert_eq!(
            row.metrics[3].value,
            AggregateValue::Decimal(Decimal::new(6250, 2))
        );
        assert_eq!(row.metrics[4].value, AggregateValue::Null);
    }
    Ok(())
}

#[tokio::test]
async fn decimal_defaults_nulls_filters_and_ordering_round_trip() -> graphql_orm::Result<()> {
    let database = database().await?;
    graphql_orm::sqlx::query(
        "INSERT INTO aggregate_work_records (id, team, category, units, hours, adjustment) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind("defaulted")
    .bind("alpha")
    .bind("shop")
    .bind(1_i64)
    .bind(0.5_f64)
    .bind(Option::<i64>::None)
    .execute(database.pool())
    .await?;
    graphql_orm::sqlx::query(
        "INSERT INTO aggregate_work_records (id, team, category, units, hours, amount, adjustment) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("larger")
    .bind("alpha")
    .bind("shop")
    .bind(1_i64)
    .bind(0.5_f64)
    .bind(950_i64)
    .bind(225_i64)
    .execute(database.pool())
    .await?;

    let ordered = AggregateWorkRecord::query(database.pool())
        .filter(AggregateWorkRecordWhereInput {
            amount: Some(DecimalFilter {
                gte: Some(Decimal::new(125, 2)),
                ..Default::default()
            }),
            ..Default::default()
        })
        .order_by(AggregateWorkRecordOrderByInput {
            amount: Some(OrderDirection::Desc),
            ..Default::default()
        })
        .fetch_all()
        .await?;
    assert_eq!(ordered.len(), 2);
    assert_eq!(ordered[0].id, "larger");
    assert_eq!(ordered[0].amount, Decimal::new(950, 2));
    assert_eq!(ordered[0].adjustment, Some(Decimal::new(225, 2)));
    assert_eq!(ordered[1].amount, Decimal::new(125, 2));
    assert_eq!(ordered[1].adjustment, None);

    let null_adjustment = AggregateWorkRecord::query(database.pool())
        .filter(AggregateWorkRecordWhereInput {
            adjustment: Some(DecimalFilter {
                is_null: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        })
        .fetch_all()
        .await?;
    assert_eq!(null_adjustment.len(), 1);
    assert_eq!(null_adjustment[0].id, "defaulted");
    Ok(())
}

#[tokio::test]
async fn empty_and_all_null_aggregate_semantics_are_explicit() -> graphql_orm::Result<()> {
    let database = database().await?;
    let empty = AggregateWorkRecord::aggregate(&database)
        .count_rows()?
        .count(AggregateWorkRecordAggregateField::Adjustment)?
        .min(AggregateWorkRecordAggregateField::Amount)?
        .max(AggregateWorkRecordAggregateField::Amount)?
        .sum(AggregateWorkRecordAggregateField::Adjustment)?
        .fetch()
        .await?;
    assert_eq!(empty.len(), 1);
    assert_eq!(empty[0].metrics[0].value, AggregateValue::Count(0));
    assert_eq!(empty[0].metrics[1].value, AggregateValue::Count(0));
    assert_eq!(empty[0].metrics[2].value, AggregateValue::Null);
    assert_eq!(empty[0].metrics[3].value, AggregateValue::Null);
    assert_eq!(empty[0].metrics[4].value, AggregateValue::Null);

    assert!(
        AggregateWorkRecord::aggregate(&database)
            .count_rows()?
            .group_limit(0)
            .is_err()
    );
    assert!(
        AggregateWorkRecord::aggregate(&database)
            .count_rows()?
            .group_limit(101)
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn application_row_policy_cannot_be_applied_after_aggregation() -> graphql_orm::Result<()> {
    let mut database = database().await?;
    database.set_row_policy(ApplicationRowPolicy);
    let result = AggregateWorkRecord::aggregate(&database)
        .count_rows()?
        .fetch()
        .await;
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn aggregate_root_is_opt_in_and_catalogued() -> graphql_orm::Result<()> {
    let database = database().await?;
    graphql_orm::sqlx::query(
        "INSERT INTO aggregate_work_records (id, team, category, units, hours, amount) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind("graphql-row")
    .bind("alpha")
    .bind("shop")
    .bind(3_i64)
    .bind(1.5_f64)
    .bind(250_i64)
    .execute(database.pool())
    .await?;
    let schema = schema_builder(database).finish();
    let sdl = schema.sdl();
    let descriptor = AggregateWorkRecord::generated_graphql_operations()
        .iter()
        .find(|operation| operation.category() == GeneratedGraphqlOperationCategory::Aggregate)
        .expect("opted-in aggregate descriptor");
    assert_eq!(descriptor.kind(), GraphqlOperationKind::Query);
    assert_eq!(descriptor.arguments().len(), 4);
    assert!(sdl.contains(descriptor.field_name()), "{sdl}");
    let response = schema
        .execute(
            "{ aggregateWorkRecordsAggregate(\
                groupBy: [team], \
                metrics: [{operator: COUNT}, {operator: SUM, field: units}], \
                groupLimit: 10\
            ) { groups { field kind value } metrics { field operator kind value } } }",
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response
        .data
        .into_json()
        .expect("aggregate response should serialize");
    let row = &data["aggregateWorkRecordsAggregate"][0];
    assert_eq!(row["groups"][0]["value"], "alpha");
    assert_eq!(row["metrics"][0]["value"], "1");
    assert_eq!(row["metrics"][1]["value"], "3");
    Ok(())
}
