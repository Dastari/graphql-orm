//! Loopback HTTP regression coverage for the private Hive integration seam.
//!
//! The parent test runs the router in a separate test process. That is
//! intentional: Hive owns a process-wide async runtime and server lifecycle.
//! The child process only starts a router; this module's parent process owns
//! the disposable loopback subgraphs and all assertions.
//!
//! The same fixture also proves the public `graphql-transport-ws` endpoint:
//! a streaming upstream event reaches a client, a retained schema ends its
//! subscriptions on replacement, and a reconnect selects the new schema.

use std::{
    env, fs,
    io::{self, BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use graphql_orm_router_protocol::{
    AdvertisedEndpoint, ArgumentDescriptor, AuthorizationRequirement, CapabilitySet,
    DescriptorFingerprints, Fingerprint, GraphqlEndpoints, OperationDescriptor, ProtocolVersion,
    RootOperationType, SchemaAdvertisement, ScopeSet, ScopeTemplate, SubgraphDescriptor,
    SubgraphId, SubgraphIdentity, SubgraphName,
};
use hive_router::{
    PluginRegistry,
    plugins::{
        hooks::{
            on_graphql_analysis::{
                OnGraphqlAnalysisHookPayload, OnGraphqlAnalysisHookResult, Selection,
            },
            on_http_request::{OnHttpRequestHookFuture, OnHttpRequestHookPayload},
            on_plugin_init::{OnPluginInitPayload, OnPluginInitResult},
        },
        plugin_trait::{RouterPlugin, StartHookPayload},
    },
};
use serde_json::{Value, json};
use sha1::{Digest as _, Sha1};
use url::Url;

use crate::{
    AdminConfig, AuthenticatedPrincipal, AuthenticationError, AuthenticationProvider,
    NetworkPolicy, RequestLimits, RouterConfig, RouterErrorKind, ScopeMatcher, StaticSubgraph,
    SubscriptionConfig, TrustedSubgraph,
};

use super::{CandidateSubgraph, GraphStore};

mod byte_routes;

const CHILD_ENV: &str = "GRAPHQL_ORM_ROUTER_WIRE_CHILD";
const PRODUCTS_ENDPOINT_ENV: &str = "GRAPHQL_ORM_ROUTER_WIRE_PRODUCTS_ENDPOINT";
const REVIEWS_ENDPOINT_ENV: &str = "GRAPHQL_ORM_ROUTER_WIRE_REVIEWS_ENDPOINT";
const PRODUCTS_SDL_ENDPOINT_ENV: &str = "GRAPHQL_ORM_ROUTER_WIRE_PRODUCTS_SDL_ENDPOINT";
const REVIEWS_SDL_ENDPOINT_ENV: &str = "GRAPHQL_ORM_ROUTER_WIRE_REVIEWS_SDL_ENDPOINT";
const PRODUCTS_PROTOCOL_ENDPOINT_ENV: &str = "GRAPHQL_ORM_ROUTER_WIRE_PRODUCTS_PROTOCOL_ENDPOINT";
const REVIEWS_PROTOCOL_ENDPOINT_ENV: &str = "GRAPHQL_ORM_ROUTER_WIRE_REVIEWS_PROTOCOL_ENDPOINT";
const PROMETHEUS_LISTENER_ENV: &str = "GRAPHQL_ORM_ROUTER_WIRE_PROMETHEUS_LISTENER";
const TEST_TIMEOUT: Duration = Duration::from_secs(10);

const PRODUCTS_V1: &str = r#"
    extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@key"])
    type Query { product(id: ID!): Product }
    type Mutation { renameProduct(id: ID!, name: String!): Product }
    type Subscription { productChanged(id: ID!): Product! }
    type Product @key(fields: "id") { id: ID!, name: String! }
"#;

const PRODUCTS_V2: &str = r#"
    extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@key"])
    type Query { product(id: ID!): Product, version: String! }
    type Mutation { renameProduct(id: ID!, name: String!): Product }
    type Subscription { productChanged(id: ID!): Product!, productChangedV2(id: ID!): Product! }
    type Product @key(fields: "id") { id: ID!, name: String! }
"#;

const REVIEWS: &str = r#"
    extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@key", "@external"])
    type Product @key(fields: "id") { id: ID! @external, reviews: [Review!]! }
    type Review { body: String! }
"#;

const CONFLICTING_REVIEWS: &str = r#"
    extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@key"])
    type Query { review: String }
    type Product @key(fields: "id") { id: Int! }
"#;

/// The selected test child runs the configured Hive HTTP app. It is a
/// no-op in the ordinary test process so the parent test can select this one
/// with `current_exe` without recursively starting a router.
#[test]
fn router_child_entrypoint() {
    let Some(mode) = env::var_os(CHILD_ENV) else {
        return;
    };

    ntex::rt::System::build()
        .name("graphql-orm-router-wire-child")
        .build(ntex::rt::DefaultRuntime)
        .block_on(async move {
            if mode == "byte-routes" {
                byte_routes::run_child().await;
                return;
            }
            if mode == "admin" {
                let public_address = child_listener_address();
                let admin_address = child_listener_address();
                let products_endpoint = env::var(PRODUCTS_ENDPOINT_ENV).unwrap();
                let products_sdl = env::var(PRODUCTS_SDL_ENDPOINT_ENV).unwrap();
                let products_protocol = env::var(PRODUCTS_PROTOCOL_ENDPOINT_ENV).unwrap();
                let reviews_endpoint = env::var(REVIEWS_ENDPOINT_ENV).unwrap();
                let reviews_sdl = env::var(REVIEWS_SDL_ENDPOINT_ENV).unwrap();
                let reviews_protocol = env::var(REVIEWS_PROTOCOL_ENDPOINT_ENV).unwrap();
                let mut network = NetworkPolicy::new()
                    .allow_host("127.0.0.1")
                    .allow_network("127.0.0.0/8".parse().unwrap())
                    .allow_loopback(true);
                for endpoint in [&reviews_endpoint, &reviews_sdl, &reviews_protocol] {
                    network = network.allow_port(
                        Url::parse(endpoint)
                            .unwrap()
                            .port_or_known_default()
                            .unwrap(),
                    );
                }
                let trusted = TrustedSubgraph::new(
                    "reviews-service",
                    "reviews-service",
                    "reviews",
                    reviews_protocol,
                    endpoint_origin(&reviews_endpoint),
                    endpoint_origin(&reviews_sdl),
                )
                .with_schema_header("authorization", "Bearer schema-secret");
                let config = RouterConfig::new(public_address)
                    .with_graphql_path("/api/graphql")
                    .with_authentication_provider(Arc::new(TestAuthenticationProvider))
                    .with_scope_matcher(Arc::new(TestHierarchicalScopeMatcher))
                    .with_request_limits(
                        RequestLimits::new()
                            .with_max_request_body_bytes(512)
                            .with_max_parser_tokens(80)
                            .with_max_depth(4)
                            .with_max_aliases(12)
                            .with_max_directives(2)
                            .with_max_fields(6),
                    )
                    .with_subgraph(
                        StaticSubgraph::new("products", products_endpoint, products_sdl)
                            .with_protocol_url(products_protocol)
                            .with_schema_header("authorization", "Bearer schema-secret"),
                    )
                    .with_admin(
                        AdminConfig::new(admin_address, network)
                            .with_authentication_provider(Arc::new(TestAuthenticationProvider))
                            .trust_subgraph(trusted)
                            .with_max_request_body_bytes(256),
                    );
                config
                    .prepare()
                    .await
                    .expect("administrative graph should prepare")
                    .run()
                    .await
                    .expect("administrative router should run");
                return;
            }
            if matches!(
                mode.to_str(),
                Some(
                    "static"
                        | "polling"
                        | "graceful"
                        | "auth"
                        | "auth-subscriptions"
                        | "auth-subscriptions-polling"
                        | "auth-subscriptions-clock-past"
                        | "auth-subscriptions-clock-ahead"
                        | "auth-subscriptions-clock-error"
                )
            ) {
                let authenticated =
                    !matches!(mode.to_str(), Some("static" | "polling" | "graceful"));
                let subscriptions = matches!(
                    mode.to_str(),
                    Some(
                        "auth-subscriptions"
                            | "auth-subscriptions-polling"
                            | "auth-subscriptions-clock-past"
                            | "auth-subscriptions-clock-ahead"
                            | "auth-subscriptions-clock-error"
                    )
                );
                let polling = matches!(
                    mode.to_str(),
                    Some("polling" | "auth-subscriptions-polling")
                );
                let public_address = child_listener_address();
                let products = StaticSubgraph::new(
                    "products",
                    env::var(PRODUCTS_ENDPOINT_ENV).expect("products endpoint"),
                    env::var(PRODUCTS_SDL_ENDPOINT_ENV).expect("products SDL endpoint"),
                )
                .with_schema_header("authorization", "Bearer schema-secret");
                let reviews = StaticSubgraph::new(
                    "reviews",
                    env::var(REVIEWS_ENDPOINT_ENV).expect("reviews endpoint"),
                    env::var(REVIEWS_SDL_ENDPOINT_ENV).expect("reviews SDL endpoint"),
                )
                .with_schema_header("authorization", "Bearer schema-secret");
                let config = RouterConfig::new(public_address)
                    .with_graphql_path("/api/graphql")
                    .forward_header("x-approved")
                    .with_subgraph(if authenticated {
                        products.with_protocol_url(
                            env::var(PRODUCTS_PROTOCOL_ENDPOINT_ENV)
                                .expect("products protocol endpoint"),
                        )
                    } else {
                        products
                    })
                    .with_subgraph(if authenticated {
                        reviews.with_protocol_url(
                            env::var(REVIEWS_PROTOCOL_ENDPOINT_ENV)
                                .expect("reviews protocol endpoint"),
                        )
                    } else {
                        reviews
                    });
                let mut config = if authenticated {
                    config
                        .with_authentication_provider(Arc::new(TestAuthenticationProvider))
                        .with_scope_matcher(Arc::new(TestHierarchicalScopeMatcher))
                } else {
                    config.allow_anonymous_development(true)
                };
                if let Ok(path) = env::var("PERSISTED_MANIFEST") {
                    config = if env::var("PERSISTED_STRICT").unwrap() == "true" {
                        config.with_persisted_allow_list(path)
                    } else {
                        config.with_persisted_ids_or_text(path)
                    };
                }
                let clock_seconds = match mode.to_str() {
                    Some("auth-subscriptions-clock-past") => Some(Some(1_600_000_000)),
                    Some("auth-subscriptions-clock-ahead") => Some(Some(4_102_444_800)),
                    Some("auth-subscriptions-clock-error") => Some(None),
                    _ => None,
                };
                if let Some(seconds) = clock_seconds {
                    config = config.with_authentication_provider(Arc::new(TestClockProvider {
                        now: seconds
                            .map(|seconds| UNIX_EPOCH + Duration::new(seconds, 123_456_789)),
                    }));
                }
                if subscriptions {
                    config = config.with_subscriptions(
                        SubscriptionConfig::new()
                            .with_max_connections(2)
                            .with_max_pending_connections(2)
                            .with_max_operations_per_connection(2)
                            .with_broadcast_capacity(2)
                            .with_subgraph_buffer_capacity(2)
                            .with_connection_init_timeout(Duration::from_millis(500)),
                    );
                }
                if polling {
                    config = config
                        .with_schema_poll_interval(Duration::from_millis(100))
                        .with_schema_refresh_attempts(1);
                }
                if env::var_os("PERSISTED_INTROSPECTION").is_some() {
                    config = config.with_introspection(true);
                }
                if let Ok(listener) = env::var(PROMETHEUS_LISTENER_ENV) {
                    config = config.with_telemetry(
                        crate::RouterTelemetryConfig::new().with_prometheus_listener(
                            listener.parse().expect("Prometheus listener address"),
                            "/internal/metrics",
                        ),
                    );
                }
                let control = env::var("PERSISTED_CONTROL").ok();
                if let Some(control) = &control {
                    config = config.with_schema_poll_interval(Duration::from_secs(3600)).validate().unwrap();
                    let hive = config.persisted_documents.as_ref().unwrap().hive_config();
                    fs::write(format!("{control}.snapshot"), hive["storage"]["path"].as_str().unwrap()).unwrap();
                }
                let prepared = config.prepare().await.expect("static graph should prepare");
                assert_eq!(prepared.active_graph().version(), 1);
                if let Some(control) = control {
                    let handle = prepared.handle();
                    prepared.run_until_shutdown(async move {
                        loop {
                            if fs::remove_file(&control).is_ok() {
                                let outcome = handle.refresh().await.unwrap();
                                let status = handle.status().await;
                                fs::write(format!("{control}.result"), serde_json::to_vec(&json!({"outcome": format!("{outcome:?}"), "error": status.last_persisted_documents_error().or_else(|| status.last_composition_error()), "snapshotError": status.last_persisted_documents_error(), "compositionError": status.last_composition_error(), "compositionFailures": handle.metrics().composition_failure_total(), "version": status.active_graph().version(), "observed": status.subgraphs().iter().map(|s| s.observed_fingerprint()).collect::<Vec<_>>()})).unwrap()).unwrap();
                            }
                            hive_router::tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                    }).await.unwrap();
                    return;
                }

                if mode == "graceful" {
                    prepared
                        .run_until_shutdown(async {
                            hive_router::tokio::time::sleep(Duration::from_secs(1)).await;
                        })
                        .await
                        .expect("static public router should shut down gracefully");
                } else {
                    prepared
                        .run()
                        .await
                        .expect("static public router should run");
                }
                return;
            }
            hive_router::init_rustls_crypto_provider();
            run_wire_app().await;
        });
}

static CHILD_LISTENERS: Mutex<Vec<TcpListener>> = Mutex::new(Vec::new());

fn child_listener_address() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("owned child listener");
    let address = listener.local_addr().unwrap();
    CHILD_LISTENERS.lock().unwrap().push(listener);
    address
}

// Authentication subscriptions need the actual public address during prepare.
// Keep the child-bound socket alive, then move it into the server without rebinding.
pub(crate) fn bind_test_listener(address: SocketAddr, kind: &str) -> io::Result<TcpListener> {
    let reserved = {
        let mut listeners = CHILD_LISTENERS.lock().unwrap();
        listeners
            .iter()
            .position(|listener| listener.local_addr().ok() == Some(address))
            .map(|index| listeners.swap_remove(index))
    };
    let listener = match reserved {
        Some(listener) => listener,
        None => TcpListener::bind(address)?,
    };
    listener.set_nonblocking(true)?;
    if env::var_os(CHILD_ENV).is_some() {
        println!("WIRE_BOUND {kind} {}", listener.local_addr()?);
    }
    Ok(listener)
}

async fn run_wire_app() {
    use hive_router::{background_tasks, config::load_config, ntex::web};
    let config = load_config(env::var("ROUTER_CONFIG_FILE_PATH").ok())
        .unwrap()
        .into_static();
    let mut telemetry = hive_router::telemetry::Telemetry::init_global(config).unwrap();
    let internal = crate::subscriptions::InternalSubscriptionEndpoint::generate().unwrap();
    internal.continuity.install(&mut telemetry.context);
    let mut tasks = background_tasks::BackgroundTasksManager::new();
    let (state, schema) = hive_router::configure_app_from_config(
        config,
        telemetry.context.clone(),
        &mut tasks,
        PluginRegistry::new().register::<WireProofPlugin>(),
    )
    .await
    .unwrap();
    let paths = hive_router::RouterPaths::new(
        config.graphql_path().to_owned(),
        Some(internal.path.clone()),
        None,
    );
    let listener = bind_test_listener("127.0.0.1:0".parse().unwrap(), "public").unwrap();
    let gateway = crate::subscriptions::SubscriptionGateway::new(
        SubscriptionConfig::default(),
        Arc::new(TestAuthenticationProvider),
        internal,
        listener.local_addr().unwrap(),
        Arc::new(crate::metrics::RouterMetrics::default()),
        None,
    );
    let public_path = config.websocket_path().unwrap().to_owned();
    web::HttpServer::new(async move || {
        web::App::new()
            .middleware(hive_router::plugins::plugins_service::PluginService::new(
                paths.clone(),
                None,
            ))
            .middleware(hive_router::pipeline::request_identifiers::RequestIdentifiersService)
            .state(schema.clone())
            .state(state.clone())
            .state(state.telemetry_context.clone())
            .state(schema.callback_subscriptions.clone())
            .configure(|routes| {
                let gateway = gateway.clone();
                routes.service(
                    web::resource(public_path.as_str())
                        .guard(web::guard::fn_guard(|head| {
                            head.headers()
                                .get(hive_router::ntex::http::header::UPGRADE)
                                .and_then(|value| value.to_str().ok())
                                .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
                        }))
                        .route(web::get().to(move |request| {
                            crate::subscriptions::websocket_index(request, gateway.clone())
                        })),
                );
                hive_router::configure_ntex_app(routes, &paths, None);
            })
    })
    .workers(1)
    .listen(listener)
    .unwrap()
    .run()
    .await
    .unwrap();
}

#[derive(Debug)]
struct TestAuthenticationProvider;

impl AuthenticationProvider for TestAuthenticationProvider {
    fn authenticate_bearer(
        &self,
        token: &str,
    ) -> Result<AuthenticatedPrincipal, AuthenticationError> {
        let (subject, scopes) = match token {
            "product-p1" => ("test-subject", vec!["products.p1.read"]),
            "product-admin" => ("test-subject", vec!["products.admin"]),
            "product-writer" => ("test-subject", vec!["products.write"]),
            "product-prefix" => ("test-subject", vec!["products.*"]),
            "product-nested" => ("test-subject", vec!["products.p1.x.read"]),
            "product-events" | "product-events-refreshed" | "short-lived" => (
                "test-subject",
                vec!["products.p1.events", "products.failure.events"],
            ),
            "admin" => (
                "operator",
                vec![
                    "router.status",
                    "router.refresh",
                    "router.register",
                    "router.remove",
                    "router.metrics",
                ],
            ),
            "status" => ("operator", vec!["router.status"]),
            "reviews-register" => ("reviews-service", vec!["router.register"]),
            "wrong-register" => ("untrusted-service", vec!["router.register"]),
            _ => {
                return Err(AuthenticationError::invalid_credential(
                    "test token rejected",
                ));
            }
        };
        let lifetime = if token == "short-lived" { 1 } else { 3_600 };
        AuthenticatedPrincipal::new(
            subject,
            scopes,
            SystemTime::now().checked_add(Duration::from_secs(lifetime)),
        )
    }
}

// The real agql adapter's signed-token/clock mapping is tested in agql.rs.
// This wire provider isolates the gateway's handling of that same clock contract.
#[derive(Debug)]
struct TestClockProvider {
    now: Option<SystemTime>,
}
impl AuthenticationProvider for TestClockProvider {
    fn authenticate_bearer(
        &self,
        token: &str,
    ) -> Result<AuthenticatedPrincipal, AuthenticationError> {
        let principal = TestAuthenticationProvider.authenticate_bearer(token)?;
        AuthenticatedPrincipal::new(
            principal.subject(),
            principal.scopes().iter().cloned(),
            self.now
                .unwrap_or_else(SystemTime::now)
                .checked_add(Duration::from_secs(8)),
        )
    }

    fn current_time(&self) -> Result<SystemTime, AuthenticationError> {
        self.now
            .ok_or_else(|| AuthenticationError::unavailable("synthetic clock failure"))
    }
}

#[derive(Debug)]
struct TestHierarchicalScopeMatcher;

impl ScopeMatcher for TestHierarchicalScopeMatcher {
    fn matches(&self, granted: &str, required: &str) -> bool {
        granted == required
            || granted
                .strip_suffix('*')
                .is_some_and(|prefix| required.starts_with(prefix))
    }
}

struct WireProofPlugin {
    store: GraphStore,
}

impl WireProofPlugin {
    fn inputs(products: &str, reviews: &str) -> Result<Vec<CandidateSubgraph>, io::Error> {
        let products_endpoint = env::var(PRODUCTS_ENDPOINT_ENV).map_err(|error| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("missing {PRODUCTS_ENDPOINT_ENV}: {error}"),
            )
        })?;
        let reviews_endpoint = env::var(REVIEWS_ENDPOINT_ENV).map_err(|error| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("missing {REVIEWS_ENDPOINT_ENV}: {error}"),
            )
        })?;
        Ok(vec![
            CandidateSubgraph::new("products", &products_endpoint, products),
            CandidateSubgraph::new("reviews", &reviews_endpoint, reviews),
        ])
    }

    fn replace(&self, products: &str, reviews: &str) {
        let Ok(inputs) = Self::inputs(products, reviews) else {
            return;
        };
        // Candidate construction is complete before GraphStore publishes it.
        // A composition error intentionally leaves the retained graph intact.
        let _ = self.store.replace(&inputs);
    }
}

#[hive_router::async_trait]
impl RouterPlugin for WireProofPlugin {
    type Config = ();

    fn plugin_name() -> &'static str {
        "graphql-orm-router-wire-proof"
    }

    fn on_plugin_init(payload: OnPluginInitPayload<Self>) -> OnPluginInitResult<Self> {
        let inputs = match Self::inputs(PRODUCTS_V1, REVIEWS) {
            Ok(inputs) => inputs,
            Err(error) => return OnPluginInitPayload::<Self>::error(error),
        };
        match GraphStore::new(&inputs) {
            Ok(store) => payload.initialize_plugin(Self { store }),
            Err(error) => OnPluginInitPayload::<Self>::error(error),
        }
    }

    fn on_http_request<'request>(
        &'request self,
        payload: OnHttpRequestHookPayload<'request>,
    ) -> OnHttpRequestHookFuture<'request> {
        Box::pin(async move {
            match payload
                .router_http_request
                .headers()
                .get("x-graphql-orm-wire-switch")
                .and_then(|value| value.to_str().ok())
            {
                Some("v2") => self.replace(PRODUCTS_V2, REVIEWS),
                Some("invalid") => self.replace(PRODUCTS_V2, CONFLICTING_REVIEWS),
                _ => {}
            }
            payload.set_supergraph(self.store.load().hive.clone());
            payload.proceed()
        })
    }

    async fn on_graphql_analysis<'execution>(
        &'execution self,
        payload: &mut OnGraphqlAnalysisHookPayload<'execution>,
    ) -> OnGraphqlAnalysisHookResult {
        let deny_roots = payload
            .router_http_request
            .headers
            .get("x-graphql-orm-wire-deny")
            .and_then(|value| value.to_str().ok())
            == Some("root");
        if deny_roots {
            // Hive evaluates this filter after variable coercion and directive
            // inclusion, so an @include controlled by a Boolean variable is
            // handled at the actual execution boundary rather than syntactically.
            payload.filter_operation(|selection| match selection {
                Selection::Field(field)
                    if matches!(
                        field.parent_type_name,
                        "Query" | "Mutation" | "Subscription"
                    ) =>
                {
                    selection.reject(hive_router::GraphQLError::from_message_and_code(
                        "loopback test policy denied root field",
                        "WIRE_DENIED",
                    ))
                }
                _ => selection.keep(),
            });
        }
        OnGraphqlAnalysisHookResult::Proceed
    }
}

#[test]
fn static_children_bind_their_own_ports_even_when_the_old_choice_is_occupied() {
    let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
    let products = LoopbackSubgraph::start(SubgraphKind::Products);
    let reviews = LoopbackSubgraph::start(SubgraphKind::Reviews);
    let products_sdl = LoopbackSdl::start(PRODUCTS_V1);
    let reviews_sdl = LoopbackSdl::start(REVIEWS);
    let mut first = TestRouter::spawn_static(
        products.endpoint(),
        reviews.endpoint(),
        products_sdl.endpoint(),
        reviews_sdl.endpoint(),
    );
    let mut second = TestRouter::spawn_static(
        products.endpoint(),
        reviews.endpoint(),
        products_sdl.endpoint(),
        reviews_sdl.endpoint(),
    );
    first.wait_until_ready();
    second.wait_until_ready();
    assert_ne!(first.address, second.address);
    assert_ne!(first.address, occupied.local_addr().unwrap());
}

fn persisted_documents_wire_case(case: &str) {
    use sha2::{Digest, Sha256};
    let products = LoopbackSubgraph::start(SubgraphKind::Products);
    let reviews = LoopbackSubgraph::start(SubgraphKind::Reviews);
    let products_sdl = LoopbackSdl::start(PRODUCTS_V1);
    let reviews_sdl = LoopbackSdl::start(REVIEWS);
    let mut operations = authenticated_product_operations(false);
    operations[0].authorization = AuthorizationRequirement::AllScopes {
        scopes: vec![scope("products.{id}.read")],
    };
    let products_protocol = LoopbackProtocol::start(protocol_descriptor(
        "products",
        products.endpoint(),
        products_sdl.endpoint(),
        operations,
    ));
    let reviews_protocol = LoopbackProtocol::start(protocol_descriptor(
        "reviews",
        reviews.endpoint(),
        reviews_sdl.endpoint(),
        Vec::new(),
    ));
    let temp = TemporaryConfig::new();
    let manifest = temp.directory.join("manifest.json");
    let query = "query { __typename }";
    let mutation = "mutation { __typename }";
    let subscription = r#"subscription { productChanged(id: "p1") { id } }"#;
    let hash = |text: &str| format!("{:x}", Sha256::digest(text.as_bytes()));
    let protected = r#"query { product(id: "p1") { id } }"#;
    let introspection = "query { __schema { queryType { name } } }";
    let padded_subscription = format!(
        "{: <4096}",
        "subscription Events($Id: ID!) { productChanged(id: $Id) { id } }"
    );
    let padded_subscription = &padded_subscription[..];
    let large_query = "query Find($Id: ID!) { product(id: $Id) { id } }";
    let sized_document = |size: usize, padding: char| {
        format!(
            "{large_query}\n#{}",
            padding.to_string().repeat(size - large_query.len() - 2)
        )
    };
    let sized_documents = [
        ("20-kib", sized_document(20 * 1024, ' '), 0, true),
        (
            "20-kib-variables",
            sized_document(20 * 1024, ' '),
            33 * 1024,
            true,
        ),
        ("20-kib-escaped", sized_document(20 * 1024, '\t'), 0, true),
        ("36-kib", sized_document(36 * 1024, ' '), 0, true),
        (
            "36-kib-variables",
            sized_document(36 * 1024, ' '),
            33 * 1024,
            false,
        ),
        ("36-kib-escaped", sized_document(36 * 1024, '\t'), 0, false),
        ("70-kib", sized_document(70 * 1024, ' '), 0, false),
    ];
    let documents = [
        query,
        mutation,
        subscription,
        protected,
        introspection,
        padded_subscription,
    ]
    .into_iter()
    .chain(
        sized_documents
            .iter()
            .filter(|_| case == "document_sizes")
            .map(|(_, document, _, _)| document.as_str()),
    )
    .map(|text| (hash(text), text))
    .collect::<std::collections::BTreeMap<_, _>>();
    fs::write(&manifest, serde_json::to_vec(&documents).unwrap()).unwrap();
    let mut command = Command::new(env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "federation::wire_tests::router_child_entrypoint",
            "--nocapture",
        ])
        .env(CHILD_ENV, "auth-subscriptions")
        .env("TMPDIR", &temp.directory)
        .env(PRODUCTS_ENDPOINT_ENV, products.endpoint())
        .env(REVIEWS_ENDPOINT_ENV, reviews.endpoint())
        .env(PRODUCTS_SDL_ENDPOINT_ENV, products_sdl.endpoint())
        .env(REVIEWS_SDL_ENDPOINT_ENV, reviews_sdl.endpoint())
        .env(PRODUCTS_PROTOCOL_ENDPOINT_ENV, products_protocol.endpoint())
        .env(REVIEWS_PROTOCOL_ENDPOINT_ENV, reviews_protocol.endpoint());
    if case != "disabled" {
        command.env("PERSISTED_MANIFEST", &manifest).env(
            "PERSISTED_STRICT",
            (!case.starts_with("non_strict")).to_string(),
        );
    }
    let control = temp.directory.join("refresh-control");
    if case.starts_with("snapshot_") {
        command.env("PERSISTED_CONTROL", &control);
    }
    if case.ends_with("authorization") {
        command.env("PERSISTED_INTROSPECTION", "true");
    }
    let child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut router = TestRouter::from_child(child, false).0;
    if case == "snapshot_cold_read" {
        // A TCP-only probe cannot invoke the Hive plugin or pin a runtime.
        wait_until("cold router TCP listener", || {
            TcpStream::connect_timeout(&router.address, Duration::from_millis(100)).is_ok()
        });
    } else {
        router.wait_until_ready();
    }
    let headers = [("authorization", "Bearer product-events")];
    let graphql = |payload| {
        router
            .graphql_response_at("/api/graphql", payload, &headers)
            .1
    };
    match case {
        "document_sizes" => {
            let mut socket = TestWebSocket::connect_at(router.address, "/api/graphql");
            socket.connection_init_bearer("product-p1");
            for (label, document, unused_bytes, fits_websocket) in &sized_documents {
                let variables = json!({"Id": "p1", "Unused": "x".repeat(*unused_bytes)});
                let result = router
                    .graphql_response_at(
                        "/api/graphql",
                        json!({"documentId": hash(document), "variables": variables}),
                        &[("authorization", "Bearer product-p1")],
                    )
                    .1;
                assert_eq!(result["data"]["product"]["id"], "p1", "{label}: {result}");
                let opens = products.opens();
                socket.send_json(&json!({"id": label, "type": "subscribe", "payload": {
                    "documentId": hash(document), "variables": variables
                }}));
                let result = socket.next_message();
                assert_eq!(result["id"], *label, "{result}");
                if *fits_websocket {
                    assert_eq!(result["type"], "next", "{label}: {result}");
                    // This scope requires the injected Id projection: a successful
                    // response proves it was preserved alongside the large document.
                    assert_eq!(
                        result["payload"]["data"]["product"]["id"], "p1",
                        "{label}: {result}"
                    );
                    let complete = socket.next_message();
                    assert_eq!(complete["type"], "complete", "{complete}");
                } else {
                    assert_eq!(result["type"], "error", "{label}: {result}");
                    assert_eq!(
                        result["payload"][0]["extensions"]["code"], "REQUEST_TOO_LARGE",
                        "{result}"
                    );
                    assert!(!result.to_string().contains("query Find"), "{result}");
                    assert_eq!(
                        products.opens(),
                        opens,
                        "oversized operation must not be forwarded"
                    );
                    // Reuse the same id immediately: the connection and operation
                    // tracking must remain usable, without a trailing complete.
                    socket.send_json(&json!({"id": label, "type": "subscribe", "payload": {"documentId": hash(query)}}));
                    let next = socket.next_message();
                    assert_eq!(next["type"], "next", "{next}");
                    assert_eq!(next["payload"]["data"]["__typename"], "Query", "{next}");
                    assert_eq!(socket.next_message()["type"], "complete");
                }
            }
            socket.close();
        }
        "ws_budget" => {
            let mut socket = TestWebSocket::connect_at(router.address, "/api/graphql");
            socket.connection_init_bearer("product-events");
            socket.send_json(&json!({"id": "large", "type": "subscribe", "payload": {"documentId": hash(query), "variables": {"Unused": "x".repeat(33 * 1024)}}}));
            let result = socket.next_message();
            assert_eq!(result["type"], "next", "{result}");
            assert_eq!(result["payload"]["data"]["__typename"], "Query", "{result}");
            assert_eq!(socket.next_message()["type"], "complete");
            socket.send_json(&json!({"id": "still-open", "type": "subscribe", "payload": {"documentId": hash(padded_subscription), "variables": {"Id": "p1", "Unused": "x".repeat(28 * 1024)}}}));
            let result = socket.next_message();
            assert_eq!(
                result["payload"]["data"]["productChanged"]["id"], "p1",
                "{result}"
            );
            socket.close();
        }
        "snapshot_cold_read" => {
            let snapshot = fs::read_to_string(format!("{}.snapshot", control.display())).unwrap();
            let original = fs::read(&snapshot).unwrap();
            let tampered = r#"mutation { renameProduct(id: "p1", name: "tampered") { id } }"#;
            fs::write(
                &snapshot,
                serde_json::to_vec(&json!({hash(query): tampered})).unwrap(),
            )
            .unwrap();
            let opens_before = products.opens();
            for _ in 0..3 {
                let (status, result) = router.graphql_response_at(
                    "/api/graphql",
                    json!({"documentId": hash(query)}),
                    &headers,
                );
                assert_eq!(status, 503, "{result}");
                assert_eq!(
                    result["errors"][0]["extensions"]["code"], "SERVICE_UNAVAILABLE",
                    "{result}"
                );
                assert!(result.get("data").is_none(), "{result}");
                assert_eq!(
                    products.opens(),
                    opens_before,
                    "no tampered document may execute"
                );
            }
            // A pre-build check failure does not poison the generation.
            fs::write(&snapshot, original).unwrap();
            let original = graphql(json!({"documentId": hash(query)}));
            assert_eq!(original["data"]["__typename"], "Query", "{original}");
        }
        "snapshot_retry" => {
            let snapshot = fs::read_to_string(format!("{}.snapshot", control.display())).unwrap();
            let original = fs::read(&snapshot).unwrap();
            fs::remove_file(&snapshot).unwrap();
            products_sdl.set(PRODUCTS_V2, "persisted-v2", 200);
            products_protocol.set(
                protocol_descriptor(
                    "products",
                    products.endpoint(),
                    products_sdl.endpoint(),
                    authenticated_product_operations(true),
                ),
                "persisted-policy-v2",
                200,
            );
            let result_path = format!("{}.result", control.display());
            let mut rounds = Vec::new();
            for round in 0..2 {
                if round == 1 {
                    fs::write(&snapshot, &original).unwrap();
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(&snapshot, fs::Permissions::from_mode(0o600)).unwrap();
                    }
                }
                fs::write(&control, "refresh").unwrap();
                wait_until("persisted refresh retry result", || {
                    std::path::Path::new(&result_path).exists()
                });
                let result: Value =
                    serde_json::from_slice(&fs::read(&result_path).unwrap()).unwrap();
                fs::remove_file(&result_path).unwrap();
                rounds.push(result);
            }
            assert_eq!(rounds[0]["outcome"], "Rejected", "{:?}", rounds);
            assert_eq!(rounds[1]["outcome"], "Activated", "{:?}", rounds);
            assert_eq!(rounds[1]["version"], 2);
            assert!(rounds[0]["compositionError"].is_null(), "{:?}", rounds);
            assert_eq!(rounds[0]["compositionFailures"], 0);
            assert!(
                rounds[0]["snapshotError"]
                    .as_str()
                    .unwrap()
                    .contains("persisted documents snapshot")
            );
            assert!(rounds[1]["snapshotError"].is_null(), "{:?}", rounds);
            assert!(
                rounds[0]["observed"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(Value::is_null),
                "{:?}",
                rounds
            );
        }

        "snapshot_rewrite" | "snapshot_delete" => {
            let original = graphql(json!({"documentId": hash(query)}));
            assert_eq!(original["data"]["__typename"], "Query", "{original}");
            let snapshot = fs::read_to_string(format!("{}.snapshot", control.display())).unwrap();
            if case == "snapshot_rewrite" {
                fs::write(
                    &snapshot,
                    serde_json::to_vec(&json!({hash(query): mutation})).unwrap(),
                )
                .unwrap();
            } else {
                fs::remove_file(&snapshot).unwrap();
            }
            products_sdl.set(PRODUCTS_V2, "persisted-v2", 200);
            products_protocol.set(
                protocol_descriptor(
                    "products",
                    products.endpoint(),
                    products_sdl.endpoint(),
                    authenticated_product_operations(true),
                ),
                "persisted-policy-v2",
                200,
            );
            fs::write(&control, "refresh").unwrap();
            let result_path = format!("{}.result", control.display());
            wait_until("explicit persisted refresh result", || {
                std::path::Path::new(&result_path).exists()
            });
            let result: Value = serde_json::from_slice(&fs::read(result_path).unwrap()).unwrap();
            assert_eq!(result["outcome"], "Rejected", "{result}");
            assert!(
                result["error"]
                    .as_str()
                    .unwrap()
                    .contains("persisted documents snapshot"),
                "{result}"
            );
            let result = graphql(json!({"documentId": hash(query)}));
            assert_eq!(result["data"]["__typename"], "Query", "{result}");
            let mut socket = TestWebSocket::connect_at(router.address, "/api/graphql");
            socket.connection_init_bearer("product-events");
            socket.send_json(&json!({"id": "original", "type": "subscribe", "payload": {"documentId": hash(query)}}));
            let result = socket.next_message();
            assert_eq!(result["payload"]["data"]["__typename"], "Query", "{result}");
            socket.close();
        }
        "introspection_disabled" | "non_strict_introspection_disabled" => {
            let result = graphql(json!({"documentId": hash(introspection)}));
            assert_eq!(
                result["errors"][0]["extensions"]["code"], "INTROSPECTION_DISABLED",
                "{result}"
            );
            if case.starts_with("non_strict") {
                let plain = graphql(json!({"query": introspection}));
                assert_eq!(
                    plain["errors"][0]["extensions"]["code"],
                    result["errors"][0]["extensions"]["code"]
                );
            }
        }
        "authorization" | "non_strict_authorization" => {
            for (text, expected, credentials) in [
                (protected, "UNAUTHENTICATED", &[][..]),
                (protected, "FORBIDDEN", &headers[..]),
                (introspection, "UNAUTHENTICATED", &[][..]),
            ] {
                let opens = products.opens();
                let result = router
                    .graphql_response_at(
                        "/api/graphql",
                        json!({"documentId": hash(text)}),
                        credentials,
                    )
                    .1;
                assert_eq!(
                    result["errors"][0]["extensions"]["code"], expected,
                    "{result}"
                );
                assert_eq!(
                    products.opens(),
                    opens,
                    "id denial must precede upstream execution"
                );
                if case == "authorization" {
                    let both = router
                        .graphql_response_at(
                            "/api/graphql",
                            json!({"documentId": hash(text), "query": query}),
                            credentials,
                        )
                        .1;
                    assert_eq!(both["errors"][0]["extensions"]["code"], expected, "{both}");
                    assert_eq!(
                        products.opens(),
                        opens,
                        "strict id-plus-text denial must precede upstream execution"
                    );
                }
                if case == "non_strict_authorization" {
                    let plain = router
                        .graphql_response_at("/api/graphql", json!({"query": text}), credentials)
                        .1;
                    assert_eq!(
                        plain["errors"][0]["extensions"]["code"],
                        result["errors"][0]["extensions"]["code"]
                    );
                    assert_eq!(
                        products.opens(),
                        opens,
                        "free-text denial must precede upstream execution"
                    );
                    let both = router
                        .graphql_response_at(
                            "/api/graphql",
                            json!({"documentId": hash(text), "query": query}),
                            credentials,
                        )
                        .1;
                    assert_eq!(both["data"]["__typename"], "Query", "{both}");
                }
            }
        }
        "non_strict_ws" => {
            for payload in [
                json!({"documentId": hash(query)}),
                json!({"query": query}),
                json!({"documentId": hash(mutation), "query": query}),
            ] {
                let mut socket = TestWebSocket::connect_at(router.address, "/api/graphql");
                socket.connection_init_bearer("product-events");
                socket
                    .send_json(&json!({"id": "ordinary", "type": "subscribe", "payload": payload}));
                let result = socket.next_message();
                assert_eq!(result["payload"]["data"]["__typename"], "Query", "{result}");
                socket.close();
            }
        }
        "apollo_unknown" => {
            let result = graphql(
                json!({"query": query, "extensions": {"persistedQuery": {"version": 1, "sha256Hash": hash("unknown")}}}),
            );
            assert_eq!(
                result["errors"][0]["extensions"]["code"], "PERSISTED_DOCUMENT_NOT_FOUND",
                "{result}"
            );
        }
        "get_extensions" => {
            let mut url = Url::parse("http://router/api/graphql").unwrap();
            url.query_pairs_mut().append_pair(
                "extensions",
                &json!({"persistedQuery": {"version": 1, "sha256Hash": hash(query)}}).to_string(),
            );
            let result = raw_http_within(
                router.address,
                "GET",
                &format!("{}?{}", url.path(), url.query().unwrap()),
                &headers,
                None,
                HTTP_STREAM_PROBE,
            )
            .unwrap();
            let result: Value = serde_json::from_slice(&result.body).unwrap();
            assert_eq!(result["data"]["__typename"], "Query", "{result}");
        }
        "non_strict" | "disabled" => {
            let plain = graphql(json!({"query": query}));
            assert_eq!(plain["data"]["__typename"], "Query", "{plain}");
            let result = graphql(json!({"documentId": hash(query)}));
            if case == "disabled" {
                assert!(result.get("errors").is_some(), "{result}");
            } else {
                assert_eq!(result["data"]["__typename"], "Query", "{result}");
            }
        }
        "post" => {
            let plain = graphql(json!({"query": query}));
            assert_eq!(
                plain["errors"][0]["extensions"]["code"], "PERSISTED_DOCUMENT_ID_REQUIRED",
                "{plain}"
            );
            for payload in [
                json!({"documentId": hash(query)}),
                json!({"extensions": {"persistedQuery": {"version": 1, "sha256Hash": hash(query)}}}),
            ] {
                let result = graphql(payload);
                assert_eq!(result["data"]["__typename"], "Query", "{result}");
            }
            let body = serde_json::to_vec(&json!({"documentId": hash(subscription)})).unwrap();
            for path in ["/api/graphql", "/api/graphql/x", "/api/graphql/sha256:abc"] {
                let result = raw_http_within(
                    router.address,
                    "POST",
                    path,
                    &headers,
                    Some(&body),
                    HTTP_STREAM_PROBE,
                )
                .unwrap();
                assert_http_subscription_refused(path, &result);
            }
            // Rewriting the operator file cannot change the prepared allow-list.
            fs::write(&manifest, b"{}").unwrap();
            let result = graphql(json!({"documentId": hash(query)}));
            assert_eq!(result["data"]["__typename"], "Query", "{result}");
        }
        "redaction" => {
            let result = graphql(json!({"documentId": query, "query": query}));
            assert_eq!(
                result["errors"][0]["extensions"]["code"], "PERSISTED_DOCUMENT_NOT_FOUND",
                "{result}"
            );
            assert!(
                !result.to_string().contains(query),
                "persisted errors must not echo document text: {result}"
            );
        }
        "unknown" => {
            let result = graphql(json!({"documentId": hash("unknown"), "query": query}));
            assert_eq!(
                result["errors"][0]["extensions"]["code"], "PERSISTED_DOCUMENT_NOT_FOUND",
                "{result}"
            );
        }
        "both" => {
            let result = graphql(json!({"documentId": hash(query), "query": mutation}));
            assert_eq!(result["data"]["__typename"], "Query", "{result}");
        }
        "batch" => {
            let result = graphql(json!([{"documentId": hash(query)}, {"query": query}]));
            assert_eq!(
                result["errors"][0]["extensions"]["code"], "PERSISTED_DOCUMENT_EXTRACTION_FAILED",
                "{result}"
            );
        }
        "get" => {
            let result = raw_http_within(
                router.address,
                "GET",
                "/api/graphql?query=query%20%7B%20__typename%20%7D",
                &headers,
                None,
                HTTP_STREAM_PROBE,
            )
            .unwrap();
            assert!(
                String::from_utf8_lossy(&result.body).contains("PERSISTED_DOCUMENT_ID_REQUIRED")
            );
            let path = format!("/api/graphql?documentId={}", hash(query));
            let result = raw_http_within(
                router.address,
                "GET",
                &path,
                &headers,
                None,
                HTTP_STREAM_PROBE,
            )
            .unwrap();
            let result: Value = serde_json::from_slice(&result.body).unwrap();
            assert_eq!(result["data"]["__typename"], "Query", "{result}");
        }
        "ws_query" | "ws_mutation" | "ws_subscription" => {
            let text = match case {
                "ws_query" => query,
                "ws_mutation" => mutation,
                _ => subscription,
            };
            let mut socket = TestWebSocket::connect_at(router.address, "/api/graphql");
            socket.connection_init_bearer("product-events");
            socket.subscribe("denied", text);
            let error = socket.next_message();
            assert_eq!(error["type"], "error", "{error}");
            assert_eq!(
                error["payload"][0]["extensions"]["code"], "PERSISTED_DOCUMENT_ID_REQUIRED",
                "{error}"
            );
            socket.send_json(&json!({"id": "unknown", "type": "subscribe", "payload": {"documentId": hash("unknown"), "query": text}}));
            let error = socket.next_message();
            assert_eq!(
                error["payload"][0]["extensions"]["code"], "PERSISTED_DOCUMENT_NOT_FOUND",
                "{error}"
            );
            socket.close();
            for payload in [
                json!({"documentId": hash(text), "query": "query { unregistered }"}),
                json!({"extensions": {"persistedQuery": {"version": 1, "sha256Hash": hash(text)}}}),
            ] {
                let mut socket = TestWebSocket::connect_at(router.address, "/api/graphql");
                socket.connection_init_bearer("product-events");
                socket
                    .send_json(&json!({"id": "allowed", "type": "subscribe", "payload": payload}));
                let result = socket.next_message();
                assert_eq!(result["type"], "next", "{result}");
                assert!(result["payload"].get("data").is_some(), "{result}");
                socket.close();
            }
        }
        _ => unreachable!(),
    }
}

#[test]
fn persisted_review_introspection_disabled() {
    persisted_documents_wire_case("introspection_disabled");
}
#[test]
fn persisted_review_non_strict_introspection_disabled() {
    persisted_documents_wire_case("non_strict_introspection_disabled");
}

#[test]
fn persisted_review_snapshot_retry() {
    persisted_documents_wire_case("snapshot_retry");
}

#[test]
fn persisted_review_snapshot_cold_read() {
    persisted_documents_wire_case("snapshot_cold_read");
}

#[test]
fn persisted_review_document_sizes() {
    persisted_documents_wire_case("document_sizes");
}

#[test]
fn persisted_review_ws_budget() {
    persisted_documents_wire_case("ws_budget");
}

#[test]
fn persisted_review_snapshot_rewrite() {
    persisted_documents_wire_case("snapshot_rewrite");
}

#[test]
fn persisted_review_snapshot_delete() {
    persisted_documents_wire_case("snapshot_delete");
}

#[test]
fn persisted_review_authorization() {
    persisted_documents_wire_case("authorization");
}

#[test]
fn persisted_review_non_strict_authorization() {
    persisted_documents_wire_case("non_strict_authorization");
}

#[test]
fn persisted_review_non_strict_ws() {
    persisted_documents_wire_case("non_strict_ws");
}

#[test]
fn persisted_review_apollo_unknown() {
    persisted_documents_wire_case("apollo_unknown");
}

#[test]
fn persisted_review_get_extensions() {
    persisted_documents_wire_case("get_extensions");
}

#[test]
fn persisted_wire_redaction() {
    persisted_documents_wire_case("redaction");
}

#[test]
fn persisted_wire_non_strict() {
    persisted_documents_wire_case("non_strict");
}

#[test]
fn persisted_wire_disabled() {
    persisted_documents_wire_case("disabled");
}

#[test]
fn persisted_wire_post() {
    persisted_documents_wire_case("post");
}

#[test]
fn persisted_wire_unknown() {
    persisted_documents_wire_case("unknown");
}

#[test]
fn persisted_wire_both() {
    persisted_documents_wire_case("both");
}

#[test]
fn persisted_wire_batch() {
    persisted_documents_wire_case("batch");
}

#[test]
fn persisted_wire_get() {
    persisted_documents_wire_case("get");
}

#[test]
fn persisted_wire_ws_query() {
    persisted_documents_wire_case("ws_query");
}

#[test]
fn persisted_wire_ws_mutation() {
    persisted_documents_wire_case("ws_mutation");
}

#[test]
fn persisted_wire_ws_subscription() {
    persisted_documents_wire_case("ws_subscription");
}

#[test]
fn hive_http_app_preserves_federation_and_replacement_invariants() {
    let products = LoopbackSubgraph::start(SubgraphKind::Products);
    let reviews = LoopbackSubgraph::start(SubgraphKind::Reviews);
    let temp = TemporaryConfig::new();
    let mut router = TestRouter::spawn(&temp, products.endpoint(), reviews.endpoint());
    router.wait_until_ready();

    let cross_subgraph = router.graphql(
        json!({"query": "query { product(id: \"p1\") { id name reviews { body } } }"}),
        &[],
    );
    assert_eq!(
        cross_subgraph["data"]["product"]["reviews"][0]["body"],
        "excellent"
    );
    assert!(
        products.opens() > 0,
        "root product request reached products"
    );
    assert!(reviews.opens() > 0, "entity follow-up reached reviews");

    let mutation = router.graphql(
        json!({"query": "mutation { renameProduct(id: \"p1\", name: \"renamed\") { id name } }"}),
        &[],
    );
    assert_eq!(mutation["data"]["renameProduct"]["name"], "renamed");

    let excluded = router.graphql(
        json!({
            "query": "query Denied($enabled: Boolean!) { product(id: \"p1\") @include(if: $enabled) { id } }",
            "variables": {"enabled": false}
        }),
        &[("x-graphql-orm-wire-deny", "root")],
    );
    assert_eq!(excluded["data"], json!({}));

    let opens_before_denial = products.opens() + reviews.opens();
    let denied = router.graphql(
        json!({
            "query": "query Denied($enabled: Boolean!) { product(id: \"p1\") @include(if: $enabled) { id } }",
            "variables": {"enabled": true}
        }),
        &[("x-graphql-orm-wire-deny", "root")],
    );
    assert_eq!(denied["errors"][0]["extensions"]["code"], "WIRE_DENIED");
    assert_eq!(
        products.opens() + reviews.opens(),
        opens_before_denial,
        "denial must happen before a downstream connection opens"
    );

    let router_for_slow_request = router.clone();
    let slow_request = thread::spawn(move || {
        router_for_slow_request.graphql(
            json!({"query": "query { product(id: \"slow\") { id name } }"}),
            &[],
        )
    });
    products.wait_for_slow_request();

    let replacement = router.graphql(
        json!({"query": "query { version }"}),
        &[("x-graphql-orm-wire-switch", "v2")],
    );
    assert_eq!(replacement["data"]["version"], "v2");

    products.release_slow_request();
    let slow_result = slow_request
        .join()
        .expect("slow request thread should not panic");
    assert_eq!(slow_result["data"]["product"]["id"], "slow");

    let invalid_replacement = router.graphql(
        json!({"query": "query { version }"}),
        &[("x-graphql-orm-wire-switch", "invalid")],
    );
    assert_eq!(
        invalid_replacement["data"]["version"], "v2",
        "an invalid candidate must preserve the last known good graph"
    );

    router.stop();
}

#[test]
fn public_static_router_serves_only_after_complete_preparation() {
    let products = LoopbackSubgraph::start(SubgraphKind::Products);
    let reviews = LoopbackSubgraph::start(SubgraphKind::Reviews);
    let products_sdl = LoopbackSdl::start(PRODUCTS_V1);
    let reviews_sdl = LoopbackSdl::start(REVIEWS);
    let mut router = TestRouter::spawn_static(
        products.endpoint(),
        reviews.endpoint(),
        products_sdl.endpoint(),
        reviews_sdl.endpoint(),
    );
    router.wait_until_ready();

    assert_eq!(router.probe("/health"), 200);
    assert_eq!(router.probe("/readiness"), 200);
    assert_eq!(router.probe("/graphql"), 404);
    assert_eq!(products_sdl.fetches(), 1);
    assert_eq!(reviews_sdl.fetches(), 1);

    let one_subgraph = router.graphql_at(
        "/api/graphql",
        json!({"query": "query { product(id: \"p1\") { id name } }"}),
        &[("x-approved", "yes"), ("x-blocked", "no")],
    );
    assert_eq!(one_subgraph["data"]["product"]["name"], "desk");
    assert!(products.saw_approved_header());
    assert!(!products.saw_blocked_header());

    let federated = router.graphql_at(
        "/api/graphql",
        json!({"query": "query { product(id: \"p1\") { id reviews { body } } }"}),
        &[],
    );
    assert_eq!(
        federated["data"]["product"]["reviews"][0]["body"],
        "excellent"
    );

    let mutation = router.graphql_at(
        "/api/graphql",
        json!({"query": "mutation { renameProduct(id: \"p1\", name: \"renamed\") { id name } }"}),
        &[],
    );
    assert_eq!(mutation["data"]["renameProduct"]["name"], "renamed");

    let downstream_error = router.graphql_at(
        "/api/graphql",
        json!({"query": "query { product(id: \"failure\") { id name } }"}),
        &[],
    );
    assert_eq!(downstream_error["errors"][0]["path"], json!(["product"]));
    assert_eq!(
        downstream_error["errors"][0]["extensions"]["code"],
        "SUBGRAPH_FAILURE"
    );

    // Pinned engine defaults: no mutations over GET and no request batching.
    let get_mutation = raw_http(
        router.address,
        "GET",
        "/api/graphql?query=mutation%20%7B%20renameProduct(id%3A%20%22p1%22%2C%20name%3A%20%22x%22)%20%7B%20id%20%7D%20%7D",
        &[("accept", "application/json")],
        None,
    )
    .unwrap();
    let get_mutation_body = String::from_utf8_lossy(&get_mutation.body);
    assert!(
        get_mutation_body.contains("MUTATION_NOT_ALLOWED_OVER_HTTP_GET"),
        "{} {get_mutation_body}",
        get_mutation.status
    );
    let batch = serde_json::to_vec(&json!([
        {"query": "query { product(id: \"p1\") { id } }"},
        {"query": "query { product(id: \"p1\") { id } }"}
    ]))
    .unwrap();
    let batched = raw_http(router.address, "POST", "/api/graphql", &[], Some(&batch)).unwrap();
    let batched_body = String::from_utf8_lossy(&batched.body);
    assert!(
        batched.status >= 400 && !batched_body.contains("\"data\""),
        "{} {batched_body}",
        batched.status
    );

    // A request head that stalls is closed by the HTTP server's header read
    // deadline instead of holding the connection open.
    let mut slow = TcpStream::connect(router.address).unwrap();
    slow.set_read_timeout(Some(TEST_TIMEOUT)).unwrap();
    slow.write_all(b"GET /health HTTP/1.1\r\nHost: router\r\n")
        .unwrap();
    let started = Instant::now();
    let mut stalled = Vec::new();
    let _ = slow.read_to_end(&mut stalled);
    assert!(
        started.elapsed() < TEST_TIMEOUT,
        "a stalled request head must be closed by the server"
    );
    assert!(
        stalled.is_empty() || stalled.starts_with(b"HTTP/1.1 408"),
        "{}",
        String::from_utf8_lossy(&stalled)
    );

    router.stop();
}

#[test]
fn prometheus_exporter_binds_only_its_separate_scrape_address() {
    let products = LoopbackSubgraph::start(SubgraphKind::Products);
    let reviews = LoopbackSubgraph::start(SubgraphKind::Reviews);
    let products_sdl = LoopbackSdl::start(PRODUCTS_V1);
    let reviews_sdl = LoopbackSdl::start(REVIEWS);
    // The public listener binds 127.0.0.1. Reserve a port on another loopback
    // address for the exporter; Hive binds it itself, so release it first.
    let metrics = {
        let reserved = TcpListener::bind("127.0.0.2:0").expect("second loopback address");
        reserved.local_addr().unwrap()
    };
    let child = Command::new(env::current_exe().expect("test executable path"))
        .args([
            "--exact",
            "federation::wire_tests::router_child_entrypoint",
            "--nocapture",
        ])
        .env(CHILD_ENV, "static")
        .env(PRODUCTS_ENDPOINT_ENV, products.endpoint())
        .env(REVIEWS_ENDPOINT_ENV, reviews.endpoint())
        .env(PRODUCTS_SDL_ENDPOINT_ENV, products_sdl.endpoint())
        .env(REVIEWS_SDL_ENDPOINT_ENV, reviews_sdl.endpoint())
        .env(PROMETHEUS_LISTENER_ENV, metrics.to_string())
        .env("RUST_TEST_THREADS", "1")
        .env("RUST_LOG", "error")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("metrics router child should start");
    let mut router = TestRouter::from_child(child, false).0;
    router.wait_until_ready();
    assert_eq!(
        router.address.ip(),
        "127.0.0.1".parse::<std::net::IpAddr>().unwrap()
    );

    let literal = "metrics-literal-7f3a";
    let response = router.graphql_at(
        "/api/graphql",
        json!({"query": format!("query {{ product(id: \"{literal}\") {{ id }} }}")}),
        &[("x-approved", "metrics-header-9c1e")],
    );
    assert!(response.get("data").is_some(), "{response}");

    let mut scrape = None;
    wait_until("Prometheus scrape", || {
        scrape = raw_http(metrics, "GET", "/internal/metrics", &[], None)
            .ok()
            .filter(|response| response.status == 200);
        scrape.is_some()
    });
    let scrape = scrape.unwrap();
    let body = String::from_utf8_lossy(&scrape.body);
    assert!(body.contains("# TYPE"), "{body}");
    for secret in [literal, "metrics-header-9c1e", "schema-secret"] {
        assert!(
            !body.contains(secret),
            "the exporter must not publish request or credential text: {secret}"
        );
    }

    // The exporter is not on the public IP address, and the public listener
    // does not serve the scrape path.
    let public_ip_metrics = std::net::SocketAddr::new(router.address.ip(), metrics.port());
    assert!(
        raw_http(public_ip_metrics, "GET", "/internal/metrics", &[], None).is_err(),
        "nothing listens on the public IP address at the exporter port"
    );
    assert_ne!(router.probe("/internal/metrics"), 200);
    router.stop();
}

#[test]
fn prepared_router_gracefully_drains_and_releases_its_listener() {
    let products = LoopbackSubgraph::start(SubgraphKind::Products);
    let reviews = LoopbackSubgraph::start(SubgraphKind::Reviews);
    let products_sdl = LoopbackSdl::start(PRODUCTS_V1);
    let reviews_sdl = LoopbackSdl::start(REVIEWS);
    let mut router = TestRouter::spawn_static_mode(
        "graceful",
        products.endpoint(),
        reviews.endpoint(),
        products_sdl.endpoint(),
        reviews_sdl.endpoint(),
    );
    router.wait_until_ready();
    let response = router.graphql_at(
        "/api/graphql",
        json!({"query": "query { product(id: \"p1\") { id name } }"}),
        &[],
    );
    assert_eq!(response["data"]["product"]["name"], "desk");
    let status = lock_child(&router.child)
        .as_mut()
        .unwrap()
        .wait()
        .expect("graceful child should exit");
    assert!(status.success());
    lock_child(&router.child).take();
    assert!(TcpStream::connect_timeout(&router.address, Duration::from_millis(100)).is_err());
}

#[test]
fn public_polling_router_atomically_reloads_and_preserves_executable_lkg() {
    let products = LoopbackSubgraph::start(SubgraphKind::Products);
    let reviews = LoopbackSubgraph::start(SubgraphKind::Reviews);
    let products_sdl = LoopbackSdl::start(PRODUCTS_V1);
    let reviews_sdl = LoopbackSdl::start(REVIEWS);
    let mut router = TestRouter::spawn_polling(
        products.endpoint(),
        reviews.endpoint(),
        products_sdl.endpoint(),
        reviews_sdl.endpoint(),
    );
    router.wait_until_ready();

    wait_until("conditional schema poll", || {
        products_sdl.conditional_fetches() > 0 && reviews_sdl.conditional_fetches() > 0
    });

    let router_for_slow_request = router.clone();
    let slow_request = thread::spawn(move || {
        router_for_slow_request.graphql_at(
            "/api/graphql",
            json!({"query": "query { product(id: \"slow\") { id name } }"}),
            &[],
        )
    });
    products.wait_for_slow_request();

    products_sdl.set(PRODUCTS_V2, "products-v2", 200);
    let replacement = wait_for_graphql(|| {
        router.graphql_at("/api/graphql", json!({"query": "query { version }"}), &[])
    });
    assert_eq!(replacement["data"]["version"], "v2");

    products.release_slow_request();
    let slow_result = slow_request
        .join()
        .expect("slow request thread should not panic");
    assert_eq!(slow_result["data"]["product"]["id"], "slow");

    let before_rejection = products_sdl.fetches();
    products_sdl.set(CONFLICTING_REVIEWS, "products-invalid", 200);
    wait_until("incompatible schema poll", || {
        products_sdl.fetches() > before_rejection
    });
    thread::sleep(Duration::from_millis(50));
    let retained_after_rejection =
        router.graphql_at("/api/graphql", json!({"query": "query { version }"}), &[]);
    assert_eq!(retained_after_rejection["data"]["version"], "v2");

    let before_outage = products_sdl.fetches();
    products_sdl.set(PRODUCTS_V2, "products-unavailable", 503);
    wait_until("unavailable schema poll", || {
        products_sdl.fetches() > before_outage
    });
    let retained_during_outage =
        router.graphql_at("/api/graphql", json!({"query": "query { version }"}), &[]);
    assert_eq!(retained_during_outage["data"]["version"], "v2");

    products_sdl.set(PRODUCTS_V2, "products-recovered", 200);
    wait_until("schema recovery and new conditional ETag", || {
        products_sdl.conditional_fetches() >= 2
    });
    let recovered = router.graphql_at("/api/graphql", json!({"query": "query { version }"}), &[]);
    assert_eq!(recovered["data"]["version"], "v2");

    router.stop();
}

#[test]
fn authenticated_admin_registration_binds_identity_destinations_and_restart_state() {
    let products = LoopbackSubgraph::start(SubgraphKind::Products);
    let reviews = LoopbackSubgraph::start(SubgraphKind::Reviews);
    let products_sdl = LoopbackSdl::start(PRODUCTS_V1);
    let reviews_sdl = LoopbackSdl::start(REVIEWS);
    let products_protocol = LoopbackProtocol::start(protocol_descriptor(
        "products",
        products.endpoint(),
        products_sdl.endpoint(),
        authenticated_product_operations(false),
    ));
    let reviews_protocol = LoopbackProtocol::start(protocol_descriptor(
        "reviews",
        reviews.endpoint(),
        reviews_sdl.endpoint(),
        Vec::new(),
    ));
    let (mut router, admin_address) = TestRouter::spawn_admin(
        products.endpoint(),
        reviews.endpoint(),
        products_sdl.endpoint(),
        reviews_sdl.endpoint(),
        products_protocol.endpoint(),
        reviews_protocol.endpoint(),
    );
    router.wait_until_ready();

    let before = router.graphql_at(
        "/api/graphql",
        json!({"query": "query { product(id: \"p1\") { id reviews { body } } }"}),
        &[("authorization", "Bearer product-events")],
    );
    assert!(before["errors"].is_array());

    assert_eq!(
        raw_http(admin_address, "GET", "/_router/status", &[], None)
            .unwrap()
            .status,
        401
    );
    assert_eq!(
        raw_http(
            admin_address,
            "GET",
            "/_router/status",
            &[("authorization", "Bearer reviews-register")],
            None,
        )
        .unwrap()
        .status,
        403
    );

    let registration = serde_json::to_vec(&json!({
        "name": "reviews",
        "metadataUrl": reviews_protocol.endpoint(),
    }))
    .unwrap();
    let wrong_identity = raw_http(
        admin_address,
        "POST",
        "/_router/subgraphs",
        &[("authorization", "Bearer wrong-register")],
        Some(&registration),
    )
    .unwrap();
    assert_eq!(wrong_identity.status, 403);
    assert!(!String::from_utf8_lossy(&wrong_identity.body).contains("wrong-register"));

    let admitted = raw_http(
        admin_address,
        "POST",
        "/_router/subgraphs",
        &[("authorization", "Bearer reviews-register")],
        Some(&registration),
    )
    .unwrap();
    assert_eq!(admitted.status, 201, "{:?}", admitted.body);
    let admitted_body: Value = serde_json::from_slice(&admitted.body).unwrap();
    assert_eq!(admitted_body["state"], "active");

    let federated = router.graphql_at(
        "/api/graphql",
        json!({"query": "query { product(id: \"p1\") { id reviews { body } } }"}),
        &[("authorization", "Bearer product-events")],
    );
    assert_eq!(
        federated["data"]["product"]["reviews"][0]["body"],
        "excellent"
    );

    let status = raw_http(
        admin_address,
        "GET",
        "/_router/status",
        &[("authorization", "Bearer status")],
        None,
    )
    .unwrap();
    assert_eq!(status.status, 200);
    let status_text = String::from_utf8(status.body).unwrap();
    assert!(status_text.contains("\"source\":\"dynamic\""));
    for forbidden in [
        "schema-secret",
        "reviews-register",
        "/.well-known/graphql-router",
        reviews.endpoint().as_str(),
    ] {
        assert!(
            !status_text.contains(forbidden),
            "status leaked {forbidden}"
        );
    }
    let metrics = raw_http(
        admin_address,
        "GET",
        "/_router/metrics",
        &[("authorization", "Bearer admin")],
        None,
    )
    .unwrap();
    assert_eq!(metrics.status, 200);
    let metrics: Value = serde_json::from_slice(&metrics.body).unwrap();
    assert!(metrics["router_graphql_requests_total"].as_u64().unwrap() >= 2);
    assert!(metrics["router_subgraph_requests_total"].as_u64().unwrap() >= 2);
    assert_eq!(metrics["router_websocket_rejections_total"], 0);
    assert_eq!(metrics["router_websocket_send_failures_total"], 0);
    // A router without byte routes reports none of their keys.
    assert!(
        metrics
            .as_object()
            .unwrap()
            .keys()
            .all(|key| !key.starts_with("router_byte_route_"))
    );
    assert!(
        metrics["router_composition_success_total"]
            .as_u64()
            .unwrap()
            >= 2
    );
    assert_eq!(metrics["router_subgraph_health"]["reviews"], 1);

    let duplicate = raw_http(
        admin_address,
        "POST",
        "/_router/subgraphs",
        &[("authorization", "Bearer reviews-register")],
        Some(&registration),
    )
    .unwrap();
    assert_eq!(duplicate.status, 403);

    // Static configuration belongs to the operator; the administrative
    // surface may remove only process-local dynamic registrations.
    let static_removal = raw_http(
        admin_address,
        "DELETE",
        "/_router/subgraphs/products",
        &[("authorization", "Bearer admin")],
        None,
    )
    .unwrap();
    assert_eq!(static_removal.status, 409);
    let static_still_served = router.graphql_at(
        "/api/graphql",
        json!({"query": "query { product(id: \"p1\") { id reviews { body } } }"}),
        &[("authorization", "Bearer product-events")],
    );
    assert_eq!(static_still_served["data"]["product"]["id"], "p1");

    let removed = raw_http(
        admin_address,
        "DELETE",
        "/_router/subgraphs/reviews",
        &[("authorization", "Bearer admin")],
        None,
    )
    .unwrap();
    assert_eq!(removed.status, 200);
    let after_removal = router.graphql_at(
        "/api/graphql",
        json!({"query": "query { product(id: \"p1\") { reviews { body } } }"}),
        &[("authorization", "Bearer product-events")],
    );
    assert!(after_removal["errors"].is_array());

    let malicious = protocol_descriptor(
        "reviews",
        "http://169.254.169.254/latest/meta-data".to_owned(),
        reviews_sdl.endpoint(),
        Vec::new(),
    );
    reviews_protocol.set(malicious, "malicious-override", 200);
    let rejected_override = raw_http(
        admin_address,
        "POST",
        "/_router/subgraphs",
        &[("authorization", "Bearer reviews-register")],
        Some(&registration),
    )
    .unwrap();
    assert_eq!(rejected_override.status, 403);
    let rejected_status = raw_http(
        admin_address,
        "GET",
        "/_router/status",
        &[("authorization", "Bearer status")],
        None,
    )
    .unwrap();
    let rejected_status: Value = serde_json::from_slice(&rejected_status.body).unwrap();
    let reviews_status = rejected_status["subgraphs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|subgraph| subgraph["name"] == "reviews")
        .unwrap();
    assert_eq!(reviews_status["state"], "rejected");
    assert_eq!(reviews_status["active"], false);
    assert!(reviews_status["lastError"].as_str().is_some());
    assert!(!reviews_status.to_string().contains("169.254.169.254"));

    let mut wrong_descriptor_identity = protocol_descriptor(
        "reviews",
        reviews.endpoint(),
        reviews_sdl.endpoint(),
        Vec::new(),
    );
    wrong_descriptor_identity.subgraph.id =
        SubgraphId::try_from("other-service".to_owned()).unwrap();
    wrong_descriptor_identity.fingerprints.combined =
        wrong_descriptor_identity.combined_fingerprint();
    reviews_protocol.set(wrong_descriptor_identity, "wrong-identity", 200);
    let rejected_identity = raw_http(
        admin_address,
        "POST",
        "/_router/subgraphs",
        &[("authorization", "Bearer reviews-register")],
        Some(&registration),
    )
    .unwrap();
    assert_eq!(rejected_identity.status, 403);

    let mut incompatible = protocol_descriptor(
        "reviews",
        reviews.endpoint(),
        reviews_sdl.endpoint(),
        Vec::new(),
    );
    incompatible.protocol_version.major = 2;
    reviews_protocol.set(incompatible, "incompatible-major", 200);
    let rejected_protocol = raw_http(
        admin_address,
        "POST",
        "/_router/subgraphs",
        &[("authorization", "Bearer reviews-register")],
        Some(&registration),
    )
    .unwrap();
    assert_eq!(rejected_protocol.status, 403);

    reviews_protocol.set_raw("x".repeat(2 * 1024 * 1024 + 1), "oversized", 200);
    let rejected_oversized = raw_http(
        admin_address,
        "POST",
        "/_router/subgraphs",
        &[("authorization", "Bearer reviews-register")],
        Some(&registration),
    )
    .unwrap();
    assert_eq!(rejected_oversized.status, 403);

    let sdl_fetches_before_redirect = reviews_sdl.fetches();
    reviews_protocol.redirect_to(reviews_sdl.endpoint());
    let rejected_redirect = raw_http(
        admin_address,
        "POST",
        "/_router/subgraphs",
        &[("authorization", "Bearer reviews-register")],
        Some(&registration),
    )
    .unwrap();
    assert_eq!(rejected_redirect.status, 403);
    assert_eq!(
        reviews_sdl.fetches(),
        sdl_fetches_before_redirect,
        "the metadata client must not follow a redirect to another allowed destination"
    );

    reviews_protocol.set(
        protocol_descriptor(
            "reviews",
            reviews.endpoint(),
            reviews_sdl.endpoint(),
            Vec::new(),
        ),
        "valid-again",
        200,
    );

    let readmitted = raw_http(
        admin_address,
        "POST",
        "/_router/subgraphs",
        &[("authorization", "Bearer reviews-register")],
        Some(&registration),
    )
    .unwrap();
    assert_eq!(readmitted.status, 201);

    let opens_before_limits = products.opens() + reviews.opens();
    let (field_status, too_many_fields) = router.graphql_response_at(
        "/api/graphql",
        json!({"query": "query { product(id: \"p1\") { id a: name b: name c: name d: name e: name } }"}),
        &[("authorization", "Bearer product-events")],
    );
    assert_eq!(field_status, 400);
    assert_eq!(
        too_many_fields["errors"][0]["extensions"]["code"],
        "OPERATION_LIMIT_EXCEEDED"
    );

    let (alias_status, too_many_aliases) = router.graphql_response_at(
        "/api/graphql",
        json!({"query": "query { product(id: \"p1\") { a:id b:id c:id d:id e:id f:id g:id h:id i:id j:id k:id l:id m:id } }"}),
        &[("authorization", "Bearer product-events")],
    );
    assert!(matches!(alias_status, 200 | 400));
    assert!(too_many_aliases["errors"].is_array());

    let (depth_status, too_deep) = router.graphql_response_at(
        "/api/graphql",
        json!({"query": "query { __schema { types { fields { type { ofType { name } } } } } }"}),
        &[("authorization", "Bearer product-events")],
    );
    assert!(matches!(depth_status, 200 | 400));
    assert!(too_deep["errors"].is_array());

    let token_heavy_query = format!("query {{ product(id: \"p1\") {{ {} }} }}", "id ".repeat(90));
    let (token_status, too_many_tokens) = router.graphql_response_at(
        "/api/graphql",
        json!({"query": token_heavy_query}),
        &[("authorization", "Bearer product-events")],
    );
    assert!(matches!(token_status, 200 | 400));
    assert!(too_many_tokens["errors"].is_array());
    assert_eq!(
        products.opens() + reviews.opens(),
        opens_before_limits,
        "request/parser/depth/field limits must reject before downstream work"
    );

    let public_oversized = raw_http(
        router.address,
        "POST",
        "/api/graphql",
        &[("authorization", "Bearer product-events")],
        Some(&vec![b' '; 513]),
    )
    .unwrap();
    assert_eq!(public_oversized.status, 413);

    let oversized = vec![b' '; 257];
    let bounded = raw_http(
        admin_address,
        "POST",
        "/_router/subgraphs",
        &[("authorization", "Bearer reviews-register")],
        Some(&oversized),
    )
    .unwrap();
    assert!(matches!(bounded.status, 400 | 413));

    router.stop();

    let (mut restarted, restarted_admin) = TestRouter::spawn_admin(
        products.endpoint(),
        reviews.endpoint(),
        products_sdl.endpoint(),
        reviews_sdl.endpoint(),
        products_protocol.endpoint(),
        reviews_protocol.endpoint(),
    );
    restarted.wait_until_ready();
    let restart_status = raw_http(
        restarted_admin,
        "GET",
        "/_router/status",
        &[("authorization", "Bearer status")],
        None,
    )
    .unwrap();
    assert!(
        !String::from_utf8(restart_status.body)
            .unwrap()
            .contains("reviews-service")
    );
    let re_registered = raw_http(
        restarted_admin,
        "POST",
        "/_router/subgraphs",
        &[("authorization", "Bearer reviews-register")],
        Some(&registration),
    )
    .unwrap();
    assert_eq!(re_registered.status, 201);
    restarted.stop();
}

#[test]
fn authenticated_static_router_denies_before_downstream_and_expands_templates() {
    let products = LoopbackSubgraph::start(SubgraphKind::Products);
    let reviews = LoopbackSubgraph::start(SubgraphKind::Reviews);
    let products_sdl = LoopbackSdl::start(PRODUCTS_V1);
    let reviews_sdl = LoopbackSdl::start(REVIEWS);
    let product_operations = vec![
        OperationDescriptor {
            root_type: RootOperationType::Query,
            field_name: "product".to_owned(),
            arguments: vec![ArgumentDescriptor {
                name: "id".to_owned(),
                graphql_type: "ID!".to_owned(),
                required: true,
            }],
            authorization: AuthorizationRequirement::AnyScopes {
                alternatives: vec![
                    ScopeSet {
                        scopes: vec![scope("products.{id}.read")],
                    },
                    ScopeSet {
                        scopes: vec![scope("products.admin")],
                    },
                ],
            },
        },
        OperationDescriptor {
            root_type: RootOperationType::Mutation,
            field_name: "renameProduct".to_owned(),
            arguments: vec![
                ArgumentDescriptor {
                    name: "id".to_owned(),
                    graphql_type: "ID!".to_owned(),
                    required: true,
                },
                ArgumentDescriptor {
                    name: "name".to_owned(),
                    graphql_type: "String!".to_owned(),
                    required: true,
                },
            ],
            authorization: AuthorizationRequirement::AllScopes {
                scopes: vec![scope("products.write")],
            },
        },
        OperationDescriptor {
            root_type: RootOperationType::Subscription,
            field_name: "productChanged".to_owned(),
            arguments: vec![ArgumentDescriptor {
                name: "id".to_owned(),
                graphql_type: "ID!".to_owned(),
                required: true,
            }],
            authorization: AuthorizationRequirement::Public,
        },
    ];
    let products_protocol = LoopbackProtocol::start(protocol_descriptor(
        "products",
        products.endpoint(),
        products_sdl.endpoint(),
        product_operations,
    ));
    let reviews_protocol = LoopbackProtocol::start(protocol_descriptor(
        "reviews",
        reviews.endpoint(),
        reviews_sdl.endpoint(),
        Vec::new(),
    ));
    let mut router = TestRouter::spawn_authenticated(
        products.endpoint(),
        reviews.endpoint(),
        products_sdl.endpoint(),
        reviews_sdl.endpoint(),
        products_protocol.endpoint(),
        reviews_protocol.endpoint(),
    );
    router.wait_until_ready();

    let opens_before = products.opens() + reviews.opens();
    let unauthenticated = router.graphql_at(
        "/api/graphql",
        json!({"query": "query { product(id: \"p1\") { id } }"}),
        &[],
    );
    assert_eq!(
        unauthenticated["errors"][0]["extensions"]["code"],
        "UNAUTHENTICATED"
    );
    assert_eq!(products.opens() + reviews.opens(), opens_before);

    let malformed_body = serde_json::to_vec(&json!({
        "query": "query { product(id: \"p1\") { id } }"
    }))
    .unwrap();
    let malformed = raw_http(
        router.address,
        "POST",
        "/api/graphql",
        &[("authorization", "Basic invalid")],
        Some(&malformed_body),
    )
    .unwrap();
    assert_eq!(malformed.status, 401);
    assert_eq!(products.opens() + reviews.opens(), opens_before);

    let excluded = router.graphql_at(
        "/api/graphql",
        json!({
            "query": "query Read($id: ID!, $enabled: Boolean!) { product(id: $id) @include(if: $enabled) { id } }",
            "variables": {"id": "p2", "enabled": false}
        }),
        &[],
    );
    assert_eq!(excluded["data"], json!({}));

    let allowed = router.graphql_at(
        "/api/graphql",
        json!({
            "query": "query Read($id: ID!) { product(id: $id) { ...ProductIdentity } } fragment ProductIdentity on Product { id name }",
            "variables": {"id": "p1"}
        }),
        &[("authorization", "Bearer product-p1")],
    );
    assert_eq!(allowed["data"]["product"]["id"], "p1");
    assert!(products.saw_bearer_header());

    let defaulted = router.graphql_at(
        "/api/graphql",
        json!({
            "query": "query Read($id: ID! = \"p1\") { product(id: $id) { id } }"
        }),
        &[("authorization", "Bearer product-p1")],
    );
    assert_eq!(defaulted["data"]["product"]["id"], "p1", "{defaulted}");

    let opens_before_variable_denial = products.opens() + reviews.opens();
    let variable_denied = router.graphql_at(
        "/api/graphql",
        json!({
            "query": "query Read($id: ID!) { product(id: $id) { id } }",
            "variables": {"id": "p2"},
            "extensions": {"graphqlOrmRouterVariables": {"id": "p1"}}
        }),
        &[("authorization", "Bearer product-p1")],
    );
    assert_eq!(
        variable_denied["errors"][0]["extensions"]["code"], "FORBIDDEN",
        "{variable_denied}"
    );
    assert_eq!(
        products.opens() + reviews.opens(),
        opens_before_variable_denial
    );

    let inline_excluded = router.graphql_at(
        "/api/graphql",
        json!({
            "query": "query Read($enabled: Boolean!) { ... @include(if: $enabled) { product(id: \"p2\") { id } } }",
            "variables": {"enabled": false}
        }),
        &[],
    );
    assert_eq!(inline_excluded["data"], json!({}));

    let hierarchical = router.graphql_at(
        "/api/graphql",
        json!({"query": "query { product(id: \"p2\") { id } }"}),
        &[("authorization", "Bearer product-prefix")],
    );
    assert_eq!(hierarchical["data"]["product"]["id"], "p1");

    let opens_before_multi = products.opens() + reviews.opens();
    let mixed_arguments = router.graphql_at(
        "/api/graphql",
        json!({
            "query": "query { first: product(id: \"p1\") { id } second: product(id: \"p2\") { id } }"
        }),
        &[("authorization", "Bearer product-p1")],
    );
    assert_eq!(
        mixed_arguments["errors"][0]["extensions"]["code"],
        "FORBIDDEN"
    );
    assert_eq!(products.opens() + reviews.opens(), opens_before_multi);

    let admin = router.graphql_at(
        "/api/graphql",
        json!({"query": "query { product(id: \"p2\") { id } }"}),
        &[("authorization", "Bearer product-admin")],
    );
    assert_eq!(admin["data"]["product"]["id"], "p1");

    let mutation = router.graphql_at(
        "/api/graphql",
        json!({"query": "mutation { renameProduct(id: \"p1\", name: \"renamed\") { name } }"}),
        &[("authorization", "Bearer product-writer")],
    );
    assert_eq!(mutation["data"]["renameProduct"]["name"], "renamed");

    // A template argument is one plain segment. Separators and wildcards are
    // refused rather than spliced into the required scope.
    let opens_before_segments = products.opens() + reviews.opens();
    for (query, variables) in [
        ("query { product(id: \"p1.x\") { id } }", json!({})),
        (
            "query Read($id: ID!) { product(id: $id) { id } }",
            json!({"id": "p1.x"}),
        ),
    ] {
        let spliced = router.graphql_at(
            "/api/graphql",
            json!({"query": query, "variables": variables}),
            &[("authorization", "Bearer product-nested")],
        );
        assert_eq!(
            spliced["errors"][0]["extensions"]["code"], "FORBIDDEN",
            "{spliced}"
        );
    }
    assert_eq!(products.opens() + reviews.opens(), opens_before_segments);

    // Federation's planner-owned root fields are not client fields. Hive's
    // client API omits them, so validation refuses them before preflight or
    // any downstream work, for anonymous and authorized callers. Preflight
    // would also refuse them; see the catalogue unit test.
    let opens_before_planner_fields = products.opens() + reviews.opens();
    for headers in [&[][..], &[("authorization", "Bearer product-admin")][..]] {
        for query in [
            "{ _entities(representations: [{__typename: \"Product\", id: \"p1\"}]) { __typename } }",
            "{ _service { sdl } }",
        ] {
            let (_, refused) =
                router.graphql_response_at("/api/graphql", json!({"query": query}), headers);
            assert!(refused["data"].is_null(), "{query}: {refused}");
            assert_eq!(
                refused["errors"][0]["extensions"]["code"], "FieldsOnCorrectType",
                "{query}: {refused}"
            );
        }
    }
    assert_eq!(
        products.opens() + reviews.opens(),
        opens_before_planner_fields
    );

    // Introspection is off unless the router configuration enables it.
    for headers in [&[][..], &[("authorization", "Bearer product-admin")][..]] {
        for query in [
            "{ __schema { queryType { name } } }",
            "query { ...Root } fragment Root on Query { __type(name: \"Query\") { name } }",
        ] {
            let (_, introspection) =
                router.graphql_response_at("/api/graphql", json!({"query": query}), headers);
            assert!(
                introspection["data"].is_null(),
                "introspection answered: {introspection}"
            );
            assert_eq!(
                introspection["errors"][0]["extensions"]["code"], "INTROSPECTION_DISABLED",
                "{introspection}"
            );
        }
    }
    let typename = router.graphql_at("/api/graphql", json!({"query": "{ __typename }"}), &[]);
    assert_eq!(typename["data"]["__typename"], "Query", "{typename}");

    router.stop();
}

#[test]
fn authenticated_websocket_uses_provider_clock_and_rejects_clock_failure() {
    let products = LoopbackSubgraph::start(SubgraphKind::Products);
    let reviews = LoopbackSubgraph::start(SubgraphKind::Reviews);
    let products_sdl = LoopbackSdl::start(PRODUCTS_V1);
    let reviews_sdl = LoopbackSdl::start(REVIEWS);
    let products_protocol = LoopbackProtocol::start(protocol_descriptor(
        "products",
        products.endpoint(),
        products_sdl.endpoint(),
        authenticated_product_operations(false),
    ));
    let reviews_protocol = LoopbackProtocol::start(protocol_descriptor(
        "reviews",
        reviews.endpoint(),
        reviews_sdl.endpoint(),
        Vec::new(),
    ));
    for mode in [
        "auth-subscriptions-clock-past",
        "auth-subscriptions-clock-ahead",
        "auth-subscriptions-clock-error",
    ] {
        let mut router = TestRouter::spawn_authenticated_mode(
            mode,
            products.endpoint(),
            reviews.endpoint(),
            products_sdl.endpoint(),
            reviews_sdl.endpoint(),
            products_protocol.endpoint(),
            reviews_protocol.endpoint(),
        );
        router.wait_until_ready();
        let mut socket = TestWebSocket::connect_at(router.address, "/api/graphql");
        if mode == "auth-subscriptions-clock-error" {
            let opens_before = products.opens();
            socket.send_json(&json!({
                "type": "connection_init",
                "payload": {"authorization": "Bearer product-events"}
            }));
            assert_eq!(
                socket.wait_for_close_before(Instant::now() + TEST_TIMEOUT),
                1011
            );
            assert_eq!(products.opens(), opens_before);
        } else {
            socket.connection_init_bearer("product-events");
            socket.subscribe(
                "clock",
                r#"subscription { productChanged(id: "p1") { id } }"#,
            );
            assert_eq!(
                socket.wait_for_operation_message("clock", "next")["payload"]["data"]["productChanged"]
                    ["id"],
                "p1"
            );
            assert_eq!(
                socket.wait_for_close_before(Instant::now() + TEST_TIMEOUT),
                4401
            );
        }
        router.stop();
    }
}

#[test]
fn authenticated_public_websocket_enforces_lifecycle_and_routes_upstream_websocket() {
    let products = LoopbackSubgraph::start(SubgraphKind::Products);
    let reviews = LoopbackSubgraph::start(SubgraphKind::Reviews);
    let products_sdl = LoopbackSdl::start(PRODUCTS_V1);
    let reviews_sdl = LoopbackSdl::start(REVIEWS);
    let products_protocol = LoopbackProtocol::start(protocol_descriptor(
        "products",
        products.endpoint(),
        products_sdl.endpoint(),
        vec![
            OperationDescriptor {
                root_type: RootOperationType::Query,
                field_name: "product".to_owned(),
                arguments: vec![ArgumentDescriptor {
                    name: "id".to_owned(),
                    graphql_type: "ID!".to_owned(),
                    required: true,
                }],
                authorization: AuthorizationRequirement::Authenticated,
            },
            OperationDescriptor {
                root_type: RootOperationType::Mutation,
                field_name: "renameProduct".to_owned(),
                arguments: vec![
                    ArgumentDescriptor {
                        name: "id".to_owned(),
                        graphql_type: "ID!".to_owned(),
                        required: true,
                    },
                    ArgumentDescriptor {
                        name: "name".to_owned(),
                        graphql_type: "String!".to_owned(),
                        required: true,
                    },
                ],
                authorization: AuthorizationRequirement::Authenticated,
            },
            OperationDescriptor {
                root_type: RootOperationType::Subscription,
                field_name: "productChanged".to_owned(),
                arguments: vec![ArgumentDescriptor {
                    name: "id".to_owned(),
                    graphql_type: "ID!".to_owned(),
                    required: true,
                }],
                authorization: AuthorizationRequirement::AllScopes {
                    scopes: vec![scope("products.{id}.events")],
                },
            },
        ],
    ));
    let reviews_protocol = LoopbackProtocol::start(protocol_descriptor(
        "reviews",
        reviews.endpoint(),
        reviews_sdl.endpoint(),
        Vec::new(),
    ));
    let mut router = TestRouter::spawn_authenticated_subscriptions(
        products.endpoint(),
        reviews.endpoint(),
        products_sdl.endpoint(),
        reviews_sdl.endpoint(),
        products_protocol.endpoint(),
        reviews_protocol.endpoint(),
    );
    router.wait_until_ready();

    let opens_before = products.opens();
    let mut timed_out = TestWebSocket::connect_at(router.address, "/api/graphql");
    assert_eq!(timed_out.wait_for_close(), 4408);

    let mut invalid = TestWebSocket::connect_at(router.address, "/api/graphql");
    invalid.send_json(&json!({
        "type": "connection_init",
        "payload": {"authorization": "Bearer rejected"}
    }));
    assert_eq!(invalid.wait_for_close(), 4401);
    assert_eq!(products.opens(), opens_before);

    let mut denied = TestWebSocket::connect_at(router.address, "/api/graphql");
    denied.connection_init_bearer("product-events");
    denied.subscribe_with_variables_and_extensions(
        "denied",
        "subscription EndpointEvents($Id: ID!) { productChanged(id: $Id) { id name } }",
        json!({"Id": "p2"}),
        json!({"graphqlOrmRouterVariables": {"Id": "p1"}}),
    );
    let denial = denied.wait_for_operation_message("denied", "next");
    assert_eq!(
        denial["payload"]["errors"][0]["extensions"]["code"], "FORBIDDEN",
        "{denial}"
    );
    assert_eq!(products.opens(), opens_before);
    denied.close();

    let mut first = TestWebSocket::connect_at(router.address, "/api/graphql");
    first.connection_init_bearer("product-events");
    first.subscribe_with_variables(
        "filtered",
        "subscription EndpointEvents($Id: ID!) { productChanged(id: $Id) { id name } }",
        json!({"Id": "p1"}),
    );
    let first_event = first.wait_for_operation_message("filtered", "next");
    assert_eq!(
        first_event["payload"]["data"]["productChanged"]["name"], "websocket",
        "{first_event}"
    );
    assert!(products.saw_bearer_header());

    // An unauthenticated socket holds only the pre-authentication budget, so
    // it cannot take the authenticated capacity the second caller needs.
    let mut pending = TestWebSocket::connect_at(router.address, "/api/graphql");
    let mut second = TestWebSocket::connect_at(router.address, "/api/graphql");
    second.connection_init_bearer("product-events");
    pending.send_json(&json!({
        "type": "connection_init",
        "payload": {"authorization": "Bearer product-events"}
    }));
    assert_eq!(
        pending.wait_for_close(),
        1013,
        "authenticated capacity is charged only after the credential validates"
    );
    second.subscribe(
        "second",
        "subscription { productChanged(id: \"p1\") { id } }",
    );
    assert_eq!(
        second.wait_for_operation_message("second", "next")["payload"]["data"]["productChanged"]["id"],
        "p1"
    );
    assert_eq!(
        websocket_upgrade_status(router.address, "/api/graphql"),
        503,
        "the process-wide WebSocket connection bound must reject before upgrade"
    );

    first.subscribe(
        "other",
        "subscription { productChanged(id: \"p1\") { id } }",
    );
    let _ = first.wait_for_operation_message("other", "next");
    first.subscribe(
        "over-limit",
        "subscription { productChanged(id: \"p1\") { id } }",
    );
    let limited = first.wait_for_operation_message("over-limit", "error");
    assert_eq!(
        limited["payload"][0]["extensions"]["code"],
        "SUBSCRIPTION_LIMIT_EXCEEDED"
    );

    first.close();
    second.close();
    thread::sleep(Duration::from_millis(50));

    let mut reconnected = TestWebSocket::connect_at(router.address, "/api/graphql");
    reconnected.connection_init_bearer("product-events");
    reconnected.subscribe(
        "fresh",
        "subscription { productChanged(id: \"p1\") { id } }",
    );
    assert_eq!(
        reconnected.wait_for_operation_message("fresh", "next")["id"],
        "fresh"
    );
    reconnected.close();
    thread::sleep(Duration::from_millis(50));

    let mut idle_first = TestWebSocket::connect_at(router.address, "/api/graphql");
    let mut idle_second = TestWebSocket::connect_at(router.address, "/api/graphql");
    assert_eq!(
        websocket_upgrade_status(router.address, "/api/graphql"),
        503,
        "unauthenticated sockets are bounded by their own pre-authentication budget"
    );
    idle_first.close();
    idle_second.close();
    thread::sleep(Duration::from_millis(50));

    for index in 0..8 {
        let mut churned = TestWebSocket::connect_at(router.address, "/api/graphql");
        churned.connection_init_bearer("product-events");
        let id = format!("churn-{index}");
        churned.subscribe(&id, "subscription { productChanged(id: \"p1\") { id } }");
        assert_eq!(churned.wait_for_operation_message(&id, "next")["id"], id);
        churned.close();
        thread::sleep(Duration::from_millis(10));
    }

    let mut oversized = TestWebSocket::connect_at(router.address, "/api/graphql");
    oversized.connection_init_bearer("product-events");
    oversized.send_oversized_text_frame();
    oversized.wait_for_disconnect_without_close_code(1011);
    thread::sleep(Duration::from_millis(50));

    let mut failing = TestWebSocket::connect_at(router.address, "/api/graphql");
    failing.connection_init_bearer("product-events");
    failing.subscribe(
        "live-events",
        "subscription ProductEvents { productChanged(id: \"p1\") { id } }",
    );
    assert_eq!(
        failing.wait_for_operation_message("live-events", "next")["id"],
        "live-events"
    );
    failing.subscribe(
        "rename-once",
        "mutation RenameProduct { renameProduct(id: \"p1\", name: \"renamed\") { id name } }",
    );
    assert_eq!(
        failing.wait_for_operation_message("rename-once", "next")["id"],
        "rename-once"
    );
    assert_eq!(
        failing.wait_for_operation_message("rename-once", "complete")["id"],
        "rename-once",
        "a one-shot mutation must complete without closing or retiring its sibling subscription"
    );
    failing.subscribe(
        "failing-events",
        "subscription { productChanged(id: \"failure\") { id } }",
    );
    let failure = failing.wait_for_operation_failure("failing-events");
    assert_eq!(failure["id"], "failing-events");
    failing.subscribe(
        "rename-after-failure",
        "mutation RenameProduct { renameProduct(id: \"p1\", name: \"renamed\") { id } }",
    );
    assert_eq!(
        failing.wait_for_operation_message("rename-after-failure", "next")["id"],
        "rename-after-failure",
        "an upstream subscription failure must not close the downstream socket"
    );
    assert_eq!(
        failing.wait_for_operation_message("rename-after-failure", "complete")["id"],
        "rename-after-failure"
    );
    failing.complete("live-events");
    failing.close();
    thread::sleep(Duration::from_millis(50));

    let healthy_after_failure = router.graphql_at(
        "/api/graphql",
        json!({"query": "query { product(id: \"p1\") { id } }"}),
        &[("authorization", "Bearer product-events")],
    );
    assert_eq!(healthy_after_failure["data"]["product"]["id"], "p1");

    let mut expiring = TestWebSocket::connect_at(router.address, "/api/graphql");
    expiring.connection_init_bearer("short-lived");
    assert_eq!(expiring.wait_for_close(), 4401);

    router.stop();
}

#[test]
fn public_http_subscription_streams_are_refused_before_upstream_work() {
    let products = LoopbackSubgraph::start(SubgraphKind::Products);
    let reviews = LoopbackSubgraph::start(SubgraphKind::Reviews);
    let products_sdl = LoopbackSdl::start(PRODUCTS_V2);
    let reviews_sdl = LoopbackSdl::start(REVIEWS);
    // `product` and `productChanged` are public, so an anonymous Hive-native
    // stream would open; `productChangedV2` carries a variable-bound scope.
    let operations = authenticated_product_operations(true)
        .into_iter()
        .map(|mut operation| {
            if matches!(operation.field_name.as_str(), "product" | "productChanged") {
                operation.authorization = AuthorizationRequirement::Public;
            }
            operation
        })
        .collect();
    let products_protocol = LoopbackProtocol::start(protocol_descriptor(
        "products",
        products.endpoint(),
        products_sdl.endpoint(),
        operations,
    ));
    let reviews_protocol = LoopbackProtocol::start(protocol_descriptor(
        "reviews",
        reviews.endpoint(),
        reviews_sdl.endpoint(),
        Vec::new(),
    ));
    let mut router = TestRouter::spawn_authenticated_subscriptions(
        products.endpoint(),
        reviews.endpoint(),
        products_sdl.endpoint(),
        reviews_sdl.endpoint(),
        products_protocol.endpoint(),
        reviews_protocol.endpoint(),
    );
    router.wait_until_ready();

    let public = json!({"query": "subscription Public { productChanged(id: \"p1\") { id name } }"});
    let scoped = json!({
        "query": "subscription Scoped($Id: ID!) { productChangedV2(id: $Id) { id name } }",
        "variables": {"Id": "p2"}
    });
    let bearer = [("authorization", "Bearer product-events")];
    let anonymous: &[(&str, &str)] = &[];
    let cases = [
        ("anonymous SSE", "text/event-stream", &public, anonymous),
        ("anonymous multipart", "multipart/mixed", &public, anonymous),
        ("bearer SSE", "text/event-stream", &public, &bearer[..]),
        ("bearer multipart", "multipart/mixed", &public, &bearer[..]),
        (
            "bearer SSE, variable-bound scope",
            "text/event-stream",
            &scoped,
            &bearer[..],
        ),
        (
            "bearer Apollo multipart, variable-bound scope",
            "multipart/mixed;subscriptionSpec=\"1.0\"",
            &scoped,
            &bearer[..],
        ),
        (
            "anonymous dual JSON and SSE",
            "application/json, text/event-stream",
            &public,
            anonymous,
        ),
        ("bearer JSON only", "application/json", &public, &bearer[..]),
    ];
    let opens_before = products.opens();
    for (case, accept, payload, credentials) in cases {
        let mut headers = vec![("accept", accept)];
        headers.extend_from_slice(credentials);
        let body = serde_json::to_vec(payload).unwrap();
        let response = raw_http_within(
            router.address,
            "POST",
            "/api/graphql",
            &headers,
            Some(&body),
            HTTP_STREAM_PROBE,
        )
        .unwrap_or_else(|error| {
            panic!("{case}: the router must answer with a finite refusal, not a stream: {error}")
        });
        assert_http_subscription_refused(case, &response);
        assert_eq!(
            products.opens(),
            opens_before,
            "{case}: the refusal must precede any upstream subscription"
        );
    }

    let mut get_url = Url::parse("http://router/api/graphql").unwrap();
    get_url.query_pairs_mut().append_pair(
        "query",
        "subscription { productChanged(id: \"p1\") { id name } }",
    );
    let get_path = format!("{}?{}", get_url.path(), get_url.query().unwrap());
    for (case, credentials) in [
        ("anonymous GET SSE", &[][..]),
        ("bearer GET SSE", &bearer[..]),
    ] {
        let mut headers = vec![("accept", "text/event-stream")];
        headers.extend_from_slice(credentials);
        let response = raw_http_within(
            router.address,
            "GET",
            &get_path,
            &headers,
            None,
            HTTP_STREAM_PROBE,
        )
        .unwrap_or_else(|error| {
            panic!("{case}: the router must answer with a finite refusal, not a stream: {error}")
        });
        assert_http_subscription_refused(case, &response);
    }

    // Hive also serves the GraphQL handler on every sub-path of the GraphQL
    // path (its persisted-document route). Each one must stay guarded.
    let public_body = serde_json::to_vec(&public).unwrap();
    for path in [
        "/api/graphql/",
        "/api/graphql/x",
        "/api/graphql/sha256:abc",
        "/api/graphql/x/y",
    ] {
        for (accept, credentials) in [
            ("text/event-stream", anonymous),
            ("multipart/mixed", &bearer[..]),
        ] {
            let case = format!("POST {path} with {accept}");
            let mut headers = vec![("accept", accept)];
            headers.extend_from_slice(credentials);
            let response = raw_http_within(
                router.address,
                "POST",
                path,
                &headers,
                Some(&public_body),
                HTTP_STREAM_PROBE,
            )
            .unwrap_or_else(|error| panic!("{case}: expected a finite refusal: {error}"));
            assert_http_subscription_refused(&case, &response);
        }
    }
    let mut get_selected = Url::parse("http://router/api/graphql/x").unwrap();
    get_selected
        .query_pairs_mut()
        .append_pair(
            "query",
            "query Lookup { product(id: \"p1\") { id } } \
             subscription Events { productChanged(id: \"p1\") { id } }",
        )
        .append_pair("operationName", "Events");
    for (case, path) in [
        (
            "GET operationName selecting the subscription",
            "/api/graphql",
        ),
        (
            "GET sub-path operationName selecting the subscription",
            get_selected.path(),
        ),
    ] {
        let target = format!("{path}?{}", get_selected.query().unwrap());
        let response = raw_http_within(
            router.address,
            "GET",
            &target,
            &[("accept", "text/event-stream")],
            None,
            HTTP_STREAM_PROBE,
        )
        .unwrap_or_else(|error| panic!("{case}: expected a finite refusal: {error}"));
        assert_http_subscription_refused(case, &response);
    }
    assert_eq!(
        products.opens(),
        opens_before,
        "no sub-path or GET subscription may open an upstream subscription"
    );

    // Every operation the request could select is checked, so neither the
    // first-operation default nor an operation name can reach a stream.
    let mixed = "subscription Events { productChanged(id: \"p1\") { id } } \
                 query Lookup { product(id: \"p1\") { id } }";
    for (case, operation_name) in [
        ("mixed document without operationName", Value::Null),
        ("mixed document selecting the subscription", json!("Events")),
    ] {
        let body =
            serde_json::to_vec(&json!({"query": mixed, "operationName": operation_name})).unwrap();
        let response = raw_http_within(
            router.address,
            "POST",
            "/api/graphql",
            &[("accept", "text/event-stream")],
            Some(&body),
            HTTP_STREAM_PROBE,
        )
        .unwrap_or_else(|error| panic!("{case}: expected a finite refusal: {error}"));
        assert_http_subscription_refused(case, &response);
    }
    // Hive 0.2 does not accept batched bodies; a batched subscription is
    // rejected without opening a stream.
    let batched = serde_json::to_vec(&json!([public.clone()])).unwrap();
    let response = raw_http_within(
        router.address,
        "POST",
        "/api/graphql",
        &[("accept", "text/event-stream")],
        Some(&batched),
        HTTP_STREAM_PROBE,
    )
    .expect("a batched subscription must not open a stream");
    assert!(
        !String::from_utf8_lossy(&response.body).contains("productChanged"),
        "a batched subscription must not deliver events"
    );
    assert_eq!(
        products.opens(),
        opens_before,
        "no refused request may open an upstream subscription"
    );

    // Queries and mutations keep working over HTTP, including with a
    // streaming-capable Accept header, GET, and a mixed document.
    let query = json!({"query": "query { product(id: \"p1\") { id } }"});
    assert_eq!(
        router.graphql_at("/api/graphql", query.clone(), &[])["data"]["product"]["id"],
        "p1"
    );
    assert_eq!(
        router.graphql_at(
            "/api/graphql",
            query,
            &[(
                "accept",
                "application/json, multipart/mixed, text/event-stream"
            )]
        )["data"]["product"]["id"],
        "p1"
    );
    assert_eq!(
        router.graphql_at(
            "/api/graphql",
            json!({"query": mixed, "operationName": "Lookup"}),
            &[]
        )["data"]["product"]["id"],
        "p1"
    );
    assert_eq!(
        router.graphql_at(
            "/api/graphql",
            json!({"query": "mutation { renameProduct(id: \"p1\", name: \"renamed\") { id name } }"}),
            &bearer
        )["data"]["renameProduct"]["name"],
        "renamed"
    );
    let mut get_query = Url::parse("http://router/api/graphql").unwrap();
    get_query
        .query_pairs_mut()
        .append_pair("query", "{ product(id: \"p1\") { id } }");
    let response = raw_http(
        router.address,
        "GET",
        &format!("{}?{}", get_query.path(), get_query.query().unwrap()),
        &[("accept", "application/json")],
        None,
    )
    .expect("GET query response");
    assert_eq!(response.status, 200);
    let value: Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(value["data"]["product"]["id"], "p1", "{value}");

    // The trusted private path behind the public WebSocket gateway still
    // serves subscriptions, with the gateway's controls.
    let mut websocket = TestWebSocket::connect_at(router.address, "/api/graphql");
    websocket.connection_init_bearer("product-events");
    websocket.subscribe(
        "gateway",
        "subscription { productChanged(id: \"p1\") { id name } }",
    );
    assert_eq!(
        websocket.wait_for_operation_message("gateway", "next")["payload"]["data"]["productChanged"]
            ["id"],
        "p1"
    );
    websocket.close();
    let mut anonymous = TestWebSocket::connect_at(router.address, "/api/graphql");
    anonymous.send_json(&json!({"type": "connection_init", "payload": {}}));
    assert_eq!(anonymous.wait_for_close(), 4401);

    router.stop();
}

const HTTP_STREAM_PROBE: Duration = Duration::from_secs(3);

fn assert_http_subscription_refused(case: &str, response: &HttpResponse) {
    assert_http_subscription_refused_with(
        case,
        response,
        "subscriptions are not supported over HTTP; use the graphql-transport-ws WebSocket endpoint",
    );
}

fn assert_http_subscription_refused_with(case: &str, response: &HttpResponse, message: &str) {
    let body = String::from_utf8_lossy(&response.body);
    assert_eq!(response.status, 400, "{case}: {body}");
    let value: Value = serde_json::from_slice(&response.body)
        .unwrap_or_else(|_| panic!("{case}: the refusal must be plain GraphQL JSON: {body}"));
    assert_eq!(
        value["errors"][0]["extensions"]["code"], "SUBSCRIPTIONS_NOT_SUPPORTED",
        "{case}: {value}"
    );
    assert_eq!(value["errors"][0]["message"], message, "{case}: {value}");
    assert!(value.get("data").is_none(), "{case}: {value}");
}

#[test]
fn http_subscription_without_a_gateway_is_refused_without_naming_one() {
    let products = LoopbackSubgraph::start(SubgraphKind::Products);
    let reviews = LoopbackSubgraph::start(SubgraphKind::Reviews);
    let products_sdl = LoopbackSdl::start(PRODUCTS_V1);
    let reviews_sdl = LoopbackSdl::start(REVIEWS);
    // No `subscriptions` configuration, so there is no WebSocket gateway.
    let mut router = TestRouter::spawn_static(
        products.endpoint(),
        reviews.endpoint(),
        products_sdl.endpoint(),
        reviews_sdl.endpoint(),
    );
    router.wait_until_ready();

    let body = serde_json::to_vec(&json!({
        "query": "subscription { productChanged(id: \"p1\") { id name } }"
    }))
    .unwrap();
    let opens_before = products.opens();
    for accept in [
        "text/event-stream",
        "multipart/mixed",
        "application/json",
        "application/json, text/event-stream",
    ] {
        let case = format!("no gateway, {accept}");
        let response = raw_http_within(
            router.address,
            "POST",
            "/api/graphql",
            &[("accept", accept)],
            Some(&body),
            HTTP_STREAM_PROBE,
        )
        .unwrap_or_else(|error| panic!("{case}: expected a finite refusal: {error}"));
        assert_http_subscription_refused_with(
            &case,
            &response,
            "subscriptions are not enabled on this router",
        );
    }
    assert_eq!(products.opens(), opens_before);
    assert_eq!(
        router.graphql_at(
            "/api/graphql",
            json!({"query": "query { product(id: \"p1\") { id } }"}),
            &[]
        )["data"]["product"]["id"],
        "p1"
    );
    router.stop();
}

#[test]
fn authenticated_quiet_subscription_complete_releases_upstream_before_next_event() {
    assert_quiet_subscription_cancellation(false, false);
}

#[test]
fn authenticated_quiet_subscription_socket_close_releases_upstream_before_next_event() {
    assert_quiet_subscription_cancellation(true, false);
}

#[test]
fn authenticated_quiet_cancellation_preserves_sibling_and_reuses_public_operation_id() {
    for close_socket in [false, true] {
        assert_quiet_subscription_cancellation(close_socket, true);
    }
}

fn assert_quiet_subscription_cancellation(close_socket: bool, keep_sibling: bool) {
    let products = LoopbackSubgraph::start(SubgraphKind::Products);
    products.quiet.enabled.store(true, Ordering::Release);
    let reviews = LoopbackSubgraph::start(SubgraphKind::Reviews);
    let products_sdl = LoopbackSdl::start(PRODUCTS_V1);
    let reviews_sdl = LoopbackSdl::start(REVIEWS);
    let products_protocol = LoopbackProtocol::start(protocol_descriptor(
        "products",
        products.endpoint(),
        products_sdl.endpoint(),
        authenticated_product_operations(false),
    ));
    let reviews_protocol = LoopbackProtocol::start(protocol_descriptor(
        "reviews",
        reviews.endpoint(),
        reviews_sdl.endpoint(),
        Vec::new(),
    ));
    let mut router = TestRouter::spawn_authenticated_subscriptions(
        products.endpoint(),
        reviews.endpoint(),
        products_sdl.endpoint(),
        reviews_sdl.endpoint(),
        products_protocol.endpoint(),
        reviews_protocol.endpoint(),
    );
    router.wait_until_ready();
    let query = "subscription { productChanged(id: \"p1\") { id } }";
    let mut first = TestWebSocket::connect_at(router.address, "/api/graphql");
    first.connection_init_bearer("product-events");
    first.subscribe("quiet", query);
    wait_until("quiet authenticated upstream registered", || {
        products.quiet.active.load(Ordering::Acquire) == 1
    });
    assert!(products.saw_bearer_header());
    assert_eq!(products.quiet.started.load(Ordering::Acquire), 1);
    let mut sibling = keep_sibling.then(|| {
        let mut socket = TestWebSocket::connect_at(router.address, "/api/graphql");
        socket.connection_init_bearer("product-events");
        // Same document, principal and public operation ID, on an independent connection.
        // Neither cancellation nor deduplication may transfer ownership between them.
        socket.subscribe("quiet", query);
        wait_until("independent quiet sibling registered", || {
            products.quiet.active_ordinals() == [1, 2]
        });
        socket
    });
    // No next frame is ever emitted by this fixture. Neither cancellation path
    // can depend on data arriving or on retiring/replacing the selected graph.
    if close_socket {
        first.close();
    } else {
        first.complete("quiet");
    }
    wait_until("quiet upstream released without a source event", || {
        products.quiet.active_ordinals() == if keep_sibling { vec![2] } else { vec![] }
    });
    if !close_socket {
        first.close();
    }
    let mut fresh = TestWebSocket::connect_at(router.address, "/api/graphql");
    fresh.connection_init_bearer("product-events-refreshed");
    fresh.subscribe("quiet", query);
    wait_until(
        "fresh authentication owns exactly one quiet subscription",
        || {
            products.quiet.active_ordinals() == if keep_sibling { vec![2, 3] } else { vec![2] }
                && products.quiet.started.load(Ordering::Acquire)
                    == if keep_sibling { 3 } else { 2 }
        },
    );
    assert!(products.quiet.saw_refreshed_bearer.load(Ordering::Acquire));
    fresh.complete("quiet");
    wait_until("fresh quiet upstream released", || {
        products.quiet.active_ordinals() == if keep_sibling { vec![2] } else { vec![] }
    });
    fresh.close();
    if let Some(socket) = sibling.as_mut() {
        socket.close();
        wait_until("quiet sibling retains its own close lifecycle", || {
            products.quiet.active_ordinals().is_empty()
        });
    }
    assert_eq!(products.quiet.active.load(Ordering::Acquire), 0);
    router.stop();
}

#[test]
fn authenticated_polling_reloads_graph_and_policy_as_one_subscription_snapshot() {
    let products = LoopbackSubgraph::start(SubgraphKind::Products);
    let reviews = LoopbackSubgraph::start(SubgraphKind::Reviews);
    let products_sdl = LoopbackSdl::start(PRODUCTS_V1);
    let reviews_sdl = LoopbackSdl::start(REVIEWS);
    let products_protocol = LoopbackProtocol::start(protocol_descriptor(
        "products",
        products.endpoint(),
        products_sdl.endpoint(),
        authenticated_product_operations(false),
    ));
    let reviews_protocol = LoopbackProtocol::start(protocol_descriptor(
        "reviews",
        reviews.endpoint(),
        reviews_sdl.endpoint(),
        Vec::new(),
    ));
    let mut router = TestRouter::spawn_authenticated_subscriptions_polling(
        products.endpoint(),
        reviews.endpoint(),
        products_sdl.endpoint(),
        reviews_sdl.endpoint(),
        products_protocol.endpoint(),
        reviews_protocol.endpoint(),
    );
    router.wait_until_ready();

    let mut selected_v1 = TestWebSocket::connect_at(router.address, "/api/graphql");
    selected_v1.connection_init_bearer("product-events");
    selected_v1.subscribe(
        "v1",
        "subscription { productChanged(id: \"p1\") { id name } }",
    );
    assert_eq!(
        selected_v1.wait_for_operation_message("v1", "next")["payload"]["data"]["productChanged"]["name"],
        "websocket"
    );

    products.quiet.enabled.store(true, Ordering::Release);
    selected_v1.subscribe(
        "quiet-reload",
        "subscription { productChanged(id: \"failure\") { id } }",
    );
    wait_until("quiet upstream registered before reload", || {
        products.quiet.active.load(Ordering::Acquire) == 1
    });

    products_sdl.set(PRODUCTS_V2, "products-sdl-v2", 200);
    // Publish the matching policy descriptor separately. A poll that observes
    // only one side is an incomplete candidate and must retain v1.
    thread::sleep(Duration::from_millis(120));
    products_protocol.set(
        protocol_descriptor(
            "products",
            products.endpoint(),
            products_sdl.endpoint(),
            authenticated_product_operations(true),
        ),
        "products-protocol-v2",
        200,
    );

    let replacement = wait_for_graphql(|| {
        router.graphql_at(
            "/api/graphql",
            json!({"query": "query { version }"}),
            &[("authorization", "Bearer product-events")],
        )
    });
    assert_eq!(replacement["data"]["version"], "v2");
    assert!(products_protocol.fetches() >= 2);
    assert!(products_protocol.conditional_fetches() > 0);

    let mut retired = std::collections::BTreeSet::new();
    for _ in 0..2 {
        let reload = selected_v1.next_message();
        assert_eq!(reload["type"], "error", "{reload}");
        assert_eq!(
            reload["payload"][0]["extensions"]["code"],
            "SUBSCRIPTION_SCHEMA_RELOAD"
        );
        retired.insert(reload["id"].as_str().unwrap().to_owned());
    }
    assert_eq!(retired, ["quiet-reload".to_owned(), "v1".to_owned()].into());
    wait_until(
        "reload releases quiet upstream without client complete",
        || products.quiet.active.load(Ordering::Acquire) == 0,
    );
    products.quiet.enabled.store(false, Ordering::Release);
    selected_v1.assert_terminal_error_and_reuse("v1");
    selected_v1.subscribe(
        "stale",
        "subscription { productChanged(id: \"p1\") { id } }",
    );
    assert_eq!(
        selected_v1.wait_for_operation_message("stale", "error")["payload"][0]["extensions"]["code"],
        "SERVICE_UNAVAILABLE"
    );
    selected_v1.close();

    let mut selected_v2 = TestWebSocket::connect_at(router.address, "/api/graphql");
    selected_v2.connection_init_bearer("product-events");
    selected_v2.subscribe(
        "v2",
        "subscription { productChangedV2(id: \"p1\") { id name } }",
    );
    assert_eq!(
        selected_v2.wait_for_operation_message("v2", "next")["payload"]["data"]["productChangedV2"]
            ["name"],
        "websocket"
    );
    selected_v2.close();
    router.stop();
}

fn websocket_upgrade_status(address: SocketAddr, path: &str) -> u16 {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
        .expect("router should accept a loopback TCP connection");
    stream
        .set_read_timeout(Some(TEST_TIMEOUT))
        .expect("WebSocket read timeout");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: graphql-transport-ws\r\n\r\n"
    )
    .expect("WebSocket upgrade should be written");
    read_head_and_body(&mut stream)
        .expect("router WebSocket response")
        .0
        .split("\r\n")
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .expect("router WebSocket response status")
}

// Keep the address owned across asynchronous schema/protocol requests. A
// released ephemeral port can be claimed by another parallel test before the
// assertion. Occupying it also makes a premature public bind fail, so the
// admission tests must still return their exact composition/authorization error.
fn preparation_listener() -> TcpListener {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("owned preparation listener");
    assert_preparation_listener_owned(&listener);
    listener
}

fn assert_preparation_listener_owned(listener: &TcpListener) {
    let address = listener.local_addr().expect("owned public address");
    let error = TcpListener::bind(address).expect_err("fixture must retain the public address");
    assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
}

#[test]
fn public_preparation_accepts_a_valid_complete_candidate_without_binding() {
    let products_sdl = LoopbackSdl::start(PRODUCTS_V1);
    let reviews_sdl = LoopbackSdl::start(REVIEWS);
    let public_listener = preparation_listener();
    let public_address = public_listener.local_addr().expect("owned public address");
    let prepared = ntex::rt::System::build()
        .name("graphql-orm-router-valid-static-candidate")
        .build(ntex::rt::DefaultRuntime)
        .block_on(async move {
            RouterConfig::new(public_address)
                .allow_anonymous_development(true)
                .with_subgraph(
                    StaticSubgraph::new(
                        "products",
                        "http://127.0.0.1:1/graphql",
                        products_sdl.endpoint(),
                    )
                    .with_schema_header("authorization", "Bearer schema-secret"),
                )
                .with_subgraph(
                    StaticSubgraph::new(
                        "reviews",
                        "http://127.0.0.1:2/graphql",
                        reviews_sdl.endpoint(),
                    )
                    .with_schema_header("authorization", "Bearer schema-secret"),
                )
                .prepare()
                .await
                .expect("valid preparation must succeed even while the public address is occupied")
        });
    assert!(prepared.is_ready());
    assert_preparation_listener_owned(&public_listener);
}

#[test]
fn public_preparation_rejects_an_invalid_complete_candidate_without_binding() {
    let products_sdl = LoopbackSdl::start(PRODUCTS_V1);
    let conflicting_sdl = LoopbackSdl::start(CONFLICTING_REVIEWS);
    let public_listener = preparation_listener();
    let public_address = public_listener.local_addr().expect("owned public address");
    let result = ntex::rt::System::build()
        .name("graphql-orm-router-invalid-static-candidate")
        .build(ntex::rt::DefaultRuntime)
        .block_on(async move {
            RouterConfig::new(public_address)
                .allow_anonymous_development(true)
                .with_subgraph(
                    StaticSubgraph::new(
                        "products",
                        "http://127.0.0.1:1/graphql",
                        products_sdl.endpoint(),
                    )
                    .with_schema_header("authorization", "Bearer schema-secret"),
                )
                .with_subgraph(
                    StaticSubgraph::new(
                        "reviews",
                        "http://127.0.0.1:2/graphql",
                        conflicting_sdl.endpoint(),
                    )
                    .with_schema_header("authorization", "Bearer schema-secret"),
                )
                .prepare()
                .await
        });
    let error = match result {
        Ok(_) => panic!("conflicting complete candidate must fail"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), RouterErrorKind::Composition);
    assert_preparation_listener_owned(&public_listener);
}

#[test]
fn authenticated_preparation_rejects_stale_policy_metadata_without_binding() {
    let products_sdl = LoopbackSdl::start(PRODUCTS_V1);
    let reviews_sdl = LoopbackSdl::start(REVIEWS);
    let products_protocol = LoopbackProtocol::start(protocol_descriptor(
        "products",
        "http://127.0.0.1:1/graphql".to_owned(),
        products_sdl.endpoint(),
        vec![OperationDescriptor {
            root_type: RootOperationType::Query,
            field_name: "product".to_owned(),
            arguments: vec![ArgumentDescriptor {
                name: "id".to_owned(),
                graphql_type: "ID!".to_owned(),
                required: true,
            }],
            authorization: AuthorizationRequirement::Authenticated,
        }],
    ));
    let reviews_protocol = LoopbackProtocol::start(protocol_descriptor(
        "reviews",
        "http://127.0.0.1:2/graphql".to_owned(),
        reviews_sdl.endpoint(),
        Vec::new(),
    ));
    let public_listener = preparation_listener();
    let public_address = public_listener.local_addr().expect("owned public address");
    let result = ntex::rt::System::build()
        .name("graphql-orm-router-invalid-auth-metadata")
        .build(ntex::rt::DefaultRuntime)
        .block_on(async move {
            RouterConfig::new(public_address)
                .with_authentication_provider(Arc::new(TestAuthenticationProvider))
                .with_subgraph(
                    StaticSubgraph::new(
                        "products",
                        "http://127.0.0.1:1/graphql",
                        products_sdl.endpoint(),
                    )
                    .with_protocol_url(products_protocol.endpoint())
                    .with_schema_header("authorization", "Bearer schema-secret"),
                )
                .with_subgraph(
                    StaticSubgraph::new(
                        "reviews",
                        "http://127.0.0.1:2/graphql",
                        reviews_sdl.endpoint(),
                    )
                    .with_protocol_url(reviews_protocol.endpoint())
                    .with_schema_header("authorization", "Bearer schema-secret"),
                )
                .prepare()
                .await
        });
    let error = match result {
        Ok(_) => panic!("incomplete authorization metadata must fail"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), RouterErrorKind::AuthorizationMetadata);
    assert_preparation_listener_owned(&public_listener);
}

#[test]
fn burst_subscription_is_complete_or_terminally_reports_continuity_loss() {
    assert_burst_continuity(false);
}

#[test]
fn burst_loss_notifies_a_quiet_tail_without_waiting_for_another_source_frame() {
    assert_burst_continuity(true);
}

fn assert_burst_continuity(quiet_tail: bool) {
    let products = LoopbackSubgraph::start(SubgraphKind::Products);
    products.quiet.burst.store(true, Ordering::Release);
    products
        .quiet
        .burst_quiet_tail
        .store(quiet_tail, Ordering::Release);
    let reviews = LoopbackSubgraph::start(SubgraphKind::Reviews);
    let products_sdl = LoopbackSdl::start(PRODUCTS_V1);
    let reviews_sdl = LoopbackSdl::start(REVIEWS);
    let products_protocol = LoopbackProtocol::start(protocol_descriptor(
        "products",
        products.endpoint(),
        products_sdl.endpoint(),
        authenticated_product_operations(false),
    ));
    let reviews_protocol = LoopbackProtocol::start(protocol_descriptor(
        "reviews",
        reviews.endpoint(),
        reviews_sdl.endpoint(),
        Vec::new(),
    ));
    let mut router = TestRouter::spawn_authenticated_subscriptions(
        products.endpoint(),
        reviews.endpoint(),
        products_sdl.endpoint(),
        reviews_sdl.endpoint(),
        products_protocol.endpoint(),
        reviews_protocol.endpoint(),
    );
    router.wait_until_ready();
    let mut socket = TestWebSocket::connect_at(router.address, "/api/graphql");
    socket.connection_init();
    let mut other = TestWebSocket::connect_at(router.address, "/api/graphql");
    other.connection_init();
    socket.subscribe("query", "query { product(id: \"slow\") { id } }");
    other.subscribe(
        "mutation",
        "mutation { renameProduct(id: \"slow\", name: \"updated\") { id name } }",
    );
    products.slow_gate.wait_for_started_count(2);
    socket.subscribe(
        "burst",
        "subscription { productChanged(id: \"p1\") { id name } }",
    );
    let mut received = Vec::new();
    loop {
        let message = socket.next_message();
        assert_eq!(message["id"], "burst", "{message}");
        match message["type"].as_str().unwrap() {
            "next" => received.push(
                message["payload"]["data"]["productChanged"]["name"]
                    .as_str()
                    .unwrap()
                    .parse::<usize>()
                    .unwrap(),
            ),
            "complete" => {
                assert!(!quiet_tail, "a quiet source cannot complete");
                assert_eq!(received, (0..1000).collect::<Vec<_>>(), "silent loss");
                break;
            }
            "error" => {
                assert_eq!(
                    message["payload"],
                    json!([{
                        "message": "Subscription continuity lost; resubscribe and refetch",
                        "extensions": {"code": "SUBSCRIPTION_CONTINUITY_LOST"}
                    }])
                );
                break;
            }
            _ => panic!("unexpected frame: {message}"),
        }
    }
    products.release_slow_request();
    for (connection, id) in [(&mut socket, "query"), (&mut other, "mutation")] {
        let result = connection.next_message();
        assert_eq!(result["id"], id, "{result}");
        assert_eq!(
            result["type"], "next",
            "unrelated one-shot was terminated: {result}"
        );
        assert!(result["payload"].get("errors").is_none(), "{result}");
        let complete = connection.next_message();
        assert_eq!(complete["id"], id, "{complete}");
        assert_eq!(complete["type"], "complete", "{complete}");
    }
    // A new query must proceed even while subscription admission is cooling
    // down, and no late burst frames may appear before it.
    socket.subscribe("barrier", "query { product(id: \"p1\") { id } }");
    let barrier = socket.next_message();
    assert_eq!(barrier["id"], "barrier", "late burst frame: {barrier}");
    assert_eq!(barrier["type"], "next", "{barrier}");
}

#[test]
fn hive_websocket_subscription_retires_and_reconnects_on_graph_replacement() {
    let products = LoopbackSubgraph::start(SubgraphKind::Products);
    let reviews = LoopbackSubgraph::start(SubgraphKind::Reviews);
    let temp = TemporaryConfig::new();
    let mut router = TestRouter::spawn(&temp, products.endpoint(), reviews.endpoint());
    router.wait_until_ready();

    let mut first_connection = TestWebSocket::connect(router.address);
    first_connection.connection_init();
    first_connection.subscribe(
        "v1",
        "subscription { productChanged(id: \"p1\") { id name } }",
    );
    let first_event = first_connection.wait_for_operation_message("v1", "next");
    assert_eq!(
        first_event["payload"]["data"]["productChanged"]["name"], "v1",
        "the first subscription must receive a test-owned upstream SSE event"
    );

    let replacement = router.graphql(
        json!({"query": "query { version }"}),
        &[("x-graphql-orm-wire-switch", "v2")],
    );
    assert_eq!(replacement["data"]["version"], "v2");

    let reload = first_connection.wait_for_operation_message("v1", "error");
    assert_eq!(
        reload["payload"][0]["extensions"]["code"], "SUBSCRIPTION_SCHEMA_RELOAD",
        "a retained graph must terminate the operation with a reload error"
    );
    first_connection.assert_terminal_error_and_reuse("v1");

    first_connection.subscribe(
        "stale",
        "subscription { productChanged(id: \"p1\") { id name } }",
    );
    let stale = first_connection.wait_for_operation_message("stale", "error");
    assert_eq!(
        stale["payload"][0]["extensions"]["code"], "SERVICE_UNAVAILABLE",
        "a connection selected from a retired graph must not start new operations"
    );
    first_connection.close();

    let mut second_connection = TestWebSocket::connect(router.address);
    second_connection.connection_init();
    second_connection.subscribe(
        "v2",
        "subscription { productChangedV2(id: \"p1\") { id name } }",
    );
    let second_event = second_connection.wait_for_operation_message("v2", "next");
    assert_eq!(
        second_event["payload"]["data"]["productChangedV2"]["name"], "v2",
        "a reconnect must select the replacement graph and its new operation"
    );
    second_connection.complete("v2");
    second_connection.close();

    router.stop();
}

/// Minimal deterministic client for the public `graphql-transport-ws`
/// endpoint. Keeping it in the fixture avoids coupling the crate to a second
/// production WebSocket stack just for a loopback regression test.
struct TestWebSocket {
    stream: TcpStream,
}

impl TestWebSocket {
    fn connect(address: SocketAddr) -> Self {
        Self::connect_at(address, "/ws")
    }

    fn connect_at(address: SocketAddr, path: &str) -> Self {
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
            .expect("router WebSocket should accept a loopback connection");
        stream
            .set_read_timeout(Some(TEST_TIMEOUT))
            .expect("WebSocket read timeout");
        stream
            .set_write_timeout(Some(TEST_TIMEOUT))
            .expect("WebSocket write timeout");
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: {address}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: graphql-transport-ws\r\n\r\n"
        )
        .expect("WebSocket upgrade should be written");

        let (head, body) = read_head_and_body(&mut stream).expect("router WebSocket upgrade");
        assert!(
            body.is_empty(),
            "WebSocket upgrade must not contain a response body"
        );
        let status = head
            .split("\r\n")
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse::<u16>().ok());
        assert_eq!(status, Some(101), "WebSocket upgrade response: {head}");
        assert!(
            head.lines().any(|line| {
                line.split_once(':').is_some_and(|(name, value)| {
                    name.eq_ignore_ascii_case("sec-websocket-protocol")
                        && value.trim().eq_ignore_ascii_case("graphql-transport-ws")
                })
            }),
            "router must negotiate graphql-transport-ws: {head}"
        );

        Self { stream }
    }

    fn connection_init(&mut self) {
        self.connection_init_bearer("product-events");
    }

    fn connection_init_bearer(&mut self, token: &str) {
        self.send_json(&json!({
            "type": "connection_init",
            "payload": {"authorization": format!("Bearer {token}")}
        }));
        let ack = self.next_message();
        assert_eq!(ack["type"], "connection_ack", "{ack}");
    }

    fn subscribe(&mut self, id: &str, query: &str) {
        self.send_json(&json!({
            "id": id,
            "type": "subscribe",
            "payload": {"query": query}
        }));
    }

    fn subscribe_with_variables(&mut self, id: &str, query: &str, variables: Value) {
        self.send_json(&json!({
            "id": id,
            "type": "subscribe",
            "payload": {"query": query, "variables": variables}
        }));
    }

    fn subscribe_with_variables_and_extensions(
        &mut self,
        id: &str,
        query: &str,
        variables: Value,
        extensions: Value,
    ) {
        self.send_json(&json!({
            "id": id,
            "type": "subscribe",
            "payload": {
                "query": query,
                "variables": variables,
                "extensions": extensions
            }
        }));
    }

    fn complete(&mut self, id: &str) {
        self.send_json(&json!({"id": id, "type": "complete"}));
    }

    fn wait_for_operation_message(&mut self, id: &str, message_type: &str) -> Value {
        let deadline = Instant::now() + TEST_TIMEOUT;
        loop {
            assert!(
                Instant::now() < deadline,
                "did not receive {message_type} for WebSocket operation {id} within {TEST_TIMEOUT:?}"
            );
            let message = self.next_message();
            if message["id"] == id && message["type"] == message_type {
                return message;
            }
        }
    }

    // A second subscribe traverses the same ordered bridge after the reload.
    // Read the very next frame: a trailing complete must fail, not be skipped by
    // wait_for_operation_message. Reusing the ID also proves error cleanup.
    fn assert_terminal_error_and_reuse(&mut self, id: &str) {
        for _ in 0..2 {
            self.subscribe(id, "subscription { productChanged(id: \"p1\") { id } }");
            let message = self.next_message();
            assert_eq!(message["id"], id, "{message}");
            assert_eq!(
                message["type"], "error",
                "no complete may follow error: {message}"
            );
            assert_eq!(
                message["payload"][0]["extensions"]["code"],
                "SERVICE_UNAVAILABLE"
            );
        }
    }

    fn wait_for_operation_failure(&mut self, id: &str) -> Value {
        let deadline = Instant::now() + TEST_TIMEOUT;
        let mut failure = None;
        loop {
            assert!(
                Instant::now() < deadline,
                "did not receive a terminal failure for WebSocket operation {id} within {TEST_TIMEOUT:?}"
            );
            let message = self.next_message();
            if message["id"] != id {
                continue;
            }
            match message["type"].as_str() {
                Some("error") => return message,
                Some("next") if message["payload"].get("errors").is_some() => {
                    failure = Some(message);
                }
                Some("complete") => {
                    return failure.expect("failed operation must report an error before complete");
                }
                _ => {}
            }
        }
    }

    fn close(&mut self) {
        let _ = write_websocket_frame(&mut self.stream, 0x8, &[]);
    }

    fn send_json(&mut self, value: &Value) {
        let bytes = serde_json::to_vec(value).expect("WebSocket JSON should serialize");
        write_websocket_frame(&mut self.stream, 0x1, &bytes)
            .expect("WebSocket client frame should be written");
    }

    fn send_oversized_text_frame(&mut self) {
        let message = json!({
            "id": "oversized",
            "type": "subscribe",
            "payload": {
                "query": "query { product(id: \"p1\") { id } }",
                "extensions": {"padding": "x".repeat(65_536)}
            }
        });
        let bytes =
            serde_json::to_vec(&message).expect("oversized WebSocket JSON should serialize");
        let _ = write_websocket_frame(&mut self.stream, 0x1, &bytes);
    }

    fn next_message(&mut self) -> Value {
        loop {
            let (opcode, payload) = read_websocket_frame(&mut self.stream)
                .expect("router should send a valid WebSocket frame");
            match opcode {
                0x1 => {
                    return serde_json::from_slice(&payload)
                        .expect("router WebSocket text frame should be GraphQL JSON");
                }
                0x8 => panic!("router closed WebSocket before expected message"),
                0x9 => write_websocket_frame(&mut self.stream, 0xA, &payload)
                    .expect("WebSocket pong should be written"),
                0xA => {}
                other => panic!("unexpected router WebSocket frame opcode {other}"),
            }
        }
    }

    fn wait_for_close(&mut self) -> u16 {
        loop {
            let (opcode, payload) = read_websocket_frame(&mut self.stream)
                .expect("router should send a WebSocket close frame");
            match opcode {
                0x8 if payload.len() >= 2 => {
                    return u16::from_be_bytes([payload[0], payload[1]]);
                }
                0x9 => write_websocket_frame(&mut self.stream, 0xA, &payload)
                    .expect("WebSocket pong should be written"),
                _ => {}
            }
        }
    }

    // Absolute deadline for the new clock controls: repeated upstream frames
    // must not keep resetting a socket timeout when the expiry timer is broken.
    fn wait_for_close_before(&mut self, deadline: Instant) -> u16 {
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "router did not close before the deadline"
            );
            self.stream.set_read_timeout(Some(remaining)).unwrap();
            let (opcode, payload) = read_websocket_frame(&mut self.stream)
                .expect("router should close within the bounded expiry window");
            match opcode {
                0x8 if payload.len() >= 2 => {
                    return u16::from_be_bytes([payload[0], payload[1]]);
                }
                0x9 => write_websocket_frame(&mut self.stream, 0xA, &payload)
                    .expect("WebSocket pong should be written"),
                _ => {}
            }
        }
    }

    fn wait_for_disconnect_without_close_code(&mut self, forbidden_code: u16) {
        loop {
            match read_websocket_frame(&mut self.stream) {
                Ok((0x8, payload)) if payload.len() >= 2 => {
                    let code = u16::from_be_bytes([payload[0], payload[1]]);
                    assert_ne!(
                        code, forbidden_code,
                        "a secondary transport task must not mask the primary protocol failure"
                    );
                    return;
                }
                Ok((0x9, payload)) => {
                    let _ = write_websocket_frame(&mut self.stream, 0xA, &payload);
                }
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::UnexpectedEof
                            | io::ErrorKind::ConnectionReset
                            | io::ErrorKind::BrokenPipe
                    ) =>
                {
                    return;
                }
                Err(error) => {
                    panic!("router did not terminate an oversized WebSocket frame: {error}")
                }
            }
        }
    }
}

fn write_websocket_frame(stream: &mut TcpStream, opcode: u8, payload: &[u8]) -> io::Result<()> {
    const MASK: [u8; 4] = [0x13, 0x37, 0xC0, 0xDE];
    stream.write_all(&[0x80 | opcode])?;
    match payload.len() {
        0..=125 => stream.write_all(&[0x80 | payload.len() as u8])?,
        126..=65_535 => {
            stream.write_all(&[0x80 | 126])?;
            stream.write_all(&(payload.len() as u16).to_be_bytes())?;
        }
        _ => {
            stream.write_all(&[0x80 | 127])?;
            stream.write_all(&(payload.len() as u64).to_be_bytes())?;
        }
    }
    stream.write_all(&MASK)?;
    for (index, byte) in payload.iter().enumerate() {
        stream.write_all(&[byte ^ MASK[index % MASK.len()]])?;
    }
    stream.flush()
}

fn read_websocket_frame(stream: &mut TcpStream) -> io::Result<(u8, Vec<u8>)> {
    let mut header = [0_u8; 2];
    stream.read_exact(&mut header)?;
    assert!(header[0] & 0x80 != 0, "fragmented test WebSocket frame");
    assert!(header[1] & 0x80 == 0, "router must not mask server frames");
    let opcode = header[0] & 0x0F;
    let length = match header[1] & 0x7F {
        length @ 0..=125 => usize::from(length),
        126 => {
            let mut bytes = [0_u8; 2];
            stream.read_exact(&mut bytes)?;
            usize::from(u16::from_be_bytes(bytes))
        }
        127 => {
            let mut bytes = [0_u8; 8];
            stream.read_exact(&mut bytes)?;
            usize::try_from(u64::from_be_bytes(bytes)).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "WebSocket frame exceeds usize")
            })?
        }
        _ => unreachable!("the WebSocket length discriminator is seven bits"),
    };
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload)?;
    Ok((opcode, payload))
}

#[derive(Clone)]
struct TestRouter {
    address: SocketAddr,
    child: Arc<Mutex<Option<Child>>>,
}

impl TestRouter {
    fn from_child(mut child: Child, has_admin: bool) -> (Self, Option<SocketAddr>) {
        let stdout = child.stdout.take().expect("piped child stdout");
        let (sender, receiver) = std::sync::mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(bound) = line.strip_prefix("WIRE_BOUND ") {
                    let _ = sender.send(bound.to_owned());
                }
            }
        });
        let mut router = Self {
            address: SocketAddr::from(([127, 0, 0, 1], 0)),
            child: Arc::new(Mutex::new(Some(child))),
        };
        let mut admin = None;
        let deadline = Instant::now() + TEST_TIMEOUT;
        for _ in 0..if has_admin { 2 } else { 1 } {
            let bound = receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("child must report its owned listener");
            let (kind, address) = bound.split_once(' ').expect("listener report");
            let address = address.parse().expect("bound socket address");
            match kind {
                "public" => router.address = address,
                "admin" => admin = Some(address),
                _ => panic!("unexpected listener kind"),
            }
        }
        assert_ne!(router.address.port(), 0);
        (router, admin)
    }

    fn spawn(
        config: &TemporaryConfig,
        products_endpoint: String,
        reviews_endpoint: String,
    ) -> Self {
        let child = Command::new(env::current_exe().expect("test executable path"))
            .arg("--exact")
            .arg("federation::wire_tests::router_child_entrypoint")
            .arg("--nocapture")
            .env(CHILD_ENV, "wire")
            .env("ROUTER_CONFIG_FILE_PATH", config.path())
            .env(PRODUCTS_ENDPOINT_ENV, products_endpoint)
            .env(REVIEWS_ENDPOINT_ENV, reviews_endpoint)
            .env("RUST_TEST_THREADS", "1")
            .env("RUST_LOG", "error")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("router child should start");
        Self::from_child(child, false).0
    }

    fn spawn_static(
        products_endpoint: String,
        reviews_endpoint: String,
        products_sdl_endpoint: String,
        reviews_sdl_endpoint: String,
    ) -> Self {
        Self::spawn_static_mode(
            "static",
            products_endpoint,
            reviews_endpoint,
            products_sdl_endpoint,
            reviews_sdl_endpoint,
        )
    }

    fn spawn_polling(
        products_endpoint: String,
        reviews_endpoint: String,
        products_sdl_endpoint: String,
        reviews_sdl_endpoint: String,
    ) -> Self {
        Self::spawn_static_mode(
            "polling",
            products_endpoint,
            reviews_endpoint,
            products_sdl_endpoint,
            reviews_sdl_endpoint,
        )
    }

    fn spawn_static_mode(
        mode: &str,
        products_endpoint: String,
        reviews_endpoint: String,
        products_sdl_endpoint: String,
        reviews_sdl_endpoint: String,
    ) -> Self {
        let child = Command::new(env::current_exe().expect("test executable path"))
            .arg("--exact")
            .arg("federation::wire_tests::router_child_entrypoint")
            .arg("--nocapture")
            .env(CHILD_ENV, mode)
            .env(PRODUCTS_ENDPOINT_ENV, products_endpoint)
            .env(REVIEWS_ENDPOINT_ENV, reviews_endpoint)
            .env(PRODUCTS_SDL_ENDPOINT_ENV, products_sdl_endpoint)
            .env(REVIEWS_SDL_ENDPOINT_ENV, reviews_sdl_endpoint)
            .env("RUST_TEST_THREADS", "1")
            .env("RUST_LOG", "error")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("static router child should start");
        Self::from_child(child, false).0
    }

    fn spawn_authenticated(
        products_endpoint: String,
        reviews_endpoint: String,
        products_sdl_endpoint: String,
        reviews_sdl_endpoint: String,
        products_protocol_endpoint: String,
        reviews_protocol_endpoint: String,
    ) -> Self {
        Self::spawn_authenticated_mode(
            "auth",
            products_endpoint,
            reviews_endpoint,
            products_sdl_endpoint,
            reviews_sdl_endpoint,
            products_protocol_endpoint,
            reviews_protocol_endpoint,
        )
    }

    fn spawn_authenticated_subscriptions(
        products_endpoint: String,
        reviews_endpoint: String,
        products_sdl_endpoint: String,
        reviews_sdl_endpoint: String,
        products_protocol_endpoint: String,
        reviews_protocol_endpoint: String,
    ) -> Self {
        Self::spawn_authenticated_mode(
            "auth-subscriptions",
            products_endpoint,
            reviews_endpoint,
            products_sdl_endpoint,
            reviews_sdl_endpoint,
            products_protocol_endpoint,
            reviews_protocol_endpoint,
        )
    }

    fn spawn_authenticated_subscriptions_polling(
        products_endpoint: String,
        reviews_endpoint: String,
        products_sdl_endpoint: String,
        reviews_sdl_endpoint: String,
        products_protocol_endpoint: String,
        reviews_protocol_endpoint: String,
    ) -> Self {
        Self::spawn_authenticated_mode(
            "auth-subscriptions-polling",
            products_endpoint,
            reviews_endpoint,
            products_sdl_endpoint,
            reviews_sdl_endpoint,
            products_protocol_endpoint,
            reviews_protocol_endpoint,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_admin(
        products_endpoint: String,
        reviews_endpoint: String,
        products_sdl_endpoint: String,
        reviews_sdl_endpoint: String,
        products_protocol_endpoint: String,
        reviews_protocol_endpoint: String,
    ) -> (Self, SocketAddr) {
        let child = Command::new(env::current_exe().expect("test executable path"))
            .arg("--exact")
            .arg("federation::wire_tests::router_child_entrypoint")
            .arg("--nocapture")
            .env(CHILD_ENV, "admin")
            .env(PRODUCTS_ENDPOINT_ENV, products_endpoint)
            .env(REVIEWS_ENDPOINT_ENV, reviews_endpoint)
            .env(PRODUCTS_SDL_ENDPOINT_ENV, products_sdl_endpoint)
            .env(REVIEWS_SDL_ENDPOINT_ENV, reviews_sdl_endpoint)
            .env(PRODUCTS_PROTOCOL_ENDPOINT_ENV, products_protocol_endpoint)
            .env(REVIEWS_PROTOCOL_ENDPOINT_ENV, reviews_protocol_endpoint)
            .env("RUST_TEST_THREADS", "1")
            .env("RUST_LOG", "error")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("administrative router child should start");
        let (router, admin) = Self::from_child(child, true);
        (router, admin.expect("bound admin address"))
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_authenticated_mode(
        mode: &str,
        products_endpoint: String,
        reviews_endpoint: String,
        products_sdl_endpoint: String,
        reviews_sdl_endpoint: String,
        products_protocol_endpoint: String,
        reviews_protocol_endpoint: String,
    ) -> Self {
        let child = Command::new(env::current_exe().expect("test executable path"))
            .arg("--exact")
            .arg("federation::wire_tests::router_child_entrypoint")
            .arg("--nocapture")
            .env(CHILD_ENV, mode)
            .env(PRODUCTS_ENDPOINT_ENV, products_endpoint)
            .env(REVIEWS_ENDPOINT_ENV, reviews_endpoint)
            .env(PRODUCTS_SDL_ENDPOINT_ENV, products_sdl_endpoint)
            .env(REVIEWS_SDL_ENDPOINT_ENV, reviews_sdl_endpoint)
            .env(PRODUCTS_PROTOCOL_ENDPOINT_ENV, products_protocol_endpoint)
            .env(REVIEWS_PROTOCOL_ENDPOINT_ENV, reviews_protocol_endpoint)
            .env("RUST_TEST_THREADS", "1")
            .env("RUST_LOG", "error")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("authenticated router child should start");
        Self::from_child(child, false).0
    }

    fn wait_until_ready(&mut self) {
        let deadline = Instant::now() + TEST_TIMEOUT;
        loop {
            if self.health_check() {
                return;
            }
            let status = {
                let mut child = lock_child(&self.child);
                child
                    .as_mut()
                    .and_then(|child| child.try_wait().expect("router child status"))
            };
            if let Some(status) = status {
                panic!("router child exited before readiness with {status}");
            }
            assert!(
                Instant::now() < deadline,
                "router did not become ready within {TEST_TIMEOUT:?}"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn health_check(&self) -> bool {
        self.probe("/health") == 200
    }

    fn probe(&self, path: &str) -> u16 {
        raw_http(self.address, "GET", path, &[], None)
            .map(|response| response.status)
            .unwrap_or_default()
    }

    fn graphql(&self, payload: Value, headers: &[(&str, &str)]) -> Value {
        self.graphql_at("/graphql", payload, headers)
    }

    fn graphql_at(&self, path: &str, payload: Value, headers: &[(&str, &str)]) -> Value {
        let (status, value) = self.graphql_response_at(path, payload, headers);
        assert_eq!(status, 200, "router response: {value}");
        value
    }

    fn graphql_response_at(
        &self,
        path: &str,
        payload: Value,
        headers: &[(&str, &str)],
    ) -> (u16, Value) {
        let body = serde_json::to_vec(&payload).expect("request JSON should serialize");
        let response = raw_http(self.address, "POST", path, headers, Some(&body))
            .expect("router should respond over loopback HTTP");
        let value =
            serde_json::from_slice(&response.body).expect("router response should be GraphQL JSON");
        (response.status, value)
    }

    fn stop(&mut self) {
        let Some(mut child) = lock_child(&self.child).take() else {
            return;
        };
        let _ = child.kill();
        let status = child.wait().expect("router child should terminate");
        assert!(
            !status.success(),
            "the test router is deliberately terminated"
        );
    }
}

impl Drop for TestRouter {
    fn drop(&mut self) {
        if Arc::strong_count(&self.child) != 1 {
            return;
        }
        let Some(mut child) = lock_child(&self.child).take() else {
            return;
        };
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn lock_child(child: &Mutex<Option<Child>>) -> std::sync::MutexGuard<'_, Option<Child>> {
    child
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct TemporaryConfig {
    directory: PathBuf,
    config: PathBuf,
}

impl TemporaryConfig {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos();
        let directory = env::temp_dir().join(format!(
            "graphql-orm-router-wire-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("temporary router directory should be created");
        let config = directory.join("router.yaml");
        fs::write(
            &config,
            "supergraph:\n  source: plugin\nhttp:\n  host: 127.0.0.1\n  port: 0\n  workers: 1\nsubscriptions:\n  enabled: true\nwebsocket:\n  enabled: true\n  path: /ws\nplugins:\n  graphql-orm-router-wire-proof: {}\n",
        )
        .expect("temporary router config should be written");
        Self { directory, config }
    }

    fn path(&self) -> &PathBuf {
        &self.config
    }
}

impl Drop for TemporaryConfig {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[derive(Clone, Copy)]
enum SubgraphKind {
    Products,
    Reviews,
}

#[derive(Default)]
struct QuietSubscriptions {
    burst: AtomicBool,
    burst_quiet_tail: AtomicBool,
    enabled: AtomicBool,
    active: AtomicUsize,
    started: AtomicUsize,
    saw_refreshed_bearer: AtomicBool,
    ordinals: Mutex<std::collections::BTreeSet<usize>>,
}

impl QuietSubscriptions {
    fn active_ordinals(&self) -> Vec<usize> {
        self.ordinals
            .lock()
            .expect("quiet subscription lock")
            .iter()
            .copied()
            .collect()
    }
}

struct QuietSubscription {
    state: Arc<QuietSubscriptions>,
    ordinal: usize,
}

impl QuietSubscription {
    fn new(state: Arc<QuietSubscriptions>) -> Self {
        state.active.fetch_add(1, Ordering::AcqRel);
        let ordinal = state.started.fetch_add(1, Ordering::AcqRel) + 1;
        state
            .ordinals
            .lock()
            .expect("quiet subscription lock")
            .insert(ordinal);
        Self { state, ordinal }
    }
}

impl Drop for QuietSubscription {
    fn drop(&mut self) {
        self.state.active.fetch_sub(1, Ordering::AcqRel);
        self.state
            .ordinals
            .lock()
            .expect("quiet subscription lock")
            .remove(&self.ordinal);
    }
}

struct LoopbackSubgraph {
    address: SocketAddr,
    opens: Arc<AtomicUsize>,
    saw_approved_header: Arc<AtomicBool>,
    saw_blocked_header: Arc<AtomicBool>,
    saw_bearer_header: Arc<AtomicBool>,
    slow_gate: Arc<SlowGate>,
    quiet: Arc<QuietSubscriptions>,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Clone)]
struct SubgraphHandlerState {
    opens: Arc<AtomicUsize>,
    saw_approved_header: Arc<AtomicBool>,
    saw_blocked_header: Arc<AtomicBool>,
    saw_bearer_header: Arc<AtomicBool>,
    slow_gate: Arc<SlowGate>,
    quiet: Arc<QuietSubscriptions>,
    stopping: Arc<AtomicBool>,
}

impl LoopbackSubgraph {
    fn start(kind: SubgraphKind) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("loopback subgraph bind");
        listener
            .set_nonblocking(true)
            .expect("subgraph listener nonblocking");
        let address = listener.local_addr().expect("subgraph address");
        let opens = Arc::new(AtomicUsize::new(0));
        let saw_approved_header = Arc::new(AtomicBool::new(false));
        let saw_blocked_header = Arc::new(AtomicBool::new(false));
        let saw_bearer_header = Arc::new(AtomicBool::new(false));
        let slow_gate = Arc::new(SlowGate::default());
        let quiet = Arc::new(QuietSubscriptions::default());
        let stopping = Arc::new(AtomicBool::new(false));
        let thread_state = SubgraphHandlerState {
            opens: opens.clone(),
            saw_approved_header: saw_approved_header.clone(),
            saw_blocked_header: saw_blocked_header.clone(),
            saw_bearer_header: saw_bearer_header.clone(),
            slow_gate: slow_gate.clone(),
            quiet: quiet.clone(),
            stopping: stopping.clone(),
        };
        let thread = thread::spawn(move || {
            while !thread_state.stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let state = thread_state.clone();
                        thread::spawn(move || handle_subgraph(stream, kind, state));
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            }
        });
        Self {
            address,
            opens,
            saw_approved_header,
            saw_blocked_header,
            saw_bearer_header,
            slow_gate,
            quiet,
            stopping,
            thread: Some(thread),
        }
    }

    fn endpoint(&self) -> String {
        format!("http://{}/graphql", self.address)
    }

    fn opens(&self) -> usize {
        self.opens.load(Ordering::Acquire)
    }

    fn saw_approved_header(&self) -> bool {
        self.saw_approved_header.load(Ordering::Acquire)
    }

    fn saw_blocked_header(&self) -> bool {
        self.saw_blocked_header.load(Ordering::Acquire)
    }

    fn saw_bearer_header(&self) -> bool {
        self.saw_bearer_header.load(Ordering::Acquire)
    }

    fn wait_for_slow_request(&self) {
        self.slow_gate.wait_for_started();
    }

    fn release_slow_request(&self) {
        self.slow_gate.release();
    }
}

impl Drop for LoopbackSubgraph {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        self.slow_gate.release();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct LoopbackSdl {
    address: SocketAddr,
    state: Arc<Mutex<LoopbackEndpointState>>,
    fetches: Arc<AtomicUsize>,
    conditional_fetches: Arc<AtomicUsize>,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Clone)]
struct LoopbackEndpointState {
    body: String,
    etag: String,
    status: u16,
    redirect: Option<String>,
}

impl LoopbackSdl {
    fn start(sdl: &str) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("loopback SDL bind");
        listener
            .set_nonblocking(true)
            .expect("SDL listener nonblocking");
        let address = listener.local_addr().expect("SDL listener address");
        let state = Arc::new(Mutex::new(LoopbackEndpointState {
            body: sdl.to_owned(),
            etag: "test-sdl-v1".to_owned(),
            status: 200,
            redirect: None,
        }));
        let fetches = Arc::new(AtomicUsize::new(0));
        let conditional_fetches = Arc::new(AtomicUsize::new(0));
        let stopping = Arc::new(AtomicBool::new(false));
        let thread_state = state.clone();
        let thread_fetches = fetches.clone();
        let thread_conditional_fetches = conditional_fetches.clone();
        let thread_stopping = stopping.clone();
        let thread = thread::spawn(move || {
            while !thread_stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let Ok((head, _)) = read_head_and_body(&mut stream) else {
                            continue;
                        };
                        if !has_header(&head, "authorization", "Bearer schema-secret") {
                            let _ = write_http_response(stream, 401, b"{}");
                            continue;
                        }
                        thread_fetches.fetch_add(1, Ordering::AcqRel);
                        let snapshot = thread_state
                            .lock()
                            .expect("loopback SDL endpoint state")
                            .clone();
                        if let Some(location) = &snapshot.redirect {
                            let _ = write!(
                                stream,
                                "HTTP/1.1 {} Redirect\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                                snapshot.status, location
                            );
                        } else if snapshot.status != 200 {
                            let _ = write!(
                                stream,
                                "HTTP/1.1 {} Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                                snapshot.status
                            );
                        } else if has_header(
                            &head,
                            "if-none-match",
                            &format!("\"{}\"", snapshot.etag),
                        ) {
                            thread_conditional_fetches.fetch_add(1, Ordering::AcqRel);
                            let _ = stream.write_all(
                                b"HTTP/1.1 304 Not Modified\r\nConnection: close\r\n\r\n",
                            );
                        } else {
                            let _ = write!(
                                stream,
                                "HTTP/1.1 200 OK\r\nContent-Type: application/graphql\r\nETag: \"{}\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                snapshot.etag,
                                snapshot.body.len(),
                                snapshot.body
                            );
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            }
        });
        Self {
            address,
            state,
            fetches,
            conditional_fetches,
            stopping,
            thread: Some(thread),
        }
    }

    fn endpoint(&self) -> String {
        format!("http://{}/sdl", self.address)
    }

    fn fetches(&self) -> usize {
        self.fetches.load(Ordering::Acquire)
    }

    fn conditional_fetches(&self) -> usize {
        self.conditional_fetches.load(Ordering::Acquire)
    }

    fn set(&self, sdl: &str, etag: &str, status: u16) {
        let mut state = self.state.lock().expect("loopback SDL endpoint state");
        state.body = sdl.to_owned();
        state.etag = etag.to_owned();
        state.status = status;
        state.redirect = None;
    }
}

impl Drop for LoopbackSdl {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct LoopbackProtocol {
    address: SocketAddr,
    state: Arc<Mutex<LoopbackEndpointState>>,
    fetches: Arc<AtomicUsize>,
    conditional_fetches: Arc<AtomicUsize>,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl LoopbackProtocol {
    fn start(descriptor: SubgraphDescriptor) -> Self {
        let body = serde_json::to_string(&descriptor).expect("protocol descriptor JSON");
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("loopback protocol bind");
        listener
            .set_nonblocking(true)
            .expect("protocol listener nonblocking");
        let address = listener.local_addr().expect("protocol listener address");
        let state = Arc::new(Mutex::new(LoopbackEndpointState {
            body,
            etag: "test-protocol-v1".to_owned(),
            status: 200,
            redirect: None,
        }));
        let fetches = Arc::new(AtomicUsize::new(0));
        let conditional_fetches = Arc::new(AtomicUsize::new(0));
        let stopping = Arc::new(AtomicBool::new(false));
        let thread_state = state.clone();
        let thread_fetches = fetches.clone();
        let thread_conditional_fetches = conditional_fetches.clone();
        let thread_stopping = stopping.clone();
        let thread = thread::spawn(move || {
            while !thread_stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let Ok((head, _)) = read_head_and_body(&mut stream) else {
                            continue;
                        };
                        if !has_header(&head, "authorization", "Bearer schema-secret") {
                            let _ = write_http_response(stream, 401, b"{}");
                            continue;
                        }
                        thread_fetches.fetch_add(1, Ordering::AcqRel);
                        let snapshot = thread_state
                            .lock()
                            .expect("loopback protocol endpoint state")
                            .clone();
                        if let Some(location) = &snapshot.redirect {
                            let _ = write!(
                                stream,
                                "HTTP/1.1 {} Redirect\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                                snapshot.status, location
                            );
                        } else if snapshot.status != 200 {
                            let _ = write!(
                                stream,
                                "HTTP/1.1 {} Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                                snapshot.status
                            );
                        } else if has_header(
                            &head,
                            "if-none-match",
                            &format!("\"{}\"", snapshot.etag),
                        ) {
                            thread_conditional_fetches.fetch_add(1, Ordering::AcqRel);
                            let _ = stream.write_all(
                                b"HTTP/1.1 304 Not Modified\r\nConnection: close\r\n\r\n",
                            );
                        } else {
                            let _ = write!(
                                stream,
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nETag: \"{}\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                snapshot.etag,
                                snapshot.body.len(),
                                snapshot.body
                            );
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            }
        });
        Self {
            address,
            state,
            fetches,
            conditional_fetches,
            stopping,
            thread: Some(thread),
        }
    }

    fn endpoint(&self) -> String {
        format!("http://{}/.well-known/graphql-router", self.address)
    }

    fn fetches(&self) -> usize {
        self.fetches.load(Ordering::Acquire)
    }

    fn conditional_fetches(&self) -> usize {
        self.conditional_fetches.load(Ordering::Acquire)
    }

    fn set(&self, descriptor: SubgraphDescriptor, etag: &str, status: u16) {
        let mut state = self.state.lock().expect("loopback protocol endpoint state");
        state.body = serde_json::to_string(&descriptor).expect("protocol descriptor JSON");
        state.etag = etag.to_owned();
        state.status = status;
        state.redirect = None;
    }

    fn set_raw(&self, body: String, etag: &str, status: u16) {
        let mut state = self.state.lock().expect("loopback protocol endpoint state");
        state.body = body;
        state.etag = etag.to_owned();
        state.status = status;
        state.redirect = None;
    }

    fn redirect_to(&self, location: String) {
        let mut state = self.state.lock().expect("loopback protocol endpoint state");
        state.body.clear();
        state.etag = "redirect".to_owned();
        state.status = 302;
        state.redirect = Some(location);
    }
}

impl Drop for LoopbackProtocol {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn protocol_endpoint(value: String) -> AdvertisedEndpoint {
    AdvertisedEndpoint::try_from(value).expect("test protocol endpoint")
}

fn protocol_descriptor(
    name: &str,
    graphql_url: String,
    schema_url: String,
    operations: Vec<OperationDescriptor>,
) -> SubgraphDescriptor {
    let has_subscriptions = operations
        .iter()
        .any(|operation| operation.root_type == RootOperationType::Subscription);
    let mut descriptor = SubgraphDescriptor {
        protocol_version: ProtocolVersion { major: 1, minor: 0 },
        subgraph: SubgraphIdentity {
            id: SubgraphId::try_from(format!("{name}-service")).unwrap(),
            name: SubgraphName::try_from(name.to_owned()).unwrap(),
        },
        graphql: GraphqlEndpoints {
            http: protocol_endpoint(graphql_url),
            websocket: None,
        },
        schema: SchemaAdvertisement {
            url: protocol_endpoint(schema_url),
        },
        capabilities: CapabilitySet {
            subscriptions: has_subscriptions,
            authorization_metadata: true,
            schema_fingerprints: true,
        },
        required_semantics: vec![
            "authorizationMetadata".to_owned(),
            "scopeTemplates".to_owned(),
        ],
        operations,
        extensions: Vec::new(),
        fingerprints: DescriptorFingerprints {
            schema: Fingerprint::sha256(format!("{name} schema")),
            authorization: Fingerprint::sha256("placeholder"),
            combined: Fingerprint::sha256("placeholder"),
        },
    };
    descriptor.fingerprints.authorization = descriptor.authorization_fingerprint();
    descriptor.fingerprints.combined = descriptor.combined_fingerprint();
    descriptor
}

fn authenticated_product_operations(include_v2: bool) -> Vec<OperationDescriptor> {
    let mut operations = vec![
        OperationDescriptor {
            root_type: RootOperationType::Query,
            field_name: "product".to_owned(),
            arguments: vec![ArgumentDescriptor {
                name: "id".to_owned(),
                graphql_type: "ID!".to_owned(),
                required: true,
            }],
            authorization: AuthorizationRequirement::Authenticated,
        },
        OperationDescriptor {
            root_type: RootOperationType::Mutation,
            field_name: "renameProduct".to_owned(),
            arguments: vec![
                ArgumentDescriptor {
                    name: "id".to_owned(),
                    graphql_type: "ID!".to_owned(),
                    required: true,
                },
                ArgumentDescriptor {
                    name: "name".to_owned(),
                    graphql_type: "String!".to_owned(),
                    required: true,
                },
            ],
            authorization: AuthorizationRequirement::Authenticated,
        },
        OperationDescriptor {
            root_type: RootOperationType::Subscription,
            field_name: "productChanged".to_owned(),
            arguments: vec![ArgumentDescriptor {
                name: "id".to_owned(),
                graphql_type: "ID!".to_owned(),
                required: true,
            }],
            authorization: AuthorizationRequirement::AllScopes {
                scopes: vec![scope("products.{id}.events")],
            },
        },
    ];
    if include_v2 {
        operations.push(OperationDescriptor {
            root_type: RootOperationType::Query,
            field_name: "version".to_owned(),
            arguments: Vec::new(),
            authorization: AuthorizationRequirement::Authenticated,
        });
        operations.push(OperationDescriptor {
            root_type: RootOperationType::Subscription,
            field_name: "productChangedV2".to_owned(),
            arguments: vec![ArgumentDescriptor {
                name: "id".to_owned(),
                graphql_type: "ID!".to_owned(),
                required: true,
            }],
            authorization: AuthorizationRequirement::AllScopes {
                scopes: vec![scope("products.{id}.events")],
            },
        });
    }
    operations
}

fn scope(value: &str) -> ScopeTemplate {
    ScopeTemplate::parse(value).unwrap()
}

#[derive(Default)]
struct SlowGate {
    state: Mutex<SlowGateState>,
    changed: Condvar,
}

#[derive(Default)]
struct SlowGateState {
    started: usize,
    released: bool,
}

impl SlowGate {
    fn hold_until_released(&self) {
        let mut state = self.state.lock().expect("slow gate lock");
        state.started += 1;
        self.changed.notify_all();
        while !state.released {
            state = self.changed.wait(state).expect("slow gate wait");
        }
    }

    fn wait_for_started(&self) {
        self.wait_for_started_count(1);
    }

    fn wait_for_started_count(&self, count: usize) {
        let deadline = Instant::now() + TEST_TIMEOUT;
        let mut state = self.state.lock().expect("slow gate lock");
        while state.started < count {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "slow request did not reach products");
            let (next, timeout) = self
                .changed
                .wait_timeout(state, remaining)
                .expect("slow gate wait");
            state = next;
            assert!(
                !timeout.timed_out() || state.started >= count,
                "slow request timed out"
            );
        }
    }

    fn release(&self) {
        let mut state = self.state.lock().expect("slow gate lock");
        state.released = true;
        self.changed.notify_all();
    }
}

fn handle_subgraph(stream: TcpStream, kind: SubgraphKind, state: SubgraphHandlerState) {
    state.opens.fetch_add(1, Ordering::AcqRel);
    let Ok(request) = read_http_request(stream) else {
        return;
    };
    if matches!(kind, SubgraphKind::Products) && has_header(&request.head, "upgrade", "websocket") {
        let _ = handle_subgraph_websocket(request, &state);
        return;
    }
    if has_header(&request.head, "x-approved", "yes") {
        state.saw_approved_header.store(true, Ordering::Release);
    }
    if has_header(&request.head, "x-blocked", "no") {
        state.saw_blocked_header.store(true, Ordering::Release);
    }
    if has_header(&request.head, "authorization", "Bearer product-p1") {
        state.saw_bearer_header.store(true, Ordering::Release);
    }
    let Ok(payload) = serde_json::from_slice::<Value>(&request.body) else {
        return;
    };
    let query = payload["query"].as_str().unwrap_or_default();

    if matches!(kind, SubgraphKind::Products) && query.contains("slow") {
        state.slow_gate.hold_until_released();
    }

    if matches!(kind, SubgraphKind::Products) && query.contains("subscription") {
        let (field, name) = if query.contains("productChangedV2") {
            ("productChangedV2", "v2")
        } else {
            ("productChanged", "v1")
        };
        if state.quiet.burst.load(Ordering::Acquire) {
            let mut stream = request.stream;
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n").unwrap();
            for sequence in 0..1000 {
                let event = json!({"data": {field: {"id": "p1", "name": sequence.to_string()}}});
                if writeln!(stream, "event: next\ndata: {event}\n").is_err() {
                    break;
                }
            }
            if state.quiet.burst_quiet_tail.load(Ordering::Acquire) {
                let _ = stream.flush();
                while !state.stopping.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(25));
                    if stream
                        .write_all(b": heartbeat\n\n")
                        .and_then(|_| stream.flush())
                        .is_err()
                    {
                        break;
                    }
                }
                return;
            }
            let _ = stream.write_all(b"event: complete\ndata: {}\n\n");
            let _ = stream.flush();
            return;
        }
        let event = json!({"data": {field: {"id": "p1", "name": name}}});
        let _ = write_sse_subscription(request.stream, &event, &state.stopping);
        return;
    }

    let response = match kind {
        SubgraphKind::Products if query.contains("_entities") => {
            json!({"data": {"_entities": [{"id": "p1", "name": "desk"}]}})
        }
        SubgraphKind::Products if query.contains("renameProduct") => {
            json!({"data": {"renameProduct": {"id": "p1", "name": "renamed"}}})
        }
        SubgraphKind::Products if query.contains("version") => json!({"data": {"version": "v2"}}),
        SubgraphKind::Products if query.contains("failure") => json!({
            "data": {"product": null},
            "errors": [{
                "message": "test-owned downstream failure",
                "path": ["product"],
                "extensions": {"code": "SUBGRAPH_FAILURE"}
            }]
        }),
        SubgraphKind::Products => {
            let id = if query.contains("slow") { "slow" } else { "p1" };
            json!({"data": {"product": {"id": id, "name": "desk"}}})
        }
        SubgraphKind::Reviews => {
            json!({"data": {"_entities": [{"reviews": [{"body": "excellent"}]}]}})
        }
    };
    let _ = write_http_response(
        request.stream,
        200,
        &serde_json::to_vec(&response).expect("response JSON"),
    );
}

fn handle_subgraph_websocket(
    mut request: HttpRequest,
    state: &SubgraphHandlerState,
) -> io::Result<()> {
    let key = header_value(&request.head, "sec-websocket-key")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing WebSocket key"))?;
    let mut digest = Sha1::new();
    digest.update(key.as_bytes());
    digest.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    let accept = BASE64.encode(digest.finalize());
    write!(
        request.stream,
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\nSec-WebSocket-Protocol: graphql-transport-ws\r\n\r\n"
    )?;
    request.stream.flush()?;

    let init = read_client_websocket_json(&mut request.stream)?;
    if init["type"] != "connection_init" {
        return write_server_websocket_close(&mut request.stream, 4400, "init required");
    }
    let authorization = init["payload"].as_object().and_then(|payload| {
        payload.iter().find_map(|(name, value)| {
            name.eq_ignore_ascii_case("authorization")
                .then(|| value.as_str())
                .flatten()
        })
    });
    if !matches!(
        authorization,
        Some("Bearer product-events" | "Bearer product-events-refreshed" | "Bearer short-lived")
    ) {
        return write_server_websocket_close(&mut request.stream, 4401, "unauthorized");
    }
    state.saw_bearer_header.store(true, Ordering::Release);
    if authorization == Some("Bearer product-events-refreshed") {
        state
            .quiet
            .saw_refreshed_bearer
            .store(true, Ordering::Release);
    }
    // Longer than the test deadline: an idle fixture timeout cannot make the
    // cancellation assertion pass. Dropping a connection releases its guards.
    if state.quiet.enabled.load(Ordering::Acquire) {
        request.stream.set_read_timeout(Some(TEST_TIMEOUT * 3))?;
    }
    let mut quiet_operations = std::collections::BTreeMap::new();
    write_server_websocket_json(&mut request.stream, &json!({"type": "connection_ack"}))?;

    loop {
        let message = match read_client_websocket_json(&mut request.stream) {
            Ok(message) => message,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::UnexpectedEof
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::BrokenPipe
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                ) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        match message["type"].as_str() {
            Some("subscribe") => {
                let id = message["id"].as_str().unwrap_or_default();
                if state.quiet.enabled.load(Ordering::Acquire) {
                    quiet_operations
                        .insert(id.to_owned(), QuietSubscription::new(state.quiet.clone()));
                    continue;
                }
                let query = message["payload"]["query"].as_str().unwrap_or_default();
                if query.contains("failure") {
                    write_server_websocket_close(&mut request.stream, 1011, "upstream failure")?;
                    return Ok(());
                }
                if query.contains("id: \"missing\"") {
                    continue;
                }
                let field = if query.contains("productChangedV2") {
                    "productChangedV2"
                } else {
                    "productChanged"
                };
                if state.quiet.burst.load(Ordering::Acquire) {
                    for sequence in 0..1000 {
                        write_server_websocket_json(
                            &mut request.stream,
                            &json!({
                                "type": "next", "id": id,
                                "payload": {"data": {field: {"id": "p1", "name": sequence.to_string()}}}
                            }),
                        )?;
                    }
                    if !state.quiet.burst_quiet_tail.load(Ordering::Acquire) {
                        write_server_websocket_json(
                            &mut request.stream,
                            &json!({"type": "complete", "id": id}),
                        )?;
                    }
                    continue;
                }
                let event = json!({
                    "type": "next",
                    "id": id,
                    "payload": {"data": {field: {"id": "p1", "name": "websocket"}}}
                });
                write_server_websocket_json(&mut request.stream, &event)?;
            }
            Some("complete") => {
                if let Some(id) = message["id"].as_str() {
                    quiet_operations.remove(id);
                }
            }
            Some("ping") => {
                write_server_websocket_json(&mut request.stream, &json!({"type": "pong"}))?;
            }
            Some("pong") => {}
            _ => return Ok(()),
        }
    }
}

fn header_value<'a>(head: &'a str, expected_name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|line| {
        line.split_once(':').and_then(|(name, value)| {
            name.eq_ignore_ascii_case(expected_name)
                .then(|| value.trim())
        })
    })
}

fn read_client_websocket_json(stream: &mut TcpStream) -> io::Result<Value> {
    loop {
        let (opcode, payload) = read_masked_websocket_frame(stream)?;
        match opcode {
            0x1 => {
                return serde_json::from_slice(&payload)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
            }
            0x8 => return Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
            0x9 => write_unmasked_websocket_frame(stream, 0xA, &payload)?,
            0xA => {}
            _ => return Err(io::Error::from(io::ErrorKind::InvalidData)),
        }
    }
}

fn read_masked_websocket_frame(stream: &mut TcpStream) -> io::Result<(u8, Vec<u8>)> {
    let mut header = [0_u8; 2];
    stream.read_exact(&mut header)?;
    if header[0] & 0x80 == 0 || header[1] & 0x80 == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid client WebSocket frame",
        ));
    }
    let opcode = header[0] & 0x0F;
    let length = websocket_payload_length(stream, header[1] & 0x7F)?;
    let mut mask = [0_u8; 4];
    stream.read_exact(&mut mask)?;
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload)?;
    for (index, byte) in payload.iter_mut().enumerate() {
        *byte ^= mask[index % mask.len()];
    }
    Ok((opcode, payload))
}

fn websocket_payload_length(stream: &mut TcpStream, discriminator: u8) -> io::Result<usize> {
    match discriminator {
        length @ 0..=125 => Ok(usize::from(length)),
        126 => {
            let mut bytes = [0_u8; 2];
            stream.read_exact(&mut bytes)?;
            Ok(usize::from(u16::from_be_bytes(bytes)))
        }
        127 => {
            let mut bytes = [0_u8; 8];
            stream.read_exact(&mut bytes)?;
            usize::try_from(u64::from_be_bytes(bytes)).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "WebSocket frame exceeds usize")
            })
        }
        _ => unreachable!("the WebSocket length discriminator is seven bits"),
    }
}

fn write_server_websocket_json(stream: &mut TcpStream, value: &Value) -> io::Result<()> {
    write_unmasked_websocket_frame(stream, 0x1, &serde_json::to_vec(value).unwrap())
}

fn write_server_websocket_close(stream: &mut TcpStream, code: u16, reason: &str) -> io::Result<()> {
    let mut payload = code.to_be_bytes().to_vec();
    payload.extend_from_slice(reason.as_bytes());
    write_unmasked_websocket_frame(stream, 0x8, &payload)
}

fn write_unmasked_websocket_frame(
    stream: &mut TcpStream,
    opcode: u8,
    payload: &[u8],
) -> io::Result<()> {
    stream.write_all(&[0x80 | opcode])?;
    match payload.len() {
        0..=125 => stream.write_all(&[payload.len() as u8])?,
        126..=65_535 => {
            stream.write_all(&[126])?;
            stream.write_all(&(payload.len() as u16).to_be_bytes())?;
        }
        _ => {
            stream.write_all(&[127])?;
            stream.write_all(&(payload.len() as u64).to_be_bytes())?;
        }
    }
    stream.write_all(payload)?;
    stream.flush()
}

fn write_sse_subscription(
    mut stream: TcpStream,
    event: &Value,
    stopping: &AtomicBool,
) -> io::Result<()> {
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n",
    )?;
    write!(
        stream,
        "event: next\ndata: {}\n\n",
        serde_json::to_string(event).expect("test subscription event should serialize")
    )?;
    stream.flush()?;

    // Keep the upstream stream open until Hive cancels it because its graph was
    // retired, or until fixture shutdown. Heartbeats make that cancellation
    // observable without relying on an unbounded parked worker thread.
    while !stopping.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(25));
        if stream.write_all(b": heartbeat\n\n").is_err() {
            break;
        }
        let _ = stream.flush();
    }
    Ok(())
}

struct HttpRequest {
    stream: TcpStream,
    head: String,
    body: Vec<u8>,
}

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

fn raw_http(
    address: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> io::Result<HttpResponse> {
    raw_http_within(address, method, path, headers, body, TEST_TIMEOUT)
}

/// Like [`raw_http`], but a response still streaming after `read_timeout`
/// is reported as an error instead of being awaited.
fn raw_http_within(
    address: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
    read_timeout: Duration,
) -> io::Result<HttpResponse> {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(read_timeout))?;
    stream.set_write_timeout(Some(TEST_TIMEOUT))?;
    let body = body.unwrap_or_default();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    )?;
    if !body.is_empty() {
        stream.write_all(b"Content-Type: application/json\r\n")?;
    }
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    stream.write_all(b"\r\n")?;
    stream.write_all(body)?;
    read_http_response(stream)
}

fn read_http_request(mut stream: TcpStream) -> io::Result<HttpRequest> {
    stream.set_read_timeout(Some(TEST_TIMEOUT))?;
    let (head, mut body) = read_head_and_body(&mut stream)?;
    read_http_body(&mut stream, &head, &mut body)?;
    Ok(HttpRequest { stream, head, body })
}

fn has_header(head: &str, expected_name: &str, expected_value: &str) -> bool {
    head.lines().skip(1).any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case(expected_name) && value.trim() == expected_value
        })
    })
}

fn read_http_response(mut stream: TcpStream) -> io::Result<HttpResponse> {
    let (head, mut body) = read_head_and_body(&mut stream)?;
    let status = head
        .split("\r\n")
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid HTTP status"))?;
    read_http_body(&mut stream, &head, &mut body)?;
    Ok(HttpResponse { status, body })
}

fn read_http_body(stream: &mut TcpStream, head: &str, body: &mut Vec<u8>) -> io::Result<()> {
    if head.lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("transfer-encoding")
                && value
                    .split(',')
                    .any(|value| value.trim().eq_ignore_ascii_case("chunked"))
        })
    }) {
        *body = decode_chunked_body(stream, std::mem::take(body))?;
    } else {
        read_remaining(stream, body, content_length(head)?)?;
    }
    Ok(())
}

fn read_head_and_body(stream: &mut TcpStream) -> io::Result<(String, Vec<u8>)> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "HTTP head ended early",
            ));
        }
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let body_start = position + 4;
            let head = String::from_utf8(bytes[..position].to_vec())
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            return Ok((head, bytes[body_start..].to_vec()));
        }
        if bytes.len() > 64 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP head too large",
            ));
        }
    }
}

fn content_length(head: &str) -> io::Result<usize> {
    head.lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim().parse::<usize>())
        .transpose()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
        .map(|length| length.unwrap_or(0))
}

fn read_remaining(stream: &mut TcpStream, body: &mut Vec<u8>, length: usize) -> io::Result<()> {
    while body.len() < length {
        let mut buffer = vec![0_u8; length - body.len()];
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "HTTP body ended early",
            ));
        }
        body.extend_from_slice(&buffer[..read]);
    }
    body.truncate(length);
    Ok(())
}

fn decode_chunked_body(stream: &mut TcpStream, mut buffered: Vec<u8>) -> io::Result<Vec<u8>> {
    let mut decoded = Vec::new();
    loop {
        let line_end = loop {
            if let Some(position) = buffered.windows(2).position(|window| window == b"\r\n") {
                break position;
            }
            read_more(stream, &mut buffered)?;
        };
        let length = std::str::from_utf8(&buffered[..line_end])
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
            .split(';')
            .next()
            .expect("chunk length always has a first segment");
        let length = usize::from_str_radix(length.trim(), 16)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let data_start = line_end + 2;
        let required = data_start + length + 2;
        while buffered.len() < required {
            read_more(stream, &mut buffered)?;
        }
        if &buffered[data_start + length..required] != b"\r\n" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid chunk terminator",
            ));
        }
        if length == 0 {
            return Ok(decoded);
        }
        decoded.extend_from_slice(&buffered[data_start..data_start + length]);
        buffered.drain(..required);
    }
}

fn read_more(stream: &mut TcpStream, buffered: &mut Vec<u8>) -> io::Result<()> {
    let mut chunk = [0_u8; 1024];
    let read = stream.read(&mut chunk)?;
    if read == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "chunked HTTP body ended early",
        ));
    }
    buffered.extend_from_slice(&chunk[..read]);
    Ok(())
}

fn write_http_response(mut stream: TcpStream, status: u16, body: &[u8]) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}

fn endpoint_origin(endpoint: &str) -> String {
    let mut url = Url::parse(endpoint).expect("test endpoint URL");
    url.set_path("");
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn wait_until(description: &str, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + TEST_TIMEOUT;
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {description}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_graphql(mut request: impl FnMut() -> Value) -> Value {
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        let response = request();
        if response
            .get("data")
            .and_then(|data| data.get("version"))
            .is_some_and(Value::is_string)
        {
            return response;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the replacement GraphQL schema: {response}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}
