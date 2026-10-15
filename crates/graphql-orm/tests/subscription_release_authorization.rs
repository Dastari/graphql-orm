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
    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new("subscription { secureRecordChanged { id sourceEntity sourceId path } }")
                .data(AuthSubject::new("alice")),
        ),
    );
    registered(&mut stream).await;
    assert_eq!(authority.entity_checks.load(Ordering::SeqCst), 1);
    authority.entity_allowed.store(false, Ordering::SeqCst);
    publish(&schema, "SecureRecord", "revoked-alice", "alice").await;
    assert!(
        timeout(Duration::from_millis(100), stream.next())
            .await
            .is_err(),
        "revoked subscriber received event metadata"
    );
    assert!(
        authority.entity_checks.load(Ordering::SeqCst) >= 2,
        "event was not reauthorized"
    );
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
