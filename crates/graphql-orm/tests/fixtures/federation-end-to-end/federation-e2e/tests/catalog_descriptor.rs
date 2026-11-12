#![cfg(feature = "sqlite")]
//! An authenticated router binds every subgraph descriptor to the composed
//! graph at startup. A generated catalogue exported unchanged must pass that
//! binding for managed-version, compare-and-swap, composite-key and keyset
//! entities, and a descriptor that drops one SDL argument must not.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_graphql::SDLExportOptions;
use graphql_orm::prelude::*;
use graphql_orm_router::{
    AuthenticatedPrincipal, AuthenticationError, AuthenticationProvider, RouterConfig,
    StaticSubgraph,
};
use graphql_orm_router_protocol::{
    CapabilitySet, Fingerprint, OperationDescriptor, SubgraphDescriptorBuilder,
};

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "sqlite",
    table = "catalog_managed_records",
    plural = "CatalogManagedRecords",
    keyset = "created_at desc, id asc",
    upsert = "slug",
    aggregate = true
)]
pub struct CatalogManagedRecord {
    #[primary_key]
    #[sortable]
    pub id: String,
    #[unique]
    #[filterable(type = "string")]
    pub slug: String,
    #[filterable(type = "string")]
    pub title: String,
    #[filterable(type = "number")]
    #[sortable]
    pub created_at: i64,
    #[graphql_orm(version = "managed", default = "0")]
    pub row_version: i64,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "sqlite",
    table = "catalog_cas_records",
    plural = "CatalogCasRecords"
)]
pub struct CatalogCasRecord {
    #[primary_key]
    pub id: String,
    #[filterable(type = "string")]
    #[sortable]
    pub label: String,
    #[graphql_orm(version, default = "0")]
    pub revision: i64,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "sqlite",
    table = "catalog_composite_records",
    plural = "CatalogCompositeRecords",
    repository_mutations = true,
    keyset = "tenant_id asc, code asc",
    default_sort = "tenant_id ASC, code ASC"
)]
pub struct CatalogCompositeRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    pub tenant_id: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    pub code: String,
    #[filterable(type = "string")]
    pub value: String,
}

schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [CatalogManagedRecord, CatalogCasRecord, CatalogCompositeRecord],
}

const SUBGRAPH: &str = "catalog";

/// The one bearer token the fixture accepts.
const TOKEN: &str = "fixture-token";

/// Authenticates [`TOKEN`] as a caller holding ordinary scopes.
#[derive(Debug)]
struct FixtureToken;

impl AuthenticationProvider for FixtureToken {
    fn authenticate_bearer(
        &self,
        token: &str,
    ) -> Result<AuthenticatedPrincipal, AuthenticationError> {
        if token != TOKEN {
            return Err(AuthenticationError::invalid_credential(
                "unknown fixture token",
            ));
        }
        AuthenticatedPrincipal::new("fixture-caller", ["catalog.read", "catalog.write"], None)
    }
}

async fn federation_sdl() -> String {
    let pool = graphql_orm::sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("SQLite test pool");
    // The catalogue describes the generated subscriptions, so the subgraph
    // must publish them to the router.
    schema_builder(graphql_orm::db::Database::new(pool))
        .enable_subscription_in_federation()
        .finish()
        .sdl_with_options(SDLExportOptions::new().federation())
}

fn descriptor_json(address: SocketAddr, sdl: &str, operations: Vec<OperationDescriptor>) -> String {
    let mut builder = SubgraphDescriptorBuilder::new(
        "catalog-service",
        SUBGRAPH,
        format!("http://{address}/graphql"),
        format!("http://{address}/sdl"),
        Fingerprint::sha256(sdl),
    )
    .expect("descriptor identity is valid")
    .capabilities(CapabilitySet {
        authorization_metadata: true,
        schema_fingerprints: true,
        ..CapabilitySet::default()
    })
    .require_semantic("authorizationMetadata");
    for operation in operations {
        builder = builder.operation(operation);
    }
    serde_json::to_string(&builder.build().expect("descriptor validates"))
        .expect("descriptor serializes")
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

/// Serves the subgraph with the generated descriptor, after `edit`, behind an
/// authenticated router, and returns the router once it has bound every
/// descriptor to the composed graph. With `serve`, the router also accepts
/// requests; a running router installs process-global tracing, so at most one
/// test in this binary serves.
async fn start_router(
    edit: impl FnOnce(&mut Vec<OperationDescriptor>),
    serve: bool,
) -> Result<RunningRouter, String> {
    let sdl = federation_sdl().await;
    let mut operations = graphql_orm_operation_catalog()
        .router_protocol_operations()
        .expect("generated catalogue exports protocol operations");
    edit(&mut operations);
    let address = subgraph_http::serve_with_descriptor(
        sdl.clone(),
        |address| descriptor_json(address, &sdl, operations),
        // A refusal must come from the router; this subgraph would answer.
        |_| async { r#"{"data":{}}"#.to_owned() },
    )
    .await;

    let router_address = std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .expect("reserve a router port");
    let (ready, ready_receiver) = std::sync::mpsc::channel();
    let (shutdown, shutdown_signal) = tokio::sync::oneshot::channel();
    let thread = std::thread::spawn(move || {
        ntex::rt::System::build()
            .name("catalog-descriptor-router")
            .build(ntex::rt::DefaultRuntime)
            .block_on(async move {
                let config = RouterConfig::new(router_address)
                    .with_authentication_provider(Arc::new(FixtureToken))
                    .with_subgraph(
                        StaticSubgraph::new(
                            SUBGRAPH,
                            format!("http://{address}/graphql"),
                            format!("http://{address}/sdl"),
                        )
                        .with_protocol_url(format!("http://{address}/descriptor")),
                    );
                let prepared = match config.prepare().await {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        let _ = ready.send(Err(error.to_string()));
                        return;
                    }
                };
                let _ = ready.send(Ok(()));
                if !serve {
                    return;
                }
                prepared
                    .run_until_shutdown(async move {
                        let _ = shutdown_signal.await;
                    })
                    .await
                    .expect("the router runs until it is asked to stop");
            });
    });
    let mut router = RunningRouter {
        address: router_address,
        shutdown: Some(shutdown),
        thread: Some(thread),
    };
    let prepared = tokio::task::spawn_blocking(move || {
        ready_receiver
            .recv_timeout(Duration::from_secs(30))
            .expect("the router reports its preparation result")
    })
    .await
    .expect("join the router preparation wait");
    if let Err(error) = prepared {
        // The router thread has already returned.
        router.shutdown = None;
        return Err(error);
    }
    Ok(router)
}

async fn wait_until_ready(client: &reqwest::Client, address: SocketAddr) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(response) = client
            .get(format!("http://{address}/readiness"))
            .send()
            .await
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

#[tokio::test(flavor = "multi_thread")]
async fn authenticated_router_accepts_the_generated_descriptor_unchanged() {
    start_router(|_| {}, false)
        .await
        .expect("an authenticated router binds the generated descriptor");
}

#[tokio::test(flavor = "multi_thread")]
async fn authenticated_router_refuses_a_descriptor_missing_an_sdl_argument() {
    let error = start_router(
        |operations| {
            let update = operations
                .iter_mut()
                .find(|operation| operation.field_name == "updateCatalogManagedRecord")
                .expect("managed update is described");
            let before = update.arguments.len();
            update
                .arguments
                .retain(|argument| argument.name != "expectedVersion");
            assert_eq!(
                update.arguments.len() + 1,
                before,
                "expectedVersion is described"
            );
        },
        false,
    )
    .await
    .err()
    .expect("a stale descriptor must not bind");
    assert!(
        error.contains("authorization argument metadata is stale"),
        "unexpected refusal: {error}"
    );
}

/// A router may exempt only the planner-owned `_entities` and `_service`
/// fields from authorization binding. The generated descriptor still binds,
/// composite-key placeholder included, when nothing else is exempt.
#[tokio::test]
async fn generated_descriptor_describes_every_root_field_but_the_planner_owned_ones() {
    use async_graphql::parser::{
        parse_schema,
        types::{TypeKind, TypeSystemDefinition},
    };
    use graphql_orm_router_protocol::RootOperationType;
    use std::collections::BTreeSet;

    let sdl = federation_sdl().await;
    let mut expected = BTreeSet::new();
    for definition in parse_schema(&sdl).expect("SDL parses").definitions {
        let TypeSystemDefinition::Type(definition) = definition else {
            continue;
        };
        let root = match definition.node.name.node.as_str() {
            "Query" => RootOperationType::Query,
            "Mutation" => RootOperationType::Mutation,
            "Subscription" => RootOperationType::Subscription,
            _ => continue,
        };
        let TypeKind::Object(object) = definition.node.kind else {
            continue;
        };
        for field in object.fields {
            let name = field.node.name.node.to_string();
            if !matches!(name.as_str(), "_entities" | "_service") {
                expected.insert((root, name));
            }
        }
    }
    let described = graphql_orm_operation_catalog()
        .router_protocol_operations()
        .expect("protocol export")
        .into_iter()
        .map(|operation| (operation.root_type, operation.field_name))
        .collect::<BTreeSet<_>>();
    assert!(expected.contains(&(
        RootOperationType::Mutation,
        "gomCatalogCompositeRecordWritesUnavailable".to_owned()
    )));
    assert_eq!(described, expected);
}

/// The composite-key placeholder is described with a requirement no caller
/// can hold, so the router refuses it before the subgraph sees it.
#[tokio::test(flavor = "multi_thread")]
async fn authenticated_router_refuses_the_composite_key_placeholder() {
    assert!(
        AuthenticatedPrincipal::new(
            "fixture-caller",
            [graphql_orm::graphql::orm::GRAPHQL_UNSATISFIABLE_SCOPE],
            None
        )
        .is_err(),
        "no principal can hold the unsatisfiable scope"
    );
    let router = start_router(|_| {}, true)
        .await
        .expect("an authenticated router binds the generated descriptor");
    let client = reqwest::Client::new();
    wait_until_ready(&client, router.address).await;
    let response: serde_json::Value = client
        .post(format!("http://{}/graphql", router.address))
        .bearer_auth(TOKEN)
        .json(&serde_json::json!({
            "query": "mutation { gomCatalogCompositeRecordWritesUnavailable }"
        }))
        .send()
        .await
        .expect("the router answers")
        .json()
        .await
        .expect("the router answers with JSON");
    assert_eq!(
        response["errors"][0]["extensions"]["code"], "FORBIDDEN",
        "{response}"
    );
    assert_eq!(
        response["errors"][0]["message"], "required scope is missing",
        "{response}"
    );
}
