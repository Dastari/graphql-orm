// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(feature = "sqlite")]
use graphql_orm::async_graphql::{Request, Response};
use graphql_orm::futures::{Stream, StreamExt, future::poll_fn};
use graphql_orm::prelude::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::task::Poll;
use tokio::time::{Duration, timeout};

#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[graphql_entity(
    table = "subscription_secure_records",
    plural = "SecureRecords",
    default_sort = "id ASC"
)]
struct SecureRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    owner: String,
    #[graphql_orm(read_policy = "label.read")]
    label: String,
}
#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[graphql_entity(
    table = "subscription_secure_audits",
    plural = "SecureAudits",
    append_only = true
)]
struct SecureAudit {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    owner: String,
    #[graphql_orm(read_policy = "label.read")]
    label: String,
}
schema_roots! { query_custom_ops: [], entities: [SecureRecord, SecureAudit], }

#[derive(Clone)]
struct Authority {
    entity_allowed: Arc<AtomicBool>,
    entity_checks: Arc<AtomicUsize>,
    row_checks: Arc<AtomicUsize>,
    sql_only: Arc<AtomicBool>,
    row_error: Arc<AtomicBool>,
    key_allowed: Arc<AtomicBool>,
    label_allowed: Arc<AtomicBool>,
}
impl EntityPolicy for Authority {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if surface == EntityAccessSurface::GraphqlSubscription {
                self.entity_checks.fetch_add(1, Ordering::SeqCst);
                Ok(self.entity_allowed.load(Ordering::SeqCst))
            } else {
                Ok(true)
            }
        })
    }
}
impl FieldPolicy for Authority {
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        field: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(match field {
                "id" => self.key_allowed.load(Ordering::SeqCst),
                "label" => self.label_allowed.load(Ordering::SeqCst),
                _ => true,
            })
        })
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
impl RowPolicy for Authority {
    fn read_visibility<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            if self.sql_only.load(Ordering::SeqCst) && entity == "SecureRecord" {
                let filter = SecureRecordWhereInput {
                    owner: Some(StringFilter {
                        eq: Some(ctx.and_then(|c| c.auth_subject_opt()).unwrap().id.clone()),
                        ..Default::default()
                    }),
                    ..Default::default()
                };
                Ok(ReadVisibility::Complete(ReadPredicate::from_filter::<
                    SqliteBackend,
                    _,
                >(&filter)?))
            } else {
                Ok(ReadVisibility::CallbackOnly)
            }
        })
    }

    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            self.row_checks.fetch_add(1, Ordering::SeqCst);
            if self.row_error.load(Ordering::SeqCst) {
                return Err(async_graphql::Error::new(
                    "synthetic-private-policy-diagnostic",
                ));
            }
            // Complete SQL visibility must not borrow authority from this permissive fallback.
            if self.sql_only.load(Ordering::SeqCst) {
                return Ok(true);
            }

            let owner = row
                .downcast_ref::<SecureRecord>()
                .map(|r| r.owner.as_str())
                .or_else(|| row.downcast_ref::<SecureAudit>().map(|r| r.owner.as_str()));
            Ok(ctx
                .and_then(|ctx| ctx.auth_subject_opt())
                .is_some_and(|subject| Some(subject.id.as_str()) == owner))
        })
    }
    fn can_read_deleted_preimage<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a Database,
        entity: &'static str,
        policy: Option<&'static str>,
        surface: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        self.can_read_row(ctx, db, entity, policy, surface, row)
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
async fn setup_db() -> (AppSchema, Authority, Database) {
    let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    for table in ["subscription_secure_records", "subscription_secure_audits"] {
        sqlx::query(&format!(
            "CREATE TABLE {table} (id TEXT PRIMARY KEY, owner TEXT NOT NULL, label TEXT NOT NULL)"
        ))
        .execute(db.pool())
        .await
        .unwrap();
    }
    let authority = Authority {
        entity_allowed: Arc::new(AtomicBool::new(true)),
        entity_checks: Arc::new(AtomicUsize::new(0)),
        row_checks: Arc::new(AtomicUsize::new(0)),
        sql_only: Arc::new(AtomicBool::new(false)),
        row_error: Arc::new(AtomicBool::new(false)),
        key_allowed: Arc::new(AtomicBool::new(true)),
        label_allowed: Arc::new(AtomicBool::new(true)),
    };
    db.set_entity_policy(authority.clone());
    db.set_row_policy(authority.clone());
    db.set_field_policy(authority.clone());
    (schema_builder(db.clone()).finish(), authority, db)
}
async fn setup() -> (AppSchema, Authority) {
    let (schema, authority, _) = setup_db().await;
    (schema, authority)
}
async fn registered<S: Stream<Item = Response> + Unpin>(stream: &mut S) {
    poll_fn(|cx| match stream.poll_next_unpin(cx) {
        Poll::Pending => Poll::Ready(()),
        other => panic!("subscription did not wait for its first event: {other:?}"),
    })
    .await;
}
async fn publish(schema: &AppSchema, entity: &str, id: &str, owner: &str) {
    let response = schema.execute(Request::new(format!(
        "mutation {{ create{entity}(input: {{id: \"{id}\", owner: \"{owner}\", label: \"synthetic-private-label\"}}) {{ success error }} }}"
    )).data(AuthSubject::new(owner))).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert!(
        serde_json::to_string(&response.data)
            .unwrap()
            .contains("\"success\":true")
    );
}

#[tokio::test]
async fn ordinary_subscription_cannot_release_bobs_identifier_or_payload_to_alice() {
    let (schema, authority) = setup().await;
    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new("subscription { secureRecordChanged { id sourceEntity sourceId path } }")
                .data(AuthSubject::new("alice")),
        ),
    );
    registered(&mut stream).await;
    publish(&schema, "SecureRecord", "private-bob", "bob").await;
    publish(&schema, "SecureRecord", "allowed-alice", "alice").await;
    let response = timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["secureRecordChanged"]["id"],
        "allowed-alice"
    );
    assert!(authority.row_checks.load(Ordering::SeqCst) >= 2);
}

#[tokio::test]
async fn append_only_subscription_rechecks_row_authority_before_metadata_release() {
    let (schema, authority) = setup().await;
    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new("subscription { secureAuditChanged { id sourceEntity sourceId path } }")
                .data(AuthSubject::new("alice")),
        ),
    );
    registered(&mut stream).await;
    publish(&schema, "SecureAudit", "private-bob", "bob").await;
    publish(&schema, "SecureAudit", "allowed-alice", "alice").await;
    let response = timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["secureAuditChanged"]["id"],
        "allowed-alice"
    );
    assert!(authority.row_checks.load(Ordering::SeqCst) >= 2);
}

#[tokio::test]
async fn policy_revocation_after_subscription_start_prevents_event_release() {
    let (schema, authority) = setup().await;
    let _clock = ManualTime::pause();
    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new("subscription { secureRecordChanged { id sourceEntity sourceId path } }")
                .data(AuthSubject::new("alice")),
        ),
    );
    registered(&mut stream).await;
    authority.entity_allowed.store(false, Ordering::SeqCst);
    publish(&schema, "SecureRecord", "revoked-alice", "alice").await;
    registered(&mut stream).await;
    tokio::time::advance(Duration::from_secs(34)).await;
    forbidden_end(&mut stream).await;
}

#[tokio::test]
async fn live_payload_is_current_authorized_row_after_ownership_transfer() {
    let (schema, _, db) = setup_db().await;
    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new(
                "subscription { secureRecordChanged { id secureRecord { owner label } } }",
            )
            .data(AuthSubject::new("alice")),
        ),
    );
    registered(&mut stream).await;
    publish(&schema, "SecureRecord", "transferred", "bob").await;
    sqlx::query("UPDATE subscription_secure_records SET owner = 'alice', label = 'current-public-label' WHERE id = 'transferred'")
        .execute(db.pool()).await.unwrap();
    let response = timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(
        data["secureRecordChanged"]["secureRecord"]["owner"],
        "alice"
    );
    assert_eq!(
        data["secureRecordChanged"]["secureRecord"]["label"],
        "current-public-label"
    );
    assert!(!data.to_string().contains("synthetic-private-label"));
}

#[tokio::test]
async fn queued_snapshot_cannot_survive_transfer_away_from_subscriber() {
    let (schema, _, db) = setup_db().await;
    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new("subscription { secureRecordChanged { id secureRecord { label } } }")
                .data(AuthSubject::new("alice")),
        ),
    );
    registered(&mut stream).await;
    publish(&schema, "SecureRecord", "transferred-away", "alice").await;
    sqlx::query(
        "UPDATE subscription_secure_records SET owner = 'bob' WHERE id = 'transferred-away'",
    )
    .execute(db.pool())
    .await
    .unwrap();
    publish(&schema, "SecureRecord", "still-alice", "alice").await;
    let response = timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["secureRecordChanged"]["id"],
        "still-alice"
    );
}

#[tokio::test]
async fn sql_only_visibility_cannot_authorize_missing_deleted_preimage() {
    let (schema, authority, _) = setup_db().await;
    authority.sql_only.store(true, Ordering::SeqCst);
    publish(&schema, "SecureRecord", "deleted-alice", "alice").await;
    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new("subscription { secureRecordChanged { id secureRecord { label } } }")
                .data(AuthSubject::new("alice")),
        ),
    );
    registered(&mut stream).await;
    let response = schema
        .execute(
            Request::new(
                "mutation { deleteSecureRecord(id: \"deleted-alice\") { success error } }",
            )
            .data(AuthSubject::new("alice")),
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert!(
        response.data.into_json().unwrap()["deleteSecureRecord"]["success"]
            .as_bool()
            .unwrap()
    );
    assert!(
        timeout(Duration::from_millis(100), stream.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn selected_scalar_and_outer_identifier_are_checked_before_metadata() {
    for key in [false, true] {
        let (schema, authority) = setup().await;
        let mut stream = Box::pin(schema.execute_stream(Request::new(
            "subscription { secureRecordChanged { id secureRecord { ...Hidden } } } fragment Hidden on SecureRecord { aliased: label }"
        ).data(AuthSubject::new("alice"))));
        registered(&mut stream).await;
        if key {
            authority.key_allowed.store(false, Ordering::SeqCst);
        } else {
            authority.label_allowed.store(false, Ordering::SeqCst);
        }
        publish(&schema, "SecureRecord", "protected-alice", "alice").await;
        assert!(
            timeout(Duration::from_millis(100), stream.next())
                .await
                .is_err(),
            "protected metadata escaped: key={key}"
        );
    }
}

#[tokio::test]
async fn direct_delete_uses_trusted_preimage_and_current_callback_authority() {
    let (schema, _) = setup().await;
    publish(&schema, "SecureRecord", "deleted-alice", "alice").await;
    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new(
                "subscription { secureRecordChanged { id action secureRecord { owner } } }",
            )
            .data(AuthSubject::new("alice")),
        ),
    );
    registered(&mut stream).await;
    let deleted = schema
        .execute(
            Request::new("mutation { deleteSecureRecord(id: \"deleted-alice\") { success } }")
                .data(AuthSubject::new("alice")),
        )
        .await;
    assert!(deleted.errors.is_empty(), "{:?}", deleted.errors);
    let response = timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data["secureRecordChanged"]["action"], "DELETED");
    assert_eq!(
        data["secureRecordChanged"]["secureRecord"]["owner"],
        "alice"
    );
}
#[tokio::test]
async fn action_filter_is_enforced_and_dummy_filter_is_rejected() {
    let (schema, _) = setup().await;
    let mut rejected = Box::pin(
        schema.execute_stream(
            Request::new("subscription { secureRecordChanged(filter: {dummy: true}) { id } }")
                .data(AuthSubject::new("alice")),
        ),
    );
    let response = rejected.next().await.unwrap();
    assert_eq!(response.errors.len(), 1);
    assert_eq!(
        response.errors[0]
            .extensions
            .as_ref()
            .unwrap()
            .get("code")
            .unwrap(),
        &async_graphql::Value::from("INVALID_INPUT")
    );
    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new(
                "subscription { secureRecordChanged(filter: {actions: [UPDATED]}) { id action } }",
            )
            .data(AuthSubject::new("alice")),
        ),
    );
    registered(&mut stream).await;
    publish(&schema, "SecureRecord", "created-alice", "alice").await;
    let updated = schema.execute(Request::new("mutation { updateSecureRecord(id: \"created-alice\", input: {label: \"new label\"}) { success } }").data(AuthSubject::new("alice"))).await;
    assert!(updated.errors.is_empty(), "{:?}", updated.errors);
    let response = timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["secureRecordChanged"]["action"],
        "UPDATED"
    );
}

#[tokio::test]
async fn policy_errors_drop_every_payload_and_diagnostic() {
    let (schema, authority) = setup().await;
    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new(
                "subscription { secureRecordChanged { id sourceId path secureRecord { label } } }",
            )
            .data(AuthSubject::new("alice")),
        ),
    );
    registered(&mut stream).await;
    authority.row_error.store(true, Ordering::SeqCst);
    publish(&schema, "SecureRecord", "policy-error", "alice").await;
    assert!(
        timeout(Duration::from_millis(100), stream.next())
            .await
            .is_err()
    );
}
#[tokio::test]
async fn skipped_protected_scalar_does_not_block_authorized_identifier() {
    let (schema, authority) = setup().await;
    authority.label_allowed.store(false, Ordering::SeqCst);
    let mut stream = Box::pin(schema.execute_stream(Request::new(
        "subscription { secureRecordChanged { id secureRecord { label @skip(if: true) } } }"
    ).data(AuthSubject::new("alice"))));
    registered(&mut stream).await;
    publish(&schema, "SecureRecord", "selected-id", "alice").await;
    let response = timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["secureRecordChanged"]["id"],
        "selected-id"
    );
}

struct PermissiveOrdinaryReadPolicy;
impl RowPolicy for PermissiveOrdinaryReadPolicy {
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
#[tokio::test]
async fn deleted_preimage_requires_explicit_opt_in_even_when_ordinary_reads_allow() {
    for permissive_callback in [false, true] {
        let (_, _, seed) = setup_db().await;
        let mut db = Database::new(seed.pool().clone());
        if permissive_callback {
            db.set_row_policy(PermissiveOrdinaryReadPolicy);
        }
        let schema = schema_builder(db).finish();
        publish(&schema, "SecureRecord", "deleted-without-opt-in", "alice").await;
        let mut stream = Box::pin(
            schema.execute_stream(
                Request::new("subscription { secureRecordChanged { id action } }")
                    .data(AuthSubject::new("alice")),
            ),
        );
        registered(&mut stream).await;
        let deleted = schema
            .execute(
                Request::new(
                    "mutation { deleteSecureRecord(id: \"deleted-without-opt-in\") { success } }",
                )
                .data(AuthSubject::new("alice")),
            )
            .await;
        assert!(deleted.errors.is_empty(), "{:?}", deleted.errors);
        assert_eq!(
            deleted.data.into_json().unwrap()["deleteSecureRecord"]["success"],
            true
        );
        publish(&schema, "SecureRecord", "live-proof", "alice").await;
        let response = timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json().unwrap();
        assert_eq!(
            data["secureRecordChanged"]["id"], "live-proof",
            "permissive_callback={permissive_callback}"
        );
        assert_eq!(data["secureRecordChanged"]["action"], "CREATED");
    }
}

#[tokio::test]
async fn exact_key_and_action_filters_narrow_normal_and_append_only_events() {
    for (entity, field) in [
        ("SecureRecord", "secureRecordChanged"),
        ("SecureAudit", "secureAuditChanged"),
    ] {
        let (schema, _) = setup().await;
        let mut stream = Box::pin(schema.execute_stream(
            Request::new(format!(
                "subscription {{ {field}(key: \"wanted\", filter: {{actions: [CREATED]}}) {{ id action }} }}"
            ))
            .data(AuthSubject::new("alice")),
        ));
        registered(&mut stream).await;
        publish(&schema, entity, "other", "alice").await;
        publish(&schema, entity, "wanted", "alice").await;
        let response = timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let value = response.data.into_json().unwrap();
        assert_eq!(value[field]["id"], "wanted");
        assert_eq!(value[field]["action"], "CREATED");
        assert!(
            timeout(Duration::from_millis(100), stream.next())
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn exact_key_never_grants_owner_or_revoked_entity_authority() {
    for (entity, field) in [
        ("SecureRecord", "secureRecordChanged"),
        ("SecureAudit", "secureAuditChanged"),
    ] {
        for revoke in [false, true] {
            let (schema, authority) = setup().await;
            let _clock = ManualTime::pause();
            let mut stream = Box::pin(
                schema.execute_stream(
                    Request::new(format!(
                        "subscription {{ {field}(key: \"wanted\") {{ id }} }}"
                    ))
                    .data(AuthSubject::new("alice")),
                ),
            );
            registered(&mut stream).await;
            if revoke {
                authority.entity_allowed.store(false, Ordering::SeqCst);
            }
            publish(
                &schema,
                entity,
                "wanted",
                if revoke { "alice" } else { "bob" },
            )
            .await;
            registered(&mut stream).await;
            if revoke {
                tokio::time::advance(Duration::from_secs(34)).await;
                forbidden_end(&mut stream).await;
            }
        }
    }
}

#[tokio::test]
async fn exact_key_requires_current_key_field_authority_for_action_only_payloads() {
    for (entity, field) in [
        ("SecureRecord", "secureRecordChanged"),
        ("SecureAudit", "secureAuditChanged"),
    ] {
        let (schema, authority) = setup().await;
        let mut stream = Box::pin(
            schema.execute_stream(
                Request::new(format!(
                    "subscription {{ {field}(key: \"wanted\") {{ action }} }}"
                ))
                .data(AuthSubject::new("alice")),
            ),
        );
        registered(&mut stream).await;
        authority.key_allowed.store(false, Ordering::SeqCst);
        publish(&schema, entity, "wanted", "alice").await;
        assert!(
            timeout(Duration::from_millis(100), stream.next())
                .await
                .is_err(),
            "key argument must not reveal a matching row through an action-only payload"
        );
    }
}

#[tokio::test]
async fn subscription_key_argument_is_in_both_sdl_and_operation_catalogue() {
    let (schema, _) = setup().await;
    assert!(schema.sdl().contains("key: String"));
    use graphql_orm::graphql::orm::GraphqlOperationKind;
    let catalog = graphql_orm_operation_catalog();
    let subscriptions = catalog
        .operations()
        .iter()
        .filter(|operation| operation.kind() == GraphqlOperationKind::Subscription)
        .collect::<Vec<_>>();
    assert_eq!(subscriptions.len(), 2);
    for operation in subscriptions {
        assert!(
            operation
                .arguments()
                .iter()
                .any(|argument| argument.graphql_name() == "key")
        );
    }
}

#[tokio::test]
async fn subscriber_revocation_is_one_sanitized_error_then_end_without_writes() {
    for field in ["secureRecordChanged", "secureAuditChanged"] {
        for key in ["", "(key: \"wanted\")"] {
            let (schema, authority) = setup().await;
            let _clock = ManualTime::pause();
            let mut stream = Box::pin(
                schema.execute_stream(
                    Request::new(format!("subscription {{ {field}{key} {{ id }} }}"))
                        .data(AuthSubject::new("alice")),
                ),
            );
            registered(&mut stream).await;
            authority.entity_allowed.store(false, Ordering::SeqCst);
            tokio::time::advance(Duration::from_secs(29)).await;
            registered(&mut stream).await;
            tokio::time::advance(Duration::from_secs(5)).await;
            forbidden_end(&mut stream).await;
            assert_eq!(authority.row_checks.load(Ordering::SeqCst), 0);
        }
    }
}

#[derive(Debug)]
struct CurrentAssurance(Arc<AtomicBool>);
impl AssuranceRequirementEvaluator for CurrentAssurance {
    fn enforce(
        &self,
        _: &async_graphql::Context<'_>,
        _: AssuranceActorClass,
        _: &str,
    ) -> async_graphql::Result<()> {
        if self.0.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(async_graphql::Error::new(
                "private session and policy diagnostic",
            ))
        }
    }
}

#[tokio::test]
async fn current_assurance_loss_is_timer_driven_independent_of_writes() {
    // Every cohort has the same observation times, irrespective of zero writes,
    // empty/unlisted keys, row ownership, or action-filtered writes.
    for (arguments, write, owner) in [
        ("", false, "alice"),
        ("(keys: [])", true, "bob"),
        ("(key: \"different\")", true, "bob"),
        ("(key: \"hidden\")", true, "bob"),
        ("(filter: {actions: [DELETED]})", true, "alice"),
    ] {
        let (_, _, db) = setup_db().await;
        let mut registry = OperationAssuranceRegistry::builder(
            graphql_orm_operation_catalog(),
            AssuranceSchemaConfig::legacy(),
        );
        registry
            .require(
                GraphqlOperationKind::Subscription,
                "secureRecordChanged",
                "current-session",
            )
            .unwrap();
        let allowed = Arc::new(AtomicBool::new(true));
        let schema = schema_builder(db)
            .data(AssuranceEnforcement::new(
                Arc::new(registry.build().unwrap()),
                Arc::new(CurrentAssurance(allowed.clone())),
            ))
            .finish();
        let _clock = ManualTime::pause();
        let start = tokio::time::Instant::now();
        let mut stream = Box::pin(
            schema.execute_stream(
                Request::new(format!(
                    "subscription {{ secureRecordChanged{arguments} {{ id }} }}"
                ))
                .data(AuthSubject::new("alice")),
            ),
        );
        registered(&mut stream).await;
        allowed.store(false, Ordering::SeqCst);
        if write {
            publish(&schema, "SecureRecord", "hidden", owner).await;
        }
        registered(&mut stream).await;
        tokio::time::advance(Duration::from_secs(29)).await;
        registered(&mut stream).await;
        if write {
            publish(&schema, "SecureRecord", "another-hidden", owner).await;
        }
        registered(&mut stream).await;
        tokio::time::advance(Duration::from_secs(5)).await;
        forbidden_end(&mut stream).await;
        assert_eq!(tokio::time::Instant::now() - start, Duration::from_secs(34));
    }
}

#[tokio::test]
async fn ordinary_and_append_only_static_roots_refuse_aliases_before_authority() {
    let (schema, authority) = setup().await;
    for field in ["secureRecordChanged", "secureAuditChanged"] {
        let mut stream = Box::pin(schema.execute_stream(format!(
            "subscription {{ a: {field}(keys: [\"a\", \"b\"]) {{ id }} b: {field}(keys: [\"c\", \"d\"]) {{ id }} c: {field}(keys: [\"e\", \"f\"]) {{ id }} }}"
        )));
        let response = stream.next().await.unwrap();
        assert_eq!(
            response.errors[0].extensions.as_ref().unwrap().get("code"),
            Some(&async_graphql::Value::from(
                "subscription_root_limit_exceeded"
            ))
        );
        assert!(stream.next().await.is_none());
    }
    assert_eq!(authority.entity_checks.load(Ordering::SeqCst), 0);
    assert_eq!(authority.row_checks.load(Ordering::SeqCst), 0);
}

struct AdmissionSource;
#[async_graphql::Subscription]
impl AdmissionSource {
    async fn source(&self, ctx: &async_graphql::Context<'_>) -> impl Stream<Item = i32> {
        ctx.data_unchecked::<Arc<AtomicUsize>>()
            .fetch_add(1, Ordering::SeqCst);
        graphql_orm::futures::stream::pending()
    }
}
#[derive(async_graphql::MergedSubscription)]
struct AdmissionRoots(AdmissionSource, SecureRecordSubscriptions);

#[tokio::test]
async fn skipped_generated_root_refuses_before_a_custom_source_opens() {
    use graphql_orm::graphql::subscriptions::GeneratedSubscriptionAdmission;
    let (_, authority, db) = setup_db().await;
    let opened = Arc::new(AtomicUsize::new(0));
    let schema = async_graphql::Schema::build(
        QueryRoot::default(),
        async_graphql::EmptyMutation,
        AdmissionRoots(AdmissionSource, SecureRecordSubscriptions),
    )
    .data(db)
    .data(opened.clone())
    .extension(GeneratedSubscriptionAdmission(vec!["secureRecordChanged"]))
    .finish();
    let mut stream = Box::pin(schema.execute_stream(
        "subscription { source secureRecordChanged(keys: []) @skip(if: true) { id } }",
    ));
    let response = stream.next().await.unwrap();
    assert_eq!(
        response.errors[0].extensions.as_ref().unwrap().get("code"),
        Some(&async_graphql::Value::from(
            "subscription_root_limit_exceeded"
        ))
    );
    assert!(stream.next().await.is_none());
    assert_eq!(opened.load(Ordering::SeqCst), 0);
    assert_eq!(authority.entity_checks.load(Ordering::SeqCst), 0);
}

struct RevokeAssuranceDuringEntityCheck {
    allowed: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
    revoke_at: usize,
}
impl EntityPolicy for RevokeAssuranceDuringEntityCheck {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if surface == EntityAccessSurface::GraphqlSubscription
                && self.calls.fetch_add(1, Ordering::SeqCst) + 1 == self.revoke_at
            {
                self.allowed.store(false, Ordering::SeqCst);
                tokio::task::yield_now().await;
            }
            Ok(true)
        })
    }
}

#[tokio::test]
async fn assurance_revocation_at_every_async_entity_check_silently_prevents_release() {
    // Learn the number of policy boundaries from an authorized delivery, then
    // revoke during each boundary, including the final release fence.
    let mut boundaries = 0;
    let mut revoke_at = 0;
    loop {
        let (_, _, mut db) = setup_db().await;
        let _clock = ManualTime::pause();
        let allowed = Arc::new(AtomicBool::new(true));
        let calls = Arc::new(AtomicUsize::new(0));
        db.set_entity_policy(RevokeAssuranceDuringEntityCheck {
            allowed: allowed.clone(),
            calls: calls.clone(),
            revoke_at,
        });
        let mut registry = OperationAssuranceRegistry::builder(
            graphql_orm_operation_catalog(),
            AssuranceSchemaConfig::legacy(),
        );
        registry
            .require(
                GraphqlOperationKind::Subscription,
                "secureRecordChanged",
                "current-session",
            )
            .unwrap();
        let schema = schema_builder(db)
            .data(AssuranceEnforcement::new(
                Arc::new(registry.build().unwrap()),
                Arc::new(CurrentAssurance(allowed.clone())),
            ))
            .finish();
        let mut stream = Box::pin(
            schema.execute_stream(
                Request::new("subscription { secureRecordChanged { id } }")
                    .data(AuthSubject::new("alice")),
            ),
        );
        registered(&mut stream).await;
        publish(&schema, "SecureRecord", "race-trigger", "alice").await;
        if revoke_at == 0 {
            let response = stream.next().await.unwrap();
            assert!(response.errors.is_empty());
            boundaries = calls.load(Ordering::SeqCst);
            assert!(boundaries > 1);
        } else {
            while allowed.load(Ordering::SeqCst) {
                registered(&mut stream).await;
                tokio::task::yield_now().await;
            }
            registered(&mut stream).await;
            tokio::time::advance(Duration::from_secs(34)).await;
            forbidden_end(&mut stream).await;
        }
        revoke_at = if revoke_at == 0 { 2 } else { revoke_at + 1 };
        if revoke_at > boundaries {
            break;
        }
    }
}

#[path = "support/subscription_time.rs"]
mod subscription_time;
use subscription_time::ManualTime;

#[tokio::test]
async fn review_revoked_unmatched_writes_do_not_signal_before_the_timer() {
    let (_, _, db) = setup_db().await;
    let mut registry = OperationAssuranceRegistry::builder(
        graphql_orm_operation_catalog(),
        AssuranceSchemaConfig::legacy(),
    );
    registry
        .require(
            GraphqlOperationKind::Subscription,
            "secureRecordChanged",
            "current-session",
        )
        .unwrap();
    let allowed = Arc::new(AtomicBool::new(true));
    let schema = schema_builder(db)
        .data(AssuranceEnforcement::new(
            Arc::new(registry.build().unwrap()),
            Arc::new(CurrentAssurance(allowed.clone())),
        ))
        .finish();
    let _clock = ManualTime::pause();
    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new("subscription { secureRecordChanged(keys: []) { id } }")
                .data(AuthSubject::new("alice")),
        ),
    );
    registered(&mut stream).await;
    allowed.store(false, Ordering::SeqCst);
    publish(&schema, "SecureRecord", "unmatched-private", "bob").await;
    registered(&mut stream).await;
    tokio::time::advance(Duration::from_secs(29)).await;
    registered(&mut stream).await;
}

#[tokio::test]
async fn review_unlisted_and_action_filtered_events_have_no_entity_policy_cost() {
    let (schema, authority) = setup().await;
    let _clock = ManualTime::pause();
    for arguments in [
        "(keys: [])",
        "(key: \"unlisted\")",
        "(filter: {actions: [DELETED]})",
    ] {
        let mut stream = Box::pin(
            schema.execute_stream(
                Request::new(format!(
                    "subscription {{ secureRecordChanged{arguments} {{ id }} }}"
                ))
                .data(AuthSubject::new("alice")),
            ),
        );
        registered(&mut stream).await;
        let before = authority.entity_checks.load(Ordering::SeqCst);
        publish(
            &schema,
            "SecureRecord",
            &format!("filtered-{before}"),
            "alice",
        )
        .await;
        registered(&mut stream).await;
        assert_eq!(authority.entity_checks.load(Ordering::SeqCst), before);
    }
}

#[tokio::test]
async fn review_hand_built_schema_requires_admission_even_with_skipped_siblings() {
    let (_, _, db) = setup_db().await;
    let sender = db.ensure_event_sender::<graphql_orm::graphql::subscriptions::SubscriptionEvent<SecureRecordChangedEvent>>();
    let schema = async_graphql::Schema::build(
        QueryRoot::default(),
        async_graphql::EmptyMutation,
        SecureRecordSubscriptions,
    )
    .data(db)
    .data(AuthSubject::new("alice"))
    .finish();
    let mut stream = Box::pin(schema.execute_stream(
        "subscription { a: secureRecordChanged { id } b: secureRecordChanged @skip(if: true) { id } }"
    ));
    let response = timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("missing admission must fail closed")
        .unwrap();
    assert_eq!(
        response.errors[0].extensions.as_ref().unwrap().get("code"),
        Some(&async_graphql::Value::from(
            "SUBSCRIPTION_ADMISSION_REQUIRED"
        ))
    );
    assert_eq!(sender.receiver_count(), 0);
    assert!(stream.next().await.is_none());
}

async fn forbidden_end<S: Stream<Item = Response> + Unpin>(stream: &mut S) {
    let response = poll_fn(|cx| match stream.poll_next_unpin(cx) {
        Poll::Ready(item) => Poll::Ready(item.expect("one terminal item")),
        Poll::Pending => panic!("the independent timer must yield a terminal item"),
    })
    .await;
    assert_eq!(response.errors.len(), 1, "{response:?}");
    assert_eq!(response.errors[0].message, "forbidden");
    assert_eq!(
        response.errors[0].extensions.as_ref().unwrap().get("code"),
        Some(&async_graphql::Value::from("FORBIDDEN"))
    );
    assert!(response.data.into_json().unwrap().is_null());
    assert!(stream.next().await.is_none());
}

struct ReviewPolicyFailure(Arc<AtomicUsize>);
impl EntityPolicy for ReviewPolicyFailure {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if surface != EntityAccessSurface::GraphqlSubscription {
                return Ok(true);
            }
            match self.0.load(Ordering::SeqCst) {
                0 => Ok(true),
                1 => Ok(false),
                2 => Err(async_graphql::Error::from(std::io::Error::other(
                    "private host failure",
                ))),
                3 => Err(async_graphql::Error::from(sqlx::Error::PoolTimedOut)),
                4 => Err(
                    graphql_orm::graphql::errors::OrmPublicError::internal("private failure")
                        .into_graphql_error(),
                ),
                5 => Err(async_graphql::Error::new("private authored refusal")),
                6 => {
                    use async_graphql::ErrorExtensions;
                    Err(async_graphql::Error::new("private authored refusal")
                        .extend_with(|_, ext| ext.set("code", "CUSTOM_DENIAL")))
                }
                8 => {
                    use async_graphql::ErrorExtensions;
                    Err(
                        async_graphql::Error::from(std::io::Error::other("private host failure"))
                            .extend_with(|_, ext| ext.set("code", "FORBIDDEN")),
                    )
                }
                _ => Err(graphql_orm::graphql::errors::OrmPublicError::new(
                    graphql_orm::graphql::errors::OrmErrorCode::ServiceUnavailable,
                )
                .into_graphql_error()),
            }
        })
    }
}

#[tokio::test]
async fn timer_distinguishes_definite_denials_from_host_failures_without_writes() {
    for mode in 1..=8 {
        let (_, authority, mut db) = setup_db().await;
        let current = Arc::new(AtomicUsize::new(0));
        db.set_entity_policy(ReviewPolicyFailure(current.clone()));
        let schema = schema_builder(db).finish();
        let _clock = ManualTime::pause();
        let mut stream = Box::pin(
            schema.execute_stream(
                Request::new("subscription { secureRecordChanged(keys: []) { id } }")
                    .data(AuthSubject::new("alice")),
            ),
        );
        registered(&mut stream).await;
        current.store(mode, Ordering::SeqCst);
        tokio::time::advance(Duration::from_secs(29)).await;
        registered(&mut stream).await;
        tokio::time::advance(Duration::from_secs(5)).await;
        let response = stream.next().await.unwrap();
        let expected = if matches!(mode, 1 | 5 | 6) {
            "FORBIDDEN"
        } else {
            "SUBSCRIPTION_CONTINUITY_LOST"
        };
        assert_eq!(response.errors.len(), 1);
        assert_eq!(
            response.errors[0].extensions.as_ref().unwrap().get("code"),
            Some(&async_graphql::Value::from(expected)),
            "mode {mode}: {response:?}"
        );
        assert!(!response.errors[0].message.contains("private"));
        assert!(response.data.into_json().unwrap().is_null());
        assert!(stream.next().await.is_none());
        assert_eq!(authority.row_checks.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn subscription_authority_interval_is_bounded_and_inherited_by_clones() {
    let (_, _, mut db) = setup_db().await;
    assert_eq!(
        db.subscription_authority_recheck_interval(),
        Duration::from_secs(30)
    );
    db.set_subscription_authority_recheck_interval(Duration::ZERO);
    assert_eq!(
        db.clone().subscription_authority_recheck_interval(),
        Duration::from_secs(1)
    );
    let db = db.with_subscription_authority_recheck_interval(Duration::MAX);
    assert_eq!(
        db.subscription_authority_recheck_interval(),
        Duration::from_secs(300)
    );
}

struct PendingFieldDiagnostic(Arc<tokio::sync::Notify>);
impl FieldPolicy for PendingFieldDiagnostic {
    fn can_read_field<'a>(
        &'a self,
        ctx: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        field: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if field == "label" {
                ctx.add_error(async_graphql::ServerError::new(
                    "private pending row diagnostic",
                    None,
                ));
                self.0.notify_one();
                return graphql_orm::futures::future::pending().await;
            }
            Ok(true)
        })
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

#[tokio::test]
async fn terminal_timer_discards_diagnostics_from_preempted_event_preparation() {
    for mode in [1, 2] {
        let (_, _, mut db) = setup_db().await;
        let current = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(tokio::sync::Notify::new());
        db.set_entity_policy(ReviewPolicyFailure(current.clone()));
        db.set_field_policy(PendingFieldDiagnostic(entered.clone()));
        let schema = schema_builder(db).finish();
        let _clock = ManualTime::pause();
        let mut stream = Box::pin(
            schema.execute_stream(
                Request::new("subscription { secureRecordChanged { id secureRecord { label } } }")
                    .data(AuthSubject::new("alice")),
            ),
        );
        registered(&mut stream).await;
        publish(&schema, "SecureRecord", "private-inflight", "alice").await;
        tokio::select! {
            () = entered.notified() => {},
            frame = stream.next() => panic!("preparation must remain pending: {frame:?}"),
        }
        current.store(mode, Ordering::SeqCst);
        tokio::time::advance(Duration::from_secs(34)).await;
        let response = stream.next().await.unwrap();
        let code = if mode == 1 {
            "FORBIDDEN"
        } else {
            "SUBSCRIPTION_CONTINUITY_LOST"
        };
        assert_eq!(response.errors.len(), 1, "{response:?}");
        assert_eq!(
            response.errors[0].extensions.as_ref().unwrap().get("code"),
            Some(&async_graphql::Value::from(code))
        );
        assert!(!response.errors[0].message.contains("private"));
        assert!(response.data.into_json().unwrap().is_null());
        assert!(stream.next().await.is_none());
    }
}
