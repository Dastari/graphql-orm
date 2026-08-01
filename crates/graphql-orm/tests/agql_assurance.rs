#![cfg(feature = "auth-agql")]

use std::sync::Arc;

use agql_auth::{
    AccessTokenMetadata, ApiTokenPrincipal, ApiTokenPrincipalKind, AssuranceMatchMode,
    AssurancePolicyId, AssurancePolicySet, AuthMethod, AuthPrincipal, AuthUser, Clock, FixedClock,
    MfaAcceptance, RecentMfaPolicy, SessionAssurance, SessionContext, SystemClock,
};
use graphql_orm::prelude::*;
use time::{Duration, OffsetDateTime};

struct Query;

#[graphql_orm::async_graphql::Object]
impl Query {
    async fn health(&self) -> bool {
        true
    }
}

struct Mutation;

#[graphql_orm::async_graphql::Object]
impl Mutation {
    #[graphql(guard = "DeclaredAssuranceGuard::new(GraphqlOperationKind::Mutation)")]
    async fn protected_change(&self) -> bool {
        true
    }

    #[graphql(guard = "DeclaredAssuranceGuard::new(GraphqlOperationKind::Mutation)")]
    async fn machine_change(&self) -> bool {
        true
    }

    #[graphql(guard = "DeclaredAssuranceGuard::new(GraphqlOperationKind::Mutation)")]
    async fn revoke_session(&self) -> bool {
        true
    }
}

fn registry() -> OperationAssuranceRegistry {
    let catalog = GraphqlOperationCatalog::compose(std::iter::empty::<(
        &'static [GeneratedGraphqlOperationDescriptor],
        bool,
        bool,
    )>());
    let config = AssuranceSchemaConfig::legacy().with_strict_mutation_classification(true);
    let mut builder = OperationAssuranceRegistry::builder(&catalog, config);
    builder
        .register_custom(
            "custom:protected-change:v1",
            GraphqlOperationKind::Mutation,
            "protectedChange",
            AssuranceActorClass::Interactive,
        )
        .unwrap()
        .require(
            GraphqlOperationKind::Mutation,
            "protectedChange",
            "interactive.recent-auth",
        )
        .unwrap()
        .register_custom(
            "custom:machine-change:v1",
            GraphqlOperationKind::Mutation,
            "machineChange",
            AssuranceActorClass::Machine,
        )
        .unwrap()
        .require(
            GraphqlOperationKind::Mutation,
            "machineChange",
            "interactive.recent-auth",
        )
        .unwrap()
        .register_custom(
            "custom:revoke-session:v1",
            GraphqlOperationKind::Mutation,
            "revokeSession",
            AssuranceActorClass::SafetyTeardown,
        )
        .unwrap()
        .exempt(
            GraphqlOperationKind::Mutation,
            "revokeSession",
            "session revocation must remain available",
        )
        .unwrap();
    builder.build().unwrap()
}

fn enforcement(now: OffsetDateTime) -> AssuranceEnforcement {
    let mut policies = AssurancePolicySet::new();
    policies.insert(
        AssurancePolicyId::new("interactive.recent-auth").unwrap(),
        RecentMfaPolicy {
            maximum_age: Duration::minutes(5),
            clock_skew: Duration::ZERO,
            allowed_amr: vec!["totp".to_string()],
            allowed_acr: vec![],
            match_mode: AssuranceMatchMode::All,
        },
    );
    AssuranceEnforcement::new(
        Arc::new(registry()),
        Arc::new(AgqlAssuranceEvaluator::new(
            Arc::new(policies),
            Arc::new(FixedClock::new(now)),
        )),
    )
}

fn assured_user(now: OffsetDateTime) -> AuthPrincipal {
    let assurance = SessionAssurance::new(
        now,
        ["pwd", "totp"],
        None,
        Some("host-defined".to_string()),
        MfaAcceptance::Satisfied,
    )
    .unwrap();
    AuthPrincipal::User(AuthUser {
        user_id: "user-1".to_string(),
        session_id: graphql_orm::uuid::Uuid::nil(),
        roles: vec![],
        scopes: vec![],
        session: SessionContext::for_auth_method(AuthMethod::Password)
            .with_assurance(assurance.clone()),
        token_claims: AccessTokenMetadata {
            auth_time: Some(assurance.auth_time()),
            amr: Some(assurance.methods.clone()),
            acr: assurance.acr.clone(),
            ..AccessTokenMetadata::default()
        },
    })
}

fn machine_principal(now: OffsetDateTime) -> AuthPrincipal {
    AuthPrincipal::ApiToken(ApiTokenPrincipal {
        token_id: graphql_orm::uuid::Uuid::nil(),
        subject: "machine-1".to_string(),
        principal_kind: ApiTokenPrincipalKind::machine(),
        scopes: vec![],
        audience: None,
        resource_type: None,
        resource_id: None,
        expires_at: now + std::time::Duration::from_secs(300),
    })
}

fn extension_code(response: &graphql_orm::async_graphql::Response) -> Option<String> {
    response
        .errors
        .first()?
        .extensions
        .as_ref()?
        .get("code")
        .map(ToString::to_string)
}

#[tokio::test]
async fn agql_guard_emits_stable_lowercase_code_extension_for_all_denial_categories()
-> Result<(), Box<dyn std::error::Error>> {
    let now = OffsetDateTime::from_unix_timestamp(SystemClock.now().unix_timestamp())?;
    let schema = graphql_orm::async_graphql::Schema::build(
        Query,
        Mutation,
        graphql_orm::async_graphql::EmptySubscription,
    )
    .data(enforcement(now))
    .finish();

    let unauthenticated = schema.execute("mutation { protectedChange }").await;
    assert_eq!(
        extension_code(&unauthenticated).as_deref(),
        Some("\"UNAUTHENTICATED\"")
    );
    assert!(
        unauthenticated.errors[0]
            .extensions
            .as_ref()
            .unwrap()
            .get("CODE")
            .is_none()
    );

    let weak_user = AuthPrincipal::User(AuthUser {
        user_id: "user-1".to_string(),
        session_id: graphql_orm::uuid::Uuid::nil(),
        roles: vec![],
        scopes: vec![],
        session: SessionContext::default(),
        token_claims: AccessTokenMetadata::default(),
    });
    let step_up = schema
        .execute(
            graphql_orm::async_graphql::Request::new("mutation { protectedChange }")
                .data(weak_user),
        )
        .await;
    assert_eq!(
        extension_code(&step_up).as_deref(),
        Some("\"STEP_UP_REQUIRED\"")
    );

    let machine = machine_principal(now);
    let forbidden = schema
        .execute(
            graphql_orm::async_graphql::Request::new("mutation { machineChange }")
                .data(machine.clone()),
        )
        .await;
    assert_eq!(extension_code(&forbidden).as_deref(), Some("\"FORBIDDEN\""));

    let teardown = schema
        .execute(
            graphql_orm::async_graphql::Request::new("mutation { revokeSession }").data(machine),
        )
        .await;
    assert!(teardown.errors.is_empty(), "{:?}", teardown.errors);
    assert_eq!(teardown.data.into_json()?["revokeSession"], true);
    Ok(())
}

#[tokio::test]
async fn agql_guard_accepts_current_host_verified_assurance()
-> Result<(), Box<dyn std::error::Error>> {
    let now = OffsetDateTime::from_unix_timestamp(SystemClock.now().unix_timestamp())?;
    let schema = graphql_orm::async_graphql::Schema::build(
        Query,
        Mutation,
        graphql_orm::async_graphql::EmptySubscription,
    )
    .data(enforcement(now))
    .finish();
    let response = schema
        .execute(
            graphql_orm::async_graphql::Request::new("mutation { protectedChange }")
                .data(assured_user(now)),
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(response.data.into_json()?["protectedChange"], true);
    Ok(())
}
