//! The subgraph that owns `Reading` and only references the foreign `Zone`.
//!
//! It declares no resolvable key of its own, so `federation: true` is what
//! makes it serve `_service` and export a federation SDL at all. The `zone`
//! field hands the router an unresolvable reference; the router turns that into
//! an `_entities` fetch against the owning subgraph.

use std::net::SocketAddr;
use std::sync::Arc;

use async_graphql::{ComplexObject, SDLExportOptions, SimpleObject};
use graphql_orm::prelude::*;

/// A reference to an entity owned by another subgraph.
#[derive(SimpleObject, Clone, Debug)]
#[graphql(name = "Zone", unresolvable = "id")]
pub struct ZoneReference {
    pub id: String,
}

#[derive(
    GraphQLEntity,
    GraphQLOperations,
    SimpleObject,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
    PartialEq,
)]
#[graphql(complex)]
#[graphql_entity(
    backend = "sqlite",
    table = "readings",
    plural = "Readings",
    default_sort = "id ASC"
)]
pub struct Reading {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub zone_id: String,

    pub label: String,
}

#[ComplexObject]
impl Reading {
    /// The join the composed graph resolves through the owning subgraph.
    async fn zone(&self) -> ZoneReference {
        ZoneReference {
            id: self.zone_id.clone(),
        }
    }
}

schema_roots! {
    backend: "sqlite",
    federation: true,
    query_custom_ops: [],
    entities: [Reading],
}

async fn seeded_database() -> graphql_orm::db::Database {
    let database = graphql_orm::db::Database::connect_sqlite("sqlite::memory:")
        .await
        .expect("open the readings fixture database");
    let pool = database.pool();
    graphql_orm::sqlx::query(
        "CREATE TABLE readings (id TEXT PRIMARY KEY, zone_id TEXT NOT NULL, label TEXT NOT NULL)",
    )
    .execute(pool)
    .await
    .expect("create the readings table");
    for (id, zone_id, label) in [
        ("reading-1", "zone-north", "inlet"),
        ("reading-2", "zone-south", "outlet"),
        ("reading-3", "zone-east", "ambient"),
    ] {
        graphql_orm::sqlx::query("INSERT INTO readings (id, zone_id, label) VALUES (?1, ?2, ?3)")
            .bind(id)
            .bind(zone_id)
            .bind(label)
            .execute(pool)
            .await
            .expect("seed a reading");
    }
    database
}

/// Starts the readings subgraph on loopback and returns its bound address.
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
    let Ok(graphql_request) = serde_json::from_str::<async_graphql::Request>(&request.body) else {
        return r#"{"errors":[{"message":"malformed GraphQL request"}]}"#.to_string();
    };
    // This subgraph guards nothing: the denial under test must come from the
    // subgraph that owns the entity, not from the one that references it.
    let graphql_request = graphql_request.data(AuthSubject::new("router-caller"));
    let response = schema.execute(graphql_request).await;
    serde_json::to_string(&response).expect("serialize the GraphQL response")
}
