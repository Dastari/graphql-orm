#![cfg(feature = "sqlite")]
use graphql_orm::async_graphql::{Request, Response};
use graphql_orm::futures::{Stream, StreamExt, future::poll_fn};
use graphql_orm::prelude::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::task::Poll;
use tokio::time::{Duration, timeout};

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "tuple_parents", plural = "TupleParents")]
struct TupleParent {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    id: String,
    organization_id: String,
    provider_account_id: String,
    subscription_id: String,
}
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "tuple_children", plural = "TupleChildren")]
struct TupleChild {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    cache_id: String,
    organization_id: String,
    provider_account_id: String,
    subscription_id: String,
    #[graphql(skip)]
    #[relation(target = "TupleParent", from = ["organization_id", "provider_account_id", "subscription_id"], to = ["organization_id", "provider_account_id", "subscription_id"], propagate_change = "up", emit_fk = false)]
    parent: Option<TupleParent>,
}
schema_roots! { query_custom_ops: [], entities: [TupleParent, TupleChild], }
#[derive(Clone)]
struct Policy {
    source: Arc<AtomicBool>,
    target: Arc<AtomicBool>,
    fields: Arc<AtomicBool>,
    target_fields: Arc<AtomicBool>,
}
impl RowPolicy for Policy {
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(if entity == "TupleChild" {
                self.source.load(Ordering::SeqCst)
            } else {
                self.target.load(Ordering::SeqCst)
            })
        })
    }
    fn can_write_row<'a>(
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
}
impl FieldPolicy for Policy {
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        entity: &'static str,
        field: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok((entity != "TupleChild"
                || field != "organizationId"
                || self.fields.load(Ordering::SeqCst))
                && (entity != "TupleParent"
                    || field != "providerAccountId"
                    || self.target_fields.load(Ordering::SeqCst)))
        })
    }
    fn can_write_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}
async fn setup() -> (Database, Policy) {
    let mut db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    sqlx::query("CREATE TABLE tuple_parents (id TEXT PRIMARY KEY, organization_id TEXT NOT NULL, provider_account_id TEXT NOT NULL, subscription_id TEXT NOT NULL)").execute(db.pool()).await.unwrap();
    sqlx::query("CREATE TABLE tuple_children (cache_id TEXT PRIMARY KEY, organization_id TEXT NOT NULL, provider_account_id TEXT NOT NULL, subscription_id TEXT NOT NULL)").execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO tuple_parents VALUES ('parent-a','org-a','account','shared'), ('parent-b','org-b','account','shared')").execute(db.pool()).await.unwrap();
    let policy = Policy {
        source: Arc::new(AtomicBool::new(true)),
        target: Arc::new(AtomicBool::new(true)),
        fields: Arc::new(AtomicBool::new(true)),
        target_fields: Arc::new(AtomicBool::new(true)),
    };
    db.set_row_policy(policy.clone());
    db.set_field_policy(policy.clone());
    (db, policy)
}
fn input(id: &str) -> CreateTupleChildInput {
    CreateTupleChildInput {
        cache_id: id.into(),
        organization_id: "org-a".into(),
        provider_account_id: "account".into(),
        subscription_id: "shared".into(),
    }
}
async fn registered<S: Stream<Item = Response> + Unpin>(stream: &mut S) {
    poll_fn(|cx| match stream.poll_next_unpin(cx) {
        Poll::Pending => Poll::Ready(()),
        other => panic!("unexpected initial event {other:?}"),
    })
    .await;
}
const QUERY: &str =
    "subscription { tupleParentChanged { id sourceId path tupleParent { organizationId } } }";

#[tokio::test]
async fn complete_relation_tuple_finds_scalar_parent_without_using_child_primary_key() {
    let (db, _) = setup().await;
    let schema = schema_builder(db.clone()).finish();
    let mut stream =
        Box::pin(schema.execute_stream(Request::new(QUERY).data(AuthSubject::new("tester"))));
    registered(&mut stream).await;
    TupleChild::insert(&db, input("unrelated-cache-pk"))
        .await
        .unwrap();
    let response = timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let value = response.data.into_json().unwrap();
    assert_eq!(value["tupleParentChanged"]["id"], "parent-a");
    assert_eq!(
        value["tupleParentChanged"]["sourceId"],
        "unrelated-cache-pk"
    );
    assert_eq!(
        value["tupleParentChanged"]["tupleParent"]["organizationId"],
        "org-a"
    );
    assert!(
        timeout(Duration::from_millis(100), stream.next())
            .await
            .is_err(),
        "must not notify colliding org-b parent"
    );
}
#[tokio::test]
async fn source_target_and_each_tuple_field_revoke_after_registration() {
    for denied in ["source", "target", "field", "target_field"] {
        let (db, policy) = setup().await;
        let schema = schema_builder(db.clone()).finish();
        let mut stream =
            Box::pin(schema.execute_stream(Request::new(QUERY).data(AuthSubject::new("tester"))));
        registered(&mut stream).await;
        match denied {
            "source" => policy.source.store(false, Ordering::SeqCst),
            "target" => policy.target.store(false, Ordering::SeqCst),
            "target_field" => policy.target_fields.store(false, Ordering::SeqCst),
            _ => policy.fields.store(false, Ordering::SeqCst),
        }
        TupleChild::insert(&db, input("denied-cache"))
            .await
            .unwrap();
        assert!(
            timeout(Duration::from_millis(100), stream.next())
                .await
                .is_err(),
            "released denied {denied}"
        );
    }
}
#[tokio::test]
async fn rolled_back_tuple_notification_never_reaches_generated_stream() {
    let (db, _) = setup().await;
    let schema = schema_builder(db.clone()).finish();
    let mut stream =
        Box::pin(schema.execute_stream(Request::new(QUERY).data(AuthSubject::new("tester"))));
    registered(&mut stream).await;
    let result: Result<(), TransactionError> = db
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                tx.insert::<TupleChild>(input("rolled-back")).await?;
                Err(OrmPublicError::forbidden())
            })
        })
        .await;
    assert!(result.is_err());
    assert!(
        timeout(Duration::from_millis(100), stream.next())
            .await
            .is_err()
    );
}
#[tokio::test]
async fn moving_relation_after_commit_suppresses_stale_provenance() {
    let (db, _) = setup().await;
    let schema = schema_builder(db.clone()).finish();
    let mut stream =
        Box::pin(schema.execute_stream(Request::new(QUERY).data(AuthSubject::new("tester"))));
    registered(&mut stream).await;
    TupleChild::insert(&db, input("moved-cache")).await.unwrap();
    sqlx::query("UPDATE tuple_children SET organization_id='org-b' WHERE cache_id='moved-cache'")
        .execute(db.pool())
        .await
        .unwrap();
    assert!(
        timeout(Duration::from_millis(100), stream.next())
            .await
            .is_err()
    );
}

// Provider IDs and cache primary keys deliberately have different Rust types.
// Using the first relation field as the parent's key fails to compile here.
mod alternate_provider_keys {
    use super::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(table = "alternate_organizations", plural = "AlternateOrganizations")]
    struct AlternateOrganization {
        #[primary_key]
        #[sortable]
        #[graphql_orm(auto_generated = false)]
        cache_id: String,
        provider_id: i32,
    }

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(table = "alternate_devices", plural = "AlternateDevices")]
    struct AlternateDevice {
        #[primary_key]
        #[sortable]
        #[graphql_orm(auto_generated = false)]
        cache_id: String,
        organization_id: i32,
        #[graphql(skip)]
        #[relation(
            target = "AlternateOrganization",
            from = "organization_id",
            to = "provider_id",
            propagate_change = "up",
            emit_fk = false
        )]
        organization: Option<AlternateOrganization>,
    }

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(table = "provider_locations", plural = "ProviderLocations")]
    struct ProviderLocation {
        #[primary_key]
        #[sortable]
        #[graphql_orm(auto_generated = false)]
        cache_id: String,
        organization_id: i32,
        provider_id: i32,
    }

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(table = "location_devices", plural = "LocationDevices")]
    struct LocationDevice {
        #[primary_key]
        #[sortable]
        #[graphql_orm(auto_generated = false)]
        cache_id: String,
        organization_id: i32,
        location_id: i32,
        #[graphql(skip)]
        #[relation(target = "ProviderLocation", from = ["organization_id", "location_id"], to = ["organization_id", "provider_id"], propagate_change = "up", emit_fk = false)]
        location: Option<ProviderLocation>,
    }

    schema_roots! { query_custom_ops: [], entities: [AlternateOrganization, AlternateDevice, ProviderLocation, LocationDevice], }

    #[tokio::test]
    async fn scalar_alternate_i32_relation_resolves_string_cache_primary_key() {
        let db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE alternate_organizations (cache_id TEXT PRIMARY KEY, provider_id INTEGER NOT NULL UNIQUE)").execute(db.pool()).await.unwrap();
        sqlx::query("CREATE TABLE alternate_devices (cache_id TEXT PRIMARY KEY, organization_id INTEGER NOT NULL)").execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO alternate_organizations VALUES ('org-cache-key',42)")
            .execute(db.pool())
            .await
            .unwrap();
        let schema = schema_builder(db.clone()).finish();
        let mut stream = Box::pin(
            schema.execute_stream(
                Request::new("subscription { alternateOrganizationChanged { id sourceId } }")
                    .data(AuthSubject::new("tester")),
            ),
        );
        registered(&mut stream).await;
        AlternateDevice::insert(
            &db,
            CreateAlternateDeviceInput {
                cache_id: "device-cache-key".into(),
                organization_id: 42,
            },
        )
        .await
        .unwrap();
        let response = timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let value = response.data.into_json().unwrap();
        assert_eq!(value["alternateOrganizationChanged"]["id"], "org-cache-key");
        assert_eq!(
            value["alternateOrganizationChanged"]["sourceId"],
            "device-cache-key"
        );
        assert!(
            timeout(Duration::from_millis(100), stream.next())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn numeric_composite_alternate_relation_keeps_organization_in_binding() {
        let db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE provider_locations (cache_id TEXT PRIMARY KEY, organization_id INTEGER NOT NULL, provider_id INTEGER NOT NULL, UNIQUE(organization_id,provider_id))").execute(db.pool()).await.unwrap();
        sqlx::query("CREATE TABLE location_devices (cache_id TEXT PRIMARY KEY, organization_id INTEGER NOT NULL, location_id INTEGER NOT NULL)").execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO provider_locations VALUES ('location-org-a',1,42), ('location-org-b',2,42)").execute(db.pool()).await.unwrap();
        let schema = schema_builder(db.clone()).finish();
        let mut stream = Box::pin(
            schema.execute_stream(
                Request::new("subscription { providerLocationChanged { id sourceId } }")
                    .data(AuthSubject::new("tester")),
            ),
        );
        registered(&mut stream).await;
        LocationDevice::insert(
            &db,
            CreateLocationDeviceInput {
                cache_id: "device-other-key".into(),
                organization_id: 1,
                location_id: 42,
            },
        )
        .await
        .unwrap();
        let response = timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let value = response.data.into_json().unwrap();
        assert_eq!(value["providerLocationChanged"]["id"], "location-org-a");
        assert_eq!(
            value["providerLocationChanged"]["sourceId"],
            "device-other-key"
        );
        assert!(
            timeout(Duration::from_millis(100), stream.next())
                .await
                .is_err(),
            "must not release org-b's colliding provider ID"
        );
    }
}
