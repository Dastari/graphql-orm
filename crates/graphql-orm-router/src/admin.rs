use std::{collections::BTreeMap, sync::Arc, time::UNIX_EPOCH};

use futures::StreamExt;
use graphql_orm_router_protocol::SubgraphDescriptor;
use reqwest::header::ETAG;
use serde::Deserialize;
use serde_json::json;
use url::Url;

use crate::{
    AdminConfig, AuthenticatedPrincipal, AuthenticationErrorKind, AuthenticationProvider,
    RouterError, RouterErrorKind, ScopeMatcher, StaticSubgraph, TrustedSubgraph,
    lifecycle::{FetchedSubgraph, GraphLifecycle},
    network::{NetworkPolicy, ResolvedUrl},
    server::strict_bearer_token,
};

const MAX_DESCRIPTOR_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct AdminRuntime {
    lifecycle: Arc<GraphLifecycle>,
    authentication: Arc<dyn AuthenticationProvider>,
    scope_matcher: Arc<dyn ScopeMatcher>,
    config: AdminConfig,
    schema_fetch_timeout: std::time::Duration,
    max_sdl_bytes: usize,
    graceful_shutdown_timeout_seconds: u16,
}

impl AdminRuntime {
    pub(crate) fn new(
        lifecycle: Arc<GraphLifecycle>,
        authentication: Arc<dyn AuthenticationProvider>,
        scope_matcher: Arc<dyn ScopeMatcher>,
        config: AdminConfig,
        schema_fetch_timeout: std::time::Duration,
        max_sdl_bytes: usize,
        graceful_shutdown_timeout_seconds: u16,
    ) -> Self {
        Self {
            lifecycle,
            authentication,
            scope_matcher,
            config,
            schema_fetch_timeout,
            max_sdl_bytes,
            graceful_shutdown_timeout_seconds,
        }
    }

    pub(crate) fn listener(&self) -> std::net::SocketAddr {
        self.config.listener
    }

    pub(crate) fn max_request_body_bytes(&self) -> usize {
        self.config.max_request_body_bytes
    }

    fn authenticate(
        &self,
        request: &hive_router::ntex::web::HttpRequest,
        required_scope: &str,
    ) -> Result<AuthenticatedPrincipal, hive_router::ntex::web::HttpResponse> {
        let Some(value) = request
            .headers()
            .get(hive_router::ntex::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
        else {
            return Err(admin_error(
                hive_router::ntex::http::StatusCode::UNAUTHORIZED,
                "UNAUTHENTICATED",
                "valid administrative authentication is required",
            ));
        };
        let Some(token) = strict_bearer_token(value) else {
            return Err(admin_error(
                hive_router::ntex::http::StatusCode::UNAUTHORIZED,
                "UNAUTHENTICATED",
                "valid administrative authentication is required",
            ));
        };
        let principal = self
            .authentication
            .authenticate_bearer(token)
            .map_err(|error| {
                let unavailable = error.kind() == AuthenticationErrorKind::Unavailable;
                admin_error(
                    if unavailable {
                        hive_router::ntex::http::StatusCode::SERVICE_UNAVAILABLE
                    } else {
                        hive_router::ntex::http::StatusCode::UNAUTHORIZED
                    },
                    "UNAUTHENTICATED",
                    if unavailable {
                        "administrative authentication is unavailable"
                    } else {
                        "valid administrative authentication is required"
                    },
                )
            })?;
        if !principal
            .scopes()
            .iter()
            .any(|granted| self.scope_matcher.matches(granted, required_scope))
        {
            return Err(admin_error(
                hive_router::ntex::http::StatusCode::FORBIDDEN,
                "FORBIDDEN",
                "administrative scope is missing",
            ));
        }
        Ok(principal)
    }

    async fn register(
        &self,
        principal: &AuthenticatedPrincipal,
        request: RegistrationRequest,
    ) -> Result<crate::ActiveGraphIdentity, RouterError> {
        let trusted = self
            .config
            .trusted_subgraphs
            .iter()
            .find(|trusted| trusted.service_subject == principal.subject())
            .ok_or_else(|| {
                RouterError::new(
                    RouterErrorKind::Registration,
                    "authenticated service identity is not trusted for registration",
                )
            })?;
        if request.name != trusted.name || !same_url(&request.metadata_url, &trusted.metadata_url) {
            return Err(RouterError::new(
                RouterErrorKind::Registration,
                "registration name or metadata destination does not match service identity",
            ));
        }
        self.lifecycle
            .record_registration(&trusted.name, &trusted.subgraph_id)
            .await;
        self.lifecycle
            .record_registration_candidate(&trusted.name, &trusted.subgraph_id)
            .await;
        let result = async {
            let metadata_target = self
                .config
                .network_policy
                .resolve_url(&request.metadata_url, "metadata endpoint")
                .await?;
            let metadata_client = self
                .config
                .network_policy
                .pinned_client(&[&metadata_target], self.schema_fetch_timeout)?;
            let (descriptor_bytes, descriptor_etag) = fetch_resource(
                &metadata_client,
                &self.config.network_policy,
                &metadata_target,
                &trusted.schema_headers,
                "application/json",
                MAX_DESCRIPTOR_BYTES,
                "metadata endpoint",
            )
            .await?;
            let descriptor_json = std::str::from_utf8(&descriptor_bytes).map_err(|_| {
                RouterError::new(
                    RouterErrorKind::Registration,
                    "router descriptor is not UTF-8",
                )
            })?;
            let descriptor =
                SubgraphDescriptor::from_json_compatible(descriptor_json).map_err(|_| {
                    RouterError::new(
                        RouterErrorKind::Registration,
                        "router descriptor is incompatible or invalid",
                    )
                })?;
            validate_identity(&descriptor, trusted)?;

            let graphql_target = self
                .config
                .network_policy
                .resolve_url(descriptor.graphql.http.as_str(), "GraphQL endpoint")
                .await?;
            require_ip_literal(&graphql_target.url, "GraphQL endpoint")?;
            require_origin(
                &graphql_target.url,
                &trusted.graphql_origin,
                "GraphQL endpoint",
            )?;
            let schema_target = self
                .config
                .network_policy
                .resolve_url(descriptor.schema.url.as_str(), "SDL endpoint")
                .await?;
            require_origin(&schema_target.url, &trusted.schema_origin, "SDL endpoint")?;
            let subscription_path = match descriptor.graphql.websocket.as_ref() {
                Some(endpoint) => Some(
                    validate_websocket(endpoint.as_str(), trusted, &self.config.network_policy)
                        .await?,
                ),
                None => None,
            };

            let client = self.config.network_policy.pinned_client(
                &[&metadata_target, &schema_target, &graphql_target],
                self.schema_fetch_timeout,
            )?;
            let (sdl_bytes, sdl_etag) = fetch_resource(
                &client,
                &self.config.network_policy,
                &schema_target,
                &trusted.schema_headers,
                "application/graphql, text/plain;q=0.9",
                self.max_sdl_bytes,
                "SDL endpoint",
            )
            .await?;
            let sdl = String::from_utf8(sdl_bytes).map_err(|_| {
                RouterError::new(RouterErrorKind::SchemaFetch, "dynamic SDL is not UTF-8")
            })?;
            let mut config = StaticSubgraph::new(
                trusted.name.clone(),
                graphql_target.as_str(),
                schema_target.as_str(),
            )
            .with_protocol_url(metadata_target.as_str());
            for (name, value) in &trusted.schema_headers {
                config = config.with_schema_header(name, value);
            }
            if let Some(path) = subscription_path {
                config = config.with_subscription_websocket_path(path);
            }
            let fetched = FetchedSubgraph::from_dynamic_parts(
                &config,
                sdl,
                descriptor,
                sdl_etag,
                descriptor_etag,
            )?;
            self.lifecycle
                .register_dynamic(config, fetched, client, self.config.network_policy.clone())
                .await
        }
        .await;
        if let Err(error) = &result {
            self.lifecycle
                .record_registration_rejected(&trusted.name, &trusted.subgraph_id, error)
                .await;
        }
        result
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegistrationRequest {
    name: String,
    metadata_url: String,
}

pub(crate) fn build_admin_server(
    runtime: AdminRuntime,
) -> Result<hive_router::ntex::server::Server, RouterError> {
    use hive_router::ntex::web;

    let listener = runtime.listener();
    let body_limit = runtime.max_request_body_bytes();
    let shutdown_timeout = runtime.graceful_shutdown_timeout_seconds;
    let server = web::HttpServer::new(async move || {
        let status_runtime = runtime.clone();
        let refresh_runtime = runtime.clone();
        let registration_runtime = runtime.clone();
        let removal_runtime = runtime.clone();
        let metrics_runtime = runtime.clone();
        web::App::new()
            .state(web::types::JsonConfig::default().limit(body_limit))
            .service(web::resource("/_router/status").route(
                web::get().to(move |request| status_handler(request, status_runtime.clone())),
            ))
            .service(web::resource("/_router/refresh").route(
                web::post().to(move |request| refresh_handler(request, refresh_runtime.clone())),
            ))
            .service(web::resource("/_router/metrics").route(
                web::get().to(move |request| metrics_handler(request, metrics_runtime.clone())),
            ))
            .service(web::resource("/_router/subgraphs").route(web::post().to(
                move |request, body| {
                    registration_handler(request, body, registration_runtime.clone())
                },
            )))
            .service(web::resource("/_router/subgraphs/{name}").route(
                web::delete().to(move |request| removal_handler(request, removal_runtime.clone())),
            ))
    })
    .shutdown_timeout(hive_router::ntex::time::Seconds(shutdown_timeout));
    #[cfg(target_family = "unix")]
    let server = server.disable_signals();
    server
        .bind(listener)
        .map(|server| server.run())
        .map_err(|_| {
            RouterError::new(
                RouterErrorKind::Server,
                format!("failed to bind administrative listener {listener}"),
            )
        })
}

async fn status_handler(
    request: hive_router::ntex::web::HttpRequest,
    runtime: AdminRuntime,
) -> hive_router::ntex::web::HttpResponse {
    if let Err(response) = runtime.authenticate(&request, &runtime.config.status_scope) {
        return response;
    }
    status_response(runtime.lifecycle.status().await)
}

async fn refresh_handler(
    request: hive_router::ntex::web::HttpRequest,
    runtime: AdminRuntime,
) -> hive_router::ntex::web::HttpResponse {
    if let Err(response) = runtime.authenticate(&request, &runtime.config.refresh_scope) {
        return response;
    }
    match runtime.lifecycle.refresh().await {
        Ok(outcome) => hive_router::ntex::web::HttpResponse::Ok().json(&json!({
            "outcome": format!("{outcome:?}").to_ascii_lowercase(),
        })),
        Err(_) => admin_error(
            hive_router::ntex::http::StatusCode::SERVICE_UNAVAILABLE,
            "REFRESH_FAILED",
            "schema refresh failed; the active graph was retained",
        ),
    }
}

async fn metrics_handler(
    request: hive_router::ntex::web::HttpRequest,
    runtime: AdminRuntime,
) -> hive_router::ntex::web::HttpResponse {
    if let Err(response) = runtime.authenticate(&request, &runtime.config.metrics_scope) {
        return response;
    }
    let metrics = runtime.lifecycle.metrics().snapshot();
    let status = runtime.lifecycle.status().await;
    let health = status
        .subgraphs()
        .iter()
        .map(|subgraph| {
            (
                subgraph.name().to_owned(),
                if subgraph.is_active() { 1_u8 } else { 0_u8 },
            )
        })
        .collect::<BTreeMap<_, _>>();
    hive_router::ntex::web::HttpResponse::Ok().json(&json!({
        "router_graphql_requests_total": metrics.graphql_requests_total(),
        "router_graphql_errors_total": metrics.graphql_errors_total(),
        "router_subgraph_requests_total": metrics.subgraph_requests_total(),
        "router_subgraph_errors_total": metrics.subgraph_errors_total(),
        "router_subgraph_latency_microseconds_total": metrics.subgraph_latency_microseconds_total(),
        "router_active_graph_version": metrics.active_graph_version(),
        "router_websocket_connections": metrics.active_websocket_connections(),
        "router_active_subscriptions": metrics.active_subscriptions(),
        "router_schema_refresh_total": metrics.schema_refresh_total(),
        "router_composition_success_total": metrics.composition_success_total(),
        "router_composition_failure_total": metrics.composition_failure_total(),
        "router_rejected_subgraphs_total": metrics.rejected_subgraphs_total(),
        "router_authorization_denied_total": metrics.authorization_denied_total(),
        "router_subgraph_health": health,
    }))
}

async fn registration_handler(
    request: hive_router::ntex::web::HttpRequest,
    body: hive_router::ntex::web::types::Json<RegistrationRequest>,
    runtime: AdminRuntime,
) -> hive_router::ntex::web::HttpResponse {
    let principal = match runtime.authenticate(&request, &runtime.config.registration_scope) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    match runtime.register(&principal, body.into_inner()).await {
        Ok(identity) => hive_router::ntex::web::HttpResponse::Created().json(&json!({
            "state": "active",
            "graph": {
                "version": identity.version(),
                "fingerprint": identity.fingerprint(),
            }
        })),
        Err(error) => admin_error(
            registration_status(error.kind()),
            "REGISTRATION_REJECTED",
            "dynamic registration was rejected; the active graph was retained",
        ),
    }
}

async fn removal_handler(
    request: hive_router::ntex::web::HttpRequest,
    runtime: AdminRuntime,
) -> hive_router::ntex::web::HttpResponse {
    if let Err(response) = runtime.authenticate(&request, &runtime.config.removal_scope) {
        return response;
    }
    let name = request.match_info().get("name").unwrap_or_default();
    match runtime.lifecycle.remove_subgraph(name).await {
        Ok(identity) => hive_router::ntex::web::HttpResponse::Ok().json(&json!({
            "state": "disabled",
            "graph": {
                "version": identity.version(),
                "fingerprint": identity.fingerprint(),
            }
        })),
        Err(_) => admin_error(
            hive_router::ntex::http::StatusCode::CONFLICT,
            "REMOVAL_REJECTED",
            "subgraph removal was rejected; the active graph was retained",
        ),
    }
}

fn status_response(status: crate::RouterStatus) -> hive_router::ntex::web::HttpResponse {
    let subgraphs = status
        .subgraphs()
        .iter()
        .map(|subgraph| {
            json!({
                "id": subgraph.id(),
                "name": subgraph.name(),
                "source": format!("{:?}", subgraph.source_kind()).to_ascii_lowercase(),
                "state": format!("{:?}", subgraph.state()).to_ascii_lowercase(),
                "active": subgraph.is_active(),
                "activeFingerprint": subgraph.active_fingerprint(),
                "observedFingerprint": subgraph.observed_fingerprint(),
                "lastError": subgraph.last_error(),
                "lastSuccessfulRefreshUnixSeconds": unix_seconds(subgraph.last_successful_refresh()),
            })
        })
        .collect::<Vec<_>>();
    hive_router::ntex::web::HttpResponse::Ok().json(&json!({
        "activeGraph": {
            "version": status.active_graph().version(),
            "fingerprint": status.active_graph().fingerprint(),
        },
        "lastSuccessfulCompositionUnixSeconds": unix_seconds(Some(status.last_successful_composition())),
        "lastCompositionError": status.last_composition_error(),
        "subgraphs": subgraphs,
        "persistence": "process-local",
    }))
}

fn unix_seconds(value: Option<std::time::SystemTime>) -> Option<u64> {
    value.and_then(|value| {
        value
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|value| value.as_secs())
    })
}

fn admin_error(
    status: hive_router::ntex::http::StatusCode,
    code: &str,
    message: &str,
) -> hive_router::ntex::web::HttpResponse {
    hive_router::ntex::web::HttpResponse::build(status).json(&json!({
        "error": {"code": code, "message": message}
    }))
}

fn registration_status(kind: RouterErrorKind) -> hive_router::ntex::http::StatusCode {
    match kind {
        RouterErrorKind::NetworkPolicy | RouterErrorKind::Registration => {
            hive_router::ntex::http::StatusCode::FORBIDDEN
        }
        RouterErrorKind::Composition
        | RouterErrorKind::Runtime
        | RouterErrorKind::AuthorizationMetadata => hive_router::ntex::http::StatusCode::CONFLICT,
        _ => hive_router::ntex::http::StatusCode::BAD_GATEWAY,
    }
}

async fn fetch_resource(
    client: &reqwest::Client,
    policy: &NetworkPolicy,
    target: &ResolvedUrl,
    headers: &BTreeMap<String, String>,
    accept: &str,
    maximum: usize,
    field: &str,
) -> Result<(Vec<u8>, Option<String>), RouterError> {
    let mut request = client
        .get(target.as_str())
        .header("accept", accept)
        .header("user-agent", "graphql-orm-router/0.1");
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let response = request.send().await.map_err(|_| {
        RouterError::new(
            RouterErrorKind::Registration,
            format!("failed to fetch {field}"),
        )
    })?;
    policy.validate_peer(&response, target, field)?;
    if !response.status().is_success() {
        return Err(RouterError::new(
            RouterErrorKind::Registration,
            format!("{field} returned a non-success status"),
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(RouterError::new(
            RouterErrorKind::Registration,
            format!("{field} exceeds its configured size limit"),
        ));
    }
    let etag = response
        .headers()
        .get(ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| {
            RouterError::new(
                RouterErrorKind::Registration,
                format!("failed while reading {field}"),
            )
        })?;
        if bytes.len().saturating_add(chunk.len()) > maximum {
            return Err(RouterError::new(
                RouterErrorKind::Registration,
                format!("{field} exceeds its configured size limit"),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((bytes, etag))
}

fn validate_identity(
    descriptor: &SubgraphDescriptor,
    trusted: &TrustedSubgraph,
) -> Result<(), RouterError> {
    if descriptor.subgraph.id.as_str() != trusted.subgraph_id
        || descriptor.subgraph.name.as_str() != trusted.name
    {
        return Err(RouterError::new(
            RouterErrorKind::Registration,
            "descriptor identity does not match the authenticated service binding",
        ));
    }
    Ok(())
}

fn same_url(left: &str, right: &str) -> bool {
    Url::parse(left).ok() == Url::parse(right).ok()
}

fn require_origin(value: &Url, expected: &str, field: &str) -> Result<(), RouterError> {
    let expected = Url::parse(expected).map_err(|_| {
        RouterError::new(
            RouterErrorKind::InvalidConfiguration,
            "trusted origin is invalid",
        )
    })?;
    if value.scheme() != expected.scheme()
        || value.host_str().map(str::to_ascii_lowercase)
            != expected.host_str().map(str::to_ascii_lowercase)
        || value.port_or_known_default() != expected.port_or_known_default()
    {
        return Err(RouterError::new(
            RouterErrorKind::NetworkPolicy,
            format!("advertised {field} does not match its trusted origin"),
        ));
    }
    Ok(())
}

fn require_ip_literal(value: &Url, field: &str) -> Result<(), RouterError> {
    if matches!(value.host(), Some(url::Host::Ipv4(_) | url::Host::Ipv6(_))) {
        return Ok(());
    }
    Err(RouterError::new(
        RouterErrorKind::NetworkPolicy,
        format!(
            "dynamic {field} must use an IP-literal host so the execution transport cannot re-resolve DNS after policy validation"
        ),
    ))
}

async fn validate_websocket(
    value: &str,
    trusted: &TrustedSubgraph,
    policy: &NetworkPolicy,
) -> Result<String, RouterError> {
    let mut url = Url::parse(value).map_err(|_| {
        RouterError::new(
            RouterErrorKind::NetworkPolicy,
            "advertised WebSocket endpoint is invalid",
        )
    })?;
    let replacement = match url.scheme() {
        "ws" => "http",
        "wss" => "https",
        _ => {
            return Err(RouterError::new(
                RouterErrorKind::NetworkPolicy,
                "advertised WebSocket endpoint must use ws or wss",
            ));
        }
    };
    url.set_scheme(replacement).map_err(|_| {
        RouterError::new(
            RouterErrorKind::NetworkPolicy,
            "advertised WebSocket endpoint is invalid",
        )
    })?;
    let target = policy
        .resolve_url(url.as_str(), "WebSocket endpoint")
        .await?;
    require_origin(&target.url, &trusted.graphql_origin, "WebSocket endpoint")?;
    Ok(target.url.path().to_owned())
}
