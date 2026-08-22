use std::{
    collections::BTreeMap,
    fmt,
    net::IpAddr,
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(feature = "auth-agql")]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[cfg(feature = "auth-agql")]
use agql_auth::{
    RoleScopeCatalogueClaims, RoleScopeCatalogueValidationOptions, RoleScopeExpansionError,
    RoleScopeExpansionProvider, SignedRoleScopeCatalogue, StaticRoleScopeExpansion,
    effective_scopes,
};
use futures::{StreamExt, future::BoxFuture};
use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    jwk::{AlgorithmParameters, JwkSet, KeyAlgorithm, KeyOperations, PublicKeyUse},
};
use reqwest::{
    Client,
    header::{HeaderMap, HeaderName, HeaderValue},
    redirect::Policy,
};
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
const DEFAULT_MAX_ROLE_SCOPE_CATALOGUE_BYTES: usize = 1024 * 1024;
const DEFAULT_ROLE_SCOPE_SIGNED_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);
const DEFAULT_ROLE_SCOPE_CLOCK_LEEWAY: Duration = Duration::from_secs(30);
const DEFAULT_ROLE_SCOPE_RETRY_BACKOFF: Duration = Duration::from_secs(1);
const DEFAULT_ROLE_SCOPE_MAXIMUM_RETRY_BACKOFF: Duration = Duration::from_secs(60);
const MAX_JWKS_KEYS: usize = 128;
const MAX_LEEWAY: Duration = Duration::from_secs(5 * 60);
#[cfg(feature = "auth-agql")]
const MAX_ACCESS_TOKEN_AUTHORIZATION_ROLES: usize = 256;
#[cfg(feature = "auth-agql")]
const MAX_ACCESS_TOKEN_AUTHORIZATION_ROLE_LENGTH: usize = 512;

/// Remote signed role-scope catalogue configuration.
#[derive(Clone)]
pub struct RoleScopeCatalogueConfig {
    url: Url,
    audience: String,
    cache_ttl: Duration,
    maximum_signed_lifetime: Duration,
    clock_skew_leeway: Duration,
    retry_backoff: Duration,
    maximum_retry_backoff: Duration,
    max_body_bytes: usize,
    allow_insecure_loopback_http: bool,
    request_headers: HeaderMap,
}

impl RoleScopeCatalogueConfig {
    /// Creates secure defaults for one catalogue URL and signature audience.
    pub fn new(url: impl AsRef<str>, audience: impl Into<String>) -> Result<Self, RouterError> {
        let url = Url::parse(url.as_ref()).map_err(|_| {
            invalid_configuration("role-scope catalogue URL is not a valid absolute URL")
        })?;
        Ok(Self {
            url,
            audience: audience.into(),
            cache_ttl: DEFAULT_CACHE_TTL,
            maximum_signed_lifetime: DEFAULT_ROLE_SCOPE_SIGNED_LIFETIME,
            clock_skew_leeway: DEFAULT_ROLE_SCOPE_CLOCK_LEEWAY,
            retry_backoff: DEFAULT_ROLE_SCOPE_RETRY_BACKOFF,
            maximum_retry_backoff: DEFAULT_ROLE_SCOPE_MAXIMUM_RETRY_BACKOFF,
            max_body_bytes: DEFAULT_MAX_ROLE_SCOPE_CATALOGUE_BYTES,
            allow_insecure_loopback_http: false,
            request_headers: HeaderMap::new(),
        })
    }

    /// Sets the soft age after which a verified snapshot is served stale while
    /// a refresh is requested. It does not constrain issuer-signed lifetime.
    pub fn with_cache_ttl(mut self, cache_ttl: Duration) -> Self {
        self.cache_ttl = cache_ttl;
        self
    }

    /// Sets the longest issuer-signed catalogue lifetime accepted on fetch.
    pub fn with_maximum_signed_lifetime(mut self, lifetime: Duration) -> Self {
        self.maximum_signed_lifetime = lifetime;
        self
    }

    /// Sets bounded clock-skew leeway for catalogue `iat` and `exp` checks.
    pub fn with_clock_skew_leeway(mut self, leeway: Duration) -> Self {
        self.clock_skew_leeway = leeway;
        self
    }

    /// Sets the initial and maximum delays for retries after a failed lazy
    /// catalogue refresh. Delays grow exponentially between these bounds.
    pub fn with_retry_backoff(mut self, initial: Duration, maximum: Duration) -> Self {
        self.retry_backoff = initial;
        self.maximum_retry_backoff = maximum;
        self
    }

    /// Sets the maximum accepted response body size.
    pub fn with_max_body_bytes(mut self, max_body_bytes: usize) -> Self {
        self.max_body_bytes = max_body_bytes;
        self
    }

    /// Permits plain HTTP only for an explicit loopback development endpoint.
    pub fn allow_insecure_loopback_http_for_development(mut self, allow: bool) -> Self {
        self.allow_insecure_loopback_http = allow;
        self
    }

    /// Adds one request header without exposing its value through diagnostics.
    pub fn with_request_header(
        mut self,
        name: impl AsRef<str>,
        value: impl AsRef<str>,
    ) -> Result<Self, RouterError> {
        let name = HeaderName::from_bytes(name.as_ref().as_bytes()).map_err(|_| {
            invalid_configuration("role-scope catalogue request header name is invalid")
        })?;
        let value = HeaderValue::from_str(value.as_ref()).map_err(|_| {
            invalid_configuration("role-scope catalogue request header value is invalid")
        })?;
        self.request_headers.insert(name, value);
        Ok(self)
    }

    #[cfg(feature = "auth-agql")]
    fn validate(&self, refresh_interval: Duration) -> Result<(), RouterError> {
        if self.audience.trim().is_empty() {
            return Err(invalid_configuration(
                "role-scope catalogue audience must not be empty",
            ));
        }
        if self.cache_ttl.is_zero() || refresh_interval >= self.cache_ttl {
            return Err(invalid_configuration(
                "role-scope catalogue cache TTL must be greater than the authentication refresh interval",
            ));
        }
        if self.max_body_bytes == 0 {
            return Err(invalid_configuration(
                "role-scope catalogue body limit must be greater than zero",
            ));
        }
        if self.maximum_signed_lifetime.is_zero() {
            return Err(invalid_configuration(
                "role-scope catalogue maximum signed lifetime must be greater than zero",
            ));
        }
        if self.clock_skew_leeway > MAX_LEEWAY {
            return Err(invalid_configuration(
                "role-scope catalogue clock leeway must not exceed 300 seconds",
            ));
        }
        if self.retry_backoff.is_zero() || self.maximum_retry_backoff < self.retry_backoff {
            return Err(invalid_configuration(
                "role-scope catalogue retry backoff must be greater than zero and not exceed its maximum",
            ));
        }
        validate_public_resource_url(
            &self.url,
            self.allow_insecure_loopback_http,
            "role-scope catalogue",
        )
    }
}

impl fmt::Debug for RoleScopeCatalogueConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RoleScopeCatalogueConfig")
            .field("origin", &redacted_origin(&self.url))
            .field("audience", &self.audience)
            .field("cache_ttl", &self.cache_ttl)
            .field("maximum_signed_lifetime", &self.maximum_signed_lifetime)
            .field("clock_skew_leeway", &self.clock_skew_leeway)
            .field("retry_backoff", &self.retry_backoff)
            .field("maximum_retry_backoff", &self.maximum_retry_backoff)
            .field("max_body_bytes", &self.max_body_bytes)
            .field(
                "request_header_names",
                &self.request_headers.keys().collect::<Vec<_>>(),
            )
            .field(
                "allow_insecure_loopback_http",
                &self.allow_insecure_loopback_http,
            )
            .finish()
    }
}

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
    role_scope_catalogue: Option<RoleScopeCatalogueConfig>,
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
            role_scope_catalogue: None,
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

    /// Enables verified role-to-scope expansion from a remote signed catalogue.
    pub fn with_role_scope_catalogue(mut self, catalogue: RoleScopeCatalogueConfig) -> Self {
        self.role_scope_catalogue = Some(catalogue);
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
        validate_public_resource_url(
            &self.jwks_url,
            self.allow_insecure_loopback_http,
            "authentication JWKS",
        )?;
        #[cfg(not(feature = "auth-agql"))]
        if self.role_scope_catalogue.is_some() {
            return Err(invalid_configuration(
                "role-scope catalogue expansion requires the auth-agql feature",
            ));
        }
        #[cfg(feature = "auth-agql")]
        if let Some(catalogue) = &self.role_scope_catalogue {
            catalogue.validate(self.refresh_interval)?;
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
            .field("role_scope_catalogue", &self.role_scope_catalogue)
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
    #[cfg(feature = "auth-agql")]
    role_scope_cache: Arc<RwLock<Option<Arc<RoleScopeCache>>>>,
    #[cfg(feature = "auth-agql")]
    role_scope_refresh_in_flight: Arc<AtomicBool>,
    #[cfg(feature = "auth-agql")]
    role_scope_refresh_failures: Arc<AtomicU64>,
    #[cfg(feature = "auth-agql")]
    role_scope_retry_after: Arc<RwLock<Option<SystemTime>>>,
    #[cfg(feature = "auth-agql")]
    role_scope_stale_serves: Arc<AtomicU64>,
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
            #[cfg(feature = "auth-agql")]
            role_scope_cache: Arc::new(RwLock::new(None)),
            #[cfg(feature = "auth-agql")]
            role_scope_refresh_in_flight: Arc::new(AtomicBool::new(false)),
            #[cfg(feature = "auth-agql")]
            role_scope_refresh_failures: Arc::new(AtomicU64::new(0)),
            #[cfg(feature = "auth-agql")]
            role_scope_retry_after: Arc::new(RwLock::new(None)),
            #[cfg(feature = "auth-agql")]
            role_scope_stale_serves: Arc::new(AtomicU64::new(0)),
        })
    }

    async fn refresh_keys(&self) -> Result<(), AuthenticationError> {
        let body = self
            .fetch_bounded(
                self.config.jwks_url.clone(),
                self.config.max_jwks_bytes,
                "JWKS",
                None,
            )
            .await?;
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

    async fn fetch_bounded(
        &self,
        url: Url,
        maximum_bytes: usize,
        resource: &'static str,
        request_headers: Option<&HeaderMap>,
    ) -> Result<Vec<u8>, AuthenticationError> {
        let mut request = self.client.get(url);
        if let Some(headers) = request_headers {
            request = request.headers(headers.clone());
        }
        let response = request.send().await.map_err(|_| {
            AuthenticationError::unavailable(format!("{resource} retrieval failed"))
        })?;
        if !response.status().is_success() {
            return Err(AuthenticationError::unavailable(format!(
                "{resource} endpoint returned an unsuccessful status"
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > maximum_bytes as u64)
        {
            return Err(AuthenticationError::unavailable(format!(
                "{resource} response exceeded its configured body limit"
            )));
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| {
                AuthenticationError::unavailable(format!("{resource} response read failed"))
            })?;
            if body.len().saturating_add(chunk.len()) > maximum_bytes {
                return Err(AuthenticationError::unavailable(format!(
                    "{resource} response exceeded its configured body limit"
                )));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    #[cfg(feature = "auth-agql")]
    async fn refresh_role_scope_catalogue(&self) -> Result<(), AuthenticationError> {
        let Some(config) = &self.config.role_scope_catalogue else {
            return Ok(());
        };
        let body = self
            .fetch_bounded(
                config.url.clone(),
                config.max_body_bytes,
                "role-scope catalogue",
                Some(&config.request_headers),
            )
            .await?;
        let envelope = serde_json::from_slice::<SignedRoleScopeCatalogue>(&body).map_err(|_| {
            AuthenticationError::unavailable("role-scope catalogue document is malformed")
        })?;
        envelope.validate_structure().map_err(|_| {
            AuthenticationError::unavailable("role-scope catalogue document is invalid")
        })?;
        let header = decode_header(&envelope.signature).map_err(|_| {
            AuthenticationError::unavailable("role-scope catalogue signature is malformed")
        })?;
        if header.alg != Algorithm::RS256 {
            return Err(AuthenticationError::unavailable(
                "role-scope catalogue signature algorithm is invalid",
            ));
        }
        let kid = header
            .kid
            .as_deref()
            .filter(|kid| !kid.is_empty())
            .ok_or_else(|| {
                AuthenticationError::unavailable("role-scope catalogue signature has no key ID")
            })?;
        let key = self.decoding_key(kid)?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_required_spec_claims(&["exp", "iat", "iss", "aud"]);
        validation.set_issuer(&[self.config.issuer.as_str()]);
        validation.set_audience(&[config.audience.as_str()]);
        validation.validate_exp = false;
        validation.validate_nbf = false;
        let claims = decode::<RoleScopeCatalogueClaims>(&envelope.signature, &key, &validation)
            .map_err(|_| {
                AuthenticationError::unavailable("role-scope catalogue signature is invalid")
            })?
            .claims;
        let now = unix_timestamp(self.config.clock.as_ref())?;
        let maximum_lifetime =
            i64::try_from(config.maximum_signed_lifetime.as_secs()).map_err(|_| {
                AuthenticationError::unavailable(
                    "role-scope catalogue maximum signed lifetime is invalid",
                )
            })?;
        let clock_skew_leeway =
            i64::try_from(config.clock_skew_leeway.as_secs()).map_err(|_| {
                AuthenticationError::unavailable("role-scope catalogue clock leeway is invalid")
            })?;
        claims
            .validate_binding_with_options(
                &envelope,
                &self.config.issuer,
                &config.audience,
                now,
                RoleScopeCatalogueValidationOptions::default()
                    .with_maximum_lifetime_seconds(maximum_lifetime)
                    .with_clock_skew_leeway_seconds(clock_skew_leeway),
            )
            .map_err(|_| {
                AuthenticationError::unavailable("role-scope catalogue signature is not bound")
            })?;
        let provider = StaticRoleScopeExpansion::new(&envelope.catalogue).map_err(|_| {
            AuthenticationError::unavailable("role-scope catalogue expansion is invalid")
        })?;
        let expires_at = u64::try_from(claims.exp)
            .ok()
            .and_then(|value| UNIX_EPOCH.checked_add(Duration::from_secs(value)))
            .ok_or_else(|| {
                AuthenticationError::unavailable("role-scope catalogue expiry is invalid")
            })?;
        let mut cache = self.role_scope_cache.write().map_err(|_| {
            AuthenticationError::unavailable("role-scope catalogue cache is unavailable")
        })?;
        *cache = Some(Arc::new(RoleScopeCache {
            loaded_at: self.config.clock.now(),
            expires_at,
            provider,
        }));
        self.role_scope_refresh_failures.store(0, Ordering::Release);
        *self.role_scope_retry_after.write().map_err(|_| {
            AuthenticationError::unavailable("role-scope catalogue retry state is unavailable")
        })? = None;
        Ok(())
    }

    #[cfg(feature = "auth-agql")]
    fn request_role_scope_refresh(&self, bypass_backoff: bool) {
        if !bypass_backoff {
            let retry_after = self
                .role_scope_retry_after
                .read()
                .ok()
                .and_then(|retry_after| *retry_after);
            if retry_after.is_some_and(|retry_after| self.config.clock.now() < retry_after) {
                return;
            }
        }
        if self
            .role_scope_refresh_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let Ok(handle) = hive_router::tokio::runtime::Handle::try_current() else {
            self.role_scope_refresh_in_flight
                .store(false, Ordering::Release);
            return;
        };
        let provider = self.clone();
        handle.spawn(async move {
            if let Err(error) = provider.refresh_role_scope_catalogue().await {
                let failure_count = provider
                    .role_scope_refresh_failures
                    .fetch_add(1, Ordering::AcqRel)
                    .saturating_add(1);
                let exponent = failure_count.saturating_sub(1).min(63) as u32;
                let multiplier = 1_u64.checked_shl(exponent).unwrap_or(u64::MAX);
                let delay = provider
                    .config
                    .role_scope_catalogue
                    .as_ref()
                    .map(|config| {
                        config
                            .retry_backoff
                            .saturating_mul(multiplier.try_into().unwrap_or(u32::MAX))
                            .min(config.maximum_retry_backoff)
                    })
                    .unwrap_or_default();
                if let Some(retry_after) = provider.config.clock.now().checked_add(delay)
                    && let Ok(mut state) = provider.role_scope_retry_after.write()
                {
                    *state = Some(retry_after);
                }
                tracing::warn!(
                    error = %error,
                    retry_delay_seconds = delay.as_secs(),
                    "role-scope catalogue refresh failed; preserving the last verified snapshot"
                );
            }
            provider
                .role_scope_refresh_in_flight
                .store(false, Ordering::Release);
        });
    }

    async fn refresh_verification_state(&self) -> Result<(), AuthenticationError> {
        self.refresh_keys().await?;
        #[cfg(feature = "auth-agql")]
        self.refresh_role_scope_catalogue().await?;
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

    #[cfg(feature = "auth-agql")]
    fn expand_roles(
        &self,
        roles: &[String],
        direct_scopes: Vec<String>,
    ) -> Result<Vec<String>, AuthenticationError> {
        if roles.is_empty() {
            return Ok(direct_scopes);
        }
        let Some(config) = &self.config.role_scope_catalogue else {
            return Err(AuthenticationError::unavailable(
                "role-scope catalogue is not configured for authorization-role grants",
            ));
        };
        let now = self.config.clock.now();
        let cache = self
            .role_scope_cache
            .read()
            .map_err(|_| {
                AuthenticationError::unavailable("role-scope catalogue cache is unavailable")
            })?
            .clone();
        let Some(cache) = cache else {
            self.request_role_scope_refresh(false);
            return Err(AuthenticationError::unavailable(
                "role-scope catalogue cache is not initialized",
            ));
        };
        let age = now.duration_since(cache.loaded_at).map_err(|_| {
            AuthenticationError::unavailable(
                "authentication clock moved before role-scope catalogue load time",
            )
        })?;
        if age >= config.cache_ttl || now >= cache.expires_at {
            let count = self
                .role_scope_stale_serves
                .fetch_add(1, Ordering::Relaxed)
                .saturating_add(1);
            if count == 1 || count.is_power_of_two() {
                tracing::warn!(
                    stale_serve_total = count,
                    stale_seconds = age.as_secs(),
                    "serving a stale signature-verified role-scope catalogue while refreshing"
                );
            }
            self.request_role_scope_refresh(false);
        }
        let expansion = cache.provider.expand_roles(roles).map_err(|error| {
            if matches!(error, RoleScopeExpansionError::UnknownRole(_)) {
                self.request_role_scope_refresh(true);
            }
            AuthenticationError::unavailable("role-scope catalogue expansion failed")
        })?;
        Ok(effective_scopes(direct_scopes, &expansion))
    }

    /// Returns the process-local count of requests served from a stale but
    /// signature-verified role-scope snapshot.
    #[cfg(feature = "auth-agql")]
    pub fn role_scope_stale_serve_total(&self) -> u64 {
        self.role_scope_stale_serves.load(Ordering::Relaxed)
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
        #[cfg(feature = "auth-agql")]
        let scopes = self.expand_roles(&claims.authorization_roles, scopes)?;
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
        Box::pin(async move {
            let result = self.refresh_verification_state().await;
            if let Err(error) = &result {
                tracing::warn!(
                    error = %error,
                    "authentication refresh failed; preserving last-known-good verification state"
                );
            }
            result
        })
    }
}

struct JwksCache {
    loaded_at: SystemTime,
    keys: BTreeMap<String, DecodingKey>,
}

#[cfg(feature = "auth-agql")]
struct RoleScopeCache {
    loaded_at: SystemTime,
    expires_at: SystemTime,
    provider: StaticRoleScopeExpansion,
}

#[derive(Deserialize)]
struct JwtClaims {
    sub: String,
    exp: u64,
    #[serde(default)]
    nbf: Option<u64>,
    #[cfg(feature = "auth-agql")]
    #[serde(default, deserialize_with = "deserialize_roles")]
    authorization_roles: Vec<String>,
    #[serde(flatten)]
    additional: BTreeMap<String, JsonValue>,
}

#[cfg(feature = "auth-agql")]
fn deserialize_roles<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let mut roles = Vec::<String>::deserialize(deserializer)?;
    if roles.len() > MAX_ACCESS_TOKEN_AUTHORIZATION_ROLES
        || roles.iter().any(|role| {
            role.is_empty()
                || role.len() > MAX_ACCESS_TOKEN_AUTHORIZATION_ROLE_LENGTH
                || role
                    .bytes()
                    .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        })
    {
        return Err(serde::de::Error::custom(
            "invalid access-token authorization roles",
        ));
    }
    roles.sort();
    roles.dedup();
    Ok(roles)
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
    let now = u64::try_from(unix_timestamp(clock)?).map_err(|_| {
        AuthenticationError::unavailable("authentication clock precedes unix epoch")
    })?;
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

fn unix_timestamp(clock: &dyn AuthenticationClock) -> Result<i64, AuthenticationError> {
    let seconds = clock
        .now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AuthenticationError::unavailable("authentication clock precedes unix epoch"))?
        .as_secs();
    i64::try_from(seconds)
        .map_err(|_| AuthenticationError::unavailable("authentication clock is out of range"))
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

fn invalid_configuration(message: impl Into<String>) -> RouterError {
    RouterError::new(RouterErrorKind::InvalidConfiguration, message)
}

fn invalid_token() -> AuthenticationError {
    AuthenticationError::invalid_credential("invalid bearer credential")
}

fn validate_public_resource_url(
    url: &Url,
    allow_insecure_loopback_http: bool,
    resource: &'static str,
) -> Result<(), RouterError> {
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid_configuration(format!(
            "{resource} URL must not contain credentials, a query, or a fragment"
        )));
    }
    match url.scheme() {
        "https" => Ok(()),
        "http" if allow_insecure_loopback_http && is_loopback_url(url) => Ok(()),
        _ => Err(invalid_configuration(format!(
            "{resource} URL must use HTTPS (plain HTTP is available only for explicit loopback development)"
        ))),
    }
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
