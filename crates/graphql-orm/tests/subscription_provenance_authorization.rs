#![cfg(feature = "sqlite")]
use graphql_orm::async_graphql::{Request, Response};
use graphql_orm::futures::{Stream, StreamExt, future::poll_fn};
use graphql_orm::prelude::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::task::Poll;
use tokio::time::{Duration, timeout};

#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[graphql_entity(table = "proof_parents", plural = "ProofParents")]
struct ProofParent {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    owner: String,
}
#[derive(
    GraphQLEntity,
    GraphQLOperations,
    GraphQLRelations,
    async_graphql::SimpleObject,
    Clone,
    Debug,
    PartialEq,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_entity(table = "proof_children", plural = "ProofChildren")]
struct ProofChild {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[graphql(guard = "NeverRelease")]
    owner: String,
    parent_id: String,
    #[graphql(skip)]
    #[relation(
        target = "ProofParent",
        from = "parent_id",
        to = "id",
        propagate_change = "up",
        emit_fk = false
    )]
    parent: Option<ProofParent>,
}
impl BatchLoadEntity for ProofParent {
    fn batch_column() -> &'static str {
        "id"
    }
    fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
        SqliteBackend::try_get_string(row, "id")
    }
}
#[derive(
    GraphQLEntity,
    GraphQLOperations,
    async_graphql::SimpleObject,
    Clone,
    Debug,
    PartialEq,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_entity(table = "guarded_key_rows", plural = "GuardedKeyRows")]
struct GuardedKeyRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[graphql(guard = "NeverRelease")]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    value: String,
}
#[derive(
    async_graphql::SimpleObject, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
struct PersistedDetails {
    #[graphql(guard = "NeverRelease")]
    secret: String,
}
#[derive(
    GraphQLEntity,
    GraphQLOperations,
    async_graphql::SimpleObject,
    Clone,
    Debug,
    PartialEq,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_entity(table = "persisted_object_rows", plural = "PersistedObjectRows")]
struct PersistedObjectRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[graphql_orm(json)]
    details: PersistedDetails,
}
#[async_graphql::ComplexObject]
impl GuardedKeyRow {}
#[async_graphql::ComplexObject]
impl PersistedObjectRow {}

struct NeverRelease;
impl async_graphql::Guard for NeverRelease {
    async fn check(&self, _: &async_graphql::Context<'_>) -> async_graphql::Result<()> {
        Err(async_graphql::Error::new("synthetic-private-field-guard"))
    }
}
schema_roots! { query_custom_ops: [], entities: [ProofParent, ProofChild, GuardedKeyRow, PersistedObjectRow], }
#[derive(Clone)]
struct Policy {
    source_allowed: Arc<AtomicBool>,
    link_allowed: Arc<AtomicBool>,
    key_allowed: Arc<AtomicBool>,
}
impl EntityPolicy for Policy {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(surface != EntityAccessSurface::GraphqlSubscription
                || entity != "ProofChild"
                || self.source_allowed.load(Ordering::SeqCst))
        })
    }
}
impl RowPolicy for Policy {
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
            let owner = row
                .downcast_ref::<ProofParent>()
                .map(|r| &r.owner)
                .or_else(|| row.downcast_ref::<ProofChild>().map(|r| &r.owner));
            Ok(ctx
                .and_then(|c| c.auth_subject_opt())
                .is_some_and(|s| Some(&s.id) == owner))
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
impl FieldPolicy for Policy {
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        entity: &'static str,
        field: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(entity != "ProofChild"
                || match field {
                    "id" => self.key_allowed.load(Ordering::SeqCst),
                    "parentId" | "parent" => self.link_allowed.load(Ordering::SeqCst),
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
async fn setup() -> (AppSchema, Database, Policy) {
    let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("CREATE TABLE proof_parents (id TEXT PRIMARY KEY, owner TEXT NOT NULL)")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("CREATE TABLE proof_children (id TEXT PRIMARY KEY, owner TEXT NOT NULL, parent_id TEXT NOT NULL)").execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO proof_parents VALUES ('parent-alice', 'alice')")
        .execute(db.pool())
        .await
        .unwrap();
    let policy = Policy {
        source_allowed: Arc::new(AtomicBool::new(true)),
        link_allowed: Arc::new(AtomicBool::new(true)),
        key_allowed: Arc::new(AtomicBool::new(true)),
    };
    db.set_entity_policy(policy.clone());
    db.set_row_policy(policy.clone());
    db.set_field_policy(policy.clone());
    (schema_builder(db.clone()).finish(), db, policy)
}
async fn registered<S: Stream<Item = Response> + Unpin>(stream: &mut S) {
    poll_fn(|cx| match stream.poll_next_unpin(cx) {
        Poll::Pending => Poll::Ready(()),
        other => panic!("unexpected initial event {other:?}"),
    })
    .await;
}
async fn create_child(schema: &AppSchema, id: &str, owner: &str) {
    let response = schema.execute(Request::new(format!("mutation {{ createProofChild(input: {{ id: \"{id}\", owner: \"{owner}\", parentId: \"parent-alice\" }}) {{ success error }} }}")).data(AuthSubject::new(owner))).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["createProofChild"]["success"],
        true
    );
}
const SUBSCRIPTION: &str =
    "subscription { proofParentChanged { id sourceEntity sourceId path proofParent { owner } } }";
#[tokio::test]
async fn target_owner_cannot_receive_private_source_provenance() {
    let (schema, _, _) = setup().await;
    let mut stream =
        Box::pin(schema.execute_stream(Request::new(SUBSCRIPTION).data(AuthSubject::new("alice"))));
    registered(&mut stream).await;
    create_child(&schema, "private-bob", "bob").await;
    create_child(&schema, "allowed-alice", "alice").await;
    let response = timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data["proofParentChanged"]["sourceId"], "allowed-alice");
    assert_eq!(data["proofParentChanged"]["sourceEntity"], "ProofChild");
    assert_eq!(
        data["proofParentChanged"]["path"],
        serde_json::json!(["parent"])
    );
}
#[tokio::test]
async fn each_source_entity_key_and_link_policy_is_required() {
    for denied in ["entity", "key", "link"] {
        let (schema, _, policy) = setup().await;
        let mut stream = Box::pin(
            schema.execute_stream(Request::new(SUBSCRIPTION).data(AuthSubject::new("alice"))),
        );
        registered(&mut stream).await;
        match denied {
            "entity" => policy.source_allowed.store(false, Ordering::SeqCst),
            "key" => policy.key_allowed.store(false, Ordering::SeqCst),
            _ => policy.link_allowed.store(false, Ordering::SeqCst),
        }
        create_child(&schema, "denied-source", "alice").await;
        assert!(
            timeout(Duration::from_millis(100), stream.next())
                .await
                .is_err(),
            "source {denied} released metadata"
        );
    }
}
#[tokio::test]
async fn missing_current_source_drops_propagated_delete() {
    let (schema, _, _) = setup().await;
    create_child(&schema, "deleted-child", "alice").await;
    let mut stream =
        Box::pin(schema.execute_stream(Request::new(SUBSCRIPTION).data(AuthSubject::new("alice"))));
    registered(&mut stream).await;
    let response = schema
        .execute(
            Request::new("mutation { deleteProofChild(id: \"deleted-child\") { success error } }")
                .data(AuthSubject::new("alice")),
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["deleteProofChild"]["success"],
        true
    );
    assert!(
        timeout(Duration::from_millis(100), stream.next())
            .await
            .is_err()
    );
}
#[tokio::test]
async fn raw_events_cannot_inject_graphql_provenance() {
    let (schema, db, _) = setup().await;
    let mut stream =
        Box::pin(schema.execute_stream(Request::new(SUBSCRIPTION).data(AuthSubject::new("alice"))));
    registered(&mut stream).await;
    db.emit_event(ProofParentChangedEvent {
        action: ChangeAction::Created,
        change_kind: ChangeKind::Propagated,
        id: "parent-alice".into(),
        source_entity: Some("secret-source".into()),
        source_id: Some("secret-id".into()),
        path: vec!["secret-path".into()],
        entity: Some(ProofParent {
            id: "parent-alice".into(),
            owner: "alice".into(),
        }),
    });
    create_child(&schema, "generated-source", "alice").await;
    let response = timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["proofParentChanged"]["sourceId"],
        "generated-source"
    );
}

#[tokio::test]
async fn stale_relationship_binding_cannot_release_prior_provenance() {
    let (schema, db, _) = setup().await;
    let mut stream =
        Box::pin(schema.execute_stream(Request::new(SUBSCRIPTION).data(AuthSubject::new("alice"))));
    registered(&mut stream).await;
    create_child(&schema, "reassigned-source", "alice").await;
    sqlx::query("INSERT INTO proof_parents VALUES ('other-parent', 'alice')")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE proof_children SET parent_id = 'other-parent' WHERE id = 'reassigned-source'",
    )
    .execute(db.pool())
    .await
    .unwrap();
    create_child(&schema, "current-source", "alice").await;
    let response = timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["proofParentChanged"]["sourceId"],
        "current-source"
    );
}

#[tokio::test]
async fn nested_relation_selection_is_rejected_before_subscription_registration() {
    let (schema, _, _) = setup().await;
    for query in [
        "subscription { proofChildChanged { id proofChild { parent { owner } } } }",
        "subscription { proofChildChanged { outer: id child: proofChild { ...Nested } } } fragment Nested on ProofChild { hidden: parent { owner } }",
    ] {
        let mut stream =
            Box::pin(schema.execute_stream(Request::new(query).data(AuthSubject::new("alice"))));
        let response = timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.errors.len(), 1, "{response:?}");
        assert_eq!(
            response.errors[0].extensions.as_ref().unwrap().get("code"),
            Some(&async_graphql::Value::from("INVALID_INPUT"))
        );
        assert!(
            response.errors[0]
                .message
                .contains("persisted subscribable scalar")
        );
        assert_eq!(response.data, async_graphql::Value::Null);
    }
}

#[tokio::test]
async fn arbitrary_graphql_scalar_guards_are_rejected_before_outer_metadata() {
    let (schema, _, _) = setup().await;
    let mut stream = Box::pin(schema.execute_stream(Request::new(
        "subscription { proofChildChanged { id child: proofChild { ...Guarded } } } fragment Guarded on ProofChild { hidden: owner }"
    ).data(AuthSubject::new("alice"))));
    let response = timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.errors.len(), 1, "{response:?}");
    assert_eq!(
        response.errors[0].extensions.as_ref().unwrap().get("code"),
        Some(&async_graphql::Value::from("INVALID_INPUT"))
    );
    assert!(
        !serde_json::to_string(&response)
            .unwrap()
            .contains("synthetic-private-field-guard")
    );
    assert_eq!(response.data, async_graphql::Value::Null);
}

#[tokio::test]
async fn unsupported_primary_key_guard_is_rejected_for_outer_identifier_aliases() {
    let (schema, _, _) = setup().await;
    for query in [
        "subscription { guardedKeyRowChanged { outward: id } }",
        "subscription { guardedKeyRowChanged { ...Identity } } fragment Identity on GuardedKeyRowChangedEvent { hidden: id }",
    ] {
        let mut stream =
            Box::pin(schema.execute_stream(Request::new(query).data(AuthSubject::new("alice"))));
        let response = timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.errors.len(), 1, "{response:?}");
        assert_eq!(
            response.errors[0].extensions.as_ref().unwrap().get("code"),
            Some(&async_graphql::Value::from("INVALID_INPUT"))
        );
        assert!(
            !serde_json::to_string(&response)
                .unwrap()
                .contains("synthetic-private-field-guard")
        );
        assert_eq!(response.data, async_graphql::Value::Null);
    }
}

#[tokio::test]
async fn persisted_object_columns_are_not_mistaken_for_scalar_payloads() {
    let (schema, _, _) = setup().await;
    for query in [
        "subscription { persistedObjectRowChanged { id persistedObjectRow { details { secret } } } }",
        "subscription { persistedObjectRowChanged { id row: persistedObjectRow { column: details { ...NestedDetails } } } } fragment NestedDetails on PersistedDetails { hidden: secret }",
    ] {
        let mut stream =
            Box::pin(schema.execute_stream(Request::new(query).data(AuthSubject::new("alice"))));
        let response = timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.errors.len(), 1, "{response:?}");
        assert_eq!(
            response.errors[0].extensions.as_ref().unwrap().get("code"),
            Some(&async_graphql::Value::from("INVALID_INPUT"))
        );
        assert!(
            !serde_json::to_string(&response)
                .unwrap()
                .contains("synthetic-private-field-guard")
        );
        assert_eq!(response.data, async_graphql::Value::Null);
    }
}
