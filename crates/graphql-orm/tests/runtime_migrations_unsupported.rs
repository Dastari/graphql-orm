#![cfg(feature = "mssql")]
use graphql_orm::db::{Database, mssql::MssqlPool};
use graphql_orm::graphql::orm::*;

fn rejected(error: RuntimeMigrationError) {
    assert!(
        matches!(error,RuntimeMigrationError::Diagnostics(ref diagnostics)
        if diagnostics.0[0].code==RuntimeMigrationDiagnosticCode::UnsupportedBackend)
    );
}
#[tokio::test]
async fn mssql_owned_migrations_reject_before_pool_acquisition() {
    // Construction is lazy; no database or network endpoint belongs to this test.
    // A migration capability must reject without asking the pool for a connection.
    let pool = MssqlPool::new(graphql_orm::tiberius::Config::new());
    let db = Database::<MssqlBackend>::new(pool);
    let target = OwnedSchemaModel::default();
    let ownership = ManagedTableSet::new(std::iter::empty()).unwrap();
    rejected(
        db.schema()
            .validate_owned_schema(&target, &ownership)
            .await
            .unwrap_err(),
    );
    rejected(
        db.schema()
            .plan_owned_migration(
                "unsupported",
                "no connection",
                &target,
                &ownership,
                PlanOptions::strict(),
            )
            .await
            .unwrap_err(),
    );
    let schema = RuntimeSchema {
        format_version: 1,
        collections: vec![],
    }
    .validate()
    .unwrap();
    let error = schema
        .physical_schema::<MssqlBackend>(Default::default())
        .unwrap_err();
    assert_eq!(
        error.0[0].code,
        RuntimeMigrationDiagnosticCode::UnsupportedBackend
    );
    let error = schema
        .physical_schema::<NoDefaultBackend>(Default::default())
        .unwrap_err();
    assert_eq!(
        error.0[0].code,
        RuntimeMigrationDiagnosticCode::UnsupportedBackend
    );
}
