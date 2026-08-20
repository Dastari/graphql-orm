#![cfg(feature = "sqlite")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use agql_auth::{
    AccessTokenMetadata, AuthPrincipal, AuthUser, CurrentPrincipalResolver, PrincipalReference,
    ResolvedPrincipal, SessionContext,
};
use async_trait::async_trait;
use futures::StreamExt;
use graphql_orm::graphql::orm::{ApplyOptions, OrmSchemaModule};
use graphql_orm::prelude::{Database, SqliteBackend};
use graphql_orm_ai::*;
use time::OffsetDateTime;
use uuid::Uuid;

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

struct ToggleResolver {
    principal: AuthPrincipal,
    active: Arc<AtomicBool>,
}

#[async_trait]
impl CurrentPrincipalResolver for ToggleResolver {
    async fn resolve(
        &self,
        reference: &PrincipalReference,
    ) -> agql_auth::AuthResult<ResolvedPrincipal> {
        if !self.active.load(Ordering::SeqCst) {
            return Err(agql_auth::AuthError::Forbidden);
        }
        ResolvedPrincipal::new(
            reference.clone(),
            self.principal.clone(),
            OffsetDateTime::now_utc(),
        )
    }
}

fn principal() -> AuthPrincipal {
    AuthPrincipal::User(AuthUser {
        user_id: "owner".to_owned(),
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

async fn apply_schema(database: &Database<SqliteBackend>, name: &str) {
    let module = AiSchemaModule;
    let plan = database
        .schema()
        .plan_migration_to_entities(name, "AI subscription service test", module.entities())
        .await
        .expect("schema plans");
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect("schema applies");
}

async fn services(
    reauthorization_interval: Duration,
) -> (
    Arc<OrmAiSessionService>,
    OrmAiSubscriptionService,
    AuthPrincipal,
    Arc<AtomicBool>,
) {
    services_with_replay_page_size(reauthorization_interval, 1).await
}

async fn services_with_replay_page_size(
    reauthorization_interval: Duration,
    replay_page_size: i64,
) -> (
    Arc<OrmAiSessionService>,
    OrmAiSubscriptionService,
    AuthPrincipal,
    Arc<AtomicBool>,
) {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .expect("in-memory SQLite opens");
    let module = AiSchemaModule;
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "ai-subscription-test-v1",
            "AI subscription service test",
            module.entities(),
        )
        .await
        .expect("schema plans");
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect("schema applies");
    let sessions = Arc::new(OrmAiSessionService::new(
        database,
        Arc::new(AllowAll),
        Arc::new(ProtectionPolicy),
        Arc::new(DatabaseManagedContentProtector),
    ));
    let principal = principal();
    let active = Arc::new(AtomicBool::new(true));
    let subscriptions = OrmAiSubscriptionService::new(
        sessions.clone(),
        Arc::new(ToggleResolver {
            principal: principal.clone(),
            active: active.clone(),
        }),
    )
    .with_reauthorization_interval(reauthorization_interval)
    .with_replay_page_size(replay_page_size);
    (sessions, subscriptions, principal, active)
}

async fn append_title_events(
    sessions: &OrmAiSessionService,
    principal: &AuthPrincipal,
    session_id: Uuid,
    count: i64,
) {
    for revision in 0..count {
        sessions
            .rename_session(
                principal,
                RenameAiSessionInput {
                    session_id,
                    title: format!("Subscription title {}", revision + 1),
                    client_mutation_id: Uuid::new_v4(),
                    expected_title_revision: Some(revision),
                },
            )
            .await
            .expect("title event should commit");
    }
}

async fn create_session(
    sessions: &OrmAiSessionService,
    principal: &AuthPrincipal,
) -> AiSessionView {
    sessions
        .create_session(
            principal,
            CreateAiSessionInput {
                scope: AiScopeInput {
                    kind: "collection".to_owned(),
                    id: "54".to_owned(),
                    tenant_id: Some("tenant-1".to_owned()),
                },
                title: Some("Subscription".to_owned()),
            },
        )
        .await
        .expect("session is created")
}

async fn send(
    sessions: &OrmAiSessionService,
    principal: &AuthPrincipal,
    session_id: Uuid,
    text: &str,
) {
    sessions
        .send_message(
            principal,
            SendAiMessageInput {
                session_id,
                text: text.to_owned(),
                attachment_ids: vec![],
                client_message_id: Uuid::new_v4(),
            },
        )
        .await
        .expect("message commits");
}

#[tokio::test]
async fn receiver_attaches_before_replay_and_delivers_durable_wakeups() {
    let (sessions, subscriptions, principal, _active) = services(Duration::from_secs(60)).await;
    let session = create_session(&sessions, &principal).await;
    let mut stream = subscriptions
        .session_events(principal.clone(), AiSessionId(session.id), 0)
        .await
        .expect("subscription opens");
    send(&sessions, &principal, session.id, "first").await;
    let item = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("subscription wakes")
        .expect("stream item")
        .expect("event delivery");
    assert!(!item.reset_required);
    assert_eq!(item.event.expect("durable event").sequence, 1);
}

#[tokio::test]
async fn replay_is_paged_to_a_watermark_and_revocation_closes_stream() {
    let (sessions, subscriptions, principal, active) = services(Duration::from_millis(20)).await;
    let session = create_session(&sessions, &principal).await;
    send(&sessions, &principal, session.id, "first").await;
    send(&sessions, &principal, session.id, "second").await;
    let mut stream = subscriptions
        .session_events(principal.clone(), AiSessionId(session.id), 0)
        .await
        .expect("subscription opens");
    let first = stream
        .next()
        .await
        .expect("first item")
        .expect("first event");
    let second = stream
        .next()
        .await
        .expect("second item")
        .expect("second event");
    assert_eq!(first.event.expect("event").sequence, 1);
    assert_eq!(second.event.expect("event").sequence, 2);

    active.store(false, Ordering::SeqCst);
    // An authoritative denial closes immediately: the typed envelope names the
    // reason, and the error still follows so existing clients keep working.
    let close = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("reauthorization runs")
        .expect("terminal close item")
        .expect("close envelope is not an error");
    assert_eq!(
        close.closed,
        Some(AiSessionStreamClose::AuthorizationRevoked)
    );
    assert!(close.event.is_none());
    assert!(!close.reset_required);
    let revoked = stream
        .next()
        .await
        .expect("terminal error item after the close envelope");
    assert!(matches!(revoked, Err(AiError::ReauthorizationFailed)));
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn maximum_sized_replay_pages_are_drained_before_live_delivery() {
    let (sessions, subscriptions, principal, _active) =
        services_with_replay_page_size(Duration::from_secs(60), 100).await;
    let session = create_session(&sessions, &principal).await;
    append_title_events(&sessions, &principal, session.id, 101).await;
    let mut stream = subscriptions
        .session_events(principal.clone(), AiSessionId(session.id), 0)
        .await
        .expect("subscription should open");

    let mut replayed = Vec::new();
    for _ in 0..101 {
        let envelope = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("replay should not stall")
            .expect("replay item should exist")
            .expect("replay item should succeed");
        assert!(!envelope.reset_required);
        replayed.push(envelope.event.expect("durable event").sequence);
    }
    assert_eq!(replayed, (1..=101).collect::<Vec<_>>());

    sessions
        .rename_session(
            &principal,
            RenameAiSessionInput {
                session_id: session.id,
                title: "Live title".to_owned(),
                client_mutation_id: Uuid::new_v4(),
                expected_title_revision: Some(101),
            },
        )
        .await
        .expect("live event should commit");
    let live = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("subscription should wake")
        .expect("live item should exist")
        .expect("live item should succeed");
    let live = live.event.expect("live durable event");
    assert_eq!(live.sequence, 102);
    assert_eq!(live.event_type, "session_title_changed");
}

/// Resolver that can be made temporarily unavailable, distinguishing a
/// dependency restart from an authoritative denial.
struct BlipResolver {
    principal: AuthPrincipal,
    unavailable: Arc<AtomicBool>,
    denied: Arc<AtomicBool>,
    attempts: Arc<AtomicUsize>,
}

#[async_trait]
impl CurrentPrincipalResolver for BlipResolver {
    async fn resolve(
        &self,
        reference: &PrincipalReference,
    ) -> agql_auth::AuthResult<ResolvedPrincipal> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        if self.denied.load(Ordering::SeqCst) {
            return Err(agql_auth::AuthError::Forbidden);
        }
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(agql_auth::AuthError::AuthServiceUnavailable);
        }
        ResolvedPrincipal::new(
            reference.clone(),
            self.principal.clone(),
            OffsetDateTime::now_utc(),
        )
    }
}

/// Work item 2: a brief authorization-service restart must not drop the
/// stream, but an authoritative denial must still close it immediately.
#[tokio::test]
async fn reauthorization_blip_is_survived_and_denial_still_fails_fast() {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .expect("in-memory SQLite opens");
    apply_schema(&database, "ai-subscription-blip-v1").await;
    let sessions = Arc::new(OrmAiSessionService::new(
        database,
        Arc::new(AllowAll),
        Arc::new(ProtectionPolicy),
        Arc::new(DatabaseManagedContentProtector),
    ));
    let principal = principal();
    let unavailable = Arc::new(AtomicBool::new(false));
    let denied = Arc::new(AtomicBool::new(false));
    let attempts = Arc::new(AtomicUsize::new(0));
    let subscriptions = OrmAiSubscriptionService::new(
        sessions.clone(),
        Arc::new(BlipResolver {
            principal: principal.clone(),
            unavailable: unavailable.clone(),
            denied: denied.clone(),
            attempts: attempts.clone(),
        }),
    )
    // Fast enough to observe several reauthorization attempts quickly, but not
    // so fast that the durable head check starves a concurrent writer of the
    // single in-memory SQLite write lock when the whole suite runs in parallel.
    .with_reauthorization_interval(Duration::from_millis(50))
    .with_reauthorization_grace(Duration::from_secs(30))
    .with_replay_check_interval(Duration::from_millis(500))
    .with_replay_page_size(50);

    let session = create_session(&sessions, &principal).await;
    send(&sessions, &principal, session.id, "first").await;
    let mut stream = subscriptions
        .session_events(principal.clone(), AiSessionId(session.id), 0)
        .await
        .expect("subscription opens");
    let first = stream
        .next()
        .await
        .expect("first item")
        .expect("first event");
    assert_eq!(first.event.expect("event").sequence, 1);

    // The dependency goes away for long enough to cover several ticks. Drive
    // the stream while waiting so its select loop keeps running, and require
    // that reauthorization was actually retried rather than abandoned.
    unavailable.store(true, Ordering::SeqCst);
    let before = attempts.load(Ordering::SeqCst);
    let outage = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if attempts.load(Ordering::SeqCst) >= before + 3 {
                return Ok::<(), AiError>(());
            }
            tokio::select! {
                item = stream.next() => {
                    let item = item.expect("stream must stay open during the outage")?;
                    assert!(
                        item.closed.is_none(),
                        "an unavailable dependency must not close the stream inside the grace window"
                    );
                }
                () = tokio::time::sleep(Duration::from_millis(20)) => {}
            }
        }
    })
    .await
    .expect("reauthorization must be retried during the outage");
    outage.expect("the stream must not error inside the grace window");

    // Still inside the grace window, durable delivery continues.
    send(&sessions, &principal, session.id, "during outage").await;
    let during = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("delivery continues during the outage")
        .expect("item")
        .expect("event delivered while reauthorization is unavailable");
    assert!(during.closed.is_none());
    assert!(during.event.is_some());

    // Recovery, then an authoritative denial closes immediately.
    unavailable.store(false, Ordering::SeqCst);
    denied.store(true, Ordering::SeqCst);
    let close = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let item = stream.next().await.expect("stream item")?;
            if item.closed.is_some() {
                return Ok::<_, AiError>(item);
            }
        }
    })
    .await
    .expect("denial closes the stream")
    .expect("close envelope is not an error");
    assert_eq!(
        close.closed,
        Some(AiSessionStreamClose::AuthorizationRevoked)
    );
}

/// Work item 2: single-replica delivery must not depend solely on the
/// in-process wakeup channel.
///
/// Two `Database` handles over one SQLite file have independent in-process
/// wakeup channels, so a commit through the writer handle never produces a
/// wakeup on the subscriber handle. That is exactly the shape of a dropped or
/// missed hint, and the bounded durable head check is the only path that can
/// deliver it.
#[tokio::test]
async fn durable_replay_fallback_delivers_without_an_in_process_wakeup() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("subscription-fallback.sqlite");
    let url = format!("sqlite://{}?mode=rwc", path.display());

    let subscriber_database = Database::<SqliteBackend>::connect_sqlite(&url)
        .await
        .expect("subscriber handle opens");
    apply_schema(&subscriber_database, "ai-subscription-fallback-v1").await;
    let writer_database = Database::<SqliteBackend>::connect_sqlite(&url)
        .await
        .expect("writer handle opens");

    let subscriber_sessions = Arc::new(OrmAiSessionService::new(
        subscriber_database,
        Arc::new(AllowAll),
        Arc::new(ProtectionPolicy),
        Arc::new(DatabaseManagedContentProtector),
    ));
    let writer_sessions = OrmAiSessionService::new(
        writer_database,
        Arc::new(AllowAll),
        Arc::new(ProtectionPolicy),
        Arc::new(DatabaseManagedContentProtector),
    );

    let principal = principal();
    let subscriptions = OrmAiSubscriptionService::new(
        subscriber_sessions.clone(),
        Arc::new(ToggleResolver {
            principal: principal.clone(),
            active: Arc::new(AtomicBool::new(true)),
        }),
    )
    .with_reauthorization_interval(Duration::from_secs(600))
    .with_replay_check_interval(Duration::from_millis(20))
    .with_replay_page_size(50);

    let session = create_session(&subscriber_sessions, &principal).await;
    let mut stream = subscriptions
        .session_events(principal.clone(), AiSessionId(session.id), 0)
        .await
        .expect("subscription opens");

    // Committed through the other handle: no wakeup reaches this subscriber.
    send(&writer_sessions, &principal, session.id, "no wakeup").await;

    let delivered = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("the durable fallback must deliver without an in-process wakeup")
        .expect("item")
        .expect("event");
    assert!(delivered.closed.is_none());
    assert_eq!(delivered.event.expect("event").sequence, 1);
}
