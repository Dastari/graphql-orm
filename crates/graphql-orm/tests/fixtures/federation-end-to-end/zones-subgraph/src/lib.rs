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

/// The scopes the router forwarded, as request data the policy can read.
#[derive(Clone, Debug, Default)]
pub struct CallerScopes(pub Vec<String>);

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    backend = "sqlite",
    table = "zones",
    plural = "Zones",
    default_sort = "name ASC",
    read_policy = "zone.read",
    federation_key
)]
pub struct Zone {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,
}

schema_roots! {
    backend: "sqlite",
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
    let database = graphql_orm::db::Database::connect_sqlite("sqlite::memory:")
        .await
        .expect("open the zones fixture database");
    let pool = database.pool();
    graphql_orm::sqlx::query("CREATE TABLE zones (id TEXT PRIMARY KEY, name TEXT NOT NULL)")
        .execute(pool)
        .await
        .expect("create the zones table");
    for (id, name) in [
        ("zone-north", "north"),
        ("zone-south", "south"),
        ("zone-east", "east"),
    ] {
        graphql_orm::sqlx::query("INSERT INTO zones (id, name) VALUES (?1, ?2)")
            .bind(id)
            .bind(name)
            .execute(pool)
            .await
            .expect("seed a zone");
    }
    graphql_orm::db::Database::with_entity_policy(pool.clone(), ScopedEntityPolicy)
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
