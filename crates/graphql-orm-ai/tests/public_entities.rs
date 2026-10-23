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
    // Callback-only row authorization needs a bounded scan for counts and
    // relation pages. This deterministic key belongs only to this fixture.
    let database =
        database.with_authorized_scan_config(AuthorizedScanConfig::new([7; 32], 32, 2048));
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
async fn public_batch_identity_preserves_uuid_type_from_owned_rows() {
    use graphql_orm::graphql::loaders::RelationKey;

    let (database, _, id, _) = fixture().await;
    let rows = <SqliteBackend as OrmBackend>::fetch_rows(
        database.pool(),
        "SELECT * FROM graphql_orm_ai_sessions WHERE id = ?",
        &[SqlValue::Uuid(id)],
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    let key =
        <AiPublicSession as BatchLoadEntity<SqliteBackend>>::batch_relation_key_from_row(&rows[0])
            .unwrap();
    let expected = RelationKey::from_sql_values(&[SqlValue::Uuid(id)]);
    assert_eq!(key, expected);
    assert_ne!(key, RelationKey::single(id.to_string()));
    assert_eq!(
        <AiPublicSession as BatchLoadEntity<SqliteBackend>>::batch_key_from_row(&rows[0]).unwrap(),
        id.to_string()
    );
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

use graphql_orm::futures::{Stream, StreamExt, future::poll_fn};
use std::task::Poll;
use tokio::time::{Duration, timeout};

async fn registered<S: Stream<Item = graphql_orm::async_graphql::Response> + Unpin>(
    stream: &mut S,
) {
    poll_fn(|cx| match stream.poll_next_unpin(cx) {
        Poll::Pending => Poll::Ready(()),
        other => panic!("unexpected initial event {other:?}"),
    })
    .await;
}
fn sessions(database: Database<SqliteBackend>, policy: Arc<Access>) -> OrmAiSessionService {
    OrmAiSessionService::new(
        database,
        policy,
        Arc::new(Protection),
        Arc::new(DatabaseManagedContentProtector),
    )
}
fn subscription(actor: &AuthPrincipal, id: Uuid) -> Request {
    Request::new(format!("subscription {{ aiPublicSessionChanged(key: \"{id}\") {{ id changeKind sourceId path aiPublicSession {{ title }} }} }}"))
        .data(actor.clone()).data(AuthSubject::new(actor.subject()))
}
async fn rename(service: &OrmAiSessionService, actor: &AuthPrincipal, id: Uuid, title: &str) {
    service
        .rename_session(
            actor,
            RenameAiSessionInput {
                session_id: id,
                title: title.into(),
                client_mutation_id: Uuid::new_v4(),
                expected_title_revision: None,
            },
        )
        .await
        .unwrap();
}
#[tokio::test]
async fn trusted_private_session_change_notifies_authorized_public_projection_only() {
    let (database, owner, id, policy) = fixture().await;
    let current = Arc::new(Current {
        principal: owner.clone(),
        revoked: AtomicBool::new(false),
    });
    let schema = schema_builder(
        AiPublicEntityAccess::new(database.clone(), policy.clone(), current).database(),
    )
    .finish();
    let mut stream = Box::pin(schema.execute_stream(subscription(&owner, id)));
    registered(&mut stream).await;
    rename(&sessions(database, policy), &owner, id, "Committed title").await;
    let response = timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let value = response.data.into_json().unwrap();
    assert_eq!(
        value["aiPublicSessionChanged"]["aiPublicSession"]["title"],
        "Committed title"
    );
    assert_eq!(value["aiPublicSessionChanged"]["id"], id.to_string());
    assert_eq!(value["aiPublicSessionChanged"]["changeKind"], "PROPAGATED");
    assert!(!schema.sdl().contains("AiSessionRecord"));
    assert!(!schema.sdl().contains("protectedPreview"));
}
#[tokio::test]
async fn sending_message_notifies_public_run_and_session_without_protected_content() {
    let (database, owner, id, policy) = fixture().await;
    let current = Arc::new(Current {
        principal: owner.clone(),
        revoked: AtomicBool::new(false),
    });
    let schema = schema_builder(
        AiPublicEntityAccess::new(database.clone(), policy.clone(), current).database(),
    )
    .finish();
    let mut session = Box::pin(schema.execute_stream(subscription(&owner, id)));
    registered(&mut session).await;
    let run_request=Request::new("subscription { aiPublicRunChanged { id changeKind path aiPublicRun { sessionId state } } }").data(owner.clone()).data(AuthSubject::new(owner.subject()));
    let mut run = Box::pin(schema.execute_stream(run_request));
    registered(&mut run).await;
    let secret = "synthetic-private-message-content";
    let accepted = sessions(database, policy)
        .send_message(
            &owner,
            SendAiMessageInput {
                session_id: id,
                text: secret.into(),
                attachment_ids: vec![],
                client_message_id: Uuid::new_v4(),
            },
        )
        .await
        .unwrap();
    let response = timeout(Duration::from_secs(2), run.next())
        .await
        .unwrap()
        .unwrap();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let value = response.data.into_json().unwrap();
    assert_eq!(
        value["aiPublicRunChanged"]["id"],
        accepted.run_id.to_string()
    );
    assert!(!value.to_string().contains(secret));
    let response = timeout(Duration::from_secs(2), session.next())
        .await
        .unwrap()
        .unwrap();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert!(
        !response
            .data
            .into_json()
            .unwrap()
            .to_string()
            .contains(secret)
    );
}
#[tokio::test]
async fn current_principal_and_ai_policy_revocation_suppress_queued_projection_metadata() {
    for deny in ["principal", "ai-policy"] {
        let (database, owner, id, policy) = fixture().await;
        let current = Arc::new(Current {
            principal: owner.clone(),
            revoked: AtomicBool::new(false),
        });
        let schema = schema_builder(
            AiPublicEntityAccess::new(database.clone(), policy.clone(), current.clone()).database(),
        )
        .finish();
        let mut stream = Box::pin(schema.execute_stream(subscription(&owner, id)));
        registered(&mut stream).await;
        rename(
            &sessions(database, policy.clone()),
            &owner,
            id,
            "Queued private metadata",
        )
        .await;
        if deny == "principal" {
            current.revoked.store(true, Ordering::SeqCst);
        } else {
            policy.0.store(false, Ordering::SeqCst);
        }
        assert!(
            timeout(Duration::from_millis(150), stream.next())
                .await
                .is_err(),
            "released revoked {deny}"
        );
    }
}
#[tokio::test]
async fn queued_projection_never_crosses_owner_or_tenant() {
    for actor in [
        principal("stranger", "tenant-1"),
        principal("owner", "tenant-2"),
    ] {
        let (database, owner, id, policy) = fixture().await;
        let current = Arc::new(Current {
            principal: actor.clone(),
            revoked: AtomicBool::new(false),
        });
        let schema = schema_builder(
            AiPublicEntityAccess::new(database.clone(), policy.clone(), current).database(),
        )
        .finish();
        let mut stream = Box::pin(schema.execute_stream(subscription(&actor, id)));
        registered(&mut stream).await;
        rename(
            &sessions(database, policy),
            &owner,
            id,
            "Owner-only metadata",
        )
        .await;
        assert!(
            timeout(Duration::from_millis(150), stream.next())
                .await
                .is_err()
        );
    }
}

#[derive(Clone)]
struct DenySubscriptionEntity {
    denied: &'static str,
    enabled: Arc<AtomicBool>,
}
impl EntityPolicy for DenySubscriptionEntity {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a graphql_orm::async_graphql::Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(surface != EntityAccessSurface::GraphqlSubscription
                || entity != self.denied
                || !self.enabled.load(Ordering::SeqCst))
        })
    }
}
#[tokio::test]
async fn additional_host_source_and_projection_entity_revocation_are_preserved() {
    for entity in ["AiSessionRecord", "AiPublicSession"] {
        let (mut database, owner, id, policy) = fixture().await;
        let denied = Arc::new(AtomicBool::new(false));
        database.set_entity_policy(DenySubscriptionEntity {
            denied: entity,
            enabled: denied.clone(),
        });
        let current = Arc::new(Current {
            principal: owner.clone(),
            revoked: AtomicBool::new(false),
        });
        let schema = schema_builder(
            AiPublicEntityAccess::new(database.clone(), policy.clone(), current).database(),
        )
        .finish();
        let mut stream = Box::pin(schema.execute_stream(subscription(&owner, id)));
        registered(&mut stream).await;
        rename(
            &sessions(database, policy),
            &owner,
            id,
            "Queued host-gated title",
        )
        .await;
        denied.store(true, Ordering::SeqCst);
        assert!(
            timeout(Duration::from_millis(150), stream.next())
                .await
                .is_err(),
            "host denied {entity}"
        );
    }
}

struct DenyPublicTitle;
impl FieldPolicy for DenyPublicTitle {
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        field: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(field != "title") })
    }
    fn can_write_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
}
#[tokio::test]
async fn scalar_projection_composition_retains_host_field_denial() {
    let (mut database, owner, id, policy) = fixture().await;
    database.set_field_policy(DenyPublicTitle);
    let current = Arc::new(Current {
        principal: owner.clone(),
        revoked: AtomicBool::new(false),
    });
    let access = AiPublicEntityAccess::new(database, policy, current);
    let response = schema_builder(access.database())
        .finish()
        .execute(request(&owner, id))
        .await;
    assert!(!response.errors.is_empty());
    assert!(!format!("{:?}", response.data).contains("Owner session"));
}

#[derive(Clone)]
struct UnboundAllow;
impl EntityPolicy for UnboundAllow {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}
impl RowPolicy for UnboundAllow {
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}
impl FieldPolicy for UnboundAllow {
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_write_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}
struct BindingReadObserver(Arc<std::sync::atomic::AtomicUsize>);
impl ReadQueryObserver for BindingReadObserver {
    fn on_read(&self, _: &str, _: usize) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
#[tokio::test]
async fn count_only_reads_require_named_bindings_before_database_access() {
    let (database, owner, _, policy) = fixture().await;
    let count_request = || {
        Request::new("{ aiPublicSessions(page: {limit: 1}) { pageInfo { totalCount } } }")
            .data(owner.clone())
            .data(AuthSubject::new(owner.subject()))
    };
    let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    for arbitrary_providers in [false, true] {
        let mut unbound = database
            .clone()
            .with_read_query_observer(BindingReadObserver(reads.clone()));
        if arbitrary_providers {
            unbound.set_entity_policy(UnboundAllow);
            unbound.set_row_policy(UnboundAllow);
            unbound.set_field_policy(UnboundAllow);
        }
        let schema = schema_builder(unbound).finish();
        assert!(schema.sdl().contains("aiPublicSessions("));
        let response = schema.execute(count_request()).await;
        assert!(
            !response.errors.is_empty(),
            "missing or unrelated providers must not release counts"
        );
        assert_eq!(reads.load(Ordering::SeqCst), 0);
    }
    let current = Arc::new(Current {
        principal: owner.clone(),
        revoked: AtomicBool::new(false),
    });
    let access = AiPublicEntityAccess::new(database, policy, current);
    let response = schema_builder(access.database())
        .finish()
        .execute(count_request())
        .await;
    assert!(
        response.errors.is_empty(),
        "valid count query with bindings: {:?}",
        response.errors
    );
    assert_eq!(
        response.data.into_json().unwrap()["aiPublicSessions"]["pageInfo"]["totalCount"],
        1
    );
}
