use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode, jwk::Jwk};
use serde_json::{Value as JsonValue, json};

use super::*;
use crate::{AuthenticationErrorKind, AuthenticationProvider};

pub(crate) const RSA_PRIVATE_KEY: &str = r#"-----BEGIN RSA PRIVATE KEY-----
MIIEpAIBAAKCAQEAyRE6rHuNR0QbHO3H3Kt2pOKGVhQqGZXInOduQNxXzuKlvQTL
UTv4l4sggh5/CYYi/cvI+SXVT9kPWSKXxJXBXd/4LkvcPuUakBoAkfh+eiFVMh2V
rUyWyj3MFl0HTVF9KwRXLAcwkREiS3npThHRyIxuy0ZMeZfxVL5arMhw1SRELB8H
oGfG/AtH89BIE9jDBHZ9dLelK9a184zAf8LwoPLxvJb3Il5nncqPcSfKDDodMFBI
Mc4lQzDKL5gvmiXLXB1AGLm8KBjfE8s3L5xqi+yUod+j8MtvIj812dkS4QMiRVN/
by2h3ZY8LYVGrqZXZTcgn2ujn8uKjXLZVD5TdQIDAQABAoIBAHREk0I0O9DvECKd
WUpAmF3mY7oY9PNQiu44Yaf+AoSuyRpRUGTMIgc3u3eivOE8ALX0BmYUO5JtuRNZ
Dpvt4SAwqCnVUinIf6C+eH/wSurCpapSM0BAHp4aOA7igptyOMgMPYBHNA1e9A7j
E0dCxKWMl3DSWNyjQTk4zeRGEAEfbNjHrq6YCtjHSZSLmWiG80hnfnYos9hOr5Jn
LnyS7ZmFE/5P3XVrxLc/tQ5zum0R4cbrgzHiQP5RgfxGJaEi7XcgherCCOgurJSS
bYH29Gz8u5fFbS+Yg8s+OiCss3cs1rSgJ9/eHZuzGEdUZVARH6hVMjSuwvqVTFaE
8AgtleECgYEA+uLMn4kNqHlJS2A5uAnCkj90ZxEtNm3E8hAxUrhssktY5XSOAPBl
xyf5RuRGIImGtUVIr4HuJSa5TX48n3Vdt9MYCprO/iYl6moNRSPt5qowIIOJmIjY
2mqPDfDt/zw+fcDD3lmCJrFlzcnh0uea1CohxEbQnL3cypeLt+WbU6kCgYEAzSp1
9m1ajieFkqgoB0YTpt/OroDx38vvI5unInJlEeOjQ+oIAQdN2wpxBvTrRorMU6P0
7mFUbt1j+Co6CbNiw+X8HcCaqYLR5clbJOOWNR36PuzOpQLkfK8woupBxzW9B8gZ
mY8rB1mbJ+/WTPrEJy6YGmIEBkWylQ2VpW8O4O0CgYEApdbvvfFBlwD9YxbrcGz7
MeNCFbMz+MucqQntIKoKJ91ImPxvtc0y6e/Rhnv0oyNlaUOwJVu0yNgNG117w0g4
t/+Q38mvVC5xV7/cn7x9UMFk6MkqVir3dYGEqIl/OP1grY2Tq9HtB5iyG9L8NIam
QOLMyUqqMUILxdthHyFmiGkCgYEAn9+PjpjGMPHxL0gj8Q8VbzsFtou6b1deIRRA
2CHmSltltR1gYVTMwXxQeUhPMmgkMqUXzs4/WijgpthY44hK1TaZEKIuoxrS70nJ
4WQLf5a9k1065fDsFZD6yGjdGxvwEmlGMZgTwqV7t1I4X0Ilqhav5hcs5apYL7gn
PYPeRz0CgYALHCj/Ji8XSsDoF/MhVhnGdIs2P99NNdmo3R2Pv0CuZbDKMU559LJH
UvrKS8WkuWRDuKrz1W/EQKApFjDGpdqToZqriUFQzwy7mR3ayIiogzNtHcvbDHx8
oFnGY0OFksX/ye0/XGpy2SFxYRwGU98HPYeBvAQQrVjdkzfy7BmXQQ==
-----END RSA PRIVATE KEY-----"#;

#[derive(Debug)]
struct TestClock(Mutex<SystemTime>);

impl TestClock {
    fn at(seconds: u64) -> Self {
        Self(Mutex::new(UNIX_EPOCH + Duration::from_secs(seconds)))
    }

    fn set(&self, seconds: u64) {
        *self.0.lock().unwrap() = UNIX_EPOCH + Duration::from_secs(seconds);
    }
}

impl AuthenticationClock for TestClock {
    fn now(&self) -> SystemTime {
        *self.0.lock().unwrap()
    }
}

struct JwksFixture {
    address: SocketAddr,
    body: Arc<Mutex<String>>,
    status: Arc<AtomicU16>,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl JwksFixture {
    fn start(body: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let body = Arc::new(Mutex::new(body));
        let status = Arc::new(AtomicU16::new(200));
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_body = body.clone();
        let thread_status = status.clone();
        let thread_requests = requests.clone();
        let thread_stop = stop.clone();
        let thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        thread_requests.fetch_add(1, Ordering::Relaxed);
                        let mut request = [0_u8; 2048];
                        let _ = stream.read(&mut request);
                        let body = thread_body.lock().unwrap().clone();
                        let status = thread_status.load(Ordering::Relaxed);
                        let reason = if status == 200 { "OK" } else { "Unavailable" };
                        let response = format!(
                            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        stream.write_all(response.as_bytes()).unwrap();
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
            body,
            status,
            requests,
            stop,
            thread: Some(thread),
        }
    }

    fn url(&self) -> String {
        format!("http://{}/jwks", self.address)
    }

    fn replace(&self, body: String) {
        *self.body.lock().unwrap() = body;
    }

    fn set_status(&self, status: u16) {
        self.status.store(status, Ordering::Relaxed);
    }
}

impl Drop for JwksFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn jwks(kid: &str) -> String {
    let key = EncodingKey::from_rsa_pem(RSA_PRIVATE_KEY.as_bytes()).unwrap();
    let mut jwk = Jwk::from_encoding_key(&key, Algorithm::RS256).unwrap();
    jwk.common.key_id = Some(kid.to_owned());
    jwk.common.public_key_use = Some(PublicKeyUse::Signature);
    serde_json::to_string(&JwkSet { keys: vec![jwk] }).unwrap()
}

fn token(kid: &str, claims: JsonValue) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.to_owned());
    encode(
        &header,
        &claims,
        &EncodingKey::from_rsa_pem(RSA_PRIVATE_KEY.as_bytes()).unwrap(),
    )
    .unwrap()
}

fn claims(exp: u64) -> JsonValue {
    json!({
        "sub": "user-7",
        "iss": "https://issuer.test",
        "aud": ["graphql-router", "another-resource"],
        "exp": exp,
        "nbf": 900,
        "scope": "products.read products.7.write"
    })
}

fn provider(
    fixture: &JwksFixture,
    clock: Arc<TestClock>,
    legacy: LegacyScopeClaims,
) -> JwksAuthenticationProvider {
    let config =
        JwksAuthenticationConfig::new(fixture.url(), "https://issuer.test", ["graphql-router"])
            .unwrap()
            .with_cache_ttl(Duration::from_secs(10))
            .with_refresh_interval(Duration::from_secs(5))
            .with_legacy_scope_claims(legacy)
            .with_clock(clock)
            .allow_insecure_loopback_http_for_development(true);
    JwksAuthenticationProvider::new(config).unwrap()
}

fn initialize(provider: &JwksAuthenticationProvider) -> Result<(), AuthenticationError> {
    let provider = provider.clone();
    ntex::rt::System::build()
        .name("jwks-provider-test")
        .build(ntex::rt::DefaultRuntime)
        .block_on(async move { provider.initialize().await })
}

fn refresh(provider: &JwksAuthenticationProvider) -> Result<(), AuthenticationError> {
    let provider = provider.clone();
    ntex::rt::System::build()
        .name("jwks-refresh-test")
        .build(ntex::rt::DefaultRuntime)
        .block_on(async move { provider.refresh().await })
}

#[test]
fn jwks_provider_validates_signature_identity_audience_time_and_scope() {
    let fixture = JwksFixture::start(jwks("key-a"));
    let clock = Arc::new(TestClock::at(1_000));
    let provider = provider(&fixture, clock, LegacyScopeClaims::Reject);
    initialize(&provider).unwrap();
    assert_eq!(fixture.requests.load(Ordering::Relaxed), 1);

    let valid = token("key-a", claims(1_100));
    let principal = provider.authenticate_bearer(&valid).unwrap();
    assert_eq!(principal.subject(), "user-7");
    assert_eq!(
        principal.scopes(),
        &["products.7.write".to_owned(), "products.read".to_owned()]
    );
    assert_eq!(
        principal.expires_at(),
        Some(UNIX_EPOCH + Duration::from_secs(1_100))
    );

    for invalid_claims in [
        {
            let mut value = claims(999);
            value["nbf"] = json!(800);
            value
        },
        {
            let mut value = claims(1_100);
            value["iss"] = json!("https://other.test");
            value
        },
        {
            let mut value = claims(1_100);
            value["aud"] = json!("another-api");
            value
        },
    ] {
        assert_eq!(
            provider
                .authenticate_bearer(&token("key-a", invalid_claims))
                .unwrap_err()
                .kind(),
            AuthenticationErrorKind::InvalidCredential
        );
    }
    assert_eq!(
        provider
            .authenticate_bearer(&token("unknown", claims(1_100)))
            .unwrap_err()
            .kind(),
        AuthenticationErrorKind::InvalidCredential
    );
    assert_eq!(
        provider
            .authenticate_bearer("not-a-jwt")
            .unwrap_err()
            .kind(),
        AuthenticationErrorKind::InvalidCredential
    );
}

#[test]
fn jwks_provider_rotates_atomically_and_rejects_stale_cache_after_refresh_failure() {
    let fixture = JwksFixture::start(jwks("key-a"));
    let clock = Arc::new(TestClock::at(2_000));
    let provider = provider(&fixture, clock.clone(), LegacyScopeClaims::Reject);
    initialize(&provider).unwrap();
    let token_a = token("key-a", claims(3_000));
    let token_b = token("key-b", claims(3_000));
    provider.authenticate_bearer(&token_a).unwrap();
    assert_eq!(
        provider.authenticate_bearer(&token_b).unwrap_err().kind(),
        AuthenticationErrorKind::InvalidCredential
    );

    fixture.replace(jwks("key-b"));
    refresh(&provider).unwrap();
    provider.authenticate_bearer(&token_b).unwrap();
    assert_eq!(
        provider.authenticate_bearer(&token_a).unwrap_err().kind(),
        AuthenticationErrorKind::InvalidCredential
    );

    fixture.set_status(503);
    assert_eq!(
        refresh(&provider).unwrap_err().kind(),
        AuthenticationErrorKind::Unavailable
    );
    // A failed refresh preserves the still-fresh complete prior key set.
    provider.authenticate_bearer(&token_b).unwrap();
    clock.set(2_010);
    assert_eq!(
        provider.authenticate_bearer(&token_b).unwrap_err().kind(),
        AuthenticationErrorKind::Unavailable
    );
}

#[test]
fn scope_claim_migration_is_explicit_and_conflicts_fail_closed() {
    let fixture = JwksFixture::start(jwks("key-a"));
    let clock = Arc::new(TestClock::at(1_000));
    let strict = provider(&fixture, clock.clone(), LegacyScopeClaims::Reject);
    initialize(&strict).unwrap();
    let legacy_only = token(
        "key-a",
        json!({
            "sub": "user-7", "iss": "https://issuer.test", "aud": "graphql-router",
            "exp": 1_100, "scopes": ["products.read"]
        }),
    );
    assert!(strict.authenticate_bearer(&legacy_only).is_err());

    let migrating = provider(&fixture, clock, LegacyScopeClaims::Accept);
    initialize(&migrating).unwrap();
    assert_eq!(
        migrating
            .authenticate_bearer(&legacy_only)
            .unwrap()
            .scopes(),
        &["products.read".to_owned()]
    );
    let matching = token(
        "key-a",
        json!({
            "sub": "user-7", "iss": "https://issuer.test", "aud": "graphql-router",
            "exp": 1_100, "scope": "products.write products.read",
            "scopes": ["products.read", "products.write"]
        }),
    );
    assert!(migrating.authenticate_bearer(&matching).is_ok());
    let conflicting = token(
        "key-a",
        json!({
            "sub": "user-7", "iss": "https://issuer.test", "aud": "graphql-router",
            "exp": 1_100, "scope": "products.read", "scopes": []
        }),
    );
    assert!(migrating.authenticate_bearer(&conflicting).is_err());

    for malformed in [
        json!(" products.read"),
        json!("products.read  products.write"),
        json!(["products.read"]),
        JsonValue::Null,
    ] {
        let malformed = token(
            "key-a",
            json!({
                "sub": "user-7", "iss": "https://issuer.test", "aud": "graphql-router",
                "exp": 1_100, "scope": malformed
            }),
        );
        assert!(migrating.authenticate_bearer(&malformed).is_err());
    }
}

#[test]
fn explicit_clock_leeway_is_bounded_and_deterministic() {
    let fixture = JwksFixture::start(jwks("key-a"));
    let clock = Arc::new(TestClock::at(1_000));
    let config =
        JwksAuthenticationConfig::new(fixture.url(), "https://issuer.test", ["graphql-router"])
            .unwrap()
            .with_cache_ttl(Duration::from_secs(10))
            .with_refresh_interval(Duration::from_secs(5))
            .with_leeway(Duration::from_secs(2))
            .with_clock(clock)
            .allow_insecure_loopback_http_for_development(true);
    let provider = JwksAuthenticationProvider::new(config).unwrap();
    initialize(&provider).unwrap();
    let mut value = claims(999);
    value["nbf"] = json!(1_002);
    assert!(provider.authenticate_bearer(&token("key-a", value)).is_ok());
}

#[test]
fn configuration_and_debug_output_keep_secure_defaults_and_redact_keys() {
    let insecure = JwksAuthenticationConfig::new(
        "http://issuer.example/jwks",
        "https://issuer.example",
        ["router"],
    )
    .unwrap();
    assert!(JwksAuthenticationProvider::new(insecure).is_err());
    let excessive_leeway = JwksAuthenticationConfig::new(
        "https://issuer.example/jwks",
        "https://issuer.example",
        ["router"],
    )
    .unwrap()
    .with_leeway(Duration::from_secs(301));
    assert!(JwksAuthenticationProvider::new(excessive_leeway).is_err());

    let fixture = JwksFixture::start(jwks("key-a"));
    let provider = provider(
        &fixture,
        Arc::new(TestClock::at(1_000)),
        LegacyScopeClaims::Reject,
    );
    initialize(&provider).unwrap();
    let diagnostics = format!("{provider:?}");
    let document = jwks("key-a");
    let modulus = serde_json::from_str::<JsonValue>(&document).unwrap()["keys"][0]["n"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!diagnostics.contains(&modulus));
    assert!(!diagnostics.contains("BEGIN RSA"));
    assert!(diagnostics.contains("cached_key_count"));
}
