#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
use graphql_orm::async_graphql::{Request, Response};
use graphql_orm::futures::{Stream, StreamExt, future::poll_fn};
use graphql_orm::prelude::*;
use std::task::Poll;
use tokio::time::{Duration, timeout};
#[cfg(feature = "mssql")]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "portable_subscription_rows",
        plural = "PortableRows"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "portable_subscription_rows",
        plural = "PortableRows"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "portable_subscription_rows",
        plural = "PortableRows"
    )
)]
struct PortableRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    owner: String,
    label: String,
}
#[cfg(feature = "sqlite")]
schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [PortableRow], }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [PortableRow], }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [PortableRow], }
struct OwnedRowPolicy;
impl RowPolicy for OwnedRowPolicy {
    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(ctx.and_then(|c| c.auth_subject_opt()).is_some_and(|s| {
                row.downcast_ref::<PortableRow>()
                    .is_some_and(|r| r.owner == s.id)
            }))
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

#[cfg(feature = "mssql")]
struct AllowEntityPolicy;
#[cfg(feature = "mssql")]
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
        let owner = FixtureOwner::start("subscription-release")?;
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

async fn registered<S: Stream<Item = Response> + Unpin>(stream: &mut S) {
    poll_fn(|cx| match stream.poll_next_unpin(cx) {
        Poll::Pending => Poll::Ready(()),
        other => panic!("unexpected initial event {other:?}"),
    })
    .await;
}
async fn publish(schema: &AppSchema, id: &str, owner: &str) {
    let response = schema.execute(Request::new(format!("mutation {{ createPortableRow(input: {{id: \"{id}\", owner: \"{owner}\", label: \"historical-private-label\"}}) {{ success error }} }}")).data(AuthSubject::new(owner))).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["createPortableRow"]["success"],
        true
    );
}
#[tokio::test]
async fn current_typed_keys_and_authorized_payloads_are_portable()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut db, mut owner) = fixture().await?;
    execute(&db, "CREATE TABLE portable_subscription_rows (id VARCHAR(80) PRIMARY KEY, owner VARCHAR(80) NOT NULL, label VARCHAR(100) NOT NULL)").await?;
    db.set_row_policy(OwnedRowPolicy);
    let schema = schema_builder(db.clone()).finish();
    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new("subscription { portableRowChanged { id portableRow { owner label } } }")
                .data(AuthSubject::new("alice")),
        ),
    );
    registered(&mut stream).await;
    publish(&schema, "denied-bob", "bob").await;
    publish(&schema, "transferred", "bob").await;
    execute(&db, "UPDATE portable_subscription_rows SET owner = 'alice', label = 'current-authorized-label' WHERE id = 'transferred'").await?;
    let response = timeout(Duration::from_secs(3), stream.next())
        .await?
        .unwrap();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data["portableRowChanged"]["id"], "transferred");
    assert_eq!(
        data["portableRowChanged"]["portableRow"]["label"],
        "current-authorized-label"
    );
    assert!(!data.to_string().contains("historical-private-label"));
    drop(stream);
    drop(schema);
    drop(db);
    owner.cleanup()?;
    Ok(())
}

#[cfg(feature = "postgres")]
struct PermissiveRlsBackedPolicy;
#[cfg(feature = "postgres")]
impl RowPolicy for PermissiveRlsBackedPolicy {
    fn can_read_row<'a>(&'a self, _: Option<&'a async_graphql::Context<'_>>, _: &'a Database,
        _: &'static str, _: Option<&'static str>, _: EntityAccessSurface, _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> { Box::pin(async { Ok(true) }) }
    fn can_write_row<'a>(&'a self, _: Option<&'a async_graphql::Context<'_>>, _: &'a Database,
        _: &'static str, _: Option<&'static str>, _: EntityAccessSurface, _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> { Box::pin(async { Ok(true) }) }
}
#[cfg(feature = "postgres")]
fn rls_request(query: impl Into<String>, actor: &str) -> Request {
    let subject = AuthSubject::new(actor);
    Request::new(query).data(DbAuthContext::from_subject(&subject)).data(subject)
}
#[cfg(feature = "postgres")]
#[tokio::test]
async fn rls_only_visibility_never_authorizes_another_owners_deleted_preimage() -> Result<(), Box<dyn std::error::Error>> {
    let (admin, mut owner) = fixture().await?;
    for sql in [
        "CREATE ROLE subscription_rls_reader NOLOGIN NOSUPERUSER NOBYPASSRLS",
        "CREATE TABLE portable_subscription_rows (id TEXT PRIMARY KEY, owner TEXT NOT NULL, label TEXT NOT NULL)",
        "ALTER TABLE portable_subscription_rows ENABLE ROW LEVEL SECURITY",
        "ALTER TABLE portable_subscription_rows FORCE ROW LEVEL SECURITY",
        "CREATE POLICY owner_access ON portable_subscription_rows FOR ALL USING (owner = current_setting('app.user_id', true)) WITH CHECK (owner = current_setting('app.user_id', true))",
        "GRANT USAGE ON SCHEMA public TO subscription_rls_reader",
        "GRANT SELECT, INSERT, UPDATE, DELETE ON portable_subscription_rows TO subscription_rls_reader",
    ] { execute(&admin, sql).await?; }
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(2).after_connect(|connection, _| {
        Box::pin(async move {
            sqlx::query("SET ROLE subscription_rls_reader").execute(connection).await?;
            Ok(())
        })
    }).connect(&owner.url).await?;
    for permissive_callback in [false, true] {
        let private_id = if permissive_callback { "private-bob-callback" } else { "private-bob-no-provider" };
        let public_id = if permissive_callback { "public-alice-callback" } else { "public-alice-no-provider" };
        execute(&admin, &format!("INSERT INTO portable_subscription_rows VALUES ('{private_id}', 'bob', 'synthetic-private-label')")).await?;
        let auth = DbAuthContext::from_subject(&AuthSubject::new("alice"));
        let rows = PostgresBackend::fetch_rows_with_auth(&pool, "SELECT id FROM portable_subscription_rows WHERE owner = 'bob'", &[], Some(&auth)).await?;
        assert!(rows.is_empty(), "test actor bypassed RLS");
        let mut db = Database::new(pool.clone());
        if permissive_callback { db.set_row_policy(PermissiveRlsBackedPolicy); }
        let schema = schema_builder(db).finish();
        let mut stream = Box::pin(schema.execute_stream(rls_request(
            "subscription { portableRowChanged { id portableRow { owner label } } }", "alice"
        )));
        registered(&mut stream).await;
        let deleted = schema.execute(rls_request(format!("mutation {{ deletePortableRow(id: \"{private_id}\") {{ success error }} }}"), "bob")).await;
        assert!(deleted.errors.is_empty(), "{:?}", deleted.errors);
        assert_eq!(deleted.data.into_json()?["deletePortableRow"]["success"], true);
        let created = schema.execute(rls_request(format!("mutation {{ createPortableRow(input: {{id: \"{public_id}\", owner: \"alice\", label: \"public-label\"}}) {{success error}} }}"), "alice")).await;
        assert!(created.errors.is_empty(), "{:?}", created.errors);
        assert_eq!(created.data.into_json()?["createPortableRow"]["success"], true);
        let response = timeout(Duration::from_secs(3), stream.next()).await?.unwrap();
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json()?;
        assert_eq!(data["portableRowChanged"]["id"], public_id, "permissive_callback={permissive_callback}");
        assert!(!data.to_string().contains("private-bob"));
        assert!(!data.to_string().contains("synthetic-private-label"));
    }
    pool.close().await;
    admin.pool().close().await;
    owner.cleanup()?;
    Ok(())
}
