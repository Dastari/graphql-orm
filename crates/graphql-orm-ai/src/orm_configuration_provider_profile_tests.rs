//! Provider-profile endpoint identity and credential authority tests.

use std::sync::Mutex;

use agql_auth::{
    AccessTokenMetadata, AssuranceMatchMode, AuthUser, FixedClock, MfaAcceptance, SessionAssurance,
    SessionContext,
};
use graphql_orm::graphql::orm::{ApplyOptions, OrmSchemaModule};
use graphql_orm::prelude::{Database, SqliteBackend};
use time::Duration;

use super::*;
use crate::AiScopeInput;

const TENANT: &str = "tenant-profiles";
const ORIGINAL_ENDPOINT: &str = "https://provider.example/v1";
const OTHER_ENDPOINT: &str = "https://other-provider.example/v1";

/// Grants exactly the listed actions and records every evaluated action.
struct GrantedActions {
    granted: Vec<AiConfigurationAction>,
    evaluated: Mutex<Vec<AiConfigurationAction>>,
}

impl GrantedActions {
    fn new(granted: &[AiConfigurationAction]) -> Arc<Self> {
        Arc::new(Self {
            granted: granted.to_vec(),
            evaluated: Mutex::new(Vec::new()),
        })
    }

    fn evaluated(&self) -> Vec<AiConfigurationAction> {
        self.evaluated
            .lock()
            .expect("evaluated actions lock should not be poisoned")
            .clone()
    }
}

#[async_trait]
impl AiConfigurationAccessPolicy for GrantedActions {
    async fn can_configure(
        &self,
        _principal: &AuthPrincipal,
        _scope: &AiScope,
        action: AiConfigurationAction,
    ) -> bool {
        self.evaluated
            .lock()
            .expect("evaluated actions lock should not be poisoned")
            .push(action);
        self.granted.contains(&action)
    }
}

struct AllowEndpoints;

impl AiProviderEndpointPolicy for AllowEndpoints {
    fn authorize_endpoint(&self, _provider_kind: AiProviderKindInput, _url: &str) -> bool {
        true
    }
}

#[derive(Default)]
struct CountingSecretStore {
    next: Mutex<u32>,
}

#[async_trait]
impl AiSecretStore for CountingSecretStore {
    async fn resolve(&self, _reference: &SecretRef) -> Result<SecretString, crate::SecretError> {
        Err(crate::SecretError::Unavailable)
    }

    async fn put(
        &self,
        _reference: Option<&SecretRef>,
        _value: SecretString,
    ) -> Result<SecretRef, crate::SecretError> {
        let mut next = self
            .next
            .lock()
            .expect("secret counter lock should not be poisoned");
        *next += 1;
        SecretRef::parse(format!("test/profile-key-{next}"))
            .map_err(|_| crate::SecretError::Unavailable)
    }

    async fn delete(&self, _reference: &SecretRef) -> Result<(), crate::SecretError> {
        Ok(())
    }
}

fn now() -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp(1_800_000_000)
        .expect("fixed test timestamp should be valid")
}

fn scope_input() -> AiScopeInput {
    AiScopeInput {
        kind: "tenant".to_owned(),
        id: TENANT.to_owned(),
        tenant_id: Some(TENANT.to_owned()),
    }
}

fn principal() -> AuthPrincipal {
    let now = now();
    let assurance = SessionAssurance::new(
        now,
        ["otp", "pwd"],
        Some("urn:test:loa:2".to_owned()),
        Some("test".to_owned()),
        MfaAcceptance::Satisfied,
    )
    .expect("test assurance should validate");
    AuthPrincipal::User(AuthUser {
        user_id: "profile-admin".to_owned(),
        session_id: Uuid::new_v4(),
        roles: vec!["admin".to_owned()],
        scopes: vec![],
        session: SessionContext::default().with_assurance(assurance),
        token_claims: AccessTokenMetadata {
            auth_time: Some(now.unix_timestamp()),
            amr: Some(vec!["otp".to_owned(), "pwd".to_owned()]),
            acr: Some("urn:test:loa:2".to_owned()),
            tenant_id: Some(TENANT.to_owned()),
            ..AccessTokenMetadata::default()
        },
    })
}

async fn database() -> Database<SqliteBackend> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .expect("in-memory SQLite should open");
    let module = crate::AiSchemaModule;
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "ai-provider-profile-test-v1",
            "AI provider profile test",
            module.entities(),
        )
        .await
        .expect("AI schema migration should plan");
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect("AI schema migration should apply");
    crate::orm_runtime_recovery::open_test_epoch(&database).await;
    database
}

fn service(
    database: &Database<SqliteBackend>,
    access_policy: Arc<dyn AiConfigurationAccessPolicy>,
) -> OrmAiConfigurationService {
    OrmAiConfigurationService::new(
        database.clone(),
        access_policy,
        Arc::new(AllowEndpoints),
        RecentMfaPolicy {
            maximum_age: Duration::minutes(5),
            clock_skew: Duration::seconds(30),
            allowed_amr: vec!["otp".to_owned()],
            allowed_acr: vec!["urn:test:loa:2".to_owned()],
            match_mode: AssuranceMatchMode::All,
        },
        Arc::new(FixedClock::new(now())),
        Arc::new(CountingSecretStore::default()),
    )
}

fn compatible_contract() -> AiOpenAiCompatibleProfileInput {
    AiOpenAiCompatibleProfileInput {
        retention: "zero-retention".to_owned(),
        custom_tools: false,
        parallel_tool_calls: false,
        structured_output: false,
        provider_retained_continuation: false,
    }
}

fn input(
    id: Option<Uuid>,
    expected_version: Option<i64>,
    provider_kind: AiProviderKindInput,
    base_url: Option<&str>,
) -> UpsertAiProviderProfileInput {
    let compatible = matches!(
        provider_kind,
        AiProviderKindInput::OpenAiCompatible | AiProviderKindInput::ChatCompletions
    );
    UpsertAiProviderProfileInput {
        id,
        scope: scope_input(),
        provider_kind,
        display_name: "Primary provider".to_owned(),
        base_url: base_url.map(str::to_owned),
        openai_compatible: compatible.then(compatible_contract),
        enabled: true,
        expected_version,
    }
}

/// Creates a profile and attaches a credential with full authority.
async fn credentialed_profile(
    database: &Database<SqliteBackend>,
    provider_kind: AiProviderKindInput,
    base_url: Option<&str>,
) -> AiProviderProfileView {
    let administrator = service(
        database,
        GrantedActions::new(&[
            AiConfigurationAction::ManageProviderProfiles,
            AiConfigurationAction::ManageProviderCredentials,
        ]),
    );
    let created = administrator
        .upsert_provider_profile(&principal(), input(None, None, provider_kind, base_url))
        .await
        .expect("profile creation should succeed");
    let credentialed = administrator
        .set_provider_credential(
            &principal(),
            created.id,
            SecretString::from("provider-api-key".to_owned()),
            created.row_version,
        )
        .await
        .expect("credential attachment should succeed");
    assert!(credentialed.credential_configured);
    credentialed
}

async fn stored(database: &Database<SqliteBackend>, id: Uuid) -> AiProviderProfileRecord {
    AiProviderProfileRecord::find_by_id(database, &id)
        .await
        .expect("profile lookup should succeed")
        .expect("profile should exist")
}

async fn audit_reasons(database: &Database<SqliteBackend>, id: Uuid) -> Vec<String> {
    let reference = id.to_string();
    let mut events = database
        .transaction(TransactionMode::Default, move |tx| {
            Box::pin(async move {
                tx.query::<AiAuditEventRecord>()
                    .fetch_all()
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await
        .expect("audit query should succeed");
    events.retain(|record| record.resource_reference == reference);
    let mut reasons: Vec<String> = events
        .into_iter()
        .map(|record| record.reason_code)
        .collect();
    reasons.sort();
    reasons
}

fn profiles_only() -> Arc<GrantedActions> {
    GrantedActions::new(&[AiConfigurationAction::ManageProviderProfiles])
}

#[tokio::test]
async fn profile_authority_alone_cannot_move_a_credentialed_endpoint() {
    let database = database().await;
    let profile = credentialed_profile(
        &database,
        AiProviderKindInput::OpenAiCompatible,
        Some(ORIGINAL_ENDPOINT),
    )
    .await;
    let before = stored(&database, profile.id).await;
    let audits_before = audit_reasons(&database, profile.id).await;

    let policy = profiles_only();
    let result = service(&database, policy.clone())
        .upsert_provider_profile(
            &principal(),
            input(
                Some(profile.id),
                Some(profile.row_version),
                AiProviderKindInput::OpenAiCompatible,
                Some(OTHER_ENDPOINT),
            ),
        )
        .await;

    assert!(matches!(result, Err(AiError::Forbidden)), "{result:?}");
    assert!(
        policy
            .evaluated()
            .contains(&AiConfigurationAction::ManageProviderCredentials),
        "the host must be asked for credential authority"
    );
    assert_eq!(stored(&database, profile.id).await, before);
    assert_eq!(audit_reasons(&database, profile.id).await, audits_before);

    // Spellings that survive normalization as a different URL are moves too.
    for spelling in [
        "https://provider.example/v1/",
        "https://provider.example/%761",
    ] {
        let result = service(&database, profiles_only())
            .upsert_provider_profile(
                &principal(),
                input(
                    Some(profile.id),
                    Some(profile.row_version),
                    AiProviderKindInput::OpenAiCompatible,
                    Some(spelling),
                ),
            )
            .await;
        assert!(
            matches!(result, Err(AiError::Forbidden)),
            "{spelling}: {result:?}"
        );
    }
    assert_eq!(stored(&database, profile.id).await, before);
}

#[tokio::test]
async fn profile_authority_alone_cannot_change_a_credentialed_provider_kind() {
    let database = database().await;
    let native = credentialed_profile(&database, AiProviderKindInput::OpenAi, None).await;
    let native_before = stored(&database, native.id).await;
    let compatible = credentialed_profile(
        &database,
        AiProviderKindInput::OpenAiCompatible,
        Some(ORIGINAL_ENDPOINT),
    )
    .await;
    let compatible_before = stored(&database, compatible.id).await;
    let profiles = service(&database, profiles_only());

    // A deployment-fixed native endpoint cannot become a configurable one.
    let native_result = profiles
        .upsert_provider_profile(
            &principal(),
            input(
                Some(native.id),
                Some(native.row_version),
                AiProviderKindInput::OpenAiCompatible,
                Some(OTHER_ENDPOINT),
            ),
        )
        .await;
    assert!(
        matches!(native_result, Err(AiError::Forbidden)),
        "{native_result:?}"
    );
    assert_eq!(stored(&database, native.id).await, native_before);

    // The same URL under another wire protocol is a different destination.
    let kind_result = profiles
        .upsert_provider_profile(
            &principal(),
            input(
                Some(compatible.id),
                Some(compatible.row_version),
                AiProviderKindInput::ChatCompletions,
                Some(ORIGINAL_ENDPOINT),
            ),
        )
        .await;
    assert!(
        matches!(kind_result, Err(AiError::Forbidden)),
        "{kind_result:?}"
    );
    assert_eq!(stored(&database, compatible.id).await, compatible_before);
}

#[tokio::test]
async fn profile_authority_still_edits_a_credentialed_profile_in_place() {
    let database = database().await;
    let profile = credentialed_profile(
        &database,
        AiProviderKindInput::OpenAiCompatible,
        Some(ORIGINAL_ENDPOINT),
    )
    .await;
    let before = stored(&database, profile.id).await;
    let policy = profiles_only();

    let mut update = input(
        Some(profile.id),
        Some(profile.row_version),
        AiProviderKindInput::OpenAiCompatible,
        Some("  HTTPS://Provider.EXAMPLE:443/v1  "),
    );
    update.display_name = "Renamed provider".to_owned();
    update.enabled = false;
    let updated = service(&database, policy.clone())
        .upsert_provider_profile(&principal(), update)
        .await
        .expect("an in-place edit should not need credential authority");

    assert_eq!(updated.display_name, "Renamed provider");
    assert!(!updated.enabled);
    assert!(updated.credential_configured);
    assert_eq!(updated.base_url.as_deref(), Some(ORIGINAL_ENDPOINT));
    assert_eq!(
        stored(&database, profile.id).await.credential_reference,
        before.credential_reference
    );
    assert_eq!(
        policy.evaluated(),
        vec![AiConfigurationAction::ManageProviderProfiles]
    );
}

#[tokio::test]
async fn profile_authority_moves_an_uncredentialed_endpoint() {
    let database = database().await;
    let policy = profiles_only();
    let profiles = service(&database, policy.clone());
    let created = profiles
        .upsert_provider_profile(
            &principal(),
            input(
                None,
                None,
                AiProviderKindInput::OpenAiCompatible,
                Some(ORIGINAL_ENDPOINT),
            ),
        )
        .await
        .expect("profile creation should succeed");
    assert!(!created.credential_configured);

    let moved = profiles
        .upsert_provider_profile(
            &principal(),
            input(
                Some(created.id),
                Some(created.row_version),
                AiProviderKindInput::ChatCompletions,
                Some(OTHER_ENDPOINT),
            ),
        )
        .await
        .expect("an uncredentialed profile can be re-pointed");

    assert_eq!(moved.base_url.as_deref(), Some(OTHER_ENDPOINT));
    assert_eq!(moved.provider_kind, "chat_completions");
    assert!(!moved.credential_configured);
    assert!(
        !policy
            .evaluated()
            .contains(&AiConfigurationAction::ManageProviderCredentials)
    );
}

#[tokio::test]
async fn credential_authority_may_move_a_credentialed_endpoint() {
    let database = database().await;
    let profile = credentialed_profile(
        &database,
        AiProviderKindInput::OpenAiCompatible,
        Some(ORIGINAL_ENDPOINT),
    )
    .await;
    let before = stored(&database, profile.id).await;
    let policy = GrantedActions::new(&[
        AiConfigurationAction::ManageProviderProfiles,
        AiConfigurationAction::ManageProviderCredentials,
    ]);

    let moved = service(&database, policy.clone())
        .upsert_provider_profile(
            &principal(),
            input(
                Some(profile.id),
                Some(profile.row_version),
                AiProviderKindInput::OpenAiCompatible,
                Some(OTHER_ENDPOINT),
            ),
        )
        .await
        .expect("credential authority may re-point a credentialed profile");

    assert_eq!(moved.base_url.as_deref(), Some(OTHER_ENDPOINT));
    assert!(moved.credential_configured);
    assert_eq!(
        stored(&database, profile.id).await.credential_reference,
        before.credential_reference
    );
    assert_eq!(
        policy.evaluated(),
        vec![
            AiConfigurationAction::ManageProviderProfiles,
            AiConfigurationAction::ManageProviderCredentials,
        ]
    );
    assert!(
        audit_reasons(&database, profile.id)
            .await
            .iter()
            .any(|reason| reason == "credentialed_endpoint_changed")
    );
}

#[tokio::test]
async fn stale_version_is_refused_before_credential_authority_is_asked() {
    let database = database().await;
    let profile = credentialed_profile(
        &database,
        AiProviderKindInput::OpenAiCompatible,
        Some(ORIGINAL_ENDPOINT),
    )
    .await;
    let before = stored(&database, profile.id).await;
    let policy = GrantedActions::new(&[
        AiConfigurationAction::ManageProviderProfiles,
        AiConfigurationAction::ManageProviderCredentials,
    ]);

    let stale = service(&database, policy.clone())
        .upsert_provider_profile(
            &principal(),
            input(
                Some(profile.id),
                Some(profile.row_version - 1),
                AiProviderKindInput::OpenAiCompatible,
                Some(OTHER_ENDPOINT),
            ),
        )
        .await;

    assert!(matches!(stale, Err(AiError::Conflict)), "{stale:?}");
    assert_eq!(
        policy.evaluated(),
        vec![AiConfigurationAction::ManageProviderProfiles]
    );
    assert_eq!(stored(&database, profile.id).await, before);
}

#[tokio::test]
async fn in_transaction_decision_refuses_an_unchecked_credentialed_move() {
    let database = database().await;
    let profile = credentialed_profile(
        &database,
        AiProviderKindInput::OpenAiCompatible,
        Some(ORIGINAL_ENDPOINT),
    )
    .await;
    let current = stored(&database, profile.id).await;
    let kind = AiProviderKindInput::OpenAiCompatible.as_str();

    // The row seen inside the transaction carries a credential and the
    // update moves it, but no credential authority was checked beforehand.
    let refused = update_reason_code(&current, kind, Some(OTHER_ENDPOINT), false)
        .expect_err("an unchecked credentialed move must be refused");
    assert_eq!(refused.code, OrmErrorCode::Forbidden);
    assert!(matches!(map_orm(refused), AiError::Forbidden));
    assert!(matches!(
        update_reason_code(&current, "chat_completions", Some(ORIGINAL_ENDPOINT), false),
        Err(error) if error.code == OrmErrorCode::Forbidden
    ));

    assert_eq!(
        update_reason_code(&current, kind, Some(OTHER_ENDPOINT), true)
            .expect("a checked move is allowed"),
        "credentialed_endpoint_changed"
    );
    assert_eq!(
        update_reason_code(&current, kind, Some(ORIGINAL_ENDPOINT), false)
            .expect("an in-place edit needs no credential authority"),
        "configuration_updated"
    );
    let mut uncredentialed = current.clone();
    uncredentialed.credential_reference = None;
    assert_eq!(
        update_reason_code(&uncredentialed, kind, Some(OTHER_ENDPOINT), false)
            .expect("an uncredentialed move needs no credential authority"),
        "configuration_updated"
    );
}

#[tokio::test]
async fn local_harness_conversion_of_a_credentialed_profile_is_refused() {
    let database = database().await;
    let profile = credentialed_profile(&database, AiProviderKindInput::OpenAi, None).await;
    let before = stored(&database, profile.id).await;
    let conversion = || {
        input(
            Some(profile.id),
            Some(profile.row_version),
            AiProviderKindInput::LocalHarness,
            None,
        )
    };

    // Without credential authority the kind change itself is refused first.
    let without_credentials = service(&database, profiles_only())
        .upsert_provider_profile(&principal(), conversion())
        .await;
    assert!(
        matches!(without_credentials, Err(AiError::Forbidden)),
        "{without_credentials:?}"
    );

    // With it, the existing local-harness credential refusal still applies.
    let with_credentials = service(
        &database,
        GrantedActions::new(&[
            AiConfigurationAction::ManageProviderProfiles,
            AiConfigurationAction::ManageProviderCredentials,
        ]),
    )
    .upsert_provider_profile(&principal(), conversion())
    .await;
    assert!(
        matches!(with_credentials, Err(AiError::InvalidInput(_))),
        "{with_credentials:?}"
    );
    assert_eq!(stored(&database, profile.id).await, before);
}
