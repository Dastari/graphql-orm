#[cfg(feature = "mssql")]
#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::prelude::*;
use std::any::Any;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    not(feature = "mssql"),
    graphql_entity(
        table = "delete_nodes",
        plural = "DeleteNodes",
        repository_mutations = true
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "delete_nodes",
        plural = "DeleteNodes",
        repository_mutations = true
    )
)]
struct DeleteNode {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    tenant: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    parent_id: Option<String>,
    value: String,
    #[graphql(skip)]
    #[relation(target = "DeleteNode", from = ["tenant", "parent_id"], to = ["tenant", "id"], propagate_change = "up", emit_fk = false)]
    parent: Option<Box<DeleteNode>>,
}
#[cfg(not(feature = "mssql"))]
schema_roots! { query_custom_ops: [], entities: [DeleteNode] }
#[cfg(feature = "mssql")]
// Composite writes use the repository API; MSSQL has no generated GraphQL mutation root for them.
schema_roots! { backend: "mssql", query_custom_ops: [], entities: [DeleteNode] }
#[cfg(feature = "sqlite")]
struct FixtureOwner;
#[cfg(feature = "sqlite")]
impl FixtureOwner {
    fn cleanup(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }
}
#[cfg(feature = "postgres")]
type FixtureOwner = owned_postgres::OwnedPostgres;
#[cfg(feature = "mssql")]
type FixtureOwner = owned_mssql::OwnedSqlServer;

struct AllowEntityPolicy;
impl EntityPolicy for AllowEntityPolicy {
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

async fn fixture() -> Result<(Database, FixtureOwner), Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    {
        Ok((
            Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?,
            FixtureOwner,
        ))
    }
    #[cfg(feature = "postgres")]
    {
        let owner = FixtureOwner::start("delete-propagation")?;
        let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&owner.url)
            .await?;
        Ok((Database::new(pool), owner))
    }
    #[cfg(feature = "mssql")]
    {
        let owner = FixtureOwner::start().await?;
        let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(
            &owner.connection_string,
        )
        .await?;
        let mut db = Database::builder(pool)
            .schema_policy(SchemaPolicy::ExternalWritable)
            .build();
        db.set_entity_policy(AllowEntityPolicy);
        Ok((db, owner))
    }
}
async fn execute(db: &Database, sql: &str) -> graphql_orm::Result<()> {
    <DefaultBackend as WriteBackend>::execute_with_binds(db.pool(), sql, &[]).await?;
    Ok(())
}

struct ParentReader(std::sync::Arc<std::sync::atomic::AtomicBool>);
impl RowPolicy for ParentReader {
    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            let row = row.downcast_ref::<DeleteNode>().unwrap();
            Ok(
                (!row.id.starts_with("secret") || self.0.load(std::sync::atomic::Ordering::SeqCst))
                    && ctx
                        .and_then(|c| c.auth_subject_opt())
                        .is_some_and(|s| s.id == "allowed"),
            )
        })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}
fn key(id: &str) -> DeleteNodeKey {
    DeleteNodeKey {
        tenant: "t".into(),
        id: id.into(),
    }
}
async fn pending<S: graphql_orm::futures::Stream<Item = async_graphql::Response> + Unpin>(
    s: &mut S,
) {
    graphql_orm::futures::future::poll_fn(|cx| match s.poll_next_unpin(cx) {
        std::task::Poll::Pending => std::task::Poll::Ready(()),
        other => panic!("unexpected {other:?}"),
    })
    .await;
}
use graphql_orm::futures::StreamExt;
use tokio::time::{Duration, timeout};

#[tokio::test]
#[cfg_attr(not(feature = "sqlite"), ignore = "requires test-owned database")]
async fn delete_invalidation_is_parent_authorized_and_transactional()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut db, mut owner) = fixture().await?;
    execute(&db, "CREATE TABLE delete_nodes (tenant VARCHAR(80) NOT NULL, id VARCHAR(80) NOT NULL, parent_id VARCHAR(80) NULL, value VARCHAR(80) NOT NULL, PRIMARY KEY (tenant,id))").await?;
    execute(&db, "INSERT INTO delete_nodes VALUES ('t','root',NULL,'root-value'), ('t','parent','root','parent-value'), ('t','other',NULL,'other-value'), ('t','secret-child','parent','private-value'), ('t','secret-two','parent','private-two'), ('t','secret-orphan',NULL,'private-orphan')").await?;
    db.set_entity_policy(AllowEntityPolicy);
    let child_readable = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    db.set_row_policy(ParentReader(child_readable.clone()));
    let schema = schema_builder(db.clone()).finish();
    let query = |id: &str| {
        format!(
            r#"subscription {{ deleteNodeChanged(key: {{tenant: "t", id: "{id}"}}) {{ id key {{ tenant id }} action changeKind sourceEntity sourceId path deleteNode {{ tenant id value }} }} }}"#
        )
    };
    let mut allowed = Box::pin(schema.execute_stream(
        async_graphql::Request::new(query("parent")).data(AuthSubject::new("allowed")),
    ));
    let mut denied = Box::pin(schema.execute_stream(
        async_graphql::Request::new(query("parent")).data(AuthSubject::new("denied")),
    ));
    let mut other = Box::pin(schema.execute_stream(
        async_graphql::Request::new(query("other")).data(AuthSubject::new("allowed")),
    ));
    let mut excluded = Box::pin(
        schema.execute_stream(
            async_graphql::Request::new(
                r#"subscription { deleteNodeChanged(filter: {actions: [CREATED]}) { id } }"#,
            )
            .data(AuthSubject::new("allowed")),
        ),
    );
    let mut root = Box::pin(schema.execute_stream(
        async_graphql::Request::new(query("root")).data(AuthSubject::new("allowed")),
    ));
    pending(&mut allowed).await;
    pending(&mut denied).await;
    pending(&mut other).await;
    pending(&mut excluded).await;
    pending(&mut root).await;
    let rollback: Result<(), TransactionError> = db
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                tx.delete_by_key::<DeleteNode>(&key("secret-child")).await?;
                Err(OrmPublicError::forbidden())
            })
        })
        .await;
    assert!(rollback.is_err());
    assert!(
        timeout(Duration::from_millis(100), allowed.next())
            .await
            .is_err()
    );
    db.transaction(TransactionMode::Default, |tx| {
        Box::pin(async move {
            tx.delete_by_key::<DeleteNode>(&key("secret-orphan"))
                .await?;
            Ok(())
        })
    })
    .await?;
    assert!(
        timeout(Duration::from_millis(100), allowed.next())
            .await
            .is_err()
    );
    assert!(
        timeout(Duration::from_millis(100), root.next())
            .await
            .is_err()
    );
    let children = || DeleteNodeWhereInput {
        parent_id: Some(StringFilter {
            eq: Some("parent".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    // Pin today's bulk update count before comparing delete invalidations.
    assert_eq!(
        DeleteNode::update_where_bounded(
            &db,
            children(),
            UpdateDeleteNodeInput {
                value: Some("updated-private".into()),
                ..Default::default()
            },
            MutationLimit::new(2)?
        )
        .await?,
        BoundedMutationOutcome::Applied { affected: 2 }
    );
    for _ in 0..2 {
        for stream in [&mut allowed, &mut root] {
            let response = timeout(Duration::from_secs(2), stream.next())
                .await?
                .unwrap();
            assert!(response.errors.is_empty(), "{:?}", response.errors);
            assert_eq!(
                response.data.into_json()?["deleteNodeChanged"]["action"],
                "UPDATED"
            );
        }
    }
    assert!(
        timeout(Duration::from_millis(100), allowed.next())
            .await
            .is_err()
    );
    child_readable.store(false, std::sync::atomic::Ordering::SeqCst);
    // Deletes keep the same per-row propagation count; no transaction coalescing.
    assert_eq!(
        DeleteNode::delete_where_bounded(&db, children(), MutationLimit::new(2)?).await?,
        BoundedMutationOutcome::Applied { affected: 2 }
    );
    for _ in 0..2 {
        for (stream, id, depth) in [(&mut allowed, "parent", 1), (&mut root, "root", 2)] {
            let response = timeout(Duration::from_secs(2), stream.next())
                .await?
                .unwrap();
            assert!(response.errors.is_empty(), "{:?}", response.errors);
            let json = response.data.into_json()?;
            let event = &json["deleteNodeChanged"];
            assert_eq!(event["key"]["id"], id);
            assert_eq!(event["action"], "DELETED");
            assert_eq!(event["changeKind"], "PROPAGATED");
            assert_eq!(event["path"].as_array().unwrap().len(), depth);
            assert!(event["sourceEntity"].is_null());
            assert!(event["sourceId"].is_null());
            assert!(!json.to_string().contains("secret"));
            assert!(!json.to_string().contains("private"));
        }
    }
    assert!(
        timeout(Duration::from_millis(100), allowed.next())
            .await
            .is_err()
    );
    assert!(
        timeout(Duration::from_millis(100), denied.next())
            .await
            .is_err()
    );
    assert!(
        timeout(Duration::from_millis(100), other.next())
            .await
            .is_err()
    );
    assert!(
        timeout(Duration::from_millis(100), excluded.next())
            .await
            .is_err()
    );
    // Null parent: no propagation. A parent deleted in the same transaction cannot be a phantom.
    db.transaction(TransactionMode::Default, |tx| {
        Box::pin(async move {
            tx.delete_by_key::<DeleteNode>(&key("parent")).await?;
            tx.delete_by_key::<DeleteNode>(&key("root")).await?;
            tx.delete_by_key::<DeleteNode>(&key("other")).await?;
            Ok(())
        })
    })
    .await?;
    assert!(
        timeout(Duration::from_millis(100), root.next())
            .await
            .is_err()
    );
    drop(allowed);
    drop(denied);
    drop(other);
    drop(excluded);
    drop(root);
    drop(schema);
    drop(db);
    owner.cleanup()?;
    Ok(())
}
