//! Denial/absence equivalence at generated relationship boundaries.
use graphql_orm::async_graphql::{Context, SimpleObject};
use graphql_orm::futures::{StreamExt, future::BoxFuture};
use graphql_orm::prelude::*;
use graphql_orm::{
    DefaultBackend,
    graphql::orm::{query_count, reset_query_count},
};
use std::{
    any::Any,
    sync::{Arc, Mutex},
};
#[cfg(feature = "mssql")]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "denial_targets",
        plural = "DenialTargets",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "denial_targets",
        plural = "DenialTargets",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "denial_targets",
        plural = "DenialTargets",
        auth = "none",
        schema_policy = "external_writable"
    )
)]
struct Target {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[filterable(type = "string")]
    label: String,
}

#[derive(
    GraphQLEntity,
    GraphQLOperations,
    GraphQLRelations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "denial_parents",
        plural = "DenialParents",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "denial_parents",
        plural = "DenialParents",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "denial_parents",
        plural = "DenialParents",
        auth = "none",
        schema_policy = "external_writable"
    )
)]
#[graphql(complex)]
struct Parent {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    id: String,
    label: String,
    target_id: String,
    #[graphql(skip)]
    #[graphql_orm(read_policy = "relation.read")]
    #[relation(target = "Target", from = "target_id", to = "id", emit_fk = false)]
    optional: Option<Target>,
    #[graphql(skip)]
    #[graphql_orm(read_policy = "relation.read")]
    #[relation(target = "Target", from = "id", to = "id", multiple, emit_fk = false)]
    children: Vec<Target>,
}
impl BatchLoadEntity for Target {
    fn batch_column() -> &'static str {
        "id"
    }
    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, graphql_orm::sqlx::Error> {
        #[cfg(not(feature = "mssql"))]
        use graphql_orm::sqlx::Row;
        row.try_get("id")
    }
}
// A host-owned non-null wrapper calls the generated nullable resolver.
struct RequiredParent(Parent);
#[async_graphql::Object]
impl RequiredParent {
    async fn id(&self) -> &str {
        &self.0.id
    }
    async fn required(&self, ctx: &Context<'_>) -> async_graphql::Result<Target> {
        self.0
            .optional(ctx)
            .await?
            .ok_or_else(|| async_graphql::Error::new("Relation unavailable"))
    }
}
#[derive(SimpleObject)]
struct RequiredEdge {
    node: RequiredParent,
}
#[derive(SimpleObject)]
struct RequiredConnection {
    edges: Vec<RequiredEdge>,
}
struct RequiredQuery;
#[async_graphql::Object]
impl RequiredQuery {
    async fn parent(
        &self,
        ctx: &Context<'_>,
        id: String,
    ) -> async_graphql::Result<Option<RequiredParent>> {
        Ok(Parent::get(ctx.data_unchecked::<Database>().pool(), &id)
            .await?
            .map(RequiredParent))
    }
    async fn parents(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<RequiredParent>> {
        Ok(self.parent(ctx, "p".into()).await?.into_iter().collect())
    }
    async fn denial_parents(
        &self,
        ctx: &Context<'_>,
        page: Option<PageInput>,
    ) -> async_graphql::Result<RequiredConnection> {
        let _ = page;
        Ok(RequiredConnection {
            edges: self
                .parents(ctx)
                .await?
                .into_iter()
                .map(|node| RequiredEdge { node })
                .collect(),
        })
    }
}
struct ListQuery;
#[async_graphql::Object]
impl ListQuery {
    async fn parents(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Parent>> {
        let db = ctx.data_unchecked::<Database>();
        Ok(Parent::get(db.pool(), &"p".to_string())
            .await?
            .into_iter()
            .collect())
    }
}
schema_roots! { auth: "none", query_custom_ops: [], entities: [Parent, Target], }
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Allow,
    Row,
    Visibility,
    Prefilter,
    Entity,
    Field,
    Error,
}
#[derive(Clone)]
struct Policy(Arc<Mutex<Mode>>);
impl RowPolicy for Policy {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            if entity == "Target"
                && matches!(*self.0.lock().unwrap(), Mode::Visibility | Mode::Prefilter)
            {
                let filter = TargetWhereInput {
                    label: Some(StringFilter {
                        eq: Some("invisible".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                };
                let predicate = ReadPredicate::from_filter::<DefaultBackend, _>(&filter)?;
                Ok(if *self.0.lock().unwrap() == Mode::Prefilter {
                    ReadVisibility::Prefilter(predicate)
                } else {
                    ReadVisibility::Complete(predicate)
                })
            } else {
                Ok(if entity == "Target" {
                    ReadVisibility::CallbackOnly
                } else {
                    ReadVisibility::Unrestricted
                })
            }
        })
    }
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            let mode = *self.0.lock().unwrap();
            if entity == "Target" && mode == Mode::Error {
                return Err(
                    OrmPublicError::new(OrmErrorCode::AuthorizationMisconfigured)
                        .into_graphql_error(),
                );
            }
            Ok(entity != "Target" || mode != Mode::Row)
        })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}
impl EntityPolicy for Policy {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(entity != "Target" || *self.0.lock().unwrap() != Mode::Entity) })
    }
}
impl FieldPolicy for Policy {
    fn can_read_field<'a>(
        &'a self,
        _: &'a Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        key: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(key != Some("relation.read") || *self.0.lock().unwrap() != Mode::Field)
        })
    }
    fn can_write_field<'a>(
        &'a self,
        _: &'a Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}
#[derive(Clone)]
struct Reads(Arc<std::sync::atomic::AtomicUsize>);
impl ReadQueryObserver for Reads {
    fn on_read(&self, _: &str, _: usize) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}
#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires owned disposable database"
)]
async fn denial_matrix() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    let mut db = Database::connect_sqlite("sqlite::memory:").await?;
    #[cfg(feature = "postgres")]
    let mut db = Database::new(owned_postgres::OwnedPostgres::pool("denied-relations", 1).await?);
    #[cfg(feature = "mssql")]
    let mut server = owned_mssql::OwnedSqlServer::start().await?;
    #[cfg(feature = "mssql")]
    let mut db = Database::<MssqlBackend>::builder(
        graphql_orm::db::mssql::MssqlPool::with_max_connections_external_writable(
            graphql_orm::tiberius::Config::from_ado_string(&server.connection_string)?,
            1,
        ),
    )
    .schema_policy(SchemaPolicy::ExternalWritable)
    .build();
    async fn sql(db: &Database, statement: &str) -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(not(feature = "mssql"))]
        graphql_orm::sqlx::query(statement)
            .execute(db.pool())
            .await?;
        #[cfg(feature = "mssql")]
        db.pool().fetch_rows(statement, &[]).await?;
        Ok(())
    }
    for statement in [
        "CREATE TABLE denial_targets (id VARCHAR(64) PRIMARY KEY, label VARCHAR(64) NOT NULL)",
        "CREATE TABLE denial_parents (id VARCHAR(64) PRIMARY KEY, label VARCHAR(64) NOT NULL, target_id VARCHAR(64) NOT NULL)",
        "INSERT INTO denial_parents VALUES ('p','parent','p')",
    ] {
        sql(&db, statement).await?;
    }
    db = db.with_authorized_scan_config(AuthorizedScanConfig::new([17; 32], 8, 64));
    let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    db = db.with_read_query_observer(Reads(reads.clone()));
    let mode = Arc::new(Mutex::new(Mode::Allow));
    db.set_field_policy(Policy(mode.clone()));
    db.set_row_policy(Policy(mode.clone()));
    db.set_entity_policy(Policy(mode.clone()));
    let schema = schema_builder(db.clone()).finish();
    let required_schema = async_graphql::Schema::build(
        RequiredQuery,
        async_graphql::EmptyMutation,
        async_graphql::EmptySubscription,
    )
    .data(db.clone())
    .finish();
    let list_schema = async_graphql::Schema::build(
        ListQuery,
        async_graphql::EmptyMutation,
        async_graphql::EmptySubscription,
    )
    .data(db.clone())
    .finish();
    assert!(schema.sdl().contains("optional: Target"));
    assert!(!schema.sdl().contains("optional: Target!"));
    assert!(required_schema.sdl().contains("required: Target!"));
    sql(&db, "INSERT INTO denial_targets VALUES ('p','target')").await?;
    let allowed = schema
        .execute("{ parent(id: \"p\") { optional { id } children { edges { node { id } } } } }")
        .await;
    assert!(allowed.errors.is_empty(), "{allowed:?}");
    let allowed = allowed.data.into_json()?;
    assert_eq!(allowed["parent"]["optional"]["id"], "p");
    assert_eq!(allowed["parent"]["children"]["edges"][0]["node"]["id"], "p");
    sql(&db, "DELETE FROM denial_targets").await?;
    // One test owns the process-global counter for this binary.
    for mechanism in [
        Mode::Row,
        Mode::Visibility,
        Mode::Prefilter,
        Mode::Entity,
        Mode::Field,
        Mode::Error,
    ] {
        for relation in [
            "optional { id }",
            "required { id }",
            "children { edges { node { id } } pageInfo { totalCount } }",
        ] {
            for (surface, query) in [
                (
                    "changed",
                    format!("subscription {{ parentChanged {{ parent {{ id {relation} }} }} }}"),
                ),
                (
                    "single",
                    format!("{{ parent(id: \"p\") {{ id {relation} }} }}"),
                ),
                ("list", format!("{{ parents {{ id {relation} }} }}")),
                (
                    "connection",
                    format!(
                        "{{ denialParents(page: {{ limit: 1 }}) {{ edges {{ node {{ id {relation} }} }} }} }}"
                    ),
                ),
            ] {
                // Generated Changed payloads have no handwritten non-null wrapper.
                if surface == "changed" && relation.starts_with("required") {
                    continue;
                }
                *mode.lock().unwrap() = mechanism;
                sql(&db, "INSERT INTO denial_targets VALUES ('p','target')").await?;
                reset_query_count();
                reads.store(0, std::sync::atomic::Ordering::Relaxed);
                let denied = if relation.starts_with("required") && surface != "changed" {
                    required_schema.execute(query.clone()).await
                } else if surface == "list" {
                    list_schema.execute(query.clone()).await
                } else if surface == "changed" {
                    schema
                        .execute_stream(query.clone())
                        .next()
                        .await
                        .expect("admission response")
                } else {
                    schema.execute(query.clone()).await
                };
                let denied_count = (
                    query_count(),
                    reads.load(std::sync::atomic::Ordering::Relaxed),
                );
                sql(&db, "DELETE FROM denial_targets").await?;
                reset_query_count();
                reads.store(0, std::sync::atomic::Ordering::Relaxed);
                let absent = if relation.starts_with("required") && surface != "changed" {
                    required_schema.execute(query.clone()).await
                } else if surface == "list" {
                    list_schema.execute(query.clone()).await
                } else if surface == "changed" {
                    schema
                        .execute_stream(query.clone())
                        .next()
                        .await
                        .expect("admission response")
                } else {
                    schema.execute(query.clone()).await
                };
                let absent_count = (
                    query_count(),
                    reads.load(std::sync::atomic::Ordering::Relaxed),
                );
                println!(
                    "{mechanism:?}/{relation}/{surface}: {} queries={denied_count:?}/{absent_count:?}",
                    serde_json::to_string(&denied)?
                );
                if !denied.errors.is_empty() {
                    let error = &denied.errors[0];
                    let code = error
                        .extensions
                        .as_ref()
                        .and_then(|extensions| extensions.get("code"));
                    let expected = if surface == "changed" {
                        Some("INVALID_INPUT")
                    } else if mechanism == Mode::Error {
                        Some("AUTHORIZATION_MISCONFIGURED")
                    } else if matches!(mechanism, Mode::Entity | Mode::Field) {
                        Some("FORBIDDEN")
                    } else {
                        None
                    };
                    assert_eq!(
                        code,
                        expected
                            .map(|value| async_graphql::Value::String(value.into()))
                            .as_ref()
                    );
                    let path = serde_json::to_value(&error.path)?;
                    let name = relation.split_whitespace().next().unwrap();
                    let expected_path = match surface {
                        "changed" => serde_json::json!(["parentChanged"]),
                        "single" => serde_json::json!(["parent", name]),
                        "list" => serde_json::json!(["parents", 0, name]),
                        _ => serde_json::json!(["denialParents", "edges", 0, "node", name]),
                    };
                    assert_eq!(path, expected_path);
                }
                if mechanism != Mode::Error {
                    assert_eq!(
                        serde_json::to_value(&denied)?,
                        serde_json::to_value(&absent)?,
                        "{mechanism:?}/{surface}"
                    );
                    assert_eq!(denied_count, absent_count);
                }
                if matches!(mechanism, Mode::Row | Mode::Visibility | Mode::Prefilter)
                    && !relation.starts_with("required")
                    && surface != "changed"
                {
                    assert!(denied.errors.is_empty(), "{denied:?}");
                } else {
                    assert!(!denied.errors.is_empty(), "{denied:?}");
                }
            }
        }
    }
    #[cfg(feature = "mssql")]
    server.cleanup()?;
    Ok(())
}
