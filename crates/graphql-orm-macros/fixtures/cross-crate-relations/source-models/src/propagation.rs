use graphql_orm::prelude::*;
use cross_crate_target_models::propagation::*;
#[derive(GraphQLEntity, GraphQLOperations, GraphQLRelations, async_graphql::SimpleObject, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql(complex)]
#[cfg_attr(feature = "sqlite", graphql_entity(backend = "sqlite", table = "up_leaves", plural = "UpLeaves", default_sort = "id ASC"))]
#[cfg_attr(feature = "postgres", graphql_entity(backend = "postgres", table = "up_leaves", plural = "UpLeaves", default_sort = "id ASC"))]
#[cfg_attr(feature = "mssql", graphql_entity(backend = "mssql", table = "up_leaves", plural = "UpLeaves", default_sort = "id ASC"))]
pub struct UpLeaf {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub id: String,
    pub tenant: String,
    pub middle_id: String,
    pub status: String,
    #[graphql(skip)]
    #[relation(target = "UpMiddle", from = ["middle_id", "tenant"], to = ["id", "tenant"], propagate_change = "up", emit_fk = false)]
    pub middle: Option<UpMiddle>,
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::*;
    use graphql_orm::db::Database;
    use graphql_orm::graphql::subscriptions::SubscriptionEvent;
    use graphql_orm::graphql::orm::ChangeKind;
    use graphql_orm::futures::StreamExt;
    use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
    use tokio::time::{timeout, Duration};

    schema_roots! { entities: [UpRoot, UpMiddle, UpLeaf] }

    #[derive(Clone)]
    struct Policy(Arc<AtomicBool>);
    impl EntityPolicy<SqliteBackend> for Policy {
        fn can_access_entity<'a>(&'a self, _: Option<&'a async_graphql::Context<'_>>,
            _: &'a Database<SqliteBackend>, entity: &'static str, _: Option<&'static str>,
            kind: EntityAccessKind, _: EntityAccessSurface)
            -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async move { Ok(!(entity == "UpLeaf" && kind == EntityAccessKind::Read && self.0.load(Ordering::SeqCst))) })
        }
    }

    async fn database() -> (Database<SqliteBackend>, Arc<AtomicBool>) {
        let pool = graphql_orm::sqlx::sqlite::SqlitePoolOptions::new().max_connections(1)
            .connect("sqlite::memory:").await.unwrap();
        for sql in ["PRAGMA foreign_keys = ON",
            "CREATE TABLE up_roots (id TEXT PRIMARY KEY, tenant TEXT NOT NULL)",
            "CREATE TABLE up_middles (id TEXT PRIMARY KEY, tenant TEXT NOT NULL, root_id TEXT NOT NULL)",
            "CREATE TABLE statuses (status TEXT PRIMARY KEY)",
            "INSERT INTO statuses VALUES ('ok')",
            "CREATE TABLE up_leaves (id TEXT PRIMARY KEY, tenant TEXT NOT NULL, middle_id TEXT NOT NULL, status TEXT NOT NULL REFERENCES statuses(status) DEFERRABLE INITIALLY DEFERRED)",
            "INSERT INTO up_roots VALUES ('root','tenant-a')",
            "INSERT INTO up_middles VALUES ('middle','tenant-a','root')"] {
            graphql_orm::sqlx::query(sql).execute(&pool).await.unwrap();
        }
        let denied = Arc::new(AtomicBool::new(false));
        (Database::with_entity_policy(pool, Policy(denied.clone())), denied)
    }

    #[tokio::test]
    async fn three_crates_commit_and_complete_tuple_rollback() {
        let (db, _) = database().await;
        let mut roots = db.ensure_event_sender::<SubscriptionEvent<UpRootChangedEvent>>().subscribe();
        let mut middles = db.ensure_event_sender::<SubscriptionEvent<UpMiddleChangedEvent>>().subscribe();
        UpLeaf::insert(&db, CreateUpLeafInput { id: "leaf".into(), tenant: "tenant-a".into(), middle_id: "middle".into(), status: "ok".into() }).await.unwrap();
        let middle = timeout(Duration::from_secs(2), middles.recv()).await.unwrap().unwrap();
        let root = timeout(Duration::from_secs(2), roots.recv()).await.unwrap().unwrap();
        assert_eq!(middle.event.change_kind, ChangeKind::Propagated);
        assert_eq!(root.event.change_kind, ChangeKind::Propagated);
        assert_eq!(root.event.path.len(), 2);
        assert_eq!(root.event.source_entity.as_deref(), Some("UpLeaf"));
        let count: i64 = graphql_orm::sqlx::query_scalar("SELECT COUNT(*) FROM up_leaves WHERE id='leaf'").fetch_one(db.pool()).await.unwrap();
        assert_eq!(count, 1, "events observed after committed data");
        // Same target id, mismatched tenant: complete tuple prevents either hop.
        UpLeaf::insert(&db, CreateUpLeafInput { id: "other".into(), tenant: "tenant-b".into(), middle_id: "middle".into(), status: "ok".into() }).await.unwrap();
        assert!(roots.try_recv().is_err());
        assert!(middles.try_recv().is_err());
        // Failure at COMMIT occurs after generated propagation has queued both hops.
        assert!(UpLeaf::insert(&db, CreateUpLeafInput { id: "rollback".into(), tenant: "tenant-a".into(), middle_id: "middle".into(), status: "missing".into() }).await.is_err());
        assert!(roots.try_recv().is_err());
        assert!(middles.try_recv().is_err());
        let count: i64 = graphql_orm::sqlx::query_scalar("SELECT COUNT(*) FROM up_leaves WHERE id='rollback'").fetch_one(db.pool()).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn source_revocation_suppresses_cross_crate_graphql_release() {
        let (db, denied) = database().await;
        let schema = schema_builder(db.clone()).data("fixture-user".to_owned()).finish();
        let mut stream = Box::pin(schema.execute_stream("subscription { upRootChanged { id changeKind path } }"));
        assert!(timeout(Duration::from_millis(30), stream.next()).await.is_err());
        UpLeaf::insert(&db, CreateUpLeafInput { id: "allowed".into(), tenant: "tenant-a".into(), middle_id: "middle".into(), status: "ok".into() }).await.unwrap();
        let event = timeout(Duration::from_secs(2), stream.next()).await.unwrap().unwrap();
        assert!(event.errors.is_empty(), "{:?}", event.errors);
        denied.store(true, Ordering::SeqCst);
        UpLeaf::insert(&db, CreateUpLeafInput { id: "denied".into(), tenant: "tenant-a".into(), middle_id: "middle".into(), status: "ok".into() }).await.unwrap();
        assert!(timeout(Duration::from_millis(100), stream.next()).await.is_err());
    }
}
