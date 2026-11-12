//! Opt-in byte routes: authenticated, streamed `GET`/`HEAD` passthrough from
//! an exact public path prefix to one statically configured upstream.
//!
//! Byte routes are off by default. Each route is fixed at startup, and the
//! request can never select the upstream scheme, host, or port. Request and
//! response headers cross the router only through explicit allow-lists.

mod proxy;

use std::{collections::BTreeSet, fmt, time::Duration};

use reqwest::header::HeaderName;
use url::Url;

use crate::{RouterError, RouterErrorKind};

pub(crate) use proxy::{ByteRouteMiddleware, ByteRouteRuntime};

const DEFAULT_MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RESPONSE_BYTES_LIMIT: u64 = 1024 * 1024 * 1024 * 1024;
const DEFAULT_MAX_CONCURRENT_REQUESTS: usize = 16;
const DEFAULT_MAX_REQUESTS_PER_SECOND: usize = 32;
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_MAX_DURATION: Duration = Duration::from_secs(15 * 60);
const MAX_PREFIX_BYTES: usize = 256;
pub(crate) const MAX_PATH_BYTES: usize = 2048;
pub(crate) const MAX_QUERY_BYTES: usize = 4096;
/// The router reserves this first segment for private endpoints.
const RESERVED_ROUTER_SEGMENT: &str = "_graphql-orm-router";

/// Request headers that are never configurable for forwarding. Credentials
/// have their own verified path, range headers follow `allow_range`, and the
/// rest are transport-owned or commonly trusted as caller identity.
const DENIED_REQUEST_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "range",
    "if-range",
    "host",
    "content-length",
    "content-type",
    "content-encoding",
    "transfer-encoding",
    "expect",
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "trailer",
    "upgrade",
    "forwarded",
    "via",
    "x-real-ip",
    "x-client-ip",
    "true-client-ip",
    "x-remote-user",
    "remote-user",
    "x-user",
    "x-user-id",
    "x-http-method-override",
    "x-http-method",
    "x-method-override",
    "x-rewrite-url",
    "x-host",
    "cf-connecting-ip",
    "x-cluster-client-ip",
    "fastly-client-ip",
];

/// Request header prefixes that are never configurable for forwarding.
const DENIED_REQUEST_HEADER_PREFIXES: &[&str] = &[
    "x-forwarded-",
    "x-original-",
    "x-auth",
    "x-authenticated-",
    "x-envoy-",
    "x-graphql-orm-router",
    "graphql-orm-router",
];

/// Response headers the router owns or that follow a dedicated route flag.
const DENIED_RESPONSE_HEADERS: &[&str] = &[
    "set-cookie",
    "set-cookie2",
    "content-length",
    "transfer-encoding",
    "content-range",
    "accept-ranges",
    "x-content-type-options",
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "te",
    "trailer",
    "upgrade",
    // These would apply to the router's whole origin, not just the download.
    "alt-svc",
    "strict-transport-security",
    "clear-site-data",
    "public-key-pins",
];

const DENIED_RESPONSE_HEADER_PREFIXES: &[&str] = &[
    "access-control-",
    "x-graphql-orm-router",
    "graphql-orm-router",
];

/// One statically configured byte route.
///
/// A request whose path is exactly `prefix`, or `prefix` followed by `/`, is
/// served by this route. The rest of the path is appended to the upstream
/// URL's path after strict validation. Routes require a verified bearer
/// credential unless they are explicitly marked public.
#[derive(Clone)]
pub struct ByteRoute {
    pub(crate) prefix: String,
    pub(crate) upstream: String,
    pub(crate) public: bool,
    pub(crate) allow_range: bool,
    pub(crate) allow_set_cookie: bool,
    pub(crate) forward_request_headers: BTreeSet<String>,
    pub(crate) pass_response_headers: BTreeSet<String>,
    pub(crate) max_response_bytes: u64,
    pub(crate) max_concurrent_requests: usize,
    pub(crate) max_requests_per_second: usize,
    pub(crate) connect_timeout: Duration,
    pub(crate) idle_timeout: Duration,
    pub(crate) max_duration: Duration,
}

impl ByteRoute {
    /// Creates an authenticated route from an exact path prefix to one
    /// upstream base URL.
    ///
    /// `prefix` is an absolute path such as `/files` with no trailing slash.
    /// `upstream` is an `http` or `https` URL without credentials, query, or
    /// fragment, such as `http://10.0.0.5:8080/downloads`.
    pub fn new(prefix: impl Into<String>, upstream: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            upstream: upstream.into(),
            public: false,
            allow_range: false,
            allow_set_cookie: false,
            forward_request_headers: BTreeSet::new(),
            pass_response_headers: BTreeSet::new(),
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            max_concurrent_requests: DEFAULT_MAX_CONCURRENT_REQUESTS,
            max_requests_per_second: DEFAULT_MAX_REQUESTS_PER_SECOND,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            max_duration: DEFAULT_MAX_DURATION,
        }
    }

    /// Serves this route without a bearer credential.
    ///
    /// A public route still verifies a bearer credential when one is supplied,
    /// and forwards it only after verification. The router logs every public
    /// route at startup.
    #[must_use]
    pub fn public(mut self, public: bool) -> Self {
        self.public = public;
        self
    }

    /// Forwards `Range` and `If-Range`, and passes `Content-Range` and
    /// `Accept-Ranges` back. Without this, a `Range` request is refused.
    #[must_use]
    pub fn allow_range(mut self, allowed: bool) -> Self {
        self.allow_range = allowed;
        self
    }

    /// Passes upstream `Set-Cookie` headers to the client. Off by default.
    #[must_use]
    pub fn allow_set_cookie(mut self, allowed: bool) -> Self {
        self.allow_set_cookie = allowed;
        self
    }

    /// Adds one non-sensitive request header to the upstream allow-list.
    ///
    /// Credentials, cookies, range, transport, forwarding, and common
    /// caller-identity headers are refused at validation.
    #[must_use]
    pub fn forward_request_header(mut self, name: impl Into<String>) -> Self {
        self.forward_request_headers.insert(name.into());
        self
    }

    /// Adds one upstream response header to the client allow-list.
    #[must_use]
    pub fn pass_response_header(mut self, name: impl Into<String>) -> Self {
        self.pass_response_headers.insert(name.into());
        self
    }

    /// Sets the largest response body the route will relay. Default 64 MiB.
    #[must_use]
    pub fn with_max_response_bytes(mut self, bytes: u64) -> Self {
        self.max_response_bytes = bytes;
        self
    }

    /// Sets how many requests the route serves at once. Default 16.
    #[must_use]
    pub fn with_max_concurrent_requests(mut self, maximum: usize) -> Self {
        self.max_concurrent_requests = maximum;
        self
    }

    /// Sets the route's request admission rate. Default 32 per second.
    #[must_use]
    pub fn with_max_requests_per_second(mut self, maximum: usize) -> Self {
        self.max_requests_per_second = maximum;
        self
    }

    /// Sets the upstream TCP and TLS connect deadline. Default 5 seconds.
    #[must_use]
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Sets the longest wait for upstream response headers, for each upstream
    /// body chunk, and for the client to accept each chunk. Default 30 seconds.
    #[must_use]
    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }

    /// Sets the total lifetime of one request. Default 15 minutes.
    #[must_use]
    pub fn with_max_duration(mut self, duration: Duration) -> Self {
        self.max_duration = duration;
        self
    }

    /// Returns the configured public path prefix.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Returns whether the route is served without a bearer credential.
    pub fn is_public(&self) -> bool {
        self.public
    }
}

impl fmt::Debug for ByteRoute {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ByteRoute")
            .field("prefix", &self.prefix)
            .field("upstream", &self.upstream)
            .field("public", &self.public)
            .field("allow_range", &self.allow_range)
            .field("allow_set_cookie", &self.allow_set_cookie)
            .field("forward_request_headers", &self.forward_request_headers)
            .field("pass_response_headers", &self.pass_response_headers)
            .field("max_response_bytes", &self.max_response_bytes)
            .field("max_concurrent_requests", &self.max_concurrent_requests)
            .field("max_requests_per_second", &self.max_requests_per_second)
            .field("connect_timeout", &self.connect_timeout)
            .field("idle_timeout", &self.idle_timeout)
            .field("max_duration", &self.max_duration)
            .finish()
    }
}

/// Paths the public listener already serves, which no byte route may shadow.
pub(crate) struct ReservedPaths<'a> {
    pub(crate) graphql_path: &'a str,
    pub(crate) authentication_configured: bool,
}

pub(crate) fn validate_byte_routes(
    routes: &mut [ByteRoute],
    reserved: &ReservedPaths<'_>,
) -> Result<(), RouterError> {
    for route in routes.iter_mut() {
        validate_prefix(&route.prefix)?;
        let prefix = route.prefix.clone();
        for path in [reserved.graphql_path, "/health", "/readiness"] {
            if paths_overlap(&prefix, path) {
                return Err(invalid(format!(
                    "byte route `{prefix}` overlaps the router path `{path}`"
                )));
            }
        }
        if prefix
            .trim_start_matches('/')
            .split('/')
            .next()
            .is_some_and(|segment| segment.eq_ignore_ascii_case(RESERVED_ROUTER_SEGMENT))
        {
            return Err(invalid(format!(
                "byte route `{prefix}` uses the reserved `/{RESERVED_ROUTER_SEGMENT}` path"
            )));
        }
        let upstream = validate_upstream(&route.upstream, &prefix)?;
        route.upstream = upstream.to_string();
        if !route.public && !reserved.authentication_configured {
            return Err(invalid(format!(
                "byte route `{prefix}` requires a resource-server authentication provider; mark it public only if it is deliberately unauthenticated"
            )));
        }
        route.forward_request_headers = normalize_headers(
            std::mem::take(&mut route.forward_request_headers),
            &prefix,
            "request",
            DENIED_REQUEST_HEADERS,
            DENIED_REQUEST_HEADER_PREFIXES,
        )?;
        route.pass_response_headers = normalize_headers(
            std::mem::take(&mut route.pass_response_headers),
            &prefix,
            "response",
            DENIED_RESPONSE_HEADERS,
            DENIED_RESPONSE_HEADER_PREFIXES,
        )?;
        if route.max_response_bytes == 0 || route.max_response_bytes > MAX_RESPONSE_BYTES_LIMIT {
            return Err(invalid(format!(
                "byte route `{prefix}` maximum response size must be between 1 byte and 1 TiB"
            )));
        }
        if !(1..=10_000).contains(&route.max_concurrent_requests) {
            return Err(invalid(format!(
                "byte route `{prefix}` maximum concurrent requests must be between 1 and 10000"
            )));
        }
        if !(1..=10_000).contains(&route.max_requests_per_second) {
            return Err(invalid(format!(
                "byte route `{prefix}` maximum requests per second must be between 1 and 10000"
            )));
        }
        if route.connect_timeout.is_zero() || route.connect_timeout > Duration::from_secs(60) {
            return Err(invalid(format!(
                "byte route `{prefix}` connect timeout must be greater than zero and at most 60 seconds"
            )));
        }
        if route.idle_timeout.is_zero() || route.idle_timeout > Duration::from_secs(300) {
            return Err(invalid(format!(
                "byte route `{prefix}` idle timeout must be greater than zero and at most 300 seconds"
            )));
        }
        if route.max_duration < route.idle_timeout
            || route.max_duration > Duration::from_secs(24 * 60 * 60)
        {
            return Err(invalid(format!(
                "byte route `{prefix}` maximum duration must be at least the idle timeout and at most 24 hours"
            )));
        }
    }
    for (index, route) in routes.iter().enumerate() {
        for other in &routes[index + 1..] {
            if paths_overlap(&route.prefix, &other.prefix) {
                return Err(invalid(format!(
                    "byte routes `{}` and `{}` overlap",
                    route.prefix, other.prefix
                )));
            }
        }
    }
    Ok(())
}

fn validate_prefix(prefix: &str) -> Result<(), RouterError> {
    let valid = prefix.len() <= MAX_PREFIX_BYTES
        && prefix.len() >= 2
        && prefix.starts_with('/')
        && !prefix.ends_with('/')
        && prefix[1..].split('/').all(|segment| {
            !segment.is_empty()
                && !segment.bytes().all(|byte| byte == b'.')
                && segment.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
                })
        });
    if valid {
        Ok(())
    } else {
        Err(invalid(format!(
            "byte route prefix `{prefix}` must be an absolute path of non-empty segments using only letters, digits, `-`, `.`, `_` or `~`, with no dot segments and no trailing slash"
        )))
    }
}

fn validate_upstream(value: &str, prefix: &str) -> Result<Url, RouterError> {
    // URL parsing silently resolves dot segments, so check the raw text too.
    let raw_path = value
        .split_once("://")
        .map_or(value, |(_, rest)| rest)
        .split(['?', '#'])
        .next()
        .unwrap_or_default();
    if raw_path.split('/').skip(1).any(|segment| {
        let segment = segment.to_ascii_lowercase();
        segment == "." || segment == ".." || segment.contains("%2e") || segment.contains('\\')
    }) {
        return Err(invalid(format!(
            "byte route `{prefix}` upstream path must not contain dot segments"
        )));
    }
    let mut url = Url::parse(value)
        .map_err(|_| invalid(format!("byte route `{prefix}` upstream is not a valid URL")))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(format!(
            "byte route `{prefix}` upstream must be an http(s) URL without embedded credentials, a query, or a fragment"
        )));
    }
    let path = url.path().trim_end_matches('/').to_owned();
    if path.split('/').skip(1).any(|segment| {
        segment.is_empty() || segment == "." || segment == ".." || segment.contains('%')
    }) {
        return Err(invalid(format!(
            "byte route `{prefix}` upstream path must not contain empty, dot, or percent-encoded segments"
        )));
    }
    url.set_path(&path);
    Ok(url)
}

fn normalize_headers(
    names: BTreeSet<String>,
    prefix: &str,
    direction: &str,
    denied: &[&str],
    denied_prefixes: &[&str],
) -> Result<BTreeSet<String>, RouterError> {
    let mut normalized = BTreeSet::new();
    for name in names {
        let parsed = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
            invalid(format!(
                "byte route `{prefix}` has an invalid {direction} header name `{name}`"
            ))
        })?;
        let name = parsed.as_str().to_owned();
        if denied.contains(&name.as_str())
            || denied_prefixes
                .iter()
                .any(|denied| name.starts_with(denied))
        {
            return Err(invalid(format!(
                "byte route `{prefix}` {direction} header `{name}` is reserved or security-sensitive"
            )));
        }
        normalized.insert(name);
    }
    Ok(normalized)
}

/// Whether two absolute paths can claim the same request path, comparing
/// whole segments so `/files` and `/files-admin` do not overlap.
fn paths_overlap(left: &str, right: &str) -> bool {
    fn contains(parent: &str, child: &str) -> bool {
        child == parent
            || (child.len() > parent.len()
                && child.starts_with(parent)
                && child.as_bytes()[parent.len()] == b'/')
    }
    let left = left.to_ascii_lowercase();
    let right = right.to_ascii_lowercase();
    contains(&left, &right) || contains(&right, &left)
}

/// The route-relative part of a request path.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum PathMatch<'a> {
    /// No byte route claims this path; the request continues to GraphQL.
    Unclaimed,
    /// A byte route claims the path, but the remainder is unsafe.
    Refused(usize),
    /// A byte route claims the path and the remainder is safe to append.
    Route(usize, &'a str),
}

/// Matches a raw, still percent-encoded request path against the routes.
///
/// A route claims its exact prefix and every path below it at a segment
/// boundary. Everything a route claims is either a safe remainder or refused,
/// so an unsafe path under a prefix never falls through to another handler.
pub(crate) fn match_path<'a>(prefixes: &[String], raw_path: &'a str) -> PathMatch<'a> {
    for (index, prefix) in prefixes.iter().enumerate() {
        let Some(remainder) = raw_path.strip_prefix(prefix.as_str()) else {
            continue;
        };
        if !(remainder.is_empty() || remainder.starts_with('/')) {
            continue;
        }
        return if raw_path.len() <= MAX_PATH_BYTES && safe_remainder(remainder) {
            PathMatch::Route(index, remainder)
        } else {
            PathMatch::Refused(index)
        };
    }
    PathMatch::Unclaimed
}

fn safe_remainder(remainder: &str) -> bool {
    if remainder.is_empty() {
        return true;
    }
    remainder[1..].split('/').all(safe_segment)
}

fn safe_segment(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    if bytes.is_empty() || bytes.iter().all(|byte| *byte == b'.') {
        return false;
    }
    if !safe_decoded_segment(bytes) {
        return false;
    }
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%' {
            let (Some(high), Some(low)) = (
                bytes.get(index + 1).and_then(|byte| hex_value(*byte)),
                bytes.get(index + 2).and_then(|byte| hex_value(*byte)),
            ) else {
                return false;
            };
            let decoded = high << 4 | low;
            // Encoded separators, dots, escapes, and controls let a later
            // decoder see a different path than the one validated here.
            if matches!(decoded, b'/' | b'\\' | b'.' | b'%' | b';')
                || decoded < 0x20
                || decoded == 0x7f
            {
                return false;
            }
            index += 3;
            continue;
        }
        // RFC 3986 unreserved characters and a conservative subset of
        // sub-delimiters. `;` (path parameters) and `\` are excluded because
        // some upstream servers treat them as path syntax.
        if !(byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b'='
                    | b':'
                    | b'@'
            ))
        {
            return false;
        }
        index += 1;
    }
    true
}

/// Refuses segments that some servers normalize into a dot segment: Windows
/// strips leading spaces and trailing spaces and dots, so `..%20` or `.. .`
/// become `..`. Overlong UTF-8 lead bytes `C0` and `C1` (for example `%C0%AE`
/// for `.`) are also refused.
fn safe_decoded_segment(bytes: &[u8]) -> bool {
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let (Some(high), Some(low)) = (
                bytes.get(index + 1).and_then(|byte| hex_value(*byte)),
                bytes.get(index + 2).and_then(|byte| hex_value(*byte)),
            )
        {
            decoded.push(high << 4 | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    if decoded.iter().any(|byte| matches!(byte, 0xc0 | 0xc1)) {
        return false;
    }
    let start = decoded
        .iter()
        .position(|byte| *byte != b' ')
        .unwrap_or(decoded.len());
    let end = decoded
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'.'))
        .map_or(start, |position| position + 1)
        .max(start);
    let core = &decoded[start..end];
    !core.is_empty()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Whether a raw query string is safe to copy onto the upstream URL.
pub(crate) fn safe_query(query: &str) -> bool {
    query.len() <= MAX_QUERY_BYTES
        && query
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'#')
}

/// The startup warning naming every route served without authentication.
pub(crate) fn public_route_warning(routes: &[ByteRoute]) -> Option<String> {
    let public = routes
        .iter()
        .filter(|route| route.public)
        .map(|route| route.prefix.as_str())
        .collect::<Vec<_>>();
    (!public.is_empty()).then(|| {
        format!(
            "byte routes serve without authentication: {}",
            public.join(", ")
        )
    })
}

fn invalid(message: impl Into<String>) -> RouterError {
    RouterError::new(RouterErrorKind::InvalidConfiguration, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reserved(authentication_configured: bool) -> ReservedPaths<'static> {
        ReservedPaths {
            graphql_path: "/graphql",
            authentication_configured,
        }
    }

    fn validate(route: ByteRoute) -> Result<ByteRoute, RouterError> {
        let mut routes = vec![route];
        validate_byte_routes(&mut routes, &reserved(true))?;
        Ok(routes.remove(0))
    }

    #[test]
    fn prefixes_match_whole_segments_only() {
        let prefixes = vec!["/files".to_owned(), "/exports/v1".to_owned()];
        assert_eq!(match_path(&prefixes, "/files"), PathMatch::Route(0, ""));
        assert_eq!(
            match_path(&prefixes, "/files/a/b.bin"),
            PathMatch::Route(0, "/a/b.bin")
        );
        assert_eq!(match_path(&prefixes, "/files-admin"), PathMatch::Unclaimed);
        assert_eq!(
            match_path(&prefixes, "/files-admin/x"),
            PathMatch::Unclaimed
        );
        assert_eq!(match_path(&prefixes, "/filesx"), PathMatch::Unclaimed);
        assert_eq!(match_path(&prefixes, "/FILES/x"), PathMatch::Unclaimed);
        assert_eq!(match_path(&prefixes, "/exports"), PathMatch::Unclaimed);
        assert_eq!(
            match_path(&prefixes, "/exports/v1/report.csv"),
            PathMatch::Route(1, "/report.csv")
        );
        assert_eq!(match_path(&prefixes, "/graphql"), PathMatch::Unclaimed);
    }

    #[test]
    fn traversal_and_encoding_tricks_under_a_prefix_are_refused() {
        let prefixes = vec!["/files".to_owned()];
        for path in [
            "/files/",
            "/files//etc/passwd",
            "/files/a//b",
            "/files/.",
            "/files/..",
            "/files/../graphql",
            "/files/a/../../graphql",
            "/files/./a",
            "/files/...",
            "/files/%2e%2e/graphql",
            "/files/%2E%2E/graphql",
            "/files/.%2e/x",
            "/files/a%2fb",
            "/files/a%2Fb",
            "/files/a%5cb",
            "/files/a%00b",
            "/files/a%0ab",
            "/files/a%252fb",
            "/files/a%2",
            "/files/a%zz",
            "/files/a\\b",
            "/files/..;/graphql",
            "/files/a;b",
            "/files/a%3bb",
            "/files/..%20/x",
            "/files/.%20/x",
            "/files/%20../x",
            "/files/..%20./x",
            "/files/.%20.%20/x",
            "/files/%C0%AE%C0%AE/x",
            "/files/%c1%9c",
            "/files/%20",
            "/files/a b",
            "/files/a\"b",
            "/files/a<b",
        ] {
            assert_eq!(
                match_path(&prefixes, path),
                PathMatch::Refused(0),
                "`{path}` must be refused"
            );
        }
        let long = format!("/files/{}", "a".repeat(MAX_PATH_BYTES));
        assert_eq!(match_path(&prefixes, &long), PathMatch::Refused(0));
        for path in [
            "/files/report.v2.tar.gz",
            "/files/a%20b",
            "/files/%C3%A9t%C3%A9.txt",
            "/files/a-b_c~d",
            "/files/k=v,w:x@y+z",
            "/files/.hidden",
            "/files/a%20b%20",
            "/files/v1.2",
        ] {
            assert!(
                matches!(match_path(&prefixes, path), PathMatch::Route(0, _)),
                "`{path}` must be accepted"
            );
        }
    }

    #[test]
    fn queries_are_bounded_printable_ascii() {
        assert!(safe_query("token=abc&name=a%20b"));
        assert!(safe_query(""));
        assert!(!safe_query("a b"));
        assert!(!safe_query("a#b"));
        assert!(!safe_query("a\u{7f}"));
        assert!(!safe_query(&"a".repeat(MAX_QUERY_BYTES + 1)));
    }

    #[test]
    fn route_validation_fails_closed() {
        for prefix in [
            "",
            "/",
            "files",
            "/files/",
            "//files",
            "/files//x",
            "/./files",
            "/files/..",
            "/fi les",
            "/files%2f",
            "/files;x",
            "/_graphql-orm-router",
            "/_GRAPHQL-ORM-ROUTER/x",
        ] {
            assert!(
                validate(ByteRoute::new(prefix, "http://files.test")).is_err(),
                "prefix `{prefix}` must be refused"
            );
        }
        for path in ["/graphql", "/graphql/files", "/health", "/readiness/x"] {
            assert!(
                validate(ByteRoute::new(path, "http://files.test")).is_err(),
                "router path `{path}` must not be shadowed"
            );
        }
        let mut nested = vec![
            ByteRoute::new("/files", "http://files.test"),
            ByteRoute::new("/files/private", "http://files.test"),
        ];
        assert!(validate_byte_routes(&mut nested, &reserved(true)).is_err());
        let mut sibling = vec![
            ByteRoute::new("/files", "http://files.test"),
            ByteRoute::new("/files-admin", "http://files.test"),
        ];
        assert!(validate_byte_routes(&mut sibling, &reserved(true)).is_ok());
        for upstream in [
            "files.test/x",
            "ftp://files.test",
            "http://user:secret@files.test",
            "http://files.test/x?y=1",
            "http://files.test/x#y",
            "http://files.test/a//b",
            "http://files.test/a/../b",
            "http://files.test/a%2fb",
        ] {
            assert!(
                validate(ByteRoute::new("/files", upstream)).is_err(),
                "upstream `{upstream}` must be refused"
            );
        }
        let normalized =
            validate(ByteRoute::new("/files", "http://files.test/downloads/")).unwrap();
        assert_eq!(normalized.upstream, "http://files.test/downloads");
        let mut anonymous = vec![ByteRoute::new("/files", "http://files.test")];
        assert!(validate_byte_routes(&mut anonymous, &reserved(false)).is_err());
        let mut anonymous_public = vec![ByteRoute::new("/files", "http://files.test").public(true)];
        assert!(validate_byte_routes(&mut anonymous_public, &reserved(false)).is_ok());
    }

    #[test]
    fn header_allow_lists_refuse_credentials_identity_and_transport() {
        for name in [
            "Authorization",
            "cookie",
            "range",
            "if-range",
            "host",
            "x-forwarded-for",
            "X-Forwarded-User",
            "forwarded",
            "x-real-ip",
            "x-user-id",
            "x-auth-request-user",
            "x-authenticated-userid",
            "x-graphql-orm-router-internal",
            "x-http-method-override",
            "x-envoy-original-path",
            "cf-connecting-ip",
            "transfer-encoding",
            "connection",
            "bad header",
        ] {
            assert!(
                validate(
                    ByteRoute::new("/files", "http://files.test").forward_request_header(name)
                )
                .is_err(),
                "request header `{name}` must be refused"
            );
        }
        for name in [
            "set-cookie",
            "Content-Length",
            "transfer-encoding",
            "content-range",
            "x-content-type-options",
            "graphql-orm-router-error",
            "strict-transport-security",
            "alt-svc",
            "access-control-allow-origin",
            "clear-site-data",
        ] {
            assert!(
                validate(ByteRoute::new("/files", "http://files.test").pass_response_header(name))
                    .is_err(),
                "response header `{name}` must be refused"
            );
        }
        let route = validate(
            ByteRoute::new("/files", "http://files.test")
                .forward_request_header("X-Request-Id")
                .pass_response_header("X-Checksum"),
        )
        .unwrap();
        assert!(route.forward_request_headers.contains("x-request-id"));
        assert!(route.pass_response_headers.contains("x-checksum"));
    }

    #[test]
    fn limits_are_bounded() {
        let base = || ByteRoute::new("/files", "http://files.test");
        assert!(validate(base().with_max_response_bytes(0)).is_err());
        assert!(validate(base().with_max_concurrent_requests(0)).is_err());
        assert!(validate(base().with_max_requests_per_second(0)).is_err());
        assert!(validate(base().with_connect_timeout(Duration::ZERO)).is_err());
        assert!(validate(base().with_connect_timeout(Duration::from_secs(61))).is_err());
        assert!(validate(base().with_idle_timeout(Duration::ZERO)).is_err());
        assert!(
            validate(
                base()
                    .with_idle_timeout(Duration::from_secs(10))
                    .with_max_duration(Duration::from_secs(5))
            )
            .is_err()
        );
        assert!(validate(base()).is_ok());
    }

    #[test]
    fn public_routes_are_named_in_the_startup_warning() {
        assert_eq!(
            public_route_warning(&[ByteRoute::new("/files", "http://files.test")]),
            None
        );
        assert_eq!(
            public_route_warning(&[
                ByteRoute::new("/files", "http://files.test"),
                ByteRoute::new("/public", "http://files.test").public(true),
                ByteRoute::new("/shared", "http://files.test").public(true),
            ])
            .as_deref(),
            Some("byte routes serve without authentication: /public, /shared")
        );
    }
}
