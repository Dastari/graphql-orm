#![cfg(all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))))]

use graphql_orm::prelude::*;
use graphql_orm::rust_decimal::Decimal;

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "postgres",
    table = "grouped_aggregate_pg_rows",
    plural = "GroupedAggregatePgRows",
    default_sort = "id ASC",
    auth = "none"
)]
struct GroupedAggregatePgRow {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    team: Option<String>,
    units: i64,
    hours: f64,
    #[filterable(type = "decimal")]
    #[graphql_orm(decimal(precision = 12, scale = 2))]
    amount: Decimal,
}

#[tokio::test]
async fn postgres_grouped_multi_sum_matches_portable_contract()
-> Result<(), Box<dyn std::error::Error>> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL grouped aggregate parity: TEST_DATABASE_URL is unset");
        return Ok(());
    };
    let database = Database::<PostgresBackend>::connect_postgres(url).await?;
    graphql_orm::sqlx::query("DROP TABLE IF EXISTS grouped_aggregate_pg_rows")
        .execute(database.pool())
        .await?;
    let version = format!("grouped-aggregate-pg-{}", graphql_orm::uuid::Uuid::new_v4());
    let plan = database
        .schema()
        .plan_migration_to_entities(
            &version,
            "grouped aggregate PostgreSQL parity",
            &[GroupedAggregatePgRow::metadata()],
        )
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;

    for (id, team, units, hours, amount) in [
        ("a", Some("alpha"), 2_i64, 0.5_f64, Decimal::new(125, 2)),
        ("b", Some("alpha"), 3_i64, 1.5_f64, Decimal::new(250, 2)),
        ("c", None, 4_i64, 2.0_f64, Decimal::new(375, 2)),
    ] {
        graphql_orm::sqlx::query(
            "INSERT INTO grouped_aggregate_pg_rows (id, team, units, hours, amount) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(team)
        .bind(units)
        .bind(hours)
        .bind(amount)
        .execute(database.pool())
        .await?;
    }

    let rows = GroupedAggregatePgRow::aggregate(&database)
        .group_by(GroupedAggregatePgRowAggregateField::Team)?
        .count_rows()?
        .sum(GroupedAggregatePgRowAggregateField::Units)?
        .sum(GroupedAggregatePgRowAggregateField::Hours)?
        .sum(GroupedAggregatePgRowAggregateField::Amount)?
        .group_limit(10)?
        .fetch()
        .await?;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].groups[0].value, AggregateValue::Null);
    assert_eq!(rows[0].metrics[0].value, AggregateValue::Count(1));
    assert_eq!(rows[0].metrics[1].value, AggregateValue::Integral(4));
    assert_eq!(rows[0].metrics[2].value, AggregateValue::Floating(2.0));
    assert_eq!(
        rows[0].metrics[3].value,
        AggregateValue::Decimal(Decimal::new(375, 2))
    );
    assert_eq!(rows[1].metrics[0].value, AggregateValue::Count(2));
    assert_eq!(rows[1].metrics[1].value, AggregateValue::Integral(5));
    assert_eq!(rows[1].metrics[2].value, AggregateValue::Floating(2.0));
    assert_eq!(
        rows[1].metrics[3].value,
        AggregateValue::Decimal(Decimal::new(375, 2))
    );
    Ok(())
}
