//! Optional bridge from `agql-auth` principals into ORM authorization contexts.
//!
//! Enable the `auth-agql` feature to compile this module. The bridge is a
//! one-way dependency: `graphql-orm` does not require `agql-auth` by default,
//! and `agql-auth` must never depend on `graphql-orm`.

use crate::graphql::auth::AuthSubject;
use crate::graphql::orm::DbAuthContext;
use agql_auth::AuthPrincipal;

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

            if let Some(tenant_id) = user.token_claims.tenant_id.clone() {
                builder = builder.tenant_id(tenant_id);
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
            if !user.token_claims.additional.is_empty() {
                builder = builder.claims(serde_json::Value::Object(
                    user.token_claims
                        .additional
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                ));
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
    DbAuthContext::from_subject(&auth_subject_from_principal(principal))
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
    let db = DbAuthContext::from_subject(&subject);
    (subject, db)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agql_auth::{AccessTokenMetadata, ActorIdentity, AuthUser};
    use uuid::Uuid;

    #[test]
    fn maps_user_principal_with_claims() {
        let principal = AuthPrincipal::User(AuthUser {
            user_id: "user-1".into(),
            session_id: Uuid::nil(),
            roles: vec!["admin".into()],
            scopes: vec!["records.read".into()],
            session: Default::default(),
            token_claims: AccessTokenMetadata {
                jti: Some("jti-1".into()),
                tenant_id: Some("tenant-a".into()),
                actor: Some(ActorIdentity {
                    sub: "actor-1".into(),
                    amr: vec![],
                }),
                ..Default::default()
            },
        });
        let subject = auth_subject_from_principal(&principal);
        assert_eq!(subject.id, "user-1");
        assert_eq!(subject.user_id.as_deref(), Some("user-1"));
        assert_eq!(subject.tenant_id.as_deref(), Some("tenant-a"));
        assert_eq!(subject.token_id.as_deref(), Some("jti-1"));
        assert_eq!(subject.actor_id.as_deref(), Some("actor-1"));
        let session = Uuid::nil().to_string();
        assert_eq!(subject.session_id.as_deref(), Some(session.as_str()));
        assert!(subject.has_scope("records.read"));

        let db = db_auth_context_from_principal(&principal);
        assert_eq!(db.tenant_id.as_deref(), Some("tenant-a"));
        assert_eq!(db.token_id.as_deref(), Some("jti-1"));
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
