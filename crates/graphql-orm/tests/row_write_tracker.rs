//! A generated write that fails makes the caller's transaction uncommittable only
//! when it changed rows first. PostgreSQL execution uses only test-owned containers.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use graphql_orm::{async_graphql, prelude::*};

#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    table = "tracked_pairs",
    plural = "TrackedPairs",
    repository_mutations = true
)]
struct TrackedPair {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    tenant_key: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    item_key: String,
    #[filterable(type = "string")]
    owner_user_id: String,
}

const REPOSITORY_ACTOR: &str = "repo";

/// Repository calls act as `repo`, which writes only its own rows.
#[derive(Clone)]
struct OwnerPolicy;

impl graphql_orm::graphql::orm::RowPolicy for OwnerPolicy {
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }

    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(row
                .downcast_ref::<TrackedPair>()
                .is_some_and(|pair| pair.owner_user_id == REPOSITORY_ACTOR))
        })
    }
}

#[derive(Clone)]
struct AllowEntities;

impl EntityPolicy for AllowEntities {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

/// Fails the After hook of a pair whose item key is `after-refuses`.
struct RefusingHook;

impl graphql_orm::graphql::orm::MutationHook for RefusingHook {
    fn on_mutation<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a mut MutationContext<'_>,
        event: &'a graphql_orm::graphql::orm::MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            let refused = event.phase == graphql_orm::graphql::orm::MutationPhase::After
                && event
                    .after::<TrackedPair>()
                    .ok()
                    .flatten()
                    .is_some_and(|pair| pair.item_key == "after-refuses");
            if refused {
                return Err(async_graphql::Error::new("hook refused"));
            }
            Ok(())
        })
    }
}

#[cfg(feature = "sqlite")]
async fn pool() -> Result<graphql_orm::sqlx::SqlitePool, Box<dyn std::error::Error>> {
    Ok(graphql_orm::sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?)
}

#[cfg(feature = "postgres")]
async fn pool() -> Result<graphql_orm::sqlx::PgPool, Box<dyn std::error::Error>> {
    owned_postgres::OwnedPostgres::pool("row-write-tracker", 5).await
}

async fn database() -> Result<Database, Box<dyn std::error::Error>> {
    let pool = pool().await?;
    graphql_orm::sqlx::query(
        "CREATE TABLE tracked_pairs (tenant_key TEXT NOT NULL, item_key TEXT NOT NULL, \
         owner_user_id TEXT NOT NULL, PRIMARY KEY (tenant_key, item_key))",
    )
    .execute(&pool)
    .await?;
    graphql_orm::sqlx::query(
        "INSERT INTO tracked_pairs (tenant_key, item_key, owner_user_id) \
         VALUES ('t', 'theirs', 'other')",
    )
    .execute(&pool)
    .await?;
    let mut db = Database::new(pool);
    db.set_entity_policy(AllowEntities);
    db.set_row_policy(OwnerPolicy);
    Ok(db)
}

fn pair(item: &str, owner: &str) -> CreateTrackedPairInput {
    CreateTrackedPairInput {
        tenant_key: "t".to_string(),
        item_key: item.to_string(),
        owner_user_id: owner.to_string(),
    }
}

/// Every stored `(item_key, owner_user_id)`, in item order.
async fn rows(db: &Database) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    Ok(graphql_orm::sqlx::query_as::<_, (String, String)>(
        "SELECT item_key, owner_user_id FROM tracked_pairs ORDER BY item_key",
    )
    .fetch_all(db.pool())
    .await?)
}

fn row(item: &str, owner: &str) -> (String, String) {
    (item.to_string(), owner.to_string())
}

/// The public code of a refused repository call.
fn refusal<T: std::fmt::Debug>(result: graphql_orm::Result<T>) -> OrmErrorCode {
    OrmPublicError::from(result.expect_err("write must be refused")).code
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "owns a disposable Docker PostgreSQL container"
)]
async fn an_insert_if_absent_conflict_on_an_unwritable_row_leaves_the_transaction_usable()
-> Result<(), Box<dyn std::error::Error>> {
    let db = database().await?;
    db.transaction(TransactionMode::StateMachine, |tx| {
        Box::pin(async move {
            // A real write first, so a stale affected-row count would show.
            tx.insert::<TrackedPair>(pair("first", REPOSITORY_ACTOR))
                .await
                .map_err(OrmPublicError::from)?;
            // The conflicting insert changes no row; the existing row is refused.
            let refused = tx
                .insert_if_absent::<TrackedPair>(pair("theirs", REPOSITORY_ACTOR))
                .await;
            assert_eq!(refusal(refused), OrmErrorCode::Forbidden);
            tx.insert::<TrackedPair>(pair("later", REPOSITORY_ACTOR))
                .await
                .map_err(OrmPublicError::from)?;
            Ok(())
        })
    })
    .await?;
    assert_eq!(
        rows(&db).await?,
        vec![
            row("first", REPOSITORY_ACTOR),
            row("later", REPOSITORY_ACTOR),
            row("theirs", "other"),
        ]
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "owns a disposable Docker PostgreSQL container"
)]
async fn a_failure_after_an_insert_if_absent_write_makes_the_transaction_uncommittable()
-> Result<(), Box<dyn std::error::Error>> {
    let mut db = database().await?;
    db.set_mutation_hook(RefusingHook);

    // The insert changes a row, then its After hook fails; the caller ignores it.
    let outcome = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                let ignored = tx
                    .insert_if_absent::<TrackedPair>(pair("after-refuses", REPOSITORY_ACTOR))
                    .await;
                assert!(ignored.is_err());
                Ok(())
            })
        })
        .await;
    assert!(
        matches!(&outcome, Err(TransactionError::Rejected(_))),
        "{outcome:?}"
    );

    // The insert writes a row the caller may not write; the caller ignores it.
    let outcome = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                let ignored = tx
                    .insert_if_absent::<TrackedPair>(pair("fresh", "other"))
                    .await;
                assert_eq!(refusal(ignored), OrmErrorCode::Forbidden);
                Ok(())
            })
        })
        .await;
    assert!(
        matches!(
            &outcome,
            Err(TransactionError::Rejected(error)) if error.code == OrmErrorCode::Forbidden
        ),
        "{outcome:?}"
    );

    assert_eq!(rows(&db).await?, vec![row("theirs", "other")]);
    Ok(())
}
