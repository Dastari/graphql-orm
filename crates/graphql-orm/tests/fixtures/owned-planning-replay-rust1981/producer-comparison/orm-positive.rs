use graphql_orm::{db::Database, graphql::orm::*};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    graphql_orm::tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
        let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
        let legacy = SchemaModel { extensions: vec![], tables: vec![TableModel {
            entity_name: "ReviewNote".into(), table_name: "review_notes".into(),
            primary_key: "id".into(), primary_keys: vec!["id".into()], default_sort: "id".into(),
            columns: vec![ColumnModel { name: "id".into(), sql_type: "TEXT".into(), spatial: None,
                nullable: false, is_primary_key: true, is_unique: false,
                default: Some("'\u{3164}'".into()) }],
            indexes: vec![], composite_unique_indexes: vec![], foreign_keys: vec![],
            search_indexes: vec![], append_only: false, retention_purge: false, check_constraints: vec![],
        }] };
        let target = OwnedSchemaModel::from(&legacy);
        let own = ManagedTableSet::new(["review_notes".into()])?;
        let plan = db.schema().plan_owned_migration("unicode-default-v1", "positive compiler qualification", &target, &own, PlanOptions::strict()).await?;
        assert!(!plan.steps().is_empty());
        println!("source={}", plan.source_schema_hash());
        println!("target={}", plan.target_schema_hash());
        println!("plan={}", plan.plan_hash());
        // Synthetic public test data only; no introspected application metadata.
        println!("steps={:?}", plan.steps());
        println!("sql={:?}", plan.statements());
        db.pool().close().await;
        assert!(db.pool().is_closed());
        Ok(())
    })
}
