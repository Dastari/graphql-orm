use std::{
    collections::BTreeMap, fs::File, io::Read, net::SocketAddr, path::Path, sync::Arc,
    time::Duration,
};

use serde::Deserialize;

#[cfg(feature = "auth-agql")]
use agql_auth::{HierarchicalScopeMatch, HierarchicalScopeOptions};

#[cfg(feature = "auth-agql")]
use crate::AgqlScopeMatcher;
use crate::{
    AdminConfig, ExactScopeMatcher, JwksAuthenticationConfig, JwksAuthenticationProvider,
    LegacyScopeClaims, NetworkCidr, NetworkPolicy, RequestLimits, RoleScopeCatalogueConfig,
    RouterConfig, RouterError, RouterErrorKind, RouterLogLevel, RouterTelemetryConfig,
    StaticSubgraph, SubscriptionConfig, TrustedSubgraph,
};

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const PUBLIC_LISTENER_ENV: &str = "GRAPHQL_ORM_ROUTER_LISTENER";
const ADMIN_LISTENER_ENV: &str = "GRAPHQL_ORM_ROUTER_ADMIN_LISTENER";

/// Strict JSON configuration consumed by the standalone router executable.
///
/// Unknown fields are rejected. Header values are never accepted directly;
/// configuration names environment variables from which secrets are loaded.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouterFileConfig {
    listener: SocketAddr,
    #[serde(default = "default_graphql_path")]
    graphql_path: String,
    #[serde(default)]
    anonymous_development: bool,
    authentication: Option<FileAuthentication>,
    scope_matcher: Option<FileScopeMatcher>,
    subgraphs: Vec<FileSubgraph>,
    #[serde(default)]
    forwarded_headers: Vec<String>,
    schema_fetch_timeout_ms: Option<u64>,
    max_sdl_bytes: Option<usize>,
    schema_poll_interval_ms: Option<u64>,
    schema_refresh_attempts: Option<usize>,
    schema_refresh_retry_delay_ms: Option<u64>,
    request_limits: Option<FileRequestLimits>,
    subscriptions: Option<FileSubscriptions>,
    admin: Option<FileAdmin>,
    telemetry: Option<FileTelemetry>,
    graceful_shutdown_timeout_seconds: Option<u64>,
    public_request_timeout_ms: Option<u64>,
    subgraph_request_timeout_ms: Option<u64>,
    max_subgraph_connections_per_host: Option<usize>,
}

impl RouterFileConfig {
    /// Parses a strict JSON document without reading environment secrets.
    pub fn from_json(json: &str) -> Result<Self, RouterError> {
        if json.len() as u64 > MAX_CONFIG_BYTES {
            return Err(invalid("router configuration exceeds 1 MiB"));
        }
        serde_json::from_str(json)
            .map_err(|error| invalid(format!("router configuration is invalid: {error}")))
    }

    /// Reads and parses a bounded strict JSON configuration file.
    pub fn load_json(path: impl AsRef<Path>) -> Result<Self, RouterError> {
        let file = File::open(path.as_ref())
            .map_err(|_| invalid("failed to open router configuration file"))?;
        let mut bytes = Vec::new();
        file.take(MAX_CONFIG_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid("failed to read router configuration file"))?;
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err(invalid("router configuration exceeds 1 MiB"));
        }
        let json = std::str::from_utf8(&bytes)
            .map_err(|_| invalid("router configuration is not UTF-8"))?;
        Self::from_json(json)
    }

    /// Resolves explicitly named environment secrets and listener overrides,
    /// then constructs the same validated programmatic configuration surface.
    pub fn into_router_config(self) -> Result<RouterConfig, RouterError> {
        self.into_router_config_with(read_environment)
    }

    fn into_router_config_with(
        self,
        environment: impl Fn(&str) -> Result<Option<String>, RouterError>,
    ) -> Result<RouterConfig, RouterError> {
        let listener = environment(PUBLIC_LISTENER_ENV)?
            .map(|value| parse_listener(&value, PUBLIC_LISTENER_ENV))
            .transpose()?
            .unwrap_or(self.listener);
        let mut config = RouterConfig::builder(listener)
            .with_graphql_path(self.graphql_path)
            .allow_anonymous_development(self.anonymous_development);
        if let Some(scope_matcher) = self.scope_matcher {
            config = scope_matcher.apply(config)?;
        }
        if let Some(authentication) = self.authentication {
            let mut jwks = JwksAuthenticationConfig::new(
                authentication.jwks_url,
                authentication.issuer,
                authentication.audiences,
            )?;
            if let Some(seconds) = authentication.cache_ttl_seconds {
                jwks = jwks.with_cache_ttl(Duration::from_secs(seconds));
            }
            if let Some(seconds) = authentication.refresh_interval_seconds {
                jwks = jwks.with_refresh_interval(Duration::from_secs(seconds));
            }
            if let Some(milliseconds) = authentication.request_timeout_ms {
                jwks = jwks.with_request_timeout(Duration::from_millis(milliseconds));
            }
            if let Some(bytes) = authentication.max_jwks_bytes {
                jwks = jwks.with_max_jwks_bytes(bytes);
            }
            if let Some(seconds) = authentication.leeway_seconds {
                jwks = jwks.with_leeway(Duration::from_secs(seconds));
            }
            jwks = jwks
                .with_legacy_scope_claims(if authentication.accept_legacy_scopes {
                    LegacyScopeClaims::Accept
                } else {
                    LegacyScopeClaims::Reject
                })
                .allow_insecure_loopback_http_for_development(
                    authentication.allow_insecure_loopback_jwks,
                );
            if let Some(catalogue) = authentication.role_scope_catalogue {
                let catalogue_url = resolve_configured_value(
                    catalogue.url,
                    catalogue.url_from_env,
                    "role-scope catalogue URL",
                    &environment,
                )?;
                let mut role_scope =
                    RoleScopeCatalogueConfig::new(catalogue_url, catalogue.audience)?;
                if let Some(seconds) = catalogue.cache_ttl_seconds {
                    role_scope = role_scope.with_cache_ttl(Duration::from_secs(seconds));
                }
                if let Some(seconds) = catalogue.maximum_signed_lifetime_seconds {
                    role_scope =
                        role_scope.with_maximum_signed_lifetime(Duration::from_secs(seconds));
                }
                if let Some(seconds) = catalogue.clock_skew_leeway_seconds {
                    role_scope = role_scope.with_clock_skew_leeway(Duration::from_secs(seconds));
                }
                if catalogue.retry_backoff_seconds.is_some()
                    || catalogue.maximum_retry_backoff_seconds.is_some()
                {
                    role_scope = role_scope.with_retry_backoff(
                        Duration::from_secs(catalogue.retry_backoff_seconds.unwrap_or(1)),
                        Duration::from_secs(catalogue.maximum_retry_backoff_seconds.unwrap_or(60)),
                    );
                }
                if let Some(bytes) = catalogue.max_body_bytes {
                    role_scope = role_scope.with_max_body_bytes(bytes);
                }
                for (name, value) in resolve_headers_for(
                    catalogue.request_headers_from_env,
                    &environment,
                    "role-scope catalogue request header",
                )? {
                    role_scope = role_scope.with_request_header(name, value)?;
                }
                let allow_insecure_loopback_http = resolve_optional_bool(
                    catalogue.allow_insecure_loopback_http,
                    catalogue.allow_insecure_loopback_http_from_env,
                    "role-scope catalogue insecure-loopback policy",
                    &environment,
                )?;
                role_scope = role_scope
                    .allow_insecure_loopback_http_for_development(allow_insecure_loopback_http);
                jwks = jwks.with_role_scope_catalogue(role_scope);
            }
            config = config
                .with_authentication_provider(Arc::new(JwksAuthenticationProvider::new(jwks)?));
        }
        for subgraph in self.subgraphs {
            let mut value =
                StaticSubgraph::new(subgraph.name, subgraph.graphql_url, subgraph.sdl_url);
            if let Some(url) = subgraph.protocol_url {
                value = value.with_protocol_url(url);
            }
            if let Some(path) = subgraph.subscription_websocket_path {
                value = value.with_subscription_websocket_path(path);
            }
            for (name, value_from_environment) in
                resolve_headers(subgraph.schema_headers_from_env, &environment)?
            {
                value = value.with_schema_header(name, value_from_environment);
            }
            config = config.with_subgraph(value);
        }
        for header in self.forwarded_headers {
            config = config.forward_header(header);
        }
        if let Some(milliseconds) = self.schema_fetch_timeout_ms {
            config = config.with_schema_fetch_timeout(Duration::from_millis(milliseconds));
        }
        if let Some(bytes) = self.max_sdl_bytes {
            config = config.with_max_sdl_bytes(bytes);
        }
        if let Some(milliseconds) = self.schema_poll_interval_ms {
            config = config.with_schema_poll_interval(Duration::from_millis(milliseconds));
        }
        if let Some(attempts) = self.schema_refresh_attempts {
            config = config.with_schema_refresh_attempts(attempts);
        }
        if let Some(milliseconds) = self.schema_refresh_retry_delay_ms {
            config = config.with_schema_refresh_retry_delay(Duration::from_millis(milliseconds));
        }
        if let Some(limits) = self.request_limits {
            config = config.with_request_limits(limits.build());
        }
        if let Some(subscriptions) = self.subscriptions {
            config = config.with_subscriptions(subscriptions.build());
        }
        if let Some(telemetry) = self.telemetry {
            config = config.with_telemetry(telemetry.build());
        }
        if let Some(seconds) = self.graceful_shutdown_timeout_seconds {
            config = config.with_graceful_shutdown_timeout(Duration::from_secs(seconds));
        }
        if let Some(milliseconds) = self.public_request_timeout_ms {
            config = config.with_public_request_timeout(Duration::from_millis(milliseconds));
        }
        if let Some(milliseconds) = self.subgraph_request_timeout_ms {
            config = config.with_subgraph_request_timeout(Duration::from_millis(milliseconds));
        }
        if let Some(maximum) = self.max_subgraph_connections_per_host {
            config = config.with_max_subgraph_connections_per_host(maximum);
        }
        if let Some(admin) = self.admin {
            let listener = environment(ADMIN_LISTENER_ENV)?
                .map(|value| parse_listener(&value, ADMIN_LISTENER_ENV))
                .transpose()?
                .unwrap_or(admin.listener);
            let mut network = NetworkPolicy::new();
            for host in admin.network.allowed_hosts {
                network = network.allow_host(host);
            }
            for port in admin.network.allowed_ports {
                network = network.allow_port(port);
            }
            for value in admin.network.allowed_networks {
                let network_value = value.parse::<NetworkCidr>().map_err(|error| {
                    invalid(format!("dynamic network `{value}` is invalid: {error}"))
                })?;
                network = network.allow_network(network_value);
            }
            network = network
                .allow_loopback(admin.network.allow_loopback)
                .allow_private(admin.network.allow_private)
                .allow_link_local(admin.network.allow_link_local);
            if let Some(milliseconds) = admin.network.dns_timeout_ms {
                network = network.with_dns_timeout(Duration::from_millis(milliseconds));
            }
            if let Some(maximum) = admin.network.max_resolved_addresses {
                network = network.with_max_resolved_addresses(maximum);
            }
            let mut value = AdminConfig::new(listener, network);
            if let Some(scope) = admin.status_scope {
                value = value.with_status_scope(scope);
            }
            if let Some(scope) = admin.refresh_scope {
                value = value.with_refresh_scope(scope);
            }
            if let Some(scope) = admin.registration_scope {
                value = value.with_registration_scope(scope);
            }
            if let Some(scope) = admin.removal_scope {
                value = value.with_removal_scope(scope);
            }
            if let Some(scope) = admin.metrics_scope {
                value = value.with_metrics_scope(scope);
            }
            if let Some(bytes) = admin.max_request_body_bytes {
                value = value.with_max_request_body_bytes(bytes);
            }
            for trusted in admin.trusted_subgraphs {
                let mut binding = TrustedSubgraph::new(
                    trusted.service_subject,
                    trusted.subgraph_id,
                    trusted.name,
                    trusted.metadata_url,
                    trusted.graphql_origin,
                    trusted.schema_origin,
                );
                for (name, value_from_environment) in
                    resolve_headers(trusted.schema_headers_from_env, &environment)?
                {
                    binding = binding.with_schema_header(name, value_from_environment);
                }
                value = value.trust_subgraph(binding);
            }
            config = config.with_admin(value);
        }
        Ok(config)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileAuthentication {
    jwks_url: String,
    issuer: String,
    audiences: Vec<String>,
    cache_ttl_seconds: Option<u64>,
    refresh_interval_seconds: Option<u64>,
    request_timeout_ms: Option<u64>,
    max_jwks_bytes: Option<usize>,
    leeway_seconds: Option<u64>,
    #[serde(default)]
    accept_legacy_scopes: bool,
    #[serde(default)]
    allow_insecure_loopback_jwks: bool,
    role_scope_catalogue: Option<FileRoleScopeCatalogue>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileRoleScopeCatalogue {
    url: Option<String>,
    url_from_env: Option<String>,
    audience: String,
    cache_ttl_seconds: Option<u64>,
    maximum_signed_lifetime_seconds: Option<u64>,
    clock_skew_leeway_seconds: Option<u64>,
    retry_backoff_seconds: Option<u64>,
    maximum_retry_backoff_seconds: Option<u64>,
    max_body_bytes: Option<usize>,
    #[serde(default)]
    allow_insecure_loopback_http: bool,
    allow_insecure_loopback_http_from_env: Option<String>,
    #[serde(default)]
    request_headers_from_env: BTreeMap<String, String>,
}

/// File-owned scope policy. Omission and `kind: exact` preserve the router's
/// historical exact-string behavior.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileScopeMatcher {
    kind: FileScopeMatcherKind,
    separator: Option<char>,
    wildcard: Option<String>,
    wildcard_matches_multi_segment: Option<bool>,
    allow_universal_wildcard: Option<bool>,
    #[serde(default)]
    super_scopes: Vec<String>,
    #[serde(default)]
    exact_only_scopes: Vec<String>,
    #[serde(default)]
    exact_only_scope_patterns: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum FileScopeMatcherKind {
    Exact,
    Hierarchical,
}

impl FileScopeMatcher {
    fn apply(self, config: RouterConfig) -> Result<RouterConfig, RouterError> {
        match self.kind {
            FileScopeMatcherKind::Exact => {
                if self.separator.is_some()
                    || self.wildcard.is_some()
                    || self.wildcard_matches_multi_segment.is_some()
                    || self.allow_universal_wildcard.is_some()
                    || !self.super_scopes.is_empty()
                    || !self.exact_only_scopes.is_empty()
                    || !self.exact_only_scope_patterns.is_empty()
                {
                    return Err(invalid(
                        "scopeMatcher kind `exact` does not accept hierarchical options",
                    ));
                }
                Ok(config.with_scope_matcher(Arc::new(ExactScopeMatcher)))
            }
            FileScopeMatcherKind::Hierarchical => build_hierarchical_scope_matcher(
                config,
                self.separator,
                self.wildcard,
                self.wildcard_matches_multi_segment,
                self.allow_universal_wildcard,
                self.super_scopes,
                self.exact_only_scopes,
                self.exact_only_scope_patterns,
            ),
        }
    }
}

#[cfg(feature = "auth-agql")]
#[allow(clippy::too_many_arguments)]
fn build_hierarchical_scope_matcher(
    config: RouterConfig,
    separator: Option<char>,
    wildcard: Option<String>,
    wildcard_matches_multi_segment: Option<bool>,
    allow_universal_wildcard: Option<bool>,
    mut super_scopes: Vec<String>,
    mut exact_only_scopes: Vec<String>,
    mut exact_only_scope_patterns: Vec<String>,
) -> Result<RouterConfig, RouterError> {
    let defaults = HierarchicalScopeOptions::default();
    let separator = separator.unwrap_or(defaults.separator);
    let wildcard = wildcard.unwrap_or(defaults.wildcard);
    validate_scope_syntax(separator, &wildcard)?;
    for (field, scopes) in [
        ("superScopes", super_scopes.as_slice()),
        ("exactOnlyScopes", exact_only_scopes.as_slice()),
        (
            "exactOnlyScopePatterns",
            exact_only_scope_patterns.as_slice(),
        ),
    ] {
        validate_scope_values(field, scopes)?;
    }
    super_scopes.sort();
    super_scopes.dedup();
    exact_only_scopes.sort();
    exact_only_scopes.dedup();
    exact_only_scope_patterns.sort();
    exact_only_scope_patterns.dedup();
    let options = HierarchicalScopeOptions::default()
        .with_separator(separator)
        .with_wildcard(wildcard)
        .with_wildcard_matches_multi_segment(
            wildcard_matches_multi_segment.unwrap_or(defaults.wildcard_matches_multi_segment),
        )
        .with_allow_universal_wildcard(
            allow_universal_wildcard.unwrap_or(defaults.allow_universal_wildcard),
        )
        .with_super_scopes(super_scopes)
        .with_exact_only_scopes(exact_only_scopes)
        .with_exact_only_scope_patterns(exact_only_scope_patterns);
    let matcher = HierarchicalScopeMatch::new(options).map_err(|error| {
        invalid(format!(
            "scopeMatcher hierarchical options are invalid: {error}"
        ))
    })?;
    Ok(config.with_scope_matcher(Arc::new(AgqlScopeMatcher::new(Arc::new(matcher)))))
}

#[cfg(not(feature = "auth-agql"))]
#[allow(clippy::too_many_arguments)]
fn build_hierarchical_scope_matcher(
    _config: RouterConfig,
    _separator: Option<char>,
    _wildcard: Option<String>,
    _wildcard_matches_multi_segment: Option<bool>,
    _allow_universal_wildcard: Option<bool>,
    _super_scopes: Vec<String>,
    _exact_only_scopes: Vec<String>,
    _exact_only_scope_patterns: Vec<String>,
) -> Result<RouterConfig, RouterError> {
    Err(invalid(
        "scopeMatcher kind `hierarchical` requires the `auth-agql` feature",
    ))
}

#[cfg(feature = "auth-agql")]
fn validate_scope_syntax(separator: char, wildcard: &str) -> Result<(), RouterError> {
    if separator.is_whitespace() || separator.is_control() {
        return Err(invalid("scopeMatcher separator must be visible"));
    }
    if wildcard.is_empty()
        || wildcard.contains(separator)
        || wildcard
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(invalid(
            "scopeMatcher wildcard must be a non-empty segment without whitespace",
        ));
    }
    Ok(())
}

#[cfg(feature = "auth-agql")]
fn validate_scope_values(field: &str, scopes: &[String]) -> Result<(), RouterError> {
    if scopes.iter().any(|scope| {
        scope.is_empty()
            || scope
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
    }) {
        return Err(invalid(format!(
            "scopeMatcher {field} contains an empty scope or whitespace"
        )));
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileSubgraph {
    name: String,
    graphql_url: String,
    sdl_url: String,
    protocol_url: Option<String>,
    subscription_websocket_path: Option<String>,
    #[serde(default)]
    schema_headers_from_env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileRequestLimits {
    max_request_body_bytes: Option<usize>,
    max_request_header_bytes: Option<usize>,
    max_parser_tokens: Option<usize>,
    max_depth: Option<usize>,
    max_aliases: Option<usize>,
    max_directives: Option<usize>,
    max_fields: Option<usize>,
}

impl FileRequestLimits {
    fn build(self) -> RequestLimits {
        let mut value = RequestLimits::new();
        if let Some(limit) = self.max_request_body_bytes {
            value = value.with_max_request_body_bytes(limit);
        }
        if let Some(limit) = self.max_request_header_bytes {
            value = value.with_max_request_header_bytes(limit);
        }
        if let Some(limit) = self.max_parser_tokens {
            value = value.with_max_parser_tokens(limit);
        }
        if let Some(limit) = self.max_depth {
            value = value.with_max_depth(limit);
        }
        if let Some(limit) = self.max_aliases {
            value = value.with_max_aliases(limit);
        }
        if let Some(limit) = self.max_directives {
            value = value.with_max_directives(limit);
        }
        if let Some(limit) = self.max_fields {
            value = value.with_max_fields(limit);
        }
        value
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileSubscriptions {
    max_connections: Option<usize>,
    max_connection_attempts_per_second: Option<usize>,
    max_operations_per_connection: Option<usize>,
    broadcast_capacity: Option<usize>,
    subgraph_buffer_capacity: Option<usize>,
    max_client_message_bytes: Option<usize>,
    connection_init_timeout_ms: Option<u64>,
}

impl FileSubscriptions {
    fn build(self) -> SubscriptionConfig {
        let mut value = SubscriptionConfig::new();
        if let Some(limit) = self.max_connections {
            value = value.with_max_connections(limit);
        }
        if let Some(limit) = self.max_connection_attempts_per_second {
            value = value.with_max_connection_attempts_per_second(limit);
        }
        if let Some(limit) = self.max_operations_per_connection {
            value = value.with_max_operations_per_connection(limit);
        }
        if let Some(limit) = self.broadcast_capacity {
            value = value.with_broadcast_capacity(limit);
        }
        if let Some(limit) = self.subgraph_buffer_capacity {
            value = value.with_subgraph_buffer_capacity(limit);
        }
        if let Some(limit) = self.max_client_message_bytes {
            value = value.with_max_client_message_bytes(limit);
        }
        if let Some(milliseconds) = self.connection_init_timeout_ms {
            value = value.with_connection_init_timeout(Duration::from_millis(milliseconds));
        }
        value
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileAdmin {
    listener: SocketAddr,
    network: FileNetworkPolicy,
    #[serde(default)]
    trusted_subgraphs: Vec<FileTrustedSubgraph>,
    status_scope: Option<String>,
    refresh_scope: Option<String>,
    registration_scope: Option<String>,
    removal_scope: Option<String>,
    metrics_scope: Option<String>,
    max_request_body_bytes: Option<usize>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileNetworkPolicy {
    #[serde(default)]
    allowed_hosts: Vec<String>,
    #[serde(default)]
    allowed_ports: Vec<u16>,
    #[serde(default)]
    allowed_networks: Vec<String>,
    #[serde(default)]
    allow_loopback: bool,
    #[serde(default)]
    allow_private: bool,
    #[serde(default)]
    allow_link_local: bool,
    dns_timeout_ms: Option<u64>,
    max_resolved_addresses: Option<usize>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileTrustedSubgraph {
    service_subject: String,
    subgraph_id: String,
    name: String,
    metadata_url: String,
    graphql_origin: String,
    schema_origin: String,
    #[serde(default)]
    schema_headers_from_env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileTelemetry {
    log_level: Option<FileLogLevel>,
    #[serde(default)]
    text_logs_for_development: bool,
    prometheus: Option<FilePrometheus>,
}

impl FileTelemetry {
    fn build(self) -> RouterTelemetryConfig {
        let mut value = RouterTelemetryConfig::new()
            .with_text_logs_for_development(self.text_logs_for_development);
        if let Some(level) = self.log_level {
            value = value.with_log_level(level.into());
        }
        if let Some(prometheus) = self.prometheus {
            value = value.with_prometheus(prometheus.port, prometheus.path);
        }
        value
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum FileLogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl From<FileLogLevel> for RouterLogLevel {
    fn from(value: FileLogLevel) -> Self {
        match value {
            FileLogLevel::Debug => Self::Debug,
            FileLogLevel::Info => Self::Info,
            FileLogLevel::Warn => Self::Warn,
            FileLogLevel::Error => Self::Error,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FilePrometheus {
    port: u16,
    #[serde(default = "default_metrics_path")]
    path: String,
}

fn resolve_headers(
    headers: BTreeMap<String, String>,
    environment: &impl Fn(&str) -> Result<Option<String>, RouterError>,
) -> Result<BTreeMap<String, String>, RouterError> {
    resolve_headers_for(headers, environment, "schema header")
}

fn resolve_headers_for(
    headers: BTreeMap<String, String>,
    environment: &impl Fn(&str) -> Result<Option<String>, RouterError>,
    resource: &'static str,
) -> Result<BTreeMap<String, String>, RouterError> {
    headers
        .into_iter()
        .map(|(name, variable)| {
            let value = environment(&variable)?.ok_or_else(|| {
                invalid(format!(
                    "required {resource} environment variable `{variable}` is missing"
                ))
            })?;
            if value.is_empty() {
                return Err(invalid(format!(
                    "required {resource} environment variable `{variable}` is empty"
                )));
            }
            Ok((name, value))
        })
        .collect()
}

fn resolve_configured_value(
    literal: Option<String>,
    from_environment: Option<String>,
    resource: &'static str,
    environment: &impl Fn(&str) -> Result<Option<String>, RouterError>,
) -> Result<String, RouterError> {
    match (literal, from_environment) {
        (Some(value), None) if !value.is_empty() => Ok(value),
        (None, Some(variable)) => environment(&variable)?
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                invalid(format!(
                    "required {resource} environment variable `{variable}` is missing or empty"
                ))
            }),
        (Some(_), Some(_)) => Err(invalid(format!(
            "{resource} must select exactly one of a literal value or an environment variable"
        ))),
        _ => Err(invalid(format!("{resource} is required"))),
    }
}

fn resolve_optional_bool(
    literal: bool,
    from_environment: Option<String>,
    resource: &'static str,
    environment: &impl Fn(&str) -> Result<Option<String>, RouterError>,
) -> Result<bool, RouterError> {
    let Some(variable) = from_environment else {
        return Ok(literal);
    };
    if literal {
        return Err(invalid(format!(
            "{resource} must select either the literal flag or an environment variable"
        )));
    }
    match environment(&variable)?.as_deref() {
        Some("1" | "true" | "TRUE") => Ok(true),
        Some("0" | "false" | "FALSE") => Ok(false),
        Some(_) => Err(invalid(format!(
            "{resource} environment variable `{variable}` must be true or false"
        ))),
        None => Ok(false),
    }
}

fn read_environment(name: &str) -> Result<Option<String>, RouterError> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(invalid(format!(
            "environment variable `{name}` is not valid UTF-8"
        ))),
    }
}

fn parse_listener(value: &str, name: &str) -> Result<SocketAddr, RouterError> {
    value
        .parse()
        .map_err(|_| invalid(format!("environment listener override `{name}` is invalid")))
}

fn default_graphql_path() -> String {
    "/graphql".to_owned()
}

fn default_metrics_path() -> String {
    "/metrics".to_owned()
}

fn invalid(message: impl Into<String>) -> RouterError {
    RouterError::new(RouterErrorKind::InvalidConfiguration, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"{
        "listener": "127.0.0.1:4000",
        "authentication": {
            "jwksUrl": "https://identity.example/jwks.json",
            "issuer": "https://identity.example",
            "audiences": ["router"]
        },
        "subgraphs": [{
            "name": "products",
            "graphqlUrl": "http://127.0.0.1:4100/graphql",
            "sdlUrl": "http://127.0.0.1:4100/sdl",
            "protocolUrl": "http://127.0.0.1:4100/.well-known/graphql-router",
            "schemaHeadersFromEnv": {"authorization": "PRODUCTS_SCHEMA_TOKEN"}
        }],
        "publicRequestTimeoutMs": 45000,
        "subgraphRequestTimeoutMs": 5000,
        "maxSubgraphConnectionsPerHost": 75,
        "requestLimits": {"maxDepth": 12},
        "subscriptions": {"maxConnectionAttemptsPerSecond": 55},
        "telemetry": {"logLevel": "warn", "prometheus": {"port": 4900}}
    }"#;

    #[test]
    fn strict_file_configuration_resolves_only_named_environment_values() {
        let file = RouterFileConfig::from_json(FILE).unwrap();
        let debug = format!("{file:?}");
        assert!(!debug.contains("schema-secret"));
        let config = file
            .into_router_config_with(|name| {
                Ok(match name {
                    "PRODUCTS_SCHEMA_TOKEN" => Some("Bearer schema-secret".to_owned()),
                    PUBLIC_LISTENER_ENV => Some("127.0.0.1:4200".to_owned()),
                    _ => None,
                })
            })
            .unwrap();
        assert_eq!(config.listener, "127.0.0.1:4200".parse().unwrap());
        assert_eq!(config.request_limits.max_depth, 12);
        assert_eq!(config.public_request_timeout, Duration::from_secs(45));
        assert_eq!(config.subgraph_request_timeout, Duration::from_secs(5));
        assert_eq!(config.max_subgraph_connections_per_host, 75);
        assert_eq!(
            config
                .subscriptions
                .as_ref()
                .unwrap()
                .max_connection_attempts_per_second,
            55
        );
        assert_eq!(config.telemetry.prometheus_port, Some(4900));
        assert_eq!(
            config.subgraphs[0].schema_headers["authorization"],
            "Bearer schema-secret"
        );
        assert!(
            config
                .scope_matcher
                .matches("products.read", "products.read")
        );
        assert!(!config.scope_matcher.matches("products.*", "products.read"));
        assert!(!format!("{config:?}").contains("schema-secret"));
    }

    #[test]
    fn file_configuration_rejects_unknown_fields_and_missing_secrets() {
        assert!(RouterFileConfig::from_json(r#"{"listener":"127.0.0.1:1","oops":1}"#).is_err());
        assert!(
            RouterFileConfig::from_json(FILE)
                .unwrap()
                .into_router_config_with(|_| Ok(None))
                .is_err()
        );
    }

    #[cfg(feature = "auth-agql")]
    #[test]
    fn hierarchical_file_matcher_applies_super_wildcard_and_exact_only_matrix() {
        let json = FILE.replacen(
            "\"authentication\":",
            r#""scopeMatcher": {
                "kind": "hierarchical",
                "superScopes": ["platform.admin"],
                "exactOnlyScopes": ["payments.credentials.release"],
                "exactOnlyScopePatterns": ["payments.account.*.credentials.release"]
            },
            "authentication":"#,
            1,
        );
        let config = RouterFileConfig::from_json(&json)
            .unwrap()
            .into_router_config_with(|name| {
                Ok((name == "PRODUCTS_SCHEMA_TOKEN").then(|| "Bearer secret".to_owned()))
            })
            .unwrap();
        let matcher = config.scope_matcher;

        for (granted, required, expected) in [
            ("platform.admin", "orders.read", true),
            ("orders.*", "orders.read", true),
            ("orders.read", "orders.read", true),
            ("platform.admin", "payments.credentials.release", false),
            ("payments.*", "payments.credentials.release", false),
            (
                "payments.credentials.release",
                "payments.credentials.release",
                true,
            ),
            (
                "platform.admin",
                "payments.account.7.credentials.release",
                false,
            ),
            (
                "payments.account.*",
                "payments.account.7.credentials.release",
                false,
            ),
            (
                "payments.account.7.credentials.release",
                "payments.account.7.credentials.release",
                true,
            ),
        ] {
            assert_eq!(
                matcher.matches(granted, required),
                expected,
                "grant {granted:?} for requirement {required:?}"
            );
        }
    }

    #[cfg(not(feature = "auth-agql"))]
    #[test]
    fn hierarchical_file_matcher_requires_auth_agql_feature() {
        let json = FILE.replacen(
            "\"authentication\":",
            r#""scopeMatcher": {"kind": "hierarchical"},
            "authentication":"#,
            1,
        );
        let error = RouterFileConfig::from_json(&json)
            .unwrap()
            .into_router_config_with(|name| {
                Ok((name == "PRODUCTS_SCHEMA_TOKEN").then(|| "Bearer secret".to_owned()))
            })
            .unwrap_err();
        assert!(error.to_string().contains("auth-agql"));
    }

    #[test]
    fn exact_file_matcher_rejects_hierarchical_options() {
        let json = FILE.replacen(
            "\"authentication\":",
            r#""scopeMatcher": {"kind": "exact", "superScopes": ["platform.admin"]},
            "authentication":"#,
            1,
        );
        let error = RouterFileConfig::from_json(&json)
            .unwrap()
            .into_router_config_with(|name| {
                Ok((name == "PRODUCTS_SCHEMA_TOKEN").then(|| "Bearer secret".to_owned()))
            })
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("kind `exact` does not accept hierarchical options")
        );
    }

    #[cfg(feature = "auth-agql")]
    #[test]
    fn signed_role_scope_catalogue_file_configuration_is_strict_and_opt_in() {
        let json = FILE.replacen(
            r#""audiences": ["router"]"#,
            r#""audiences": ["router"],
            "refreshIntervalSeconds": 30,
            "roleScopeCatalogue": {
                "urlFromEnv": "ROLE_SCOPE_URL",
                "audience": "resource-servers",
                "cacheTtlSeconds": 120,
                "maximumSignedLifetimeSeconds": 86400,
                "clockSkewLeewaySeconds": 30,
                "retryBackoffSeconds": 2,
                "maximumRetryBackoffSeconds": 30,
                "maxBodyBytes": 262144,
                "requestHeadersFromEnv": {"authorization": "ROLE_SCOPE_TOKEN"}
            }"#,
            1,
        );
        RouterFileConfig::from_json(&json)
            .unwrap()
            .into_router_config_with(|name| {
                Ok(match name {
                    "PRODUCTS_SCHEMA_TOKEN" => Some("Bearer schema-secret".to_owned()),
                    "ROLE_SCOPE_URL" => Some("https://identity.example/role-scopes".to_owned()),
                    "ROLE_SCOPE_TOKEN" => Some("Bearer catalogue-secret".to_owned()),
                    _ => None,
                })
            })
            .unwrap();

        let missing = RouterFileConfig::from_json(&json)
            .unwrap()
            .into_router_config_with(|name| {
                Ok((name == "PRODUCTS_SCHEMA_TOKEN").then(|| "Bearer schema-secret".to_owned()))
            })
            .unwrap_err();
        assert!(missing.to_string().contains("ROLE_SCOPE_URL"));

        let unknown = json.replacen(
            r#""maxBodyBytes": 262144"#,
            r#""maxBodyBytes": 262144, "consumerPolicy": true"#,
            1,
        );
        assert!(RouterFileConfig::from_json(&unknown).is_err());
    }

    #[cfg(not(feature = "auth-agql"))]
    #[test]
    fn signed_role_scope_catalogue_requires_auth_agql_feature() {
        let json = FILE.replacen(
            r#""audiences": ["router"]"#,
            r#""audiences": ["router"],
            "roleScopeCatalogue": {
                "url": "https://identity.example/role-scopes",
                "audience": "resource-servers"
            }"#,
            1,
        );
        let error = RouterFileConfig::from_json(&json)
            .unwrap()
            .into_router_config_with(|name| {
                Ok((name == "PRODUCTS_SCHEMA_TOKEN").then(|| "Bearer secret".to_owned()))
            })
            .unwrap_err();
        assert!(error.to_string().contains("auth-agql"));
    }

    #[cfg(feature = "auth-agql")]
    #[test]
    fn hierarchical_file_matcher_rejects_bare_wildcard_exact_only_pattern() {
        let json = FILE.replacen(
            "\"authentication\":",
            r#""scopeMatcher": {
                "kind": "hierarchical",
                "allowUniversalWildcard": true,
                "exactOnlyScopePatterns": ["*"]
            },
            "authentication":"#,
            1,
        );
        let error = RouterFileConfig::from_json(&json)
            .unwrap()
            .into_router_config_with(|name| {
                Ok((name == "PRODUCTS_SCHEMA_TOKEN").then(|| "Bearer secret".to_owned()))
            })
            .unwrap_err();
        assert!(error.to_string().contains("must not be the bare wildcard"));
    }
}
