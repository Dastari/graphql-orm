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
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "readings",
        plural = "Readings",
        default_sort = "id ASC",
        aggregate = true
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "readings",
        plural = "Readings",
        default_sort = "id ASC",
        aggregate = true
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_read_only",
        table = "readings",
        plural = "Readings",
        default_sort = "id ASC",
        aggregate = true
    )
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
    /// A non-null reference to the foreign entity.
    ///
    /// Nullability here is the referencing subgraph's choice and it decides
    /// what a refused or failed join costs the caller. A non-null field cannot
    /// hold the null that a failed `_entities` fetch produces, so the null
    /// propagates to the nearest nullable ancestor exactly as the GraphQL spec
    /// requires. The fixture keeps both shapes so the difference is pinned.
    async fn zone(&self) -> ZoneReference {
        ZoneReference {
            id: self.zone_id.clone(),
        }
    }

    /// A nullable reference to the same foreign entity.
    ///
    /// The value is always `Some`; the option expresses that the *join* may
    /// fail, not that the local column may be absent.
    async fn optional_zone(&self) -> Option<ZoneReference> {
        Some(ZoneReference {
            id: self.zone_id.clone(),
        })
    }
}

/// A sensor whose `id` deliberately collides with a `Zone` id in the zones
/// subgraph: the type in the `NodeId` keeps the two records apart.
#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(backend = "sqlite", table = "sensors", plural = "Sensors", node_id)
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(backend = "postgres", table = "sensors", plural = "Sensors", node_id)
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_read_only",
        table = "sensors",
        plural = "Sensors",
        node_id
    )
)]
pub struct Sensor {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,

    pub name: String,
}

/// A composite-key entity: its `NodeId` encodes both key parts in order.
#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "calibrations",
        plural = "Calibrations",
        node_id
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "calibrations",
        plural = "Calibrations",
        node_id
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_read_only",
        table = "calibrations",
        plural = "Calibrations",
        node_id
    )
)]
pub struct Calibration {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    pub sensor_id: String,

    #[primary_key]
    #[filterable(type = "number")]
    pub slot: i64,

    pub label: String,
}

#[cfg(feature = "sqlite")]
schema_roots! {
    backend: "sqlite",
    federation: true,
    query_custom_ops: [],
    entities: [Reading, Sensor, Calibration],
}
#[cfg(feature = "postgres")]
schema_roots! {
    backend: "postgres",
    federation: true,
    query_custom_ops: [],
    entities: [Reading, Sensor, Calibration],
}
#[cfg(feature = "mssql")]
schema_roots! {
    backend: "mssql",
    schema_policy: "external_read_only",
    federation: true,
    query_custom_ops: [],
    entities: [Reading, Sensor, Calibration],
}

async fn seeded_database() -> graphql_orm::db::Database {
    let text = fixture_database::text_type();
    let mut statements = vec![
        format!(
            "CREATE TABLE readings (id {text} NOT NULL PRIMARY KEY, zone_id {text} NOT NULL, label {text} NOT NULL)"
        ),
        format!("CREATE TABLE sensors (id {text} NOT NULL PRIMARY KEY, name {text} NOT NULL)"),
        format!(
            "CREATE TABLE calibrations (sensor_id {text} NOT NULL, slot BIGINT NOT NULL, label {text} NOT NULL, PRIMARY KEY (sensor_id, slot))"
        ),
    ];
    for (id, zone_id, label) in [
        ("reading-1", "zone-north", "inlet"),
        ("reading-2", "zone-south", "outlet"),
        ("reading-3", "zone-east", "ambient"),
    ] {
        statements.push(format!(
            "INSERT INTO readings (id, zone_id, label) VALUES ('{id}', '{zone_id}', '{label}')"
        ));
    }
    for (id, name) in [
        ("zone-north", "colliding sensor"),
        ("sensor-2", "second sensor"),
        ("sensor-3", "third sensor"),
    ] {
        statements.push(format!(
            "INSERT INTO sensors (id, name) VALUES ('{id}', '{name}')"
        ));
    }
    for (sensor_id, slot, label) in [
        ("zone-north", 1, "low"),
        ("zone-north", 2, "high"),
        ("sensor-2", 1, "only"),
    ] {
        statements.push(format!(
            "INSERT INTO calibrations (sensor_id, slot, label) VALUES ('{sensor_id}', {slot}, '{label}')"
        ));
    }
    fixture_database::seeded("readings", &statements).await
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
