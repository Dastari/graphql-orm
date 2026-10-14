#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]

use graphql_orm::async_graphql::{
    EmptyMutation, EmptySubscription, Object, Request, Response, Schema, SimpleObject,
};
use graphql_orm::graphql::errors::{graphql_error_from_sqlx, graphql_internal_error};
use graphql_orm::prelude::*;

#[cfg(feature = "mssql")]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
    PartialEq,
)]
#[graphql(complex)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "sanitizer_private_nodes",
        plural = "SanitizerNodes",
        default_sort = "id ASC",
        upsert = "label",
        federation_key
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "sanitizer_private_nodes",
        plural = "SanitizerNodes",
        default_sort = "id ASC",
        upsert = "label",
        federation_key
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "sanitizer_private_nodes",
        plural = "SanitizerNodes",
        default_sort = "id ASC",
        schema_policy = "external_writable",
        upsert = "label",
        federation_key
    )
)]
struct SanitizerNode {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[unique]
    #[filterable(type = "string")]
    #[sortable]
    label: String,
    #[filterable(type = "string")]
    parent_id: Option<String>,
    #[graphql(skip)]
    #[relation(
        target = "SanitizerNode",
        from = "parent_id",
        to = "id",
        emit_fk = false
    )]
    parent: Option<Box<SanitizerNode>>,
    #[graphql(skip)]
    #[relation(
        target = "SanitizerNode",
        from = "id",
        to = "parent_id",
        multiple,
        emit_fk = false
    )]
    children: Vec<SanitizerNode>,
}
impl BatchLoadEntity for SanitizerNode {
    fn batch_column() -> &'static str {
        "parent_id"
    }
    fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
        DefaultBackend::try_get_string(row, "parent_id")
    }
}
#[cfg(feature = "sqlite")]
schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [SanitizerNode], }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [SanitizerNode], }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [SanitizerNode], }

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
        let owner = FixtureOwner::start("generated-error-sanitization")?;
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
fn request(query: &str) -> Request {
    Request::new(query).data(AuthSubject::new("synthetic-principal"))
}
fn assert_private_details_absent(response: &Response) {
    let rendered = serde_json::to_string(response).unwrap();
    for private in [
        "sanitizer_private",
        "synthetic-secret",
        "no such table",
        "does not exist",
        "Invalid object",
        "duplicate key",
        "SELECT ",
        "INSERT ",
        "UPDATE ",
        "DELETE ",
        "password=",
    ] {
        assert!(
            !rendered.contains(private),
            "public response leaked {private}: {rendered}"
        );
    }
}
fn assert_code(response: &Response, code: &str, message: &str) {
    assert!(
        !response.errors.is_empty(),
        "expected failure: {:?}",
        response.data
    );
    assert_private_details_absent(response);
    for error in &response.errors {
        assert_eq!(error.message, message);
        assert_eq!(
            error.extensions.as_ref().and_then(|e| e.get("code")),
            Some(&async_graphql::Value::String(code.into()))
        );
    }
}

#[tokio::test]
async fn missing_table_queries_mutations_and_federation_use_stable_public_errors()
-> Result<(), Box<dyn std::error::Error>> {
    let (db, mut owner) = fixture().await?;
    let schema = schema_builder(db).finish();
    for query in [
        "{ sanitizerNode(id: \"a\") { id label } }",
        "{ sanitizerNodes(page: {limit: 1}) { edges { node { id label } } pageInfo { totalCount } } }",
        "mutation { createSanitizerNode(input: {id: \"a\", label: \"synthetic-secret\"}) { success error } }",
        "mutation { upsertSanitizerNode(input: {id: \"a\", label: \"synthetic-secret\"}) { success error } }",
        "mutation { updateSanitizerNode(id: \"a\", input: {label: \"synthetic-secret\"}) { success error } }",
        "mutation { deleteSanitizerNode(id: \"a\") { success error } }",
        "{ _entities(representations: [{__typename: \"SanitizerNode\", id: \"a\"}]) { ... on SanitizerNode { id label } } }",
    ] {
        let response = schema.execute(request(query)).await;
        assert_code(&response, "INTERNAL_ERROR", "internal error");
    }
    owner.cleanup()?;
    Ok(())
}

struct PreloadedQuery;
#[Object]
impl PreloadedQuery {
    async fn source(&self) -> SanitizerNode {
        let snapshot = SanitizerNode {
            id: "b".into(),
            label: "synthetic-secret".into(),
            parent_id: None,
            parent: None,
            children: vec![],
        };
        SanitizerNode {
            id: "a".into(),
            label: "source".into(),
            parent_id: Some("b".into()),
            parent: Some(Box::new(snapshot.clone())),
            children: vec![snapshot],
        }
    }
}
#[tokio::test]
async fn preloaded_and_loader_relation_failures_are_sanitized()
-> Result<(), Box<dyn std::error::Error>> {
    let (db, mut owner) = fixture().await?;
    for use_loader in [false, true] {
        let builder =
            Schema::build(PreloadedQuery, EmptyMutation, EmptySubscription).data(db.clone());
        let schema = if use_loader {
            builder
                .data(async_graphql::dataloader::DataLoader::new(
                    RelationLoader::<SanitizerNode>::new(db.clone()),
                    tokio::spawn,
                ))
                .finish()
        } else {
            builder.finish()
        };
        for query in [
            "{ source { parent { id label } } }",
            "{ source { children(page: {limit: 1}) { edges { node { id label } } } } }",
        ] {
            let response = schema.execute(request(query)).await;
            assert_code(&response, "INTERNAL_ERROR", "internal error");
        }
    }
    owner.cleanup()?;
    Ok(())
}

#[tokio::test]
async fn constraint_failures_sanitize_top_level_and_legacy_mutation_payloads()
-> Result<(), Box<dyn std::error::Error>> {
    let (db, mut owner) = fixture().await?;
    execute(&db, "CREATE TABLE sanitizer_private_nodes (id VARCHAR(80) PRIMARY KEY, label VARCHAR(80) NOT NULL UNIQUE, parent_id VARCHAR(80) NULL)").await?;
    execute(
        &db,
        "INSERT INTO sanitizer_private_nodes VALUES ('a', 'alpha', NULL), ('b', 'beta', NULL)",
    )
    .await?;
    execute(&db, "CREATE TABLE sanitizer_private_refs (node_id VARCHAR(80) NOT NULL REFERENCES sanitizer_private_nodes(id))").await?;
    execute(&db, "INSERT INTO sanitizer_private_refs VALUES ('a')").await?;
    let schema = schema_builder(db).finish();
    let create = schema.execute(request("mutation { createSanitizerNode(input: {id: \"a\", label: \"synthetic-secret\"}) { success error } }")).await;
    // Tiberius currently maps server constraints into SQLx Protocol; the
    // existing backend-neutral sanitizer intentionally reports INTERNAL_ERROR.
    #[cfg(not(feature = "mssql"))]
    assert_code(&create, "CONSTRAINT_VIOLATION", "constraint violation");
    #[cfg(feature = "mssql")]
    assert_code(&create, "INTERNAL_ERROR", "internal error");
    for (query, field) in [
        (
            "mutation { updateSanitizerNode(id: \"a\", input: {label: \"beta\"}) { success error } }",
            "updateSanitizerNode",
        ),
        (
            "mutation { updateManySanitizerNodes(where: {id: {eq: \"a\"}}, input: {label: \"beta\"}) { success error affectedCount } }",
            "updateManySanitizerNodes",
        ),
        (
            "mutation { deleteSanitizerNode(id: \"a\") { success error } }",
            "deleteSanitizerNode",
        ),
        (
            "mutation { deleteManySanitizerNodes(where: {id: {eq: \"a\"}}) { success error deletedCount } }",
            "deleteManySanitizerNodes",
        ),
    ] {
        let response = schema.execute(request(query)).await;
        assert_private_details_absent(&response);
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json()?;
        assert_eq!(data[field]["success"], false);
        #[cfg(not(feature = "mssql"))]
        assert_eq!(data[field]["error"], "constraint violation");
        #[cfg(feature = "mssql")]
        assert_eq!(data[field]["error"], "internal error");
    }
    owner.cleanup()?;
    Ok(())
}

#[test]
fn timeout_connection_and_decode_diagnostics_do_not_enter_public_fields() {
    for error in [
        graphql_orm::sqlx::Error::PoolTimedOut,
        graphql_orm::sqlx::Error::PoolClosed,
    ] {
        let public = graphql_error_from_sqlx(error);
        assert_eq!(public.message, "service unavailable");
        assert_eq!(
            public.extensions.as_ref().unwrap().get("code"),
            Some(&async_graphql::Value::String("SERVICE_UNAVAILABLE".into()))
        );
    }
    let public = graphql_error_from_sqlx(graphql_orm::sqlx::Error::ColumnDecode {
        index: "sanitizer_private_column".into(),
        source: "password=synthetic-secret".into(),
    });
    let other = graphql_internal_error("loader SELECT sanitizer_private password=synthetic-secret");
    for error in [public, other] {
        assert_eq!(error.message, "internal error");
        assert!(!format!("{:?}", error.extensions).contains("synthetic-secret"));
    }
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn generated_pool_timeout_and_closed_connection_are_service_unavailable() {
    let pool = graphql_orm::sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_millis(50))
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let schema = schema_builder(Database::new(pool.clone())).finish();
    let lease = pool.acquire().await.unwrap();
    let response = schema
        .execute(request("{ sanitizerNode(id: \"a\") { id label } }"))
        .await;
    assert_code(&response, "SERVICE_UNAVAILABLE", "service unavailable");
    drop(lease);
    pool.close().await;
    let response = schema
        .execute(request("{ sanitizerNode(id: \"a\") { id label } }"))
        .await;
    assert_code(&response, "SERVICE_UNAVAILABLE", "service unavailable");
}

#[tokio::test]
async fn generated_decode_failure_keeps_column_and_type_details_private()
-> Result<(), Box<dyn std::error::Error>> {
    let (db, mut owner) = fixture().await?;
    execute(&db, "CREATE TABLE sanitizer_private_nodes (id VARCHAR(80) PRIMARY KEY, label INTEGER NOT NULL, parent_id VARCHAR(80) NULL)").await?;
    execute(
        &db,
        "INSERT INTO sanitizer_private_nodes VALUES ('a', 42, NULL)",
    )
    .await?;
    let response = schema_builder(db)
        .finish()
        .execute(request("{ sanitizerNode(id: \"a\") { id label } }"))
        .await;
    assert_code(&response, "INTERNAL_ERROR", "internal error");
    owner.cleanup()?;
    Ok(())
}
