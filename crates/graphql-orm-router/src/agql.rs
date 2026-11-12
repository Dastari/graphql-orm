use std::{
    fmt,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use agql_auth::{AccessTokenValidator, AuthError, HierarchicalScopeMatch, ScopeMatch};

use crate::{
    AuthenticatedPrincipal, AuthenticationError, AuthenticationProvider, RouterError,
    RouterErrorKind, ScopeMatcher, auth::is_plain_scope_character,
};

/// One-way resource-server adapter over the pinned `agql-auth` validator.
///
/// The validator does not expose its accepted audiences, so this adapter
/// leaves [`AuthenticationProvider::accepted_audiences`] as `None`. When it
/// authenticates the admin listener, the router cannot check that the admin
/// and public audiences differ; build the two validators with distinct
/// audiences.
pub struct AgqlAuthenticationProvider {
    validator: Arc<AccessTokenValidator>,
}

impl AgqlAuthenticationProvider {
    /// Wraps a host-configured public-key/JWKS access-token validator.
    ///
    /// Standard/legacy scope policy is configured directly on the validator.
    pub fn new(validator: Arc<AccessTokenValidator>) -> Self {
        Self { validator }
    }

    /// Adapts the validator's exact or host-configured matcher to router policy.
    pub fn scope_matcher(&self) -> AgqlScopeMatcher {
        AgqlScopeMatcher::new(self.validator.scope_matcher())
    }
}

impl fmt::Debug for AgqlAuthenticationProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgqlAuthenticationProvider")
            .finish_non_exhaustive()
    }
}

impl AuthenticationProvider for AgqlAuthenticationProvider {
    fn authenticate_bearer(
        &self,
        token: &str,
    ) -> Result<AuthenticatedPrincipal, AuthenticationError> {
        let user = self
            .validator
            .authenticate_access_token(token)
            .map_err(map_agql_error)?;
        let expires_at = user.token_claims.expires_at.ok_or_else(|| {
            AuthenticationError::invalid_credential("validated token has no usable expiry")
        })?;
        let expires_at = checked_system_time(expires_at.unix_timestamp_nanos()).map_err(|_| {
            AuthenticationError::invalid_credential("validated token has no usable expiry")
        })?;
        AuthenticatedPrincipal::new(user.user_id, user.scopes, Some(expires_at))
    }

    fn current_time(&self) -> Result<SystemTime, AuthenticationError> {
        // One sample supplies both seconds and fractional precision.
        checked_system_time(self.validator.current_time().unix_timestamp_nanos())
    }
}

fn checked_system_time(unix_nanos: i128) -> Result<SystemTime, AuthenticationError> {
    let unavailable =
        || AuthenticationError::unavailable("authentication time is not representable");
    let magnitude = unix_nanos.checked_abs().ok_or_else(unavailable)?;
    let seconds = u64::try_from(magnitude / 1_000_000_000).map_err(|_| unavailable())?;
    let nanos = u32::try_from(magnitude % 1_000_000_000).map_err(|_| unavailable())?;
    let duration = Duration::new(seconds, nanos);
    if unix_nanos < 0 {
        UNIX_EPOCH.checked_sub(duration)
    } else {
        UNIX_EPOCH.checked_add(duration)
    }
    .ok_or_else(unavailable)
}

/// Router matcher backed by the exact pinned `agql-auth` matcher contract.
#[derive(Clone)]
pub struct AgqlScopeMatcher {
    matcher: Arc<dyn ScopeMatch>,
}

impl AgqlScopeMatcher {
    /// Adapts a host-configured `agql-auth` matcher to router policy without
    /// checking its syntax.
    ///
    /// # Warning
    ///
    /// The router cannot inspect an opaque matcher. Scope-template values may
    /// contain ASCII letters, digits, `-` and `_`, so a matcher whose
    /// separator or wildcard is made of those characters would split or widen
    /// a rendered scope. For example, separator `-` turns the value `a-b` into
    /// two segments. Use [`hierarchical`](Self::hierarchical) for an `agql-auth`
    /// hierarchical matcher, which refuses that syntax. Pass only matchers
    /// whose separators and wildcards lie outside the template value
    /// characters to this constructor.
    pub fn new(matcher: Arc<dyn ScopeMatch>) -> Self {
        Self { matcher }
    }

    /// Adapts an `agql-auth` hierarchical matcher after checking that its
    /// separator and wildcard cannot appear inside a scope-template value.
    ///
    /// # Errors
    ///
    /// Returns [`RouterErrorKind::InvalidConfiguration`] when the separator is
    /// an ASCII letter, digit, `-` or `_`, or the wildcard consists only of
    /// those characters.
    pub fn hierarchical(matcher: HierarchicalScopeMatch) -> Result<Self, RouterError> {
        let options = matcher.options();
        validate_hierarchical_syntax(options.separator, &options.wildcard)?;
        Ok(Self::new(Arc::new(matcher)))
    }
}

/// Refuses hierarchical syntax that a scope-template value could contain.
pub(crate) fn validate_hierarchical_syntax(
    separator: char,
    wildcard: &str,
) -> Result<(), RouterError> {
    if is_plain_scope_character(separator) {
        return Err(RouterError::new(
            RouterErrorKind::InvalidConfiguration,
            "scope matcher separator must not be a letter, digit, `-` or `_`",
        ));
    }
    if wildcard.chars().all(is_plain_scope_character) {
        return Err(RouterError::new(
            RouterErrorKind::InvalidConfiguration,
            "scope matcher wildcard must contain a character other than a letter, digit, `-` or `_`",
        ));
    }
    Ok(())
}

impl fmt::Debug for AgqlScopeMatcher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AgqlScopeMatcher(..)")
    }
}

impl ScopeMatcher for AgqlScopeMatcher {
    fn matches(&self, granted: &str, required: &str) -> bool {
        self.matcher.matches(granted, required)
    }
}

fn map_agql_error(error: AuthError) -> AuthenticationError {
    if matches!(error, AuthError::AuthServiceUnavailable) {
        AuthenticationError::unavailable("authentication service unavailable")
    } else {
        AuthenticationError::invalid_credential("invalid bearer credential")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agql_auth::{HierarchicalScopeMatch, LegacyScopeClaims};
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode, jwk::Jwk};
    use serde_json::json;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use crate::jwt::tests::RSA_PRIVATE_KEY;

    #[test]
    fn hierarchical_adapter_refuses_syntax_inside_template_values() {
        use agql_auth::HierarchicalScopeOptions;
        assert!(AgqlScopeMatcher::hierarchical(HierarchicalScopeMatch::with_defaults()).is_ok());
        for options in [
            HierarchicalScopeOptions::default().with_separator('-'),
            HierarchicalScopeOptions::default().with_separator('_'),
            HierarchicalScopeOptions::default().with_separator('a'),
            HierarchicalScopeOptions::default().with_wildcard("any"),
            HierarchicalScopeOptions::default().with_wildcard("_"),
        ] {
            let Ok(matcher) = HierarchicalScopeMatch::new(options.clone()) else {
                continue;
            };
            let error = AgqlScopeMatcher::hierarchical(matcher).unwrap_err();
            assert_eq!(
                error.kind(),
                crate::RouterErrorKind::InvalidConfiguration,
                "{options:?}"
            );
        }
        // Each refused shape is constructible, so the adapter is what refuses it.
        assert!(
            HierarchicalScopeMatch::new(HierarchicalScopeOptions::default().with_separator('-'))
                .is_ok()
        );
    }

    #[test]
    fn pinned_validator_adapter_preserves_scope_migration_and_matcher_semantics() {
        let signing_key = EncodingKey::from_rsa_pem(RSA_PRIVATE_KEY.as_bytes()).unwrap();
        let mut jwk = Jwk::from_encoding_key(&signing_key, Algorithm::RS256).unwrap();
        jwk.common.key_id = Some("agql-key".to_owned());
        let jwks_json = serde_json::to_string(&json!({"keys": [jwk]})).unwrap();
        let strict_validator = Arc::new(
            AccessTokenValidator::builder()
                .issuer("https://issuer.test")
                .audience("graphql-router")
                .jwks_json(jwks_json.clone())
                .scope_matcher(Arc::new(HierarchicalScopeMatch::with_defaults()))
                .legacy_scope_claims(LegacyScopeClaims::Reject)
                .build()
                .unwrap(),
        );
        let strict = AgqlAuthenticationProvider::new(strict_validator);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let standard = signed_token(
            &signing_key,
            json!({
                "sub": "agql-user", "sid": "67e55044-10b1-426f-9247-bb680e5fe0c8",
                "roles": [], "iss": "https://issuer.test", "aud": "graphql-router",
                "iat": now, "exp": now + 60, "scope": "products.*"
            }),
        );
        let principal = strict.authenticate_bearer(&standard).unwrap();
        assert_eq!(principal.subject(), "agql-user");
        assert_eq!(principal.scopes(), &["products.*".to_owned()]);
        assert!(
            strict
                .scope_matcher()
                .matches("products.*", "products.7.read")
        );

        let legacy = signed_token(
            &signing_key,
            json!({
                "sub": "agql-user", "sid": "67e55044-10b1-426f-9247-bb680e5fe0c8",
                "roles": [], "scopes": ["products.read"],
                "iss": "https://issuer.test", "aud": "graphql-router",
                "iat": now, "exp": now + 60
            }),
        );
        assert!(strict.authenticate_bearer(&legacy).is_err());
        let migrating_validator = Arc::new(
            AccessTokenValidator::builder()
                .issuer("https://issuer.test")
                .audience("graphql-router")
                .jwks_json(jwks_json)
                .legacy_scope_claims(LegacyScopeClaims::Accept)
                .build()
                .unwrap(),
        );
        let migrating = AgqlAuthenticationProvider::new(migrating_validator);
        assert_eq!(
            migrating.authenticate_bearer(&legacy).unwrap().scopes(),
            &["products.read".to_owned()]
        );
        let conflict = signed_token(
            &signing_key,
            json!({
                "sub": "agql-user", "sid": "67e55044-10b1-426f-9247-bb680e5fe0c8",
                "roles": [], "scopes": [], "scope": "products.read",
                "iss": "https://issuer.test", "aud": "graphql-router",
                "iat": now, "exp": now + 60
            }),
        );
        assert!(migrating.authenticate_bearer(&conflict).is_err());
        assert_eq!(
            migrating.authenticate_bearer("invalid").unwrap_err().kind(),
            crate::AuthenticationErrorKind::InvalidCredential
        );
        assert!(
            principal
                .expires_at()
                .is_some_and(|expiry| expiry > SystemTime::now() + Duration::from_secs(30))
        );
    }

    #[test]
    fn injected_validator_clock_preserves_subscription_lifetime_and_nanoseconds() {
        let signing_key = EncodingKey::from_rsa_pem(RSA_PRIVATE_KEY.as_bytes()).unwrap();
        let mut jwk = Jwk::from_encoding_key(&signing_key, Algorithm::RS256).unwrap();
        jwk.common.key_id = Some("agql-key".to_owned());
        let jwks = serde_json::to_string(&json!({"keys": [jwk]})).unwrap();
        // Both epochs differ substantially from the system clock. No wall-time
        // sleeps or real services participate in this regression.
        for seconds in [1_600_000_000, 4_102_444_800] {
            let now = time::OffsetDateTime::from_unix_timestamp(seconds)
                .unwrap()
                .replace_nanosecond(123_456_789)
                .unwrap();
            let clock = Arc::new(agql_auth::FixedClock::new(now));
            let validator = Arc::new(
                AccessTokenValidator::builder()
                    .issuer("https://issuer.test")
                    .audience("graphql-router")
                    .jwks_json(jwks.clone())
                    .clock(clock.clone())
                    .build()
                    .unwrap(),
            );
            let provider = AgqlAuthenticationProvider::new(validator);
            let token = signed_token(
                &signing_key,
                json!({
                    "sub": "clock-user", "sid": "67e55044-10b1-426f-9247-bb680e5fe0c8",
                    "roles": [], "iss": "https://issuer.test", "aud": "graphql-router",
                    "iat": seconds, "exp": seconds + 60, "scope": "products.read"
                }),
            );
            let principal = provider.authenticate_bearer(&token).unwrap();
            let expected_now = UNIX_EPOCH
                .checked_add(Duration::new(u64::try_from(seconds).unwrap(), 123_456_789))
                .unwrap();
            assert_eq!(provider.current_time().unwrap(), expected_now);
            // These are the exact expiry/current-time inputs used by the
            // WebSocket admission check and expiry scheduler.
            assert_eq!(
                principal
                    .expires_at()
                    .unwrap()
                    .duration_since(provider.current_time().unwrap())
                    .unwrap(),
                Duration::new(59, 876_543_211)
            );
            clock.advance_seconds(10);
            assert_eq!(
                principal
                    .expires_at()
                    .unwrap()
                    .duration_since(provider.current_time().unwrap())
                    .unwrap(),
                Duration::new(49, 876_543_211)
            );
        }
    }

    #[test]
    fn adapter_samples_validator_clock_once_per_time_decision() {
        #[derive(Debug)]
        struct CountingClock {
            reads: std::sync::atomic::AtomicUsize,
            now: time::OffsetDateTime,
        }
        impl agql_auth::Clock for CountingClock {
            fn now(&self) -> time::OffsetDateTime {
                self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                self.now
            }
        }
        let signing_key = EncodingKey::from_rsa_pem(RSA_PRIVATE_KEY.as_bytes()).unwrap();
        let mut jwk = Jwk::from_encoding_key(&signing_key, Algorithm::RS256).unwrap();
        jwk.common.key_id = Some("agql-key".to_owned());
        let clock = Arc::new(CountingClock {
            reads: std::sync::atomic::AtomicUsize::new(0),
            now: time::OffsetDateTime::from_unix_timestamp(1_600_000_000).unwrap(),
        });
        let validator = Arc::new(
            AccessTokenValidator::builder()
                .issuer("https://issuer.test")
                .audience("graphql-router")
                .jwks_json(serde_json::to_string(&json!({"keys": [jwk]})).unwrap())
                .clock(clock.clone())
                .build()
                .unwrap(),
        );
        let provider = AgqlAuthenticationProvider::new(validator);
        clock.reads.store(0, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            provider.current_time().unwrap(),
            UNIX_EPOCH + Duration::from_secs(1_600_000_000)
        );
        assert_eq!(clock.reads.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn checked_time_conversion_preserves_signed_precision_and_rejects_overflow() {
        assert_eq!(
            checked_system_time(1_123_456_789).unwrap(),
            UNIX_EPOCH + Duration::new(1, 123_456_789)
        );
        // A platform without pre-epoch SystemTime support must return a safe
        // unavailable error rather than panic or silently use another clock.
        match UNIX_EPOCH.checked_sub(Duration::new(1, 123_456_789)) {
            Some(expected) => assert_eq!(checked_system_time(-1_123_456_789).unwrap(), expected),
            None => assert_eq!(
                checked_system_time(-1_123_456_789).unwrap_err().kind(),
                crate::AuthenticationErrorKind::Unavailable
            ),
        }
        for unsupported in [i128::MIN, i128::MAX] {
            assert_eq!(
                checked_system_time(unsupported).unwrap_err().kind(),
                crate::AuthenticationErrorKind::Unavailable
            );
        }
    }

    fn signed_token(key: &EncodingKey, claims: serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("agql-key".to_owned());
        encode(&header, &claims, key).unwrap()
    }
}
