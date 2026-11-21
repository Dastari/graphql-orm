// One backend per build: the generated code below names no backend.
#![cfg(all(feature = "postgres", not(feature = "sqlite")))]

use graphql_orm::prelude::*;
use std::sync::Arc;

#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use owned_postgres::OwnedPostgres;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "postgres",
    table = "retained_pg_events",
    plural = "RetainedPgEvents",
    append_only = true,
    retention_purge = "retained_pg_event.purge"
)]
#[graphql_rls(
    force = true,
    select(predicate = "current_setting('app.user_id', true) = 'retention-worker'"),
    insert(predicate = "current_setting('app.user_id', true) = 'retention-worker'")
)]
struct RetainedPgEvent {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    kind: String,
    payload: String,
}

schema_roots! {
    backend: "postgres",
    query_custom_ops: [],
    entities: [RetainedPgEvent],
}

#[derive(Clone)]
struct RetentionPolicy;

impl graphql_orm::graphql::orm::EntityPolicy<PostgresBackend> for RetentionPolicy {
    fn can_access_entity<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<PostgresBackend>,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if surface != EntityAccessSurface::RetentionMaintenance {
                return Ok(policy_key.is_none());
            }
            Ok(entity_name == "RetainedPgEvent"
                && policy_key == Some("retained_pg_event.purge")
                && kind == EntityAccessKind::Write)
        })
    }
}

fn create(kind: &str) -> CreateRetainedPgEventInput {
    CreateRetainedPgEventInput {
        kind: kind.to_string(),
        payload: "protected".to_string(),
    }
}

fn kind_filter(kind: &str) -> RetainedPgEventWhereInput {
    RetainedPgEventWhereInput {
        kind: Some(StringFilter {
            eq: Some(kind.to_string()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[tokio::test]
#[ignore = "creates and owns a disposable Docker PostgreSQL container"]
async fn disposable_postgres_retention_runtime_and_rls_parity()
-> Result<(), Box<dyn std::error::Error>> {
    let owned = OwnedPostgres::start_postgres_16("retention")?;
    let admin = graphql_orm::sqlx::PgPool::connect(&owned.url).await?;
    let app_password = graphql_orm::uuid::Uuid::new_v4().simple().to_string();
    graphql_orm::sqlx::query(&format!(
        "CREATE ROLE retention_app LOGIN PASSWORD '{app_password}'"
    ))
    .execute(&admin)
    .await?;
    graphql_orm::sqlx::query("GRANT ALL ON SCHEMA public TO retention_app")
        .execute(&admin)
        .await?;
    graphql_orm::sqlx::query(
        "DO $$ BEGIN EXECUTE format('GRANT CREATE ON DATABASE %I TO retention_app', current_database()); END $$",
    )
    .execute(&admin)
    .await?;
    let app_options = admin
        .connect_options()
        .as_ref()
        .clone()
        .username("retention_app")
        .password(&app_password);
    drop(admin);

    let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
        .connect_with(app_options)
        .await?;
    let mut database =
        Database::<PostgresBackend>::new(pool).with_schema_policy(SchemaPolicy::Managed);
    database.set_entity_policy(RetentionPolicy);
    let target = graphql_orm_schema_target();
    let plan = database
        .schema()
        .plan_schema_target("retention-pg-v1", "retention parity", &target)
        .await?;
    database
        .schema()
        .apply_schema_target(&plan, ApplyOptions::default())
        .await?;

    let auth = DbAuthContext {
        user_id: Some("retention-worker".to_string()),
        subject: Some("retention-worker".to_string()),
        correlation_id: Some("retention-test".to_string()),
        ..Default::default()
    };
    database
        .retention_transaction_with_auth(Some(&auth), |maintenance| {
            Box::pin(async move {
                maintenance
                    .insert::<RetainedPgEvent>(create("expired"))
                    .await?;
                maintenance
                    .insert::<RetainedPgEvent>(create("keep"))
                    .await?;
                Ok(())
            })
        })
        .await?;
    let ordinary_delete = graphql_orm::sqlx::query("DELETE FROM retained_pg_events")
        .execute(database.pool())
        .await?;
    assert_eq!(ordinary_delete.rows_affected(), 0);
    let admin = graphql_orm::sqlx::PgPool::connect(&owned.url).await?;
    assert!(
        graphql_orm::sqlx::query("DELETE FROM retained_pg_events")
            .execute(&admin)
            .await
            .is_err(),
        "append-only trigger must reject a DELETE even for the disposable test administrator"
    );
    assert!(
        graphql_orm::sqlx::query("UPDATE retained_pg_events SET payload = 'tampered'")
            .execute(&admin)
            .await
            .is_err(),
        "append-only trigger must reject an UPDATE even for the disposable test administrator"
    );
    drop(admin);
    let normal_authenticated_delete = database
        .transaction_with_auth(TransactionMode::StateMachine, Some(&auth), |context| {
            Box::pin(async move {
                graphql_orm::sqlx::query("DELETE FROM retained_pg_events")
                    .execute(context.executor()?)
                    .await
                    .map(|result| result.rows_affected())
                    .map_err(|_| OrmPublicError::new(OrmErrorCode::ConstraintViolation))
            })
        })
        .await;
    assert_eq!(normal_authenticated_delete?, 0);
    let normal_authenticated_update = database
        .transaction_with_auth(TransactionMode::StateMachine, Some(&auth), |context| {
            Box::pin(async move {
                graphql_orm::sqlx::query("UPDATE retained_pg_events SET payload = 'tampered'")
                    .execute(context.executor()?)
                    .await
                    .map(|result| result.rows_affected())
                    .map_err(|_| OrmPublicError::new(OrmErrorCode::ConstraintViolation))
            })
        })
        .await;
    assert_eq!(normal_authenticated_update?, 0);

    let mut changed_events = database
        .ensure_event_sender::<RetainedPgEventChangedEvent>()
        .subscribe();
    let outcome = database
        .retention_transaction_with_auth(Some(&auth), |maintenance| {
            Box::pin(async move {
                maintenance
                    .purge::<RetainedPgEvent>(kind_filter("expired"), MutationLimit::new(1)?)
                    .await
                    .map_err(Into::into)
            })
        })
        .await?;
    assert_eq!(outcome, RetentionPurgeOutcome::Purged { affected: 1 });
    assert_eq!(
        changed_events
            .recv()
            .await
            .expect("post-commit PostgreSQL change event")
            .action,
        ChangeAction::Deleted
    );
    assert!(matches!(
        changed_events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    let leaked: Option<String> = graphql_orm::sqlx::query_scalar(
        "SELECT current_setting('graphql_orm.retention_entity', true)",
    )
    .fetch_one(database.pool())
    .await?;
    assert!(leaked.as_deref().unwrap_or_default().is_empty());

    database
        .retention_transaction_with_auth(Some(&auth), |maintenance| {
            Box::pin(async move {
                maintenance
                    .insert::<RetainedPgEvent>(create("rollback"))
                    .await?;
                maintenance
                    .insert::<RetainedPgEvent>(create("cancel"))
                    .await?;
                Ok(())
            })
        })
        .await?;
    let mut purge_events = database
        .ensure_event_sender::<RetentionPurgeEvent>()
        .subscribe();
    let rollback = database
        .retention_transaction_with_auth(Some(&auth), |maintenance| {
            Box::pin(async move {
                let _ = maintenance
                    .purge::<RetainedPgEvent>(kind_filter("rollback"), MutationLimit::new(1)?)
                    .await?;
                Err::<(), _>(OrmPublicError::new(OrmErrorCode::Conflict))
            })
        })
        .await;
    assert!(matches!(rollback, Err(TransactionError::Rejected(_))));
    assert!(matches!(
        purge_events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));

    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let task_database = database.clone();
    let task_auth = auth.clone();
    let task_started = started.clone();
    let task_release = release.clone();
    let task = tokio::spawn(async move {
        task_database
            .retention_transaction_with_auth(Some(&task_auth), |maintenance| {
                Box::pin(async move {
                    let _ = maintenance
                        .purge::<RetainedPgEvent>(kind_filter("cancel"), MutationLimit::new(1)?)
                        .await?;
                    task_started.notify_one();
                    task_release.notified().await;
                    Ok(())
                })
            })
            .await
    });
    started.notified().await;
    task.abort();
    let _ = task.await;
    assert!(matches!(
        purge_events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));

    for kind in ["rollback", "cancel"] {
        let expected_kind = kind.to_string();
        let rows = database
            .retention_transaction_with_auth(Some(&auth), |maintenance| {
                Box::pin(async move {
                    maintenance
                        .query::<RetainedPgEvent>()
                        .filter(kind_filter(&expected_kind))
                        .fetch_all()
                        .await
                        .map_err(Into::into)
                })
            })
            .await?;
        assert_eq!(rows.len(), 1, "{kind} purge must have rolled back");
    }
    let leaked_after_cancel: Option<String> = graphql_orm::sqlx::query_scalar(
        "SELECT current_setting('graphql_orm.retention_entity', true)",
    )
    .fetch_one(database.pool())
    .await?;
    assert!(
        leaked_after_cancel
            .as_deref()
            .unwrap_or_default()
            .is_empty()
    );

    let live = introspect_postgres_schema(&database).await?;
    let table = live
        .tables
        .iter()
        .find(|table| table.table_name == "retained_pg_events")
        .expect("retained PostgreSQL table");
    assert!(table.append_only);
    assert!(table.retention_purge);

    graphql_orm::sqlx::query(
        "CREATE OR REPLACE FUNCTION graphql_orm_append_only_retained_pg_events()
         RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog
         AS $$ BEGIN RETURN OLD; END $$",
    )
    .execute(database.pool())
    .await?;
    let tampered = introspect_postgres_schema(&database).await?;
    let tampered_table = tampered
        .tables
        .iter()
        .find(|table| table.table_name == "retained_pg_events")
        .expect("tampered PostgreSQL table");
    assert!(!tampered_table.append_only);
    assert!(!tampered_table.retention_purge);
    let repair = database
        .schema()
        .plan_schema_target(
            "retention-pg-v1",
            "recorded retention enforcement drift",
            &target,
        )
        .await?;
    assert!(!repair.statements.is_empty());
    database
        .schema()
        .apply_schema_target(&repair, ApplyOptions::default())
        .await
        .expect_err("a recorded PostgreSQL version with weakened enforcement must fail closed");
    drop(database);
    drop(owned);
    Ok(())
}
