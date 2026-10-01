//! Disposable, SQL-free host example: cached snapshots grant no target access.
#[cfg(feature = "sqlite")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use async_graphql::{
        Context, EmptyMutation, EmptySubscription, Object, Schema, dataloader::DataLoader,
    };
    use cross_crate_source_models::Session;
    use cross_crate_target_models::{CreateEndpointInput, Endpoint};
    use graphql_orm::{db::Database, graphql::loaders::RelationLoader, prelude::*};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct Policy(Arc<AtomicBool>);
    impl EntityPolicy<SqliteBackend> for Policy {
        fn can_access_entity<'a>(
            &'a self,
            _: Option<&'a Context<'_>>,
            _: &'a Database<SqliteBackend>,
            entity: &'static str,
            _: Option<&'static str>,
            kind: EntityAccessKind,
            _: EntityAccessSurface,
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async move {
                Ok(entity != "Endpoint"
                    || kind != EntityAccessKind::Read
                    || self.0.load(Ordering::SeqCst))
            })
        }
    }
    struct Query;
    #[Object]
    impl Query {
        async fn session(&self) -> Session {
            Session {
                id: "session-1".into(),
                endpoint_id: "endpoint-1".into(),
                tenant_id: "tenant-1".into(),
                endpoint: Some(Endpoint {
                    id: "forged".into(),
                    tenant_id: "tenant-2".into(),
                    name: "private snapshot".into(),
                }),
            }
        }
    }
    let allowed = Arc::new(AtomicBool::new(true));
    let pool = graphql_orm::sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    let db = Database::<SqliteBackend>::with_entity_policy(pool, Policy(allowed.clone()));
    let plan = db
        .schema()
        .plan_migration_to_entities("example", "owned fixture", &[Endpoint::metadata()])
        .await?;
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    Endpoint::insert(
        &db,
        CreateEndpointInput {
            id: "endpoint-1".into(),
            tenant_id: "tenant-1".into(),
            name: "Current authorized target".into(),
        },
    )
    .await?;
    let schema = Schema::build(Query, EmptyMutation, EmptySubscription)
        .data(db.clone())
        .data("example-user".to_owned())
        .data(DataLoader::new(
            RelationLoader::<Endpoint, SqliteBackend>::new(db),
            tokio::spawn,
        ))
        .finish();
    let query =
        "{ session { id link: endpoint { ...Target } } } fragment Target on Endpoint { id name }";
    let current = schema.execute(query).await;
    assert!(current.errors.is_empty());
    assert_eq!(
        current.data.into_json()?["session"]["link"]["name"],
        "Current authorized target"
    );
    allowed.store(false, Ordering::SeqCst);
    let denied = schema.execute(query).await;
    let data = denied.data.into_json()?;
    assert_eq!(data["session"]["id"], "session-1");
    assert!(data["session"]["link"].is_null());
    assert_eq!(denied.errors[0].message, "forbidden");
    println!("current target resolved; denied nullable target is null; parent preserved");
    Ok(())
}
#[cfg(not(feature = "sqlite"))]
fn main() {
    println!("Run with the sqlite feature for disposable execution.");
}
