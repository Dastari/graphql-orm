#![cfg(any(feature = "sqlite", feature = "postgres"))]

use graphql_orm::prelude::*;
use repository_aggregate_consumer::*;

struct RepositoryFields(bool);

impl FieldPolicy<Backend> for RepositoryFields {
    fn can_read_field<'a>(
        &'a self,
        _ctx: &'a graphql_orm::async_graphql::Context<'_>,
        _db: &'a Database<Backend>,
        _entity: &'static str,
        _field: &'static str,
        _policy: Option<&'static str>,
        _record: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }

    fn can_write_field<'a>(
        &'a self,
        _ctx: &'a graphql_orm::async_graphql::Context<'_>,
        _db: &'a Database<Backend>,
        _entity: &'static str,
        _field: &'static str,
        _policy: Option<&'static str>,
        _record: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _value: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }

    fn can_read_repository_field<'a>(
        &'a self,
        _access: Option<AccessContext<'a>>,
        _db: &'a Database<Backend>,
        _entity: &'static str,
        _field: &'static str,
        policy: Option<&'static str>,
        _record: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        Box::pin(async move { Ok(self.0 || policy.is_none()) })
    }
}

async fn verify(mut database: Database<Backend>) -> Result<(), Box<dyn std::error::Error>> {
    database.set_field_policy(RepositoryFields(true));
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "repository-aggregate-consumer",
            "plain repository aggregates",
            &[ScalarAggregate::metadata(), CompositeAggregate::metadata()],
        )
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    build_aggregates(&database)?;

    database
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                for (id, team, units) in [
                    ("a", Some("alpha"), 2),
                    ("b", Some("alpha"), 3),
                    ("c", None, 4),
                ] {
                    tx.insert::<ScalarAggregate>(CreateScalarAggregateInput {
                        id: id.into(),
                        team: team.map(str::to_owned),
                        units,
                        secret: Some("protected".into()),
                        unreadable: Some("hidden".into()),
                    })
                    .await?;
                }
                Ok(())
            })
        })
        .await?;

    graphql_orm::sqlx::query(
        "INSERT INTO repository_aggregate_composite (tenant, item, units, secret) VALUES ('tenant', 'item', 9, NULL)",
    )
    .execute(database.pool())
    .await?;

    let groups = ScalarAggregate::aggregate(&database)
        .group_by(ScalarAggregateAggregateField::Team)?
        .count_rows()?
        .sum(ScalarAggregateAggregateField::Units)?
        .fetch()
        .await?;
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].groups[0].value, AggregateValue::Null);
    assert_eq!(groups[0].metrics[0].value, AggregateValue::Count(1));
    assert_eq!(groups[0].metrics[1].value, AggregateValue::Integral(4));
    assert_eq!(
        groups[1].groups[0].value,
        AggregateValue::Text("alpha".into())
    );
    assert_eq!(groups[1].metrics[0].value, AggregateValue::Count(2));
    assert_eq!(groups[1].metrics[1].value, AggregateValue::Integral(5));
    let composite = CompositeAggregate::aggregate(&database)
        .group_by(CompositeAggregateAggregateField::Tenant)?
        .sum(CompositeAggregateAggregateField::Units)?
        .fetch()
        .await?;
    assert_eq!(composite[0].metrics[0].value, AggregateValue::Integral(9));

    database.set_field_policy(RepositoryFields(false));
    let protected = database.with_authorization_mode(AuthorizationMode::DeclaredPoliciesRequired);
    reset_query_count();
    let error = ScalarAggregate::aggregate(&protected)
        .sum(ScalarAggregateAggregateField::Units)?
        .fetch()
        .await
        .expect_err("denied declared field authority denies aggregates");
    assert_eq!(query_count(), 0, "denial must precede aggregate SQL");
    let public = OrmPublicError::from(error);
    assert_eq!(public.code, OrmErrorCode::Forbidden);
    assert!(!public.to_string().contains("protected"));
    protected.pool().close().await;
    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_plain_repository_aggregates_and_field_denial()
-> Result<(), Box<dyn std::error::Error>> {
    verify(Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?).await
}

#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[path = "../../../support/owned_postgres.rs"]
mod owned_postgres;

#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[tokio::test]
#[ignore = "starts an owned loopback-only PostgreSQL container"]
async fn postgres_plain_repository_aggregates_and_field_denial()
-> Result<(), Box<dyn std::error::Error>> {
    for variable in [
        "DATABASE_URL",
        "TEST_DATABASE_URL",
        "MSSQL_TEST_DATABASE_URL",
    ] {
        assert!(
            std::env::var_os(variable).is_none_or(|value| value.is_empty()),
            "refusing ambient {variable}"
        );
    }
    let mut owned = owned_postgres::OwnedPostgres::start("repository-aggregate-consumer")?;
    let result = verify(Database::<PostgresBackend>::connect_postgres(&owned.url).await?).await;
    owned.cleanup()?;
    result
}
