//! ORM-backed append-only egress decision audit.

#![cfg(any(feature = "sqlite", feature = "postgres"))]

use async_trait::async_trait;
use graphql_orm::db::Database;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::graphql::filters::UuidFilter;
use graphql_orm::graphql::orm::{DefaultWriteBackend, TransactionError, TransactionMode};

use crate::persistence::*;
use crate::{
    AiEgressCapability, AiEgressDecision, AiEgressDecisionAudit, AiEgressManifest, AiEgressOutcome,
    AiEgressReason, AiError, DataClassification,
};

/// Immutable egress audit implemented only through generated `graphql-orm`
/// repository operations.
#[derive(Clone)]
pub struct OrmAiEgressDecisionAudit {
    database: Database<DefaultWriteBackend>,
}

impl OrmAiEgressDecisionAudit {
    /// Creates an ORM-backed egress decision audit.
    pub fn new(database: Database<DefaultWriteBackend>) -> Self {
        Self { database }
    }

    /// Returns the ORM database handle for host schema composition.
    pub fn database(&self) -> &Database<DefaultWriteBackend> {
        &self.database
    }
}

#[async_trait]
impl AiEgressDecisionAudit for OrmAiEgressDecisionAudit {
    async fn record(
        &self,
        manifest: &AiEgressManifest,
        decision: &AiEgressDecision,
    ) -> Result<(), AiError> {
        validate_record(manifest, decision)?;
        let mut retries = 0;
        loop {
            let manifest = manifest.clone();
            let decision = decision.clone();
            let result = self
                .database
                // Check-and-insert is a state-machine decision, including exact replays.
                // SQLite must acquire its write lock before reading the existing record.
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        if let Some(existing) = tx
                            .query::<AiEgressEventRecord>()
                            .filter(AiEgressEventRecordWhereInput {
                                id: Some(UuidFilter {
                                    eq: Some(decision.id.0),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .limit(1)
                            .fetch_one()
                            .await
                            .map_err(OrmPublicError::from)?
                        {
                            if record_matches(&existing, &manifest, &decision) {
                                return Ok(());
                            }
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let classification =
                            classification_value(manifest.maximum_classification()).to_owned();
                        tx.insert::<AiEgressEventRecord>(CreateAiEgressEventRecordInput {
                            id: decision.id.0,
                            run_id: manifest.run_id.map(|run_id| run_id.0),
                            principal_subject: decision.principal_subject,
                            scope_kind: manifest.scope.kind,
                            scope_id: manifest.scope.id,
                            manifest_hash: decision.manifest_hash,
                            destination: manifest.destination,
                            capability: capability_value(manifest.capability).to_owned(),
                            classification,
                            outcome: outcome_value(decision.outcome).to_owned(),
                            reason_code: reason_value(decision.reason).to_owned(),
                            policy_version: decision.policy_version,
                            estimated_bytes: i64::try_from(manifest.estimated_bytes)
                                .map_err(|_| OrmPublicError::new(OrmErrorCode::InvalidInput))?,
                            estimated_tokens: i64::try_from(manifest.estimated_tokens)
                                .map_err(|_| OrmPublicError::new(OrmErrorCode::InvalidInput))?,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                        Ok(())
                    })
                })
                .await;
            match result {
                Err(TransactionError::Retryable(_)) if retries < 3 => {
                    retries += 1;
                    tokio::task::yield_now().await;
                }
                result => return result.map_err(map_transaction),
            }
        }
    }
}

fn validate_record(
    manifest: &AiEgressManifest,
    decision: &AiEgressDecision,
) -> Result<(), AiError> {
    if decision.manifest_hash != manifest.stable_hash()
        || decision.principal_subject.trim().is_empty()
        || decision.principal_subject.len() > 1_024
        || manifest.scope.kind.trim().is_empty()
        || manifest.scope.kind.len() > 128
        || manifest.scope.id.trim().is_empty()
        || manifest.scope.id.len() > 1_024
        || manifest.destination.trim().is_empty()
        || manifest.destination.len() > 1_024
        || decision.policy_version.trim().is_empty()
        || decision.policy_version.len() > 256
        || i64::try_from(manifest.estimated_bytes).is_err()
        || i64::try_from(manifest.estimated_tokens).is_err()
    {
        return Err(AiError::InvalidInput(
            "invalid redacted egress audit event".to_owned(),
        ));
    }
    Ok(())
}

fn record_matches(
    record: &AiEgressEventRecord,
    manifest: &AiEgressManifest,
    decision: &AiEgressDecision,
) -> bool {
    record.id == decision.id.0
        && record.run_id == manifest.run_id.map(|run_id| run_id.0)
        && record.principal_subject == decision.principal_subject
        && record.scope_kind == manifest.scope.kind
        && record.scope_id == manifest.scope.id
        && record.manifest_hash == decision.manifest_hash
        && record.destination == manifest.destination
        && record.capability == capability_value(manifest.capability)
        && record.classification == classification_value(manifest.maximum_classification())
        && record.outcome == outcome_value(decision.outcome)
        && record.reason_code == reason_value(decision.reason)
        && record.policy_version == decision.policy_version
        && u64::try_from(record.estimated_bytes).ok() == Some(manifest.estimated_bytes)
        && u64::try_from(record.estimated_tokens).ok() == Some(manifest.estimated_tokens)
}

const fn capability_value(value: AiEgressCapability) -> &'static str {
    match value {
        AiEgressCapability::ModelInference => "model_inference",
        AiEgressCapability::WebSearch => "web_search",
        AiEgressCapability::ImageAnalysis => "image_analysis",
        AiEgressCapability::ImageGeneration => "image_generation",
        AiEgressCapability::ProviderFile => "provider_file",
        AiEgressCapability::CodeExecution => "code_execution",
        AiEgressCapability::RemoteMcp => "remote_mcp",
        AiEgressCapability::ToolResult => "tool_result",
    }
}

const fn classification_value(value: DataClassification) -> &'static str {
    match value {
        DataClassification::Public => "public",
        DataClassification::Internal => "internal",
        DataClassification::Confidential => "confidential",
        DataClassification::Restricted => "restricted",
        DataClassification::Secret => "secret",
    }
}

const fn outcome_value(value: AiEgressOutcome) -> &'static str {
    match value {
        AiEgressOutcome::Allow => "allow",
        AiEgressOutcome::Deny => "deny",
    }
}

const fn reason_value(value: AiEgressReason) -> &'static str {
    match value {
        AiEgressReason::Allowed => "allowed",
        AiEgressReason::DeploymentDenied => "deployment_denied",
        AiEgressReason::PolicyDenied => "policy_denied",
        AiEgressReason::PrincipalDenied => "principal_denied",
        AiEgressReason::ClassificationDenied => "classification_denied",
        AiEgressReason::SecretDataDenied => "secret_data_denied",
        AiEgressReason::ConsentRequired => "consent_required",
        AiEgressReason::LimitExceeded => "limit_exceeded",
    }
}

fn map_transaction(error: TransactionError) -> AiError {
    let public = error.public_error();
    match public.code {
        OrmErrorCode::InvalidInput
        | OrmErrorCode::CursorInvalid
        | OrmErrorCode::PageLimitExceeded => AiError::InvalidInput(public.message.clone()),
        OrmErrorCode::Unauthenticated | OrmErrorCode::Forbidden => AiError::Forbidden,
        OrmErrorCode::NotFound => AiError::NotFound,
        OrmErrorCode::Conflict | OrmErrorCode::ConstraintViolation => AiError::Conflict,
        OrmErrorCode::ServiceUnavailable
        | OrmErrorCode::InternalError
        | OrmErrorCode::AuthorizationMisconfigured => AiError::PersistenceFailed,
    }
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::*;
    use crate::{AiDestinationTrust, AiScope};
    use graphql_orm::db::ConnectionOptions;
    use graphql_orm::graphql::orm::{ApplyOptions, OrmSchemaModule};
    use graphql_orm::prelude::SqliteBackend;
    use std::sync::Arc;
    use tokio::sync::Barrier;

    async fn fixture() -> (tempfile::TempDir, OrmAiEgressDecisionAudit) {
        let directory = tempfile::tempdir().unwrap();
        let url = format!(
            "sqlite://{}?mode=rwc",
            directory.path().join("audit.sqlite").display()
        );
        let database = Database::<SqliteBackend>::connect_sqlite_with_options(
            url,
            ConnectionOptions::default().max_connections(8),
        )
        .await
        .unwrap();
        let plan = database
            .schema()
            .plan_migration_to_entities(
                "egress-audit-test",
                "Egress audit concurrency",
                crate::AiSchemaModule.entities(),
            )
            .await
            .unwrap();
        database
            .schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await
            .unwrap();
        (directory, OrmAiEgressDecisionAudit::new(database))
    }

    fn manifest() -> AiEgressManifest {
        AiEgressManifest {
            provider_profile_id: "synthetic".into(),
            provider_kind: "local_harness".into(),
            model: "synthetic".into(),
            destination: "synthetic-provider".into(),
            destination_trust: AiDestinationTrust::ManagedProvider,
            capability: AiEgressCapability::ToolResult,
            scope: AiScope::new("workspace", "synthetic"),
            session_id: None,
            run_id: None,
            sources: vec![],
            estimated_bytes: 32,
            estimated_tokens: 0,
            attachment_count: 0,
            purpose: "synthetic test".into(),
            retention: "none".into(),
            residency: None,
            policy_version: "v1".into(),
            consent_reference: None,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_audit_writes_preserve_every_decision() {
        let (_directory, audit) = fixture().await;
        let barrier = Arc::new(Barrier::new(16));
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let audit = audit.clone();
            let barrier = barrier.clone();
            tasks.spawn(async move {
                let manifest = manifest();
                let decision = AiEgressDecision::allow(&manifest, "v1", "synthetic-user");
                barrier.wait().await;
                let result = audit.record(&manifest, &decision).await;
                (decision.id.0, result)
            });
        }
        let mut failures = 0;
        let mut ids = Vec::new();
        while let Some(result) = tasks.join_next().await {
            let (id, result) = result.unwrap();
            ids.push(id);
            failures += usize::from(result.is_err());
        }
        assert_eq!(
            failures, 0,
            "concurrent audit records must not lose a read-to-write race"
        );
        for id in ids {
            let record = audit
                .database()
                .transaction(TransactionMode::Default, move |tx| {
                    Box::pin(async move {
                        tx.query::<AiEgressEventRecord>()
                            .filter(AiEgressEventRecordWhereInput {
                                id: Some(UuidFilter {
                                    eq: Some(id),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .fetch_one()
                            .await
                            .map_err(OrmPublicError::from)
                    })
                })
                .await
                .unwrap();
            assert!(record.is_some(), "every successful audit must be durable");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_exact_replays_remain_idempotent() {
        let (_directory, audit) = fixture().await;
        let manifest = manifest();
        let decision = AiEgressDecision::allow(&manifest, "v1", "synthetic-user");
        let barrier = Arc::new(Barrier::new(16));
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let audit = audit.clone();
            let manifest = manifest.clone();
            let decision = decision.clone();
            let barrier = barrier.clone();
            tasks.spawn(async move {
                barrier.wait().await;
                audit.record(&manifest, &decision).await
            });
        }
        while let Some(result) = tasks.join_next().await {
            result.unwrap().unwrap();
        }
        audit.record(&manifest, &decision).await.unwrap();
    }

    #[tokio::test]
    async fn exact_audit_replay_is_idempotent_but_changed_evidence_is_rejected() {
        let (_directory, audit) = fixture().await;
        let mut manifest = manifest();
        let mut decision = AiEgressDecision::allow(&manifest, "v1", "synthetic-user");
        audit.record(&manifest, &decision).await.unwrap();
        audit.record(&manifest, &decision).await.unwrap();
        manifest.estimated_bytes += 1;
        decision.manifest_hash = manifest.stable_hash();
        assert!(matches!(
            audit.record(&manifest, &decision).await,
            Err(AiError::Conflict)
        ));
    }
}
