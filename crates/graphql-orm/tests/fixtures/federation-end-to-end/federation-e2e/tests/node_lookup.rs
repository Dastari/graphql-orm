//! Global `node(NodeId)` and `nodes(NodeIds)` lookup through the router.
//!
//! Two real ORM subgraphs declare `node_id`; neither declares `interface Node`.
//! The router discovers their `@key(fields: "NodeId")` types, composes its own
//! `Node` subgraph and answers it in-process, and the planner fetches every
//! requested field from the owning subgraph. The subgraphs run on the backend
//! the fixture's `sqlite`, `postgres` or `mssql` feature selects; PostgreSQL
//! and SQL Server databases are test-owned containers.

use std::net::{SocketAddr, TcpListener};
use std::time::Duration;

use base64::Engine as _;
use graphql_orm_router::{NodeLookupConfig, RouterConfig, StaticSubgraph};
use serde_json::{Value, json};

const SCOPE_HEADER: &str = "x-caller-scopes";

/// The client-side encoding: base64url, no padding, of compact JSON.
fn node_id(parts: Value) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&parts).expect("node id parts"))
}

struct RunningRouter {
    address: SocketAddr,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for RunningRouter {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn start_router(zones: SocketAddr, readings: SocketAddr, max_nodes: usize) -> RunningRouter {
    let address = TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .expect("reserve a router port");
    let (ready_sender, ready_receiver) = std::sync::mpsc::channel();
    let (shutdown, shutdown_signal) = tokio::sync::oneshot::channel();
    let thread = std::thread::Builder::new()
        .name("node-lookup-router".to_string())
        .spawn(move || {
            ntex::rt::System::build()
                .name("node-lookup-router")
                .build(ntex::rt::DefaultRuntime)
                .block_on(async move {
                    let config = RouterConfig::new(address)
                        .allow_anonymous_development(true)
                        .forward_header(SCOPE_HEADER)
                        .with_node_lookup(NodeLookupConfig::new().with_max_nodes(max_nodes))
                        .with_subgraph(StaticSubgraph::new(
                            "zones",
                            format!("http://{zones}/graphql"),
                            format!("http://{zones}/sdl"),
                        ))
                        .with_subgraph(StaticSubgraph::new(
                            "readings",
                            format!("http://{readings}/graphql"),
                            format!("http://{readings}/sdl"),
                        ));
                    let prepared = match config.prepare().await {
                        Ok(prepared) => prepared,
                        Err(error) => {
                            let _ = ready_sender.send(Err(error.to_string()));
                            return;
                        }
                    };
                    let _ = ready_sender.send(Ok(prepared.composition_warnings().to_vec()));
                    prepared
                        .run_until_shutdown(async move {
                            let _ = shutdown_signal.await;
                        })
                        .await
                        .expect("the router runs until it is asked to stop");
                });
        })
        .expect("spawn the router thread");
    let warnings = ready_receiver
        .recv_timeout(Duration::from_secs(60))
        .expect("the router reports its preparation result")
        .unwrap_or_else(|error| panic!("the node lookup graph must prepare: {error}"));
    assert!(warnings.is_empty(), "composition warnings: {warnings:?}");
    RunningRouter {
        address,
        shutdown: Some(shutdown),
        thread: Some(thread),
    }
}

async fn wait_until_ready(client: &reqwest::Client, address: SocketAddr) {
    let url = format!("http://{address}/readiness");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(response) = client.get(&url).send().await
            && response.status().is_success()
        {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "router readiness");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn post(
    client: &reqwest::Client,
    router: &RunningRouter,
    query: &str,
    variables: Value,
) -> Value {
    let response = client
        .post(format!("http://{}/graphql", router.address))
        .header(SCOPE_HEADER, zones_subgraph::READ_SCOPE)
        .json(&json!({ "query": query, "variables": variables }))
        .send()
        .await
        .expect("the router answers");
    let body = response.text().await.expect("a response body");
    serde_json::from_str(&body).unwrap_or_else(|error| panic!("non-JSON body {error}: {body}"))
}

const SELECTION: &str = "__typename NodeId \
    ... on Zone { id name } \
    ... on Sensor { id name } \
    ... on Calibration { sensorId slot label }";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn node_and_nodes_resolve_each_type_from_its_owning_subgraph() {
    let _cleanup = fixture_database::CleanupGuard;
    println!("node lookup backend: {}", fixture_database::BACKEND);
    let zones = zones_subgraph::start().await;
    let readings = readings_subgraph::start().await;
    // Hive installs a process-wide tracing subscriber, so one router per process.
    let router = start_router(zones, readings, 8);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .expect("build the test client");
    wait_until_ready(&client, router.address).await;

    // A Zone and a Sensor share the key value `zone-north`; their NodeIds,
    // which the owning subgraphs generate, differ only by type.
    let zone_north = node_id(json!(["Zone", "zone-north"]));
    let sensor_north = node_id(json!(["Sensor", "zone-north"]));
    let calibration = node_id(json!(["Calibration", "zone-north", 2]));
    let generated = post(
        &client,
        &router,
        r#"{ zone(id: "zone-north") { NodeId } sensor(id: "zone-north") { NodeId }
             calibration(sensorId: "zone-north", slot: 2) { NodeId } }"#,
        json!({}),
    )
    .await;
    assert!(generated.get("errors").is_none(), "{generated}");
    assert_eq!(generated["data"]["zone"]["NodeId"], zone_north.as_str());
    assert_eq!(generated["data"]["sensor"]["NodeId"], sensor_north.as_str());
    assert_eq!(
        generated["data"]["calibration"]["NodeId"],
        calibration.as_str()
    );

    let single = post(
        &client,
        &router,
        &format!("query($id: ID!) {{ node(NodeId: $id) {{ {SELECTION} }} }}"),
        json!({ "id": calibration }),
    )
    .await;
    assert!(single.get("errors").is_none(), "{single}");
    assert_eq!(
        single["data"]["node"],
        json!({
            "__typename": "Calibration",
            "NodeId": calibration,
            "sensorId": "zone-north",
            "slot": 2,
            "label": "high",
        })
    );

    let hidden = node_id(json!(["Zone", zones_subgraph::HIDDEN_ZONE]));
    let missing = node_id(json!(["Zone", "zone-missing"]));
    // A Zone's identity is not a Sensor's, and the reverse.
    let wrong_type = node_id(json!(["Zone", "sensor-2"]));
    let unknown_type = node_id(json!(["Reading", "reading-1"]));
    let ids = json!([
        zone_north,
        sensor_north,
        calibration,
        hidden,
        missing,
        wrong_type,
        unknown_type,
        "not-a-node-id",
    ]);
    let many = post(
        &client,
        &router,
        &format!("query($ids: [ID!]!) {{ nodes(NodeIds: $ids) {{ {SELECTION} }} }}"),
        json!({ "ids": ids }),
    )
    .await;
    assert!(many.get("errors").is_none(), "{many}");
    let nodes = many["data"]["nodes"].as_array().expect("nodes list");
    assert_eq!(nodes[0]["__typename"], "Zone");
    assert_eq!(nodes[0]["name"], "north");
    assert_eq!(nodes[1]["__typename"], "Sensor");
    assert_eq!(nodes[1]["name"], "colliding sensor");
    assert_eq!(nodes[2]["label"], "high");
    // Denied, missing, mismatched, unknown and malformed are all plain null.
    for (index, node) in nodes.iter().enumerate().skip(3) {
        assert!(node.is_null(), "item {index} must be null: {many}");
    }
    assert_eq!(nodes.len(), 8);

    // N NodeIds of one type are one `_entities` fetch and one statement.
    let sensors = json!([
        sensor_north,
        node_id(json!(["Sensor", "sensor-2"])),
        node_id(json!(["Sensor", "sensor-3"])),
    ]);
    zones_subgraph::reset_statement_count();
    let batched = post(
        &client,
        &router,
        "query($ids: [ID!]!) { nodes(NodeIds: $ids) { ... on Sensor { name } } }",
        json!({ "ids": sensors }),
    )
    .await;
    let statements = zones_subgraph::statement_count();
    assert!(batched.get("errors").is_none(), "{batched}");
    assert_eq!(batched["data"]["nodes"][2]["name"], "third sensor");
    assert_eq!(
        statements, 1,
        "three Sensor NodeIds resolve in one statement"
    );

    // The router enforces its configured bound before any subgraph fetch.
    let over = post(
        &client,
        &router,
        "query($ids: [ID!]!) { nodes(NodeIds: $ids) { NodeId } }",
        json!({ "ids": ["a", "b", "c", "d", "e", "f", "g", "h", "i"] }),
    )
    .await;
    assert!(over["data"].is_null(), "{over}");
    assert_eq!(
        over["errors"][0]["extensions"]["code"], "INVALID_INPUT",
        "{over}"
    );
}
