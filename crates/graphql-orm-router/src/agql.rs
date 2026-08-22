use std::{fmt, sync::Arc, time::UNIX_EPOCH};

use agql_auth::{AccessTokenValidator, AuthError, ScopeMatch};

use crate::{AuthenticatedPrincipal, AuthenticationError, AuthenticationProvider, ScopeMatcher};

/// One-way resource-server adapter over the pinned `agql-auth` validator.
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
        let expires_at = user
            .token_claims
            .expires_at
            .and_then(|expiry| u64::try_from(expiry.unix_timestamp()).ok())
            .and_then(|expiry| UNIX_EPOCH.checked_add(std::time::Duration::from_secs(expiry)))
            .ok_or_else(|| {
                AuthenticationError::invalid_credential("validated token has no usable expiry")
            })?;
        AuthenticatedPrincipal::new(user.user_id, user.scopes, Some(expires_at))
    }
}

/// Router matcher backed by the exact pinned `agql-auth` matcher contract.
#[derive(Clone)]
pub struct AgqlScopeMatcher {
    matcher: Arc<dyn ScopeMatch>,
}

impl AgqlScopeMatcher {
    /// Adapts a host-configured `agql-auth` matcher to router policy.
    pub fn new(matcher: Arc<dyn ScopeMatch>) -> Self {
        Self { matcher }
    }
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

    fn signed_token(key: &EncodingKey, claims: serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("agql-key".to_owned());
        encode(&header, &claims, key).unwrap()
    }
}
