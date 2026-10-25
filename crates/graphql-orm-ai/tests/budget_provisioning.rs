//! Trusted deployment provisioning of budget policies on SQLite and on a
//! test-owned disposable PostgreSQL server.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
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

struct NoEndpoints;

impl AiProviderEndpointPolicy for NoEndpoints {
    fn authorize_endpoint(&self, _: AiProviderKindInput, _: &str) -> bool {
        false
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

fn limits(maximum_policies_per_scope: usize) -> AiBudgetPolicyManagementLimits {
    AiBudgetPolicyManagementLimits::new(
        AiBudgetAmounts {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            tool_units: 10_000,
            image_units: 10_000,
            cost_microunits: 1_000_000_000,
            runs: 100_000,
        },
        maximum_policies_per_scope,
    )
    .expect("budget management limits should validate")
}

fn scope() -> AiScope {
    AiScope::new("project", "provisioned-project").with_tenant_id("tenant-1")
}

fn desired() -> AiDeploymentBudgetPolicy {
    AiDeploymentBudgetPolicy {
        scope: scope(),
        principal_kind: None,
        principal_subject: None,
        interval: AiBudgetIntervalInput::Day,
        maximum_input_tokens: Some(200_000),
        maximum_output_tokens: Some(50_000),
        maximum_tool_units: None,
        maximum_image_units: None,
        maximum_cost_microunits: Some(0),
        maximum_runs: Some(1_000),
        enabled: true,
    }
}

fn recent_principal(now: OffsetDateTime) -> AuthPrincipal {
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
async fn database(_label: &str) -> Database<DefaultWriteBackend> {
    Database::<graphql_orm::prelude::SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .expect("in-memory SQLite should open")
}

#[cfg(feature = "postgres")]
async fn database(label: &str) -> Database<DefaultWriteBackend> {
    let pool = owned_postgres::OwnedPostgres::pool(label, 8)
        .await
        .expect("test-owned PostgreSQL should start");
    Database::<graphql_orm::prelude::PostgresBackend>::new(pool)
}

async fn migrated(label: &str) -> Database<DefaultWriteBackend> {
    let database = database(label).await;
    let module = AiSchemaModule;
    let plan = database
        .schema()
        .plan_migration_to_entities("ai-provisioning-v1", "AI provisioning", module.entities())
        .await
        .expect("AI schema should plan");
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect("AI schema should apply");
    database
}

fn provisioner(
    database: &Database<DefaultWriteBackend>,
    actor: &str,
) -> OrmAiBudgetPolicyProvisioner {
    OrmAiBudgetPolicyProvisioner::new(
        database.clone(),
        AiDeploymentActor::new(actor).expect("actor should validate"),
        limits(2),
    )
}

fn configuration(
    database: &Database<DefaultWriteBackend>,
    now: OffsetDateTime,
) -> OrmAiConfigurationService {
    OrmAiConfigurationService::new(
        database.clone(),
        Arc::new(AllowConfiguration),
        Arc::new(NoEndpoints),
        RecentMfaPolicy {
            maximum_age: Duration::minutes(5),
            clock_skew: Duration::seconds(30),
            allowed_amr: vec!["otp".to_owned()],
            allowed_acr: vec!["urn:test:loa:2".to_owned()],
            match_mode: AssuranceMatchMode::All,
        },
        Arc::new(FixedClock::new(now)),
        Arc::new(NoSecrets),
    )
    .with_budget_policy_management(limits(2))
}

async fn restore_audit_complete(database: &Database<DefaultWriteBackend>) -> bool {
    let pricing = AiPricingCatalogManagementLimits::new(1_000_000, 1_000_000, 10)
        .expect("pricing limits should validate");
    let collected = OrmAiRestoreFactCollector::new(database.clone())
        .with_policy_audits(
            AiRestorePolicyAuditLimits::new(limits(2), pricing, 100, 100, 1_000)
                .expect("restore limits should validate"),
        )
        .collect("module-fingerprint")
        .await
        .expect("restore facts should collect");
    collected
        .audit_statuses()
        .get(&AiRestoreAuditKind::BudgetPolicies)
        == Some(&AiRestoreAuditStatus::Complete)
}

#[tokio::test]
async fn ensure_is_idempotent_and_interactive_edits_win() {
    let database = migrated("ai-provisioning-ensure").await;
    let now = OffsetDateTime::from_unix_timestamp(OffsetDateTime::now_utc().unix_timestamp())
        .expect("now");
    let provisioner = provisioner(&database, "config-file");

    let AiDeploymentBudgetPolicyOutcome::Created(created) = provisioner
        .ensure_budget_policy(desired())
        .await
        .expect("first ensure should create")
    else {
        panic!("first ensure must create");
    };
    assert_eq!(created.row_version, 0);
    assert!(created.enabled);
    assert_eq!(created.maximum_cost_microunits, Some(0));
    assert_eq!(created.id.get_version_num(), 8);
    let unchanged = provisioner.ensure_budget_policy(desired()).await.unwrap();
    assert!(matches!(
        &unchanged,
        AiDeploymentBudgetPolicyOutcome::Unchanged(policy) if policy.id == created.id
    ));
    assert!(restore_audit_complete(&database).await);

    // Changed desired ceilings never overwrite the stored policy.
    let wider = provisioner
        .ensure_budget_policy(AiDeploymentBudgetPolicy {
            maximum_input_tokens: Some(900_000),
            ..desired()
        })
        .await
        .unwrap();
    assert!(matches!(
        &wider,
        AiDeploymentBudgetPolicyOutcome::Diverged(policy)
            if policy.maximum_input_tokens == Some(200_000) && policy.row_version == 0
    ));

    // An MFA-authorized interactive edit wins; provisioning never re-enables it.
    let principal = recent_principal(now);
    let disabled = configuration(&database, now)
        .upsert_budget_policy(
            &principal,
            UpsertAiBudgetPolicyInput {
                id: Some(created.id),
                scope: AiScopeInput {
                    kind: "project".to_owned(),
                    id: "provisioned-project".to_owned(),
                    tenant_id: Some("tenant-1".to_owned()),
                },
                principal_kind: None,
                principal_subject: None,
                interval: AiBudgetIntervalInput::Day,
                maximum_input_tokens: Some(200_000),
                maximum_output_tokens: Some(50_000),
                maximum_tool_units: None,
                maximum_image_units: None,
                maximum_cost_microunits: Some(0),
                maximum_runs: Some(1_000),
                enabled: false,
                expected_version: Some(0),
            },
        )
        .await
        .expect("interactive MFA edit of a provisioned policy should succeed");
    assert!(!disabled.enabled);
    let after = provisioner.ensure_budget_policy(desired()).await.unwrap();
    assert!(matches!(
        &after,
        AiDeploymentBudgetPolicyOutcome::Diverged(policy) if !policy.enabled && policy.row_version == 1
    ));
    assert!(restore_audit_complete(&database).await);
    let visible = configuration(&database, now)
        .budget_policies(&principal, scope())
        .await
        .expect("interactive reads see the provisioned policy");
    assert_eq!(visible.len(), 1);
}

#[tokio::test]
async fn ensure_refuses_invalid_ceilings_and_full_scopes() {
    let database = migrated("ai-provisioning-limits").await;
    let provisioner = provisioner(&database, "config-file");
    for invalid in [
        AiDeploymentBudgetPolicy {
            maximum_input_tokens: Some(1_000_001),
            ..desired()
        },
        AiDeploymentBudgetPolicy {
            maximum_runs: Some(-1),
            ..desired()
        },
        AiDeploymentBudgetPolicy {
            maximum_input_tokens: None,
            maximum_output_tokens: None,
            maximum_cost_microunits: None,
            maximum_runs: None,
            ..desired()
        },
        AiDeploymentBudgetPolicy {
            principal_kind: Some("user".to_owned()),
            ..desired()
        },
        AiDeploymentBudgetPolicy {
            scope: AiScope::new("", "x"),
            ..desired()
        },
    ] {
        assert!(matches!(
            provisioner.ensure_budget_policy(invalid).await,
            Err(AiError::InvalidInput(_))
        ));
    }
    assert!(AiDeploymentActor::new("Not Valid").is_err());

    // The per-scope bound counts interactive and deployment policies alike.
    for interval in [AiBudgetIntervalInput::Day, AiBudgetIntervalInput::Month] {
        provisioner
            .ensure_budget_policy(AiDeploymentBudgetPolicy {
                interval,
                ..desired()
            })
            .await
            .expect("policies within the scope bound should create");
    }
    assert!(matches!(
        provisioner
            .ensure_budget_policy(AiDeploymentBudgetPolicy {
                interval: AiBudgetIntervalInput::Hour,
                ..desired()
            })
            .await,
        Err(AiError::InvalidInput(_))
    ));
    // An existing policy is still reported once the scope is full.
    assert!(matches!(
        provisioner.ensure_budget_policy(desired()).await,
        Ok(AiDeploymentBudgetPolicyOutcome::Unchanged(_))
    ));
}

#[tokio::test]
async fn concurrent_ensures_create_one_policy_and_one_audit_fact() {
    let database = migrated("ai-provisioning-concurrent").await;
    let config_file = provisioner(&database, "config-file");
    let results =
        futures::future::join_all((0..4).map(|_| config_file.ensure_budget_policy(desired())))
            .await;
    let mut created = 0;
    for result in results {
        match result.expect("every concurrent ensure should resolve") {
            AiDeploymentBudgetPolicyOutcome::Created(_) => created += 1,
            AiDeploymentBudgetPolicyOutcome::Unchanged(_) => {}
            other => panic!("unexpected outcome {other:?}"),
        }
    }
    assert_eq!(created, 1);
    // The restore audit requires exactly one creation fact per provisioned row.
    assert!(restore_audit_complete(&database).await);

    // Different actors own different policies for the same scope.
    assert!(matches!(
        provisioner(&database, "operator-file")
            .ensure_budget_policy(desired())
            .await,
        Ok(AiDeploymentBudgetPolicyOutcome::Created(_))
    ));
    assert!(restore_audit_complete(&database).await);
}

/// Pre-provisioning (module 0.72.0) shape of `graphql_orm_ai_budget_policies`: generated
/// ID and no provenance column.
#[derive(
    graphql_orm::prelude::GraphQLEntity,
    graphql_orm::prelude::GraphQLOperations,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
    PartialEq,
)]
#[graphql_entity(
    table = "graphql_orm_ai_budget_policies",
    plural = "LegacyGraphqlOrmAiBudgetPolicies",
    default_sort = "updated_at DESC"
)]
struct LegacyBudgetPolicyRecord {
    #[primary_key]
    #[filterable(type = "uuid")]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    scope_key: String,
    #[filterable(type = "string")]
    scope_kind: String,
    #[filterable(type = "string")]
    scope_id: String,
    tenant_id: Option<String>,
    principal_kind: Option<String>,
    principal_subject: Option<String>,
    interval_kind: String,
    maximum_input_tokens: Option<i64>,
    maximum_output_tokens: Option<i64>,
    maximum_tool_units: Option<i64>,
    maximum_image_units: Option<i64>,
    maximum_cost_microunits: Option<i64>,
    maximum_runs: Option<i64>,
    #[filterable(type = "boolean")]
    enabled: bool,
    #[graphql_orm(version, default = "0")]
    row_version: i64,
    #[sortable]
    updated_at: i64,
}

#[tokio::test]
async fn existing_databases_upgrade_from_the_prior_budget_shape_and_replan_empty() {
    use graphql_orm::graphql::orm::{Entity, ManagedTableSet, OwnedSchemaModel, PlanOptions};

    let database = database("ai-provisioning-upgrade").await;
    let module = AiSchemaModule;
    let prior = module
        .entities()
        .iter()
        .copied()
        .map(|entity| {
            if entity.table_name == "graphql_orm_ai_budget_policies" {
                LegacyBudgetPolicyRecord::metadata()
            } else {
                entity
            }
        })
        .collect::<Vec<_>>();
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "ai-provisioning-v072",
            "AI module before provisioning",
            &prior,
        )
        .await
        .expect("prior module schema should plan");
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect("prior module schema should apply");
    let scope = scope();
    let legacy = LegacyBudgetPolicyRecord::insert(
        &database,
        CreateLegacyBudgetPolicyRecordInput {
            scope_key: ai_scope_key(&scope),
            scope_kind: scope.kind.clone(),
            scope_id: scope.id.clone(),
            tenant_id: scope.tenant_id.clone(),
            principal_kind: None,
            principal_subject: None,
            interval_kind: "month".to_owned(),
            maximum_input_tokens: Some(10),
            maximum_output_tokens: None,
            maximum_tool_units: None,
            maximum_image_units: None,
            maximum_cost_microunits: None,
            maximum_runs: None,
            enabled: true,
        },
    )
    .await
    .expect("a prior-module interactive policy should insert");

    // Upgrade exactly the changed table. Other module tables are untouched,
    // so this proves the budget-policy change independently of unrelated
    // module drift.
    let budget = module
        .entities()
        .iter()
        .copied()
        .filter(|entity| entity.table_name == "graphql_orm_ai_budget_policies")
        .collect::<Vec<_>>();
    assert_eq!(budget.len(), 1);
    let upgrade = database
        .schema()
        .plan_migration_to_entities_with_options(
            "ai-provisioning-budget-upgrade",
            "budget policy provenance",
            &budget,
            PlanOptions::managed_tables_only(),
        )
        .await
        .expect("budget-policy upgrade should plan");
    // The only DDL on both backends is the nullable provenance column.
    assert_eq!(
        upgrade.statements,
        ["ALTER TABLE graphql_orm_ai_budget_policies ADD COLUMN provisioned_by TEXT"]
    );
    assert!(
        upgrade
            .statements
            .iter()
            .any(|statement| statement.contains("provisioned_by")),
        "{:?}",
        upgrade.statements
    );
    database
        .schema()
        .apply_migration(&upgrade, ApplyOptions::default())
        .await
        .expect("budget-policy upgrade should apply to the existing database");
    let replan = database
        .schema()
        .plan_migration_to_entities_with_options(
            "ai-provisioning-budget-replan",
            "replan",
            &budget,
            PlanOptions::managed_tables_only(),
        )
        .await
        .expect("upgraded schema should re-plan");
    assert!(replan.steps.is_empty(), "{:?}", replan.statements);
    let target = OwnedSchemaModel::from(&graphql_orm::graphql::orm::SchemaModel::from_entities(
        &budget,
    ));
    let ownership = ManagedTableSet::new(["graphql_orm_ai_budget_policies".to_owned()])
        .expect("budget table forms a managed set");
    let owned = database
        .schema()
        .plan_owned_migration(
            "ai-owned-budget",
            "certify",
            &target,
            &ownership,
            PlanOptions::strict(),
        )
        .await
        .expect("upgraded table should plan as an owned migration");
    assert!(owned.steps().is_empty(), "{:?}", owned.statements());

    // The pre-existing interactive row survives as interactive provenance and
    // provisioning works on the upgraded database.
    let now = OffsetDateTime::from_unix_timestamp(OffsetDateTime::now_utc().unix_timestamp())
        .expect("now");
    let visible = configuration(&database, now)
        .budget_policies(&recent_principal(now), scope.clone())
        .await
        .expect("upgraded policies should read");
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, legacy.id);
    assert!(matches!(
        provisioner(&database, "config-file")
            .ensure_budget_policy(desired())
            .await,
        Ok(AiDeploymentBudgetPolicyOutcome::Created(_))
    ));
    assert!(restore_audit_complete(&database).await);
}
