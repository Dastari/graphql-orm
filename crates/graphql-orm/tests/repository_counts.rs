#![cfg(any(feature = "sqlite", feature = "postgres"))]

use graphql_orm::prelude::*;

#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(backend = "sqlite", table = "count_rows", plural = "CountRows")
)]
#[cfg_attr(
    feature = "postgres",
    repository_entity(backend = "postgres", table = "count_rows", plural = "CountRows")
)]
struct CountRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    id: String,
    #[filterable(type = "string")]
    tenant: String,
}

#[derive(Clone, Copy)]
enum Visibility {
    Unrestricted,
    Complete,
    CallbackOnly,
    Prefilter,
    WrongEntity,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(backend = "sqlite", table = "other_rows", plural = "OtherRows")
)]
#[cfg_attr(
    feature = "postgres",
    repository_entity(backend = "postgres", table = "other_rows", plural = "OtherRows")
)]
struct OtherRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    id: String,
}

impl RowPolicy<Backend> for Visibility {
    fn read_visibility<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<Backend>,
        _entity: &'static str,
        _key: Option<&'static str>,
        surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        assert!(ctx.is_none());
        assert_eq!(surface, EntityAccessSurface::Repository);
        Box::pin(async move {
            Ok(match self {
                Self::Unrestricted => ReadVisibility::Unrestricted,
                Self::CallbackOnly => ReadVisibility::CallbackOnly,
                Self::WrongEntity => {
                    ReadVisibility::Complete(ReadPredicate::from_filter::<Backend, _>(
                        &OtherRowWhereInput {
                            id: Some(StringFilter {
                                eq: Some("1".into()),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    )?)
                }
                Self::Complete | Self::Prefilter => {
                    let predicate =
                        ReadPredicate::from_filter::<Backend, _>(&CountRowWhereInput {
                            tenant: Some(StringFilter {
                                eq: Some("alpha".into()),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })?;
                    if matches!(self, Self::Complete) {
                        ReadVisibility::Complete(predicate)
                    } else {
                        ReadVisibility::Prefilter(predicate)
                    }
                }
            })
        })
    }

    fn can_read_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<Backend>,
        _entity: &'static str,
        _key: Option<&'static str>,
        _surface: EntityAccessSurface,
        _row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { panic!("SQL counts must not decode rows for callbacks") })
    }

    fn can_write_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<Backend>,
        _entity: &'static str,
        _key: Option<&'static str>,
        _surface: EntityAccessSurface,
        _row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

struct DenyEntity;

impl EntityPolicy<Backend> for DenyEntity {
    fn can_access_entity<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<Backend>,
        _entity: &'static str,
        _key: Option<&'static str>,
        _kind: EntityAccessKind,
        _surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
}

struct Fixture {
    db: Database<Backend>,
    #[cfg(feature = "postgres")]
    owned: owned_postgres::OwnedPostgres,
}
impl std::ops::Deref for Fixture {
    type Target = Database<Backend>;
    fn deref(&self) -> &Self::Target {
        &self.db
    }
}
impl std::ops::DerefMut for Fixture {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.db
    }
}
impl Fixture {
    async fn finish(self) {
        self.db.pool().close().await;
        #[cfg(feature = "postgres")]
        {
            let mut owned = self.owned;
            owned.cleanup().unwrap();
        }
    }
}

async fn fixture(visibility: Option<Visibility>) -> Fixture {
    #[cfg(feature = "sqlite")]
    let mut db = Database::<Backend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    #[cfg(feature = "postgres")]
    let owned = owned_postgres::OwnedPostgres::start("repository-counts").unwrap();
    #[cfg(feature = "postgres")]
    let mut db = Database::<Backend>::connect_postgres(&owned.url)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE count_rows(id TEXT PRIMARY KEY, tenant TEXT NOT NULL)")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO count_rows VALUES('1','alpha'),('2','beta'),('3','alpha')")
        .execute(db.pool())
        .await
        .unwrap();
    if let Some(policy) = visibility {
        db.set_row_policy(policy);
    }
    Fixture {
        db,
        #[cfg(feature = "postgres")]
        owned,
    }
}

#[tokio::test]
async fn unrestricted_policy_counts_inside_repository_transaction() {
    let db = fixture(Some(Visibility::Unrestricted)).await;
    let total = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move { Ok(tx.query::<CountRow>().count().await?) })
        })
        .await
        .unwrap();
    assert_eq!(total, 3);
    db.finish().await;
}

#[tokio::test]
async fn complete_visibility_intersects_caller_filter_and_exists() {
    let db = fixture(Some(Visibility::Complete)).await;
    let result = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                let visible = tx.query::<CountRow>().count().await?;
                let hidden = tx
                    .query::<CountRow>()
                    .filter(CountRowWhereInput {
                        tenant: Some(StringFilter {
                            eq: Some("beta".into()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .exists()
                    .await?;
                Ok((visible, hidden))
            })
        })
        .await
        .unwrap();
    assert_eq!(result, (2, false));
    db.finish().await;
}

#[tokio::test]
async fn callback_and_prefilter_counts_remain_rejected() {
    for policy in [Visibility::CallbackOnly, Visibility::Prefilter] {
        let db = fixture(Some(policy)).await;
        assert!(
            db.transaction(TransactionMode::StateMachine, |tx| {
                Box::pin(async move { Ok(tx.query::<CountRow>().count().await?) })
            })
            .await
            .is_err()
        );
        db.finish().await;
    }
}

#[tokio::test]
async fn no_policy_count_keeps_existing_behavior() {
    let db = fixture(None).await;
    let count = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move { Ok(tx.query::<CountRow>().count().await?) })
        })
        .await
        .unwrap();
    assert_eq!(count, 3);
    db.finish().await;
}

#[tokio::test]
async fn wrong_entity_predicate_is_rejected_before_sql() {
    let db = fixture(Some(Visibility::WrongEntity)).await;
    assert!(
        db.transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move { Ok(tx.query::<CountRow>().count().await?) })
        })
        .await
        .is_err()
    );
    db.finish().await;
}

#[tokio::test]
async fn unrestricted_visibility_does_not_override_entity_denial() {
    let mut db = fixture(Some(Visibility::Unrestricted)).await;
    db.set_entity_policy(DenyEntity);
    assert!(
        db.transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move { Ok(tx.query::<CountRow>().count().await?) })
        })
        .await
        .is_err()
    );
    db.finish().await;
}

#[tokio::test]
async fn counts_use_the_current_policy_instead_of_reusing_visibility() {
    let mut db = fixture(Some(Visibility::Unrestricted)).await;
    let before = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move { Ok(tx.query::<CountRow>().count().await?) })
        })
        .await
        .unwrap();
    db.set_row_policy(Visibility::Complete);
    let after = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move { Ok(tx.query::<CountRow>().count().await?) })
        })
        .await
        .unwrap();
    assert_eq!((before, after), (3, 2));
    db.finish().await;
}

#[tokio::test]
async fn count_sees_writes_in_the_same_transaction_and_rollback_keeps_them_private() {
    let db = fixture(Some(Visibility::Complete)).await;
    let result: Result<(), _> = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                tx.insert::<CountRow>(CreateCountRowInput {
                    id: "4".into(),
                    tenant: "alpha".into(),
                })
                .await?;
                assert_eq!(tx.query::<CountRow>().count().await?, 3);
                Err(OrmPublicError::with_message(
                    OrmErrorCode::Conflict,
                    "deliberate rollback",
                ))
            })
        })
        .await;
    assert!(result.is_err());
    let after = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move { Ok(tx.query::<CountRow>().count().await?) })
        })
        .await
        .unwrap();
    assert_eq!(after, 2);
    db.finish().await;
}
