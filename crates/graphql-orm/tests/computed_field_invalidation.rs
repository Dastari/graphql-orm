// This fixture selects one backend per build.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))),
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))),
))]

#[cfg(feature = "mssql")]
#[allow(dead_code)] // This fixture uses the owner's drop guard.
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use graphql_orm::async_graphql::{Context, Request, Response, Schema};
use graphql_orm::futures::{Stream, StreamExt, future::BoxFuture};
use graphql_orm::graphql::errors::OrmPublicError;
use graphql_orm::graphql::input_validation::GeneratedInputValidation;
use graphql_orm::graphql::subscriptions::{
    ComputedFieldInvalidationStream, ComputedFieldTopic, GeneratedSubscriptionAdmission,
    subscribe_computed_field,
};
use graphql_orm::prelude::*;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::time::{Duration, timeout};

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    not(feature = "mssql"),
    graphql_entity(table = "shelves", plural = "Shelves")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        table = "shelves",
        plural = "Shelves",
        schema_policy = "external_writable"
    )
)]
#[graphql_orm(object_fields)]
#[graphql_orm(operation_authorization(categories = ["subscription"], all_scopes = ["shelves.watch"]))]
struct Shelf {
    #[primary_key]
    #[sortable]
    #[filterable(type = "string")]
    #[graphql_orm(auto_generated = false)]
    id: String,
    owner: String,
    #[graphql_orm(version = "managed", default = "0")]
    revision: i64,
}

#[graphql_object_fields]
impl Shelf {
    async fn quantity(&self, ctx: &Context<'_>) -> async_graphql::Result<i32> {
        graphql_orm::graphql::auth::enforce_resolver_scopes(ctx, &[&["quantity.read"]])?;
        let db = ctx.data::<Database>()?;
        let rows = EntityQuery::<Placement, DefaultBackend>::new()
            .where_clause("shelf_id = ?", SqlValue::String(self.id.clone()))
            .fetch_all(db.pool())
            .await?;
        Ok(rows.into_iter().map(|row| row.quantity).sum())
    }
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    not(feature = "mssql"),
    graphql_entity(table = "placements", plural = "Placements")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        table = "placements",
        plural = "Placements",
        schema_policy = "external_writable"
    )
)]
struct Placement {
    #[primary_key]
    #[sortable]
    #[filterable(type = "string")]
    #[graphql_orm(auto_generated = false)]
    id: String,
    shelf_id: String,
    quantity: i32,
    #[graphql(skip)]
    #[relation(
        target = "Shelf",
        from = "shelf_id",
        to = "id",
        propagate_change = "up",
        propagate_change_action = "updated"
    )]
    shelf: Option<Shelf>,
}

#[derive(Default)]
struct Grants {
    target: AtomicBool,
    disclose: AtomicBool,
}

struct Quantity;

impl ComputedFieldTopic<DefaultBackend> for Quantity {
    type Target = Shelf;
    type Key = ShelfKey;
    const TOPIC: &'static str = "quantity";

    fn can_disclose<'a>(
        ctx: &'a Context<'_>,
        _: &'a Database,
        _: &'a Shelf,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(ctx.data::<Arc<Grants>>()?.disclose.load(Ordering::SeqCst)
                && ctx
                    .auth_subject_opt()
                    .is_some_and(|actor| actor.has_scope("quantity.read")))
        })
    }
}

struct GrantPolicy(Arc<Grants>);

impl EntityPolicy for GrantPolicy {
    fn can_access_entity<'a>(
        &'a self,
        ctx: Option<&'a Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if entity == "Shelf" && surface == EntityAccessSurface::GraphqlSubscription {
                return Ok(self.0.target.load(Ordering::SeqCst));
            }
            Ok(entity != "Placement"
                || ctx
                    .and_then(|ctx| ctx.auth_subject_opt())
                    .is_some_and(|actor| actor.has_scope("placements.read")))
        })
    }
}

struct Ownership;

impl RowPolicy for Ownership {
    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(row.downcast_ref::<Shelf>().is_none_or(|shelf| {
                ctx.and_then(|ctx| ctx.auth_subject_opt())
                    .is_some_and(|actor| actor.id == shelf.owner)
            }))
        })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

struct Invalidations;

#[async_graphql::Subscription]
impl Invalidations {
    async fn quantity_invalidated<'a>(
        &self,
        ctx: &Context<'a>,
    ) -> async_graphql::Result<ComputedFieldInvalidationStream<'a>> {
        subscribe_computed_field::<Quantity, DefaultBackend>(ctx).await
    }
}

#[derive(async_graphql::MergedSubscription)]
struct Subscriptions(Invalidations, ShelfSubscriptions, PlacementSubscriptions);

type TestSchema = Schema<ShelfQueries, PlacementMutations, Subscriptions>;

async fn fixture() -> (impl Sized, Database, TestSchema, Arc<Grants>) {
    #[cfg(feature = "sqlite")]
    let (mut db, owner) = (
        Database::connect_sqlite("sqlite::memory:").await.unwrap(),
        (),
    );
    #[cfg(feature = "postgres")]
    let (mut db, owner) = {
        let owner = owned_postgres::OwnedPostgres::start("computed-invalidation").unwrap();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(Duration::from_secs(10))
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("SET statement_timeout = '5s'")
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&owner.url)
            .await
            .unwrap();
        (Database::new(pool), owner)
    };
    #[cfg(feature = "mssql")]
    let (mut db, owner) = {
        use graphql_orm::db::mssql::{MssqlPool, MssqlPoolOptions};
        let owner = owned_mssql::OwnedSqlServer::start().await.unwrap();
        let pool = MssqlPool::connect_ado_external_writable_with(
            &owner.connection_string,
            MssqlPoolOptions::new()
                .max_connections(2)
                .query_timeout(Some(Duration::from_secs(5))),
        )
        .await
        .unwrap();
        (
            Database::builder(pool)
                .schema_policy(SchemaPolicy::ExternalWritable)
                .build(),
            owner,
        )
    };
    for sql in [
        "CREATE TABLE shelves (id VARCHAR(80) PRIMARY KEY, owner VARCHAR(80) NOT NULL, revision BIGINT NOT NULL)",
        "CREATE TABLE placements (id VARCHAR(80) PRIMARY KEY, shelf_id VARCHAR(80) NOT NULL, quantity INT NOT NULL)",
        "INSERT INTO shelves VALUES ('visible', 'reader', 7), ('foreign', 'other', 12)",
        "INSERT INTO placements VALUES ('private-placement', 'visible', 1)",
    ] {
        <DefaultBackend as WriteBackend>::execute_with_binds(db.pool(), sql, &[])
            .await
            .unwrap();
    }
    let grants = Arc::new(Grants {
        target: AtomicBool::new(true),
        disclose: AtomicBool::new(true),
    });
    db.set_entity_policy(GrantPolicy(grants.clone()));
    db.set_row_policy(Ownership);
    let schema = Schema::build(
        ShelfQueries,
        PlacementMutations,
        Subscriptions(Invalidations, ShelfSubscriptions, PlacementSubscriptions),
    )
    .data(db.clone())
    .data(grants.clone())
    .extension(GeneratedInputValidation)
    .extension(GeneratedSubscriptionAdmission(vec![
        "quantityInvalidated",
        "shelfChanged",
        "placementChanged",
    ]))
    .finish();
    (owner, db, schema, grants)
}

fn request(query: &str, scopes: &[&str]) -> Request {
    Request::new(query).data(
        AuthSubject::builder("reader")
            .scopes(scopes.iter().map(ToString::to_string).collect())
            .build(),
    )
}

const INVALIDATION: &str = "subscription { quantityInvalidated { id topic } }";
const READER_SCOPES: &[&str] = &["shelves.watch", "quantity.read"];

async fn listen(
    schema: &TestSchema,
    query: &str,
    scopes: &[&str],
) -> impl Stream<Item = Response> + Unpin {
    let mut stream = Box::pin(schema.execute_stream(request(query, scopes)));
    quiet(&mut stream).await;
    stream
}

async fn quiet(stream: &mut (impl Stream<Item = Response> + Unpin)) {
    assert!(
        timeout(Duration::from_millis(120), stream.next())
            .await
            .is_err()
    );
}

async fn read(stream: &mut (impl Stream<Item = Response> + Unpin)) -> Value {
    let response = timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().unwrap()
}

async fn update_placement_and_enqueue(
    tx: &mut MutationContext<'_>,
    key: String,
) -> Result<(), OrmPublicError> {
    tx.execute(
        "UPDATE placements SET quantity = 2 WHERE id = 'private-placement'",
        &[],
    )
    .await
    .map_err(|error| OrmPublicError::from_sqlx(&error))?;
    tx.invalidate_computed_field::<Quantity>(&key);
    Ok(())
}

async fn write_related(db: &Database, key: &str, rollback: bool) -> Result<(), TransactionError> {
    let key = key.to_owned();
    db.transaction(TransactionMode::Default, |tx| {
        Box::pin(async move {
            update_placement_and_enqueue(tx, key).await?;
            if rollback {
                // Even an explicit drain of the regular event queue must
                // leave this invalidation pending until a successful commit.
                tx.emit_queued_events();
                Err(OrmPublicError::forbidden())
            } else {
                Ok(())
            }
        })
    })
    .await
}

async fn assert_quantity(schema: &TestSchema, expected: i32) {
    let query = r#"{ shelf(id: "visible") { id revision quantity } }"#;
    let result = schema.execute(request(query, READER_SCOPES)).await;
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(
        result.data.into_json().unwrap(),
        json!({
            "shelf": {"id": "visible", "revision": 7, "quantity": expected},
        })
    );
    let denied = schema.execute(request(query, &["shelves.watch"])).await;
    assert!(!denied.errors.is_empty());
}

async fn preserves_changed_and_target_revision(db: &Database, schema: &TestSchema) {
    let mut invalidations = listen(schema, INVALIDATION, READER_SCOPES).await;
    let changed =
        "subscription { shelfChanged { id action changeKind sourceEntity sourceId path } }";
    let mut targets = listen(schema, changed, &["shelves.watch", "placements.read"]).await;
    let mut private_targets = listen(schema, changed, READER_SCOPES).await;
    let mut sources = listen(
        schema,
        "subscription { placementChanged { id action changeKind sourceEntity sourceId path } }",
        &["placements.read"],
    )
    .await;
    assert_quantity(schema, 1).await;
    write_related(db, "visible", false).await.unwrap();
    assert_eq!(
        read(&mut invalidations).await,
        json!({
            "quantityInvalidated": {"id": "visible", "topic": "quantity"},
        })
    );
    quiet(&mut invalidations).await;
    quiet(&mut targets).await;
    quiet(&mut sources).await;
    assert_quantity(schema, 2).await;

    // An ordinary source write still has both Changed events and the same
    // provenance release contract. No invalidation is inferred from it.
    let created = schema.execute(request(
        r#"mutation { createPlacement(input: {id: "new-placement", shelfId: "visible", quantity: 5}) { success error } }"#,
        &["placements.read"],
    )).await;
    assert!(created.errors.is_empty(), "{:?}", created.errors);
    assert_eq!(
        created.data.into_json().unwrap()["createPlacement"]["success"],
        true
    );
    assert_eq!(
        read(&mut sources).await,
        json!({"placementChanged": {
            "id": "new-placement", "action": "CREATED", "changeKind": "DIRECT",
            "sourceEntity": null, "sourceId": null, "path": [],
        }})
    );
    assert_eq!(
        read(&mut targets).await,
        json!({"shelfChanged": {
            "id": "visible", "action": "UPDATED", "changeKind": "PROPAGATED",
            "sourceEntity": "Placement", "sourceId": "new-placement", "path": ["shelf"],
        }})
    );
    quiet(&mut private_targets).await;
    quiet(&mut invalidations).await;
    assert_quantity(schema, 7).await;
}

async fn rollback_and_delivery_denials(db: &Database, schema: &TestSchema, grants: &Grants) {
    let refused = schema.execute_stream(request(INVALIDATION, &["quantity.read"]));
    let mut refused = Box::pin(refused);
    let refused = timeout(Duration::from_secs(5), refused.next())
        .await
        .unwrap()
        .unwrap();
    assert!(!refused.errors.is_empty());

    let mut stream = listen(schema, INVALIDATION, READER_SCOPES).await;
    assert!(write_related(db, "visible", true).await.is_err());
    quiet(&mut stream).await;
    assert_quantity(schema, 1).await;

    write_related(db, "foreign", false).await.unwrap();
    quiet(&mut stream).await;

    write_related(db, "visible", false).await.unwrap();
    grants.disclose.store(false, Ordering::SeqCst);
    quiet(&mut stream).await;
    grants.disclose.store(true, Ordering::SeqCst);

    write_related(db, "visible", false).await.unwrap();
    grants.target.store(false, Ordering::SeqCst);
    quiet(&mut stream).await;
    grants.target.store(true, Ordering::SeqCst);

    write_related(db, "visible", false).await.unwrap();
    assert_eq!(
        read(&mut stream).await,
        json!({
            "quantityInvalidated": {"id": "visible", "topic": "quantity"},
        })
    );
    quiet(&mut stream).await;
}

#[tokio::test]
#[cfg_attr(
    not(feature = "sqlite"),
    ignore = "starts a capped, test-owned database"
)]
async fn commit_delivers_once_and_the_normal_query_reads_the_recomputed_value() {
    let (_owner, db, schema, _) = fixture().await;
    let mut invalidations = listen(&schema, INVALIDATION, READER_SCOPES).await;
    assert_quantity(&schema, 1).await;
    // Pending asynchronous actions must not delay a committed notification.
    let (resume, pending) = tokio::sync::oneshot::channel();
    let transaction = db.transaction(TransactionMode::Default, |tx| {
        Box::pin(async move {
            tx.defer(move |_| async {
                pending.await?;
                Ok::<(), tokio::sync::oneshot::error::RecvError>(())
            });
            update_placement_and_enqueue(tx, "visible".to_owned()).await
        })
    });
    let receive = async {
        assert_eq!(
            read(&mut invalidations).await["quantityInvalidated"]["id"],
            "visible"
        );
        assert_quantity(&schema, 2).await;
        resume.send(()).unwrap();
    };
    let (committed, ()) = tokio::join!(transaction, receive);
    committed.unwrap();
    quiet(&mut invalidations).await;
}

#[tokio::test]
#[cfg_attr(
    not(feature = "sqlite"),
    ignore = "starts a capped, test-owned database"
)]
async fn target_revision_and_existing_changed_events_keep_their_contracts() {
    let (_owner, db, schema, _) = fixture().await;
    preserves_changed_and_target_revision(&db, &schema).await;
}

#[tokio::test]
#[cfg_attr(
    not(feature = "sqlite"),
    ignore = "starts a capped, test-owned database"
)]
async fn payload_is_exactly_the_target_id_and_topic() {
    let (_owner, db, schema, _) = fixture().await;
    let mut invalidations = listen(&schema, INVALIDATION, READER_SCOPES).await;
    write_related(&db, "visible", false).await.unwrap();
    assert_eq!(
        read(&mut invalidations).await,
        json!({
            "quantityInvalidated": {"id": "visible", "topic": "quantity"},
        })
    );
    let shape = schema
        .execute(r#"{ __type(name: "ComputedFieldInvalidation") { fields { name } } }"#)
        .await;
    assert!(shape.errors.is_empty(), "{:?}", shape.errors);
    assert_eq!(
        shape.data.into_json().unwrap(),
        json!({
            "__type": {"fields": [{"name": "id"}, {"name": "topic"}]},
        })
    );
}

#[tokio::test]
#[cfg_attr(
    not(feature = "sqlite"),
    ignore = "starts a capped, test-owned database"
)]
async fn rollback_foreign_row_revoked_grant_and_disclosure_denial() {
    let (_owner, db, schema, grants) = fixture().await;
    rollback_and_delivery_denials(&db, &schema, &grants).await;
}
