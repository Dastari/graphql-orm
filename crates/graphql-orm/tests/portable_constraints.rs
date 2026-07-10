#![cfg(feature = "sqlite")]

use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "constrained_records", plural = "ConstrainedRecords")]
struct ConstrainedRecord {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[graphql_orm(non_negative, max = 100)]
    amount: i64,
    #[graphql_orm(min_length = 2, max_length = 8)]
    #[filterable(type = "string")]
    #[sortable]
    code: String,
    #[graphql_orm(min_length = 2, max_length = 4)]
    bytes: Vec<u8>,
    #[graphql_orm(one_of = ["pending", "running", "done"])]
    status: String,
    created_at: i64,
    #[graphql_orm(gte_field = "created_at")]
    updated_at: i64,
}

fn input(amount: i64, code: &str, bytes: &[u8], status: &str) -> CreateConstrainedRecordInput {
    CreateConstrainedRecordInput {
        amount,
        code: code.to_string(),
        bytes: bytes.to_vec(),
        status: status.to_string(),
    }
}

#[tokio::test]
async fn portable_constraints_render_enforce_hash_and_reopen_without_drift()
-> graphql_orm::Result<()> {
    let path = std::env::temp_dir().join(format!(
        "graphql-orm-constraints-{}.sqlite",
        graphql_orm::uuid::Uuid::new_v4()
    ));
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let database = Database::<SqliteBackend>::connect_sqlite(&url).await?;
    let entities = [ConstrainedRecord::metadata()];
    let target = SchemaModel::from_entities(&entities);
    assert_eq!(target.tables[0].check_constraints.len(), 8);
    let target_hash = target.stable_hash();
    let mut without_constraints = target.clone();
    without_constraints.tables[0].check_constraints.clear();
    assert_ne!(target_hash, without_constraints.stable_hash());
    let plan = database
        .schema()
        .plan_migration_to_entities("constraints-init", "portable checks", &entities)
        .await?;
    assert!(plan.statements.iter().any(|sql| {
        sql.contains("graphql_orm_check_constrained_records_status_one_of")
            && sql.contains("status IN ('pending', 'running', 'done')")
    }));
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;

    ConstrainedRecord::insert(&database, input(10, "valid", &[1, 2], "pending")).await?;
    for invalid in [
        input(-1, "valid", &[1, 2], "pending"),
        input(101, "valid", &[1, 2], "pending"),
        input(1, "x", &[1, 2], "pending"),
        input(1, "valid", &[1], "pending"),
        input(1, "valid", &[1, 2], "unknown"),
    ] {
        let error = ConstrainedRecord::insert(&database, invalid)
            .await
            .expect_err("constraint rejects invalid row");
        assert_eq!(
            OrmPublicError::from_sqlx(&error).code,
            OrmErrorCode::ConstraintViolation
        );
    }
    let cross_field_error = graphql_orm::sqlx::query(
        "INSERT INTO constrained_records
         (id, amount, code, bytes, status, created_at, updated_at)
         VALUES (?, 1, 'valid', X'0102', 'pending', 10, 9)",
    )
    .bind(graphql_orm::uuid::Uuid::new_v4().to_string())
    .execute(database.pool())
    .await
    .expect_err("cross-field check rejects invalid timestamps");
    assert_eq!(
        OrmPublicError::from_sqlx(&cross_field_error).code,
        OrmErrorCode::ConstraintViolation
    );
    drop(database);

    let reopened = Database::<SqliteBackend>::connect_sqlite(&url).await?;
    let live = introspect_sqlite_schema(&reopened).await?;
    let live_table = live
        .tables
        .iter()
        .find(|table| table.table_name == "constrained_records")
        .expect("constraint table introspected");
    assert_eq!(
        live_table.check_constraints,
        target.tables[0].check_constraints
    );
    let weakening = reopened.schema().plan_migration(
        "constraints-weaken",
        "remove checks",
        &live,
        &without_constraints,
    )?;
    assert!(
        weakening
            .steps
            .iter()
            .any(|step| matches!(step.step, MigrationStep::SetCheckConstraints { .. }))
    );
    assert!(
        weakening
            .statements
            .iter()
            .any(|sql| sql.contains("__graphql_orm_constrained_records_new"))
    );
    let restart = reopened
        .schema()
        .plan_migration_to_entities("constraints-restart", "restart", &entities)
        .await?;
    assert!(restart.steps.is_empty(), "unexpected drift: {restart:?}");
    drop(reopened);
    let _ = std::fs::remove_file(path);
    Ok(())
}
