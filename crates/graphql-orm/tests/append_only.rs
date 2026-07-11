#![cfg(feature = "sqlite")]

use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "audit_events", plural = "AuditEvents", append_only = true)]
struct AuditEvent {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    kind: String,
    payload: String,
}

schema_roots! {
    query_custom_ops: [],
    entities: [AuditEvent],
}

fn input(kind: &str) -> CreateAuditEventInput {
    CreateAuditEventInput {
        kind: kind.to_string(),
        payload: "payload".to_string(),
    }
}

#[tokio::test]
async fn managed_append_only_storage_rejects_mutation_and_detects_tampering()
-> graphql_orm::Result<()> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let sdl = schema_builder(database.clone()).finish().sdl();
    assert!(sdl.contains("createAuditEvent"));
    assert!(!sdl.contains("updateAuditEvent"));
    assert!(!sdl.contains("deleteAuditEvent"));
    assert!(!sdl.contains("upsertAuditEvent"));
    let entities = [AuditEvent::metadata()];
    assert!(entities[0].append_only);
    let target = SchemaModel::from_entities(&entities);
    assert!(target.tables[0].append_only);
    let append_hash = target.stable_hash();
    let mut mutable_target = target.clone();
    mutable_target.tables[0].append_only = false;
    assert_ne!(append_hash, mutable_target.stable_hash());

    let plan = database
        .schema()
        .plan_migration_to_entities("append-only-init", "append-only test", &entities)
        .await?;
    assert!(
        plan.statements
            .iter()
            .any(|sql| sql.contains("CREATE TRIGGER"))
    );
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;

    let _event = AuditEvent::insert(&database, input("created")).await?;
    let update = graphql_orm::sqlx::query("UPDATE audit_events SET kind = 'changed'")
        .execute(database.pool())
        .await;
    assert!(update.is_err());
    let delete = graphql_orm::sqlx::query("DELETE FROM audit_events")
        .execute(database.pool())
        .await;
    assert!(delete.is_err());

    let live = introspect_sqlite_schema(&database).await?;
    let table = live
        .tables
        .iter()
        .find(|table| table.table_name == "audit_events")
        .expect("audit table introspected");
    assert!(table.append_only);
    let clean = database
        .schema()
        .plan_migration("append-only-clean", "clean", &live, &target)?;
    assert!(
        clean
            .steps
            .iter()
            .all(|step| !matches!(step.step, MigrationStep::SetAppendOnly { .. }))
    );

    graphql_orm::sqlx::query("CREATE TABLE audit_events_other (id TEXT PRIMARY KEY)")
        .execute(database.pool())
        .await?;
    let tampered_triggers = [
        (
            "graphql_orm_append_only_audit_events_update",
            "CREATE TRIGGER graphql_orm_append_only_audit_events_update
             BEFORE UPDATE ON audit_events BEGIN SELECT 1; END",
        ),
        (
            "graphql_orm_append_only_audit_events_delete",
            "CREATE TRIGGER graphql_orm_append_only_audit_events_delete
             BEFORE DELETE ON audit_events BEGIN SELECT 1; END",
        ),
        (
            "graphql_orm_append_only_audit_events_update",
            "CREATE TRIGGER graphql_orm_append_only_audit_events_update
             BEFORE UPDATE ON audit_events WHEN 0
             BEGIN SELECT RAISE(ABORT, 'append-only entity'); END",
        ),
        (
            "graphql_orm_append_only_audit_events_update",
            "CREATE TRIGGER graphql_orm_append_only_audit_events_update
             BEFORE DELETE ON audit_events BEGIN
             SELECT RAISE(ABORT, 'append-only entity'); END",
        ),
        (
            "graphql_orm_append_only_audit_events_update",
            "CREATE TRIGGER graphql_orm_append_only_audit_events_update
             BEFORE UPDATE ON audit_events_other BEGIN
             SELECT RAISE(ABORT, 'append-only entity'); END",
        ),
    ];
    let mut last_repair = None;
    for (trigger_name, replacement) in tampered_triggers {
        graphql_orm::sqlx::query(&format!("DROP TRIGGER {trigger_name}"))
            .execute(database.pool())
            .await?;
        graphql_orm::sqlx::query(replacement)
            .execute(database.pool())
            .await?;
        let tampered = introspect_sqlite_schema(&database).await?;
        assert!(
            !tampered
                .tables
                .iter()
                .find(|table| table.table_name == "audit_events")
                .expect("tampered table introspected")
                .append_only,
            "same-name replacement must not be accepted: {replacement}"
        );
        let repair = database.schema().plan_migration(
            "append-only-init",
            "recorded append-only version must fail",
            &tampered,
            &target,
        )?;
        assert!(repair.steps.iter().any(|step| matches!(
            step.step,
            MigrationStep::SetAppendOnly { enabled: true, .. }
        )));
        last_repair = Some(repair);

        graphql_orm::sqlx::query(&format!("DROP TRIGGER {trigger_name}"))
            .execute(database.pool())
            .await?;
        let event = trigger_name
            .strip_prefix("graphql_orm_append_only_audit_events_")
            .expect("known generated trigger suffix");
        graphql_orm::sqlx::query(&format!(
            "CREATE TRIGGER {trigger_name} BEFORE {} ON audit_events
             BEGIN SELECT RAISE(ABORT, 'append-only entity'); END",
            event.to_ascii_uppercase()
        ))
        .execute(database.pool())
        .await?;
    }
    database
        .schema()
        .apply_migration(
            &last_repair.expect("at least one trigger tamper case"),
            ApplyOptions::default(),
        )
        .await
        .expect_err("recorded version with append-only drift must fail closed");

    graphql_orm::sqlx::query("DROP TABLE audit_events_other")
        .execute(database.pool())
        .await?;
    graphql_orm::sqlx::query("DROP TRIGGER graphql_orm_append_only_audit_events_update")
        .execute(database.pool())
        .await?;
    graphql_orm::sqlx::query(
        "CREATE TRIGGER graphql_orm_append_only_audit_events_update
         BEFORE UPDATE ON audit_events BEGIN SELECT 1; END",
    )
    .execute(database.pool())
    .await?;
    let repair_live = introspect_sqlite_schema(&database).await?;
    let repair = database.schema().plan_migration(
        "append-only-structural-repair",
        "replace same-name tampered trigger",
        &repair_live,
        &target,
    )?;
    database
        .schema()
        .apply_migration(&repair, ApplyOptions::default())
        .await?;
    let repaired = introspect_sqlite_schema(&database).await?;
    assert!(
        repaired
            .tables
            .iter()
            .find(|table| table.table_name == "audit_events")
            .expect("repaired table introspected")
            .append_only
    );
    Ok(())
}
