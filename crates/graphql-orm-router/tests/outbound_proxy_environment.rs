//! Static outbound clients ignore proxy environment variables by default.
//!
//! This binary holds exactly one test because it changes process-wide proxy
//! variables before any client is built.

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use graphql_orm_router::{
    AuthenticationProvider, JwksAuthenticationConfig, JwksAuthenticationProvider, RouterConfig,
    StaticSubgraph,
};
use jsonwebtoken::{
    Algorithm, EncodingKey,
    jwk::{Jwk, JwkSet, PublicKeyUse},
};

const RSA_PRIVATE_KEY: &str = include_str!("fixtures/test-rsa-private.pem");
const PROXY_VARIABLES: [&str; 6] = [
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
];

/// A loopback listener that answers every request with one fixed body.
struct Fixture {
    address: SocketAddr,
    connections: Arc<AtomicUsize>,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Fixture {
    fn start(content_type: &'static str, body: String) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let connections = Arc::new(AtomicUsize::new(0));
        let stopping = Arc::new(AtomicBool::new(false));
        let thread_connections = connections.clone();
        let thread_stopping = stopping.clone();
        let thread = thread::spawn(move || {
            while !thread_stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        thread_connections.fetch_add(1, Ordering::AcqRel);
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                        let mut request = Vec::new();
                        let mut buffer = [0_u8; 1024];
                        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                            match stream.read(&mut buffer) {
                                Ok(0) | Err(_) => break,
                                Ok(count) => request.extend_from_slice(&buffer[..count]),
                            }
                        }
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => return,
                }
            }
        });
        Self {
            address,
            connections,
            stopping,
            thread: Some(thread),
        }
    }

    fn origin(&self) -> String {
        format!("http://{}", self.address)
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::Acquire)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn jwks() -> String {
    let key = EncodingKey::from_rsa_pem(RSA_PRIVATE_KEY.as_bytes()).unwrap();
    let mut jwk = Jwk::from_encoding_key(&key, Algorithm::RS256).unwrap();
    jwk.common.key_id = Some("proxy-test-key".to_owned());
    jwk.common.public_key_use = Some(PublicKeyUse::Signature);
    serde_json::to_string(&JwkSet { keys: vec![jwk] }).unwrap()
}

#[ntex::test]
async fn static_outbound_clients_ignore_proxy_environment_unless_opted_in() {
    // The "proxy" answers with a body no client accepts, so a proxied request
    // is visible both as a connection and as a failure.
    let proxy = Fixture::start("text/plain", "proxied".to_owned());
    let sdl = Fixture::start(
        "application/graphql",
        "type Query { value: String }".to_owned(),
    );
    let keys = Fixture::start("application/json", jwks());
    // SAFETY: this binary's only test sets the variables before it builds any
    // client or starts any thread that reads the environment.
    unsafe {
        for name in PROXY_VARIABLES {
            std::env::set_var(name, proxy.origin());
        }
        std::env::remove_var("NO_PROXY");
        std::env::remove_var("no_proxy");
    }

    let router = || {
        RouterConfig::new("127.0.0.1:0".parse().unwrap())
            .allow_anonymous_development(true)
            .with_subgraph(StaticSubgraph::new(
                "values",
                format!("{}/graphql", sdl.origin()),
                format!("{}/sdl", sdl.origin()),
            ))
    };
    router()
        .prepare()
        .await
        .expect("the schema client must connect directly");
    assert_eq!(sdl.connections(), 1);
    assert_eq!(proxy.connections(), 0, "the schema client used the proxy");

    let authentication = || {
        JwksAuthenticationConfig::new(
            format!("{}/jwks", keys.origin()),
            "https://issuer.test",
            ["graphql-router"],
        )
        .unwrap()
        .allow_insecure_loopback_http_for_development(true)
    };
    JwksAuthenticationProvider::new(authentication())
        .unwrap()
        .initialize()
        .await
        .expect("the JWKS client must connect directly");
    assert_eq!(keys.connections(), 1);
    assert_eq!(proxy.connections(), 0, "the JWKS client used the proxy");

    // The explicit opt-in restores the environment proxy for both clients.
    assert!(
        router()
            .use_environment_proxy(true)
            .prepare()
            .await
            .is_err()
    );
    assert_eq!(proxy.connections(), 1);
    assert!(
        JwksAuthenticationProvider::new(authentication().use_environment_proxy(true))
            .unwrap()
            .initialize()
            .await
            .is_err()
    );
    assert_eq!(proxy.connections(), 2);
    assert_eq!(sdl.connections(), 1);
    assert_eq!(keys.connections(), 1);
}
