use super::*;
use tokio::sync::Notify;
use tokio::time::{Duration, timeout};

#[derive(Default)]
pub(super) struct Pause {
    entered: Notify,
    resume: Notify,
}
pub(super) async fn pause(slot: &Mutex<Option<Arc<Pause>>>) {
    let paused = slot.lock().unwrap().clone();
    if let Some(paused) = paused {
        paused.entered.notify_one();
        paused.resume.notified().await;
    }
}
async fn entered(paused: &Pause) {
    timeout(Duration::from_secs(5), paused.entered.notified())
        .await
        .expect("policy must reach the controlled await");
}

#[derive(Default)]
pub(super) struct Captured(Mutex<Option<AgqlOwnedOperation<Release>>>);
struct Release {
    record_id: &'static str,
    deleted: bool,
}
struct OwnedPolicy(Arc<Host>);
impl AgqlOwnedOperationAuthorizer<Release> for OwnedPolicy {
    fn authorize_bindings<'a>(
        &'a self,
        accepted: &'a AgqlAcceptedRequest,
        target: AgqlOperationTarget,
        _: &'a Release,
    ) -> BoxFuture<'a, AuthResult<bool>> {
        Box::pin(async move {
            pause(&self.0.binding_pause).await;
            Ok(target.entity_name == "SecureRecord"
                && target.policy_key == Some("records.read")
                && target.kind == EntityAccessKind::Read
                && target.surface == EntityAccessSurface::GraphqlSubscription
                && accepted.principal().subject() == "user-1"
                && self.0.binding_allowed.load(Ordering::SeqCst))
        })
    }
    fn authorize_business<'a>(
        &'a self,
        accepted: &'a AgqlAcceptedRequest,
        _: AgqlOperationTarget,
        release: &'a Release,
    ) -> BoxFuture<'a, AuthResult<bool>> {
        Box::pin(async move {
            self.0.business_calls.fetch_add(1, Ordering::SeqCst);
            pause(&self.0.business_pause).await;
            let owner = self.0.current_owner.lock().unwrap().clone();
            Ok(!release.deleted
                && release.record_id == "record-1"
                && owner.as_deref() == Some(accepted.principal().subject())
                && accepted.subject().has_scope("records.read"))
        })
    }
}
pub(super) fn capture_owned(ctx: &Context<'_>) -> graphql_orm::async_graphql::Result<bool> {
    let enforcement = ctx.data_unchecked::<Arc<AgqlCurrentAuthorityEnforcement>>();
    let operation = AgqlOwnedOperation::from_context(
        ctx,
        enforcement.clone(),
        AgqlOperationTarget {
            entity_name: "SecureRecord",
            policy_key: Some("records.read"),
            kind: EntityAccessKind::Read,
            surface: EntityAccessSurface::GraphqlSubscription,
        },
        Arc::new(OwnedPolicy(ctx.data_unchecked::<Arc<Host>>().clone())),
    )?;
    *ctx.data_unchecked::<Captured>().0.lock().unwrap() = Some(operation);
    Ok(true)
}
async fn issued_fixture() -> (AppSchema, Arc<Host>, AgqlAcceptedRequest) {
    let store = Arc::new(LoginStore::default());
    let auth = agql_auth::AuthService::new(
        agql_auth::AuthConfig::new("synthetic-local-signing-secret-at-least-32-bytes"),
        store.clone(),
        store,
    )
    .unwrap();
    let issued = auth
        .issue_session_for_user_with_scopes(
            "user-1",
            vec!["reader".into()],
            vec!["records.read".into(), "records.write".into()],
            Default::default(),
            Default::default(),
        )
        .await
        .unwrap();
    let accepted = auth
        .authenticate_access_token(&issued.access_token)
        .unwrap();
    let fixture = fixture_for(AuthPrincipal::User(accepted), None, false).await;
    fixture.1.clock.set(OffsetDateTime::now_utc());
    fixture
}
async fn capture(schema: &AppSchema, bundle: &AgqlAcceptedRequest) -> AgqlOwnedOperation<Release> {
    let response = schema.execute(request(bundle, "{ captureOwned }")).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    schema
        .data::<Captured>()
        .unwrap()
        .0
        .lock()
        .unwrap()
        .take()
        .unwrap()
}

#[tokio::test]
async fn revocation_during_binding_await_denies_before_business() {
    let (schema, host, bundle) = issued_fixture().await;
    let paused = Arc::new(Pause::default());
    *host.binding_pause.lock().unwrap() = Some(paused.clone());
    let now = host.clock.now();
    let execution = schema.execute(request(&bundle, READ));
    let mutation = async {
        entered(&paused).await;
        host.revoked.store(true, Ordering::SeqCst);
        paused.resume.notify_one();
    };
    let (response, ()) = tokio::join!(execution, mutation);
    assert!(!response.errors.is_empty());
    assert_eq!(host.business_calls.load(Ordering::SeqCst), 0);
    assert_eq!(host.checks.load(Ordering::SeqCst), 2);
    assert_eq!(host.clock.now(), now);
}

#[tokio::test]
async fn authority_change_during_business_await_denies_release() {
    for change in 0..7 {
        let (schema, host, bundle) = fixture().await;
        let paused = Arc::new(Pause::default());
        *host.business_pause.lock().unwrap() = Some(paused.clone());
        let now = host.clock.now();
        let execution = schema.execute(request(&bundle, READ));
        let mutation = async {
            entered(&paused).await;
            match change {
                0 => host.revoked.store(true, Ordering::SeqCst),
                1..=4 => {
                    let mut current = host.current.lock().unwrap();
                    let AuthPrincipal::User(user) = &mut *current else {
                        unreachable!()
                    };
                    match change {
                        1 => user.roles.clear(),
                        2 => user.token_claims.authorization_roles.clear(),
                        3 => user.token_claims.expires_at = Some(now),
                        _ => user.scopes.clear(),
                    }
                }
                5 => host.unavailable.store(true, Ordering::SeqCst),
                _ => host.resolver_unavailable.store(true, Ordering::SeqCst),
            }
            paused.resume.notify_one();
        };
        let (response, ()) = tokio::join!(execution, mutation);
        assert!(!response.errors.is_empty(), "change {change}");
        assert_eq!(host.business_calls.load(Ordering::SeqCst), 1);
        assert_eq!(host.clock.now(), now);
        assert_eq!(bundle.principal(), &principal());
    }
}

#[tokio::test]
async fn owned_operation_survives_context_and_denies_queued_revoke_transfer_and_deleted_preimage() {
    for change in 0..5 {
        let (schema, host, bundle) = issued_fixture().await;
        let original = bundle.clone();
        let operation = capture(&schema, &bundle).await;
        drop(schema);
        // This future is owned and Send; no Context or unsafe lifetime survives.
        fn assert_owned<T: Send + 'static>(_: &T) {}
        assert_owned(&operation);
        let release = Release {
            record_id: "record-1",
            deleted: false,
        };
        operation.check_release(&release).await.unwrap();
        match change {
            0 => host.revoked.store(true, Ordering::SeqCst),
            1 => *host.current_owner.lock().unwrap() = Some("other-owner".into()),
            2 => *host.current_owner.lock().unwrap() = None,
            3 => host.unavailable.store(true, Ordering::SeqCst),
            _ => host.resolver_unavailable.store(true, Ordering::SeqCst),
        }
        assert!(operation.check_release(&release).await.is_err());
        assert_eq!(operation.accepted(), &original);
    }
}

#[tokio::test]
async fn owned_operation_revalidates_after_owner_await_and_preserves_denial() {
    let (schema, host, bundle) = issued_fixture().await;
    let operation = capture(&schema, &bundle).await;
    let paused = Arc::new(Pause::default());
    *host.business_pause.lock().unwrap() = Some(paused.clone());
    let release = Release {
        record_id: "record-1",
        deleted: false,
    };
    let evaluation = operation.check_release(&release);
    let mutation = async {
        entered(&paused).await;
        let mut current = host.current.lock().unwrap();
        let AuthPrincipal::User(user) = &mut *current else {
            unreachable!()
        };
        user.scopes.clear();
        paused.resume.notify_one();
    };
    let (result, ()) = tokio::join!(evaluation, mutation);
    assert!(result.is_err());
    assert_eq!(host.business_calls.load(Ordering::SeqCst), 1);
    assert_eq!(operation.accepted(), &bundle);
    *host.business_pause.lock().unwrap() = None;
    *host.current.lock().unwrap() = bundle.principal().clone();
    let deleted = Release {
        record_id: "record-1",
        deleted: true,
    };
    assert!(operation.check_release(&deleted).await.is_err());
}

#[tokio::test]
async fn owned_owner_transfer_during_policy_await_denies_before_release() {
    let (schema, host, bundle) = issued_fixture().await;
    let operation = capture(&schema, &bundle).await;
    let paused = Arc::new(Pause::default());
    *host.business_pause.lock().unwrap() = Some(paused.clone());
    let release = Release {
        record_id: "record-1",
        deleted: false,
    };
    let evaluation = operation.check_release(&release);
    let transfer = async {
        entered(&paused).await;
        *host.current_owner.lock().unwrap() = Some("other-owner".into());
        paused.resume.notify_one();
    };
    let (result, ()) = tokio::join!(evaluation, transfer);
    assert!(result.is_err());
    assert_eq!(host.business_calls.load(Ordering::SeqCst), 1);
    assert_eq!(operation.accepted(), &bundle);
}
