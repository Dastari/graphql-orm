use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use jsonwebtoken::{
    Algorithm, EncodingKey, Header, encode,
    jwk::{Jwk, JwkSet},
};
use serde_json::{Value as JsonValue, json};

#[cfg(feature = "auth-agql")]
use agql_auth::{
    RoleScopeCatalogue, RoleScopeCatalogueClaims, RoleScopeDefinition, RoleScopeGrant,
    SignedRoleScopeCatalogue,
};

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

/// Deterministic wall and monotonic clocks. `set` models elapsed time: the
/// monotonic side advances by any forward wall step. `set_wall` moves only
/// the wall clock, as an operator or NTP step would.
#[derive(Debug)]
struct TestClock {
    wall: Mutex<SystemTime>,
    base: Instant,
    elapsed: Mutex<Duration>,
}

impl TestClock {
    fn at(seconds: u64) -> Self {
        Self {
            wall: Mutex::new(UNIX_EPOCH + Duration::from_secs(seconds)),
            base: Instant::now(),
            elapsed: Mutex::new(Duration::ZERO),
        }
    }

    fn set(&self, seconds: u64) {
        let target = UNIX_EPOCH + Duration::from_secs(seconds);
        let mut wall = self.wall.lock().unwrap();
        if let Ok(step) = target.duration_since(*wall) {
            *self.elapsed.lock().unwrap() += step;
        }
        *wall = target;
    }

    #[cfg(feature = "auth-agql")]
    fn set_wall(&self, seconds: u64) {
        *self.wall.lock().unwrap() = UNIX_EPOCH + Duration::from_secs(seconds);
    }

    #[cfg(feature = "auth-agql")]
    fn advance_monotonic(&self, by: Duration) {
        *self.elapsed.lock().unwrap() += by;
    }
}

impl AuthenticationClock for TestClock {
    fn now(&self) -> SystemTime {
        *self.wall.lock().unwrap()
    }

    fn monotonic_now(&self) -> Instant {
        self.base + *self.elapsed.lock().unwrap()
    }
}

struct JwksFixture {
    address: SocketAddr,
    body: Arc<Mutex<String>>,
    status: Arc<AtomicU16>,
    requests: Arc<AtomicUsize>,
    /// Delays only the next accepted request's response, in milliseconds.
    delay_next_ms: Arc<AtomicU64>,
    #[cfg(feature = "auth-agql")]
    last_request: Arc<Mutex<String>>,
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
        let delay_next_ms = Arc::new(AtomicU64::new(0));
        #[cfg(feature = "auth-agql")]
        let last_request = Arc::new(Mutex::new(String::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_body = body.clone();
        let thread_status = status.clone();
        let thread_requests = requests.clone();
        let thread_delay = delay_next_ms.clone();
        #[cfg(feature = "auth-agql")]
        let thread_last_request = last_request.clone();
        let thread_stop = stop.clone();
        let thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        thread_requests.fetch_add(1, Ordering::Relaxed);
                        // The response is chosen when the request arrives, so
                        // a delayed response carries the then-current body.
                        let body = thread_body.lock().unwrap().clone();
                        let status = thread_status.load(Ordering::Relaxed);
                        let delay = Duration::from_millis(thread_delay.swap(0, Ordering::AcqRel));
                        #[cfg(feature = "auth-agql")]
                        let last_request = thread_last_request.clone();
                        thread::spawn(move || {
                            let _ = stream.set_nonblocking(false);
                            let mut request = [0_u8; 2048];
                            #[cfg(feature = "auth-agql")]
                            let bytes_read = stream.read(&mut request).unwrap_or_default();
                            #[cfg(not(feature = "auth-agql"))]
                            let _ = stream.read(&mut request);
                            #[cfg(feature = "auth-agql")]
                            {
                                *last_request.lock().unwrap() =
                                    String::from_utf8_lossy(&request[..bytes_read]).into_owned();
                            }
                            thread::sleep(delay);
                            let reason = if status == 200 { "OK" } else { "Unavailable" };
                            let response = format!(
                                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            );
                            // A bounded client may close immediately after rejecting Content-Length.
                            let _ = stream.write_all(response.as_bytes());
                        });
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
            delay_next_ms,
            #[cfg(feature = "auth-agql")]
            last_request,
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

    fn delay_next_response(&self, delay: Duration) {
        self.delay_next_ms
            .store(delay.as_millis() as u64, Ordering::Release);
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

#[cfg(feature = "auth-agql")]
fn signed_catalogue(kid: &str, issued_at: i64, expires_at: i64) -> String {
    signed_catalogue_with_role(kid, issued_at, expires_at, "inventory-operator")
}

#[cfg(feature = "auth-agql")]
fn signed_catalogue_with_role(kid: &str, issued_at: i64, expires_at: i64, role_id: &str) -> String {
    let catalogue = RoleScopeCatalogue::new(
        "revision-7",
        [
            RoleScopeDefinition::new("inventory.read"),
            RoleScopeDefinition::new("inventory.write").exact_only(),
            RoleScopeDefinition::new("profile.read"),
        ],
        [RoleScopeGrant::new(
            role_id,
            "Inventory operator",
            ["inventory.read", "inventory.write"],
        )],
    );
    let claims = RoleScopeCatalogueClaims::new(
        catalogue.clone(),
        "https://issuer.test",
        "role-catalogue-clients",
        issued_at,
        expires_at,
    );
    let signature = token(kid, serde_json::to_value(claims).unwrap());
    serde_json::to_string(&SignedRoleScopeCatalogue::new(catalogue, signature)).unwrap()
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

#[cfg(feature = "auth-agql")]
fn provider_with_catalogue(
    keys: &JwksFixture,
    catalogue: &JwksFixture,
    clock: Arc<TestClock>,
) -> JwksAuthenticationProvider {
    provider_with_catalogue_config(keys, catalogue, clock, |config| config).unwrap()
}

#[cfg(feature = "auth-agql")]
fn provider_with_catalogue_config(
    keys: &JwksFixture,
    catalogue: &JwksFixture,
    clock: Arc<TestClock>,
    configure: impl FnOnce(RoleScopeCatalogueConfig) -> RoleScopeCatalogueConfig,
) -> Result<JwksAuthenticationProvider, RouterError> {
    let role_scope = configure(
        RoleScopeCatalogueConfig::new(catalogue.url(), "role-catalogue-clients")
            .unwrap()
            .with_cache_ttl(Duration::from_secs(10))
            .allow_insecure_loopback_http_for_development(true),
    );
    let config =
        JwksAuthenticationConfig::new(keys.url(), "https://issuer.test", ["graphql-router"])
            .unwrap()
            .with_cache_ttl(Duration::from_secs(100))
            .with_refresh_interval(Duration::from_secs(5))
            .with_clock(clock)
            .allow_insecure_loopback_http_for_development(true)
            .with_role_scope_catalogue(role_scope);
    JwksAuthenticationProvider::new(config)
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

#[cfg(feature = "auth-agql")]
fn refresh_catalogue(provider: &JwksAuthenticationProvider) -> Result<(), AuthenticationError> {
    let provider = provider.clone();
    ntex::rt::System::build()
        .name("role-scope-refresh-test")
        .build(ntex::rt::DefaultRuntime)
        .block_on(async move { provider.refresh_role_scope_catalogue().await })
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

#[cfg(feature = "auth-agql")]
#[test]
fn signed_role_catalogue_expands_roles_and_preserves_direct_scopes() {
    let keys = JwksFixture::start(jwks("key-a"));
    let catalogue = JwksFixture::start(signed_catalogue("key-a", 1_000, 1_010));
    let clock = Arc::new(TestClock::at(1_000));
    let provider = provider_with_catalogue(&keys, &catalogue, clock);
    initialize(&provider).unwrap();
    assert_eq!(keys.requests.load(Ordering::Relaxed), 1);
    assert_eq!(catalogue.requests.load(Ordering::Relaxed), 0);
    refresh_catalogue(&provider).unwrap();
    assert_eq!(catalogue.requests.load(Ordering::Relaxed), 1);

    let credential = token(
        "key-a",
        json!({
            "sub": "user-7", "iss": "https://issuer.test", "aud": "graphql-router",
            "exp": 1_100, "scope": "profile.read",
            "roles": ["application-admin"],
            "authorization_roles": ["inventory-operator"]
        }),
    );
    assert_eq!(
        provider.authenticate_bearer(&credential).unwrap().scopes(),
        &["inventory.read", "inventory.write", "profile.read"]
    );
}

#[cfg(feature = "auth-agql")]
#[test]
fn role_catalogue_request_headers_are_applied_and_redacted() {
    let keys = JwksFixture::start(jwks("key-a"));
    let catalogue = JwksFixture::start(signed_catalogue("key-a", 1_000, 1_010));
    let role_scope = RoleScopeCatalogueConfig::new(catalogue.url(), "role-catalogue-clients")
        .unwrap()
        .with_cache_ttl(Duration::from_secs(10))
        .allow_insecure_loopback_http_for_development(true)
        .with_request_header("x-catalogue-token", "catalogue-secret")
        .unwrap();
    let config =
        JwksAuthenticationConfig::new(keys.url(), "https://issuer.test", ["graphql-router"])
            .unwrap()
            .with_cache_ttl(Duration::from_secs(100))
            .with_refresh_interval(Duration::from_secs(5))
            .with_clock(Arc::new(TestClock::at(1_000)))
            .allow_insecure_loopback_http_for_development(true)
            .with_role_scope_catalogue(role_scope);
    let provider = JwksAuthenticationProvider::new(config).unwrap();
    assert!(!format!("{provider:?}").contains("catalogue-secret"));

    initialize(&provider).unwrap();
    refresh_catalogue(&provider).unwrap();
    let request = catalogue.last_request.lock().unwrap().to_ascii_lowercase();
    assert!(request.contains("x-catalogue-token: catalogue-secret"));
}

#[cfg(feature = "auth-agql")]
#[test]
fn role_catalogue_is_mandatory_current_and_verified_for_role_bearers() {
    let keys = JwksFixture::start(jwks("key-a"));
    let catalogue = JwksFixture::start(signed_catalogue("key-a", 1_000, 1_010));
    let clock = Arc::new(TestClock::at(1_000));
    let provider = provider_with_catalogue(&keys, &catalogue, clock.clone());
    initialize(&provider).unwrap();
    refresh_catalogue(&provider).unwrap();
    let role_credential = token(
        "key-a",
        json!({
            "sub": "user-7", "iss": "https://issuer.test", "aud": "graphql-router",
            "exp": 1_100, "authorization_roles": ["inventory-operator"]
        }),
    );
    provider.authenticate_bearer(&role_credential).unwrap();

    catalogue.set_status(503);
    assert_eq!(
        refresh(&provider).unwrap_err().kind(),
        AuthenticationErrorKind::Unavailable
    );
    provider.authenticate_bearer(&role_credential).unwrap();
    clock.set(1_010);
    assert_eq!(
        provider
            .authenticate_bearer(&role_credential)
            .unwrap()
            .scopes(),
        &["inventory.read", "inventory.write"]
    );
    assert_eq!(provider.role_scope_stale_serve_total(), 1);

    let direct_only = token(
        "key-a",
        json!({
            "sub": "user-7", "iss": "https://issuer.test", "aud": "graphql-router",
            "exp": 1_100, "scope": "profile.read"
        }),
    );
    assert_eq!(
        provider.authenticate_bearer(&direct_only).unwrap().scopes(),
        &["profile.read"]
    );

    let malformed = token(
        "key-a",
        json!({
            "sub": "user-7", "iss": "https://issuer.test", "aud": "graphql-router",
            "exp": 1_100, "authorization_roles": ["bad role"]
        }),
    );
    assert_eq!(
        provider.authenticate_bearer(&malformed).unwrap_err().kind(),
        AuthenticationErrorKind::InvalidCredential
    );
}

#[cfg(feature = "auth-agql")]
#[test]
fn stale_role_catalogue_is_refused_after_its_maximum_staleness() {
    let keys = JwksFixture::start(jwks("key-a"));
    // Signed expiry 1_010 equals the soft age (loaded 1_000, TTL 10), so the
    // snapshot is stale from 1_010 and may serve roles until 1_030.
    let catalogue = JwksFixture::start(signed_catalogue("key-a", 1_000, 1_010));
    let clock = Arc::new(TestClock::at(1_000));
    let provider = provider_with_catalogue_config(&keys, &catalogue, clock.clone(), |config| {
        config.with_maximum_staleness(Duration::from_secs(20))
    })
    .unwrap();
    initialize(&provider).unwrap();
    refresh_catalogue(&provider).unwrap();
    let role_credential = token(
        "key-a",
        json!({
            "sub": "user-7", "iss": "https://issuer.test", "aud": "graphql-router",
            "exp": 2_000, "authorization_roles": ["inventory-operator"]
        }),
    );
    let direct_only = token(
        "key-a",
        json!({
            "sub": "user-7", "iss": "https://issuer.test", "aud": "graphql-router",
            "exp": 2_000, "scope": "profile.read"
        }),
    );

    catalogue.set_status(503);
    assert_eq!(provider.role_scope_refresh_failure_total(), 0);
    assert!(refresh(&provider).is_err());
    assert_eq!(provider.role_scope_refresh_failure_total(), 1);

    clock.set(1_029);
    assert_eq!(
        provider
            .authenticate_bearer(&role_credential)
            .unwrap()
            .scopes(),
        &["inventory.read", "inventory.write"],
        "within the maximum staleness the last verified snapshot still serves"
    );
    assert_eq!(provider.role_scope_stale_serve_total(), 1);
    assert_eq!(provider.role_scope_stale_refusal_total(), 0);

    clock.set(1_030);
    let refused = provider.authenticate_bearer(&role_credential).unwrap_err();
    assert_eq!(refused.kind(), AuthenticationErrorKind::Unavailable);
    assert_eq!(
        refused.detail(),
        "role-scope catalogue snapshot exceeded its maximum staleness"
    );
    assert_eq!(provider.role_scope_stale_refusal_total(), 1);
    assert_eq!(
        provider.role_scope_stale_serve_total(),
        1,
        "a refusal is not counted as a stale serve"
    );
    assert_eq!(
        provider.authenticate_bearer(&direct_only).unwrap().scopes(),
        &["profile.read"],
        "direct-scope-only credentials do not depend on the catalogue"
    );

    // A later successful refresh restores role expansion.
    catalogue.replace(signed_catalogue("key-a", 1_030, 1_100));
    catalogue.set_status(200);
    refresh_catalogue(&provider).unwrap();
    assert_eq!(
        provider
            .authenticate_bearer(&role_credential)
            .unwrap()
            .scopes(),
        &["inventory.read", "inventory.write"]
    );
    assert_eq!(provider.role_scope_stale_refusal_total(), 1);
}

#[cfg(feature = "auth-agql")]
#[test]
fn catalogue_staleness_follows_a_monotonic_deadline_not_the_wall_clock() {
    let keys = JwksFixture::start(jwks("key-a"));
    let catalogue = JwksFixture::start(signed_catalogue("key-a", 1_000, 1_010));
    let role_credential = token(
        "key-a",
        json!({
            "sub": "user-7", "iss": "https://issuer.test", "aud": "graphql-router",
            "exp": 9_000, "authorization_roles": ["inventory-operator"]
        }),
    );
    let staleness_refused = |provider: &JwksAuthenticationProvider| {
        provider
            .authenticate_bearer(&role_credential)
            .is_err_and(|error| {
                error.detail() == "role-scope catalogue snapshot exceeded its maximum staleness"
            })
    };
    // Loaded at 1_000, stale at 1_010, refused from 1_030 on the monotonic clock.
    let fresh = || {
        let clock = Arc::new(TestClock::at(1_000));
        let provider = provider_with_catalogue_config(&keys, &catalogue, clock.clone(), |config| {
            config.with_maximum_staleness(Duration::from_secs(20))
        })
        .unwrap();
        initialize(&provider).unwrap();
        refresh_catalogue(&provider).unwrap();
        (clock, provider)
    };
    catalogue.set_status(200);

    // A refusal is not reopened by moving the wall clock back, as long as it
    // stays after the load time.
    let (clock, provider) = fresh();
    catalogue.set_status(503);
    clock.set(1_030);
    assert!(staleness_refused(&provider));
    clock.set_wall(1_005);
    assert!(
        staleness_refused(&provider),
        "a wall-clock rollback must not reopen the snapshot"
    );
    // Behind the load time it still fails closed, just as before.
    clock.set_wall(999);
    assert!(provider.authenticate_bearer(&role_credential).is_err());

    // Rolling the wall clock back before the cutoff does not extend it.
    catalogue.set_status(200);
    let (clock, provider) = fresh();
    catalogue.set_status(503);
    clock.set(1_020);
    clock.set_wall(1_001);
    provider.authenticate_bearer(&role_credential).unwrap();
    clock.advance_monotonic(Duration::from_secs(10));
    assert!(
        staleness_refused(&provider),
        "the cutoff stays 30 seconds of elapsed time after acceptance"
    );

    // A wall-clock jump forward neither ends nor extends the snapshot.
    catalogue.set_status(200);
    let (clock, provider) = fresh();
    catalogue.set_status(503);
    clock.set_wall(8_000);
    // Reload the keys at the new wall time so only the catalogue is in play.
    keys.set_status(200);
    initialize(&provider).unwrap();
    let principal = provider.authenticate_bearer(&role_credential).unwrap();
    assert_eq!(provider.role_scope_stale_serve_total(), 0, "not stale yet");
    assert_eq!(
        principal.authority_valid_until(),
        Some(clock.monotonic_now() + Duration::from_secs(30))
    );
    clock.advance_monotonic(Duration::from_secs(29));
    provider.authenticate_bearer(&role_credential).unwrap();
    clock.advance_monotonic(Duration::from_secs(1));
    assert!(staleness_refused(&provider));
}

#[cfg(feature = "auth-agql")]
#[test]
fn role_expansion_carries_its_snapshot_deadline_and_direct_scopes_do_not() {
    let keys = JwksFixture::start(jwks("key-a"));
    let catalogue = JwksFixture::start(signed_catalogue("key-a", 1_000, 1_100));
    let clock = Arc::new(TestClock::at(1_000));
    let provider = provider_with_catalogue_config(&keys, &catalogue, clock.clone(), |config| {
        config.with_maximum_staleness(Duration::from_secs(20))
    })
    .unwrap();
    initialize(&provider).unwrap();
    refresh_catalogue(&provider).unwrap();
    let role = provider
        .authenticate_bearer(&token(
            "key-a",
            json!({
                "sub": "user-7", "iss": "https://issuer.test", "aud": "graphql-router",
                "exp": 9_000, "authorization_roles": ["inventory-operator"]
            }),
        ))
        .unwrap();
    // Soft TTL 10 s is earlier than the signed expiry, plus 20 s staleness.
    assert_eq!(
        role.authority_valid_until(),
        Some(clock.monotonic_now() + Duration::from_secs(30))
    );
    let direct = provider
        .authenticate_bearer(&token(
            "key-a",
            json!({
                "sub": "user-7", "iss": "https://issuer.test", "aud": "graphql-router",
                "exp": 9_000, "scope": "profile.read"
            }),
        ))
        .unwrap();
    assert_eq!(direct.authority_valid_until(), None);
    assert_eq!(provider.monotonic_now(), clock.monotonic_now());
}

#[cfg(feature = "auth-agql")]
#[test]
fn role_catalogue_maximum_staleness_has_a_bounded_default() {
    let keys = JwksFixture::start(jwks("key-a"));
    let catalogue = JwksFixture::start(signed_catalogue("key-a", 1_000, 1_010));
    let default = RoleScopeCatalogueConfig::new(catalogue.url(), "role-catalogue-clients").unwrap();
    assert!(format!("{default:?}").contains("maximum_staleness: 900s"));
    let error = provider_with_catalogue_config(
        &keys,
        &catalogue,
        Arc::new(TestClock::at(1_000)),
        |config| config.with_maximum_staleness(Duration::from_secs(86_401)),
    )
    .unwrap_err();
    assert!(error.to_string().contains("maximum staleness"), "{error}");

    // The default serves a stale snapshot for 15 minutes past staleness only.
    let clock = Arc::new(TestClock::at(1_000));
    let provider = provider_with_catalogue(&keys, &catalogue, clock.clone());
    initialize(&provider).unwrap();
    refresh_catalogue(&provider).unwrap();
    catalogue.set_status(503);
    let role_credential = token(
        "key-a",
        json!({
            "sub": "user-7", "iss": "https://issuer.test", "aud": "graphql-router",
            "exp": 5_000, "authorization_roles": ["inventory-operator"]
        }),
    );
    // Reload the keys so only the catalogue is stale.
    clock.set(1_010 + 899);
    initialize(&provider).unwrap();
    provider.authenticate_bearer(&role_credential).unwrap();
    clock.set(1_010 + 900);
    initialize(&provider).unwrap();
    assert_eq!(
        provider
            .authenticate_bearer(&role_credential)
            .unwrap_err()
            .detail(),
        "role-scope catalogue snapshot exceeded its maximum staleness"
    );

    // Zero refuses role expansion as soon as the snapshot is stale.
    let clock = Arc::new(TestClock::at(1_000));
    let strict = provider_with_catalogue_config(&keys, &catalogue, clock.clone(), |config| {
        config.with_maximum_staleness(Duration::ZERO)
    })
    .unwrap();
    catalogue.set_status(200);
    initialize(&strict).unwrap();
    refresh_catalogue(&strict).unwrap();
    clock.set(1_009);
    strict.authenticate_bearer(&role_credential).unwrap();
    clock.set(1_010);
    assert!(strict.authenticate_bearer(&role_credential).is_err());
    assert_eq!(strict.role_scope_stale_serve_total(), 0);
    assert_eq!(strict.role_scope_stale_refusal_total(), 1);
}

#[cfg(feature = "auth-agql")]
#[test]
fn forged_role_catalogue_does_not_block_startup_and_role_bearers_fail_closed() {
    let keys = JwksFixture::start(jwks("key-a"));
    let catalogue = JwksFixture::start(signed_catalogue("other-key", 1_000, 1_010));
    let provider = provider_with_catalogue(&keys, &catalogue, Arc::new(TestClock::at(1_000)));
    initialize(&provider).unwrap();
    assert_eq!(catalogue.requests.load(Ordering::Relaxed), 0);
    let role_credential = token(
        "key-a",
        json!({
            "sub": "user-7", "iss": "https://issuer.test", "aud": "graphql-router",
            "exp": 1_100, "authorization_roles": ["inventory-operator"]
        }),
    );
    assert_eq!(
        provider
            .authenticate_bearer(&role_credential)
            .unwrap_err()
            .kind(),
        AuthenticationErrorKind::Unavailable
    );
}

#[cfg(feature = "auth-agql")]
#[test]
fn first_role_catalogue_fetch_is_lazy_and_retried_on_demand() {
    let keys = JwksFixture::start(jwks("key-a"));
    let catalogue = JwksFixture::start(signed_catalogue("key-a", 1_000, 1_010));
    catalogue.set_status(503);
    let provider = provider_with_catalogue(&keys, &catalogue, Arc::new(TestClock::at(1_000)));
    initialize(&provider).unwrap();
    assert_eq!(catalogue.requests.load(Ordering::Relaxed), 0);
    catalogue.set_status(200);

    let credential = token(
        "key-a",
        json!({
            "sub": "user-7", "iss": "https://issuer.test", "aud": "graphql-router",
            "exp": 1_100, "authorization_roles": ["inventory-operator"]
        }),
    );
    let provider_for_request = provider.clone();
    let credential_for_request = credential.clone();
    let request_count = catalogue.requests.clone();
    let request_count_for_wait = request_count.clone();
    let first = ntex::rt::System::build()
        .name("lazy-role-scope-fetch-test")
        .build(ntex::rt::DefaultRuntime)
        .block_on(async move {
            let result = provider_for_request.authenticate_bearer(&credential_for_request);
            for _ in 0..100 {
                if request_count_for_wait.load(Ordering::Relaxed) >= 1
                    && !provider_for_request
                        .role_scope_refresh_in_flight
                        .load(Ordering::Acquire)
                {
                    break;
                }
                hive_router::tokio::time::sleep(Duration::from_millis(2)).await;
            }
            result
        });
    assert_eq!(
        first.unwrap_err().kind(),
        AuthenticationErrorKind::Unavailable
    );
    assert_eq!(request_count.load(Ordering::Relaxed), 1);
    assert_eq!(
        provider.authenticate_bearer(&credential).unwrap().scopes(),
        &["inventory.read", "inventory.write"]
    );
}

#[cfg(feature = "auth-agql")]
#[test]
fn failed_lazy_catalogue_fetch_observes_retry_backoff() {
    let keys = JwksFixture::start(jwks("key-a"));
    let catalogue = JwksFixture::start(signed_catalogue("key-a", 1_000, 1_010));
    catalogue.set_status(503);
    let clock = Arc::new(TestClock::at(1_000));
    let provider = provider_with_catalogue(&keys, &catalogue, clock.clone());
    initialize(&provider).unwrap();
    let credential = token(
        "key-a",
        json!({
            "sub": "user-7", "iss": "https://issuer.test", "aud": "graphql-router",
            "exp": 1_100, "authorization_roles": ["inventory-operator"]
        }),
    );

    let provider_for_request = provider.clone();
    let credential_for_request = credential.clone();
    let request_count = catalogue.requests.clone();
    ntex::rt::System::build()
        .name("role-scope-backoff-test")
        .build(ntex::rt::DefaultRuntime)
        .block_on(async move {
            assert!(
                provider_for_request
                    .authenticate_bearer(&credential_for_request)
                    .is_err()
            );
            for _ in 0..100 {
                if request_count.load(Ordering::Relaxed) >= 1
                    && !provider_for_request
                        .role_scope_refresh_in_flight
                        .load(Ordering::Acquire)
                {
                    break;
                }
                hive_router::tokio::time::sleep(Duration::from_millis(2)).await;
            }
            assert!(
                provider_for_request
                    .authenticate_bearer(&credential_for_request)
                    .is_err()
            );
            hive_router::tokio::time::sleep(Duration::from_millis(5)).await;
        });
    assert_eq!(catalogue.requests.load(Ordering::Relaxed), 1);

    catalogue.set_status(200);
    clock.set(1_001);
    let provider_for_request = provider.clone();
    let credential_for_request = credential.clone();
    let request_count = catalogue.requests.clone();
    ntex::rt::System::build()
        .name("role-scope-backoff-retry-test")
        .build(ntex::rt::DefaultRuntime)
        .block_on(async move {
            assert!(
                provider_for_request
                    .authenticate_bearer(&credential_for_request)
                    .is_err()
            );
            for _ in 0..100 {
                if request_count.load(Ordering::Relaxed) >= 2
                    && !provider_for_request
                        .role_scope_refresh_in_flight
                        .load(Ordering::Acquire)
                {
                    break;
                }
                hive_router::tokio::time::sleep(Duration::from_millis(2)).await;
            }
        });
    assert_eq!(catalogue.requests.load(Ordering::Relaxed), 2);
    assert_eq!(
        provider.authenticate_bearer(&credential).unwrap().scopes(),
        &["inventory.read", "inventory.write"]
    );
}

#[cfg(feature = "auth-agql")]
#[test]
fn application_roles_do_not_require_or_feed_catalogue_expansion() {
    let keys = JwksFixture::start(jwks("key-a"));
    let provider = provider(
        &keys,
        Arc::new(TestClock::at(1_000)),
        LegacyScopeClaims::Reject,
    );
    initialize(&provider).unwrap();
    let application_role = token(
        "key-a",
        json!({
            "sub": "user-7", "iss": "https://issuer.test", "aud": "graphql-router",
            "exp": 1_100, "scope": "profile.read", "roles": ["application-admin"]
        }),
    );
    assert_eq!(
        provider
            .authenticate_bearer(&application_role)
            .unwrap()
            .scopes(),
        &["profile.read"]
    );

    let authorization_role = token(
        "key-a",
        json!({
            "sub": "user-7", "iss": "https://issuer.test", "aud": "graphql-router",
            "exp": 1_100, "scope": "profile.read",
            "authorization_roles": ["inventory-operator"]
        }),
    );
    assert_eq!(
        provider
            .authenticate_bearer(&authorization_role)
            .unwrap_err()
            .kind(),
        AuthenticationErrorKind::Unavailable
    );
}

#[cfg(feature = "auth-agql")]
#[test]
fn unknown_authorization_role_fails_instead_of_dropping_inherited_scopes() {
    let keys = JwksFixture::start(jwks("key-a"));
    let catalogue = JwksFixture::start(signed_catalogue("key-a", 1_000, 1_010));
    let provider = provider_with_catalogue(&keys, &catalogue, Arc::new(TestClock::at(1_000)));
    initialize(&provider).unwrap();
    refresh_catalogue(&provider).unwrap();
    catalogue.replace(signed_catalogue_with_role(
        "key-a",
        1_000,
        1_010,
        "replacement-role",
    ));
    let credential = token(
        "key-a",
        json!({
            "sub": "user-7", "iss": "https://issuer.test", "aud": "graphql-router",
            "exp": 1_100, "scope": "profile.read",
            "authorization_roles": ["replacement-role"]
        }),
    );
    let provider_for_request = provider.clone();
    let credential_for_request = credential.clone();
    let request_count = catalogue.requests.clone();
    let request_count_for_wait = request_count.clone();
    let request_result = ntex::rt::System::build()
        .name("role-scope-refresh-request-test")
        .build(ntex::rt::DefaultRuntime)
        .block_on(async move {
            let result = provider_for_request.authenticate_bearer(&credential_for_request);
            for _ in 0..100 {
                if request_count_for_wait.load(Ordering::Relaxed) >= 2
                    && !provider_for_request
                        .role_scope_refresh_in_flight
                        .load(Ordering::Acquire)
                {
                    break;
                }
                hive_router::tokio::time::sleep(Duration::from_millis(2)).await;
            }
            result
        });
    assert_eq!(
        request_result.unwrap_err().kind(),
        AuthenticationErrorKind::Unavailable
    );
    assert!(request_count.load(Ordering::Relaxed) >= 2);

    assert_eq!(
        provider.authenticate_bearer(&credential).unwrap().scopes(),
        &["inventory.read", "inventory.write", "profile.read"]
    );
}

#[cfg(feature = "auth-agql")]
#[test]
fn complete_signed_catalogue_over_one_mib_is_verified_with_finite_body_limits() {
    let keys = JwksFixture::start(jwks("key-a"));
    let scopes = (0..128)
        .map(|index| format!("catalogue.resources.{index:03}.authorization.read"))
        .collect::<Vec<_>>();
    let catalogue = RoleScopeCatalogue::new(
        "large-revision",
        scopes.iter().cloned().map(RoleScopeDefinition::new),
        (0..128).map(|index| {
            RoleScopeGrant::new(
                format!("role-{index:03}"),
                format!("Role {index:03}"),
                scopes.clone(),
            )
        }),
    );
    let claims = RoleScopeCatalogueClaims::new(
        catalogue.clone(),
        "https://issuer.test",
        "role-catalogue-clients",
        1_000,
        1_100,
    );
    let envelope = SignedRoleScopeCatalogue::new(
        catalogue,
        token("key-a", serde_json::to_value(claims).unwrap()),
    );
    envelope.validate_structure().unwrap();
    let body = serde_json::to_string(&envelope).unwrap();
    assert!(body.len() > 1024 * 1024, "{} bytes", body.len());
    assert!(body.len() < DEFAULT_MAX_ROLE_SCOPE_CATALOGUE_BYTES);
    let remote = JwksFixture::start(body.clone());
    let provider = provider_with_catalogue(&keys, &remote, Arc::new(TestClock::at(1_000)));
    initialize(&provider).unwrap();
    refresh_catalogue(&provider).unwrap();
    let credential = token(
        "key-a",
        json!({
            "sub": "user-7", "iss": "https://issuer.test", "aud": "graphql-router",
            "exp": 1_100, "authorization_roles": ["role-127"]
        }),
    );
    assert_eq!(
        provider.authenticate_bearer(&credential).unwrap().scopes(),
        scopes.as_slice()
    );

    // An explicitly smaller host transport bound remains authoritative.
    let mut limited_config = provider.config.clone();
    limited_config
        .role_scope_catalogue
        .as_mut()
        .unwrap()
        .max_body_bytes = 1024 * 1024;
    let limited = JwksAuthenticationProvider::new(limited_config).unwrap();
    initialize(&limited).unwrap();
    assert_eq!(
        refresh_catalogue(&limited).unwrap_err().kind(),
        AuthenticationErrorKind::Unavailable
    );
    assert!(limited.authenticate_bearer(&credential).is_err());

    // Valid trailing JSON whitespace cannot bypass the finite default transport cap.
    let mut oversized = body;
    oversized.extend(std::iter::repeat_n(
        ' ',
        DEFAULT_MAX_ROLE_SCOPE_CATALOGUE_BYTES + 1 - oversized.len(),
    ));
    remote.replace(oversized);
    let over = provider_with_catalogue(&keys, &remote, Arc::new(TestClock::at(1_000)));
    initialize(&over).unwrap();
    assert_eq!(
        refresh_catalogue(&over).unwrap_err().kind(),
        AuthenticationErrorKind::Unavailable
    );
    assert!(over.authenticate_bearer(&credential).is_err());

    // A large structurally legal document still needs the genuine bound signature.
    let mut forged = envelope;
    forged.catalogue.roles[0].name = "Tampered metadata".into();
    remote.replace(serde_json::to_string(&forged).unwrap());
    let invalid = provider_with_catalogue(&keys, &remote, Arc::new(TestClock::at(1_000)));
    initialize(&invalid).unwrap();
    assert_eq!(
        refresh_catalogue(&invalid).unwrap_err().kind(),
        AuthenticationErrorKind::Unavailable
    );
    assert!(invalid.authenticate_bearer(&credential).is_err());
}

fn rsa_jwk(kid: &str) -> JsonValue {
    serde_json::from_str::<JsonValue>(&jwks(kid)).unwrap()["keys"][0].clone()
}

/// Waits inside a runtime so a background JWKS refresh can run to completion.
fn authenticate_then_settle(
    provider: &JwksAuthenticationProvider,
    fixture: &JwksFixture,
    token: &str,
    expected_requests: usize,
) -> Result<AuthenticatedPrincipal, AuthenticationError> {
    let provider = provider.clone();
    let token = token.to_owned();
    let requests = fixture.requests.clone();
    ntex::rt::System::build()
        .name("jwks-unknown-key-test")
        .build(ntex::rt::DefaultRuntime)
        .block_on(async move {
            let result = provider.authenticate_bearer(&token);
            for _ in 0..250 {
                if requests.load(Ordering::Relaxed) >= expected_requests {
                    break;
                }
                hive_router::tokio::time::sleep(Duration::from_millis(2)).await;
            }
            // Let the refresh publish its keys, or an unrequested one show up.
            hive_router::tokio::time::sleep(Duration::from_millis(50)).await;
            result
        })
}

#[test]
fn jwks_document_with_unusable_keys_keeps_its_usable_rs256_keys() {
    let mut encryption = rsa_jwk("encryption-key");
    encryption["use"] = json!("enc");
    let mut without_id = rsa_jwk("unused");
    without_id.as_object_mut().unwrap().remove("kid");
    let document = json!({
        "keys": [
            // RFC 7517 appendix A.1 elliptic-curve public key.
            {
                "kty": "EC", "crv": "P-256", "use": "sig", "kid": "ec-key",
                "x": "MKBCTNIcKUSDii11ySs3526iDZ8AiTo7Tu6KPAqv7D4",
                "y": "4Etl6SRW2YiLUrN5vfvVHuhp7x8PxltmWWlbbM4IFyM"
            },
            encryption,
            without_id,
            { "kty": "future-key-type", "kid": "future-key" },
            rsa_jwk("key-a"),
        ]
    });
    let fixture = JwksFixture::start(document.to_string());
    let clock = Arc::new(TestClock::at(1_000));
    let provider = provider(&fixture, clock, LegacyScopeClaims::Reject);
    initialize(&provider).unwrap();
    provider
        .authenticate_bearer(&token("key-a", claims(1_100)))
        .unwrap();
    assert_eq!(
        provider
            .authenticate_bearer(&token("encryption-key", claims(1_100)))
            .unwrap_err()
            .kind(),
        AuthenticationErrorKind::InvalidCredential
    );

    // A document with no usable key still fails closed, preserving the
    // previous complete key set.
    fixture.replace(
        json!({"keys": [{
            "kty": "EC", "crv": "P-256", "kid": "ec-key",
            "x": "MKBCTNIcKUSDii11ySs3526iDZ8AiTo7Tu6KPAqv7D4",
            "y": "4Etl6SRW2YiLUrN5vfvVHuhp7x8PxltmWWlbbM4IFyM"
        }]})
        .to_string(),
    );
    assert_eq!(
        refresh(&provider).unwrap_err().kind(),
        AuthenticationErrorKind::Unavailable
    );
    provider
        .authenticate_bearer(&token("key-a", claims(1_100)))
        .unwrap();

    // Two usable keys under one ID stay ambiguous and fail closed.
    fixture.replace(json!({"keys": [rsa_jwk("key-a"), rsa_jwk("key-a")]}).to_string());
    assert_eq!(
        refresh(&provider).unwrap_err().kind(),
        AuthenticationErrorKind::Unavailable
    );
}

#[test]
fn unknown_key_id_requests_a_rate_limited_jwks_refresh() {
    let fixture = JwksFixture::start(jwks("key-a"));
    let clock = Arc::new(TestClock::at(2_000));
    let config =
        JwksAuthenticationConfig::new(fixture.url(), "https://issuer.test", ["graphql-router"])
            .unwrap()
            .with_cache_ttl(Duration::from_secs(600))
            .with_refresh_interval(Duration::from_secs(300))
            .with_clock(clock.clone())
            .allow_insecure_loopback_http_for_development(true);
    let provider = JwksAuthenticationProvider::new(config).unwrap();
    initialize(&provider).unwrap();
    assert_eq!(fixture.requests.load(Ordering::Relaxed), 1);

    // The issuer rotates before the scheduled refresh. The first bearer that
    // names the new key is refused, and asks for one background refresh.
    fixture.replace(json!({"keys": [rsa_jwk("key-a"), rsa_jwk("key-b")]}).to_string());
    let rotated = token("key-b", claims(3_000));
    assert_eq!(
        authenticate_then_settle(&provider, &fixture, &rotated, 2)
            .unwrap_err()
            .kind(),
        AuthenticationErrorKind::InvalidCredential
    );
    assert_eq!(fixture.requests.load(Ordering::Relaxed), 2);
    provider.authenticate_bearer(&rotated).unwrap();

    // Unknown IDs inside the cooldown do not reach the issuer again.
    let forged = token("forged-key", claims(3_000));
    for _ in 0..3 {
        assert_eq!(
            authenticate_then_settle(&provider, &fixture, &forged, 2)
                .unwrap_err()
                .kind(),
            AuthenticationErrorKind::InvalidCredential
        );
    }
    assert_eq!(fixture.requests.load(Ordering::Relaxed), 2);

    // After the default 30-second cooldown one more unknown ID may refresh.
    clock.set(2_030);
    assert!(authenticate_then_settle(&provider, &fixture, &forged, 3).is_err());
    assert_eq!(fixture.requests.load(Ordering::Relaxed), 3);
}

#[test]
fn unknown_key_refresh_cooldown_is_bounded_and_overflow_is_never_due() {
    let config = |cooldown: Duration| {
        JwksAuthenticationConfig::new("https://issuer.test/jwks", "https://issuer.test", ["api"])
            .unwrap()
            .with_unknown_key_refresh_cooldown(cooldown)
    };
    for cooldown in [
        Duration::ZERO,
        MAX_UNKNOWN_KEY_REFRESH_COOLDOWN + Duration::from_secs(1),
    ] {
        assert!(JwksAuthenticationProvider::new(config(cooldown)).is_err());
    }
    JwksAuthenticationProvider::new(config(MAX_UNKNOWN_KEY_REFRESH_COOLDOWN)).unwrap();

    // A previous refresh so late that adding the cooldown overflows is not
    // treated as "due"; overflow must never lift the rate limit.
    let fixture = JwksFixture::start(jwks("key-a"));
    let clock = Arc::new(TestClock::at(2_000));
    let provider = JwksAuthenticationProvider::new(
        JwksAuthenticationConfig::new(fixture.url(), "https://issuer.test", ["graphql-router"])
            .unwrap()
            .with_cache_ttl(Duration::from_secs(600))
            .with_refresh_interval(Duration::from_secs(300))
            .with_unknown_key_refresh_cooldown(MAX_UNKNOWN_KEY_REFRESH_COOLDOWN)
            .with_clock(clock)
            .allow_insecure_loopback_http_for_development(true),
    )
    .unwrap();
    initialize(&provider).unwrap();
    *provider.last_unknown_key_refresh.write().unwrap() =
        SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(i64::MAX as u64 - 60));
    let forged = token("forged-key", claims(3_000));
    assert!(authenticate_then_settle(&provider, &fixture, &forged, 2).is_err());
    assert_eq!(fixture.requests.load(Ordering::Relaxed), 1);
}

#[test]
fn a_slow_older_jwks_fetch_never_replaces_a_newer_key_set() {
    let fixture = JwksFixture::start(jwks("key-a"));
    let clock = Arc::new(TestClock::at(2_000));
    let provider = provider(&fixture, clock, LegacyScopeClaims::Reject);
    initialize(&provider).unwrap();

    // The first refresh is answered slowly with the key set current when it
    // arrived; a second refresh starts while it is still in flight.
    fixture.replace(jwks("key-old"));
    fixture.delay_next_response(Duration::from_millis(400));
    let slow = provider.clone();
    let first = thread::spawn(move || refresh(&slow));
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while fixture.requests.load(Ordering::Relaxed) < 2 {
        assert!(std::time::Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    fixture.replace(jwks("key-new"));
    refresh(&provider).unwrap();
    first.join().unwrap().unwrap();

    provider
        .authenticate_bearer(&token("key-new", claims(3_000)))
        .unwrap();
    assert_eq!(
        provider
            .authenticate_bearer(&token("key-old", claims(3_000)))
            .unwrap_err()
            .kind(),
        AuthenticationErrorKind::InvalidCredential
    );
}

#[test]
fn a_dropped_unknown_key_refresh_task_clears_its_in_flight_flag() {
    let fixture = JwksFixture::start(jwks("key-a"));
    let clock = Arc::new(TestClock::at(2_000));
    let provider = provider(&fixture, clock, LegacyScopeClaims::Reject);
    initialize(&provider).unwrap();

    fixture.delay_next_response(Duration::from_secs(2));
    let in_runtime = provider.clone();
    ntex::rt::System::build()
        .name("jwks-dropped-refresh-test")
        .build(ntex::rt::DefaultRuntime)
        .block_on(async move {
            assert!(
                in_runtime
                    .authenticate_bearer(&token("unknown", claims(3_000)))
                    .is_err()
            );
            assert!(in_runtime.key_refresh_in_flight.load(Ordering::Acquire));
        });
    // The runtime ended with the refresh task still waiting on its fetch.
    assert!(!provider.key_refresh_in_flight.load(Ordering::Acquire));
}
