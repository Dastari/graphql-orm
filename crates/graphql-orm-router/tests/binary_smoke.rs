#![cfg(target_family = "unix")]

use std::{
    env, fs,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::Engine;
use graphql_orm_router_protocol::{
    AdvertisedEndpoint, ArgumentDescriptor, AuthorizationRequirement, CapabilitySet,
    DescriptorFingerprints, Fingerprint, GraphqlEndpoints, OperationDescriptor, ProtocolVersion,
    RootOperationType, SchemaAdvertisement, SubgraphDescriptor, SubgraphId, SubgraphIdentity,
    SubgraphName,
};
use jsonwebtoken::{
    Algorithm, EncodingKey, Header, encode,
    jwk::{Jwk, JwkSet, PublicKeyUse},
};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

const TIMEOUT: Duration = Duration::from_secs(10);
const SDL: &str = "type Query { hello: String! } type Subscription { tick: String! }";
const RSA_PRIVATE_KEY: &str = include_str!("fixtures/test-rsa-private.pem");
const TOKEN_ISSUER: &str = "https://binary-smoke.test";
const TOKEN_AUDIENCE: &str = "graphql-router";
const TOKEN_KID: &str = "binary-smoke-key";

#[test]
fn executable_routes_http_and_websocket_then_handles_sigterm() {
    let subgraph = FixtureSubgraph::start();
    let token = bearer_token();
    let router_address = reserve_address();
    let config = TemporaryConfig::new(format!(
        r#"{{
          "listener": "{router_address}",
          "graphqlPath": "/graphql",
          "authentication": {{
            "jwksUrl": "{}/jwks",
            "issuer": "{TOKEN_ISSUER}",
            "audiences": ["{TOKEN_AUDIENCE}"],
            "allowInsecureLoopbackJwks": true
          }},
          "gracefulShutdownTimeoutSeconds": 1,
          "publicRequestTimeoutMs": 1000,
          "subgraphRequestTimeoutMs": 100,
          "subgraphs": [{{
            "name": "hello",
            "graphqlUrl": "{}/graphql",
            "sdlUrl": "{}/sdl",
            "protocolUrl": "{}/.well-known/graphql-router"
          }}],
          "subscriptions": {{
            "maxConnections": 4,
            "maxOperationsPerConnection": 4,
            "broadcastCapacity": 4,
            "subgraphBufferCapacity": 4
          }},
          "telemetry": {{"logLevel": "debug", "textLogsForDevelopment": true}}
        }}"#,
        subgraph.origin(),
        subgraph.origin(),
        subgraph.origin(),
        subgraph.origin(),
    ));
    let checked = Command::new(env!("CARGO_BIN_EXE_graphql-orm-router"))
        .arg("--config")
        .arg(config.path())
        .arg("--check")
        .output()
        .expect("router executable check should run");
    assert!(
        checked.status.success(),
        "check failed: {}",
        String::from_utf8_lossy(&checked.stderr)
    );
    assert!(String::from_utf8_lossy(&checked.stdout).contains("configuration ready"));
    assert!(
        TcpListener::bind(router_address).is_ok(),
        "--check must not bind the public listener"
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_graphql-orm-router"))
        .arg("--config")
        .arg(config.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("router executable should start");

    wait_until_ready(router_address, &mut child);
    let authorization = format!("Bearer {token}");
    let timed_out = http(
        router_address,
        "POST",
        "/graphql",
        &[("authorization", authorization.as_str())],
        br#"{"query":"query { hello }"}"#,
    )
    .expect("timed-out router GraphQL response");
    assert_eq!(timed_out.0, 200);
    let timed_out: Value = serde_json::from_slice(&timed_out.1).unwrap();
    assert!(
        timed_out["errors"].is_array(),
        "the delayed subgraph should time out: {timed_out}"
    );

    let response = http(
        router_address,
        "POST",
        "/graphql",
        &[("authorization", authorization.as_str())],
        br#"{"query":"query { hello }"}"#,
    )
    .expect("router GraphQL response");
    assert_eq!(response.0, 200, "{}", String::from_utf8_lossy(&response.1));
    let body: serde_json::Value = serde_json::from_slice(&response.1)
        .unwrap_or_else(|error| panic!("invalid router JSON {error}: {:?}", response.1));
    assert_eq!(body["data"]["hello"], "world");

    let mut websocket = TestWebSocket::connect(router_address);
    websocket.send_json(&json!({
        "type": "connection_init",
        "payload": {"authorization": format!("Bearer {token}")}
    }));
    assert_eq!(websocket.next_json()["type"], "connection_ack");
    websocket.send_json(&json!({
        "id": "smoke",
        "type": "subscribe",
        "payload": {"query": "subscription { tick }"}
    }));
    let event = websocket.next_json();
    assert_eq!(event["id"], "smoke");
    assert_eq!(event["type"], "next");
    assert_eq!(event["payload"]["data"]["tick"], "world");
    websocket.close();

    let signal = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("SIGTERM command should run");
    assert!(signal.success());
    let status = wait_for_exit(&mut child);
    assert!(status.success(), "router exited with {status}");
    assert!(TcpStream::connect_timeout(&router_address, Duration::from_millis(100)).is_err());
}

fn wait_for_exit(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("router child status") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let mut stdout = String::new();
            let mut stderr = String::new();
            if let Some(mut output) = child.stdout.take() {
                let _ = output.read_to_string(&mut stdout);
            }
            if let Some(mut output) = child.stderr.take() {
                let _ = output.read_to_string(&mut stderr);
            }
            panic!(
                "router did not exit within {TIMEOUT:?} after SIGTERM\nstdout: {stdout}\nstderr: {stderr}"
            );
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn wait_until_ready(address: SocketAddr, child: &mut Child) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if http(address, "GET", "/readiness", &[], &[]).is_ok_and(|response| response.0 == 200) {
            return;
        }
        if let Some(status) = child.try_wait().expect("router child status") {
            panic!("router exited before readiness with {status}");
        }
        assert!(Instant::now() < deadline, "router readiness timed out");
        thread::sleep(Duration::from_millis(25));
    }
}

fn reserve_address() -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.local_addr().unwrap()
}

fn http(
    address: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> std::io::Result<(u16, Vec<u8>)> {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(1))?;
    stream.set_read_timeout(Some(TIMEOUT))?;
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
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes)?;
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| std::io::Error::other("HTTP response has no header boundary"))?;
    let head = std::str::from_utf8(&bytes[..split])
        .map_err(|_| std::io::Error::other("HTTP response head is not UTF-8"))?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| std::io::Error::other("HTTP response status is invalid"))?;
    let body = &bytes[split + 4..];
    let body = if head.lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("transfer-encoding")
                && value.trim().eq_ignore_ascii_case("chunked")
        })
    }) {
        decode_chunked(body)?
    } else {
        let length = head.lines().find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
        });
        body[..length.unwrap_or(body.len()).min(body.len())].to_vec()
    };
    Ok((status, body))
}

fn bearer_token() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(TOKEN_KID.to_owned());
    encode(
        &header,
        &json!({
            "sub": "binary-smoke-client",
            "iss": TOKEN_ISSUER,
            "aud": [TOKEN_AUDIENCE],
            "nbf": now.saturating_sub(10),
            "exp": now + 3_600,
            "scope": "router.smoke"
        }),
        &EncodingKey::from_rsa_pem(RSA_PRIVATE_KEY.as_bytes()).unwrap(),
    )
    .unwrap()
}

fn jwks_document() -> Vec<u8> {
    let key = EncodingKey::from_rsa_pem(RSA_PRIVATE_KEY.as_bytes()).unwrap();
    let mut jwk = Jwk::from_encoding_key(&key, Algorithm::RS256).unwrap();
    jwk.common.key_id = Some(TOKEN_KID.to_owned());
    jwk.common.public_key_use = Some(PublicKeyUse::Signature);
    serde_json::to_vec(&JwkSet { keys: vec![jwk] }).unwrap()
}

fn descriptor_document(address: SocketAddr) -> Vec<u8> {
    let endpoint = |path: &str| {
        AdvertisedEndpoint::try_from(format!("http://{address}{path}"))
            .expect("fixture endpoint is valid")
    };
    let mut descriptor = SubgraphDescriptor {
        protocol_version: ProtocolVersion { major: 1, minor: 0 },
        subgraph: SubgraphIdentity {
            id: SubgraphId::try_from("hello-service".to_owned()).unwrap(),
            name: SubgraphName::try_from("hello".to_owned()).unwrap(),
        },
        graphql: GraphqlEndpoints {
            http: endpoint("/graphql"),
            websocket: None,
        },
        schema: SchemaAdvertisement {
            url: endpoint("/sdl"),
        },
        capabilities: CapabilitySet {
            subscriptions: true,
            authorization_metadata: true,
            schema_fingerprints: true,
        },
        required_semantics: vec!["authorizationMetadata".to_owned()],
        operations: vec![
            OperationDescriptor {
                root_type: RootOperationType::Query,
                field_name: "hello".to_owned(),
                arguments: Vec::<ArgumentDescriptor>::new(),
                authorization: AuthorizationRequirement::Public,
            },
            OperationDescriptor {
                root_type: RootOperationType::Subscription,
                field_name: "tick".to_owned(),
                arguments: Vec::<ArgumentDescriptor>::new(),
                authorization: AuthorizationRequirement::Authenticated,
            },
        ],
        fingerprints: DescriptorFingerprints {
            schema: Fingerprint::sha256(SDL),
            authorization: Fingerprint::sha256("pending"),
            combined: Fingerprint::sha256("pending"),
        },
    };
    descriptor.fingerprints.authorization = descriptor.authorization_fingerprint();
    descriptor.fingerprints.combined = descriptor.combined_fingerprint();
    serde_json::to_vec(&descriptor).unwrap()
}

fn decode_chunked(mut input: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    loop {
        let boundary = input
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| std::io::Error::other("chunk size is incomplete"))?;
        let size = std::str::from_utf8(&input[..boundary])
            .ok()
            .and_then(|value| usize::from_str_radix(value, 16).ok())
            .ok_or_else(|| std::io::Error::other("chunk size is invalid"))?;
        input = &input[boundary + 2..];
        if size == 0 {
            return Ok(output);
        }
        if input.len() < size + 2 || &input[size..size + 2] != b"\r\n" {
            return Err(std::io::Error::other("chunk body is incomplete"));
        }
        output.extend_from_slice(&input[..size]);
        input = &input[size + 2..];
    }
}

struct FixtureSubgraph {
    address: SocketAddr,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FixtureSubgraph {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stopping = Arc::new(AtomicBool::new(false));
        let graphql_requests = Arc::new(AtomicUsize::new(0));
        let thread_stopping = stopping.clone();
        let thread_graphql_requests = graphql_requests.clone();
        let thread = thread::spawn(move || {
            while !thread_stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        handle_fixture_request(stream, address, thread_graphql_requests.clone())
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            address,
            stopping,
            thread: Some(thread),
        }
    }

    fn origin(&self) -> String {
        format!("http://{}", self.address)
    }
}

impl Drop for FixtureSubgraph {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn handle_fixture_request(
    mut stream: TcpStream,
    address: SocketAddr,
    graphql_requests: Arc<AtomicUsize>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let Ok(request) = read_http_head(&mut stream) else {
        return;
    };
    if request.starts_with("GET /graphql ")
        && header_value(&request, "upgrade")
            .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
    {
        let _ = handle_upstream_websocket(stream, &request);
        return;
    }
    let (content_type, body) = if request.starts_with("GET /sdl ") {
        ("text/plain", SDL.as_bytes().to_vec())
    } else if request.starts_with("GET /jwks ") {
        ("application/json", jwks_document())
    } else if request.starts_with("GET /.well-known/graphql-router ") {
        ("application/json", descriptor_document(address))
    } else if request.starts_with("POST /graphql ") {
        if graphql_requests.fetch_add(1, Ordering::AcqRel) == 0 {
            thread::sleep(Duration::from_millis(250));
        }
        (
            "application/json",
            br#"{"data":{"hello":"world"}}"#.to_vec(),
        )
    } else {
        ("text/plain", b"not found".to_vec())
    };
    let status = if request.starts_with("GET /sdl ")
        || request.starts_with("GET /jwks ")
        || request.starts_with("GET /.well-known/graphql-router ")
        || request.starts_with("POST /graphql ")
    {
        "200 OK"
    } else {
        "404 Not Found"
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(&body);
}

fn read_http_head(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        }
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(boundary) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
            return String::from_utf8(bytes[..boundary].to_vec())
                .map_err(|_| std::io::Error::other("HTTP head is not UTF-8"));
        }
        if bytes.len() > 64 * 1024 {
            return Err(std::io::Error::other("HTTP head is too large"));
        }
    }
}

fn header_value<'a>(head: &'a str, expected: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|line| {
        line.split_once(':')
            .and_then(|(name, value)| name.eq_ignore_ascii_case(expected).then_some(value.trim()))
    })
}

fn handle_upstream_websocket(mut stream: TcpStream, request: &str) -> std::io::Result<()> {
    let key = header_value(request, "sec-websocket-key")
        .ok_or_else(|| std::io::Error::other("WebSocket key is missing"))?;
    let mut digest = Sha1::new();
    digest.update(key.as_bytes());
    digest.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    let accept = base64::engine::general_purpose::STANDARD.encode(digest.finalize());
    write!(
        stream,
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\nSec-WebSocket-Protocol: graphql-transport-ws\r\n\r\n"
    )?;
    stream.flush()?;

    loop {
        let (opcode, payload) = read_websocket_frame(&mut stream, true)?;
        match opcode {
            0x1 => {
                let message: Value = serde_json::from_slice(&payload)
                    .map_err(|_| std::io::Error::other("invalid WebSocket JSON"))?;
                match message["type"].as_str() {
                    Some("connection_init") => write_websocket_frame(
                        &mut stream,
                        0x1,
                        serde_json::to_string(&json!({"type": "connection_ack"}))?.as_bytes(),
                        false,
                    )?,
                    Some("subscribe") => {
                        let id = message["id"].as_str().unwrap_or("smoke");
                        let event = json!({
                            "id": id,
                            "type": "next",
                            "payload": {"data": {"tick": "world"}}
                        });
                        write_websocket_frame(
                            &mut stream,
                            0x1,
                            serde_json::to_string(&event)?.as_bytes(),
                            false,
                        )?;
                    }
                    Some("complete") => {
                        let event = json!({"id": message["id"], "type": "complete"});
                        write_websocket_frame(
                            &mut stream,
                            0x1,
                            serde_json::to_string(&event)?.as_bytes(),
                            false,
                        )?;
                    }
                    _ => {}
                }
            }
            0x8 => {
                write_websocket_frame(&mut stream, 0x8, &payload, false)?;
                return Ok(());
            }
            0x9 => write_websocket_frame(&mut stream, 0xA, &payload, false)?,
            _ => {}
        }
    }
}

struct TestWebSocket {
    stream: TcpStream,
}

impl TestWebSocket {
    fn connect(address: SocketAddr) -> Self {
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2)).unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        stream.set_write_timeout(Some(TIMEOUT)).unwrap();
        write!(
            stream,
            "GET /graphql HTTP/1.1\r\nHost: {address}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: graphql-transport-ws\r\n\r\n"
        )
        .unwrap();
        let head = read_http_head(&mut stream).unwrap();
        assert!(head.starts_with("HTTP/1.1 101 "), "{head}");
        Self { stream }
    }

    fn send_json(&mut self, value: &Value) {
        write_websocket_frame(
            &mut self.stream,
            0x1,
            serde_json::to_string(value).unwrap().as_bytes(),
            true,
        )
        .unwrap();
    }

    fn next_json(&mut self) -> Value {
        loop {
            let (opcode, payload) = read_websocket_frame(&mut self.stream, false).unwrap();
            match opcode {
                0x1 => return serde_json::from_slice(&payload).unwrap(),
                0x9 => write_websocket_frame(&mut self.stream, 0xA, &payload, true).unwrap(),
                0x8 => panic!("router closed before the expected WebSocket message"),
                _ => {}
            }
        }
    }

    fn close(&mut self) {
        let _ = write_websocket_frame(&mut self.stream, 0x8, &[], true);
    }
}

fn write_websocket_frame(
    stream: &mut TcpStream,
    opcode: u8,
    payload: &[u8],
    masked: bool,
) -> std::io::Result<()> {
    const MASK: [u8; 4] = [0x13, 0x37, 0xc0, 0xde];
    stream.write_all(&[0x80 | opcode])?;
    let mask_bit = if masked { 0x80 } else { 0 };
    match payload.len() {
        0..=125 => stream.write_all(&[mask_bit | payload.len() as u8])?,
        126..=65_535 => {
            stream.write_all(&[mask_bit | 126])?;
            stream.write_all(&(payload.len() as u16).to_be_bytes())?;
        }
        _ => {
            stream.write_all(&[mask_bit | 127])?;
            stream.write_all(&(payload.len() as u64).to_be_bytes())?;
        }
    }
    if masked {
        stream.write_all(&MASK)?;
        let encoded = payload
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ MASK[index % MASK.len()])
            .collect::<Vec<_>>();
        stream.write_all(&encoded)?;
    } else {
        stream.write_all(payload)?;
    }
    stream.flush()
}

fn read_websocket_frame(
    stream: &mut TcpStream,
    expected_masked: bool,
) -> std::io::Result<(u8, Vec<u8>)> {
    let mut header = [0_u8; 2];
    stream.read_exact(&mut header)?;
    if header[0] & 0x80 == 0 || (header[1] & 0x80 != 0) != expected_masked {
        return Err(std::io::Error::other("invalid WebSocket frame flags"));
    }
    let length = match header[1] & 0x7f {
        value @ 0..=125 => usize::from(value),
        126 => {
            let mut value = [0_u8; 2];
            stream.read_exact(&mut value)?;
            usize::from(u16::from_be_bytes(value))
        }
        127 => {
            let mut value = [0_u8; 8];
            stream.read_exact(&mut value)?;
            usize::try_from(u64::from_be_bytes(value))
                .map_err(|_| std::io::Error::other("WebSocket frame is too large"))?
        }
        _ => unreachable!(),
    };
    let mut mask = [0_u8; 4];
    if expected_masked {
        stream.read_exact(&mut mask)?;
    }
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload)?;
    if expected_masked {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % mask.len()];
        }
    }
    Ok((header[0] & 0x0f, payload))
}

struct TemporaryConfig {
    directory: PathBuf,
    path: PathBuf,
}

impl TemporaryConfig {
    fn new(contents: String) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = env::temp_dir().join(format!(
            "graphql-orm-router-binary-smoke-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("router.json");
        fs::write(&path, contents).unwrap();
        Self { directory, path }
    }

    fn path(&self) -> &PathBuf {
        &self.path
    }
}

impl Drop for TemporaryConfig {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
