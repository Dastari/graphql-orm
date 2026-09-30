//! Run with `cargo run -p graphql-orm --example runtime_owned_migration -- --apply`.
//! The example uses a disposable in-memory SQLite database; the default is preview-only.
#[cfg(feature = "sqlite")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::db::Database;
    use graphql_orm::graphql::orm::*;
    let definition: RuntimeSchema = serde_json::from_value(serde_json::json!({
        "format_version": 1,
        "collections": [{
            "id": "notes", "api_type_name": "Note", "api_plural_name": "Notes",
            "physical_table": "notes", "primary_key": ["note_id"],
            "append_only": false, "retention_purge": false,
            "fields": [{
                "id": "note_id", "api_name": "id", "physical_column": "id",
                "value_kind": "uuid", "nullable": false, "unique": false,
                "filterable": false, "sortable": true, "generated": true, "default": null
            }, {
                "id": "note_label", "api_name": "label", "physical_column": "label",
                "value_kind": "string", "nullable": false, "unique": false,
                "filterable": true, "sortable": false, "generated": false, "default": null
            }],
            "relations": [], "indexes": [], "composite_unique": [],
            "default_order": [{"field": "note_id", "direction": "asc"}]
        }]
    }))?;
    let schema = std::sync::Arc::new(definition.validate()?);
    let target = schema
        .physical_schema::<SqliteBackend>(RuntimeMigrationLimits::default())?
        .with_static_entities(&[])?; // Add host system metadata here, without replacing host RLS.
    let ownership = ManagedTableSet::new(["notes".to_owned()])?;
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let manager = db.schema();
    let plan = manager
        .plan_owned_migration(
            "notes-v1",
            "initial notes",
            &target,
            &ownership,
            PlanOptions::strict(),
        )
        .await?;
    println!(
        "source={} target={} plan={}",
        plan.source_schema_hash(),
        plan.target_schema_hash(),
        plan.plan_hash()
    );
    for step in plan.steps() {
        println!("{:?}: {}", step.risk(), step.reason());
    }
    if std::env::args().any(|arg| arg == "--apply") {
        let report = manager
            .apply_owned_migration(
                &plan,
                ApplyOptions {
                    additive_only: true,
                    ..Default::default()
                },
            )
            .await?;
        println!("committed {} statements", report.statements_applied);
        let no_op = manager
            .plan_owned_migration(
                "notes-v1",
                "replan",
                &target,
                &ownership,
                PlanOptions::strict(),
            )
            .await?;
        assert!(no_op.steps().is_empty());
        let environment = manager
            .runtime_mutation_environment(schema, &target, &ownership)
            .await?;
        println!("verified baseline={}", environment.physical_baseline_hash());
    }
    Ok(())
}
#[cfg(not(feature = "sqlite"))]
fn main() {
    println!("This disposable example requires --features sqlite.");
}
