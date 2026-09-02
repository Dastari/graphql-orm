use std::{
    collections::BTreeMap,
    future::Future,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use futures::future::try_join_all;
use hive_router::{
    GraphQLError, PlanExecutionOutput, PluginRegistry, RouterPaths, background_tasks,
    ntex::web,
    pipeline::request_identifiers::RequestIdentifiersService,
    plugins::{
        hooks::{
            on_execute::{OnExecuteStartHookPayload, OnExecuteStartHookResult},
            on_graphql_analysis::{OnGraphqlAnalysisHookPayload, OnGraphqlAnalysisHookResult},
            on_graphql_error::{OnGraphQLErrorHookPayload, OnGraphQLErrorHookResult},
            on_http_request::{OnHttpRequestHookPayload, OnHttpRequestHookResult},
            on_plugin_init::{OnPluginInitPayload, OnPluginInitResult},
            on_subgraph_http_request::{
                OnSubgraphHttpRequestHookPayload, OnSubgraphHttpRequestHookResult,
            },
        },
        plugin_trait::{
            EndHookPayload, FromGraphQLErrorsToResponse, RouterPlugin, StartHookPayload,
        },
        plugins_service::PluginService,
    },
    sonic_rs::JsonContainerTrait,
};
use hive_router_config::HiveRouterConfig;
use reqwest::{Client, redirect::Policy};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::VariableResolution;
use crate::{
    AuthenticatedPrincipal, AuthenticationProvider, RouterConfig, RouterError, RouterErrorKind,
    ScopeMatcher,
    admin::{AdminRuntime, build_admin_server},
    federation::ActiveGraph,
    lifecycle::{GraphLifecycle, RouterHandle, fetch_initial},
    subscriptions::{
        INTERNAL_SUBSCRIPTION_HEADER, INTERNAL_SUBSCRIPTION_VARIABLES_EXTENSION,
        InternalSubscriptionEndpoint, SubscriptionGateway, websocket_index,
    },
};

const STATIC_GRAPH_PLUGIN: &str = "graphql-orm-router-static-graph";
static NEXT_RUNTIME_ID: AtomicU64 = AtomicU64::new(1);
static PLUGIN_RUNTIMES: OnceLock<Mutex<BTreeMap<u64, PluginRuntime>>> = OnceLock::new();

/// Prepares and runs a router on its owned Ntex runtime.
///
/// Embedders that already manage an asynchronous runtime can instead await
/// [`RouterConfig::prepare`] and [`PreparedRouter::run`] directly.
pub fn run(config: RouterConfig) -> Result<(), RouterError> {
    #[cfg(target_family = "unix")]
    let process_signals = ProcessSignals::install()?;

    let system = hive_router::ntex::rt::System::build().name("graphql-orm-router");
    #[cfg(not(target_family = "unix"))]
    let system = system.enable_signals();

    let result = system
        .build(hive_router::ntex::rt::DefaultRuntime)
        .block_on(async move {
            #[cfg(target_family = "unix")]
            let shutdown = async move {
                let _ = process_signals.receiver.await;
            };
            #[cfg(not(target_family = "unix"))]
            let shutdown = async {
                let _ = hive_router::ntex::rt::signals::signal().await;
            };

            config.prepare().await?.run_until_shutdown(shutdown).await
        });

    #[cfg(target_family = "unix")]
    {
        process_signals.handle.close();
        let _ = process_signals.thread.join();
    }
    result
}

#[cfg(target_family = "unix")]
struct ProcessSignals {
    receiver: futures::channel::oneshot::Receiver<()>,
    handle: signal_hook::iterator::Handle,
    thread: std::thread::JoinHandle<()>,
}

#[cfg(target_family = "unix")]
impl ProcessSignals {
    fn install() -> Result<Self, RouterError> {
        use signal_hook::{
            consts::signal::{SIGINT, SIGQUIT, SIGTERM},
            iterator::Signals,
        };

        let mut signals = Signals::new([SIGINT, SIGQUIT, SIGTERM]).map_err(|error| {
            RouterError::new(
                RouterErrorKind::Runtime,
                format!("failed to install process signal handlers: {error}"),
            )
        })?;
        let handle = signals.handle();
        let close_on_error = handle.clone();
        let (sender, receiver) = futures::channel::oneshot::channel();
        let thread = std::thread::Builder::new()
            .name("graphql-orm-router-signals".to_owned())
            .spawn(move || {
                if signals.forever().next().is_some() {
                    let _ = sender.send(());
                }
            })
            .map_err(|error| {
                close_on_error.close();
                RouterError::new(
                    RouterErrorKind::Runtime,
                    format!("failed to start process signal listener: {error}"),
                )
            })?;
        Ok(Self {
            receiver,
            handle,
            thread,
        })
    }
}

/// Public identity of one complete executable graph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveGraphIdentity {
    version: u64,
    fingerprint: String,
}

impl ActiveGraphIdentity {
    /// Monotonic process-local graph version, starting at one.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// SHA-256 identity of the normalized executable supergraph.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

impl From<&ActiveGraph> for ActiveGraphIdentity {
    fn from(graph: &ActiveGraph) -> Self {
        Self {
            version: graph.version,
            fingerprint: graph.fingerprint.clone(),
        }
    }
}

/// A fully fetched, composed, and executable static graph ready to bind.
pub struct PreparedRouter {
    config: RouterConfig,
    lifecycle: Arc<GraphLifecycle>,
    identity: ActiveGraphIdentity,
    composition_warnings: Vec<String>,
}

impl RouterConfig {
    /// Fetches every configured SDL and builds the complete graph without
    /// opening the public listener.
    pub async fn prepare(self) -> Result<PreparedRouter, RouterError> {
        PreparedRouter::prepare(self).await
    }
}

impl PreparedRouter {
    async fn prepare(config: RouterConfig) -> Result<Self, RouterError> {
        let config = config.validate()?;
        if let Some(provider) = &config.authentication {
            provider.initialize().await.map_err(|error| {
                RouterError::new(
                    RouterErrorKind::Runtime,
                    format!("authentication provider failed to initialize: {error}"),
                )
            })?;
        }
        let client = Client::builder()
            .redirect(Policy::none())
            .timeout(config.schema_fetch_timeout)
            .build()
            .map_err(|error| {
                RouterError::new(
                    RouterErrorKind::InvalidConfiguration,
                    format!("failed to construct the schema client: {error}"),
                )
            })?;
        let fetched = try_join_all(
            config
                .subgraphs
                .iter()
                .map(|subgraph| fetch_initial(&client, subgraph, config.max_sdl_bytes)),
        )
        .await?;
        let lifecycle = GraphLifecycle::new(
            &config.subgraphs,
            fetched,
            client,
            config.max_sdl_bytes,
            config.schema_refresh_attempts,
            config.schema_refresh_retry_delay,
            config.schema_poll_interval,
            config.authentication.is_some(),
            config.subscriptions.is_some(),
        )?;
        let active = lifecycle.load();
        let graph = &active.graph;
        let identity = ActiveGraphIdentity::from(graph.as_ref());
        let composition_warnings = graph
            .warnings
            .iter()
            .map(|warning| warning.message.clone())
            .collect();
        Ok(Self {
            config,
            lifecycle,
            identity,
            composition_warnings,
        })
    }

    /// Returns the identity selected only after complete startup validation.
    pub fn active_graph(&self) -> &ActiveGraphIdentity {
        &self.identity
    }

    /// Returns non-fatal composition warnings produced during preparation.
    pub fn composition_warnings(&self) -> &[String] {
        &self.composition_warnings
    }

    /// Returns a process-local lifecycle handle that remains valid while the
    /// prepared router runs.
    pub fn handle(&self) -> RouterHandle {
        RouterHandle::new(self.lifecycle.clone())
    }

    /// Returns true because preparation publishes only a complete executable
    /// graph and fails instead of constructing an unready value.
    pub fn is_ready(&self) -> bool {
        true
    }

    /// Runs the public GraphQL, liveness, and readiness HTTP server until it
    /// is stopped by the surrounding Ntex runtime.
    pub async fn run(self) -> Result<(), RouterError> {
        self.run_until_shutdown(futures::future::pending()).await
    }

    /// Runs until a listener fails or `shutdown` resolves, then stops accepting
    /// new work, drains the Ntex servers, stops background tasks, flushes
    /// telemetry, and invokes private engine shutdown hooks.
    pub async fn run_until_shutdown<F>(self, shutdown: F) -> Result<(), RouterError>
    where
        F: Future<Output = ()>,
    {
        hive_router::init_rustls_crypto_provider();
        let runtime_id = NEXT_RUNTIME_ID.fetch_add(1, Ordering::Relaxed);
        let internal_subscription = if self.config.subscriptions.is_some() {
            Some(InternalSubscriptionEndpoint::generate()?)
        } else {
            None
        };
        let runtime = PluginRuntime {
            lifecycle: self.lifecycle.clone(),
            authentication: self.config.authentication.clone(),
            scope_matcher: self.config.scope_matcher.clone(),
            internal_subscription: internal_subscription.clone(),
            request_limits: self.config.request_limits.clone(),
        };
        plugin_runtimes()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(runtime_id, runtime);
        let hive_config =
            match build_hive_config(&self.config, runtime_id, internal_subscription.as_ref()) {
                Ok(config) => config,
                Err(error) => {
                    remove_plugin_runtime(runtime_id);
                    return Err(error);
                }
            };
        let telemetry =
            hive_router::telemetry::Telemetry::init_global(&hive_config).map_err(|error| {
                remove_plugin_runtime(runtime_id);
                RouterError::new(
                    RouterErrorKind::Runtime,
                    format!("failed to initialize router telemetry: {error}"),
                )
            })?;
        let mut background_tasks = background_tasks::BackgroundTasksManager::new();
        let configured = hive_router::configure_app_from_config(
            hive_config,
            telemetry.context.clone(),
            &mut background_tasks,
            PluginRegistry::new().register::<StaticGraphPlugin>(),
        )
        .await;
        let (shared_state, schema_state) = match configured {
            Ok(configured) => configured,
            Err(error) => {
                remove_plugin_runtime(runtime_id);
                background_tasks.shutdown();
                telemetry.graceful_shutdown().await;
                return Err(RouterError::new(
                    RouterErrorKind::Runtime,
                    format!("failed to initialize the executable router: {error}"),
                ));
            }
        };

        let listener = self.config.listener;
        let paths = RouterPaths::new(
            self.config.graphql_path.clone(),
            internal_subscription
                .as_ref()
                .map(|endpoint| endpoint.path.clone()),
            None,
        );
        let plugin_paths = paths.clone();
        let callback_subscriptions = schema_state.callback_subscriptions.clone();
        let server_shared_state = shared_state.clone();
        let server_schema_state = schema_state.clone();
        let telemetry_context = shared_state.telemetry_context.clone();
        let subscription_gateway = self.config.subscriptions.clone().map(|config| {
            SubscriptionGateway::new(
                config,
                self.config
                    .authentication
                    .clone()
                    .expect("subscription configuration requires authentication"),
                internal_subscription
                    .clone()
                    .expect("subscription configuration has a private endpoint"),
                listener,
                self.lifecycle.metrics(),
            )
        });
        let public_graphql_path = self.config.graphql_path.clone();
        let admin_runtime = self.config.admin.clone().map(|admin| {
            AdminRuntime::new(
                self.lifecycle.clone(),
                self.config
                    .authentication
                    .clone()
                    .expect("validated administrative configuration requires authentication"),
                self.config.scope_matcher.clone(),
                admin,
                self.config.schema_fetch_timeout,
                self.config.max_sdl_bytes,
                u16::try_from(self.config.graceful_shutdown_timeout.as_secs())
                    .expect("validated graceful shutdown timeout fits u16"),
            )
        });
        let graceful_shutdown_timeout = hive_router::ntex::time::Seconds(
            u16::try_from(self.config.graceful_shutdown_timeout.as_secs())
                .expect("validated graceful shutdown timeout fits u16"),
        );
        let server =
            web::HttpServer::new(async move || {
                let paths = paths.clone();
                let subscription_gateway = subscription_gateway.clone();
                let public_graphql_path = public_graphql_path.clone();
                web::App::new()
                    .middleware(PluginService::new(plugin_paths.clone(), None))
                    .middleware(RequestIdentifiersService)
                    .state(server_shared_state.clone())
                    .state(server_schema_state.clone())
                    .state(telemetry_context.clone())
                    .state(callback_subscriptions.clone())
                    .configure(move |routes| {
                        if let Some(gateway) = subscription_gateway.clone() {
                            routes.service(
                                web::resource(public_graphql_path.as_str())
                                    .guard(web::guard::fn_guard(|head| {
                                        head.headers()
                                            .get(hive_router::ntex::http::header::UPGRADE)
                                            .and_then(|value| value.to_str().ok())
                                            .is_some_and(|value| {
                                                value.eq_ignore_ascii_case("websocket")
                                            })
                                    }))
                                    .route(web::get().to(move |request| {
                                        websocket_index(request, gateway.clone())
                                    })),
                            );
                        }
                        hive_router::configure_ntex_app(routes, &paths, None);
                    })
            })
            .shutdown_timeout(graceful_shutdown_timeout);
        #[cfg(target_family = "unix")]
        let server = server.disable_signals();
        let server = server.bind(listener).map_err(|error| {
            RouterError::new(
                RouterErrorKind::Server,
                format!("failed to bind public listener {listener}: {error}"),
            )
        });

        let admin_server = admin_runtime.map(build_admin_server).transpose();
        let servers = match (server, admin_server) {
            (Ok(server), Ok(Some(admin))) => Ok(vec![server.run(), admin]),
            (Ok(server), Ok(None)) => Ok(vec![server.run()]),
            (_, Err(error)) => Err(error),
            (Err(error), _) => Err(error),
        };
        let result = match servers {
            Ok(servers) => {
                let controls = servers.clone();
                let serving = futures::future::try_join_all(servers);
                match futures::future::select(Box::pin(serving), Box::pin(shutdown)).await {
                    futures::future::Either::Left((result, _)) => result,
                    futures::future::Either::Right(((), serving)) => {
                        futures::future::join_all(controls.iter().map(|server| server.stop(true)))
                            .await;
                        serving.await
                    }
                }
                .map(|_| ())
                .map_err(|error| {
                    RouterError::new(
                        RouterErrorKind::Server,
                        format!("router listener failed: {error}"),
                    )
                })
            }
            Err(error) => Err(error),
        };
        background_tasks.shutdown();
        telemetry.graceful_shutdown().await;
        hive_router::invoke_shutdown_hooks(&shared_state).await;
        remove_plugin_runtime(runtime_id);
        result
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StaticGraphPluginConfig {
    runtime_id: u64,
}

struct StaticGraphPlugin {
    runtime: PluginRuntime,
}

struct SelectedRouterPolicy(Option<Arc<crate::auth::AuthorizationCatalog>>);

#[derive(Clone, Copy)]
struct TrustedInternalSubscription;

#[derive(Clone)]
struct PluginRuntime {
    lifecycle: Arc<GraphLifecycle>,
    authentication: Option<Arc<dyn AuthenticationProvider>>,
    scope_matcher: Arc<dyn ScopeMatcher>,
    internal_subscription: Option<InternalSubscriptionEndpoint>,
    request_limits: crate::RequestLimits,
}

fn plugin_runtimes() -> &'static Mutex<BTreeMap<u64, PluginRuntime>> {
    PLUGIN_RUNTIMES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn remove_plugin_runtime(runtime_id: u64) -> Option<PluginRuntime> {
    plugin_runtimes()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&runtime_id)
}

#[hive_router::async_trait]
impl RouterPlugin for StaticGraphPlugin {
    type Config = StaticGraphPluginConfig;

    fn plugin_name() -> &'static str {
        STATIC_GRAPH_PLUGIN
    }

    fn on_plugin_init(mut payload: OnPluginInitPayload<Self>) -> OnPluginInitResult<Self> {
        let config = payload.config()?;
        let Some(runtime) = remove_plugin_runtime(config.runtime_id) else {
            return OnPluginInitPayload::<Self>::error(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "prepared router runtime is unavailable",
            ));
        };
        let active = runtime.lifecycle.load();
        if let Some(authorization) = &active.authorization
            && let Err(error) = authorization.ensure_bound_to(&active.graph)
        {
            return OnPluginInitPayload::<Self>::error(error);
        }
        if let Some(provider) = &runtime.authentication
            && let Some(interval) = provider.refresh_interval()
        {
            payload.register_background_task(AuthenticationRefreshTask {
                provider: provider.clone(),
                interval,
            });
        }
        payload.register_background_task(SchemaRefreshTask {
            lifecycle: runtime.lifecycle.clone(),
            interval: runtime.lifecycle.poll_interval(),
        });
        payload.initialize_plugin(Self { runtime })
    }

    fn on_http_request<'request>(
        &'request self,
        payload: OnHttpRequestHookPayload<'request>,
    ) -> OnHttpRequestHookResult<'request> {
        self.runtime.lifecycle.metrics().graphql_request();
        let selected = self.runtime.lifecycle.load();
        payload.set_supergraph(selected.graph.hive.clone());
        payload
            .context
            .insert(SelectedRouterPolicy(selected.authorization.clone()));
        if let Some(internal) = &self.runtime.internal_subscription {
            let is_internal_subscription = payload.router_http_request.path() == internal.path;
            let supplied = payload
                .router_http_request
                .headers()
                .get(INTERNAL_SUBSCRIPTION_HEADER)
                .and_then(|value| value.to_str().ok());
            if !internal.authorizes(payload.router_http_request.path(), supplied) {
                return invalid_bearer_response(payload);
            }
            if is_internal_subscription {
                payload.context.insert(TrustedInternalSubscription);
            }
        }
        let Some(provider) = &self.runtime.authentication else {
            return payload.proceed();
        };
        let Some(authorization) = payload
            .router_http_request
            .headers()
            .get(hive_router::http::header::AUTHORIZATION)
        else {
            return payload.proceed();
        };
        let Ok(authorization) = authorization.to_str() else {
            return invalid_bearer_response(payload);
        };
        let Some(token) = strict_bearer_token(authorization) else {
            return invalid_bearer_response(payload);
        };
        match provider.authenticate_bearer(token) {
            Ok(principal) => {
                payload.context.insert(principal);
                payload.proceed()
            }
            Err(error) => {
                let unavailable =
                    matches!(error.kind(), crate::AuthenticationErrorKind::Unavailable);
                payload.end_with_graphql_error(
                    GraphQLError::from_message_and_code(
                        if unavailable {
                            "authentication service unavailable"
                        } else {
                            "invalid bearer credential"
                        },
                        "UNAUTHENTICATED",
                    ),
                    if unavailable {
                        hive_router::http::StatusCode::SERVICE_UNAVAILABLE
                    } else {
                        hive_router::http::StatusCode::UNAUTHORIZED
                    },
                )
            }
        }
    }

    async fn on_graphql_analysis<'execution>(
        &'execution self,
        payload: &mut OnGraphqlAnalysisHookPayload<'execution>,
    ) -> OnGraphqlAnalysisHookResult {
        if payload.filtered_operation_for_plan.selection_set.cost()
            > self.runtime.request_limits.max_fields as u64
        {
            return OnGraphqlAnalysisHookResult::EndWithResponse(
                PlanExecutionOutput::from_graphql_errors_to_response(
                    vec![GraphQLError::from_message_and_code(
                        "GraphQL operation exceeds the configured field limit",
                        "OPERATION_LIMIT_EXCEEDED",
                    )],
                    hive_router::http::StatusCode::BAD_REQUEST,
                ),
            );
        }
        let selected = payload.context.get_ref::<SelectedRouterPolicy>();
        let Some(catalog) = selected
            .as_deref()
            .and_then(|selected| selected.0.as_deref())
        else {
            if self.runtime.authentication.is_some() {
                return OnGraphqlAnalysisHookResult::EndWithResponse(
                    PlanExecutionOutput::from_graphql_errors_to_response(
                        vec![GraphQLError::from_message_and_code(
                            "authorization state is unavailable",
                            "UNAUTHENTICATED",
                        )],
                        hive_router::http::StatusCode::SERVICE_UNAVAILABLE,
                    ),
                );
            }
            return OnGraphqlAnalysisHookResult::Proceed;
        };
        let principal = payload.context.get_ref::<AuthenticatedPrincipal>();
        // Hive has already moved supplied values into its coerced-variable
        // payload at this hook. The public WebSocket gateway therefore copies
        // a bounded projection of authorization-capable scalar values into a
        // reserved extension before it enters Hive. Only the authenticated
        // private endpoint may activate that extension; ordinary HTTP clients
        // cannot spoof it. Values omitted by the bound fail closed under
        // complete variable resolution.
        let trusted_subscription = payload
            .context
            .get_ref::<TrustedInternalSubscription>()
            .is_some();
        let subscription_variables = trusted_subscription.then(|| {
            payload
                .graphql_params
                .extensions
                .as_ref()
                .and_then(|extensions| extensions.get(INTERNAL_SUBSCRIPTION_VARIABLES_EXTENSION))
                .and_then(|variables| variables.as_object())
                .map(|variables| {
                    variables
                        .iter()
                        .map(|(name, value)| (name.to_owned(), value.clone()))
                        .collect::<std::collections::HashMap<_, _>>()
                })
                .unwrap_or_default()
        });
        let variables = subscription_variables
            .as_ref()
            .unwrap_or(&payload.graphql_params.variables);
        let denials = catalog.authorize_operation(
            payload.filtered_operation_for_plan,
            variables,
            principal.as_deref(),
            self.runtime.scope_matcher.as_ref(),
            if trusted_subscription {
                VariableResolution::Complete
            } else {
                VariableResolution::Preflight
            },
        );
        if denials.is_empty() {
            return OnGraphqlAnalysisHookResult::Proceed;
        }
        self.runtime
            .lifecycle
            .metrics()
            .authorization_denied(denials.len());
        let errors = denials
            .into_iter()
            .map(|denial| GraphQLError::from_message_and_code(denial.message, denial.code))
            .collect::<Vec<_>>();
        OnGraphqlAnalysisHookResult::EndWithResponse(
            PlanExecutionOutput::from_graphql_errors_to_response(
                errors,
                hive_router::http::StatusCode::OK,
            ),
        )
    }

    async fn on_execute<'execution>(
        &'execution self,
        payload: OnExecuteStartHookPayload<'execution>,
    ) -> OnExecuteStartHookResult<'execution> {
        let selected = payload.context.get_ref::<SelectedRouterPolicy>();
        let Some(catalog) = selected
            .as_deref()
            .and_then(|selected| selected.0.as_deref())
        else {
            if self.runtime.authentication.is_some() {
                return payload.end_with_graphql_error(
                    GraphQLError::from_message_and_code(
                        "authorization state is unavailable",
                        "UNAUTHENTICATED",
                    ),
                    hive_router::http::StatusCode::SERVICE_UNAVAILABLE,
                );
            }
            return payload.proceed();
        };
        let principal = payload.context.get_ref::<AuthenticatedPrincipal>();
        // WebSocket plugin context is connection-scoped, so never cache an
        // operation's variables there. The execute hook owns the exact coerced
        // values for this HTTP or WebSocket operation and still runs before any
        // subgraph execution.
        let empty_variables = std::collections::HashMap::new();
        let variables = payload.variable_values.as_ref().unwrap_or(&empty_variables);
        let denials = catalog.authorize_operation(
            payload.operation_for_plan,
            variables,
            principal.as_deref(),
            self.runtime.scope_matcher.as_ref(),
            VariableResolution::Complete,
        );
        if denials.is_empty() {
            return payload.proceed();
        }
        self.runtime
            .lifecycle
            .metrics()
            .authorization_denied(denials.len());
        payload.end_with_graphql_errors(
            denials
                .into_iter()
                .map(|denial| GraphQLError::from_message_and_code(denial.message, denial.code)),
            hive_router::http::StatusCode::OK,
        )
    }

    async fn on_subgraph_http_request<'execution>(
        &'execution self,
        payload: OnSubgraphHttpRequestHookPayload<'execution>,
    ) -> OnSubgraphHttpRequestHookResult<'execution> {
        let metrics = self.runtime.lifecycle.metrics();
        metrics.subgraph_request();
        let started = Instant::now();
        if self
            .runtime
            .lifecycle
            .validate_execution_target(payload.subgraph_name, &payload.endpoint.to_string())
            .await
            .is_err()
        {
            return payload.end_with_graphql_error(
                GraphQLError::from_message_and_code(
                    "dynamic subgraph destination failed network policy",
                    "SUBGRAPH_UNAVAILABLE",
                ),
                hive_router::http::StatusCode::SERVICE_UNAVAILABLE,
            );
        }
        payload.on_end(move |payload| {
            metrics.subgraph_response(
                u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
                payload.response.status.is_success(),
            );
            payload.proceed()
        })
    }

    fn on_graphql_error<'request>(
        &'request self,
        payload: OnGraphQLErrorHookPayload<'request>,
    ) -> OnGraphQLErrorHookResult<'request> {
        self.runtime.lifecycle.metrics().graphql_error();
        payload.proceed()
    }
}

struct AuthenticationRefreshTask {
    provider: Arc<dyn AuthenticationProvider>,
    interval: std::time::Duration,
}

struct SchemaRefreshTask {
    lifecycle: Arc<GraphLifecycle>,
    interval: std::time::Duration,
}

#[hive_router::async_trait]
impl background_tasks::BackgroundTask for SchemaRefreshTask {
    fn id(&self) -> &str {
        "graphql-orm-router-schema-refresh"
    }

    async fn run(&self, token: background_tasks::CancellationToken) {
        loop {
            hive_router::tokio::select! {
                _ = token.cancelled() => return,
                _ = hive_router::tokio::time::sleep(self.interval) => {}
            }
            // Keep refresh cancellation-aware after the timer has fired. Dropping
            // the serialized refresh future before its one publication point
            // preserves the exact active snapshot during graceful shutdown.
            hive_router::tokio::select! {
                _ = token.cancelled() => return,
                _ = self.lifecycle.refresh() => {}
            }
        }
    }
}

#[hive_router::async_trait]
impl background_tasks::BackgroundTask for AuthenticationRefreshTask {
    fn id(&self) -> &str {
        "graphql-orm-router-authentication-refresh"
    }

    async fn run(&self, token: background_tasks::CancellationToken) {
        loop {
            hive_router::tokio::select! {
                _ = token.cancelled() => return,
                _ = hive_router::tokio::time::sleep(self.interval) => {
                    let _ = self.provider.refresh().await;
                }
            }
        }
    }
}

fn invalid_bearer_response(payload: OnHttpRequestHookPayload<'_>) -> OnHttpRequestHookResult<'_> {
    payload.end_with_graphql_error(
        GraphQLError::from_message_and_code("invalid bearer credential", "UNAUTHENTICATED"),
        hive_router::http::StatusCode::UNAUTHORIZED,
    )
}

pub(crate) fn strict_bearer_token(authorization: &str) -> Option<&str> {
    let mut parts = authorization.split(' ');
    let scheme = parts.next()?;
    let token = parts.next()?;
    if !scheme.eq_ignore_ascii_case("bearer")
        || token.is_empty()
        || token.bytes().any(|byte| byte.is_ascii_whitespace())
        || parts.next().is_some()
    {
        return None;
    }
    Some(token)
}

fn build_hive_config(
    config: &RouterConfig,
    runtime_id: u64,
    internal_subscription: Option<&InternalSubscriptionEndpoint>,
) -> Result<HiveRouterConfig, RouterError> {
    let plugin_config = StaticGraphPluginConfig { runtime_id };
    let mut request_rules = config
        .forwarded_headers
        .iter()
        .map(|name| json!({"propagate": {"named": name}}))
        .collect::<Vec<_>>();
    if config.authentication.is_some() {
        request_rules.push(json!({"propagate": {"named": "authorization"}}));
    }
    let mut plugins = BTreeMap::new();
    plugins.insert(
        STATIC_GRAPH_PLUGIN,
        json!({"enabled": true, "config": plugin_config}),
    );
    let websocket = internal_subscription.map_or_else(
        || json!({"enabled": false}),
        |endpoint| {
            json!({
                "enabled": true,
                "path": endpoint.path,
                "headers": {"source": "connection", "persist": false}
            })
        },
    );
    let subscriptions = config.subscriptions.as_ref().map_or_else(
        || json!({"enabled": false}),
        |subscriptions| {
            let subgraphs = config
                .subgraphs
                .iter()
                .filter_map(|subgraph| {
                    subgraph
                        .subscription_websocket_path
                        .as_ref()
                        .map(|path| (subgraph.name.clone(), json!({"path": path})))
                })
                .collect::<BTreeMap<_, _>>();
            json!({
                "enabled": true,
                "broadcast_capacity": subscriptions.broadcast_capacity,
                "subgraph_buffer_capacity": subscriptions.subgraph_buffer_capacity,
                "websocket": {"all": {}, "subgraphs": subgraphs}
            })
        },
    );
    let max_long_lived_clients = config
        .subscriptions
        .as_ref()
        .map_or(1_000, |subscriptions| subscriptions.max_connections);
    let value = json!({
        "supergraph": {"source": "plugin"},
        "http": {
            "host": config.listener.ip().to_string(),
            "port": config.listener.port(),
            "graphql_endpoint": config.graphql_path,
        },
        "laboratory": {"enabled": false},
        "headers": {"all": {"request": request_rules}},
        "websocket": websocket,
        "subscriptions": subscriptions,
        "traffic_shaping": {
            "all": {
                "request_timeout": format!("{}ms", config.subgraph_request_timeout.as_millis())
            },
            "max_connections_per_host": config.max_subgraph_connections_per_host,
            "router": {
                "request_timeout": format!("{}ms", config.public_request_timeout.as_millis()),
                "max_long_lived_clients": max_long_lived_clients,
                "dedupe": {"enabled": false, "headers": {"include": ["authorization"]}}
            }
        },
        "limits": {
            "max_request_body_size": format!("{}B", config.request_limits.max_request_body_bytes),
            "max_request_header_size": format!("{}B", config.request_limits.max_request_header_bytes),
            "max_tokens": {"n": config.request_limits.max_parser_tokens},
            "max_depth": {
                "n": config.request_limits.max_depth,
                "ignore_introspection": false,
                "flatten_fragments": true
            },
            "max_aliases": {"n": config.request_limits.max_aliases},
            "max_directives": {"n": config.request_limits.max_directives}
        },
        "log": {
            "level": config.telemetry.log_level.as_str(),
            "format": if config.telemetry.json_logs { "json" } else { "text" },
            "log_internals": false
        },
        "telemetry": config.telemetry.prometheus_port.map_or_else(
            || json!({}),
            |port| json!({
                "metrics": {
                    "exporters": [{
                        "kind": "prometheus",
                        "enabled": true,
                        "port": port,
                        "path": config.telemetry.prometheus_path.as_str(),
                    }]
                }
            }),
        ),
        "plugins": plugins,
    });
    serde_json::from_value::<HiveRouterConfig>(value).map_err(|error| {
        RouterError::new(
            RouterErrorKind::InvalidConfiguration,
            format!("failed to translate validated router configuration: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StaticSubgraph;

    #[derive(Debug)]
    struct RejectingProvider;

    impl AuthenticationProvider for RejectingProvider {
        fn authenticate_bearer(
            &self,
            _token: &str,
        ) -> Result<AuthenticatedPrincipal, crate::AuthenticationError> {
            Err(crate::AuthenticationError::invalid_credential("rejected"))
        }
    }

    #[test]
    fn private_hive_configuration_contains_only_approved_header_rules() {
        let config = RouterConfig::new("127.0.0.1:4000".parse().unwrap())
            .allow_anonymous_development(true)
            .with_subgraph(StaticSubgraph::new(
                "status",
                "http://status.test/graphql",
                "http://status.test/sdl",
            ))
            .forward_header("x-request-id")
            .validate()
            .unwrap();
        let hive = build_hive_config(&config, 7, None).unwrap();
        let serialized = serde_json::to_value(hive).unwrap();
        assert_eq!(
            serialized["headers"]["all"]["request"][0]["propagate"]["named"],
            "x-request-id"
        );
        assert_eq!(serialized["supergraph"]["source"], "plugin");
        assert_eq!(serialized["websocket"]["enabled"], false);
        assert_eq!(serialized["limits"]["max_request_body_size"], "1048576 B");
        assert_eq!(serialized["limits"]["max_request_header_size"], "65536 B");
        assert_eq!(serialized["limits"]["max_tokens"]["n"], 10_000);
        assert_eq!(serialized["limits"]["max_depth"]["n"], 20);
        assert_eq!(serialized["limits"]["max_aliases"]["n"], 50);
        assert_eq!(serialized["limits"]["max_directives"]["n"], 100);
        assert_eq!(serialized["log"]["format"], "json");
        assert_eq!(serialized["log"]["level"], "info");
        assert_eq!(
            serialized["traffic_shaping"]["all"]["request_timeout"],
            "30s"
        );
        assert_eq!(
            serialized["traffic_shaping"]["router"]["request_timeout"],
            "1m"
        );
        assert_eq!(
            serialized["traffic_shaping"]["max_connections_per_host"],
            100
        );
        assert!(
            serialized["telemetry"]["metrics"]["exporters"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn private_hive_telemetry_translation_enables_only_explicit_export() {
        let config = RouterConfig::new("127.0.0.1:4000".parse().unwrap())
            .allow_anonymous_development(true)
            .with_subgraph(StaticSubgraph::new(
                "status",
                "http://status.test/graphql",
                "http://status.test/sdl",
            ))
            .with_telemetry(
                crate::RouterTelemetryConfig::new()
                    .with_log_level(crate::RouterLogLevel::Warn)
                    .with_prometheus(4900, "/internal/metrics"),
            )
            .validate()
            .unwrap();
        let serialized =
            serde_json::to_value(build_hive_config(&config, 9, None).unwrap()).unwrap();
        assert_eq!(serialized["log"]["format"], "json");
        assert_eq!(serialized["log"]["level"], "warn");
        assert_eq!(
            serialized["telemetry"]["metrics"]["exporters"][0]["kind"],
            "prometheus"
        );
        assert_eq!(
            serialized["telemetry"]["metrics"]["exporters"][0]["port"],
            4900
        );
    }

    #[test]
    fn private_hive_subscription_transport_is_bounded_and_never_refreshes_credentials() {
        let config = RouterConfig::new("127.0.0.1:4000".parse().unwrap())
            .with_authentication_provider(Arc::new(RejectingProvider))
            .with_subscriptions(
                crate::SubscriptionConfig::new()
                    .with_max_connections(9)
                    .with_broadcast_capacity(7)
                    .with_subgraph_buffer_capacity(11),
            )
            .with_subgraph(
                StaticSubgraph::new(
                    "status",
                    "http://status.test/graphql",
                    "http://status.test/sdl",
                )
                .with_protocol_url("http://status.test/.well-known/graphql-router")
                .with_subscription_websocket_path("/events"),
            )
            .validate()
            .unwrap();
        let internal = InternalSubscriptionEndpoint::generate().unwrap();
        let hive = build_hive_config(&config, 7, Some(&internal)).unwrap();
        let serialized = serde_json::to_value(hive).unwrap();
        assert_eq!(serialized["websocket"]["enabled"], true);
        assert_eq!(serialized["websocket"]["headers"]["source"], "connection");
        assert_eq!(serialized["websocket"]["headers"]["persist"], false);
        assert_eq!(serialized["subscriptions"]["enabled"], true);
        assert_eq!(serialized["subscriptions"]["broadcast_capacity"], 7);
        assert_eq!(serialized["subscriptions"]["subgraph_buffer_capacity"], 11);
        assert_eq!(
            serialized["subscriptions"]["websocket"]["subgraphs"]["status"]["path"],
            "/events"
        );
        assert_eq!(
            serialized["traffic_shaping"]["router"]["max_long_lived_clients"],
            9
        );
        assert_eq!(
            serialized["traffic_shaping"]["router"]["dedupe"]["enabled"],
            false
        );
    }
}
