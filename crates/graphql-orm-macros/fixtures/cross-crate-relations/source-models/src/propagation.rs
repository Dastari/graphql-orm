use graphql_orm::prelude::*;
use cross_crate_target_models::propagation::*;
#[derive(GraphQLEntity, GraphQLOperations, GraphQLRelations, async_graphql::SimpleObject, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql(complex)]
#[cfg_attr(feature = "sqlite", graphql_entity(backend = "sqlite", table = "up_leaves", plural = "UpLeaves", default_sort = "id ASC"))]
#[cfg_attr(feature = "postgres", graphql_entity(backend = "postgres", table = "up_leaves", plural = "UpLeaves", default_sort = "id ASC"))]
#[cfg_attr(feature = "mssql", graphql_entity(backend = "mssql", table = "up_leaves", plural = "UpLeaves", default_sort = "id ASC"))]
#[graphql_orm(operation_authorization(categories = ["subscription"], all_scopes = ["up.leaf"]))]
pub struct UpLeaf {
    #[primary_key]
    #[sortable]
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

    fn subject(scopes: &[&str]) -> AuthSubject {
        AuthSubject::with_details("fixture-user", vec![], scopes.iter().map(|s| (*s).to_owned()).collect(), None)
    }

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
        let schema = schema_builder(db.clone()).data(subject(&["up.root", "up.middle", "up.leaf"])).finish();
        let mut stream = Box::pin(schema.execute_stream("subscription { upRootChanged { id changeKind path } }"));
        assert!(timeout(Duration::from_millis(30), stream.next()).await.is_err());
        UpLeaf::insert(&db, CreateUpLeafInput { id: "allowed".into(), tenant: "tenant-a".into(), middle_id: "middle".into(), status: "ok".into() }).await.unwrap();
        let event = timeout(Duration::from_secs(2), stream.next()).await.unwrap().unwrap();
        assert!(event.errors.is_empty(), "{:?}", event.errors);
        denied.store(true, Ordering::SeqCst);
        UpLeaf::insert(&db, CreateUpLeafInput { id: "denied".into(), tenant: "tenant-a".into(), middle_id: "middle".into(), status: "ok".into() }).await.unwrap();
        assert!(timeout(Duration::from_millis(100), stream.next()).await.is_err());
    }
    #[derive(Debug)]
    struct Assurance {
        denied: Arc<std::sync::atomic::AtomicUsize>,
        calls: Arc<std::sync::Mutex<Vec<String>>>,
        leaf_calls: Arc<std::sync::atomic::AtomicUsize>,
        revoke_leaf_at: usize,
    }
    impl AssuranceRequirementEvaluator for Assurance {
        fn enforce(&self, _: &async_graphql::Context<'_>, _: AssuranceActorClass, policy: &str) -> async_graphql::Result<()> {
            self.calls.lock().unwrap().push(policy.to_owned());
            let bit = match policy { "up.root" => 1, "up.middle" => 2, "up.leaf" => 4, _ => return Err("unknown policy".into()) };
            if policy == "up.leaf" && self.leaf_calls.fetch_add(1, Ordering::SeqCst) + 1 >= self.revoke_leaf_at {
                return Err("source assurance revoked".into());
            }
            if self.denied.load(Ordering::SeqCst) & bit != 0 { Err("assurance denied".into()) } else { Ok(()) }
        }
    }
    fn assurance(evaluator: Assurance) -> AssuranceEnforcement {
        let mut registry = OperationAssuranceRegistry::builder(graphql_orm_operation_catalog(), AssuranceSchemaConfig::legacy());
        for (field, policy) in [("upRootChanged", "up.root"), ("upMiddleChanged", "up.middle"), ("upLeafChanged", "up.leaf")] {
            registry.require(GraphqlOperationKind::Subscription, field, policy).unwrap();
        }
        AssuranceEnforcement::new(Arc::new(registry.build().unwrap()), Arc::new(evaluator))
    }

    #[tokio::test]
    async fn every_crate_requires_its_own_scope_and_assurance() {
        for missing in [None, Some("up.leaf"), Some("up.middle"), Some("up.root")] {
            let (db, _) = database().await;
            let scopes = ["up.root", "up.middle", "up.leaf"].into_iter().filter(|s| Some(*s) != missing).collect::<Vec<_>>();
            let schema = schema_builder(db.clone()).data(subject(&scopes)).finish();
            let mut stream = Box::pin(schema.execute_stream("subscription { upRootChanged { id path } }"));
            if missing == Some("up.root") {
                let error = timeout(Duration::from_secs(2), stream.next()).await.unwrap().unwrap();
                assert!(!error.errors.is_empty()); continue;
            }
            assert!(timeout(Duration::from_millis(30), stream.next()).await.is_err());
            UpLeaf::insert(&db, CreateUpLeafInput { id: "scoped".into(), tenant: "tenant-a".into(), middle_id: "middle".into(), status: "ok".into() }).await.unwrap();
            if missing.is_some() { assert!(timeout(Duration::from_millis(100), stream.next()).await.is_err()); }
            else { assert!(timeout(Duration::from_secs(2), stream.next()).await.unwrap().unwrap().errors.is_empty()); }
        }
        for denied_bit in [0, 1, 2, 4] {
            let (db, _) = database().await;
            let denied = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
            let enforcement = assurance(Assurance { denied: denied.clone(), calls: calls.clone(), leaf_calls: Default::default(), revoke_leaf_at: usize::MAX });
            let schema = schema_builder(db.clone()).data(subject(&["up.root", "up.middle", "up.leaf"])).data(enforcement).finish();
            let mut stream = Box::pin(schema.execute_stream("subscription { upRootChanged { id path } }"));
            assert!(timeout(Duration::from_millis(30), stream.next()).await.is_err());
            // Materialization/commit succeeds independently of subscriber policy.
            UpLeaf::insert(&db, CreateUpLeafInput { id: "assured".into(), tenant: "tenant-a".into(), middle_id: "middle".into(), status: "ok".into() }).await.unwrap();
            denied.store(denied_bit, Ordering::SeqCst);
            if denied_bit != 0 { assert!(timeout(Duration::from_millis(100), stream.next()).await.is_err()); }
            else {
                assert!(timeout(Duration::from_secs(2), stream.next()).await.unwrap().unwrap().errors.is_empty());
                let log = calls.lock().unwrap();
                for policy in ["up.root", "up.middle", "up.leaf"] {
                    assert!(log.iter().filter(|actual| actual.as_str() == policy).count() >= 2, "operation coordinate {policy} must be checked again at final release");
                }
            }
        }
    }

    #[tokio::test]
    async fn source_assurance_is_rechecked_after_payload_serialization() {
        let (db, _) = database().await;
        let leaf_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let enforcement = assurance(Assurance { denied: Default::default(), calls: Default::default(), leaf_calls: leaf_calls.clone(),
            // The first authorize_current proof checks source authority four
            // times around row/field awaits. The next proof follows prepare().
            revoke_leaf_at: 5 });
        let schema = schema_builder(db.clone()).data(subject(&["up.root", "up.middle", "up.leaf"])).data(enforcement).finish();
        let mut stream = Box::pin(schema.execute_stream("subscription { upRootChanged { id path } }"));
        assert!(timeout(Duration::from_millis(30), stream.next()).await.is_err());
        UpLeaf::insert(&db, CreateUpLeafInput { id: "final-proof".into(), tenant: "tenant-a".into(), middle_id: "middle".into(), status: "ok".into() }).await.unwrap();
        assert!(timeout(Duration::from_millis(100), stream.next()).await.is_err());
        assert!(leaf_calls.load(Ordering::SeqCst) >= 5);
    }

}

#[derive(GraphQLEntity, GraphQLOperations, GraphQLRelations, async_graphql::SimpleObject, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql(complex)]
#[cfg_attr(feature = "sqlite", graphql_entity(backend = "sqlite", table = "required_leaves", plural = "RequiredLeafRows", auth = "required", default_sort = "id ASC"))]
#[cfg_attr(feature = "postgres", graphql_entity(backend = "postgres", table = "required_leaves", plural = "RequiredLeafRows", auth = "required", default_sort = "id ASC"))]
#[cfg_attr(feature = "mssql", graphql_entity(backend = "mssql", table = "required_leaves", plural = "RequiredLeafRows", auth = "required", default_sort = "id ASC"))]
pub struct RequiredLeaf {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    pub id: String,
    pub tenant: String,
    pub parent_id: String,
    #[graphql(skip)]
    #[relation(target = "OptionalMiddle", from = ["parent_id", "tenant"], to = ["id", "tenant"], propagate_change = "up", emit_fk = false)]
    pub parent: Option<OptionalMiddle>,
}

#[derive(GraphQLEntity, GraphQLOperations, GraphQLRelations, async_graphql::SimpleObject, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql(complex)]
#[cfg_attr(feature = "sqlite", graphql_entity(backend = "sqlite", table = "optional_leaves", plural = "OptionalLeafRows", auth = "optional", default_sort = "id ASC"))]
#[cfg_attr(feature = "postgres", graphql_entity(backend = "postgres", table = "optional_leaves", plural = "OptionalLeafRows", auth = "optional", default_sort = "id ASC"))]
#[cfg_attr(feature = "mssql", graphql_entity(backend = "mssql", table = "optional_leaves", plural = "OptionalLeafRows", auth = "optional", default_sort = "id ASC"))]
pub struct OptionalLeaf {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    pub id: String,
    pub tenant: String,
    pub parent_id: String,
    #[graphql(skip)]
    #[relation(target = "RequiredMiddle", from = ["parent_id", "tenant"], to = ["id", "tenant"], propagate_change = "up", emit_fk = false)]
    pub parent: Option<RequiredMiddle>,
}

#[cfg(all(test, feature = "sqlite"))]
mod auth_mode_tests {
    use super::*;
    use graphql_orm::futures::StreamExt;
    use tokio::time::{timeout, Duration};
    schema_roots! { entities: [AuthRoot, RequiredAuthRoot, RequiredMiddle, OptionalMiddle, RequiredLeaf, OptionalLeaf] }
    #[tokio::test]
    async fn source_and_middle_required_auth_do_not_inherit_public_root_mode() {
        for required_source in [true, false] {
            for authenticated in [true, false] {
                let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await.unwrap();
                for sql in [
                    "CREATE TABLE auth_roots (id TEXT PRIMARY KEY, tenant TEXT NOT NULL)",
                    "CREATE TABLE required_auth_roots (id TEXT PRIMARY KEY, tenant TEXT NOT NULL)",
                    "CREATE TABLE required_middles (id TEXT PRIMARY KEY, tenant TEXT NOT NULL, parent_id TEXT NOT NULL)",
                    "CREATE TABLE optional_middles (id TEXT PRIMARY KEY, tenant TEXT NOT NULL, parent_id TEXT NOT NULL)",
                    "CREATE TABLE required_leaves (id TEXT PRIMARY KEY, tenant TEXT NOT NULL, parent_id TEXT NOT NULL)",
                    "CREATE TABLE optional_leaves (id TEXT PRIMARY KEY, tenant TEXT NOT NULL, parent_id TEXT NOT NULL)",
                    "INSERT INTO auth_roots VALUES ('root','tenant')",
                    "INSERT INTO required_middles VALUES ('middle','tenant','root')",
                    "INSERT INTO optional_middles VALUES ('middle','tenant','root')",
                ] { graphql_orm::sqlx::query(sql).execute(db.pool()).await.unwrap(); }
                let schema = schema_builder(db.clone()).data(ResolverAuthConfig::new(ResolverAuthMode::None)).finish();
                let mut request = async_graphql::Request::new("subscription { authRootChanged { id path } }");
                if authenticated { request = request.data(AuthSubject::new("fixture-user")); }
                let mut stream = Box::pin(schema.execute_stream(request));
                assert!(timeout(Duration::from_millis(30), stream.next()).await.is_err(), "public root accepts setup without auth");
                if required_source {
                    RequiredLeaf::insert(&db, CreateRequiredLeafInput { id: "leaf".into(), tenant: "tenant".into(), parent_id: "middle".into() }).await.unwrap();
                } else {
                    OptionalLeaf::insert(&db, CreateOptionalLeafInput { id: "leaf".into(), tenant: "tenant".into(), parent_id: "middle".into() }).await.unwrap();
                }
                if authenticated { assert!(timeout(Duration::from_secs(2), stream.next()).await.unwrap().unwrap().errors.is_empty()); }
                else { assert!(timeout(Duration::from_millis(100), stream.next()).await.is_err()); }
                let mut required_root = Box::pin(schema.execute_stream("subscription { requiredAuthRootChanged { id } }"));
                let rejected = timeout(Duration::from_secs(2), required_root.next()).await.unwrap().unwrap();
                assert!(!rejected.errors.is_empty(), "explicit required root overrides schema None mode");
                let request = async_graphql::Request::new("subscription { requiredAuthRootChanged { id } }").data(AuthSubject::new("fixture-user"));
                let mut allowed_root = Box::pin(schema.execute_stream(request));
                assert!(timeout(Duration::from_millis(30), allowed_root.next()).await.is_err());
                RequiredAuthRoot::insert(&db, CreateRequiredAuthRootInput { id: "root-positive".into(), tenant: "tenant".into() }).await.unwrap();
                assert!(timeout(Duration::from_secs(2), allowed_root.next()).await.unwrap().unwrap().errors.is_empty());
            }
        }
    }
}
