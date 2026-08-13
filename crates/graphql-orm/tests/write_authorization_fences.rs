#![cfg(feature = "sqlite")]

use graphql_orm::prelude::*;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::Notify;

struct TestDatabaseFile(PathBuf);

impl Drop for TestDatabaseFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("sqlite-shm"));
        let _ = std::fs::remove_file(self.0.with_extension("sqlite-wal"));
    }
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(table = "write_fence_rows", plural = "WriteFenceRows")]
struct WriteFenceRow {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    cohort: String,
    owner_id: String,
    value: String,
}

#[derive(Clone)]
struct CoordinatedRowPolicy {
    entered: Arc<Notify>,
    concurrent_finished: Arc<Notify>,
    coordinate_once: Arc<AtomicBool>,
    escaped_before_decision: Arc<AtomicBool>,
}

impl graphql_orm::graphql::orm::RowPolicy for CoordinatedRowPolicy {
    fn can_read_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a graphql_orm::db::Database,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: EntityAccessSurface,
        _row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }

    fn can_write_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a graphql_orm::db::Database,
        entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            let Some(row) = row.downcast_ref::<WriteFenceRow>() else {
                return Ok(entity_name != "WriteFenceRow");
            };
            if !self.coordinate_once.swap(true, Ordering::SeqCst) {
                self.entered.notify_one();
                if tokio::time::timeout(
                    Duration::from_millis(200),
                    self.concurrent_finished.notified(),
                )
                .await
                .is_ok()
                {
                    self.escaped_before_decision.store(true, Ordering::SeqCst);
                }
            }
            Ok(row.owner_id == "allowed")
        })
    }
}

async fn setup() -> Result<
    (
        TestDatabaseFile,
        sqlx::SqlitePool,
        graphql_orm::db::Database,
        CoordinatedRowPolicy,
    ),
    Box<dyn std::error::Error>,
> {
    let database_path = std::env::temp_dir().join(format!(
        "graphql-orm-authorization-fence-{}.sqlite",
        graphql_orm::uuid::Uuid::new_v4()
    ));
    let database_file = TestDatabaseFile(database_path.clone());
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", database_path.display()))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(3)
        .connect_with(options)
        .await?;
    sqlx::query(
        "CREATE TABLE write_fence_rows (
            id TEXT PRIMARY KEY,
            cohort TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            value TEXT NOT NULL
        )",
    )
    .execute(&pool)
    .await?;

    let policy = CoordinatedRowPolicy {
        entered: Arc::new(Notify::new()),
        concurrent_finished: Arc::new(Notify::new()),
        coordinate_once: Arc::new(AtomicBool::new(false)),
        escaped_before_decision: Arc::new(AtomicBool::new(false)),
    };
    let mut database = graphql_orm::db::Database::new(pool.clone());
    database.set_row_policy(policy.clone());
    Ok((database_file, pool, database, policy))
}

async fn insert_raw(
    pool: &sqlx::SqlitePool,
    id: graphql_orm::uuid::Uuid,
    cohort: &str,
    owner_id: &str,
    value: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO write_fence_rows (id, cohort, owner_id, value) VALUES (?, ?, ?, ?)")
        .bind(id.to_string())
        .bind(cohort)
        .bind(owner_id)
        .bind(value)
        .execute(pool)
        .await?;
    Ok(())
}

async fn load_raw(
    pool: &sqlx::SqlitePool,
    id: graphql_orm::uuid::Uuid,
) -> Result<(String, String), sqlx::Error> {
    use sqlx::Row;
    let row = sqlx::query("SELECT owner_id, value FROM write_fence_rows WHERE id = ?")
        .bind(id.to_string())
        .fetch_one(pool)
        .await?;
    Ok((row.try_get("owner_id")?, row.try_get("value")?))
}

#[tokio::test]
async fn single_row_policy_preimage_and_update_share_the_write_transaction()
-> Result<(), Box<dyn std::error::Error>> {
    let (_directory, pool, database, policy) = setup().await?;
    let id = graphql_orm::uuid::Uuid::new_v4();
    insert_raw(&pool, id, "single", "allowed", "before").await?;

    let concurrent_pool = pool.clone();
    let entered = policy.entered.clone();
    let finished = policy.concurrent_finished.clone();
    let concurrent = tokio::spawn(async move {
        entered.notified().await;
        let result = sqlx::query("UPDATE write_fence_rows SET owner_id = ? WHERE id = ?")
            .bind("denied")
            .bind(id.to_string())
            .execute(&concurrent_pool)
            .await;
        finished.notify_one();
        result
    });

    let updated = WriteFenceRow::update_by_id(
        &database,
        &id,
        UpdateWriteFenceRowInput {
            value: Some("authorized-update".to_string()),
            ..Default::default()
        },
    )
    .await?
    .expect("authorized row should update");
    assert_eq!(updated.value, "authorized-update");
    concurrent.await??;
    assert!(
        !policy.escaped_before_decision.load(Ordering::SeqCst),
        "a concurrent owner change completed between row-policy evaluation and DML"
    );

    let stored = load_raw(&pool, id).await?;
    assert_eq!(stored.0, "denied");
    assert_eq!(stored.1, "authorized-update");
    Ok(())
}

#[tokio::test]
async fn bulk_update_materializes_authorized_keys_and_excludes_new_matches()
-> Result<(), Box<dyn std::error::Error>> {
    let (_directory, pool, database, policy) = setup().await?;
    let selected_id = graphql_orm::uuid::Uuid::new_v4();
    let phantom_id = graphql_orm::uuid::Uuid::new_v4();
    insert_raw(&pool, selected_id, "target", "allowed", "selected").await?;
    let mut events = database
        .ensure_event_sender::<WriteFenceRowChangedEvent>()
        .subscribe();

    let concurrent_pool = pool.clone();
    let entered = policy.entered.clone();
    let finished = policy.concurrent_finished.clone();
    let concurrent = tokio::spawn(async move {
        entered.notified().await;
        let result = insert_raw(&concurrent_pool, phantom_id, "target", "denied", "phantom").await;
        finished.notify_one();
        result
    });

    let affected = WriteFenceRow::update_where(
        &database,
        WriteFenceRowWhereInput {
            cohort: Some(StringFilter {
                eq: Some("target".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        },
        UpdateWriteFenceRowInput {
            value: Some("authorized-update".to_string()),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(affected, 1);
    concurrent.await??;
    assert!(
        !policy.escaped_before_decision.load(Ordering::SeqCst),
        "a new matching row committed before the authorized key-set write"
    );

    let selected = load_raw(&pool, selected_id).await?;
    let phantom = load_raw(&pool, phantom_id).await?;
    assert_eq!(selected.1, "authorized-update");
    assert_eq!(phantom.1, "phantom");
    assert_eq!(events.recv().await?.id, selected_id);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), events.recv())
            .await
            .is_err(),
        "no event may be emitted for a row outside the authorized key set"
    );
    Ok(())
}

#[tokio::test]
async fn bulk_delete_materializes_authorized_keys_and_excludes_new_matches()
-> Result<(), Box<dyn std::error::Error>> {
    let (_directory, pool, database, policy) = setup().await?;
    let selected_id = graphql_orm::uuid::Uuid::new_v4();
    let phantom_id = graphql_orm::uuid::Uuid::new_v4();
    insert_raw(&pool, selected_id, "target", "allowed", "selected").await?;
    let mut events = database
        .ensure_event_sender::<WriteFenceRowChangedEvent>()
        .subscribe();

    let concurrent_pool = pool.clone();
    let entered = policy.entered.clone();
    let finished = policy.concurrent_finished.clone();
    let concurrent = tokio::spawn(async move {
        entered.notified().await;
        let result = insert_raw(&concurrent_pool, phantom_id, "target", "denied", "phantom").await;
        finished.notify_one();
        result
    });

    let affected = WriteFenceRow::delete_where(
        &database,
        WriteFenceRowWhereInput {
            cohort: Some(StringFilter {
                eq: Some("target".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(affected, 1);
    concurrent.await??;
    assert!(
        !policy.escaped_before_decision.load(Ordering::SeqCst),
        "a new matching row committed before the authorized key-set delete"
    );

    assert!(
        sqlx::query("SELECT 1 FROM write_fence_rows WHERE id = ?")
            .bind(selected_id.to_string())
            .fetch_optional(&pool)
            .await?
            .is_none()
    );
    let phantom = load_raw(&pool, phantom_id).await?;
    assert_eq!(phantom.1, "phantom");
    assert_eq!(events.recv().await?.id, selected_id);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), events.recv())
            .await
            .is_err(),
        "no event may be emitted for a row outside the authorized key set"
    );
    Ok(())
}
