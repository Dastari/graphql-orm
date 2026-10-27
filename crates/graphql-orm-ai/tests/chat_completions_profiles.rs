//! Chat Completions provider profiles persist through the configuration
//! service on SQLite and on a test-owned disposable PostgreSQL server.
#![cfg(all(
    feature = "provider-chat-completions-core",
    any(
        all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
        all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
    )
))]

#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "../../graphql-orm/tests/support/owned_postgres.rs"]
mod owned_postgres;

use std::sync::Arc;

use agql_auth::{
    AccessTokenMetadata, AssuranceMatchMode, AuthPrincipal, AuthUser, FixedClock, MfaAcceptance,
    RecentMfaPolicy, SessionAssurance, SessionContext,
};
use async_trait::async_trait;
use graphql_orm::graphql::orm::{ApplyOptions, DefaultWriteBackend, OrmSchemaModule};
use graphql_orm::prelude::Database;
use graphql_orm_ai::*;
use secrecy::SecretString;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const ENDPOINT: &str = "http://127.0.0.1:1234/v1/chat/completions";

struct AllowConfiguration;

#[async_trait]
impl AiConfigurationAccessPolicy for AllowConfiguration {
    async fn can_configure(
        &self,
        _principal: &AuthPrincipal,
        _scope: &AiScope,
        _action: AiConfigurationAction,
    ) -> bool {
        true
    }
}

struct LoopbackChat;

impl AiProviderEndpointPolicy for LoopbackChat {
    fn authorize_endpoint(&self, kind: AiProviderKindInput, normalized_url: &str) -> bool {
        kind == AiProviderKindInput::ChatCompletions && normalized_url == ENDPOINT
    }
}

struct NoSecrets;

#[async_trait]
impl AiSecretStore for NoSecrets {
    async fn resolve(&self, _: &SecretRef) -> Result<SecretString, SecretError> {
        Err(SecretError::Unavailable)
    }
    async fn put(&self, _: Option<&SecretRef>, _: SecretString) -> Result<SecretRef, SecretError> {
        Err(SecretError::Unavailable)
    }
    async fn delete(&self, _: &SecretRef) -> Result<(), SecretError> {
        Ok(())
    }
}

fn principal(now: OffsetDateTime) -> AuthPrincipal {
    let assurance = SessionAssurance::new(
        now,
        ["otp", "pwd"],
        Some("urn:test:loa:2".to_owned()),
        Some("test".to_owned()),
        MfaAcceptance::Satisfied,
    )
    .expect("valid assurance");
    AuthPrincipal::User(AuthUser {
        user_id: "admin-1".to_owned(),
        session_id: Uuid::new_v4(),
        roles: vec!["admin".to_owned()],
        scopes: vec![],
        session: SessionContext::default().with_assurance(assurance),
        token_claims: AccessTokenMetadata {
            auth_time: Some(now.unix_timestamp()),
            amr: Some(vec!["otp".to_owned(), "pwd".to_owned()]),
            acr: Some("urn:test:loa:2".to_owned()),
            tenant_id: Some("tenant-1".to_owned()),
            ..AccessTokenMetadata::default()
        },
    })
}

#[cfg(feature = "sqlite")]
async fn database() -> Database<DefaultWriteBackend> {
    Database::<graphql_orm::prelude::SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .expect("in-memory SQLite should open")
}

#[cfg(feature = "postgres")]
async fn database() -> Database<DefaultWriteBackend> {
    let pool = owned_postgres::OwnedPostgres::pool("ai-chat-profiles", 5)
        .await
        .expect("test-owned PostgreSQL should start");
    Database::<graphql_orm::prelude::PostgresBackend>::new(pool)
}

fn input(retained: bool) -> UpsertAiProviderProfileInput {
    UpsertAiProviderProfileInput {
        id: None,
        scope: AiScopeInput {
            kind: "tenant".to_owned(),
            id: "tenant-1".to_owned(),
            tenant_id: Some("tenant-1".to_owned()),
        },
        provider_kind: AiProviderKindInput::ChatCompletions,
        display_name: "Local model".to_owned(),
        base_url: Some(ENDPOINT.to_owned()),
        openai_compatible: Some(AiOpenAiCompatibleProfileInput {
            retention: "local-none".to_owned(),
            custom_tools: true,
            parallel_tool_calls: false,
            structured_output: true,
            provider_retained_continuation: retained,
        }),
        enabled: true,
        expected_version: None,
    }
}

#[tokio::test]
async fn chat_completions_profiles_round_trip_and_refuse_retention() {
    let database = database().await;
    let module = AiSchemaModule;
    let plan = database
        .schema()
        .plan_migration_to_entities("ai-chat-v1", "AI chat profiles", module.entities())
        .await
        .expect("AI schema should plan");
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect("AI schema should apply");
    let now = OffsetDateTime::from_unix_timestamp(OffsetDateTime::now_utc().unix_timestamp())
        .expect("now");
    let service = OrmAiConfigurationService::new(
        database,
        Arc::new(AllowConfiguration),
        Arc::new(LoopbackChat),
        RecentMfaPolicy {
            maximum_age: Duration::minutes(5),
            clock_skew: Duration::seconds(30),
            allowed_amr: vec!["otp".to_owned()],
            allowed_acr: vec!["urn:test:loa:2".to_owned()],
            match_mode: AssuranceMatchMode::All,
        },
        Arc::new(FixedClock::new(now)),
        Arc::new(NoSecrets),
    );
    let admin = principal(now);

    assert!(matches!(
        service.upsert_provider_profile(&admin, input(true)).await,
        Err(AiError::InvalidInput(_))
    ));
    let mut missing = input(false);
    missing.openai_compatible = None;
    assert!(matches!(
        service.upsert_provider_profile(&admin, missing).await,
        Err(AiError::InvalidInput(_))
    ));

    let created = service
        .upsert_provider_profile(&admin, input(false))
        .await
        .expect("a reviewed Chat Completions profile should save");
    assert_eq!(created.provider_kind, "chat_completions");
    assert_eq!(created.base_url.as_deref(), Some(ENDPOINT));
    assert!(!created.credential_configured);
    let listed = service
        .provider_profiles(
            &admin,
            AiScope::new("tenant", "tenant-1").with_tenant_id("tenant-1"),
        )
        .await
        .expect("profiles should list");
    let view = listed
        .iter()
        .find(|profile| profile.id == created.id)
        .expect("saved profile should be listed");
    let contract = view.openai_compatible.as_ref().expect("reviewed contract");
    assert!(contract.custom_tools && contract.structured_output);
    assert!(!contract.provider_retained_continuation);

    // The persisted view builds a credential-free adapter for the exact endpoint.
    let config = ChatCompletionsProviderConfig::from_profile(view, None)
        .expect("profile view should configure the adapter");
    let provider =
        ChatCompletionsProvider::new(config, Arc::new(LoopbackChat), Arc::new(NoSecrets))
            .expect("authorized endpoint should build");
    assert_eq!(provider.provider_kind(), ProviderKind::ChatCompletions);
    assert!(provider.capabilities().stateless_continuation);
}
