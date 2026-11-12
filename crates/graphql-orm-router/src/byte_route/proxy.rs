//! Streaming byte-route proxy on the public listener.
//!
//! The middleware sits outside Hive's plugin chain. It claims only paths at or
//! below a configured prefix and leaves every other request untouched.

use std::{
    rc::Rc,
    sync::Arc,
    task::{Context, Poll},
    time::{Duration, Instant},
};

use futures::{SinkExt as _, StreamExt as _, channel::mpsc};
use hive_router::ntex::{
    self, Middleware, SharedCfg,
    http::{
        StatusCode,
        body::{Body, BodySize, MessageBody},
        header::{self as ntex_header, HeaderValue as NtexHeaderValue},
    },
    io::IoRef,
    service::{Service, ServiceCtx},
    util::Bytes,
    web::{self, DefaultError, HttpResponse},
};
use hive_router::tokio::time::timeout;
use reqwest::{
    Client,
    header::{HeaderName, HeaderValue},
    redirect::Policy,
};
use serde_json::json;
use url::Url;

use super::{ByteRoute, PathMatch, match_path, safe_query};
use crate::{
    AuthenticationErrorKind, AuthenticationProvider, RouterError, RouterErrorKind,
    metrics::{ByteRouteCounters, RouterMetrics},
    server::strict_bearer_token,
    subscriptions::ConnectionAttemptLimiter,
};

/// Header naming the router-generated error code, which an upstream can never
/// set because response headers cross only through an allow-list.
pub(crate) const ROUTER_ERROR_HEADER: &str = "graphql-orm-router-error";

const BASE_REQUEST_HEADERS: &[&str] = &[
    "accept",
    "accept-encoding",
    "accept-language",
    "if-match",
    "if-none-match",
    "if-modified-since",
    "if-unmodified-since",
];
const RANGE_REQUEST_HEADERS: &[&str] = &["range", "if-range"];
const BASE_RESPONSE_HEADERS: &[&str] = &[
    "content-type",
    "content-disposition",
    "content-encoding",
    "content-language",
    "cache-control",
    "expires",
    "etag",
    "last-modified",
    "vary",
    "age",
    "retry-after",
];
const RANGE_RESPONSE_HEADERS: &[&str] = &["content-range", "accept-ranges"];
const SANDBOX_POLICY: &str = "default-src 'none'; sandbox";
/// Chunks in flight between the upstream reader and the client writer.
const STREAM_BUFFER_CHUNKS: usize = 1;

pub(crate) struct ByteRouteRuntime {
    prefixes: Vec<String>,
    routes: Vec<CompiledRoute>,
    authentication: Option<Arc<dyn AuthenticationProvider>>,
}

struct CompiledRoute {
    config: ByteRoute,
    upstream: Url,
    client: Client,
    limiter: ConnectionAttemptLimiter,
    counters: Arc<ByteRouteCounters>,
    request_headers: Vec<HeaderName>,
    response_headers: Vec<HeaderName>,
}

impl ByteRouteRuntime {
    /// Builds the runtime for validated routes, or `None` when none are
    /// configured so the public listener is unchanged.
    pub(crate) fn new(
        routes: &[ByteRoute],
        authentication: Option<Arc<dyn AuthenticationProvider>>,
        metrics: &RouterMetrics,
    ) -> Result<Option<Arc<Self>>, RouterError> {
        if routes.is_empty() {
            return Ok(None);
        }
        let prefixes = routes
            .iter()
            .map(|route| route.prefix.clone())
            .collect::<Vec<_>>();
        let counters = metrics.install_byte_routes(&prefixes);
        let routes = routes
            .iter()
            .zip(counters)
            .map(|(route, counters)| CompiledRoute::new(route, counters))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(Arc::new(Self {
            prefixes,
            routes,
            authentication,
        })))
    }

    async fn handle(
        &self,
        index: usize,
        remainder: &str,
        request: &web::WebRequest<DefaultError>,
    ) -> HttpResponse {
        let started = Instant::now();
        let route = &self.routes[index];
        route.counters.request();
        let method = request.method();
        let head = if *method == ntex::http::Method::HEAD {
            true
        } else if *method == ntex::http::Method::GET {
            false
        } else {
            route.counters.rejected();
            let mut response = router_error(
                StatusCode::METHOD_NOT_ALLOWED,
                "BYTE_ROUTE_METHOD_NOT_ALLOWED",
                "byte routes accept only GET and HEAD",
            );
            response.headers_mut().insert(
                ntex_header::ALLOW,
                NtexHeaderValue::from_static("GET, HEAD"),
            );
            return response;
        };
        // HTTP/1.0 delimits an unknown-length body by closing the connection,
        // so an aborted transfer would look complete. Require HTTP/1.1 or later.
        if request.version() < ntex::http::Version::HTTP_11 {
            route.counters.rejected();
            return router_error(
                StatusCode::HTTP_VERSION_NOT_SUPPORTED,
                "BYTE_ROUTE_HTTP_VERSION_NOT_SUPPORTED",
                "byte routes require HTTP/1.1 or later",
            );
        }
        let headers = request.headers();
        let declares_body = headers.contains_key(ntex_header::TRANSFER_ENCODING)
            || headers
                .get_all(ntex_header::CONTENT_LENGTH)
                .any(|value| value.as_bytes() != b"0");
        if declares_body {
            route.counters.rejected();
            return router_error(
                StatusCode::BAD_REQUEST,
                "BYTE_ROUTE_REQUEST_BODY_NOT_ALLOWED",
                "byte route requests must not carry a body",
            );
        }
        let query = request.uri().query();
        if query.is_some_and(|query| !safe_query(query)) {
            route.counters.rejected();
            return invalid_path();
        }
        // Authenticate before taking a token, so anonymous traffic cannot use
        // up an authenticated route's shared admission budget.
        let authorization = match self.authenticate(route, headers) {
            Ok(authorization) => authorization,
            Err(response) => {
                route.counters.rejected();
                return response;
            }
        };
        if !route.limiter.try_acquire() {
            route.counters.rejected();
            let mut response = router_error(
                StatusCode::TOO_MANY_REQUESTS,
                "BYTE_ROUTE_RATE_LIMITED",
                "byte route request rate exceeded",
            );
            response
                .headers_mut()
                .insert(ntex_header::RETRY_AFTER, NtexHeaderValue::from_static("1"));
            return response;
        }
        if !route.config.allow_range && headers.contains_key(ntex_header::RANGE) {
            route.counters.rejected();
            let mut response = router_error(
                StatusCode::RANGE_NOT_SATISFIABLE,
                "BYTE_ROUTE_RANGE_NOT_ALLOWED",
                "byte route does not accept range requests",
            );
            response.headers_mut().insert(
                ntex_header::ACCEPT_RANGES,
                NtexHeaderValue::from_static("none"),
            );
            return response;
        }
        let Some(url) = route.upstream_url(remainder, query) else {
            route.counters.rejected();
            return invalid_path();
        };
        if !route
            .counters
            .try_acquire(route.config.max_concurrent_requests)
        {
            route.counters.rejected();
            return router_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "BYTE_ROUTE_BUSY",
                "byte route is at its concurrency limit",
            );
        }
        let slot = ConcurrencySlot(route.counters.clone());
        let deadline = started + route.config.max_duration;

        let mut upstream = route.client.request(
            if head {
                reqwest::Method::HEAD
            } else {
                reqwest::Method::GET
            },
            url,
        );
        for name in &route.request_headers {
            for value in headers.get_all(name) {
                if let Ok(value) = HeaderValue::from_bytes(value.as_bytes()) {
                    upstream = upstream.header(name.clone(), value);
                }
            }
        }
        if let Some(authorization) = authorization {
            upstream = upstream.header(reqwest::header::AUTHORIZATION, authorization);
        }
        // The connect timeout bounds connecting and the idle timeout bounds
        // the wait for headers after it, so a slow connect reports 502.
        let wait = (route.config.connect_timeout + route.config.idle_timeout)
            .min(deadline.saturating_duration_since(Instant::now()));
        let response = match timeout(wait, upstream.send()).await {
            Ok(Ok(response)) => response,
            Ok(Err(error)) if error.is_timeout() && !error.is_connect() => {
                route.counters.upstream_error();
                return upstream_timeout();
            }
            Ok(Err(_)) => {
                route.counters.upstream_error();
                return router_error(
                    StatusCode::BAD_GATEWAY,
                    "BYTE_ROUTE_UPSTREAM_UNAVAILABLE",
                    "byte route upstream is unreachable",
                );
            }
            Err(_) => {
                route.counters.upstream_error();
                return upstream_timeout();
            }
        };

        let declared_length = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        let chunked = response
            .headers()
            .get_all(reqwest::header::TRANSFER_ENCODING)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .any(|coding| coding.trim().eq_ignore_ascii_case("chunked"));
        let has_body = !head
            && response.status() != reqwest::StatusCode::NO_CONTENT
            && response.status() != reqwest::StatusCode::NOT_MODIFIED;
        // An HTTP/1 body ended only by the connection closing cannot be told
        // apart from a truncated one, so it is never relayed as complete.
        if has_body
            && declared_length.is_none()
            && !chunked
            && response.version() < reqwest::Version::HTTP_2
        {
            route.counters.upstream_error();
            return router_error(
                StatusCode::BAD_GATEWAY,
                "BYTE_ROUTE_UPSTREAM_UNAVAILABLE",
                "byte route upstream response has no length or chunked framing",
            );
        }
        if declared_length.is_some_and(|length| length > route.config.max_response_bytes) {
            route.counters.upstream_error();
            return router_error(
                StatusCode::BAD_GATEWAY,
                "BYTE_ROUTE_RESPONSE_TOO_LARGE",
                "byte route upstream response exceeds the configured size limit",
            );
        }
        // An interim or upgrade status has no place in a download; relaying
        // 101 would also ask the server to treat the connection as upgraded.
        let Some(status) = StatusCode::from_u16(response.status().as_u16())
            .ok()
            .filter(|status| !status.is_informational())
        else {
            route.counters.upstream_error();
            return router_error(
                StatusCode::BAD_GATEWAY,
                "BYTE_ROUTE_UPSTREAM_UNAVAILABLE",
                "byte route upstream sent an invalid response",
            );
        };
        let mut builder = HttpResponse::build(status);
        for name in &route.response_headers {
            for value in response.headers().get_all(name) {
                if let Ok(value) = NtexHeaderValue::from_bytes(value.as_bytes()) {
                    builder.header(name.clone(), value);
                }
            }
        }
        let mut client_response = builder.finish();
        secure_response_headers(&mut client_response, route);

        let bodiless =
            head || status == StatusCode::NO_CONTENT || status == StatusCode::NOT_MODIFIED;
        if bodiless {
            drop(slot);
            let size = match (head, declared_length) {
                (true, Some(length)) => BodySize::Sized(length),
                _ => BodySize::None,
            };
            return client_response.set_body(Body::Message(Box::new(EmptyBody(size))));
        }
        // HTTP/1 has no way to signal a failed body except closing the
        // connection. Terminate it directly, so the dispatcher never writes an
        // error response into a stream whose head has already been sent. An
        // HTTP/2 stream is reset through the body error instead.
        let connection = (request.version() < ntex::http::Version::HTTP_2)
            .then(|| request.io().cloned())
            .flatten();
        let (sender, receiver) = mpsc::channel(STREAM_BUFFER_CHUNKS);
        ntex::rt::spawn(pump(
            response,
            sender,
            connection.clone(),
            PumpLimits {
                max_bytes: route.config.max_response_bytes,
                idle: route.config.idle_timeout,
                deadline,
            },
            route.counters.clone(),
            slot,
        ));
        client_response.set_body(Body::Message(Box::new(StreamedBody {
            size: declared_length.map_or(BodySize::Stream, BodySize::Sized),
            receiver,
            counters: route.counters.clone(),
            connection,
            finished: false,
        })))
    }

    /// Applies the GraphQL endpoint's bearer rules. Returns the verified
    /// `Authorization` value to forward, if any.
    fn authenticate(
        &self,
        route: &CompiledRoute,
        headers: &ntex::http::HeaderMap,
    ) -> Result<Option<HeaderValue>, HttpResponse> {
        let mut supplied = headers.get_all(ntex_header::AUTHORIZATION);
        let Some(value) = supplied.next() else {
            return if route.config.public {
                Ok(None)
            } else {
                Err(unauthenticated(
                    StatusCode::UNAUTHORIZED,
                    "authentication is required",
                ))
            };
        };
        let invalid = || unauthenticated(StatusCode::UNAUTHORIZED, "invalid bearer credential");
        if supplied.next().is_some() {
            return Err(invalid());
        }
        let Some(provider) = &self.authentication else {
            // Only public routes exist without a provider. Never forward a
            // credential the router cannot verify.
            return Err(invalid());
        };
        let Some(token) = value.to_str().ok().and_then(strict_bearer_token) else {
            return Err(invalid());
        };
        match provider.authenticate_bearer(token) {
            Ok(_) => HeaderValue::from_bytes(value.as_bytes())
                .map(Some)
                .map_err(|_| invalid()),
            Err(error) if error.kind() == AuthenticationErrorKind::Unavailable => {
                Err(unauthenticated(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "authentication service unavailable",
                ))
            }
            Err(_) => Err(invalid()),
        }
    }
}

impl CompiledRoute {
    fn new(config: &ByteRoute, counters: Arc<ByteRouteCounters>) -> Result<Self, RouterError> {
        let upstream = Url::parse(&config.upstream).map_err(|_| {
            RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                format!("byte route `{}` upstream is not a valid URL", config.prefix),
            )
        })?;
        let client = Client::builder()
            .redirect(Policy::none())
            .no_proxy()
            .connect_timeout(config.connect_timeout)
            .read_timeout(config.idle_timeout)
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(config.max_concurrent_requests)
            .tcp_nodelay(true)
            .build()
            .map_err(|error| {
                RouterError::new(
                    RouterErrorKind::InvalidConfiguration,
                    format!(
                        "failed to construct the byte route `{}` client: {error}",
                        config.prefix
                    ),
                )
            })?;
        let header_names =
            |base: &[&str], range: &[&str], extra: &std::collections::BTreeSet<String>| {
                base.iter()
                    .copied()
                    .chain(
                        config
                            .allow_range
                            .then_some(range)
                            .into_iter()
                            .flatten()
                            .copied(),
                    )
                    .map(str::to_owned)
                    .chain(extra.iter().cloned())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .filter_map(|name| HeaderName::from_bytes(name.as_bytes()).ok())
                    .collect::<Vec<_>>()
            };
        let request_headers = header_names(
            BASE_REQUEST_HEADERS,
            RANGE_REQUEST_HEADERS,
            &config.forward_request_headers,
        );
        let mut response_headers = header_names(
            BASE_RESPONSE_HEADERS,
            RANGE_RESPONSE_HEADERS,
            &config.pass_response_headers,
        );
        if config.allow_set_cookie {
            response_headers.push(reqwest::header::SET_COOKIE);
        }
        Ok(Self {
            config: config.clone(),
            upstream,
            client,
            limiter: ConnectionAttemptLimiter::new(config.max_requests_per_second),
            counters,
            request_headers,
            response_headers,
        })
    }

    /// Appends a validated remainder and query to the configured upstream.
    /// The request never changes the scheme, host, or port.
    fn upstream_url(&self, remainder: &str, query: Option<&str>) -> Option<Url> {
        let mut url = self.upstream.clone();
        let base = url.path().trim_end_matches('/').to_owned();
        let path = format!("{base}{remainder}");
        url.set_path(if path.is_empty() { "/" } else { &path });
        url.set_query(query);
        (url.scheme() == self.upstream.scheme()
            && url.host() == self.upstream.host()
            && url.port_or_known_default() == self.upstream.port_or_known_default()
            && url.path() == if path.is_empty() { "/" } else { path.as_str() })
        .then_some(url)
    }
}

fn secure_response_headers(response: &mut HttpResponse, route: &CompiledRoute) {
    let headers = response.headers_mut();
    // A relayed file is served from the router's origin. Never let a browser
    // sniff or execute it as active content there.
    headers.insert(
        ntex_header::X_CONTENT_TYPE_OPTIONS,
        NtexHeaderValue::from_static("nosniff"),
    );
    if !headers.contains_key(ntex_header::CONTENT_SECURITY_POLICY) {
        headers.insert(
            ntex_header::CONTENT_SECURITY_POLICY,
            NtexHeaderValue::from_static(SANDBOX_POLICY),
        );
    }
    if !route.config.allow_range {
        headers.insert(
            ntex_header::ACCEPT_RANGES,
            NtexHeaderValue::from_static("none"),
        );
    }
}

struct PumpLimits {
    max_bytes: u64,
    idle: Duration,
    deadline: Instant,
}

enum Frame {
    Data(Bytes),
    End,
}

/// Holds one of the route's concurrency slots until the request ends.
struct ConcurrencySlot(Arc<ByteRouteCounters>);

impl Drop for ConcurrencySlot {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// Relays upstream chunks through a one-chunk channel. Every wait is bounded
/// by the idle timeout and the request deadline, on both the upstream read and
/// the client side, so neither a slow upstream nor a client that stops
/// reading can hold the upstream connection, the client connection, or the
/// concurrency slot.
async fn pump(
    mut response: reqwest::Response,
    mut sender: mpsc::Sender<Frame>,
    connection: Option<IoRef>,
    limits: PumpLimits,
    counters: Arc<ByteRouteCounters>,
    slot: ConcurrencySlot,
) {
    let wait = |limits: &PumpLimits| {
        limits
            .idle
            .min(limits.deadline.saturating_duration_since(Instant::now()))
    };
    let mut relayed = 0_u64;
    let completed = loop {
        let chunk = match timeout(wait(&limits), response.chunk()).await {
            // An empty chunk would read as the end of a chunked response.
            Ok(Ok(Some(chunk))) if chunk.is_empty() => continue,
            Ok(Ok(Some(chunk))) => chunk,
            Ok(Ok(None)) => break true,
            Ok(Err(_)) | Err(_) => break false,
        };
        relayed = relayed.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        if relayed > limits.max_bytes {
            break false;
        }
        let frame = Frame::Data(Bytes::copy_from_slice(&chunk));
        match timeout(wait(&limits), sender.send(frame)).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) | Err(_) => break false,
        }
    };
    drop(response);
    let delivered = completed
        && matches!(
            timeout(wait(&limits), sender.send(Frame::End)).await,
            Ok(Ok(()))
        );
    if !delivered {
        counters.aborted();
        // Closing here also reaches a client that has stopped reading, whose
        // dispatcher no longer polls the body.
        if let Some(connection) = &connection {
            connection.terminate();
        }
    }
    drop(slot);
}

/// Response body fed by [`pump`]. A channel that closes without the final
/// frame is an abort, so the client sees a truncated, failed transfer rather
/// than a short body that looks complete.
struct StreamedBody {
    size: BodySize,
    receiver: mpsc::Receiver<Frame>,
    counters: Arc<ByteRouteCounters>,
    connection: Option<IoRef>,
    finished: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("byte route stream ended before completion")]
struct StreamAborted;

impl MessageBody for StreamedBody {
    fn size(&self) -> BodySize {
        self.size
    }

    fn poll_next_chunk(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Bytes, Rc<dyn std::error::Error>>>> {
        if self.finished {
            return if self.connection.is_some() {
                Poll::Pending
            } else {
                Poll::Ready(None)
            };
        }
        match self.receiver.poll_next_unpin(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(Frame::Data(chunk))) => {
                self.counters.bytes(chunk.len());
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Frame::End)) => {
                self.finished = true;
                Poll::Ready(None)
            }
            Poll::Ready(None) => {
                self.finished = true;
                match &self.connection {
                    // The dispatcher observes the closed connection and drops
                    // this body without writing anything further.
                    Some(connection) => {
                        connection.terminate();
                        Poll::Pending
                    }
                    None => Poll::Ready(Some(Err(Rc::new(StreamAborted)))),
                }
            }
        }
    }
}

/// A body with no bytes that still reports a `HEAD` response's length.
struct EmptyBody(BodySize);

impl MessageBody for EmptyBody {
    fn size(&self) -> BodySize {
        self.0
    }

    fn poll_next_chunk(
        &mut self,
        _: &mut Context<'_>,
    ) -> Poll<Option<Result<Bytes, Rc<dyn std::error::Error>>>> {
        Poll::Ready(None)
    }
}

/// Router errors use the GraphQL endpoint's error shape, plus a header that
/// distinguishes them from relayed upstream responses.
fn router_error(status: StatusCode, code: &'static str, message: &str) -> HttpResponse {
    HttpResponse::build(status)
        .header(ntex_header::CONTENT_TYPE, "application/json")
        .header(ntex_header::CACHE_CONTROL, "no-store")
        .header(ntex_header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(ROUTER_ERROR_HEADER, code)
        .body(json!({"errors": [{"message": message, "extensions": {"code": code}}]}).to_string())
}

fn unauthenticated(status: StatusCode, message: &str) -> HttpResponse {
    router_error(status, "UNAUTHENTICATED", message)
}

fn invalid_path() -> HttpResponse {
    router_error(
        StatusCode::BAD_REQUEST,
        "BYTE_ROUTE_INVALID_PATH",
        "byte route path or query is not allowed",
    )
}

fn upstream_timeout() -> HttpResponse {
    router_error(
        StatusCode::GATEWAY_TIMEOUT,
        "BYTE_ROUTE_UPSTREAM_TIMEOUT",
        "byte route upstream did not respond in time",
    )
}

/// Outermost public-listener middleware that serves byte routes. Without
/// configured routes it forwards every request unchanged.
pub(crate) struct ByteRouteMiddleware(pub(crate) Option<Arc<ByteRouteRuntime>>);

impl<S> Middleware<S, SharedCfg> for ByteRouteMiddleware {
    type Service = ByteRouteService<S>;

    fn create(&self, service: S, _cfg: SharedCfg) -> Self::Service {
        ByteRouteService {
            service,
            runtime: self.0.clone(),
        }
    }
}

pub(crate) struct ByteRouteService<S> {
    service: S,
    runtime: Option<Arc<ByteRouteRuntime>>,
}

impl<S> Service<web::WebRequest<DefaultError>> for ByteRouteService<S>
where
    S: Service<web::WebRequest<DefaultError>, Response = web::WebResponse, Error = web::Error>,
{
    type Response = web::WebResponse;
    type Error = S::Error;

    ntex::forward_ready!(service);

    async fn call(
        &self,
        request: web::WebRequest<DefaultError>,
        ctx: ServiceCtx<'_, Self>,
    ) -> Result<Self::Response, Self::Error> {
        let Some(runtime) = &self.runtime else {
            return ctx.call(&self.service, request).await;
        };
        let path = request.uri().path().to_owned();
        match match_path(&runtime.prefixes, &path) {
            PathMatch::Unclaimed => ctx.call(&self.service, request).await,
            PathMatch::Refused(index) => {
                let route = &runtime.routes[index];
                route.counters.request();
                route.counters.rejected();
                Ok(request.into_response(invalid_path()))
            }
            PathMatch::Route(index, remainder) => {
                let response = runtime.handle(index, remainder, &request).await;
                Ok(request.into_response(response))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compiled(route: ByteRoute) -> CompiledRoute {
        let metrics = RouterMetrics::default();
        let counters = metrics.install_byte_routes(std::slice::from_ref(&route.prefix));
        CompiledRoute::new(&route, counters[0].clone()).unwrap()
    }

    #[test]
    fn upstream_url_keeps_the_configured_origin_and_base_path() {
        let route = compiled(ByteRoute::new("/files", "http://files.test:8080/downloads"));
        assert_eq!(
            route
                .upstream_url("/a/b.bin", Some("v=1"))
                .unwrap()
                .as_str(),
            "http://files.test:8080/downloads/a/b.bin?v=1"
        );
        assert_eq!(
            route.upstream_url("", None).unwrap().as_str(),
            "http://files.test:8080/downloads"
        );
        let root = compiled(ByteRoute::new("/files", "https://files.test"));
        assert_eq!(
            root.upstream_url("", None).unwrap().as_str(),
            "https://files.test/"
        );
        assert_eq!(
            root.upstream_url("/x%20y", None).unwrap().as_str(),
            "https://files.test/x%20y"
        );
        // A remainder that slipped past validation still cannot change the
        // authority: only the path component of the configured URL is set.
        for remainder in ["//evil.test/x", "/@evil.test", "/x?y#z"] {
            let url = root.upstream_url(remainder, Some("a=b#c"));
            if let Some(url) = url {
                assert_eq!(url.host_str(), Some("files.test"), "{remainder}");
                assert_eq!(url.port_or_known_default(), Some(443), "{remainder}");
                assert_eq!(url.scheme(), "https", "{remainder}");
            }
        }
    }

    #[test]
    fn header_allow_lists_follow_route_flags() {
        let plain = compiled(ByteRoute::new("/files", "http://files.test"));
        let names = |names: &[HeaderName]| {
            names
                .iter()
                .map(HeaderName::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        assert!(!names(&plain.request_headers).contains(&"range".to_owned()));
        assert!(!names(&plain.request_headers).contains(&"authorization".to_owned()));
        assert!(!names(&plain.request_headers).contains(&"cookie".to_owned()));
        assert!(!names(&plain.response_headers).contains(&"set-cookie".to_owned()));
        assert!(!names(&plain.response_headers).contains(&"content-range".to_owned()));
        let open = compiled(
            ByteRoute::new("/files", "http://files.test")
                .allow_range(true)
                .allow_set_cookie(true)
                .forward_request_header("x-request-id")
                .pass_response_header("x-checksum"),
        );
        for name in ["range", "if-range", "x-request-id", "accept"] {
            assert!(
                names(&open.request_headers).contains(&name.to_owned()),
                "{name}"
            );
        }
        for name in ["content-range", "accept-ranges", "set-cookie", "x-checksum"] {
            assert!(
                names(&open.response_headers).contains(&name.to_owned()),
                "{name}"
            );
        }
    }
}
