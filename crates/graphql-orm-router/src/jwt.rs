use std::{
    collections::BTreeMap,
    fmt,
    net::IpAddr,
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures::{StreamExt, future::BoxFuture};
use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    jwk::{AlgorithmParameters, JwkSet, KeyAlgorithm, KeyOperations, PublicKeyUse},
};
use reqwest::{Client, redirect::Policy};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use url::Url;

use crate::{
    AuthenticatedPrincipal, AuthenticationError, AuthenticationProvider, RouterError,
    RouterErrorKind,
};

const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_MAX_JWKS_BYTES: usize = 1024 * 1024;
const MAX_JWKS_KEYS: usize = 128;
const MAX_LEEWAY: Duration = Duration::from_secs(5 * 60);

/// Whether the project-specific JWT `scopes` array is accepted during migration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LegacyScopeClaims {
    /// Reject credentials containing the legacy claim.
    #[default]
    Reject,
    /// Accept a well-formed legacy array. When both claims exist, their sets
    /// must be identical.
    Accept,
}

/// Injectable clock used for deterministic expiry and cache-staleness policy.
pub trait AuthenticationClock: Send + Sync + fmt::Debug + 'static {
    /// Returns the resource server's current wall-clock time.
    fn now(&self) -> SystemTime;
}

/// System wall clock used by default.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemAuthenticationClock;

impl AuthenticationClock for SystemAuthenticationClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Configuration for a remote-JWKS RS256 resource server.
#[derive(Clone)]
pub struct JwksAuthenticationConfig {
    jwks_url: Url,
    issuer: String,
    audiences: Vec<String>,
    cache_ttl: Duration,
    refresh_interval: Duration,
    request_timeout: Duration,
    max_jwks_bytes: usize,
    leeway: Duration,
    legacy_scope_claims: LegacyScopeClaims,
    allow_insecure_loopback_http: bool,
    clock: Arc<dyn AuthenticationClock>,
}

impl JwksAuthenticationConfig {
    /// Creates secure defaults for one issuer and one or more audiences.
    pub fn new(
        jwks_url: impl AsRef<str>,
        issuer: impl Into<String>,
        audiences: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, RouterError> {
        let jwks_url = Url::parse(jwks_url.as_ref()).map_err(|_| {
            invalid_configuration("authentication JWKS URL is not a valid absolute URL")
        })?;
        Ok(Self {
            jwks_url,
            issuer: issuer.into(),
            audiences: audiences.into_iter().map(Into::into).collect(),
            cache_ttl: DEFAULT_CACHE_TTL,
            refresh_interval: DEFAULT_REFRESH_INTERVAL,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            max_jwks_bytes: DEFAULT_MAX_JWKS_BYTES,
            leeway: Duration::ZERO,
            legacy_scope_claims: LegacyScopeClaims::Reject,
            allow_insecure_loopback_http: false,
            clock: Arc::new(SystemAuthenticationClock),
        })
    }

    /// Sets how long successfully loaded verification keys remain usable.
    pub fn with_cache_ttl(mut self, cache_ttl: Duration) -> Self {
        self.cache_ttl = cache_ttl;
        self
    }

    /// Sets the background public-key refresh interval.
    pub fn with_refresh_interval(mut self, refresh_interval: Duration) -> Self {
        self.refresh_interval = refresh_interval;
        self
    }

    /// Sets the bounded JWKS request timeout.
    pub fn with_request_timeout(mut self, request_timeout: Duration) -> Self {
        self.request_timeout = request_timeout;
        self
    }

    /// Sets the maximum accepted JWKS response body size.
    pub fn with_max_jwks_bytes(mut self, max_jwks_bytes: usize) -> Self {
        self.max_jwks_bytes = max_jwks_bytes;
        self
    }

    /// Sets the maximum accepted clock skew for `exp` and `nbf`.
    pub fn with_leeway(mut self, leeway: Duration) -> Self {
        self.leeway = leeway;
        self
    }

    /// Controls the explicit legacy `scopes` migration mode.
    pub fn with_legacy_scope_claims(mut self, mode: LegacyScopeClaims) -> Self {
        self.legacy_scope_claims = mode;
        self
    }

    /// Permits plain HTTP only when the JWKS destination is loopback.
    ///
    /// This exists solely for test-owned and local-development issuers.
    pub fn allow_insecure_loopback_http_for_development(mut self, allow: bool) -> Self {
        self.allow_insecure_loopback_http = allow;
        self
    }

    /// Installs an explicit clock for tests and host-controlled time policy.
    pub fn with_clock(mut self, clock: Arc<dyn AuthenticationClock>) -> Self {
        self.clock = clock;
        self
    }

    fn validate(&self) -> Result<(), RouterError> {
        if self.issuer.trim().is_empty() {
            return Err(invalid_configuration(
                "authentication issuer must not be empty",
            ));
        }
        if self.audiences.is_empty()
            || self
                .audiences
                .iter()
                .any(|audience| audience.trim().is_empty())
        {
            return Err(invalid_configuration(
                "authentication must configure at least one non-empty audience",
            ));
        }
        if self.cache_ttl.is_zero() {
            return Err(invalid_configuration(
                "authentication JWKS cache TTL must be greater than zero",
            ));
        }
        if self.refresh_interval.is_zero() || self.refresh_interval >= self.cache_ttl {
            return Err(invalid_configuration(
                "authentication JWKS refresh interval must be greater than zero and shorter than its cache TTL",
            ));
        }
        if self.request_timeout.is_zero() {
            return Err(invalid_configuration(
                "authentication JWKS request timeout must be greater than zero",
            ));
        }
        if self.max_jwks_bytes == 0 {
            return Err(invalid_configuration(
                "authentication JWKS body limit must be greater than zero",
            ));
        }
        if self.leeway > MAX_LEEWAY {
            return Err(invalid_configuration(
                "authentication clock leeway must not exceed 300 seconds",
            ));
        }
        if !self.jwks_url.username().is_empty()
            || self.jwks_url.password().is_some()
            || self.jwks_url.query().is_some()
            || self.jwks_url.fragment().is_some()
        {
            return Err(invalid_configuration(
                "authentication JWKS URL must not contain credentials, a query, or a fragment",
            ));
        }
        match self.jwks_url.scheme() {
            "https" => {}
            "http" if self.allow_insecure_loopback_http && is_loopback_url(&self.jwks_url) => {}
            _ => {
                return Err(invalid_configuration(
                    "authentication JWKS URL must use HTTPS (plain HTTP is available only for explicit loopback development)",
                ));
            }
        }
        Ok(())
    }
}

impl fmt::Debug for JwksAuthenticationConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JwksAuthenticationConfig")
            .field("jwks_origin", &redacted_origin(&self.jwks_url))
            .field("issuer", &self.issuer)
            .field("audiences", &self.audiences)
            .field("cache_ttl", &self.cache_ttl)
            .field("refresh_interval", &self.refresh_interval)
            .field("request_timeout", &self.request_timeout)
            .field("max_jwks_bytes", &self.max_jwks_bytes)
            .field("leeway", &self.leeway)
            .field("legacy_scope_claims", &self.legacy_scope_claims)
            .field(
                "allow_insecure_loopback_http",
                &self.allow_insecure_loopback_http,
            )
            .finish_non_exhaustive()
    }
}

/// RS256 bearer-token validator backed by a bounded, rotating remote JWKS.
#[derive(Clone)]
pub struct JwksAuthenticationProvider {
    config: JwksAuthenticationConfig,
    client: Client,
    cache: Arc<RwLock<Option<JwksCache>>>,
}

impl JwksAuthenticationProvider {
    /// Builds a provider without contacting the issuer. Router preparation
    /// performs the mandatory initial JWKS load before readiness.
    pub fn new(config: JwksAuthenticationConfig) -> Result<Self, RouterError> {
        config.validate()?;
        let client = Client::builder()
            .redirect(Policy::none())
            .timeout(config.request_timeout)
            .build()
            .map_err(|_| {
                invalid_configuration("failed to construct the authentication JWKS client")
            })?;
        Ok(Self {
            config,
            client,
            cache: Arc::new(RwLock::new(None)),
        })
    }

    async fn refresh_keys(&self) -> Result<(), AuthenticationError> {
        let response = self
            .client
            .get(self.config.jwks_url.clone())
            .send()
            .await
            .map_err(|_| AuthenticationError::unavailable("JWKS retrieval failed"))?;
        if !response.status().is_success() {
            return Err(AuthenticationError::unavailable(
                "JWKS endpoint returned an unsuccessful status",
            ));
        }
        if response
            .content_length()
            .is_some_and(|length| length > self.config.max_jwks_bytes as u64)
        {
            return Err(AuthenticationError::unavailable(
                "JWKS response exceeded its configured body limit",
            ));
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|_| AuthenticationError::unavailable("JWKS response read failed"))?;
            if body.len().saturating_add(chunk.len()) > self.config.max_jwks_bytes {
                return Err(AuthenticationError::unavailable(
                    "JWKS response exceeded its configured body limit",
                ));
            }
            body.extend_from_slice(&chunk);
        }
        let document = serde_json::from_slice::<JwkSet>(&body)
            .map_err(|_| AuthenticationError::unavailable("JWKS document is malformed"))?;
        let keys = validate_jwks(document)?;
        let loaded_at = self.config.clock.now();
        let mut cache = self
            .cache
            .write()
            .map_err(|_| AuthenticationError::unavailable("JWKS cache is unavailable"))?;
        *cache = Some(JwksCache { loaded_at, keys });
        Ok(())
    }

    fn decoding_key(&self, kid: &str) -> Result<DecodingKey, AuthenticationError> {
        let now = self.config.clock.now();
        let cache = self
            .cache
            .read()
            .map_err(|_| AuthenticationError::unavailable("JWKS cache is unavailable"))?;
        let cache = cache
            .as_ref()
            .ok_or_else(|| AuthenticationError::unavailable("JWKS cache is not initialized"))?;
        let age = now.duration_since(cache.loaded_at).map_err(|_| {
            AuthenticationError::unavailable("authentication clock moved before JWKS load time")
        })?;
        if age >= self.config.cache_ttl {
            return Err(AuthenticationError::unavailable("JWKS cache is stale"));
        }
        cache.keys.get(kid).cloned().ok_or_else(|| {
            AuthenticationError::invalid_credential("bearer credential references an unknown key")
        })
    }
}

impl fmt::Debug for JwksAuthenticationProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let key_count = self
            .cache
            .read()
            .ok()
            .and_then(|cache| cache.as_ref().map(|cache| cache.keys.len()));
        formatter
            .debug_struct("JwksAuthenticationProvider")
            .field("config", &self.config)
            .field("cached_key_count", &key_count)
            .finish_non_exhaustive()
    }
}

impl AuthenticationProvider for JwksAuthenticationProvider {
    fn initialize(&self) -> BoxFuture<'_, Result<(), AuthenticationError>> {
        Box::pin(async move { self.refresh_keys().await })
    }

    fn authenticate_bearer(
        &self,
        token: &str,
    ) -> Result<AuthenticatedPrincipal, AuthenticationError> {
        let header = decode_header(token).map_err(|_| invalid_token())?;
        if header.alg != Algorithm::RS256 {
            return Err(invalid_token());
        }
        let kid = header.kid.as_deref().ok_or_else(invalid_token)?;
        if kid.is_empty() {
            return Err(invalid_token());
        }
        let key = self.decoding_key(kid)?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.set_issuer(&[self.config.issuer.as_str()]);
        validation.set_audience(&self.config.audiences);
        // Time policy uses the explicit router clock below.
        validation.validate_exp = false;
        validation.validate_nbf = false;
        let claims = decode::<JwtClaims>(token, &key, &validation)
            .map_err(|_| invalid_token())?
            .claims;
        validate_time_claims(&claims, self.config.clock.as_ref(), self.config.leeway)?;
        let scopes = parse_scope_claims(&claims.additional, self.config.legacy_scope_claims)?;
        let expires_at = UNIX_EPOCH.checked_add(Duration::from_secs(claims.exp));
        let expires_at = expires_at.ok_or_else(invalid_token)?;
        AuthenticatedPrincipal::new(claims.sub, scopes, Some(expires_at))
    }

    fn current_time(&self) -> SystemTime {
        self.config.clock.now()
    }

    fn refresh_interval(&self) -> Option<Duration> {
        Some(self.config.refresh_interval)
    }

    fn refresh(&self) -> BoxFuture<'_, Result<(), AuthenticationError>> {
        Box::pin(async move { self.refresh_keys().await })
    }
}

struct JwksCache {
    loaded_at: SystemTime,
    keys: BTreeMap<String, DecodingKey>,
}

#[derive(Deserialize)]
struct JwtClaims {
    sub: String,
    exp: u64,
    #[serde(default)]
    nbf: Option<u64>,
    #[serde(flatten)]
    additional: BTreeMap<String, JsonValue>,
}

fn validate_jwks(document: JwkSet) -> Result<BTreeMap<String, DecodingKey>, AuthenticationError> {
    if document.keys.is_empty() || document.keys.len() > MAX_JWKS_KEYS {
        return Err(AuthenticationError::unavailable(
            "JWKS document contains an invalid number of keys",
        ));
    }
    let mut keys = BTreeMap::new();
    for jwk in document.keys {
        if !matches!(jwk.algorithm, AlgorithmParameters::RSA(_))
            || !matches!(jwk.common.key_algorithm, None | Some(KeyAlgorithm::RS256))
            || matches!(
                jwk.common.public_key_use,
                Some(PublicKeyUse::Encryption | PublicKeyUse::Other(_))
            )
            || jwk
                .common
                .key_operations
                .as_ref()
                .is_some_and(|operations| !operations.contains(&KeyOperations::Verify))
            || (jwk.common.public_key_use.is_some() && jwk.common.key_operations.is_some())
        {
            return Err(AuthenticationError::unavailable(
                "JWKS document contains a key that is not an RS256 verification key",
            ));
        }
        let kid = jwk
            .common
            .key_id
            .as_deref()
            .filter(|kid| !kid.is_empty())
            .ok_or_else(|| {
                AuthenticationError::unavailable("JWKS document contains a key without an ID")
            })?;
        let decoding_key = DecodingKey::from_jwk(&jwk).map_err(|_| {
            AuthenticationError::unavailable("JWKS document contains invalid key material")
        })?;
        if keys.insert(kid.to_owned(), decoding_key).is_some() {
            return Err(AuthenticationError::unavailable(
                "JWKS document contains duplicate key IDs",
            ));
        }
    }
    Ok(keys)
}

fn validate_time_claims(
    claims: &JwtClaims,
    clock: &dyn AuthenticationClock,
    leeway: Duration,
) -> Result<(), AuthenticationError> {
    let now = clock
        .now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AuthenticationError::unavailable("authentication clock precedes unix epoch"))?
        .as_secs();
    let skew = leeway.as_secs();
    if claims.exp.saturating_add(skew) <= now {
        return Err(AuthenticationError::invalid_credential(
            "bearer credential is expired",
        ));
    }
    if claims
        .nbf
        .is_some_and(|not_before| not_before > now.saturating_add(skew))
    {
        return Err(invalid_token());
    }
    Ok(())
}

pub(crate) fn parse_scope_claims(
    claims: &BTreeMap<String, JsonValue>,
    legacy_mode: LegacyScopeClaims,
) -> Result<Vec<String>, AuthenticationError> {
    let standard = claims.get("scope").map(parse_standard_scope).transpose()?;
    let legacy = match claims.get("scopes") {
        None => None,
        Some(_) if legacy_mode == LegacyScopeClaims::Reject => {
            return Err(AuthenticationError::invalid_credential(
                "legacy scopes claim is not accepted",
            ));
        }
        Some(value) => Some(parse_legacy_scopes(value)?),
    };
    match (standard, legacy) {
        (Some(standard), Some(legacy)) if standard != legacy => Err(
            AuthenticationError::invalid_credential("standard and legacy scope claims conflict"),
        ),
        (Some(standard), _) => Ok(standard),
        (_, Some(legacy)) => Ok(legacy),
        (None, None) => Ok(Vec::new()),
    }
}

fn parse_standard_scope(value: &JsonValue) -> Result<Vec<String>, AuthenticationError> {
    let value = value.as_str().ok_or_else(|| {
        AuthenticationError::invalid_credential("standard scope claim must be a string")
    })?;
    if value.is_empty() {
        return Err(AuthenticationError::invalid_credential(
            "standard scope claim must not be empty",
        ));
    }
    let mut scopes = value.split(' ').map(str::to_owned).collect::<Vec<_>>();
    if scopes.iter().any(|scope| !valid_scope_token(scope)) {
        return Err(AuthenticationError::invalid_credential(
            "standard scope claim is malformed",
        ));
    }
    scopes.sort();
    scopes.dedup();
    Ok(scopes)
}

fn parse_legacy_scopes(value: &JsonValue) -> Result<Vec<String>, AuthenticationError> {
    let values = value.as_array().ok_or_else(|| {
        AuthenticationError::invalid_credential("legacy scopes claim must be a string array")
    })?;
    let mut scopes = Vec::with_capacity(values.len());
    for value in values {
        let scope = value
            .as_str()
            .filter(|scope| valid_scope_token(scope))
            .ok_or_else(|| {
                AuthenticationError::invalid_credential(
                    "legacy scopes claim contains an invalid scope",
                )
            })?;
        scopes.push(scope.to_owned());
    }
    scopes.sort();
    scopes.dedup();
    Ok(scopes)
}

fn valid_scope_token(scope: &str) -> bool {
    !scope.is_empty()
        && scope
            .bytes()
            .all(|byte| matches!(byte, 0x21 | 0x23..=0x5b | 0x5d..=0x7e))
}

fn invalid_configuration(message: &'static str) -> RouterError {
    RouterError::new(RouterErrorKind::InvalidConfiguration, message)
}

fn invalid_token() -> AuthenticationError {
    AuthenticationError::invalid_credential("invalid bearer credential")
}

fn is_loopback_url(url: &Url) -> bool {
    match url.host_str() {
        Some("localhost") => true,
        Some(host) => host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback()),
        None => false,
    }
}

fn redacted_origin(url: &Url) -> String {
    let host = url.host_str().unwrap_or("<invalid-host>");
    match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    }
}

#[cfg(test)]
pub(crate) mod tests;
