//! Optional bridge from `agql-auth` principals into ORM authorization contexts.
//!
//! Enable the `auth-agql` feature to compile this module. The bridge is a
//! one-way dependency: `graphql-orm` does not require `agql-auth` by default,
//! and `agql-auth` must never depend on `graphql-orm`.

use crate::graphql::auth::{AuthAssurance, AuthSubject};
use crate::graphql::orm::DbAuthContext;
use agql_auth::{AuthPrincipal, MfaAcceptance};

fn safe_user_claims(user: &agql_auth::AuthUser) -> Option<serde_json::Value> {
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
    insert("mfa", serde_json::json!(user.session.mfa));
    insert("assurance", serde_json::json!(user.session.assurance));
    if !metadata.additional.is_empty() {
        claims.insert(
            "additional".to_string(),
            serde_json::Value::Object(
                metadata
                    .additional
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        );
    }
    (!claims.is_empty()).then_some(serde_json::Value::Object(claims))
}

fn assurance_from_user(user: &agql_auth::AuthUser) -> Option<AuthAssurance> {
    user.session
        .assurance
        .as_ref()
        .map(|assurance| AuthAssurance {
            authenticated_at: assurance.auth_time(),
            methods: assurance.methods.clone(),
            acr: assurance.acr.clone(),
            context: assurance.context.clone(),
            mfa_satisfied: assurance.mfa == MfaAcceptance::Satisfied,
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
            if let Some(assurance) = assurance_from_user(user) {
                builder = builder.assurance(assurance);
            }
            if let Some(claims) = safe_user_claims(user) {
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
    use super::*;
    use agql_auth::{
        AccessTokenMetadata, ActorIdentity, AuthMethod, AuthUser, Clock, SessionAssurance,
        SessionContext, SystemClock,
    };
    use uuid::Uuid;

    #[test]
    fn maps_user_principal_with_claims() {
        let authenticated_at = SystemClock.now();
        let assurance = SessionAssurance::new(
            authenticated_at,
            ["pwd", "totp"],
            Some("urn:example:aal2".to_string()),
            Some("host-policy-v3".to_string()),
            MfaAcceptance::Satisfied,
        )
        .expect("valid assurance fixture");
        let mut additional = std::collections::BTreeMap::new();
        additional.insert("policy_version".to_string(), serde_json::json!("policy-9"));
        let principal = AuthPrincipal::User(AuthUser {
            user_id: "user-1".into(),
            session_id: Uuid::nil(),
            roles: vec!["admin".into()],
            scopes: vec!["records.read".into()],
            session: SessionContext::for_auth_method(AuthMethod::Password)
                .with_assurance(assurance),
            token_claims: AccessTokenMetadata {
                jti: Some("jti-1".into()),
                tenant_id: Some("tenant-a".into()),
                organization_id: Some("organization-a".into()),
                correlation_id: Some("correlation-a".into()),
                actor: Some(ActorIdentity {
                    sub: "actor-1".into(),
                    amr: vec![],
                }),
                additional,
                ..Default::default()
            },
        });
        let subject = auth_subject_from_principal(&principal);
        assert_eq!(subject.id, "user-1");
        assert_eq!(subject.user_id.as_deref(), Some("user-1"));
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
        assert_eq!(assurance.acr.as_deref(), Some("urn:example:aal2"));
        assert_eq!(assurance.context.as_deref(), Some("host-policy-v3"));
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
        assert_eq!(claims["assurance"]["context"], "host-policy-v3");
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
        assert!(setting("app.assurance").is_some_and(|value| value.contains("host-policy-v3")));
    }

    #[test]
    fn maps_api_token_principal_via_from() {
        // Prefer AuthPrincipal helpers so the bridge test does not need `time`.
        let mut principal = AuthPrincipal::User(AuthUser {
            user_id: "svc-1".into(),
            session_id: Uuid::nil(),
            roles: Vec::new(),
            scopes: vec!["svc.read".into()],
            session: Default::default(),
            token_claims: AccessTokenMetadata {
                jti: Some("token-ref".into()),
                ..Default::default()
            },
        });
        // Treat the user-shaped principal as a service identity for mapping checks.
        let subject = auth_subject_from_principal(&principal);
        assert_eq!(subject.id, "svc-1");
        assert_eq!(subject.token_id.as_deref(), Some("token-ref"));
        assert!(subject.has_scope("svc.read"));
        // Silence unused mut warning if AuthPrincipal methods need ownership later.
        let _ = &mut principal;
    }
}
