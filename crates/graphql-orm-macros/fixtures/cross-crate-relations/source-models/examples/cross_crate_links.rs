#[cfg(feature = "sqlite")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use async_graphql::{EmptyMutation, EmptySubscription, Object, Schema, dataloader::DataLoader};
    use cross_crate_source_models::{Activity, Session};
    use cross_crate_target_models::{CreateEndpointInput, Endpoint};
    use graphql_orm::{db::Database, graphql::loaders::RelationLoader, prelude::*};

    struct Query;
    #[Object]
    impl Query {
        async fn session(&self) -> Session {
            Session {
                id: "session-1".into(),
                endpoint_id: "endpoint-1".into(),
                tenant_id: "tenant-1".into(),
                endpoint: None,
            }
        }
        async fn activity(&self) -> Activity {
            Activity {
                id: "activity-1".into(),
                endpoint_id: Some("endpoint-1".into()),
                tenant_id: Some("tenant-1".into()),
                endpoint_name: Some("Name at occurrence".into()),
                endpoint: None,
            }
        }
    }
    let pool = graphql_orm::sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    let db = Database::<SqliteBackend>::new(pool);
    let plan = db
        .schema()
        .plan_migration_to_entities("example", "disposable target", &[Endpoint::metadata()])
        .await?;
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    Endpoint::insert(
        &db,
        CreateEndpointInput {
            id: "endpoint-1".into(),
            tenant_id: "tenant-1".into(),
            name: "Current name".into(),
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
    let response = schema.execute("{ session { id endpoint { id name } } activity { id endpointName endpoint { id name } } }").await;
    if !response.errors.is_empty() {
        return Err(format!("{:?}", response.errors).into());
    }
    println!("{}", response.data.into_json()?);
    Ok(())
}
#[cfg(not(feature = "sqlite"))]
fn main() {
    println!("Run this disposable example with the sqlite feature.");
}
