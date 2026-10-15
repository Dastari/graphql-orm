#![cfg(feature = "postgres")]

#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use std::process::Command;

#[test]
fn ambient_database_configuration_is_rejected_before_container_start() {
    if std::env::var_os("GRAPHQL_ORM_TEST_AMBIENT_CHILD").is_some() {
        let error = owned_postgres::OwnedPostgres::start("ambient-must-not-start")
            .err()
            .expect("ambient URL must be rejected");
        assert!(
            error
                .to_string()
                .contains("refusing ambient TEST_DATABASE_URL")
        );
        return;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "ambient_database_configuration_is_rejected_before_container_start",
        ])
        .env_remove("DATABASE_URL")
        .env_remove("MSSQL_TEST_DATABASE_URL")
        .env("TEST_DATABASE_URL", "synthetic-endpoint-must-never-be-used")
        .env("GRAPHQL_ORM_TEST_AMBIENT_CHILD", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn owned_container_survives_pool_clones_and_is_removed_after_last_drop()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = owned_postgres::OwnedPostgres::pool("lifecycle", 2).await?;
    let database: String = graphql_orm::sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await?;
    let token = database.strip_prefix("graphql_orm_").unwrap();
    let output = Command::new("docker")
        .args([
            "ps",
            "--no-trunc",
            "--filter",
            &format!("label=graphql-orm.test-owner={token}"),
            "--format",
            "{{.ID}}",
        ])
        .output()?;
    assert!(output.status.success());
    let container = String::from_utf8(output.stdout)?.trim().to_owned();
    assert_eq!(container.len(), 64, "one exact owned identity is required");
    let clone = pool.clone();
    drop(pool);
    let value: i32 = graphql_orm::sqlx::query_scalar("SELECT 42")
        .fetch_one(&clone)
        .await?;
    assert_eq!(value, 42);
    clone.close().await;
    drop(clone);
    for _ in 0..40 {
        if !Command::new("docker")
            .args(["inspect", &container])
            .output()?
            .status
            .success()
        {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("owned container remained after last pool was dropped");
}
