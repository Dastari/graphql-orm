//! One composed graph, two ORM subgraphs, one router, real HTTP.
//!
//! The test proves the three things only an end-to-end run can prove: that two
//! ORM subgraphs compose at all, that a generated `@key` resolver answers a
//! router-planned `_entities` fetch in one statement, and that the subgraph —
//! not the router — is what refuses an unscoped caller.
//!
//! The router owns a process-wide Ntex runtime, so it runs on its own thread
//! with its own system while the subgraphs and the client stay on the test's
//! Tokio runtime.

use std::net::{SocketAddr, TcpListener};
use std::time::Duration;

use graphql_orm_router::{RouterConfig, StaticSubgraph};
use serde_json::{Value, json};

const SCOPE_HEADER: &str = "x-caller-scopes";

/// A reading selection that also selects the joined foreign field.
const JOINED_QUERY: &str = "{ readings { edges { node { label zone { name } } } } }";

/// The same selection without the join, used to measure the readings subgraph's
/// own statement cost.
const LOCAL_ONLY_QUERY: &str = "{ readings { edges { node { label } } } }";

struct RunningRouter {
    address: SocketAddr,
    composition_warnings: Vec<String>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RunningRouter {
    fn url(&self) -> String {
        format!("http://{}/graphql", self.address)
    }
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

/// Picks a free loopback port by binding and releasing it.
///
/// The router binds by address rather than accepting a listener, so the port
/// has to be chosen before it starts.
fn free_port() -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("reserve a router port");
    listener.local_addr().expect("reserved router address")
}

/// Starts the router on its own Ntex system thread.
///
/// The preparation result crosses back before the server starts so a
/// composition failure is reported as a composition failure rather than as a
/// connection refusal later.
fn start_router(zones: SocketAddr, readings: SocketAddr) -> Result<RunningRouter, String> {
    let address = free_port();
    let (ready_sender, ready_receiver) = std::sync::mpsc::channel();
    let (shutdown, shutdown_signal) = tokio::sync::oneshot::channel();
    let thread = std::thread::Builder::new()
        .name("federation-e2e-router".to_string())
        .spawn(move || {
            ntex::rt::System::build()
                .name("federation-e2e-router")
                .build(ntex::rt::DefaultRuntime)
                .block_on(async move {
                    let config = RouterConfig::new(address)
                        .allow_anonymous_development(true)
                        .forward_header(SCOPE_HEADER)
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

    let composition_warnings = ready_receiver
        .recv_timeout(Duration::from_secs(30))
        .expect("the router reports its preparation result")?;
    Ok(RunningRouter {
        address,
        composition_warnings,
        shutdown: Some(shutdown),
        thread: Some(thread),
    })
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
        assert!(
            std::time::Instant::now() < deadline,
            "the router became ready within the deadline"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn post(
    client: &reqwest::Client,
    router: &RunningRouter,
    scopes: Option<&str>,
    query: &str,
) -> Value {
    let mut request = client.post(router.url()).json(&json!({ "query": query }));
    if let Some(scopes) = scopes {
        request = request.header(SCOPE_HEADER, scopes);
    }
    let response = request
        .send()
        .await
        .expect("the router answers the request");
    let status = response.status();
    let body = response.text().await.expect("the router returns a body");
    serde_json::from_str(&body).unwrap_or_else(|error| {
        panic!("router returned {status} with a non-JSON body: {error}\n{body}")
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_orm_subgraphs_compose_and_join_through_a_generated_entity_key() {
    let zones = zones_subgraph::start().await;
    let readings = readings_subgraph::start().await;

    // (a) Composition proof. Both subgraphs export `PageInfo`; without
    // `@shareable` on it the composed graph is rejected here.
    let router = match start_router(zones, readings) {
        Ok(router) => router,
        Err(error) => panic!("the composed graph must prepare: {error}"),
    };
    assert!(
        router.composition_warnings.is_empty(),
        "composition warnings: {:?}",
        router.composition_warnings
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .expect("build the test client");
    wait_until_ready(&client, router.address).await;

    // (b) The join resolves: `zone { name }` can only come from the zones
    // subgraph's generated `@key` resolver answering an `_entities` fetch.
    let joined = post(
        &client,
        &router,
        Some(zones_subgraph::READ_SCOPE),
        JOINED_QUERY,
    )
    .await;
    assert!(
        joined.get("errors").is_none(),
        "the scoped join succeeds: {joined}"
    );
    let nodes = joined["data"]["readings"]["edges"]
        .as_array()
        .unwrap_or_else(|| panic!("the composed query returns readings: {joined}"));
    assert_eq!(nodes.len(), 3, "{joined}");
    assert_eq!(nodes[0]["node"]["label"], "inlet");
    assert_eq!(nodes[0]["node"]["zone"]["name"], "north");
    assert_eq!(nodes[1]["node"]["zone"]["name"], "south");
    assert_eq!(nodes[2]["node"]["zone"]["name"], "east");

    // (d) Three joined representations cost the zones subgraph one statement.
    //
    // The ORM statement counter is process-global and both subgraphs share this
    // process, so the zones subgraph's own cost is measured as the difference
    // between a query that never reaches it and the same query with the join
    // added. The counter only observes statements issued through the counted
    // execution helpers, which is exactly the batched loader path the generated
    // `@key` resolver uses; the connection read the referencing subgraph
    // performs is invisible to it, so the baseline below is expected to be
    // zero and the difference is entirely the entity fetch.
    zones_subgraph::reset_statement_count();
    let local_only = post(
        &client,
        &router,
        Some(zones_subgraph::READ_SCOPE),
        LOCAL_ONLY_QUERY,
    )
    .await;
    assert!(
        local_only.get("errors").is_none(),
        "the unjoined query succeeds: {local_only}"
    );
    let readings_only_statements = zones_subgraph::statement_count();

    zones_subgraph::reset_statement_count();
    let joined_again = post(
        &client,
        &router,
        Some(zones_subgraph::READ_SCOPE),
        JOINED_QUERY,
    )
    .await;
    assert!(
        joined_again.get("errors").is_none(),
        "the scoped join succeeds again: {joined_again}"
    );
    let joined_statements = zones_subgraph::statement_count();
    assert_eq!(
        joined_statements - readings_only_statements,
        1,
        "three representations collapse into one zones statement \
         (unjoined: {readings_only_statements}, joined: {joined_statements})"
    );

    // (c) The denial. The router accepted the unscoped request and planned it;
    // the entity fetch is what fails.
    //
    // Hive does not null just the joined field: an errored `_entities` fetch
    // nulls the whole response, so the parent subgraph's own fields do not
    // survive alongside the error. The parent's fields are still served when
    // the same caller asks for them without the join, which is the assertion
    // below.
    let denied = post(&client, &router, None, JOINED_QUERY).await;
    assert!(
        denied["data"].is_null(),
        "PINNED BEHAVIOUR: a denied entity fetch nulls the whole response: {denied}"
    );
    let error = &denied["errors"][0];
    assert_eq!(error["extensions"]["code"], "DOWNSTREAM_SERVICE_ERROR");
    assert_eq!(
        error["extensions"]["service"], "zones",
        "the denial is attributed to the owning subgraph: {denied}"
    );
    assert_eq!(error["path"][0], "_entities", "{denied}");

    let unscoped_parent_only = post(&client, &router, None, LOCAL_ONLY_QUERY).await;
    assert!(
        unscoped_parent_only.get("errors").is_none(),
        "the unscoped caller still reads the referencing subgraph: {unscoped_parent_only}"
    );
    assert_eq!(
        unscoped_parent_only["data"]["readings"]["edges"][0]["node"]["label"], "inlet",
        "{unscoped_parent_only}"
    );

    // (e) The router granted nothing. It was configured with
    // `allow_anonymous_development(true)` and no scope matcher, so it took no
    // authorization decision at all: it forwarded the header and relayed a
    // sanitized downstream failure. Hive replaces the subgraph's own message
    // with "Unexpected error", so the proof that the text originates in the
    // zones subgraph is read from the subgraph directly.
    let direct = client
        .post(format!("http://{zones}/graphql"))
        .json(&json!({
            "query": "{ _entities(representations: [{ __typename: \"Zone\", id: \"zone-north\" }]) \
                     { ... on Zone { name } } }"
        }))
        .send()
        .await
        .expect("the zones subgraph answers a direct request")
        .json::<Value>()
        .await
        .expect("the zones subgraph returns JSON");
    assert_eq!(
        direct["errors"][0]["message"],
        format!(
            "zones subgraph denied the caller: scope `{}` is required",
            zones_subgraph::READ_SCOPE
        ),
        "the denial originates in the subgraph, not the router: {direct}"
    );
}
