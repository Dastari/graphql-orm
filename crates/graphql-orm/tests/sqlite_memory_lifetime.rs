#![cfg(feature = "sqlite")]

use graphql_orm::{db::Database, graphql::orm::SqliteBackend};

#[tokio::test]
async fn replacement_preserves_memory_schema_and_rows() {
    for url in [
        "sqlite::memory:",
        "sqlite://:memory:",
        "sqlite://?mode=memory",
        "sqlite://lifetime-proof?mode=memory&cache=private",
        "sqlite://file:lifetime%3Fcache%3Dprivate?mode=memory",
    ] {
        let db = Database::<SqliteBackend>::connect_sqlite(url)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE lifetime_proof (value INTEGER NOT NULL)")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO lifetime_proof VALUES (42)")
            .execute(db.pool())
            .await
            .unwrap();
        // Force the same path as a failed release ping without waiting for a reaper.
        db.pool().acquire().await.unwrap().close().await.unwrap();
        let value: i64 = sqlx::query_scalar("SELECT value FROM lifetime_proof")
            .fetch_one(db.pool())
            .await
            .unwrap_or_else(|error| panic!("replacement for {url}: {error}"));
        assert_eq!(value, 42);
        let options = db.pool().options();
        assert_eq!(options.get_idle_timeout(), None);
        assert_eq!(options.get_max_lifetime(), None);
        assert_eq!(options.get_min_connections(), 1);
    }
}

#[tokio::test]
async fn explicit_memory_pool_size_shares_rows_and_anonymous_pools_are_isolated() {
    use graphql_orm::db::ConnectionOptions;
    let db = Database::<SqliteBackend>::connect_sqlite_with_options(
        "sqlite::memory:",
        ConnectionOptions::new().max_connections(2),
    )
    .await
    .unwrap();
    let mut first = db.pool().acquire().await.unwrap();
    sqlx::query("CREATE TABLE shared_proof (value INTEGER)")
        .execute(&mut *first)
        .await
        .unwrap();
    sqlx::query("INSERT INTO shared_proof VALUES (7)")
        .execute(&mut *first)
        .await
        .unwrap();
    let mut second = db.pool().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT value FROM shared_proof")
            .fetch_one(&mut *second)
            .await
            .unwrap(),
        7
    );
    let isolated = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM sqlite_master WHERE name = 'shared_proof'"
        )
        .fetch_one(isolated.pool())
        .await
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn private_named_memory_pools_do_not_share_with_each_other() {
    let url = "sqlite://private-pool-proof?mode=memory&cache=private";
    let first = Database::<SqliteBackend>::connect_sqlite(url)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE private_proof (value INTEGER)")
        .execute(first.pool())
        .await
        .unwrap();
    let second = Database::<SqliteBackend>::connect_sqlite(url)
        .await
        .unwrap();
    let tables: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE name = 'private_proof'")
            .fetch_one(second.pool())
            .await
            .unwrap();
    assert_eq!(
        tables, 0,
        "cache=private must retain isolation between pools"
    );
}

#[tokio::test]
async fn named_memory_without_cache_option_is_isolated_between_pools() {
    let url = "sqlite:file:implicit-pool-proof?mode=memory";
    let first = Database::<SqliteBackend>::connect_sqlite(url)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE implicit_proof (value INTEGER)")
        .execute(first.pool())
        .await
        .unwrap();
    let second = Database::<SqliteBackend>::connect_sqlite(url)
        .await
        .unwrap();
    let tables: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE name = 'implicit_proof'")
            .fetch_one(second.pool())
            .await
            .unwrap();
    assert_eq!(
        tables, 0,
        "sharing requires an explicit cache=shared opt-in"
    );
}

#[tokio::test]
async fn explicit_shared_memory_preserves_sharing_between_pools() {
    for url in [
        "sqlite:file:shared-pool-proof?mode=memory&cache=shared",
        "sqlite:file:encoded-shared-pool-proof?mode=memory&ca%63he=sha%72ed",
    ] {
        let first = Database::<SqliteBackend>::connect_sqlite(url)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE shared_pool_proof (value INTEGER)")
            .execute(first.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO shared_pool_proof VALUES (42)")
            .execute(first.pool())
            .await
            .unwrap();
        let second = Database::<SqliteBackend>::connect_sqlite(url)
            .await
            .unwrap();
        let value: i64 = sqlx::query_scalar("SELECT value FROM shared_pool_proof")
            .fetch_one(second.pool())
            .await
            .unwrap();
        assert_eq!(value, 42);
    }
}
