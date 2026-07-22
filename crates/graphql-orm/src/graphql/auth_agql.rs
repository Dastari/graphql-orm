//! Optional bridge from `agql-auth` principals into ORM authorization contexts.
//!
//! Enable the `auth-agql` feature to compile this module. The bridge is a
//! one-way dependency: `graphql-orm` does not require `agql-auth` by default,
//! and `agql-auth` must never depend on `graphql-orm`.
//! It projects only an already accepted [`agql_auth::AuthPrincipal`]; it does
//! not create or evaluate OIDC requests/outcomes, persist rate-limit state,
//! mint tokens, infer MFA, or own product authorization policy.

use crate::graphql::auth::{AuthAssurance, AuthSubject};
use crate::graphql::orm::DbAuthContext;
use agql_auth::{AuthPrincipal, MfaAcceptance};

fn safe_user_claims(
    user: &agql_auth::AuthUser,
    has_consistent_assurance: bool,
) -> Option<serde_json::Value> {
    let metadata = &user.token_claims;
    let mut claims = serde_json::Map::new();
    let mut insert = |name: &str, value: serde_json::Value| {
        if !value.is_null() {
            claims.insert(name.to_string(), value);
        }
    };
    insert(
        "organization_id",
        serde_json::json!(metadata.organization_id),
    );
    insert(
        "session_family_id",
        serde_json::json!(metadata.session_family_id),
    );
    insert("correlation_id", serde_json::json!(metadata.correlation_id));
    insert("auth_time", serde_json::json!(metadata.auth_time));
    insert("amr", serde_json::json!(metadata.amr));
    insert("acr", serde_json::json!(metadata.acr));
    insert("actor", serde_json::json!(metadata.actor));
    insert("resource_type", serde_json::json!(metadata.resource_type));
    insert("resource_id", serde_json::json!(metadata.resource_id));
    insert("purpose", serde_json::json!(metadata.purpose));
    insert("active_scope", serde_json::json!(user.session.active_scope));
    insert("auth_method", serde_json::json!(user.session.auth_method));
    if has_consistent_assurance {
        insert("mfa", serde_json::json!(user.session.mfa));
        insert("assurance", serde_json::json!(user.session.assurance));
    }
    if let Some(policy_version) = metadata
        .additional
        .get("policy_version")
        .and_then(serde_json::Value::as_str)
    {
        claims.insert(
            "additional".to_string(),
            serde_json::json!({ "policy_version": policy_version }),
        );
    }
    (!claims.is_empty()).then_some(serde_json::Value::Object(claims))
}

fn assurance_from_user(user: &agql_auth::AuthUser) -> Option<AuthAssurance> {
    let assurance = user.session.assurance.as_ref()?;
    assurance.validate().ok()?;

    let mfa_satisfied = assurance.mfa == MfaAcceptance::Satisfied;
    if user.session.mfa.satisfied != mfa_satisfied
        || user.token_claims.auth_time != Some(assurance.auth_time())
        || user.token_claims.amr.as_deref() != Some(assurance.methods.as_slice())
        || user.token_claims.acr.as_deref() != assurance.acr.as_deref()
    {
        return None;
    }

    Some(AuthAssurance {
        authenticated_at: assurance.auth_time(),
        methods: assurance.methods.clone(),
        acr: assurance.acr.clone(),
        context: assurance.context.clone(),
        mfa_satisfied,
    })
}

/// Convert an `agql-auth` principal into a project-agnostic [`AuthSubject`].
///
/// Mapping rules:
/// - subject → `AuthSubject.id`
/// - user id (user principals) → `user_id`
/// - roles / scopes → copied and deterministically deduplicated
/// - session id / token id / jti → string references only (never raw secrets)
/// - tenant / organization from access-token metadata when present
/// - actor identity from token claims when present
pub fn auth_subject_from_principal(principal: &AuthPrincipal) -> AuthSubject {
    match principal {
        AuthPrincipal::User(user) => {
            let assurance = assurance_from_user(user);
            let mut builder = AuthSubject::builder(user.user_id.clone())
                .user_id(user.user_id.clone())
                .roles(user.roles.clone())
                .scopes(user.scopes.clone())
                .session_id(user.session_id.to_string());

            if let Some(tenant_id) = user.token_claims.tenant_id.clone().or_else(|| {
                user.session
                    .active_scope
                    .as_ref()
                    .and_then(|scope| scope.tenant_id.clone())
            }) {
                builder = builder.tenant_id(tenant_id);
            }
            if let Some(organization_id) = user.token_claims.organization_id.clone().or_else(|| {
                user.session
                    .active_scope
                    .as_ref()
                    .and_then(|scope| scope.organization_id.clone())
            }) {
                builder = builder.organization_id(organization_id);
            }
            if let Some(correlation_id) = user.token_claims.correlation_id.clone() {
                builder = builder.correlation_id(correlation_id);
            }
            if let Some(token_id) = user
                .token_claims
                .jti
                .clone()
                .or_else(|| principal.token_ref())
            {
                builder = builder.token_id(token_id);
            }
            if let Some(actor) = user.token_claims.actor.as_ref() {
                builder = builder.actor_id(actor.sub.clone());
            }
            if let Some(assurance) = assurance.clone() {
                builder = builder.assurance(assurance);
            }
            if let Some(claims) = safe_user_claims(user, assurance.is_some()) {
                builder = builder.claims(claims);
            }
            builder.build()
        }
        AuthPrincipal::ApiToken(token) => AuthSubject::builder(token.subject.clone())
            .scopes(token.scopes.clone())
            .token_id(token.token_id.to_string())
            .build(),
    }
}

/// Convert an `agql-auth` principal into a [`DbAuthContext`] for RLS and
/// structural authorization.
pub fn db_auth_context_from_principal(principal: &AuthPrincipal) -> DbAuthContext {
    let mut context = DbAuthContext::from_subject(&auth_subject_from_principal(principal));
    if let AuthPrincipal::User(user) = principal {
        context.policy_version = user
            .token_claims
            .additional
            .get("policy_version")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
    }
    context
}

/// Convert an `AuthPrincipal` and optional tenant override into a database auth
/// context. The override wins when provided.
pub fn db_auth_context_from_principal_with_tenant(
    principal: &AuthPrincipal,
    tenant_id: Option<String>,
) -> DbAuthContext {
    let mut context = db_auth_context_from_principal(principal);
    if tenant_id.is_some() {
        context.tenant_id = tenant_id;
    }
    context
}

/// Convert an `AuthPrincipal` into both GraphQL and database auth values.
pub fn auth_bundle_from_principal(principal: &AuthPrincipal) -> (AuthSubject, DbAuthContext) {
    let subject = auth_subject_from_principal(principal);
    let mut db = DbAuthContext::from_subject(&subject);
    if let AuthPrincipal::User(user) = principal {
        db.policy_version = user
            .token_claims
            .additional
            .get("policy_version")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
    }
    (subject, db)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use agql_auth::{
        AccessTokenMetadata, ActiveScope, ActorIdentity, ApiTokenPrincipal, ApiTokenPrincipalKind,
        AuthMethod, AuthUser, Clock, MfaState, OidcAuthorizationOutcome, SessionAssurance,
        SessionContext, SystemClock,
    };
    use uuid::Uuid;

    fn user_principal(session: SessionContext, token_claims: AccessTokenMetadata) -> AuthPrincipal {
        AuthPrincipal::User(AuthUser {
            user_id: "user-1".into(),
            session_id: Uuid::nil(),
            roles: vec!["operator".into(), "admin".into(), "operator".into()],
            scopes: vec![
                "records.write".into(),
                "records.read".into(),
                "records.read".into(),
            ],
            session,
            token_claims,
        })
    }

    fn matching_token_claims(
        assurance: &SessionAssurance,
        additional: BTreeMap<String, serde_json::Value>,
    ) -> AccessTokenMetadata {
        AccessTokenMetadata {
            auth_time: Some(assurance.auth_time()),
            amr: Some(assurance.methods.clone()),
            acr: assurance.acr.clone(),
            additional,
            ..Default::default()
        }
    }

    fn assert_no_orm_assurance(principal: &AuthPrincipal) {
        let (subject, db) = auth_bundle_from_principal(principal);
        assert!(subject.assurance.is_none());
        assert!(db.assurance.is_none());
        let claims = subject
            .claims
            .as_ref()
            .expect("safe identity claims remain");
        assert!(claims.get("assurance").is_none());
        assert!(claims.get("mfa").is_none());
    }

    #[test]
    fn maps_host_accepted_assurance_and_identity_fields_exactly() {
        let authenticated_at = SystemClock.now();
        let standard_acr = "urn:example:aal2";
        let policy_context = "microsoft-entra/acrs/c1";
        let assurance = SessionAssurance::new(
            authenticated_at,
            ["pwd", "totp"],
            Some(standard_acr.to_string()),
            Some(policy_context.to_string()),
            MfaAcceptance::Satisfied,
        )
        .expect("valid assurance fixture");
        let mut additional = BTreeMap::new();
        additional.insert("policy_version".to_string(), serde_json::json!("policy-9"));
        let mut token_claims = matching_token_claims(&assurance, additional);
        token_claims.jti = Some("jti-1".into());
        token_claims.tenant_id = Some("tenant-a".into());
        token_claims.organization_id = Some("organization-a".into());
        token_claims.correlation_id = Some("correlation-a".into());
        token_claims.actor = Some(ActorIdentity {
            sub: "actor-1".into(),
            amr: vec!["actor-method".into()],
        });
        let mut session =
            SessionContext::for_auth_method(AuthMethod::Password).with_assurance(assurance);
        session.active_scope = Some(ActiveScope {
            tenant_id: Some("fallback-tenant".into()),
            organization_id: Some("fallback-organization".into()),
            catalog_id: Some("catalog-a".into()),
        });
        let principal = user_principal(session, token_claims);

        let subject = auth_subject_from_principal(&principal);
        assert_eq!(subject.id, "user-1");
        assert_eq!(subject.user_id.as_deref(), Some("user-1"));
        assert_eq!(subject.roles, ["admin", "operator"]);
        assert_eq!(subject.scopes, ["records.read", "records.write"]);
        assert_eq!(subject.tenant_id.as_deref(), Some("tenant-a"));
        assert_eq!(subject.token_id.as_deref(), Some("jti-1"));
        assert_eq!(subject.actor_id.as_deref(), Some("actor-1"));
        assert_eq!(subject.organization_id.as_deref(), Some("organization-a"));
        assert_eq!(subject.correlation_id.as_deref(), Some("correlation-a"));
        let assurance = subject.assurance.as_ref().expect("assurance survives");
        assert_eq!(
            assurance.authenticated_at,
            authenticated_at.unix_timestamp()
        );
        assert_eq!(assurance.methods, ["pwd", "totp"]);
        assert_eq!(assurance.acr.as_deref(), Some(standard_acr));
        assert_eq!(assurance.context.as_deref(), Some(policy_context));
        assert_ne!(assurance.acr, assurance.context);
        assert!(assurance.mfa_satisfied);
        let session = Uuid::nil().to_string();
        assert_eq!(subject.session_id.as_deref(), Some(session.as_str()));
        assert!(subject.has_scope("records.read"));

        let db = db_auth_context_from_principal(&principal);
        assert_eq!(db.tenant_id.as_deref(), Some("tenant-a"));
        assert_eq!(db.token_id.as_deref(), Some("jti-1"));
        assert_eq!(db.correlation_id.as_deref(), Some("correlation-a"));
        assert_eq!(db.policy_version.as_deref(), Some("policy-9"));
        assert_eq!(db.assurance, subject.assurance);
        let claims = db.claims_json.as_ref().expect("safe claims survive");
        assert_eq!(claims["additional"]["policy_version"], "policy-9");
        assert_eq!(claims["acr"], standard_acr);
        assert_eq!(claims["assurance"]["acr"], standard_acr);
        assert_eq!(claims["assurance"]["context"], policy_context);
        assert_ne!(claims["assurance"]["acr"], claims["assurance"]["context"]);
        let settings = db.postgres_settings().expect("database settings");
        let setting = |name| {
            settings
                .iter()
                .find(|(candidate, _)| *candidate == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(setting("app.tenant_id"), Some("tenant-a"));
        assert_eq!(setting("app.organization_id"), Some("organization-a"));
        assert_eq!(setting("app.correlation_id"), Some("correlation-a"));
        assert_eq!(setting("app.policy_version"), Some("policy-9"));
        assert!(setting("app.assurance").is_some_and(|value| value.contains(policy_context)));
    }

    #[test]
    fn acr_and_acrs_context_are_not_synthesized_when_absent() {
        let authenticated_at = SystemClock.now();
        let assurance = SessionAssurance::new(
            authenticated_at,
            ["pwd"],
            None,
            None,
            MfaAcceptance::Satisfied,
        )
        .expect("valid assurance fixture");
        let token_claims = matching_token_claims(&assurance, BTreeMap::new());
        let principal = user_principal(
            SessionContext::for_auth_method(AuthMethod::Password).with_assurance(assurance),
            token_claims,
        );

        let (subject, db) = auth_bundle_from_principal(&principal);
        let mapped = subject.assurance.as_ref().expect("assurance maps");
        assert!(mapped.acr.is_none());
        assert!(mapped.context.is_none());
        assert_eq!(db.assurance, subject.assurance);
        let claims = subject.claims.as_ref().expect("safe claims");
        assert!(claims.get("acr").is_none());
        assert!(claims["assurance"].get("acr").is_none());
        assert!(claims["assurance"].get("context").is_none());
    }

    #[test]
    fn provider_acrs_outcome_without_session_assurance_grants_nothing() {
        let provider_context = "microsoft-entra/acrs/c1";
        let outcome = OidcAuthorizationOutcome {
            policy: None,
            enforced_auth_time: None,
            matched_acr: None,
            matched_acrs: Some(provider_context.to_string()),
        };
        assert_eq!(outcome.matched_acrs.as_deref(), Some(provider_context));

        let mut additional = BTreeMap::new();
        additional.insert("matched_acrs".into(), serde_json::json!(provider_context));
        let principal = user_principal(
            SessionContext::for_auth_method(AuthMethod::MicrosoftOidc),
            AccessTokenMetadata {
                acr: Some("urn:standard:acr".into()),
                additional,
                ..Default::default()
            },
        );
        let (subject, db) = auth_bundle_from_principal(&principal);
        assert!(subject.assurance.is_none());
        assert!(db.assurance.is_none());
        assert!(!subject.roles.iter().any(|value| value == provider_context));
        assert!(!subject.scopes.iter().any(|value| value == provider_context));
        assert_ne!(subject.tenant_id.as_deref(), Some(provider_context));
        let serialized = serde_json::to_string(&subject.claims).expect("serialize safe claims");
        assert!(!serialized.contains(provider_context));
    }

    #[test]
    fn malformed_inconsistent_and_missing_metadata_assurance_fail_closed() {
        let authenticated_at = SystemClock.now();
        let malformed = SessionAssurance {
            authenticated_at,
            methods: vec!["PWD".into()],
            acr: Some("urn:example:aal2".into()),
            context: Some("host-policy".into()),
            mfa: MfaAcceptance::Satisfied,
        };
        let malformed_principal = user_principal(
            SessionContext {
                auth_method: AuthMethod::Password,
                mfa: MfaState {
                    satisfied: true,
                    methods: Vec::new(),
                },
                assurance: Some(malformed.clone()),
                active_scope: None,
            },
            matching_token_claims(&malformed, BTreeMap::new()),
        );
        assert_no_orm_assurance(&malformed_principal);

        let valid = SessionAssurance::new(
            authenticated_at,
            ["pwd", "totp"],
            Some("urn:example:aal2".into()),
            Some("host-policy".into()),
            MfaAcceptance::Satisfied,
        )
        .expect("valid fixture");
        let mut inconsistent_session =
            SessionContext::for_auth_method(AuthMethod::Password).with_assurance(valid.clone());
        inconsistent_session.mfa.satisfied = false;
        let inconsistent_session_principal = user_principal(
            inconsistent_session,
            matching_token_claims(&valid, BTreeMap::new()),
        );
        assert_no_orm_assurance(&inconsistent_session_principal);

        let mut inconsistent_claims = matching_token_claims(&valid, BTreeMap::new());
        inconsistent_claims.amr = Some(vec!["pwd".into()]);
        let inconsistent_token_principal = user_principal(
            SessionContext::for_auth_method(AuthMethod::Password).with_assurance(valid.clone()),
            inconsistent_claims,
        );
        assert_no_orm_assurance(&inconsistent_token_principal);

        let missing_metadata_principal = user_principal(
            SessionContext::for_auth_method(AuthMethod::Password).with_assurance(valid),
            AccessTokenMetadata::default(),
        );
        assert_no_orm_assurance(&missing_metadata_principal);
    }

    #[test]
    fn unsatisfied_mfa_acceptance_remains_an_exact_negative_decision() {
        let assurance = SessionAssurance::new(
            SystemClock.now(),
            ["pwd"],
            None,
            Some("password-only".into()),
            MfaAcceptance::Unsatisfied,
        )
        .expect("valid negative assurance fixture");
        let claims = matching_token_claims(&assurance, BTreeMap::new());
        let principal = user_principal(
            SessionContext::for_auth_method(AuthMethod::Password).with_assurance(assurance),
            claims,
        );
        let (subject, db) = auth_bundle_from_principal(&principal);
        let mapped = subject.assurance.as_ref().expect("negative decision maps");
        assert!(!mapped.mfa_satisfied);
        assert_eq!(db.assurance, subject.assurance);
        assert!(
            !subject.claims.as_ref().expect("claims")["mfa"]["satisfied"]
                .as_bool()
                .expect("MFA satisfaction must be boolean")
        );
    }

    #[test]
    fn api_tokens_never_gain_user_assurance_or_authority() {
        let principal = AuthPrincipal::ApiToken(ApiTokenPrincipal {
            token_id: Uuid::nil(),
            subject: "svc-1".into(),
            principal_kind: ApiTokenPrincipalKind::service(),
            scopes: vec!["svc.write".into(), "svc.read".into(), "svc.read".into()],
            audience: Some("api".into()),
            resource_type: Some("record".into()),
            resource_id: Some("record-1".into()),
            expires_at: SystemClock.now(),
        });
        let (subject, db) = auth_bundle_from_principal(&principal);
        let nil_token_id = Uuid::nil().to_string();
        assert_eq!(subject.id, "svc-1");
        assert_eq!(subject.token_id.as_deref(), Some(nil_token_id.as_str()));
        assert_eq!(subject.scopes, ["svc.read", "svc.write"]);
        assert!(subject.has_scope("svc.read"));
        assert!(subject.roles.is_empty());
        assert!(subject.user_id.is_none());
        assert!(subject.tenant_id.is_none());
        assert!(subject.assurance.is_none());
        assert!(subject.claims.is_none());
        assert!(db.assurance.is_none());
        assert!(db.policy_version.is_none());
    }

    #[test]
    fn debug_and_serialized_contexts_exclude_provider_and_credential_artifacts() {
        let sensitive = [
            ("raw_jwt", "eyJ.raw-jwt.signature"),
            ("refresh_token", "refresh-token-secret"),
            ("oauth_state", "oauth-state-secret"),
            ("nonce", "nonce-secret"),
            ("authorization_code", "authorization-code-secret"),
            (
                "authorization_url",
                "https://provider.invalid/authorize?secret=1",
            ),
            ("claims_request", "{essential-acrs-claims-request}"),
            ("cookie", "session=cookie-secret"),
            ("provider_response", "provider-response-secret"),
        ];
        let mut additional = BTreeMap::new();
        additional.insert("policy_version".into(), serde_json::json!("policy-9"));
        for (name, value) in sensitive {
            additional.insert(name.into(), serde_json::json!({ "secret": value }));
        }
        let principal = user_principal(
            SessionContext::for_auth_method(AuthMethod::MicrosoftOidc),
            AccessTokenMetadata {
                jti: Some("safe-token-reference".into()),
                additional,
                ..Default::default()
            },
        );

        let (subject, db) = auth_bundle_from_principal(&principal);
        assert_eq!(db.policy_version.as_deref(), Some("policy-9"));
        assert_eq!(
            subject.claims.as_ref().expect("safe claims")["additional"],
            serde_json::json!({ "policy_version": "policy-9" })
        );
        let output = format!(
            "{subject:?}\n{db:?}\n{}\n{}\n{}",
            serde_json::to_string(&subject.claims).expect("serialize subject claims"),
            serde_json::to_string(&db.claims_json).expect("serialize database claims"),
            serde_json::to_string(&db.postgres_settings().expect("database settings"))
                .expect("serialize settings"),
        );
        for (_, secret) in sensitive {
            assert!(!output.contains(secret), "bridge output leaked {secret}");
        }
    }
}
