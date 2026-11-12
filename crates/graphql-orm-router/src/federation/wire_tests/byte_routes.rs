//! Loopback coverage for opt-in byte routes on the public listener.
//!
//! The parent owns a raw-socket byte upstream and every assertion; the child
//! process runs an authenticated router with byte routes and an admin listener.

use super::*;
use crate::ByteRoute;

const BYTES_ENDPOINT_ENV: &str = "GRAPHQL_ORM_ROUTER_WIRE_BYTES_ENDPOINT";
const DOWN_ENDPOINT_ENV: &str = "GRAPHQL_ORM_ROUTER_WIRE_DOWN_ENDPOINT";
const LARGE_BODY_BYTES: usize = 128 * 1024 * 1024;
/// Generous headroom over the router's steady state. Buffering the large body
/// would raise the peak by at least `LARGE_BODY_BYTES`.
const MAX_STREAMING_PEAK_GROWTH_BYTES: u64 = 48 * 1024 * 1024;

pub(super) async fn run_child() {
    let public_address = child_listener_address();
    let admin_address = child_listener_address();
    let bytes = env::var(BYTES_ENDPOINT_ENV).expect("byte upstream endpoint");
    let down = env::var(DOWN_ENDPOINT_ENV).expect("unreachable endpoint");
    let products = StaticSubgraph::new(
        "products",
        env::var(PRODUCTS_ENDPOINT_ENV).expect("products endpoint"),
        env::var(PRODUCTS_SDL_ENDPOINT_ENV).expect("products SDL endpoint"),
    )
    .with_schema_header("authorization", "Bearer schema-secret")
    .with_protocol_url(env::var(PRODUCTS_PROTOCOL_ENDPOINT_ENV).expect("products protocol"));
    let reviews = StaticSubgraph::new(
        "reviews",
        env::var(REVIEWS_ENDPOINT_ENV).expect("reviews endpoint"),
        env::var(REVIEWS_SDL_ENDPOINT_ENV).expect("reviews SDL endpoint"),
    )
    .with_schema_header("authorization", "Bearer schema-secret")
    .with_protocol_url(env::var(REVIEWS_PROTOCOL_ENDPOINT_ENV).expect("reviews protocol"));
    let config = RouterConfig::new(public_address)
        .with_graphql_path("/api/graphql")
        .with_authentication_provider(Arc::new(TestAuthenticationProvider))
        .with_scope_matcher(Arc::new(TestHierarchicalScopeMatcher))
        .with_subgraph(products)
        .with_subgraph(reviews)
        .with_admin(AdminConfig::new(admin_address, NetworkPolicy::new()))
        .with_byte_route(
            ByteRoute::new("/files", format!("{bytes}/files"))
                .with_max_response_bytes(LARGE_BODY_BYTES as u64 + 1024)
                .with_idle_timeout(Duration::from_secs(5)),
        )
        .with_byte_route(ByteRoute::new("/public", format!("{bytes}/files")).public(true))
        .with_byte_route(
            ByteRoute::new("/ranged", format!("{bytes}/files"))
                .allow_range(true)
                .allow_set_cookie(true)
                .forward_request_header("x-request-id")
                .pass_response_header("x-checksum"),
        )
        .with_byte_route(
            ByteRoute::new("/small", format!("{bytes}/files")).with_max_response_bytes(1024),
        )
        .with_byte_route(
            ByteRoute::new("/slow", format!("{bytes}/files"))
                .with_idle_timeout(Duration::from_millis(300))
                .with_max_duration(Duration::from_secs(2)),
        )
        .with_byte_route(
            ByteRoute::new("/stall", format!("{bytes}/files"))
                .with_idle_timeout(Duration::from_millis(300))
                .with_max_duration(Duration::from_secs(600)),
        )
        .with_byte_route(
            ByteRoute::new("/down", down).with_connect_timeout(Duration::from_millis(500)),
        )
        .with_byte_route(
            ByteRoute::new("/limited", format!("{bytes}/files")).with_max_requests_per_second(1),
        )
        .with_byte_route(
            ByteRoute::new("/single", format!("{bytes}/files")).with_max_concurrent_requests(1),
        );
    config
        .prepare()
        .await
        .expect("byte route router should prepare")
        .run()
        .await
        .expect("byte route router should run");
}

/// Disposable subgraphs, byte upstream, and router child for one test.
struct Fixture {
    router: TestRouter,
    admin: SocketAddr,
    upstream: ByteUpstream,
    _subgraphs: (LoopbackSubgraph, LoopbackSubgraph),
    _schemas: (LoopbackSdl, LoopbackSdl),
    _protocols: (LoopbackProtocol, LoopbackProtocol),
}

impl Fixture {
    fn start() -> Self {
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
        let upstream = ByteUpstream::start();
        let down = {
            let listener = TcpListener::bind(("127.0.0.1", 0)).expect("closed port probe");
            format!("http://{}/files", listener.local_addr().unwrap())
        };
        let child = Command::new(env::current_exe().expect("test executable path"))
            .arg("--exact")
            .arg("federation::wire_tests::router_child_entrypoint")
            .arg("--nocapture")
            .env(CHILD_ENV, "byte-routes")
            .env(PRODUCTS_ENDPOINT_ENV, products.endpoint())
            .env(REVIEWS_ENDPOINT_ENV, reviews.endpoint())
            .env(PRODUCTS_SDL_ENDPOINT_ENV, products_sdl.endpoint())
            .env(REVIEWS_SDL_ENDPOINT_ENV, reviews_sdl.endpoint())
            .env(PRODUCTS_PROTOCOL_ENDPOINT_ENV, products_protocol.endpoint())
            .env(REVIEWS_PROTOCOL_ENDPOINT_ENV, reviews_protocol.endpoint())
            .env(BYTES_ENDPOINT_ENV, upstream.origin())
            .env(DOWN_ENDPOINT_ENV, down)
            .env("RUST_TEST_THREADS", "1")
            .env("RUST_LOG", "error")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("byte route router child should start");
        let (mut router, admin) = TestRouter::from_child(child, true);
        router.wait_until_ready();
        Self {
            router,
            admin: admin.expect("bound admin address"),
            upstream,
            _subgraphs: (products, reviews),
            _schemas: (products_sdl, reviews_sdl),
            _protocols: (products_protocol, reviews_protocol),
        }
    }

    fn get(&self, path: &str, headers: &[(&str, &str)]) -> Response {
        self.request("GET", path, headers)
    }

    fn request(&self, method: &str, path: &str, headers: &[(&str, &str)]) -> Response {
        let mut request =
            format!("{method} {path} HTTP/1.1\r\nHost: router.test\r\nConnection: close\r\n");
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("\r\n");
        send(self.router.address, request.as_bytes(), method == "HEAD")
    }

    fn metrics(&self) -> Value {
        let response = raw_http(
            self.admin,
            "GET",
            "/_router/metrics",
            &[("authorization", "Bearer admin")],
            None,
        )
        .expect("admin metrics");
        assert_eq!(response.status, 200);
        serde_json::from_slice(&response.body).expect("metrics JSON")
    }

    fn child_memory(&self, field: &str) -> u64 {
        let pid = lock_child(&self.router.child)
            .as_ref()
            .expect("running router child")
            .id();
        let status = fs::read_to_string(format!("/proc/{pid}/status")).expect("child status");
        status
            .lines()
            .find_map(|line| line.strip_prefix(field))
            .and_then(|value| {
                value
                    .trim()
                    .trim_end_matches("kB")
                    .trim()
                    .parse::<u64>()
                    .ok()
            })
            .expect("child memory field")
            * 1024
    }
}

struct Response {
    status: u16,
    head: String,
    body: Vec<u8>,
    /// The body ended exactly as its framing declared.
    complete: bool,
}

impl Response {
    fn header(&self, name: &str) -> Option<&str> {
        self.head.lines().skip(1).find_map(|line| {
            line.split_once(':')
                .filter(|(header, _)| header.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.trim())
        })
    }

    fn router_error(&self) -> Option<&str> {
        self.header("graphql-orm-router-error")
    }

    fn error_code(&self) -> String {
        let value: Value = serde_json::from_slice(&self.body).expect("router error JSON");
        value["errors"][0]["extensions"]["code"]
            .as_str()
            .expect("error code")
            .to_owned()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

fn connect(address: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect_timeout(&address, Duration::from_secs(2)).expect("connect");
    stream.set_read_timeout(Some(TEST_TIMEOUT)).unwrap();
    stream.set_write_timeout(Some(TEST_TIMEOUT)).unwrap();
    stream
}

fn send(address: SocketAddr, request: &[u8], head_only: bool) -> Response {
    let mut stream = connect(address);
    stream.write_all(request).expect("write request");
    read_response(&mut stream, head_only)
}

fn read_head(stream: &mut TcpStream) -> (String, Vec<u8>) {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8(bytes[..end].to_vec()).expect("UTF-8 head");
            return (head, bytes[end + 4..].to_vec());
        }
        let read = stream.read(&mut buffer).expect("read response head");
        assert_ne!(read, 0, "connection closed before the response head");
        bytes.extend_from_slice(&buffer[..read]);
    }
}

/// Reads the rest of the body, tolerating an aborted transfer.
fn read_response(stream: &mut TcpStream, head_only: bool) -> Response {
    let (head, mut rest) = read_head(stream);
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .expect("status");
    let mut response = Response {
        status,
        head,
        body: Vec::new(),
        complete: false,
    };
    if head_only {
        response.complete = true;
        return response;
    }
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut eof = false;
    let mut more = |rest: &mut Vec<u8>, eof: &mut bool| {
        if *eof {
            return false;
        }
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => {
                *eof = true;
                false
            }
            Ok(read) => {
                rest.extend_from_slice(&buffer[..read]);
                true
            }
        }
    };
    if response
        .header("transfer-encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
    {
        loop {
            let Some(line_end) = (loop {
                if let Some(position) = rest.windows(2).position(|window| window == b"\r\n") {
                    break Some(position);
                }
                if !more(&mut rest, &mut eof) {
                    break None;
                }
            }) else {
                return response;
            };
            let length = usize::from_str_radix(
                std::str::from_utf8(&rest[..line_end])
                    .unwrap_or_default()
                    .trim(),
                16,
            )
            .unwrap_or_else(|_| {
                panic!(
                    "invalid chunk header in {:?} after {:?}",
                    String::from_utf8_lossy(&rest[..rest.len().min(200)]),
                    response.head
                )
            });
            let required = line_end + 2 + length + 2;
            while rest.len() < required {
                if !more(&mut rest, &mut eof) {
                    let available = rest.len().saturating_sub(line_end + 2).min(length);
                    response
                        .body
                        .extend_from_slice(&rest[line_end + 2..line_end + 2 + available]);
                    return response;
                }
            }
            if length == 0 {
                response.complete = true;
                return response;
            }
            response
                .body
                .extend_from_slice(&rest[line_end + 2..line_end + 2 + length]);
            rest.drain(..required);
        }
    }
    let declared = response
        .header("content-length")
        .map(|value| value.parse::<usize>().expect("content length"));
    loop {
        if let Some(length) = declared
            && rest.len() >= length
        {
            rest.truncate(length);
            response.body = rest;
            response.complete = true;
            return response;
        }
        if !more(&mut rest, &mut eof) {
            response.complete = declared.is_none();
            response.body = rest;
            return response;
        }
    }
}

/// Raw-socket upstream with deterministic, inspectable behaviour per path.
struct ByteUpstream {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<String>>>,
    gate: Arc<SlowGate>,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ByteUpstream {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("byte upstream bind");
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let gate = Arc::new(SlowGate::default());
        let stopping = Arc::new(AtomicBool::new(false));
        let (thread_requests, thread_gate, thread_stopping) =
            (requests.clone(), gate.clone(), stopping.clone());
        let thread = thread::spawn(move || {
            while !thread_stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let (requests, gate) = (thread_requests.clone(), thread_gate.clone());
                        thread::spawn(move || serve_bytes(stream, &requests, &gate));
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
            requests,
            gate,
            stopping,
            thread: Some(thread),
        }
    }

    fn origin(&self) -> String {
        format!("http://{}", self.address)
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

impl Drop for ByteUpstream {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        self.gate.release();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve_bytes(mut stream: TcpStream, requests: &Mutex<Vec<String>>, gate: &SlowGate) {
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(TEST_TIMEOUT)).unwrap();
    let Ok((head, _)) = read_head_and_body(&mut stream) else {
        return;
    };
    requests.lock().unwrap().push(head.clone());
    let target = head
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let size = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("size="))
        .and_then(|size| size.parse::<usize>().ok())
        .unwrap_or(0);
    let head_request = head.starts_with("HEAD ");
    let fixed = |stream: &mut TcpStream, status: &str, headers: &str, body: &[u8]| {
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
            body.len()
        );
        if !head_request {
            let _ = stream.write_all(body);
        }
    };
    match path {
        "/files/big" => {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"big.bin\"\r\nConnection: close\r\n\r\n"
            );
            if head_request {
                return;
            }
            let block = (0..64 * 1024).map(|index| (index % 251) as u8).collect::<Vec<_>>();
            let mut remaining = size;
            while remaining > 0 {
                let length = remaining.min(block.len());
                if stream.write_all(&block[..length]).is_err() {
                    return;
                }
                remaining -= length;
            }
        }
        "/files/chunked" => {
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
            );
            let block = vec![b'c'; 512];
            let mut remaining = size;
            while remaining > 0 {
                let length = remaining.min(block.len());
                if write!(stream, "{length:x}\r\n").is_err()
                    || stream.write_all(&block[..length]).is_err()
                    || stream.write_all(b"\r\n").is_err()
                    || stream.flush().is_err()
                {
                    return;
                }
                remaining -= length;
                thread::sleep(Duration::from_millis(2));
            }
            let _ = stream.write_all(b"0\r\n\r\n");
        }
        "/files/gated" => {
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nfirst\r\n",
            );
            let _ = stream.flush();
            gate.hold_until_released();
            let _ = stream.write_all(b"6\r\nsecond\r\n0\r\n\r\n");
        }
        "/files/echo" => fixed(
            &mut stream,
            "200 OK",
            "Content-Type: text/plain\r\nContent-Disposition: inline\r\nCache-Control: private, max-age=60\r\nETag: \"v1\"\r\nSet-Cookie: session=upstream\r\nX-Checksum: abc123\r\nX-Internal-Secret: hidden\r\nServer: upstream-internal\r\nAccept-Ranges: bytes\r\n",
            head.as_bytes(),
        ),
        "/files/html" => fixed(
            &mut stream,
            "200 OK",
            "Content-Type: text/html\r\n",
            b"<script>alert(1)</script>",
        ),
        "/files/range" => {
            if has_header(&head, "range", "bytes=0-3") {
                fixed(
                    &mut stream,
                    "206 Partial Content",
                    "Content-Range: bytes 0-3/10\r\nAccept-Ranges: bytes\r\n",
                    b"0123",
                );
            } else {
                fixed(&mut stream, "200 OK", "Accept-Ranges: bytes\r\n", b"0123456789");
            }
        }
        "/files/status/418" => fixed(
            &mut stream,
            "418 I'm a teapot",
            "Content-Type: text/plain\r\nRetry-After: 7\r\n",
            b"short and stout",
        ),
        "/files/status/503" => fixed(
            &mut stream,
            "503 Service Unavailable",
            "Content-Type: application/json\r\n",
            br#"{"errors":[{"message":"upstream says no","extensions":{"code":"BYTE_ROUTE_UPSTREAM_UNAVAILABLE"}}]}"#,
        ),
        "/files/close-delimited" => {
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\npartial",
            );
        }
        "/files/upgrade" => {
            let _ = stream.write_all(
                b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
            );
        }
        "/files/not-modified" => {
            let _ = stream.write_all(b"HTTP/1.1 304 Not Modified\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n");
        }
        "/files/slow-headers" => {
            thread::sleep(Duration::from_secs(2));
            fixed(&mut stream, "200 OK", "", b"late");
        }
        "/files/slow-body" => {
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nhello",
            );
            let _ = stream.flush();
            thread::sleep(Duration::from_secs(2));
        }
        "/files/drip" => {
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            );
            for _ in 0..100 {
                if stream.write_all(b"1\r\nx\r\n").is_err() || stream.flush().is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
            let _ = stream.write_all(b"0\r\n\r\n");
        }
        _ => fixed(&mut stream, "404 Not Found", "", b"missing"),
    }
}

fn received_header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|line| {
        line.split_once(':')
            .filter(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim())
    })
}

#[test]
fn byte_routes_stream_large_and_chunked_bodies_without_buffering() {
    let fixture = Fixture::start();
    let bearer = [("authorization", "Bearer product-p1")];

    // A chunked upstream body streams through: the client receives the first
    // chunk while the upstream is still holding the rest back.
    let mut stream = connect(fixture.router.address);
    stream
        .write_all(
            b"GET /files/gated HTTP/1.1\r\nHost: router.test\r\nConnection: close\r\nAuthorization: Bearer product-p1\r\n\r\n",
        )
        .unwrap();
    let (head, mut early) = read_head(&mut stream);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let mut buffer = [0_u8; 256];
    while !early.windows(5).any(|window| window == b"first") {
        let read = stream
            .read(&mut buffer)
            .expect("first chunk before release");
        assert_ne!(read, 0, "stream closed before the first chunk");
        early.extend_from_slice(&buffer[..read]);
    }
    fixture.upstream.gate.release();
    let mut rest = early;
    stream.read_to_end(&mut rest).expect("rest of gated body");
    let text = String::from_utf8_lossy(&rest);
    assert!(
        text.contains("second") && text.ends_with("0\r\n\r\n"),
        "{text}"
    );

    // A large body with a declared length keeps that length, arrives intact,
    // and does not raise the router's peak memory by anything like its size.
    let peak_before = fixture.child_memory("VmHWM:");
    let path = format!("/files/big?size={LARGE_BODY_BYTES}");
    let response = fixture.get(&path, &bearer);
    assert_eq!(response.status, 200);
    assert!(response.complete);
    assert_eq!(
        response.header("content-length"),
        Some(LARGE_BODY_BYTES.to_string().as_str())
    );
    assert_eq!(
        response.header("content-type"),
        Some("application/octet-stream")
    );
    assert_eq!(
        response.header("content-disposition"),
        Some("attachment; filename=\"big.bin\"")
    );
    assert_eq!(response.body.len(), LARGE_BODY_BYTES);
    assert!(
        response
            .body
            .iter()
            .enumerate()
            .all(|(index, byte)| *byte == ((index % (64 * 1024)) % 251) as u8)
    );
    let peak_after = fixture.child_memory("VmHWM:");
    assert!(
        peak_after.saturating_sub(peak_before) < MAX_STREAMING_PEAK_GROWTH_BYTES,
        "router peak memory grew from {peak_before} to {peak_after} bytes while streaming"
    );

    // HEAD forwards as HEAD and keeps the upstream length without a body.
    let response = fixture.request("HEAD", "/files/big?size=1000", &bearer);
    assert_eq!(response.status, 200);
    assert_eq!(response.header("content-length"), Some("1000"));
    assert!(
        fixture
            .upstream
            .requests()
            .last()
            .unwrap()
            .starts_with("HEAD /files/big?size=1000 ")
    );

    // 304 passes through without a body.
    let response = fixture.get("/files/not-modified", &bearer);
    assert_eq!(response.status, 304);
    assert_eq!(response.header("etag"), Some("\"v1\""));

    let metrics = fixture.metrics();
    assert!(
        metrics["router_byte_route_bytes_total"]["/files"]
            .as_u64()
            .unwrap()
            >= LARGE_BODY_BYTES as u64 + 11
    );
    assert_eq!(metrics["router_byte_route_requests_total"]["/files"], 4);
    assert_eq!(metrics["router_byte_route_active_requests"]["/files"], 0);
    assert_eq!(metrics["router_byte_route_aborted_total"]["/files"], 0);
}

#[test]
fn byte_routes_authenticate_and_allow_list_headers() {
    let fixture = Fixture::start();

    // Authenticated by default, with the GraphQL endpoint's error shape, and
    // the upstream is never contacted.
    for (headers, status, message) in [
        (vec![], 401, "authentication is required"),
        (
            vec![("authorization", "Bearer wrong")],
            401,
            "invalid bearer credential",
        ),
        (
            vec![("authorization", "Basic dXNlcjpwYXNz")],
            401,
            "invalid bearer credential",
        ),
        (
            vec![
                ("authorization", "Bearer product-p1"),
                ("authorization", "Bearer product-admin"),
            ],
            401,
            "invalid bearer credential",
        ),
    ] {
        let response = fixture.get("/files/echo", &headers);
        assert_eq!(response.status, status, "{headers:?}");
        assert_eq!(response.router_error(), Some("UNAUTHENTICATED"));
        let value: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(value["errors"][0]["message"], message);
        assert_eq!(value["errors"][0]["extensions"]["code"], "UNAUTHENTICATED");
    }
    let graphql = fixture.router.graphql_response_at(
        "/api/graphql",
        json!({"query": "{ __typename }"}),
        &[("authorization", "Bearer wrong")],
    );
    assert_eq!(graphql.0, 401, "byte routes mirror the GraphQL endpoint");
    assert_eq!(fixture.upstream.request_count(), 0);

    // The verified bearer is forwarded verbatim, client identity and router
    // headers are not, and only allow-listed request headers cross.
    let response = fixture.get(
        "/files/echo?x=1",
        &[
            ("authorization", "Bearer product-p1"),
            ("accept", "application/octet-stream"),
            ("if-none-match", "\"v0\""),
            ("cookie", "session=client"),
            ("x-forwarded-for", "203.0.113.9"),
            ("x-forwarded-host", "evil.test"),
            ("forwarded", "for=203.0.113.9"),
            ("x-user-id", "someone-else"),
            ("x-graphql-orm-router-internal", "guess"),
            ("x-request-id", "not-on-this-route"),
            ("proxy-authorization", "Basic abc"),
            ("te", "trailers"),
            ("user-agent", "client"),
        ],
    );
    assert_eq!(response.status, 200, "{}", response.text());
    let received = response.text();
    assert!(
        received.starts_with("GET /files/echo?x=1 HTTP/1.1"),
        "{received}"
    );
    assert_eq!(
        received_header(&received, "authorization"),
        Some("Bearer product-p1")
    );
    assert_eq!(
        received_header(&received, "host"),
        Some(fixture.upstream.address.to_string().as_str())
    );
    assert_eq!(
        received_header(&received, "accept"),
        Some("application/octet-stream")
    );
    assert_eq!(received_header(&received, "if-none-match"), Some("\"v0\""));
    for name in [
        "cookie",
        "x-forwarded-for",
        "x-forwarded-host",
        "forwarded",
        "x-user-id",
        "x-graphql-orm-router-internal",
        "x-request-id",
        "proxy-authorization",
        "te",
        "user-agent",
        "range",
    ] {
        assert_eq!(
            received_header(&received, name),
            None,
            "{name} must not be forwarded"
        );
    }

    // Only allow-listed response headers come back; Set-Cookie and unknown
    // headers are dropped, and the router adds its own content policy.
    assert_eq!(response.header("content-type"), Some("text/plain"));
    assert_eq!(response.header("content-disposition"), Some("inline"));
    assert_eq!(
        response.header("cache-control"),
        Some("private, max-age=60")
    );
    assert_eq!(response.header("etag"), Some("\"v1\""));
    assert_eq!(response.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(
        response.header("content-security-policy"),
        Some("default-src 'none'; sandbox")
    );
    assert_eq!(response.header("accept-ranges"), Some("none"));
    for name in ["set-cookie", "x-checksum", "x-internal-secret", "server"] {
        assert_eq!(response.header(name), None, "{name} must not pass through");
    }
    assert_eq!(response.router_error(), None);

    // A deliberately public route needs no bearer and forwards none; a bad
    // bearer is still refused exactly as on GraphQL.
    let response = fixture.get("/public/echo", &[("cookie", "session=client")]);
    assert_eq!(response.status, 200);
    assert_eq!(received_header(&response.text(), "authorization"), None);
    assert_eq!(received_header(&response.text(), "cookie"), None);
    let response = fixture.get("/public/echo", &[("authorization", "Bearer product-p1")]);
    assert_eq!(
        received_header(&response.text(), "authorization"),
        Some("Bearer product-p1")
    );
    let response = fixture.get("/public/echo", &[("authorization", "Bearer wrong")]);
    assert_eq!(response.status, 401);
    assert_eq!(response.router_error(), Some("UNAUTHENTICATED"));

    // Range is refused without opt-in, before the upstream is contacted.
    let before = fixture.upstream.request_count();
    let response = fixture.get(
        "/files/range",
        &[
            ("authorization", "Bearer product-p1"),
            ("range", "bytes=0-3"),
        ],
    );
    assert_eq!(response.status, 416);
    assert_eq!(response.error_code(), "BYTE_ROUTE_RANGE_NOT_ALLOWED");
    assert_eq!(response.header("accept-ranges"), Some("none"));
    assert_eq!(fixture.upstream.request_count(), before);

    // An opted-in route forwards Range and its configured extras.
    let response = fixture.get(
        "/ranged/range",
        &[
            ("authorization", "Bearer product-p1"),
            ("range", "bytes=0-3"),
        ],
    );
    assert_eq!(response.status, 206);
    assert_eq!(response.body, b"0123");
    assert_eq!(response.header("content-range"), Some("bytes 0-3/10"));
    assert_eq!(response.header("accept-ranges"), Some("bytes"));
    let response = fixture.get(
        "/ranged/echo",
        &[
            ("authorization", "Bearer product-p1"),
            ("x-request-id", "trace-1"),
        ],
    );
    assert_eq!(
        received_header(&response.text(), "x-request-id"),
        Some("trace-1")
    );
    assert_eq!(response.header("set-cookie"), Some("session=upstream"));
    assert_eq!(response.header("x-checksum"), Some("abc123"));
    assert_eq!(response.header("x-internal-secret"), None);

    // Active content from the upstream is never sniffed or run on the
    // router's origin.
    let response = fixture.get("/files/html", &[("authorization", "Bearer product-p1")]);
    assert_eq!(response.header("content-type"), Some("text/html"));
    assert_eq!(response.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(
        response.header("content-security-policy"),
        Some("default-src 'none'; sandbox")
    );

    // Upstream errors pass through with status, allowed headers, and body,
    // and carry no router error marker even when the body imitates one.
    let response = fixture.get(
        "/files/status/418",
        &[("authorization", "Bearer product-p1")],
    );
    assert_eq!(response.status, 418);
    assert_eq!(response.body, b"short and stout");
    assert_eq!(response.header("retry-after"), Some("7"));
    assert_eq!(response.router_error(), None);
    let response = fixture.get(
        "/files/status/503",
        &[("authorization", "Bearer product-p1")],
    );
    assert_eq!(response.status, 503);
    assert!(response.text().contains("upstream says no"));
    assert_eq!(response.router_error(), None);
    let response = fixture.get("/files/missing", &[("authorization", "Bearer product-p1")]);
    assert_eq!(response.status, 404);
    assert_eq!(response.body, b"missing");
    assert_eq!(response.router_error(), None);
}

#[test]
fn byte_routes_refuse_unsafe_requests_before_the_upstream() {
    let fixture = Fixture::start();
    let bearer = ("authorization", "Bearer product-p1");

    for method in ["POST", "PUT", "PATCH", "DELETE", "OPTIONS", "TRACE"] {
        let response = fixture.request(method, "/files/echo", &[bearer]);
        assert_eq!(response.status, 405, "{method}");
        assert_eq!(response.error_code(), "BYTE_ROUTE_METHOD_NOT_ALLOWED");
        assert_eq!(response.header("allow"), Some("GET, HEAD"));
    }

    // HTTP/1.0 cannot tell an aborted unknown-length body from a complete one.
    let response = send(
        fixture.router.address,
        b"GET /files/echo HTTP/1.0\r\nHost: router.test\r\nAuthorization: Bearer product-p1\r\n\r\n",
        false,
    );
    assert_eq!(response.status, 505);
    assert_eq!(
        response.error_code(),
        "BYTE_ROUTE_HTTP_VERSION_NOT_SUPPORTED"
    );

    // A request body is refused whether framed by length or by chunks.
    let response = send(
        fixture.router.address,
        b"GET /files/echo HTTP/1.1\r\nHost: router.test\r\nConnection: close\r\nAuthorization: Bearer product-p1\r\nContent-Length: 5\r\n\r\nhello",
        false,
    );
    assert_eq!(response.status, 400);
    assert_eq!(response.error_code(), "BYTE_ROUTE_REQUEST_BODY_NOT_ALLOWED");
    let response = send(
        fixture.router.address,
        b"GET /files/echo HTTP/1.1\r\nHost: router.test\r\nConnection: close\r\nAuthorization: Bearer product-p1\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
        false,
    );
    assert_eq!(response.status, 400);
    assert_eq!(response.error_code(), "BYTE_ROUTE_REQUEST_BODY_NOT_ALLOWED");

    // Traversal, dot segments, double slashes, and encoded separators under a
    // prefix are refused by the route and never fall through to GraphQL.
    for path in [
        "/files/",
        "/files/../api/graphql",
        "/files/./echo",
        "/files//echo",
        "/files/%2e%2e/api/graphql",
        "/files/%2E%2E/api/graphql",
        "/files/.%2e/echo",
        "/files/a%2fb",
        "/files/a%5cb",
        "/files/a%00b",
        "/files/a%252e%252e",
        "/files/..;/echo",
    ] {
        let response = fixture.get(path, &[bearer]);
        assert_eq!(response.status, 400, "{path}: {}", response.text());
        assert_eq!(response.error_code(), "BYTE_ROUTE_INVALID_PATH", "{path}");
    }
    // A sibling path that only shares the prefix's characters is not claimed.
    for path in [
        "/files-admin",
        "/files-admin/echo",
        "/filesecho",
        "/FILES/echo",
    ] {
        let response = fixture.get(path, &[bearer]);
        assert_eq!(response.status, 404, "{path}");
        assert_eq!(response.router_error(), None, "{path}");
    }
    // An absolute-form target cannot steer the upstream authority.
    let response = fixture.get("http://evil.test:9/files/echo", &[bearer]);
    assert_eq!(response.status, 200, "{}", response.text());
    assert_eq!(
        received_header(&response.text(), "host"),
        Some(fixture.upstream.address.to_string().as_str())
    );
    let upstream_requests = fixture.upstream.requests();
    assert_eq!(upstream_requests.len(), 1, "{upstream_requests:?}");

    // Rate limit: one admission per second on this route. Refused anonymous
    // callers do not spend it.
    for _ in 0..3 {
        assert_eq!(fixture.get("/limited/echo", &[]).status, 401);
    }
    let first = fixture.get("/limited/echo", &[bearer]);
    assert_eq!(first.status, 200);
    let second = fixture.get("/limited/echo", &[bearer]);
    assert_eq!(second.status, 429);
    assert_eq!(second.error_code(), "BYTE_ROUTE_RATE_LIMITED");
    assert_eq!(second.header("retry-after"), Some("1"));

    // Concurrency: a held download occupies the only slot.
    let mut held = connect(fixture.router.address);
    held.write_all(
        b"GET /single/gated HTTP/1.1\r\nHost: router.test\r\nConnection: close\r\nAuthorization: Bearer product-p1\r\n\r\n",
    )
    .unwrap();
    let (head, _) = read_head(&mut held);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let busy = fixture.get("/single/echo", &[bearer]);
    assert_eq!(busy.status, 503);
    assert_eq!(busy.error_code(), "BYTE_ROUTE_BUSY");
    fixture.upstream.gate.release();
    let mut rest = Vec::new();
    held.read_to_end(&mut rest).unwrap();
    wait_until("the concurrency slot to be released", || {
        fixture.metrics()["router_byte_route_active_requests"]["/single"] == 0
    });
    assert_eq!(fixture.get("/single/echo", &[bearer]).status, 200);

    // The GraphQL endpoint is untouched by the byte-route middleware.
    let (status, value) = fixture.router.graphql_response_at(
        "/api/graphql",
        json!({"query": "{ __typename }"}),
        &[bearer],
    );
    assert_eq!(status, 200, "{value}");
    assert_eq!(value["data"]["__typename"], "Query");

    let metrics = fixture.metrics();
    assert!(
        metrics["router_byte_route_rejected_total"]["/files"]
            .as_u64()
            .unwrap()
            >= 20
    );
    assert_eq!(metrics["router_byte_route_rejected_total"]["/limited"], 4);
    assert_eq!(metrics["router_byte_route_rejected_total"]["/single"], 1);
}

#[test]
fn byte_routes_enforce_limits_and_report_upstream_failures() {
    let fixture = Fixture::start();
    let bearer = [("authorization", "Bearer product-p1")];

    // A declared length over the limit is refused before any body is sent.
    let response = fixture.get("/small/big?size=4096", &bearer);
    assert_eq!(response.status, 502);
    assert_eq!(response.error_code(), "BYTE_ROUTE_RESPONSE_TOO_LARGE");
    assert_eq!(
        response.router_error(),
        Some("BYTE_ROUTE_RESPONSE_TOO_LARGE")
    );
    let response = fixture.request("HEAD", "/small/big?size=4096", &bearer);
    assert_eq!(response.status, 502);
    // Within the limit it streams normally.
    let response = fixture.get("/small/big?size=1024", &bearer);
    assert_eq!(response.status, 200);
    assert!(response.complete);
    assert_eq!(response.body.len(), 1024);

    // An undeclared body that grows past the limit is aborted mid-stream:
    // the client never sees a complete transfer.
    let response = fixture.get("/small/chunked?size=8192", &bearer);
    assert_eq!(response.status, 200);
    assert!(
        !response.complete,
        "an oversized stream must not look complete"
    );
    assert!(response.body.len() <= 1024, "{}", response.body.len());

    // Waiting for headers is bounded by the idle timeout.
    let started = Instant::now();
    let response = fixture.get("/slow/slow-headers", &bearer);
    assert_eq!(response.status, 504);
    assert_eq!(response.error_code(), "BYTE_ROUTE_UPSTREAM_TIMEOUT");
    assert!(started.elapsed() < Duration::from_millis(1_500));

    // A stalled body is aborted after the idle timeout.
    let started = Instant::now();
    let response = fixture.get("/slow/slow-body", &bearer);
    assert_eq!(response.status, 200);
    assert!(!response.complete);
    assert_eq!(response.body, b"hello");
    assert!(started.elapsed() < Duration::from_millis(1_500));

    // A steady drip that never idles still ends at the request deadline.
    let started = Instant::now();
    let response = fixture.get("/slow/drip", &bearer);
    assert_eq!(response.status, 200);
    assert!(!response.complete);
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(1_500) && elapsed < Duration::from_secs(4),
        "{elapsed:?}"
    );

    // A client that stops reading is disconnected after the idle timeout, and
    // its concurrency slot and upstream connection are released while it is
    // still not reading. The route's deadline is far away, so only the stall
    // timeout can end this request.
    let started = Instant::now();
    let mut stalled = connect(fixture.router.address);
    stalled
        .write_all(
            b"GET /stall/big?size=33554432 HTTP/1.1\r\nHost: router.test\r\nConnection: close\r\nAuthorization: Bearer product-p1\r\n\r\n",
        )
        .unwrap();
    let (head, _) = read_head(&mut stalled);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    wait_until("the stalled client to be disconnected", || {
        let metrics = fixture.metrics();
        metrics["router_byte_route_aborted_total"]["/stall"] == 1
            && metrics["router_byte_route_active_requests"]["/stall"] == 0
    });
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    let mut drained = Vec::new();
    let _ = stalled.read_to_end(&mut drained);
    assert!(
        drained.len() < 33_554_432,
        "a stalled client must not receive the whole body"
    );

    // A body framed only by the upstream closing its connection cannot show
    // truncation, so it is refused rather than relayed as complete.
    let response = fixture.get("/small/close-delimited", &bearer);
    assert_eq!(response.status, 502, "{}", response.text());
    assert_eq!(response.error_code(), "BYTE_ROUTE_UPSTREAM_UNAVAILABLE");

    // An upstream upgrade response is never relayed.
    let response = fixture.get("/down/../files/upgrade", &bearer);
    assert_eq!(response.status, 400);
    let response = fixture.get("/small/upgrade", &bearer);
    assert_eq!(response.status, 502, "{}", response.text());
    assert_eq!(response.error_code(), "BYTE_ROUTE_UPSTREAM_UNAVAILABLE");

    // An unreachable upstream is a distinct router error.
    let response = fixture.get("/down/anything", &bearer);
    assert_eq!(response.status, 502);
    assert_eq!(response.error_code(), "BYTE_ROUTE_UPSTREAM_UNAVAILABLE");
    assert_eq!(
        response.router_error(),
        Some("BYTE_ROUTE_UPSTREAM_UNAVAILABLE")
    );

    let metrics = fixture.metrics();
    for (name, route, expected) in [
        ("router_byte_route_requests_total", "/small", 6),
        ("router_byte_route_upstream_errors_total", "/small", 4),
        ("router_byte_route_aborted_total", "/small", 1),
        ("router_byte_route_requests_total", "/slow", 3),
        ("router_byte_route_upstream_errors_total", "/slow", 1),
        ("router_byte_route_aborted_total", "/slow", 2),
        ("router_byte_route_requests_total", "/stall", 1),
        ("router_byte_route_active_requests", "/stall", 0),
        ("router_byte_route_requests_total", "/down", 2),
        ("router_byte_route_upstream_errors_total", "/down", 1),
        ("router_byte_route_active_requests", "/slow", 0),
        ("router_byte_route_requests_total", "/public", 0),
    ] {
        assert_eq!(
            metrics[name][route], expected,
            "{name}{{{route}}}: {metrics}"
        );
    }
    // The aborted stream may have handed over up to the limit before abort.
    let small_bytes = metrics["router_byte_route_bytes_total"]["/small"]
        .as_u64()
        .unwrap();
    assert!((1024..=2048).contains(&small_bytes), "{small_bytes}");
}
