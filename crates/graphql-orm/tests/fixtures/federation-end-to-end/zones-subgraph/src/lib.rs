//! The subgraph that owns the `Zone` entity and its resolvable Federation key.
//!
//! Authorization lives here on purpose: the router grants nothing, so a caller
//! without the read scope must be refused by this process even though the
//! router accepted the request anonymously.

use std::net::SocketAddr;
use std::sync::Arc;

use async_graphql::SDLExportOptions;
use graphql_orm::prelude::*;

/// The scope a caller must present for any read of this subgraph.
pub const READ_SCOPE: &str = "zones.read";

/// A zone no caller may read; its absence must look exactly like a missing zone.
pub const HIDDEN_ZONE: &str = "zone-hidden";

/// The scopes the router forwarded, as request data the policy can read.
#[derive(Clone, Debug, Default)]
pub struct CallerScopes(pub Vec<String>);

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "zones",
        plural = "Zones",
        default_sort = "name ASC",
        read_policy = "zone.read",
        federation_key,
        node_id,
        aggregate = true
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "zones",
        plural = "Zones",
        default_sort = "name ASC",
        read_policy = "zone.read",
        federation_key,
        node_id,
        aggregate = true
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_read_only",
        table = "zones",
        plural = "Zones",
        default_sort = "name ASC",
        read_policy = "zone.read",
        federation_key,
        node_id,
        aggregate = true
    )
)]
pub struct Zone {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    /// Served by the router as the fixed `RecordVersion` field as well.
    #[graphql_orm(version = "managed", default = "0")]
    pub row_version: i64,
}

#[cfg(feature = "sqlite")]
schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [Zone],
}
#[cfg(feature = "postgres")]
schema_roots! {
    backend: "postgres",
    query_custom_ops: [],
    entities: [Zone],
}
#[cfg(feature = "mssql")]
schema_roots! {
    backend: "mssql",
    schema_policy: "external_read_only",
    query_custom_ops: [],
    entities: [Zone],
}

/// Entity policy that answers from the caller's forwarded scopes.
///
/// The policy key is non-`None` because the entity declares `read_policy`, so a
/// caller without the scope is denied rather than silently allowed.
#[derive(Clone, Copy, Debug, Default)]
struct ScopedEntityPolicy;

impl graphql_orm::graphql::orm::EntityPolicy for ScopedEntityPolicy {
    fn can_access_entity<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a graphql_orm::db::Database,
        _entity_name: &'static str,
        policy_key: Option<&'static str>,
        _kind: graphql_orm::graphql::orm::EntityAccessKind,
        _surface: graphql_orm::graphql::orm::EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if policy_key.is_none() {
                return Ok(true);
            }
            let holds_scope = ctx
                .and_then(|ctx| ctx.data_opt::<CallerScopes>())
                .is_some_and(|scopes| scopes.0.iter().any(|scope| scope == READ_SCOPE));
            if holds_scope {
                Ok(true)
            } else {
                Err(async_graphql::Error::new(format!(
                    "zones subgraph denied the caller: scope `{READ_SCOPE}` is required"
                )))
            }
        })
    }
}

/// Federation entity-key resolution also checks the loaded row's current scope.
struct ScopedRowPolicy;

impl RowPolicy for ScopedRowPolicy {
    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a graphql_orm::db::Database,
        entity: &'static str,
        policy_key: Option<&'static str>,
        _surface: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(entity == "Zone"
                && policy_key == Some("zone.read")
                && row
                    .downcast_ref::<Zone>()
                    .is_some_and(|zone| zone.id != HIDDEN_ZONE)
                && ctx
                    .and_then(|ctx| ctx.data_opt::<CallerScopes>())
                    .is_some_and(|scopes| scopes.0.iter().any(|scope| scope == READ_SCOPE)))
        })
    }

    fn can_write_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a graphql_orm::db::Database,
        _entity: &'static str,
        _policy_key: Option<&'static str>,
        _surface: EntityAccessSurface,
        _row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
}

/// Resets the ORM's process-global statement counter.
pub fn reset_statement_count() {
    graphql_orm::graphql::orm::reset_query_count();
}

/// Reads the ORM's process-global statement counter.
///
/// The counter is shared by every ORM entity in the process, so a caller that
/// needs a per-subgraph figure must compare two measured runs rather than read
/// this once.
pub fn statement_count() -> usize {
    graphql_orm::graphql::orm::query_count()
}

async fn seeded_database() -> graphql_orm::db::Database {
    let text = fixture_database::text_type();
    let mut statements = vec![format!(
        "CREATE TABLE zones (id {text} NOT NULL PRIMARY KEY, name {text} NOT NULL, row_version BIGINT NOT NULL DEFAULT 0)"
    )];
    for (id, name, version) in [
        ("zone-north", "north", 3),
        ("zone-south", "south", 1),
        ("zone-east", "east", 0),
        (HIDDEN_ZONE, "hidden", 0),
    ] {
        statements.push(format!(
            "INSERT INTO zones (id, name, row_version) VALUES ('{id}', '{name}', {version})"
        ));
    }
    let database = fixture_database::seeded("zones", &statements).await;
    let mut database =
        graphql_orm::db::Database::with_entity_policy(database.pool().clone(), ScopedEntityPolicy);
    database.set_row_policy(ScopedRowPolicy);
    database
}

/// Starts the zones subgraph on loopback and returns its bound address.
pub async fn start() -> SocketAddr {
    let schema = Arc::new(schema_builder(seeded_database().await).finish());
    let sdl = schema.sdl_with_options(SDLExportOptions::new().federation());
    subgraph_http::serve(sdl, move |request| {
        let schema = schema.clone();
        async move { execute(&schema, request).await }
    })
    .await
}

async fn execute(schema: &AppSchema, request: subgraph_http::GraphqlRequest) -> String {
    let scopes = request
        .header("x-caller-scopes")
        .map(|value| {
            value
                .split(',')
                .map(|scope| scope.trim().to_string())
                .filter(|scope| !scope.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let Ok(graphql_request) = serde_json::from_str::<async_graphql::Request>(&request.body) else {
        return r#"{"errors":[{"message":"malformed GraphQL request"}]}"#.to_string();
    };
    // The generated resolvers require an authenticated subject; the caller's
    // authority is expressed by the forwarded scopes, not by this identity.
    let graphql_request = graphql_request
        .data(AuthSubject::new("router-caller"))
        .data(CallerScopes(scopes));
    let response = schema.execute(graphql_request).await;
    serde_json::to_string(&response).expect("serialize the GraphQL response")
}
