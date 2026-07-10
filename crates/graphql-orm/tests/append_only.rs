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

    graphql_orm::sqlx::query("DROP TRIGGER graphql_orm_append_only_audit_events_delete")
        .execute(database.pool())
        .await?;
    let tampered = introspect_sqlite_schema(&database).await?;
    assert!(
        !tampered
            .tables
            .iter()
            .find(|table| table.table_name == "audit_events")
            .expect("tampered table introspected")
            .append_only
    );
    let repair = database.schema().plan_migration(
        "append-only-repair",
        "repair append-only enforcement",
        &tampered,
        &target,
    )?;
    assert!(repair.steps.iter().any(|step| matches!(
        step.step,
        MigrationStep::SetAppendOnly { enabled: true, .. }
    )));
    Ok(())
}
