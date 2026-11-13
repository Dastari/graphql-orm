#![cfg(feature = "sqlite")]

//! The crate's own persistence does not depend on what the host's
//! `FieldPolicy` answers for repository field decisions: generated repository
//! reads and writes ask it about every field, without an access context, and
//! the crate answers for its own custody records. Host entities and the
//! handle returned to host code keep the host's answer.

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agql_auth::{AccessTokenMetadata, AuthPrincipal, AuthUser, SessionContext};
use async_trait::async_trait;
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::graphql::orm::{ApplyOptions, OrmSchemaModule};
use graphql_orm::prelude::*;
use graphql_orm_ai::*;
use uuid::Uuid;

/// A host field policy that refuses every repository field decision, as a
/// policy does that denies calls without an access context or entities it
/// does not know. It counts what it was asked about host entities.
#[derive(Clone, Default)]
struct DenyEveryRepositoryField {
    host_questions: Arc<AtomicUsize>,
}
impl DenyEveryRepositoryField {
    fn refuse(&self, entity: &'static str) -> BoxFuture<'static, async_graphql::Result<bool>> {
        if entity == "HostNote" {
            self.host_questions.fetch_add(1, Ordering::SeqCst);
        }
        Box::pin(async { Ok(false) })
    }
}
impl FieldPolicy for DenyEveryRepositoryField {
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_write_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_read_repository_field<'a>(
        &'a self,
        access: Option<AccessContext<'a>>,
        _: &'a Database,
        entity: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        assert!(access.is_none());
        self.refuse(entity)
    }
    fn can_write_repository_field<'a>(
        &'a self,
        access: Option<AccessContext<'a>>,
        _: &'a Database,
        entity: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        assert!(access.is_none());
        self.refuse(entity)
    }
}

/// An entity the host owns, stored beside the AI schema.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "custody_host_notes", plural = "HostNotes")]
struct HostNote {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    body: String,
}

struct AllowAll;

#[async_trait]
impl AiAccessPolicy for AllowAll {
    async fn can_access_scope(
        &self,
        _principal: &AuthPrincipal,
        _scope: &AiScope,
        _action: AiSessionAction,
    ) -> AiAccessDecision {
        AiAccessDecision::allow("test", "test-v1")
    }

    async fn can_access_session(
        &self,
        _principal: &AuthPrincipal,
        _session_id: AiSessionId,
        _action: AiSessionAction,
    ) -> AiAccessDecision {
        AiAccessDecision::allow("test", "test-v1")
    }
}

struct ProtectionPolicy;

#[async_trait]
impl AiContentProtectionPolicyResolver for ProtectionPolicy {
    async fn resolve(
        &self,
        _principal: &AuthPrincipal,
        scope: &AiScope,
    ) -> Result<AiContentProtectionPolicy, AiError> {
        Ok(AiContentProtectionPolicy {
            scope: scope.clone(),
            mode: AiContentProtectionMode::DatabaseManaged,
            key_policy_reference: None,
            version: 1,
            ready: true,
        })
    }
}

fn principal(subject: &str) -> AuthPrincipal {
    AuthPrincipal::User(AuthUser {
        user_id: subject.to_owned(),
        session_id: Uuid::new_v4(),
        roles: vec![],
        scopes: vec![],
        session: SessionContext::default(),
        token_claims: AccessTokenMetadata {
            tenant_id: Some("tenant-1".to_owned()),
            ..AccessTokenMetadata::default()
        },
    })
}

async fn host_database() -> (Database<SqliteBackend>, DenyEveryRepositoryField) {
    let mut database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .expect("in-memory SQLite should open");
    let mut entities = AiSchemaModule.entities().to_vec();
    entities.push(HostNote::metadata());
    let plan = database
        .schema()
        .plan_migration_to_entities("custody-test-v1", "field policy custody test", &entities)
        .await
        .expect("schema migration should plan");
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect("schema migration should apply");
    let policy = DenyEveryRepositoryField::default();
    database.set_field_policy(policy.clone());
    (database, policy)
}

#[tokio::test]
async fn the_crate_persists_its_own_records_under_a_host_policy_that_denies_every_repository_field()
{
    let (database, policy) = host_database().await;
    OrmAiRuntimeRecoveryService::new(database.clone(), Arc::new(agql_auth::SystemClock))
        .readiness_proof()
        .await
        .expect("the recovery service records its first live epoch");
    let service = OrmAiSessionService::new(
        database.clone(),
        Arc::new(AllowAll),
        Arc::new(ProtectionPolicy),
        Arc::new(DatabaseManagedContentProtector),
    );
    let owner = principal("owner");

    // Inserts, reads, updates and compare-and-swap writes of the crate's
    // own records all succeed.
    let session = service
        .create_session(
            &owner,
            CreateAiSessionInput {
                execution_selection: None,
                scope: AiScopeInput {
                    kind: "collection".to_owned(),
                    id: "54".to_owned(),
                    tenant_id: Some("tenant-1".to_owned()),
                },
                title: Some("Research".to_owned()),
            },
        )
        .await
        .expect("the crate creates a session under the denying host policy");
    let sent = service
        .send_message(
            &owner,
            SendAiMessageInput {
                session_id: session.id,
                text: "Find records containing the example term".to_owned(),
                attachment_ids: vec![],
                client_message_id: Uuid::new_v4(),
            },
        )
        .await
        .expect("the crate commits a message and its run");
    assert!(
        service
            .session_for_run(&owner, AiRunId(sent.run_id))
            .await
            .expect("the crate reads its own records")
            .is_some()
    );

    // The handle host code receives still carries the host's own policy:
    // a host entity is refused, and so is an AI record written through it.
    let refused = HostNote::insert(
        service.database(),
        CreateHostNoteInput {
            id: "h1".to_owned(),
            body: "text".to_owned(),
        },
    )
    .await
    .expect_err("the host policy still refuses a host entity");
    assert_eq!(
        OrmPublicError::from_sqlx(&refused).code,
        OrmErrorCode::Forbidden
    );
    assert!(policy.host_questions.load(Ordering::SeqCst) > 0);
    let exposed = service
        .database()
        .ensure_repository_writable_field(None, "AiSessionRecord", "title", None, None, None)
        .await
        .expect_err("the host handle does not carry the crate's custody grant");
    assert_eq!(exposed.code, OrmErrorCode::Forbidden);
}
