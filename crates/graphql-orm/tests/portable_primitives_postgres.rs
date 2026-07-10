#![cfg(all(feature = "postgres", not(feature = "sqlite")))]

use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "pg_state_rows",
    plural = "PgStateRows",
    backend = "postgres",
    keyset = "status asc, id asc"
)]
struct PgStateRow {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    status: String,
    payload: String,
    #[graphql_orm(version, default = "0")]
    #[filterable(type = "number")]
    version: i64,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "pg_events",
    plural = "PgEvents",
    backend = "postgres",
    append_only = true
)]
struct PgEvent {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    kind: String,
}

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    table = "pg_constrained",
    plural = "PgConstrained",
    backend = "postgres"
)]
#[allow(dead_code)]
struct PgConstrained {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[graphql_orm(non_negative, max = 10)]
    amount: i64,
    #[graphql_orm(min_length = 1, max_length = 20, one_of = ["open", "closed"])]
    status: String,
    created_at: i64,
    #[graphql_orm(gte_field = "created_at")]
    updated_at: i64,
}

#[test]
fn postgres_rendering_covers_append_only_and_portable_checks() {
    let target = SchemaModel::from_entities(&[PgEvent::metadata(), PgConstrained::metadata()]);
    let plan = build_migration_plan(
        DatabaseBackend::Postgres,
        &SchemaModel {
            extensions: vec![],
            tables: vec![],
        },
        &target,
    );
    let sql = plan.statements.join("\n");
    assert!(sql.contains("SECURITY DEFINER SET search_path = pg_catalog"));
    assert!(sql.contains("REVOKE ALL ON FUNCTION graphql_orm_append_only_pg_events() FROM PUBLIC"));
    assert!(sql.contains("BEFORE UPDATE OR DELETE ON pg_events"));
    assert!(sql.contains("graphql_orm_check_pg_constrained_amount_non_negative"));
    assert!(sql.contains("status IN ('open', 'closed')"));
    assert!(sql.contains("updated_at >= created_at"));
}

#[tokio::test]
async fn postgres_live_portable_primitives_enforce_and_introspect()
-> Result<(), Box<dyn std::error::Error>> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping disposable PostgreSQL primitive test: TEST_DATABASE_URL is unset");
        return Ok(());
    };
    let database = Database::<PostgresBackend>::connect_postgres(url).await?;
    for table in ["pg_state_rows", "pg_events", "pg_constrained"] {
        graphql_orm::sqlx::query(&format!("DROP TABLE IF EXISTS {table} CASCADE"))
            .execute(database.pool())
            .await?;
    }
    graphql_orm::sqlx::query("DROP FUNCTION IF EXISTS graphql_orm_append_only_pg_events() CASCADE")
        .execute(database.pool())
        .await?;

    let entities = [
        PgStateRow::metadata(),
        PgEvent::metadata(),
        PgConstrained::metadata(),
    ];
    let target = SchemaModel::from_entities(&entities);
    // A previous interrupted run may have recorded its migration version before
    // the test reached cleanup. Use a fresh version so fail-closed migration
    // history does not mask the behavior this disposable test is exercising.
    let migration_version = format!("portable-pg-init-{}", uuid::Uuid::new_v4());
    let plan = database
        .schema()
        .plan_migration_to_entities_with_options(
            migration_version,
            "portable primitives",
            &entities,
            PlanOptions::managed_tables_only(),
        )
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;

    let event = PgEvent::insert(
        &database,
        CreatePgEventInput {
            kind: "created".to_string(),
        },
    )
    .await?;
    assert!(
        graphql_orm::sqlx::query("UPDATE pg_events SET kind = 'changed' WHERE id = $1")
            .bind(event.id)
            .execute(database.pool())
            .await
            .is_err()
    );
    assert!(
        graphql_orm::sqlx::query("DELETE FROM pg_events WHERE id = $1")
            .bind(event.id)
            .execute(database.pool())
            .await
            .is_err()
    );

    let check_error = graphql_orm::sqlx::query(
        "INSERT INTO pg_constrained (id, amount, status, created_at, updated_at)
         VALUES ($1, -1, 'invalid', 10, 9)",
    )
    .bind(graphql_orm::uuid::Uuid::new_v4())
    .execute(database.pool())
    .await
    .expect_err("portable PostgreSQL checks reject invalid data");
    assert_eq!(
        OrmPublicError::from_sqlx(&check_error).code,
        OrmErrorCode::ConstraintViolation
    );

    let live = introspect_postgres_schema(&database).await?;
    let live_event = live
        .tables
        .iter()
        .find(|table| table.table_name == "pg_events")
        .unwrap();
    assert!(live_event.append_only);
    let live_checks = live
        .tables
        .iter()
        .find(|table| table.table_name == "pg_constrained")
        .unwrap();
    let target_checks = target
        .tables
        .iter()
        .find(|table| table.table_name == "pg_constrained")
        .unwrap();
    assert_eq!(
        live_checks.check_constraints,
        target_checks.check_constraints
    );

    graphql_orm::sqlx::query("ALTER FUNCTION graphql_orm_append_only_pg_events() SECURITY INVOKER")
        .execute(database.pool())
        .await?;
    let weakened = introspect_postgres_schema(&database).await?;
    assert!(
        !weakened
            .tables
            .iter()
            .find(|table| table.table_name == "pg_events")
            .unwrap()
            .append_only
    );
    let repair =
        database
            .schema()
            .plan_migration("portable-pg-repair", "repair", &weakened, &target)?;
    assert!(repair.steps.iter().any(|step| matches!(
        step.step,
        MigrationStep::SetAppendOnly { enabled: true, .. }
    )));

    let state = PgStateRow::insert(
        &database,
        CreatePgStateRowInput {
            status: "pending".to_string(),
            payload: "one".to_string(),
        },
    )
    .await?;
    let updated = PgStateRow::compare_and_swap(
        &database,
        &state.id,
        0,
        PgStateRowWhereInput {
            status: Some(StringFilter {
                eq: Some("pending".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        },
        UpdatePgStateRowInput {
            status: None,
            payload: Some("two".to_string()),
        },
    )
    .await?;
    assert!(matches!(
        updated,
        ConditionalUpdateOutcome::Updated(PgStateRow { version: 1, .. })
    ));
    let page = PgStateRow::keyset_page(
        &database,
        PgStateRowWhereInput::default(),
        KeysetPageInput {
            limit: Some(1),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(page.edges.len(), 1);
    assert!(page.edges[0].cursor.starts_with("gomk1."));

    for table in ["pg_state_rows", "pg_events", "pg_constrained"] {
        graphql_orm::sqlx::query(&format!("DROP TABLE IF EXISTS {table} CASCADE"))
            .execute(database.pool())
            .await?;
    }
    graphql_orm::sqlx::query("DROP FUNCTION IF EXISTS graphql_orm_append_only_pg_events() CASCADE")
        .execute(database.pool())
        .await?;
    Ok(())
}

#[allow(dead_code)]
async fn postgres_public_apis_typecheck(
    database: &Database<PostgresBackend>,
    id: graphql_orm::uuid::Uuid,
) {
    let expected = PgStateRowWhereInput {
        status: Some(StringFilter {
            eq: Some("pending".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let _ = PgStateRow::compare_and_swap(
        database,
        &id,
        0,
        expected,
        UpdatePgStateRowInput {
            status: None,
            payload: Some("next".to_string()),
        },
    )
    .await;
    let _ = PgStateRow::keyset_page(
        database,
        PgStateRowWhereInput::default(),
        KeysetPageInput {
            limit: Some(10),
            ..Default::default()
        },
    )
    .await;
    let _ = database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move { tx.find_by_id::<PgStateRow>(&id).await.map_err(Into::into) })
        })
        .await;
}
