#![cfg(all(any(feature = "sqlite", feature = "postgres"), not(feature = "mssql")))]

use graphql_orm::prelude::*;
#[path = "support/host_primary_key_contract.rs"]
mod host_primary_key_contract;
#[path = "support/host_primary_keys.rs"]
mod host_primary_keys;
#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "owns a disposable PostgreSQL container"
)]
async fn supplied_keys_use_typed_bindings_across_repository_insert_paths()
-> Result<(), Box<dyn std::error::Error>> {
    use host_primary_key_contract::Backend;
    use host_primary_keys::*;
    for variable in [
        "DATABASE_URL",
        "TEST_DATABASE_URL",
        "MSSQL_TEST_DATABASE_URL",
    ] {
        assert!(
            std::env::var_os(variable).is_none(),
            "refusing ambient database configuration"
        );
    }
    #[cfg(feature = "sqlite")]
    let mut database = Database::<Backend>::connect_sqlite("sqlite::memory:").await?;
    #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
    let mut owned = owned_postgres::OwnedPostgres::start("host-primary-keys")?;
    #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
    let mut database = Database::<Backend>::connect_postgres(&owned.url).await?;
    database.set_entity_policy(HostKeyFixturePolicy);
    #[cfg(feature = "sqlite")]
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "host-keys",
            "test-owned host primary keys",
            &[
                HostI16::metadata(),
                HostI32::metadata(),
                HostI64::metadata(),
                HostString::metadata(),
                HostUuid::metadata(),
                HostBytes::metadata(),
                HostComposite::metadata(),
            ],
        )
        .await?;
    #[cfg(feature = "sqlite")]
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
    {
        // Narrow Rust integers require their matching externally owned PostgreSQL types.
        // Do not change the static migration engine's existing BIGINT integer default.
        for sql in [
            "CREATE TABLE host_i16 (id SMALLINT PRIMARY KEY, label TEXT NOT NULL)",
            "CREATE TABLE host_i32 (id INT PRIMARY KEY, label TEXT NOT NULL)",
            "CREATE TABLE host_i64 (id BIGINT PRIMARY KEY, label TEXT NOT NULL)",
            "CREATE TABLE host_string (id TEXT PRIMARY KEY, label TEXT NOT NULL)",
            "CREATE TABLE host_uuid (id UUID PRIMARY KEY, label TEXT NOT NULL)",
            "CREATE TABLE host_bytes (id BYTEA PRIMARY KEY, label TEXT NOT NULL)",
            "CREATE TABLE host_composite (namespace TEXT NOT NULL, id INT NOT NULL, label TEXT NOT NULL, PRIMARY KEY(namespace,id))",
        ] {
            graphql_orm::sqlx::query(sql)
                .execute(database.pool())
                .await?;
        }
    }
    host_primary_key_contract::exercise(&database).await?;
    database.pool().close().await;
    #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
    owned.cleanup()?;
    Ok(())
}
