use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use graphql_orm_router_protocol::{SubgraphId, SubgraphName};
use reqwest::header::{HeaderName, HeaderValue};
use url::Url;

use crate::{
    AuthenticationProvider, ByteRoute, ExactScopeMatcher, NetworkPolicy, RouterError,
    RouterErrorKind, ScopeMatcher,
    node_lookup::{NODE_SUBGRAPH_NAME, NodeLookupConfig},
};

const DEFAULT_MAX_SDL_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_SCHEMA_FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_SCHEMA_POLL_INTERVAL: Duration = Duration::from_secs(30);
const DEFAULT_SCHEMA_REFRESH_RETRY_DELAY: Duration = Duration::from_millis(100);
const DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_PUBLIC_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_SUBGRAPH_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_WEBSOCKET_MESSAGE_BYTES: usize = 64 * 1024;
/// Safety ceiling for every subscription count. Values above it are refused
/// rather than reaching allocation or channel construction.
const MAX_SUBSCRIPTION_LIMIT: usize = 1 << 20;
const RESERVED_PUBLIC_HEADERS: &[&str] = &[
    "authorization",
    "cookie",
    "proxy-authorization",
    "set-cookie",
];
const HOP_BY_HOP_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "proxy-connection",
    "host",
    "content-length",
];

/// Static configuration for the public listener and its initial subgraphs.
///
/// Authentication is fail-closed by default. Callers install a resource-server
/// provider, or explicitly opt into anonymous mode for trusted development.
#[derive(Clone)]
pub struct RouterConfig {
    pub(crate) persisted_documents: Option<crate::persisted::PersistedDocuments>,
    pub(crate) listener: SocketAddr,
    pub(crate) graphql_path: String,
    pub(crate) anonymous_development_mode: bool,
    pub(crate) introspection: bool,
    pub(crate) forwarded_headers: BTreeSet<String>,
    pub(crate) subgraphs: Vec<StaticSubgraph>,
    pub(crate) schema_fetch_timeout: Duration,
    pub(crate) max_sdl_bytes: usize,
    pub(crate) schema_poll_interval: Duration,
    pub(crate) schema_refresh_attempts: usize,
    pub(crate) schema_refresh_retry_delay: Duration,
    pub(crate) authentication: Option<Arc<dyn AuthenticationProvider>>,
    pub(crate) scope_matcher: Arc<dyn ScopeMatcher>,
    pub(crate) subscriptions: Option<SubscriptionConfig>,
    pub(crate) request_limits: RequestLimits,
    pub(crate) admin: Option<AdminConfig>,
    pub(crate) telemetry: RouterTelemetryConfig,
    pub(crate) graceful_shutdown_timeout: Duration,
    pub(crate) public_request_timeout: Duration,
    pub(crate) subgraph_request_timeout: Duration,
    pub(crate) max_subgraph_connections_per_host: usize,
    pub(crate) node_lookup: Option<NodeLookupConfig>,
    pub(crate) byte_routes: Vec<ByteRoute>,
    pub(crate) allow_plaintext_http: bool,
    pub(crate) use_environment_proxy: bool,
}

/// Fluent engine-neutral router builder.
///
/// `RouterConfig` remains the built value for source compatibility; the alias
/// makes the intended programmatic builder role explicit without introducing
/// a second representation that could drift from validation.
pub type RouterBuilder = RouterConfig;

impl RouterConfig {
    /// Creates a fail-closed router configuration bound to `listener`.
    pub fn new(listener: SocketAddr) -> Self {
        Self {
            persisted_documents: None,
            listener,
            graphql_path: "/graphql".to_owned(),
            anonymous_development_mode: false,
            introspection: false,
            forwarded_headers: BTreeSet::new(),
            subgraphs: Vec::new(),
            node_lookup: None,
            schema_fetch_timeout: DEFAULT_SCHEMA_FETCH_TIMEOUT,
            max_sdl_bytes: DEFAULT_MAX_SDL_BYTES,
            schema_poll_interval: DEFAULT_SCHEMA_POLL_INTERVAL,
            schema_refresh_attempts: 2,
            schema_refresh_retry_delay: DEFAULT_SCHEMA_REFRESH_RETRY_DELAY,
            authentication: None,
            scope_matcher: Arc::new(ExactScopeMatcher),
            subscriptions: None,
            request_limits: RequestLimits::default(),
            admin: None,
            telemetry: RouterTelemetryConfig::default(),
            graceful_shutdown_timeout: DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT,
            public_request_timeout: DEFAULT_PUBLIC_REQUEST_TIMEOUT,
            subgraph_request_timeout: DEFAULT_SUBGRAPH_REQUEST_TIMEOUT,
            max_subgraph_connections_per_host: 100,
            byte_routes: Vec::new(),
            allow_plaintext_http: false,
            use_environment_proxy: false,
        }
    }

    /// Starts the fluent engine-neutral router builder.
    pub fn builder(listener: SocketAddr) -> RouterBuilder {
        Self::new(listener)
    }

    /// Sets the public GraphQL path.
    #[must_use]
    pub fn with_graphql_path(mut self, path: impl Into<String>) -> Self {
        self.graphql_path = path.into();
        self
    }

    /// Permits plaintext `http` subgraph, byte route, and trusted dynamic
    /// subgraph URLs whose host is not loopback.
    ///
    /// Off by default: plaintext `http` is accepted only for `localhost` and
    /// loopback IP literals, and every other outbound URL must use `https`.
    /// Enable this only when the network path to those hosts is otherwise
    /// protected, for example inside a private service mesh.
    #[must_use]
    pub fn allow_plaintext_http(mut self, allowed: bool) -> Self {
        self.allow_plaintext_http = allowed;
        self
    }

    /// Routes the schema, SDL, and router-protocol client through the proxy
    /// named by the standard proxy environment variables.
    ///
    /// Off by default: that client connects directly and ignores
    /// `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, and `NO_PROXY`. Subgraph
    /// execution, byte routes, and dynamic registration never use a proxy.
    /// The JWKS client has its own
    /// [`JwksAuthenticationConfig::use_environment_proxy`](crate::JwksAuthenticationConfig::use_environment_proxy).
    #[must_use]
    pub fn use_environment_proxy(mut self, enabled: bool) -> Self {
        self.use_environment_proxy = enabled;
        self
    }

    /// Explicitly permits unauthenticated requests for local development.
    #[must_use]
    pub fn allow_anonymous_development(mut self, allowed: bool) -> Self {
        self.anonymous_development_mode = allowed;
        self
    }

    /// Enables GraphQL introspection (`__schema` and `__type`) on the public
    /// endpoint. It is disabled by default.
    ///
    /// With an authentication provider installed, enabled introspection still
    /// requires a validated bearer credential; anonymous callers are refused.
    /// In anonymous development mode it is open to every caller. The
    /// configured depth limit applies to introspection either way. The root
    /// `__typename` meta field is never affected.
    #[must_use]
    pub fn with_introspection(mut self, enabled: bool) -> Self {
        self.introspection = enabled;
        self
    }

    /// Adds a statically configured subgraph.
    #[must_use]
    pub fn with_subgraph(mut self, subgraph: StaticSubgraph) -> Self {
        self.subgraphs.push(subgraph);
        self
    }

    /// Allows one non-sensitive incoming header to be copied to subgraphs.
    ///
    /// Bearer credentials and cookies have a separate authenticated forwarding
    /// path and cannot be enabled through this development allowlist.
    #[must_use]
    pub fn forward_header(mut self, name: impl Into<String>) -> Self {
        self.forwarded_headers.insert(name.into());
        self
    }

    /// Sets the timeout for each startup SDL request.
    #[must_use]
    pub fn with_schema_fetch_timeout(mut self, timeout: Duration) -> Self {
        self.schema_fetch_timeout = timeout;
        self
    }

    /// Sets the maximum accepted SDL response size.
    #[must_use]
    pub fn with_max_sdl_bytes(mut self, bytes: usize) -> Self {
        self.max_sdl_bytes = bytes;
        self
    }

    /// Sets the interval between automatic conditional schema refresh rounds.
    #[must_use]
    pub fn with_schema_poll_interval(mut self, interval: Duration) -> Self {
        self.schema_poll_interval = interval;
        self
    }

    /// Sets the bounded number of fetch attempts in one refresh round.
    #[must_use]
    pub fn with_schema_refresh_attempts(mut self, attempts: usize) -> Self {
        self.schema_refresh_attempts = attempts;
        self
    }

    /// Sets the delay between retryable schema refresh attempts.
    #[must_use]
    pub fn with_schema_refresh_retry_delay(mut self, delay: Duration) -> Self {
        self.schema_refresh_retry_delay = delay;
        self
    }

    /// Installs the resource-server authentication provider.
    #[must_use]
    pub fn with_authentication_provider(
        mut self,
        provider: Arc<dyn AuthenticationProvider>,
    ) -> Self {
        self.authentication = Some(provider);
        self
    }

    /// Installs the scope matcher shared with router preflight policy.
    ///
    /// Scope-template values may contain ASCII letters, digits, `-` and `_`.
    /// A matcher must not treat any of those characters as a separator or
    /// wildcard, or a substituted value could split or widen a rendered
    /// requirement. See [`ScopeMatcher`].
    #[must_use]
    pub fn with_scope_matcher(mut self, matcher: Arc<dyn ScopeMatcher>) -> Self {
        self.scope_matcher = matcher;
        self
    }

    /// Enables authenticated `graphql-transport-ws` subscriptions.
    #[must_use]
    pub fn with_subscriptions(mut self, subscriptions: SubscriptionConfig) -> Self {
        self.subscriptions = Some(subscriptions);
        self
    }

    /// Replaces bounded HTTP and GraphQL parser/operation limits.
    #[must_use]
    pub fn with_request_limits(mut self, limits: RequestLimits) -> Self {
        self.request_limits = limits;
        self
    }

    /// Enables a separately bound authenticated administrative service.
    #[must_use]
    pub fn with_admin(mut self, admin: AdminConfig) -> Self {
        self.admin = Some(admin);
        self
    }

    /// Replaces structured logging and optional private metrics-export policy.
    #[must_use]
    pub fn with_telemetry(mut self, telemetry: RouterTelemetryConfig) -> Self {
        self.telemetry = telemetry;
        self
    }

    /// Sets the bounded listener drain window used by graceful shutdown.
    #[must_use]
    pub fn with_graceful_shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.graceful_shutdown_timeout = timeout;
        self
    }

    /// Sets the total execution deadline for one public GraphQL request.
    #[must_use]
    pub fn with_public_request_timeout(mut self, timeout: Duration) -> Self {
        self.public_request_timeout = timeout;
        self
    }

    /// Sets the deadline for each HTTP request made to a subgraph.
    #[must_use]
    pub fn with_subgraph_request_timeout(mut self, timeout: Duration) -> Self {
        self.subgraph_request_timeout = timeout;
        self
    }

    /// Enables the router-hosted `node(NodeId)` and `nodes(NodeIds)` lookup.
    ///
    /// Composition adds a router-hosted subgraph that owns
    /// `interface Node @key(fields: "NodeId")` whenever a subgraph type
    /// declares `NodeId: ID!` with a resolvable `@key(fields: "NodeId")`, and
    /// the router answers that subgraph in-process. Without such a type the
    /// composed graph is unchanged.
    #[must_use]
    pub fn with_node_lookup(mut self, node_lookup: NodeLookupConfig) -> Self {
        self.node_lookup = Some(node_lookup);
        self
    }

    /// Adds one statically configured byte route on the public listener.
    ///
    /// Byte routes are off by default; only listed prefixes are served. See
    /// [`ByteRoute`] for the authentication, header, and limit rules.
    #[must_use]
    pub fn with_byte_route(mut self, route: ByteRoute) -> Self {
        self.byte_routes.push(route);
        self
    }

    /// Sets the connection-pool bound applied to each downstream host.
    #[must_use]
    pub fn with_max_subgraph_connections_per_host(mut self, maximum: usize) -> Self {
        self.max_subgraph_connections_per_host = maximum;
        self
    }

    /// Requires registered ids from a local JSON id-to-document manifest.
    ///
    /// Keys are lowercase SHA-256 hashes of exact UTF-8 document bytes. Startup
    /// validates the 16 MiB / 10,000-entry manifest and 256 KiB document bounds.
    /// A private snapshot is checked on every graph candidate; restart with a
    /// new configuration to adopt operator-file changes. Authorization remains
    /// mandatory according to the operation's existing policy.
    #[must_use]
    pub fn with_persisted_allow_list(mut self, path: impl AsRef<std::path::Path>) -> Self {
        self.persisted_documents = Some(crate::persisted::PersistedDocuments::new(path, true));
        self
    }

    /// Enables registered ids and also permits ordinary document text.
    ///
    /// Uses the same validated local manifest and bounds as
    /// [`Self::with_persisted_allow_list`]. When a request supplies both an id
    /// and text, its text runs. This mode does not enforce an allow-list.
    #[must_use]
    pub fn with_persisted_ids_or_text(mut self, path: impl AsRef<std::path::Path>) -> Self {
        self.persisted_documents = Some(crate::persisted::PersistedDocuments::new(path, false));
        self
    }

    pub(crate) fn validate(mut self) -> Result<Self, RouterError> {
        if let Some(documents) = &mut self.persisted_documents {
            documents.validate()?;
        }
        if self.authentication.is_none() && !self.anonymous_development_mode {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "authentication is not configured; explicitly enable anonymous development mode only for a trusted development deployment",
            ));
        }
        if self.authentication.is_some() && self.anonymous_development_mode {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "authentication and anonymous development mode are mutually exclusive",
            ));
        }
        if self.subscriptions.is_some() && self.authentication.is_none() {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "subscriptions require a resource-server authentication provider",
            ));
        }
        if let Some(subscriptions) = &self.subscriptions {
            subscriptions.validate()?;
            if self.listener.port() == 0 {
                return Err(RouterError::new(
                    RouterErrorKind::InvalidConfiguration,
                    "authenticated subscriptions require an explicit listener port",
                ));
            }
        }
        self.request_limits.validate()?;
        if let Some(admin) = &self.admin {
            let Some(public) = &self.authentication else {
                return Err(RouterError::new(
                    RouterErrorKind::InvalidConfiguration,
                    "administrative endpoints require a resource-server authentication provider",
                ));
            };
            validate_admin_authentication(public, admin.authentication.as_ref())?;
            admin.validate(self.listener, &self.subgraphs, self.allow_plaintext_http)?;
        }
        self.telemetry.validate(
            self.listener,
            self.admin.as_ref().map(|admin| admin.listener),
        )?;
        if self.graceful_shutdown_timeout.is_zero()
            || self.graceful_shutdown_timeout > Duration::from_secs(60)
            || self.graceful_shutdown_timeout.subsec_nanos() != 0
        {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "graceful shutdown timeout must be a whole number of seconds between 1 and 60",
            ));
        }
        for (name, timeout) in [
            ("public request timeout", self.public_request_timeout),
            ("subgraph request timeout", self.subgraph_request_timeout),
        ] {
            if timeout.is_zero() || timeout > Duration::from_secs(300) {
                return Err(RouterError::new(
                    RouterErrorKind::InvalidConfiguration,
                    format!("{name} must be greater than zero and at most 300 seconds"),
                ));
            }
        }
        if self.max_subgraph_connections_per_host == 0
            || self.max_subgraph_connections_per_host > 10_000
        {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "maximum subgraph connections per host must be between 1 and 10000",
            ));
        }
        validate_graphql_path(&self.graphql_path)?;
        crate::byte_route::validate_byte_routes(
            &mut self.byte_routes,
            &crate::byte_route::ReservedPaths {
                graphql_path: &self.graphql_path,
                authentication_configured: self.authentication.is_some(),
            },
            self.allow_plaintext_http,
        )?;
        if let Some(node_lookup) = &self.node_lookup
            && !(1..=10_000).contains(&node_lookup.max_nodes)
        {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "node lookup maximum NodeIds must be between 1 and 10000",
            ));
        }
        if self
            .subgraphs
            .iter()
            .any(|subgraph| subgraph.name == NODE_SUBGRAPH_NAME)
        {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                format!(
                    "subgraph name `{NODE_SUBGRAPH_NAME}` is reserved for the router-hosted node lookup"
                ),
            ));
        }
        if self.subgraphs.is_empty() {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "at least one static subgraph is required",
            ));
        }
        if self.schema_fetch_timeout.is_zero() {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "schema fetch timeout must be greater than zero",
            ));
        }
        if self.max_sdl_bytes == 0 {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "maximum SDL size must be greater than zero",
            ));
        }
        if self.schema_poll_interval.is_zero() {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "schema poll interval must be greater than zero",
            ));
        }
        if self.schema_refresh_attempts == 0 || self.schema_refresh_attempts > 10 {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "schema refresh attempts must be between 1 and 10",
            ));
        }
        if self.schema_refresh_retry_delay > self.schema_poll_interval {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "schema refresh retry delay must not exceed the poll interval",
            ));
        }

        let mut names = BTreeSet::new();
        for subgraph in &self.subgraphs {
            SubgraphName::try_from(subgraph.name.clone()).map_err(|details| {
                RouterError::new(
                    RouterErrorKind::InvalidConfiguration,
                    format!("invalid subgraph name: {details}"),
                )
            })?;
            if !names.insert(subgraph.name.clone()) {
                return Err(RouterError::new(
                    RouterErrorKind::InvalidConfiguration,
                    format!("duplicate static subgraph name `{}`", subgraph.name),
                ));
            }
            let plaintext = self.allow_plaintext_http;
            validate_outbound_url(&subgraph.graphql_url, "GraphQL endpoint", plaintext)?;
            validate_outbound_url(&subgraph.sdl_url, "SDL endpoint", plaintext)?;
            if self.authentication.is_some() {
                let Some(protocol_url) = &subgraph.protocol_url else {
                    return Err(RouterError::new(
                        RouterErrorKind::InvalidConfiguration,
                        format!(
                            "authenticated subgraph `{}` requires a router protocol endpoint",
                            subgraph.name
                        ),
                    ));
                };
                validate_outbound_url(protocol_url, "router protocol endpoint", plaintext)?;
            } else if let Some(protocol_url) = &subgraph.protocol_url {
                validate_outbound_url(protocol_url, "router protocol endpoint", plaintext)?;
            }
            for (name, value) in &subgraph.schema_headers {
                validate_header(name, value)?;
                if HOP_BY_HOP_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
                    return Err(RouterError::new(
                        RouterErrorKind::InvalidConfiguration,
                        format!(
                            "schema credential header `{name}` is hop-by-hop or transport-owned"
                        ),
                    ));
                }
            }
            if let Some(path) = &subgraph.subscription_websocket_path {
                validate_subscription_path(path)?;
            }
        }

        let mut normalized = BTreeSet::new();
        for name in self.forwarded_headers {
            let parsed = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                RouterError::new(
                    RouterErrorKind::InvalidConfiguration,
                    format!("invalid downstream header name `{name}`"),
                )
            })?;
            let name = parsed.as_str().to_owned();
            if HOP_BY_HOP_HEADERS.contains(&name.as_str())
                || RESERVED_PUBLIC_HEADERS.contains(&name.as_str())
            {
                return Err(RouterError::new(
                    RouterErrorKind::InvalidConfiguration,
                    format!("downstream header `{name}` is reserved or security-sensitive"),
                ));
            }
            normalized.insert(name);
        }
        self.forwarded_headers = normalized;
        Ok(self)
    }
}

impl fmt::Debug for RouterConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RouterConfig")
            .field("listener", &self.listener)
            .field("graphql_path", &self.graphql_path)
            .field(
                "anonymous_development_mode",
                &self.anonymous_development_mode,
            )
            .field("forwarded_headers", &self.forwarded_headers)
            .field("subgraphs", &self.subgraphs)
            .field("schema_fetch_timeout", &self.schema_fetch_timeout)
            .field("max_sdl_bytes", &self.max_sdl_bytes)
            .field("schema_poll_interval", &self.schema_poll_interval)
            .field("schema_refresh_attempts", &self.schema_refresh_attempts)
            .field(
                "schema_refresh_retry_delay",
                &self.schema_refresh_retry_delay,
            )
            .field("authentication_configured", &self.authentication.is_some())
            .field("scope_matcher", &self.scope_matcher)
            .field("subscriptions", &self.subscriptions)
            .field("request_limits", &self.request_limits)
            .field("admin", &self.admin)
            .field("telemetry", &self.telemetry)
            .field("graceful_shutdown_timeout", &self.graceful_shutdown_timeout)
            .field("public_request_timeout", &self.public_request_timeout)
            .field("subgraph_request_timeout", &self.subgraph_request_timeout)
            .field(
                "max_subgraph_connections_per_host",
                &self.max_subgraph_connections_per_host,
            )
            .field("byte_routes", &self.byte_routes)
            .field("allow_plaintext_http", &self.allow_plaintext_http)
            .field("use_environment_proxy", &self.use_environment_proxy)
            .finish()
    }
}

/// One statically configured Federation subgraph.
#[derive(Clone)]
pub struct StaticSubgraph {
    pub(crate) name: String,
    pub(crate) graphql_url: String,
    pub(crate) sdl_url: String,
    pub(crate) protocol_url: Option<String>,
    pub(crate) schema_headers: BTreeMap<String, String>,
    pub(crate) subscription_websocket_path: Option<String>,
}

impl StaticSubgraph {
    /// Creates a subgraph whose SDL is fetched from `sdl_url` at startup.
    pub fn new(
        name: impl Into<String>,
        graphql_url: impl Into<String>,
        sdl_url: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            graphql_url: graphql_url.into(),
            sdl_url: sdl_url.into(),
            protocol_url: None,
            schema_headers: BTreeMap::new(),
            subscription_websocket_path: None,
        }
    }

    /// Configures the project-neutral router protocol descriptor endpoint.
    #[must_use]
    pub fn with_protocol_url(mut self, url: impl Into<String>) -> Self {
        self.protocol_url = Some(url.into());
        self
    }

    /// Adds a credential or other private header used only to retrieve SDL.
    #[must_use]
    pub fn with_schema_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.schema_headers.insert(name.into(), value.into());
        self
    }

    /// Overrides the owning subgraph's WebSocket subscription path.
    ///
    /// The host and port continue to come from the deployment-owned GraphQL
    /// endpoint; advertised protocol endpoints remain inert metadata.
    #[must_use]
    pub fn with_subscription_websocket_path(mut self, path: impl Into<String>) -> Self {
        self.subscription_websocket_path = Some(path.into());
        self
    }
}

impl fmt::Debug for StaticSubgraph {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StaticSubgraph")
            .field("name", &self.name)
            .field("graphql_url", &redacted_url(&self.graphql_url))
            .field("sdl_url", &redacted_url(&self.sdl_url))
            .field(
                "protocol_url",
                &self.protocol_url.as_deref().map(redacted_url),
            )
            .field(
                "schema_header_names",
                &self.schema_headers.keys().collect::<Vec<_>>(),
            )
            .field(
                "subscription_websocket_path",
                &self.subscription_websocket_path,
            )
            .finish()
    }
}

/// Bounded public and upstream subscription transport settings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubscriptionConfig {
    pub(crate) max_connections: usize,
    pub(crate) max_pending_connections: Option<usize>,
    pub(crate) max_connection_attempts_per_second: usize,
    pub(crate) max_operations_per_connection: usize,
    pub(crate) broadcast_capacity: usize,
    pub(crate) subgraph_buffer_capacity: usize,
    pub(crate) max_client_message_bytes: usize,
    pub(crate) connection_init_timeout: Duration,
}

impl SubscriptionConfig {
    /// Creates secure bounded defaults for the authenticated WebSocket path.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the process-wide limit on authenticated public WebSocket
    /// connections.
    ///
    /// A connection is charged against this limit only after its
    /// `connection_init` bearer credential validates, so unauthenticated
    /// sockets can never occupy it. A connection that authenticates while the
    /// limit is reached closes with code `1013`. When the limit is already
    /// reached, new upgrades are refused with `503` before any budget is used.
    #[must_use]
    pub fn with_max_connections(mut self, value: usize) -> Self {
        self.max_connections = value;
        self
    }

    /// Sets the process-wide limit on public WebSocket connections that have
    /// not yet authenticated.
    ///
    /// Every upgrade holds one of these slots until its `connection_init`
    /// credential validates, the socket closes, or the connection-init timeout
    /// expires. It is a separate budget from
    /// [`with_max_connections`](Self::with_max_connections), so sockets that
    /// never authenticate cannot take authenticated capacity. Upgrades beyond
    /// the limit are refused with `503`.
    ///
    /// When unset, the limit is the number of upgrades the attempt rate admits
    /// within one connection-init timeout, rounded up and capped at
    /// `max_connections`:
    /// `min(ceil(max_connection_attempts_per_second × timeout), max_connections)`.
    /// The attempt rate is then the first limit that idle upgrades reach,
    /// unless `max_connections` is the smaller value. An explicit value below
    /// that product lets fewer idle upgrades than the rate admits fill the
    /// budget.
    #[must_use]
    pub fn with_max_pending_connections(mut self, value: usize) -> Self {
        self.max_pending_connections = Some(value);
        self
    }

    /// The effective pending-connection limit: the configured value, or the
    /// default derived from the attempt rate, init timeout and connection
    /// limit.
    pub(crate) fn pending_connection_limit(&self) -> usize {
        self.max_pending_connections.unwrap_or_else(|| {
            let admitted = (self.max_connection_attempts_per_second as u128)
                .saturating_mul(self.connection_init_timeout.as_millis())
                .div_ceil(1_000);
            usize::try_from(admitted)
                .unwrap_or(usize::MAX)
                .clamp(1, self.max_connections.max(1))
        })
    }

    /// Sets the process-wide public WebSocket upgrade-attempt rate.
    ///
    /// The limiter uses a token bucket with one second of burst capacity. It
    /// contains reconnect storms before they can repeatedly create private
    /// subscription transports. It is charged before any credential is
    /// presented, so it is a shared upgrade-rate limit rather than a per-caller
    /// quota; apply per-client connection limits at the proxy or load balancer
    /// that sees client addresses. Unless
    /// [`with_max_pending_connections`](Self::with_max_pending_connections) is
    /// set, this rate also sizes the pending-connection budget.
    #[must_use]
    pub fn with_max_connection_attempts_per_second(mut self, value: usize) -> Self {
        self.max_connection_attempts_per_second = value;
        self
    }

    /// Sets the concurrent operation limit for each connection.
    #[must_use]
    pub fn with_max_operations_per_connection(mut self, value: usize) -> Self {
        self.max_operations_per_connection = value;
        self
    }

    /// Sets the bounded downstream fan-out capacity per subscription.
    #[must_use]
    pub fn with_broadcast_capacity(mut self, value: usize) -> Self {
        self.broadcast_capacity = value;
        self
    }

    /// Sets the bounded subgraph-to-router event buffer capacity.
    #[must_use]
    pub fn with_subgraph_buffer_capacity(mut self, value: usize) -> Self {
        self.subgraph_buffer_capacity = value;
        self
    }

    /// Sets the maximum accepted client WebSocket message size.
    ///
    /// Ntex applies a hard 64 KiB frame bound; smaller configured limits are
    /// additionally enforced by the router protocol gateway.
    #[must_use]
    pub fn with_max_client_message_bytes(mut self, value: usize) -> Self {
        self.max_client_message_bytes = value;
        self
    }

    /// Sets how long a client may wait before sending `connection_init`.
    ///
    /// Defaults to 3 seconds and must be between zero and 60 seconds. Unless
    /// [`with_max_pending_connections`](Self::with_max_pending_connections) is
    /// set, it also sizes the pending-connection budget.
    #[must_use]
    pub fn with_connection_init_timeout(mut self, value: Duration) -> Self {
        self.connection_init_timeout = value;
        self
    }

    fn validate(&self) -> Result<(), RouterError> {
        let bounded = [
            ("maximum WebSocket connections", self.max_connections),
            (
                "maximum pending WebSocket connections",
                self.max_pending_connections.unwrap_or(1),
            ),
            (
                "maximum WebSocket connection attempts per second",
                self.max_connection_attempts_per_second,
            ),
            (
                "maximum operations per WebSocket connection",
                self.max_operations_per_connection,
            ),
            ("subscription broadcast capacity", self.broadcast_capacity),
            (
                "subscription subgraph buffer capacity",
                self.subgraph_buffer_capacity,
            ),
        ];
        if let Some((name, _)) = bounded.iter().find(|(_, value)| *value == 0) {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                format!("{name} must be greater than zero"),
            ));
        }
        if let Some((name, _)) = bounded
            .iter()
            .find(|(_, value)| *value > MAX_SUBSCRIPTION_LIMIT)
        {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                format!("{name} must not exceed {MAX_SUBSCRIPTION_LIMIT}"),
            ));
        }
        if self.max_client_message_bytes == 0
            || self.max_client_message_bytes > MAX_WEBSOCKET_MESSAGE_BYTES
        {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "maximum WebSocket message size must be between 1 and 65536 bytes",
            ));
        }
        if self.connection_init_timeout.is_zero()
            || self.connection_init_timeout > Duration::from_secs(60)
        {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "WebSocket connection-init timeout must be between zero and 60 seconds",
            ));
        }
        Ok(())
    }
}

impl Default for SubscriptionConfig {
    fn default() -> Self {
        Self {
            max_connections: 1_024,
            max_pending_connections: None,
            max_connection_attempts_per_second: 128,
            max_operations_per_connection: 32,
            broadcast_capacity: 32,
            subgraph_buffer_capacity: 1_024,
            max_client_message_bytes: MAX_WEBSOCKET_MESSAGE_BYTES,
            connection_init_timeout: Duration::from_secs(3),
        }
    }
}

/// Bounded public HTTP and GraphQL parsing/operation limits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestLimits {
    pub(crate) max_request_body_bytes: usize,
    pub(crate) max_request_header_bytes: usize,
    pub(crate) max_parser_tokens: usize,
    pub(crate) max_depth: usize,
    pub(crate) max_aliases: usize,
    pub(crate) max_directives: usize,
    pub(crate) max_fields: usize,
}

impl RequestLimits {
    /// Creates production-bounded defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the maximum GraphQL HTTP request body size.
    #[must_use]
    pub fn with_max_request_body_bytes(mut self, value: usize) -> Self {
        self.max_request_body_bytes = value;
        self
    }

    /// Sets the maximum aggregate HTTP request-header size.
    #[must_use]
    pub fn with_max_request_header_bytes(mut self, value: usize) -> Self {
        self.max_request_header_bytes = value;
        self
    }

    /// Sets the maximum token count accepted by the GraphQL parser.
    #[must_use]
    pub fn with_max_parser_tokens(mut self, value: usize) -> Self {
        self.max_parser_tokens = value;
        self
    }

    /// Sets the maximum flattened GraphQL selection depth.
    #[must_use]
    pub fn with_max_depth(mut self, value: usize) -> Self {
        self.max_depth = value;
        self
    }

    /// Sets the maximum aliases in one operation.
    #[must_use]
    pub fn with_max_aliases(mut self, value: usize) -> Self {
        self.max_aliases = value;
        self
    }

    /// Sets the maximum directives in one operation.
    #[must_use]
    pub fn with_max_directives(mut self, value: usize) -> Self {
        self.max_directives = value;
        self
    }

    /// Sets the maximum normalized selection cost (field/fragment nodes).
    #[must_use]
    pub fn with_max_fields(mut self, value: usize) -> Self {
        self.max_fields = value;
        self
    }

    fn validate(&self) -> Result<(), RouterError> {
        let values = [
            ("maximum request body bytes", self.max_request_body_bytes),
            (
                "maximum request header bytes",
                self.max_request_header_bytes,
            ),
            ("maximum parser tokens", self.max_parser_tokens),
            ("maximum GraphQL depth", self.max_depth),
            ("maximum GraphQL aliases", self.max_aliases),
            ("maximum GraphQL directives", self.max_directives),
            ("maximum GraphQL fields", self.max_fields),
        ];
        if let Some((name, _)) = values.iter().find(|(_, value)| *value == 0) {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                format!("{name} must be greater than zero"),
            ));
        }
        if self.max_request_body_bytes > 16 * 1024 * 1024
            || self.max_request_header_bytes > 1024 * 1024
            || self.max_parser_tokens > 1_000_000
            || self.max_depth > 1_000
            || self.max_aliases > 100_000
            || self.max_directives > 100_000
            || self.max_fields > 1_000_000
        {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "request limit exceeds the router's supported safety bound",
            ));
        }
        Ok(())
    }
}

impl Default for RequestLimits {
    fn default() -> Self {
        Self {
            max_request_body_bytes: 1024 * 1024,
            max_request_header_bytes: 64 * 1024,
            max_parser_tokens: 10_000,
            max_depth: 20,
            max_aliases: 50,
            max_directives: 100,
            max_fields: 500,
        }
    }
}

/// Log severity for router-owned structured execution events.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RouterLogLevel {
    /// Diagnostic events intended for controlled troubleshooting.
    Debug,
    /// Normal lifecycle and request summaries.
    #[default]
    Info,
    /// Recoverable or degraded behavior.
    Warn,
    /// Failures requiring operator attention.
    Error,
}

impl RouterLogLevel {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// Engine-neutral structured logging and metrics-export configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouterTelemetryConfig {
    pub(crate) log_level: RouterLogLevel,
    pub(crate) json_logs: bool,
    pub(crate) prometheus_port: Option<u16>,
    pub(crate) prometheus_host: Option<IpAddr>,
    pub(crate) prometheus_path: String,
}

impl RouterTelemetryConfig {
    /// Creates production-oriented JSON logging with Prometheus disabled until
    /// an operator assigns a network-controlled listener port.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the minimum structured log severity.
    #[must_use]
    pub fn with_log_level(mut self, level: RouterLogLevel) -> Self {
        self.log_level = level;
        self
    }

    /// Uses compact text logs instead of JSON for local development.
    #[must_use]
    pub fn with_text_logs_for_development(mut self, enabled: bool) -> Self {
        self.json_logs = !enabled;
        self
    }

    /// Enables Hive execution and subscription metrics on a separate port.
    ///
    /// The scrape listener binds the public listener's IP address on `port`.
    /// Network access control for this scrape listener is deployment-owned.
    /// Use [`with_prometheus_listener`](Self::with_prometheus_listener) to
    /// bind it to a different address, such as loopback or a private
    /// interface.
    #[must_use]
    pub fn with_prometheus(mut self, port: u16, path: impl Into<String>) -> Self {
        self.prometheus_port = Some(port);
        self.prometheus_host = None;
        self.prometheus_path = path.into();
        self
    }

    /// Enables Hive execution and subscription metrics on an explicit socket
    /// address, independent of the public listener's IP address.
    ///
    /// The port follows the same rules as
    /// [`with_prometheus`](Self::with_prometheus): nonzero and distinct from
    /// the public and administrative listener ports, whatever their IP
    /// addresses. The scrape listener has no application authentication. An
    /// unspecified address (`0.0.0.0` or `::`) binds every interface and logs a
    /// warning at startup.
    #[must_use]
    pub fn with_prometheus_listener(
        mut self,
        address: SocketAddr,
        path: impl Into<String>,
    ) -> Self {
        self.prometheus_port = Some(address.port());
        self.prometheus_host = Some(address.ip());
        self.prometheus_path = path.into();
        self
    }

    /// Returns the address the optional scrape listener binds for a router
    /// whose public listener is `public_listener`.
    pub(crate) fn prometheus_listener(&self, public_listener: SocketAddr) -> Option<SocketAddr> {
        self.prometheus_port
            .map(|port| SocketAddr::new(self.prometheus_host.unwrap_or(public_listener.ip()), port))
    }

    fn validate(
        &self,
        public_listener: SocketAddr,
        admin_listener: Option<SocketAddr>,
    ) -> Result<(), RouterError> {
        if !self.prometheus_path.starts_with('/')
            || self.prometheus_path == "/"
            || self.prometheus_path.contains('?')
            || self.prometheus_path.contains('#')
        {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "Prometheus path must be an absolute non-root path without a query or fragment",
            ));
        }
        if let Some(port) = self.prometheus_port
            && (port == 0
                || port == public_listener.port()
                || admin_listener.is_some_and(|listener| listener.port() == port))
        {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "Prometheus must use a nonzero port distinct from public and administrative listeners",
            ));
        }
        Ok(())
    }
}

impl Default for RouterTelemetryConfig {
    fn default() -> Self {
        Self {
            log_level: RouterLogLevel::Info,
            json_logs: true,
            prometheus_port: None,
            prometheus_host: None,
            prometheus_path: "/metrics".to_owned(),
        }
    }
}

/// Configuration for the separately bound administrative service.
#[derive(Clone)]
pub struct AdminConfig {
    pub(crate) listener: SocketAddr,
    pub(crate) authentication: Option<Arc<dyn AuthenticationProvider>>,
    pub(crate) network_policy: NetworkPolicy,
    pub(crate) trusted_subgraphs: Vec<TrustedSubgraph>,
    pub(crate) status_scope: String,
    pub(crate) refresh_scope: String,
    pub(crate) registration_scope: String,
    pub(crate) removal_scope: String,
    pub(crate) metrics_scope: String,
    pub(crate) max_request_body_bytes: usize,
}

impl AdminConfig {
    /// Creates a deny-by-default administrative listener configuration.
    pub fn new(listener: SocketAddr, network_policy: NetworkPolicy) -> Self {
        Self {
            listener,
            authentication: None,
            network_policy,
            trusted_subgraphs: Vec::new(),
            status_scope: "router.status".to_owned(),
            refresh_scope: "router.refresh".to_owned(),
            registration_scope: "router.register".to_owned(),
            removal_scope: "router.remove".to_owned(),
            metrics_scope: "router.metrics".to_owned(),
            max_request_body_bytes: 16 * 1024,
        }
    }

    /// Installs the resource-server provider that authenticates every
    /// administrative request. Required.
    ///
    /// It must accept a token audience that public GraphQL bearers do not
    /// carry, so a token issued for the public API cannot administer the
    /// router even when it holds an administrative scope. Validation refuses
    /// the public provider itself, and refuses overlapping audiences when
    /// both providers state them through
    /// [`AuthenticationProvider::accepted_audiences`]. Providers that cannot
    /// state them, including the `auth-agql` adapter, are not compared, so
    /// their validators must be configured with distinct audiences. A token
    /// whose `aud` lists both audiences is accepted by both providers, so
    /// issuers must never mint one.
    #[must_use]
    pub fn with_authentication_provider(
        mut self,
        provider: Arc<dyn AuthenticationProvider>,
    ) -> Self {
        self.authentication = Some(provider);
        self
    }

    /// Adds one identity- and destination-bound dynamic subgraph trust record.
    #[must_use]
    pub fn trust_subgraph(mut self, subgraph: TrustedSubgraph) -> Self {
        self.trusted_subgraphs.push(subgraph);
        self
    }

    /// Replaces the exact scope required to read status.
    #[must_use]
    pub fn with_status_scope(mut self, scope: impl Into<String>) -> Self {
        self.status_scope = scope.into();
        self
    }

    /// Replaces the exact scope required to trigger refresh.
    #[must_use]
    pub fn with_refresh_scope(mut self, scope: impl Into<String>) -> Self {
        self.refresh_scope = scope.into();
        self
    }

    /// Replaces the exact scope required to register a candidate.
    #[must_use]
    pub fn with_registration_scope(mut self, scope: impl Into<String>) -> Self {
        self.registration_scope = scope.into();
        self
    }

    /// Replaces the exact scope required to explicitly remove a subgraph.
    #[must_use]
    pub fn with_removal_scope(mut self, scope: impl Into<String>) -> Self {
        self.removal_scope = scope.into();
        self
    }

    /// Replaces the exact scope required to read process-local metrics.
    #[must_use]
    pub fn with_metrics_scope(mut self, scope: impl Into<String>) -> Self {
        self.metrics_scope = scope.into();
        self
    }

    /// Sets the bounded administrative JSON body size.
    #[must_use]
    pub fn with_max_request_body_bytes(mut self, bytes: usize) -> Self {
        self.max_request_body_bytes = bytes;
        self
    }

    fn validate(
        &self,
        public_listener: SocketAddr,
        static_subgraphs: &[StaticSubgraph],
        allow_plaintext_http: bool,
    ) -> Result<(), RouterError> {
        // Compare ports, not whole addresses: an unspecified or second address
        // on the public port would share or shadow the public listener.
        if self.listener.port() == 0 || self.listener.port() == public_listener.port() {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "the administrative listener needs a nonzero port distinct from the public listener",
            ));
        }
        self.network_policy.validate()?;
        if self.max_request_body_bytes == 0 || self.max_request_body_bytes > 1024 * 1024 {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "administrative request body limit must be between 1 byte and 1 MiB",
            ));
        }
        for scope in [
            &self.status_scope,
            &self.refresh_scope,
            &self.registration_scope,
            &self.removal_scope,
            &self.metrics_scope,
        ] {
            if scope.is_empty()
                || scope
                    .bytes()
                    .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
            {
                return Err(RouterError::new(
                    RouterErrorKind::InvalidConfiguration,
                    "administrative scopes must be non-empty single tokens",
                ));
            }
        }
        let static_names = static_subgraphs
            .iter()
            .map(|subgraph| subgraph.name.as_str())
            .collect::<BTreeSet<_>>();
        let mut subjects = BTreeSet::new();
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        for trusted in &self.trusted_subgraphs {
            trusted.validate(allow_plaintext_http)?;
            if !subjects.insert(trusted.service_subject.as_str())
                || !ids.insert(trusted.subgraph_id.as_str())
                || !names.insert(trusted.name.as_str())
                || static_names.contains(trusted.name.as_str())
            {
                return Err(RouterError::new(
                    RouterErrorKind::InvalidConfiguration,
                    "trusted dynamic subgraph identities must be unique and distinct from static subgraphs",
                ));
            }
        }
        Ok(())
    }
}

impl fmt::Debug for AdminConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminConfig")
            .field("listener", &self.listener)
            .field("authentication_configured", &self.authentication.is_some())
            .field("network_policy", &self.network_policy)
            .field("trusted_subgraphs", &self.trusted_subgraphs)
            .field("status_scope", &self.status_scope)
            .field("refresh_scope", &self.refresh_scope)
            .field("registration_scope", &self.registration_scope)
            .field("removal_scope", &self.removal_scope)
            .field("metrics_scope", &self.metrics_scope)
            .field("max_request_body_bytes", &self.max_request_body_bytes)
            .finish()
    }
}

/// Preconfigured identity and destination binding for one dynamic service.
#[derive(Clone)]
pub struct TrustedSubgraph {
    pub(crate) service_subject: String,
    pub(crate) subgraph_id: String,
    pub(crate) name: String,
    pub(crate) metadata_url: String,
    pub(crate) graphql_origin: String,
    pub(crate) schema_origin: String,
    pub(crate) schema_headers: BTreeMap<String, String>,
}

impl TrustedSubgraph {
    /// Creates an exact service-subject, subgraph identity, registration URL,
    /// and advertised-origin binding.
    pub fn new(
        service_subject: impl Into<String>,
        subgraph_id: impl Into<String>,
        name: impl Into<String>,
        metadata_url: impl Into<String>,
        graphql_origin: impl Into<String>,
        schema_origin: impl Into<String>,
    ) -> Self {
        Self {
            service_subject: service_subject.into(),
            subgraph_id: subgraph_id.into(),
            name: name.into(),
            metadata_url: metadata_url.into(),
            graphql_origin: graphql_origin.into(),
            schema_origin: schema_origin.into(),
            schema_headers: BTreeMap::new(),
        }
    }

    /// Adds a router-owned credential/header used only for this service's
    /// descriptor and SDL retrieval.
    #[must_use]
    pub fn with_schema_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.schema_headers.insert(name.into(), value.into());
        self
    }

    fn validate(&self, allow_plaintext_http: bool) -> Result<(), RouterError> {
        if self.service_subject.trim().is_empty() {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "trusted service subject must not be empty",
            ));
        }
        SubgraphId::try_from(self.subgraph_id.clone())
            .map_err(|detail| RouterError::new(RouterErrorKind::InvalidConfiguration, detail))?;
        SubgraphName::try_from(self.name.clone())
            .map_err(|detail| RouterError::new(RouterErrorKind::InvalidConfiguration, detail))?;
        validate_outbound_url(
            &self.metadata_url,
            "trusted metadata endpoint",
            allow_plaintext_http,
        )?;
        for (origin, field) in [
            (&self.graphql_origin, "trusted GraphQL origin"),
            (&self.schema_origin, "trusted schema origin"),
        ] {
            require_secure_transport(
                &validate_origin(origin, field)?,
                allow_plaintext_http,
                field,
            )?;
        }
        for (name, value) in &self.schema_headers {
            validate_header(name, value)?;
            if HOP_BY_HOP_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
                return Err(RouterError::new(
                    RouterErrorKind::InvalidConfiguration,
                    format!("trusted service header `{name}` is transport-owned"),
                ));
            }
        }
        Ok(())
    }
}

impl fmt::Debug for TrustedSubgraph {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TrustedSubgraph")
            .field("service_subject", &self.service_subject)
            .field("subgraph_id", &self.subgraph_id)
            .field("name", &self.name)
            .field("metadata_url", &redacted_url(&self.metadata_url))
            .field("graphql_origin", &redacted_url(&self.graphql_origin))
            .field("schema_origin", &redacted_url(&self.schema_origin))
            .field(
                "schema_header_names",
                &self.schema_headers.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

fn validate_graphql_path(path: &str) -> Result<(), RouterError> {
    if !path.starts_with('/')
        || path.len() < 2
        || path.ends_with('/')
        || path.contains('?')
        || path.contains('#')
        || path == "/health"
        || path == "/readiness"
    {
        return Err(RouterError::new(
            RouterErrorKind::InvalidConfiguration,
            "GraphQL path must be an absolute non-root path without a trailing slash and must not collide with /health or /readiness",
        ));
    }
    Ok(())
}

fn validate_subscription_path(path: &str) -> Result<(), RouterError> {
    if !path.starts_with('/') || path.contains('?') || path.contains('#') {
        return Err(RouterError::new(
            RouterErrorKind::InvalidConfiguration,
            "subscription WebSocket path must be an absolute path without a query or fragment",
        ));
    }
    Ok(())
}

fn validate_http_url(value: &str, field: &str) -> Result<Url, RouterError> {
    let url = Url::parse(value).map_err(|_| {
        RouterError::new(
            RouterErrorKind::InvalidConfiguration,
            format!("{field} is not a valid absolute URL"),
        )
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(RouterError::new(
            RouterErrorKind::InvalidConfiguration,
            format!(
                "{field} must be an http(s) URL without embedded credentials, a query, or a fragment"
            ),
        ));
    }
    Ok(url)
}

/// Public bearer tokens must never authenticate administrative requests.
fn validate_admin_authentication(
    public: &Arc<dyn AuthenticationProvider>,
    admin: Option<&Arc<dyn AuthenticationProvider>>,
) -> Result<(), RouterError> {
    let Some(admin) = admin else {
        return Err(RouterError::new(
            RouterErrorKind::InvalidConfiguration,
            "administrative endpoints require their own authentication provider, accepting an audience that public bearer tokens do not carry",
        ));
    };
    let same_provider = std::ptr::addr_eq(Arc::as_ptr(public), Arc::as_ptr(admin));
    let shared_audience = match (public.accepted_audiences(), admin.accepted_audiences()) {
        (Some(public), Some(admin)) => admin.iter().any(|audience| public.contains(audience)),
        _ => false,
    };
    if same_provider || shared_audience {
        return Err(RouterError::new(
            RouterErrorKind::InvalidConfiguration,
            "the administrative authentication provider must not accept a public token audience",
        ));
    }
    Ok(())
}

fn validate_outbound_url(
    value: &str,
    field: &str,
    allow_plaintext_http: bool,
) -> Result<Url, RouterError> {
    let url = validate_http_url(value, field)?;
    require_secure_transport(&url, allow_plaintext_http, field)?;
    Ok(url)
}

/// Refuses plaintext `http` to a host that is not loopback unless the
/// operator has explicitly allowed it.
pub(crate) fn require_secure_transport(
    url: &Url,
    allow_plaintext_http: bool,
    field: &str,
) -> Result<(), RouterError> {
    let loopback = match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    };
    if url.scheme() == "http" && !loopback && !allow_plaintext_http {
        return Err(RouterError::new(
            RouterErrorKind::InvalidConfiguration,
            format!(
                "{field} uses plaintext HTTP to a host that is not loopback; use HTTPS, or explicitly allow plaintext HTTP"
            ),
        ));
    }
    Ok(())
}

fn validate_origin(value: &str, field: &str) -> Result<Url, RouterError> {
    let url = validate_http_url(value, field)?;
    if !matches!(url.path(), "" | "/") {
        return Err(RouterError::new(
            RouterErrorKind::InvalidConfiguration,
            format!("{field} must contain only a scheme, host, and optional port"),
        ));
    }
    Ok(url)
}

fn redacted_url(value: &str) -> String {
    let Ok(mut url) = Url::parse(value) else {
        return "[invalid URL]".to_owned();
    };
    // Redaction failure must never fall back to the original credentials.
    if !url.username().is_empty() && url.set_username("[redacted]").is_err() {
        return "[redacted URL]".to_owned();
    }
    if url.password().is_some() && url.set_password(Some("[redacted]")).is_err() {
        return "[redacted URL]".to_owned();
    }
    if url.query().is_some() {
        url.set_query(Some("[redacted]"));
    }
    url.to_string()
}

fn validate_header(name: &str, value: &str) -> Result<(), RouterError> {
    HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
        RouterError::new(
            RouterErrorKind::InvalidConfiguration,
            format!("invalid schema credential header name `{name}`"),
        )
    })?;
    HeaderValue::from_str(value).map_err(|_| {
        RouterError::new(
            RouterErrorKind::InvalidConfiguration,
            format!("schema credential header `{name}` has an invalid value"),
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct RejectingProvider;

    impl AuthenticationProvider for RejectingProvider {
        fn authenticate_bearer(
            &self,
            _token: &str,
        ) -> Result<crate::AuthenticatedPrincipal, crate::AuthenticationError> {
            Err(crate::AuthenticationError::invalid_credential("rejected"))
        }
    }

    fn base() -> RouterConfig {
        RouterConfig::new("127.0.0.1:4000".parse().unwrap())
            .allow_plaintext_http(true)
            .allow_anonymous_development(true)
            .with_subgraph(StaticSubgraph::new(
                "products",
                "http://products.test/graphql",
                "http://products.test/sdl",
            ))
    }

    fn check_manifest(bytes: &[u8]) -> Result<RouterConfig, RouterError> {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "router-manifest-{}-{nonce}.json",
            std::process::id()
        ));
        std::fs::write(&path, bytes).unwrap();
        let result = base().with_persisted_allow_list(&path).validate();
        std::fs::remove_file(path).unwrap();
        result
    }

    #[test]
    fn persisted_manifest_accepts_documents_up_to_256_kib() {
        use sha2::{Digest, Sha256};
        for size in [20 * 1024, 70 * 1024, 256 * 1024] {
            let query = "query { __typename }";
            let doc = format!("{query}{}", " ".repeat(size - query.len()));
            let bytes = serde_json::to_vec(&BTreeMap::from([(
                format!("{:x}", Sha256::digest(doc.as_bytes())),
                doc,
            )]))
            .unwrap();
            check_manifest(&bytes).unwrap();
        }
    }

    #[test]
    fn persisted_manifest_document_size_is_bounded() {
        use sha2::{Digest, Sha256};
        let doc = format!("{}query {{ __typename }}", " ".repeat(256 * 1024));
        let bytes = serde_json::to_vec(&BTreeMap::from([(
            format!("{:x}", Sha256::digest(doc.as_bytes())),
            doc,
        )]))
        .unwrap();
        let error = check_manifest(&bytes).unwrap_err().to_string();
        assert!(error.contains("262144"), "{error}");
    }

    #[test]
    fn persisted_snapshot_revalidation_refuses_changes_and_removal() {
        use sha2::{Digest, Sha256};
        let doc = "query { __typename }";
        let config = check_manifest(
            &serde_json::to_vec(&BTreeMap::from([(
                format!("{:x}", Sha256::digest(doc.as_bytes())),
                doc,
            )]))
            .unwrap(),
        )
        .unwrap();
        let hive = config.persisted_documents.as_ref().unwrap().hive_config();
        let path = std::path::Path::new(hive["storage"]["path"].as_str().unwrap());
        std::fs::write(path, b"{}").unwrap();
        assert!(config.clone().validate().is_err());
        std::fs::remove_file(path).unwrap();
        assert!(config.validate().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn persisted_snapshot_directory_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let config = check_manifest(b"{}").unwrap();
        let hive = config.persisted_documents.as_ref().unwrap().hive_config();
        let path = std::path::Path::new(hive["storage"]["path"].as_str().unwrap());
        assert_eq!(
            std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[cfg(unix)]
    #[test]
    fn persisted_snapshot_permissions_reject_group_or_other_access() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let config = check_manifest(b"{}").unwrap();
        let hive = config.persisted_documents.as_ref().unwrap().hive_config();
        let path = std::path::Path::new(hive["storage"]["path"].as_str().unwrap());
        for target in [path.parent().unwrap(), path] {
            let metadata = std::fs::metadata(target).unwrap();
            crate::persisted::verify_private_metadata(&metadata, metadata.uid()).unwrap();
            for access in [0o040, 0o004, 0o020, 0o002, 0o010, 0o001] {
                std::fs::set_permissions(
                    target,
                    std::fs::Permissions::from_mode(metadata.mode() | access),
                )
                .unwrap();
                let unsafe_metadata = std::fs::metadata(target).unwrap();
                let result =
                    crate::persisted::verify_private_metadata(&unsafe_metadata, metadata.uid());
                std::fs::set_permissions(target, metadata.permissions()).unwrap();
                assert!(result.is_err(), "group/other access must refuse startup");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn persisted_snapshot_permissions_reject_another_owner() {
        use std::os::unix::fs::MetadataExt;
        let config = check_manifest(b"{}").unwrap();
        let hive = config.persisted_documents.as_ref().unwrap().hive_config();
        let path = std::path::Path::new(hive["storage"]["path"].as_str().unwrap());
        for target in [path.parent().unwrap(), path] {
            let metadata = std::fs::metadata(target).unwrap();
            assert!(
                crate::persisted::verify_private_metadata(
                    &metadata,
                    metadata.uid().wrapping_add(1)
                )
                .is_err()
            );
        }
    }

    #[test]
    fn persisted_manifest_bad_hash() {
        let error = check_manifest(br#"{"bad":"secret document"}"#)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("secret document"));
    }

    #[test]
    fn persisted_manifest_oversized() {
        assert!(check_manifest(&vec![b' '; 16 * 1024 * 1024 + 1]).is_err());
    }

    #[test]
    fn persisted_manifest_too_many_entries() {
        use sha2::{Digest, Sha256};
        let entries = (0..10_001)
            .map(|n| {
                let doc = format!("query Q{n} {{ __typename }}");
                (format!("{:x}", Sha256::digest(doc.as_bytes())), doc)
            })
            .collect::<BTreeMap<_, _>>();
        assert!(check_manifest(&serde_json::to_vec(&entries).unwrap()).is_err());
    }

    #[test]
    fn persisted_manifest_remote_storage_refused() {
        for path in [
            "hive://manifest",
            "https://example.test/manifest",
            "s3://bucket/manifest",
        ] {
            assert!(base().with_persisted_ids_or_text(path).validate().is_err());
        }
    }

    #[test]
    fn persisted_manifest_exact_hash_and_immutable_snapshot() {
        use sha2::{Digest, Sha256};
        let doc = "{ __typename }";
        let config = check_manifest(
            &serde_json::to_vec(&BTreeMap::from([(
                format!("{:x}", Sha256::digest(doc.as_bytes())),
                doc,
            )]))
            .unwrap(),
        )
        .unwrap();
        // The source has been removed. Revalidation must retain the snapshot.
        assert!(config.validate().is_ok());
    }

    #[test]
    fn node_lookup_reserves_its_subgraph_name_and_bounds_its_list() {
        assert!(
            base()
                .with_node_lookup(NodeLookupConfig::new())
                .validate()
                .is_ok()
        );
        for max_nodes in [0, 10_001] {
            let error = base()
                .with_node_lookup(NodeLookupConfig::new().with_max_nodes(max_nodes))
                .validate()
                .unwrap_err();
            assert_eq!(error.kind(), RouterErrorKind::InvalidConfiguration);
        }
        let error = base()
            .with_subgraph(StaticSubgraph::new(
                NODE_SUBGRAPH_NAME,
                "http://node.test/graphql",
                "http://node.test/sdl",
            ))
            .validate()
            .unwrap_err();
        assert!(error.to_string().contains("reserved"), "{error}");
    }

    #[test]
    fn anonymous_mode_is_explicit_and_sensitive_forwarding_is_rejected() {
        let closed = RouterConfig::new("127.0.0.1:4000".parse().unwrap());
        assert_eq!(
            closed.validate().unwrap_err().kind(),
            RouterErrorKind::InvalidConfiguration
        );

        let error = base()
            .forward_header("authorization")
            .validate()
            .unwrap_err();
        assert_eq!(error.kind(), RouterErrorKind::InvalidConfiguration);
    }

    #[test]
    fn static_config_rejects_duplicate_names_and_unsafe_urls() {
        let duplicate = base().with_subgraph(StaticSubgraph::new(
            "products",
            "http://second.test/graphql",
            "http://second.test/sdl",
        ));
        assert!(duplicate.validate().is_err());

        let embedded_secret = RouterConfig::new("127.0.0.1:4000".parse().unwrap())
            .allow_anonymous_development(true)
            .with_subgraph(StaticSubgraph::new(
                "products",
                "http://user:secret@products.test/graphql",
                "http://products.test/sdl",
            ));
        assert!(embedded_secret.validate().is_err());
    }

    #[test]
    fn plaintext_outbound_urls_need_a_loopback_host() {
        let anonymous = |graphql: &str, sdl: &str| {
            RouterConfig::new("127.0.0.1:4000".parse().unwrap())
                .allow_anonymous_development(true)
                .with_subgraph(StaticSubgraph::new("products", graphql, sdl))
        };
        for (graphql, sdl) in [
            ("http://products.test/graphql", "https://products.test/sdl"),
            ("https://products.test/graphql", "http://products.test/sdl"),
            ("http://10.0.0.5/graphql", "https://10.0.0.5/sdl"),
            (
                "http://localhost.example/graphql",
                "https://localhost.example/sdl",
            ),
        ] {
            let error = anonymous(graphql, sdl).validate().unwrap_err();
            assert_eq!(error.kind(), RouterErrorKind::InvalidConfiguration);
            assert!(error.to_string().contains("plaintext"), "{error}");
        }
        let protocol = RouterConfig::new("127.0.0.1:4000".parse().unwrap())
            .allow_anonymous_development(true)
            .with_subgraph(
                StaticSubgraph::new(
                    "products",
                    "https://products.test/graphql",
                    "https://products.test/sdl",
                )
                .with_protocol_url("http://products.test/.well-known/graphql-router"),
            );
        assert!(protocol.validate().is_err());
        let byte_route = anonymous("https://products.test/graphql", "https://products.test/sdl")
            .with_byte_route(ByteRoute::new("/files", "http://files.test/download").public(true));
        assert!(byte_route.validate().is_err());
        let trusted = TrustedSubgraph::new(
            "reviews-service",
            "reviews-id",
            "reviews",
            "http://reviews.test:8080/.well-known/graphql-router",
            "http://reviews.test:8080",
            "http://reviews.test:8080",
        );
        let admin = RouterConfig::new("127.0.0.1:4000".parse().unwrap())
            .with_authentication_provider(Arc::new(RejectingProvider))
            .with_subgraph(
                StaticSubgraph::new(
                    "products",
                    "https://products.test/graphql",
                    "https://products.test/sdl",
                )
                .with_protocol_url("https://products.test/.well-known/graphql-router"),
            )
            .with_admin(
                AdminConfig::new("127.0.0.1:4001".parse().unwrap(), NetworkPolicy::new())
                    .trust_subgraph(trusted),
            );
        assert!(admin.validate().is_err());

        for host in [
            "127.0.0.1:4100",
            "localhost:4100",
            "[::1]:4100",
            "127.8.9.10",
        ] {
            anonymous(
                &format!("http://{host}/graphql"),
                &format!("http://{host}/sdl"),
            )
            .with_byte_route(ByteRoute::new("/files", format!("http://{host}/files")).public(true))
            .validate()
            .unwrap_or_else(|error| panic!("{host}: {error}"));
        }
        assert!(
            anonymous("https://products.test/graphql", "https://products.test/sdl")
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn debug_output_never_contains_schema_credentials() {
        let subgraph = StaticSubgraph::new(
            "products",
            "http://products.test/graphql",
            "http://products.test/sdl",
        )
        .with_schema_header("authorization", "Bearer very-secret");
        let debug = format!("{subgraph:?}");
        assert!(debug.contains("authorization"));
        assert!(!debug.contains("very-secret"));
    }

    #[test]
    fn subscription_counts_have_upper_safety_bounds() {
        type Setter = fn(SubscriptionConfig, usize) -> SubscriptionConfig;
        let limit = MAX_SUBSCRIPTION_LIMIT;
        let setters: [(&str, Setter); 6] = [
            (
                "maximum WebSocket connections",
                SubscriptionConfig::with_max_connections,
            ),
            (
                "maximum pending WebSocket connections",
                SubscriptionConfig::with_max_pending_connections,
            ),
            (
                "maximum WebSocket connection attempts per second",
                SubscriptionConfig::with_max_connection_attempts_per_second,
            ),
            (
                "maximum operations per WebSocket connection",
                SubscriptionConfig::with_max_operations_per_connection,
            ),
            (
                "subscription broadcast capacity",
                SubscriptionConfig::with_broadcast_capacity,
            ),
            (
                "subscription subgraph buffer capacity",
                SubscriptionConfig::with_subgraph_buffer_capacity,
            ),
        ];
        for (name, set) in setters {
            set(SubscriptionConfig::new(), limit)
                .validate()
                .unwrap_or_else(|error| panic!("{name} at the bound: {error}"));
            for value in [limit + 1, usize::MAX] {
                let error = set(SubscriptionConfig::new(), value)
                    .validate()
                    .unwrap_err();
                assert_eq!(error.kind(), RouterErrorKind::InvalidConfiguration);
                assert!(error.to_string().starts_with(name), "{error}");
            }
        }
    }

    #[test]
    fn subscription_limits_and_security_requirements_fail_closed() {
        assert!(
            base()
                .with_subscriptions(SubscriptionConfig::new())
                .validate()
                .is_err()
        );
        assert!(
            SubscriptionConfig::new()
                .with_max_connections(0)
                .validate()
                .is_err()
        );
        assert!(
            SubscriptionConfig::new()
                .with_max_connection_attempts_per_second(0)
                .validate()
                .is_err()
        );
        assert!(
            SubscriptionConfig::new()
                .with_max_client_message_bytes(65_537)
                .validate()
                .is_err()
        );

        let ephemeral = RouterConfig::new("127.0.0.1:0".parse().unwrap())
            .with_authentication_provider(Arc::new(RejectingProvider))
            .with_subscriptions(SubscriptionConfig::new())
            .with_subgraph(
                StaticSubgraph::new(
                    "products",
                    "http://products.test/graphql",
                    "http://products.test/sdl",
                )
                .with_protocol_url("http://products.test/.well-known/graphql-router"),
            );
        assert!(ephemeral.validate().is_err());

        let unsafe_path = StaticSubgraph::new(
            "products",
            "http://products.test/graphql",
            "http://products.test/sdl",
        )
        .with_subscription_websocket_path("relative");
        assert!(
            RouterConfig::new("127.0.0.1:4000".parse().unwrap())
                .with_authentication_provider(Arc::new(RejectingProvider))
                .with_subscriptions(SubscriptionConfig::new())
                .with_subgraph(
                    unsafe_path
                        .with_protocol_url("http://products.test/.well-known/graphql-router")
                )
                .validate()
                .is_err()
        );
    }

    #[test]
    fn administrative_and_request_limits_are_explicit_bounded_and_redacted() {
        assert!(
            base()
                .with_request_limits(RequestLimits::new().with_max_depth(0))
                .validate()
                .is_err()
        );
        assert!(
            base()
                .with_public_request_timeout(Duration::ZERO)
                .validate()
                .is_err()
        );
        assert!(
            base()
                .with_subgraph_request_timeout(Duration::from_secs(301))
                .validate()
                .is_err()
        );
        assert!(
            base()
                .with_max_subgraph_connections_per_host(0)
                .validate()
                .is_err()
        );

        let trusted = TrustedSubgraph::new(
            "inventory-service",
            "inventory-id",
            "Inventory",
            "http://inventory.test:8080/.well-known/graphql-router",
            "http://inventory.test:8080",
            "http://inventory.test:8080",
        )
        .with_schema_header("authorization", "Bearer metadata-secret");
        let debug = format!("{trusted:?}");
        assert!(debug.contains("authorization"));
        assert!(!debug.contains("metadata-secret"));

        let policy = NetworkPolicy::new()
            .allow_host("inventory.test")
            .allow_port(8080)
            .allow_network("10.0.0.0/8".parse().unwrap())
            .allow_private(true);
        let unauthenticated = base().with_admin(
            AdminConfig::new("127.0.0.1:4001".parse().unwrap(), policy.clone())
                .trust_subgraph(trusted.clone()),
        );
        assert!(unauthenticated.validate().is_err());

        let authenticated = RouterConfig::new("127.0.0.1:4000".parse().unwrap())
            .allow_plaintext_http(true)
            .with_authentication_provider(Arc::new(RejectingProvider))
            .with_subgraph(
                StaticSubgraph::new(
                    "products",
                    "http://products.test/graphql",
                    "http://products.test/sdl",
                )
                .with_protocol_url("http://products.test/.well-known/graphql-router"),
            )
            .with_admin(
                AdminConfig::new("127.0.0.1:4001".parse().unwrap(), policy)
                    .with_authentication_provider(Arc::new(RejectingProvider))
                    .trust_subgraph(trusted),
            );
        assert!(authenticated.validate().is_ok());
    }

    fn authenticated_with_admin(admin: AdminConfig) -> RouterConfig {
        let admin = if admin.authentication.is_none() {
            admin.with_authentication_provider(Arc::new(RejectingProvider))
        } else {
            admin
        };
        RouterConfig::new("127.0.0.1:4000".parse().unwrap())
            .allow_plaintext_http(true)
            .with_authentication_provider(Arc::new(RejectingProvider))
            .with_subgraph(
                StaticSubgraph::new(
                    "products",
                    "http://products.test/graphql",
                    "http://products.test/sdl",
                )
                .with_protocol_url("http://products.test/.well-known/graphql-router"),
            )
            .with_admin(admin)
    }

    #[test]
    fn administrative_listener_needs_its_own_port_on_every_address() {
        for listener in ["0.0.0.0:4000", "127.0.0.2:4000", "[::]:4000"] {
            let error = authenticated_with_admin(AdminConfig::new(
                listener.parse().unwrap(),
                NetworkPolicy::new(),
            ))
            .validate()
            .unwrap_err();
            assert_eq!(
                error.kind(),
                RouterErrorKind::InvalidConfiguration,
                "{listener}"
            );
        }
        assert!(
            authenticated_with_admin(AdminConfig::new(
                "0.0.0.0:4001".parse().unwrap(),
                NetworkPolicy::new(),
            ))
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn administrative_endpoints_need_a_provider_that_refuses_public_audiences() {
        let admin = || AdminConfig::new("127.0.0.1:4001".parse().unwrap(), NetworkPolicy::new());
        let jwks = |audiences: &[&str]| -> Arc<dyn AuthenticationProvider> {
            Arc::new(
                crate::JwksAuthenticationProvider::new(
                    crate::JwksAuthenticationConfig::new(
                        "https://issuer.test/jwks",
                        "https://issuer.test",
                        audiences.iter().copied(),
                    )
                    .unwrap(),
                )
                .unwrap(),
            )
        };
        let router = |public: Arc<dyn AuthenticationProvider>, admin: AdminConfig| {
            RouterConfig::new("127.0.0.1:4000".parse().unwrap())
                .with_authentication_provider(public)
                .with_subgraph(
                    StaticSubgraph::new(
                        "products",
                        "http://127.0.0.1:4100/graphql",
                        "http://127.0.0.1:4100/sdl",
                    )
                    .with_protocol_url("http://127.0.0.1:4100/.well-known/graphql-router"),
                )
                .with_admin(admin)
        };

        // Without its own provider a public bearer could administer the router.
        let error = router(jwks(&["graphql-router"]), admin())
            .validate()
            .unwrap_err();
        assert!(error.to_string().contains("own authentication"), "{error}");
        let public = jwks(&["graphql-router"]);
        let error = router(public.clone(), admin().with_authentication_provider(public))
            .validate()
            .unwrap_err();
        assert!(
            error.to_string().contains("public token audience"),
            "{error}"
        );
        let error = router(
            jwks(&["graphql-router", "another-api"]),
            admin().with_authentication_provider(jwks(&["router-admin", "another-api"])),
        )
        .validate()
        .unwrap_err();
        assert!(
            error.to_string().contains("public token audience"),
            "{error}"
        );

        router(
            jwks(&["graphql-router"]),
            admin().with_authentication_provider(jwks(&["router-admin"])),
        )
        .validate()
        .unwrap();
        // A provider that cannot state its audiences is the operator's to keep distinct.
        router(
            Arc::new(RejectingProvider),
            admin().with_authentication_provider(Arc::new(RejectingProvider)),
        )
        .validate()
        .unwrap();
    }

    #[test]
    fn plaintext_outbound_urls_to_other_hosts_need_an_explicit_opt_in() {
        let config = RouterConfig::new("127.0.0.1:4000".parse().unwrap())
            .allow_anonymous_development(true)
            .allow_plaintext_http(true)
            .with_subgraph(
                StaticSubgraph::new(
                    "products",
                    "http://products.internal/graphql",
                    "http://10.0.0.5/sdl",
                )
                .with_protocol_url("http://products.internal/.well-known/graphql-router"),
            )
            .with_byte_route(
                ByteRoute::new("/files", "http://files.internal/download").public(true),
            );
        config.validate().unwrap();
    }

    #[test]
    fn trusted_dynamic_subgraphs_cannot_take_the_reserved_node_lookup_name() {
        let trusted = TrustedSubgraph::new(
            "node-service",
            "node-id",
            NODE_SUBGRAPH_NAME,
            "http://node.test:8080/.well-known/graphql-router",
            "http://node.test:8080",
            "http://node.test:8080",
        );
        let error = authenticated_with_admin(
            AdminConfig::new("127.0.0.1:4001".parse().unwrap(), NetworkPolicy::new())
                .trust_subgraph(trusted),
        )
        .validate()
        .unwrap_err();
        // The reserved name is not GraphQL-name compatible, so trusted-record
        // validation refuses it before any registration can use it.
        assert_eq!(
            error.kind(),
            RouterErrorKind::InvalidConfiguration,
            "{error}"
        );
    }

    #[test]
    fn introspection_is_disabled_unless_explicitly_enabled() {
        assert!(!base().validate().unwrap().introspection);
        assert!(
            base()
                .with_introspection(true)
                .validate()
                .unwrap()
                .introspection
        );
    }

    #[test]
    fn pending_websocket_budget_is_bounded_and_separate() {
        let defaults = SubscriptionConfig::new();
        assert_eq!(defaults.connection_init_timeout, Duration::from_secs(3));
        assert_eq!(defaults.max_connections, 1_024);
        // The attempt rate is the first limit a stream of idle upgrades hits:
        // by default the pending budget holds a full init timeout of attempts.
        for config in [
            defaults.clone(),
            SubscriptionConfig::new().with_max_connection_attempts_per_second(10),
            SubscriptionConfig::new().with_connection_init_timeout(Duration::from_millis(2_500)),
            SubscriptionConfig::new()
                .with_max_connection_attempts_per_second(1_000)
                .with_connection_init_timeout(Duration::from_secs(60)),
            SubscriptionConfig::new()
                .with_max_connection_attempts_per_second(3)
                .with_connection_init_timeout(Duration::from_millis(1)),
        ] {
            let held = (config.max_connection_attempts_per_second as u128
                * config.connection_init_timeout.as_millis())
            .div_ceil(1_000) as usize;
            assert_eq!(
                config.pending_connection_limit(),
                held.clamp(1, config.max_connections),
                "{config:?}"
            );
            assert!(
                config.pending_connection_limit() >= held.min(config.max_connections),
                "{config:?}"
            );
        }
        assert_eq!(defaults.pending_connection_limit(), 384);
        assert_eq!(
            SubscriptionConfig::new()
                .with_max_pending_connections(5)
                .pending_connection_limit(),
            5
        );
        assert!(
            SubscriptionConfig::new()
                .with_max_pending_connections(0)
                .validate()
                .is_err()
        );
        assert!(
            SubscriptionConfig::new()
                .with_max_pending_connections(3)
                .validate()
                .is_ok()
        );
    }
}
