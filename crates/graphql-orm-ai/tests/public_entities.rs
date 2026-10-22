#![cfg(feature = "sqlite")]

use agql_auth::{
    AccessTokenMetadata, AuthPrincipal, AuthUser, CurrentPrincipalResolver, PrincipalReference,
    ResolvedPrincipal, SessionContext,
};
use async_trait::async_trait;
use graphql_orm::{async_graphql::Request, graphql::orm::OrmSchemaModule, prelude::*};
use graphql_orm_ai::{public_entities::*, *};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use time::OffsetDateTime;
use uuid::Uuid;

schema_roots! {
    query_custom_ops: [],
    generated_mutations: "none",
    entities: [AiPublicSession, AiPublicMessage, AiPublicRun, AiPublicAttachment, AiPublicToolPolicy],
}

struct Access(AtomicBool);
#[async_trait]
impl AiAccessPolicy for Access {
    async fn can_access_scope(
        &self,
        _: &AuthPrincipal,
        _: &AiScope,
        _: AiSessionAction,
    ) -> AiAccessDecision {
        if self.0.load(Ordering::SeqCst) {
            AiAccessDecision::allow("fixture", "1")
        } else {
            AiAccessDecision::deny("fixture", "1")
        }
    }
    async fn can_access_session(
        &self,
        _: &AuthPrincipal,
        _: AiSessionId,
        _: AiSessionAction,
    ) -> AiAccessDecision {
        if self.0.load(Ordering::SeqCst) {
            AiAccessDecision::allow("fixture", "1")
        } else {
            AiAccessDecision::deny("fixture", "1")
        }
    }
}
struct Protection;
#[async_trait]
impl AiContentProtectionPolicyResolver for Protection {
    async fn resolve(
        &self,
        _: &AuthPrincipal,
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
struct Current {
    principal: AuthPrincipal,
    revoked: AtomicBool,
}
#[async_trait]
impl CurrentPrincipalResolver for Current {
    async fn resolve(
        &self,
        reference: &PrincipalReference,
    ) -> agql_auth::AuthResult<ResolvedPrincipal> {
        if self.revoked.load(Ordering::SeqCst) {
            return Err(agql_auth::AuthError::TokenRevoked);
        }
        ResolvedPrincipal::new(
            reference.clone(),
            self.principal.clone(),
            OffsetDateTime::now_utc(),
        )
    }
}
fn principal(subject: &str, tenant: &str) -> AuthPrincipal {
    AuthPrincipal::User(AuthUser {
        user_id: subject.into(),
        session_id: Uuid::new_v4(),
        roles: vec![],
        scopes: vec![],
        session: SessionContext::default(),
        token_claims: AccessTokenMetadata {
            tenant_id: Some(tenant.into()),
            ..Default::default()
        },
    })
}
async fn fixture() -> (Database<SqliteBackend>, AuthPrincipal, Uuid, Arc<Access>) {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    let module = AiSchemaModule;
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "public-entities-test",
            "disposable AI metadata fixture",
            module.entities(),
        )
        .await
        .unwrap();
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .unwrap();
    let owner = principal("owner", "tenant-1");
    let policy = Arc::new(Access(AtomicBool::new(true)));
    let service = OrmAiSessionService::new(
        database.clone(),
        policy.clone(),
        Arc::new(Protection),
        Arc::new(DatabaseManagedContentProtector),
    );
    let session = service
        .create_session(
            &owner,
            CreateAiSessionInput {
                execution_selection: None,
                scope: AiScopeInput {
                    kind: "workspace".into(),
                    id: "fixture".into(),
                    tenant_id: Some("tenant-1".into()),
                },
                title: Some("Owner session".into()),
            },
        )
        .await
        .unwrap();
    (database, owner, session.id, policy)
}
fn request(actor: &AuthPrincipal, id: Uuid) -> Request {
    Request::new(format!("{{ aiPublicSession(id: \"{id}\") {{ id title messages {{ edges {{ node {{ id }} }} }} }} }}"))
        .data(actor.clone()).data(AuthSubject::new(actor.subject()))
}

#[tokio::test]
async fn generated_reads_and_relations_preserve_owner_and_current_policy() {
    let (database, owner, id, policy) = fixture().await;
    let current = Arc::new(Current {
        principal: owner.clone(),
        revoked: AtomicBool::new(false),
    });
    let access = AiPublicEntityAccess::new(database, policy.clone(), current.clone());
    let schema = schema_builder(access.database()).finish();
    let response = schema.execute(request(&owner, id)).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["aiPublicSession"]["title"],
        "Owner session"
    );
    policy.0.store(false, Ordering::SeqCst);
    let response = schema.execute(request(&owner, id)).await;
    assert!(!format!("{:?}", response.data).contains("Owner session"));
    policy.0.store(true, Ordering::SeqCst);
    current.revoked.store(true, Ordering::SeqCst);
    let response = schema.execute(request(&owner, id)).await;
    assert!(!response.errors.is_empty());
    assert!(!format!("{:?}", response.data).contains("Owner session"));
}

#[tokio::test]
async fn owner_and_tenant_boundaries_do_not_depend_on_a_permissive_scope_policy() {
    let (database, _, id, policy) = fixture().await;
    for actor in [
        principal("stranger", "tenant-1"),
        principal("owner", "tenant-2"),
    ] {
        let current = Arc::new(Current {
            principal: actor.clone(),
            revoked: AtomicBool::new(false),
        });
        let access = AiPublicEntityAccess::new(database.clone(), policy.clone(), current);
        let response = schema_builder(access.database())
            .finish()
            .execute(request(&actor, id))
            .await;
        assert!(!format!("{:?}", response.data).contains("Owner session"));
    }
}

#[tokio::test]
async fn missing_binding_fails_closed_and_schema_never_exposes_custody_columns_or_writes() {
    let (database, owner, id, _) = fixture().await;
    let schema = schema_builder(database).finish();
    assert!(!schema.execute(request(&owner, id)).await.errors.is_empty());
    let sdl = schema.sdl();
    for name in [
        "protectedPreview",
        "protectedPayload",
        "uploadTokenHash",
        "quarantineBlobReference",
        "blobReference",
        "principalReference",
        "leaseGeneration",
        "latestCheckpointId",
        "constraints",
        "createAiPublic",
        "updateAiPublic",
        "deleteAiPublic",
    ] {
        assert!(
            !sdl.contains(name),
            "unexpected custody/write exposure: {name}"
        );
    }
    assert!(
        AiSchemaModule
            .entities()
            .iter()
            .all(|entity| !entity.entity_name.starts_with("AiPublic"))
    );
    for entity in graphql_orm_entity_metadata() {
        assert_eq!(entity.schema_policy, Some(SchemaPolicy::ExternalReadOnly));
        assert!(!entity.backup_enabled);
    }
}
