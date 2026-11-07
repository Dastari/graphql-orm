//! Local identities and keyed live delivery on test-owned databases.
#![cfg(all(
    feature = "runtime-graphql",
    any(
        all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
        all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
    )
))]
use graphql_orm::{
    async_graphql::{Request, Response, dynamic::Schema},
    db::Database,
    futures::{StreamExt, future::BoxFuture, stream::BoxStream},
    graphql::{orm::*, runtime::*},
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{Notify, broadcast},
    time::timeout,
};
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
fn fid(s: &str) -> FieldId {
    FieldId::new(s).unwrap()
}
fn field(name: &str, kind: RuntimeValueKind) -> RuntimeField {
    RuntimeField {
        id: fid(name),
        api_name: name.into(),
        physical_column: name.to_ascii_lowercase(),
        value_kind: kind,
        nullable: false,
        unique: false,
        filterable: true,
        sortable: true,
        generated: false,
        default: None,
        format: None,
        description: None,
        classification: None,
        export: None,
        auto_timestamp: None,
        auto_actor: None,
    }
}
fn definition() -> RuntimeSchema {
    let mut version = field("rowVersion", RuntimeValueKind::Integer);
    version.default = Some(RuntimeDefault::Literal("1".into()));
    RuntimeSchema {
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![RuntimeCollection {
            id: CollectionId::new("records").unwrap(),
            api_type_name: "Record".into(),
            api_plural_name: "Records".into(),
            physical_table: "runtime_live_records".into(),
            primary_key: vec![fid("id")],
            append_only: false,
            retention_purge: false,
            fields: vec![
                field("id", RuntimeValueKind::String),
                field("title", RuntimeValueKind::String),
                field("tenant", RuntimeValueKind::String),
                version,
            ],
            relations: vec![],
            indexes: vec![],
            composite_unique: vec![],
            default_order: vec![],
            description: None,
            classification: None,
            label_fields: vec![],
            search: None,
            node_identity: Some(RuntimeNodeIdentity {
                version_field: fid("rowVersion"),
            }),
            live_subscriptions: Some(RuntimeLiveSubscriptions {
                subscription_key_limit: 2,
                deleted_images: true,
            }),
        }],
    }
}
struct Source {
    tx: broadcast::Sender<RuntimeLiveEvent>,
    subscribed: Notify,
}
impl RuntimeLiveSource for Source {
    fn subscribe(
        &self,
        _: &SchemaFingerprint,
        _: &CollectionId,
    ) -> Result<broadcast::Receiver<RuntimeLiveEvent>, RuntimeGraphqlError> {
        let receiver = self.tx.subscribe();
        self.subscribed.notify_one();
        Ok(receiver)
    }
}
#[derive(Default)]
struct Authority {
    registration_all: AtomicBool,
    revoked: AtomicBool,
    revoke_after_grant: AtomicBool,
    move_scope_after_grant: AtomicBool,
    use_south: AtomicBool,
    revoke_image_after_check: AtomicBool,
    deny_title: AtomicBool,
    allow_delete: AtomicBool,
    pause: AtomicBool,
    entered: Notify,
    resume: Notify,
    purposes: Mutex<Vec<RuntimeReadPurpose>>,
    operation_checks: Mutex<Vec<(RuntimeReadPurpose, RuntimeReadPurpose)>>,
}
impl RuntimeReadAuthority for Authority {
    fn authorize_mutation_result<'a>(
        &'a self,
        check: RuntimeMutationOutputCheck<'a>,
    ) -> BoxFuture<'a, Result<(), RuntimeGraphqlError>> {
        Box::pin(async move {
            if self.revoked.load(Ordering::SeqCst) {
                return Err(RuntimeGraphqlError::denied());
            }
            let tenant = check
                .schema
                .resolve_field(check.collection, &fid("tenant"))
                .unwrap();
            if check.record.value(&tenant).ok() != Some(&RuntimeValue::String("north".into())) {
                return Err(RuntimeGraphqlError::denied());
            }
            Ok(())
        })
    }

    fn authorize<'a>(
        &'a self,
        check: RuntimeReadCheck<'a>,
    ) -> BoxFuture<'a, Result<RuntimeReadGrant, RuntimeGraphqlError>> {
        Box::pin(async move {
            self.purposes.lock().unwrap().push(check.purpose);
            self.operation_checks
                .lock()
                .unwrap()
                .push((check.purpose, check.operation_purpose));
            if check.purpose == RuntimeReadPurpose::SubscriptionDelivery
                && self.pause.swap(false, Ordering::SeqCst)
            {
                self.entered.notify_one();
                self.resume.notified().await;
            }
            if self.revoked.load(Ordering::SeqCst)
                || (self.deny_title.load(Ordering::SeqCst)
                    && check.projection.iter().any(|f| f.id() == &fid("title")))
            {
                return Err(RuntimeGraphqlError::denied());
            }
            if check.purpose == RuntimeReadPurpose::SubscriptionRegistration
                && self.registration_all.load(Ordering::SeqCst)
            {
                return Ok(RuntimeReadGrant::new(None));
            }
            let tenant = check
                .schema
                .resolve_field(check.collection, &fid("tenant"))
                .unwrap();
            let predicate = check
                .schema
                .runtime_compare(
                    check.collection,
                    &tenant,
                    RuntimeScalarOperator::Eq,
                    RuntimeValue::String(if self.use_south.load(Ordering::SeqCst) {
                        "south".into()
                    } else {
                        "north".into()
                    }),
                    Default::default(),
                )
                .unwrap();
            if check.purpose == RuntimeReadPurpose::SubscriptionDelivery {
                if self.revoke_after_grant.swap(false, Ordering::SeqCst) {
                    self.revoked.store(true, Ordering::SeqCst);
                }
                if self.move_scope_after_grant.swap(false, Ordering::SeqCst) {
                    self.use_south.store(true, Ordering::SeqCst);
                }
            }
            Ok(RuntimeReadGrant::new(Some(predicate)))
        })
    }
    fn authorize_deleted_image<'a>(
        &'a self,
        check: RuntimeDeletedImageCheck<'a>,
    ) -> BoxFuture<'a, Result<(), RuntimeGraphqlError>> {
        Box::pin(async move {
            if !self.allow_delete.load(Ordering::SeqCst) {
                return Err(RuntimeGraphqlError::denied());
            }
            let tenant = check
                .schema
                .resolve_field(check.key.collection(), &fid("tenant"))
                .unwrap();
            if check.image.value(&tenant).ok() != Some(&RuntimeValue::String("north".into())) {
                return Err(RuntimeGraphqlError::denied());
            }
            if self.revoke_image_after_check.swap(false, Ordering::SeqCst) {
                self.allow_delete.store(false, Ordering::SeqCst);
            }
            Ok(())
        })
    }
}
struct Fixture {
    db: Database<Backend>,
    schema: Arc<ValidatedRuntimeSchema>,
    environment: Arc<RuntimeMutationEnvironment>,
    api: Schema,
    source: Arc<Source>,
    authority: Arc<Authority>,
    #[cfg(feature = "postgres")]
    _owned: owned_postgres::OwnedPostgres,
}
impl Fixture {
    async fn new() -> Self {
        Self::with_definition(definition()).await
    }
    async fn with_definition(definition: RuntimeSchema) -> Self {
        #[cfg(feature = "postgres")]
        let owned = owned_postgres::OwnedPostgres::start("runtime-live").unwrap();
        #[cfg(feature = "postgres")]
        let db = Database::<Backend>::connect_postgres(&owned.url)
            .await
            .unwrap();
        #[cfg(feature = "sqlite")]
        let db = Database::<Backend>::connect_sqlite("sqlite::memory:")
            .await
            .unwrap();
        let schema = Arc::new(definition.validate().unwrap());
        let target = schema
            .physical_schema::<Backend>(Default::default())
            .unwrap();
        let ownership = ManagedTableSet::new(["runtime_live_records".into()]).unwrap();
        let plan = db
            .schema()
            .plan_owned_migration(
                "init",
                "runtime live",
                &target,
                &ownership,
                PlanOptions::strict(),
            )
            .await
            .unwrap();
        db.schema()
            .apply_owned_migration(&plan, ApplyOptions::default())
            .await
            .unwrap();
        let environment = Arc::new(
            db.schema()
                .runtime_mutation_environment(schema.clone(), &target, &ownership)
                .await
                .unwrap(),
        );
        let insert = match schema.schema().collections[0].fields[0].value_kind {
            RuntimeValueKind::Integer => {
                "INSERT INTO runtime_live_records (id,title,tenant) VALUES (1,'initial','north'),(2,'other','north'),(3,'secret','south')"
            }
            RuntimeValueKind::Uuid => {
                "INSERT INTO runtime_live_records (id,title,tenant) VALUES ('11111111-1111-4111-8111-111111111111','initial','north'),('22222222-2222-4222-8222-222222222222','other','north'),('33333333-3333-4333-8333-333333333333','secret','south')"
            }
            RuntimeValueKind::Boolean => {
                "INSERT INTO runtime_live_records (id,title,tenant) VALUES (TRUE,'initial','north'),(FALSE,'secret','south')"
            }
            _ => {
                "INSERT INTO runtime_live_records (id,title,tenant) VALUES ('visible','initial','north'),('other','other','north'),('hidden','secret','south')"
            }
        };
        graphql_orm::sqlx::query(insert)
            .execute(db.pool())
            .await
            .unwrap();
        let (tx, _) = broadcast::channel(8);
        let source = Arc::new(Source {
            tx,
            subscribed: Notify::new(),
        });
        #[cfg(feature = "sqlite")]
        let match_profile = RuntimeMatchProfile::SqliteBinaryCanonical;
        #[cfg(feature = "postgres")]
        let match_profile = RuntimeMatchProfile::PostgresCUtf8;
        let module = RuntimeGraphqlModule::compile(
            schema.clone(),
            RuntimeGraphqlOptions::default()
                .with_cursor_profile(RuntimeCursorProfile::Unprotected)
                .with_live_source(
                    source.clone(),
                    RuntimeLiveProfile::ApplicationPolicy { match_profile },
                ),
        )
        .unwrap();
        let (api, catalog) = RuntimeGraphqlComposer::new(db.clone(), "Query", Default::default())
            .unwrap()
            .install(module)
            .unwrap()
            .finish_with_semantic_catalog()
            .unwrap();
        catalog.validate().unwrap();
        assert_eq!(
            catalog
                .entities
                .iter()
                .find(|e| e.entity_name == "Record")
                .unwrap()
                .record_version
                .as_ref()
                .unwrap()
                .mode,
            GraphqlRecordVersionMode::Managed
        );
        Self {
            db,
            schema,
            environment,
            api,
            source,
            authority: Arc::new(Authority::default()),
            #[cfg(feature = "postgres")]
            _owned: owned,
        }
    }
    fn request(&self, query: &str) -> Request {
        Request::new(query).data(RuntimeGraphqlRequest::<Backend>::new(
            self.schema.fingerprint(),
            self.authority.clone(),
            None,
            Default::default(),
        ))
    }
    fn collection(&self) -> RuntimeCollectionHandle {
        self.schema
            .resolve_collection(&CollectionId::new("records").unwrap())
            .unwrap()
    }
    fn field(&self, name: &str) -> RuntimeFieldHandle {
        self.schema
            .resolve_field(&self.collection(), &fid(name))
            .unwrap()
    }
    fn key(&self, key: &str) -> RuntimeKey {
        let fields = self.schema.schema().collections[0]
            .primary_key
            .iter()
            .map(|id| {
                (
                    self.schema.resolve_field(&self.collection(), id).unwrap(),
                    RuntimeValue::String(if id == &fid("tenant") {
                        "north".into()
                    } else {
                        key.into()
                    }),
                )
            })
            .collect::<Vec<_>>();
        self.schema
            .runtime_key(&self.collection(), &fields)
            .unwrap()
    }
    fn emit(&self, key: &str, action: RuntimeMutationAction, preimage: Option<RuntimeRecord>) {
        self.source
            .tx
            .send(RuntimeLiveEvent::Changed {
                key: self.key(key),
                action,
                preimage,
            })
            .unwrap();
    }
    async fn stream(&self, query: &str) -> BoxStream<'static, Response> {
        let mut stream = self.api.execute_stream(self.request(query)).boxed();
        tokio::select! { response = stream.next() => panic!("registration produced a frame: {response:?}"), () = self.source.subscribed.notified() => {} }
        stream
    }
    async fn image(&self, key: &str, fields: &[&str]) -> RuntimeRecord {
        let projection = self
            .schema
            .resolve_projection(
                &self.collection(),
                &fields.iter().map(|f| self.field(f)).collect::<Vec<_>>(),
            )
            .unwrap();
        let restriction = self
            .schema
            .runtime_and(&self.collection(), vec![], Default::default())
            .unwrap();
        let request = self
            .schema
            .runtime_key_read_request(
                &self.key(key),
                &projection,
                &restriction,
                None,
                Default::default(),
            )
            .unwrap();
        self.db
            .execute_runtime_key_read(&request, None)
            .await
            .unwrap()
            .unwrap()
    }
}
const LIVE: &str = r#"subscription { recordChanged(keys:["visible"]) { action id record { id title NodeId RecordVersion } } }"#;
async fn next(stream: &mut BoxStream<'static, Response>) -> Response {
    timeout(Duration::from_secs(10), stream.next())
        .await
        .unwrap()
        .expect("a positive baseline must deliver")
}
async fn no_delivery(stream: &mut BoxStream<'static, Response>) {
    assert!(
        timeout(Duration::from_millis(100), stream.next())
            .await
            .is_err(),
        "refused event must produce no frame"
    );
}
async fn baseline(f: &Fixture, stream: &mut BoxStream<'static, Response>) {
    f.emit("visible", RuntimeMutationAction::Update, None);
    let value = next(stream).await;
    assert!(value.errors.is_empty(), "{value:?}");
    assert_eq!(
        value.data.into_json().unwrap()["recordChanged"]["record"]["id"],
        "visible"
    );
}
#[tokio::test]
async fn node_identity_preserves_id_and_hidden_equals_missing() {
    let f = Fixture::new().await;
    for (key, visible) in [("visible", true), ("hidden", false), ("missing", false)] {
        let id = graphql_orm::graphql::federation::encode_node_id("Record", vec![key.into()]);
        let query = format!(
            "{{ node(NodeId:{}) {{ NodeId ... on Record {{ id title RecordVersion }} }} }}",
            serde_json::to_string(&id.0).unwrap()
        );
        let response = f.api.execute(f.request(&query)).await;
        assert!(response.errors.is_empty(), "{response:?}");
        let value = response.data.into_json().unwrap();
        if visible {
            assert_eq!(value["node"]["id"], key);
            assert_eq!(value["node"]["NodeId"], id.0);
            assert_eq!(value["node"]["RecordVersion"], 1);
        } else {
            assert_eq!(value, serde_json::json!({"node":null}));
        }
    }
    let list = f
        .api
        .execute(f.request("{ records { edges { node { id NodeId RecordVersion } } } }"))
        .await;
    assert!(list.errors.is_empty(), "{list:?}");
    let sdl = f.api.sdl();
    assert!(sdl.contains("NodeId: ID!"));
    assert!(sdl.contains("id: String!"));
    assert!(sdl.contains("RecordVersion: Int!"));
    assert!(sdl.contains("node(NodeId: ID!): Node"));
    let purposes = f.authority.purposes.lock().unwrap();
    assert!(purposes.contains(&RuntimeReadPurpose::Select));
    assert!(purposes.contains(&RuntimeReadPurpose::NodeLookup));
}
#[tokio::test]
async fn hidden_missing_and_other_key_events_produce_no_frames_after_positive_baseline() {
    let f = Fixture::new().await;
    let mut stream = f.stream("subscription { recordChanged(keys:[\"visible\",\"hidden\"]) { id record { id title } } }").await;
    baseline(&f, &mut stream).await;
    // The bound is on the list as sent, before deduplication or row access.
    let mut refused_stream = f
        .api
        .execute_stream(f.request(
            "subscription { recordChanged(keys:[\"visible\",\"visible\",\"visible\"]) { id } }",
        ))
        .boxed();
    let over = timeout(Duration::from_secs(10), refused_stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(!over.errors.is_empty());
    assert_eq!(over.data, graphql_orm::async_graphql::Value::Null);
    assert!(
        timeout(Duration::from_secs(10), refused_stream.next())
            .await
            .unwrap()
            .is_none()
    );
    f.emit("hidden", RuntimeMutationAction::Update, None);
    no_delivery(&mut stream).await;
    f.emit("other", RuntimeMutationAction::Update, None);
    no_delivery(&mut stream).await;
    f.authority.revoked.store(true, Ordering::SeqCst);
    f.emit("hidden", RuntimeMutationAction::Update, None);
    no_delivery(&mut stream).await;
    assert!(
        f.authority
            .purposes
            .lock()
            .unwrap()
            .contains(&RuntimeReadPurpose::SubscriptionRegistration)
    );
    assert!(
        f.authority
            .purposes
            .lock()
            .unwrap()
            .contains(&RuntimeReadPurpose::SubscriptionDelivery)
    );
}
#[tokio::test]
async fn field_denial_and_queued_policy_revocation_stop_delivery() {
    let f = Fixture::new().await;
    let mut stream = f.stream(LIVE).await;
    baseline(&f, &mut stream).await;
    f.authority.deny_title.store(true, Ordering::SeqCst);
    f.emit("visible", RuntimeMutationAction::Update, None);
    assert!(
        timeout(Duration::from_secs(10), stream.next())
            .await
            .unwrap()
            .is_none()
    );
    f.authority.deny_title.store(false, Ordering::SeqCst);
    let mut stream = f.stream(LIVE).await;
    baseline(&f, &mut stream).await;
    f.authority.pause.store(true, Ordering::SeqCst);
    f.emit("visible", RuntimeMutationAction::Update, None);
    let task = tokio::spawn(async move {
        let value = stream.next().await;
        (value, stream)
    });
    f.authority.entered.notified().await;
    f.authority.revoked.store(true, Ordering::SeqCst);
    f.authority.resume.notify_one();
    let (value, _) = task.await.unwrap();
    assert!(value.is_none(), "a queued event crossed revocation");
}
#[tokio::test]
async fn reload_reads_current_state_and_visibility_loss_is_removed() {
    let f = Fixture::new().await;
    let mut stream = f.stream(LIVE).await;
    baseline(&f, &mut stream).await;
    graphql_orm::sqlx::query("UPDATE runtime_live_records SET title='current',rowversion=rowversion+1 WHERE id='visible'").execute(f.db.pool()).await.unwrap();
    f.emit("visible", RuntimeMutationAction::Update, None);
    let response = next(&mut stream).await;
    assert!(response.errors.is_empty(), "{response:?}");
    let value = response.data.into_json().unwrap();
    assert_eq!(value["recordChanged"]["record"]["title"], "current");
    assert_eq!(value["recordChanged"]["record"]["RecordVersion"], 2);
    let image = f
        .image("visible", &["id", "title", "tenant", "rowVersion"])
        .await;
    graphql_orm::sqlx::query(
        "UPDATE runtime_live_records SET tenant='south',rowversion=rowversion+1 WHERE id='visible'",
    )
    .execute(f.db.pool())
    .await
    .unwrap();
    f.emit("visible", RuntimeMutationAction::Update, None);
    no_delivery(&mut stream).await;
    f.authority.allow_delete.store(true, Ordering::SeqCst);
    f.emit("visible", RuntimeMutationAction::Update, Some(image));
    let response = next(&mut stream).await;
    assert!(response.errors.is_empty());
    let value = response.data.into_json().unwrap();
    assert_eq!(value["recordChanged"]["action"], "DELETED");
    assert!(value["recordChanged"]["record"].is_null());
    f.emit("visible", RuntimeMutationAction::Update, None);
    no_delivery(&mut stream).await;
}
#[tokio::test]
async fn deleted_images_require_complete_evidence_current_policy_and_application_profile() {
    let f = Fixture::new().await;
    let mut stream = f.stream(LIVE).await;
    baseline(&f, &mut stream).await;
    let image = f
        .image("visible", &["id", "title", "tenant", "rowVersion"])
        .await;
    let missing = f.image("visible", &["id"]).await;
    let foreign = f
        .image("hidden", &["id", "title", "tenant", "rowVersion"])
        .await;
    graphql_orm::sqlx::query("DELETE FROM runtime_live_records WHERE id='visible'")
        .execute(f.db.pool())
        .await
        .unwrap();
    f.emit("visible", RuntimeMutationAction::Delete, None);
    no_delivery(&mut stream).await;
    f.emit(
        "visible",
        RuntimeMutationAction::Delete,
        Some(image.clone()),
    );
    no_delivery(&mut stream).await;
    f.authority.allow_delete.store(true, Ordering::SeqCst);
    f.emit("visible", RuntimeMutationAction::Delete, Some(missing));
    no_delivery(&mut stream).await;
    f.emit("visible", RuntimeMutationAction::Delete, Some(foreign));
    no_delivery(&mut stream).await;
    f.emit("visible", RuntimeMutationAction::Delete, Some(image));
    let response = next(&mut stream).await;
    assert!(response.errors.is_empty(), "{response:?}");
    let value = response.data.into_json().unwrap();
    assert_eq!(value["recordChanged"]["id"], "visible");
    assert!(value["recordChanged"]["record"].is_null());
    let module = RuntimeGraphqlModule::compile(
        f.schema.clone(),
        RuntimeGraphqlOptions::default()
            .with_cursor_profile(RuntimeCursorProfile::Unprotected)
            .with_live_source(f.source.clone(), RuntimeLiveProfile::NativeRls),
    )
    .unwrap();
    let error = RuntimeGraphqlComposer::new(f.db.clone(), "Query", Default::default())
        .unwrap()
        .install(module)
        .err()
        .unwrap();
    assert_eq!(error.0[0].code(), "unsupported_deleted_image_profile");
}
#[tokio::test]
async fn overflow_during_authorization_closes_before_any_partial_payload() {
    let f = Fixture::new().await;
    let mut stream = f.stream(LIVE).await;
    baseline(&f, &mut stream).await;
    f.authority.pause.store(true, Ordering::SeqCst);
    f.emit("visible", RuntimeMutationAction::Update, None);
    let task = tokio::spawn(async move {
        let value = stream.next().await;
        (value, stream)
    });
    f.authority.entered.notified().await;
    for _ in 0..32 {
        f.emit("visible", RuntimeMutationAction::Update, None);
    }
    f.authority.resume.notify_one();
    let (response, mut stream) = task.await.unwrap();
    let response = response.unwrap();
    assert_eq!(response.data, graphql_orm::async_graphql::Value::Null);
    assert_eq!(response.errors.len(), 1);
    assert_eq!(
        response.errors[0]
            .extensions
            .as_ref()
            .unwrap()
            .get("code")
            .unwrap()
            .to_string(),
        "\"SUBSCRIPTION_CONTINUITY_LOST\""
    );
    assert!(stream.next().await.is_none());
}
#[tokio::test]
async fn host_restart_revision_and_source_close_use_terminal_error_then_complete() {
    let f = Fixture::new().await;
    for signal in [
        RuntimeLiveEvent::ContinuityLost,
        RuntimeLiveEvent::SourceClosed,
    ] {
        let mut stream = f.stream(LIVE).await;
        baseline(&f, &mut stream).await;
        let code = if matches!(signal, RuntimeLiveEvent::ContinuityLost) {
            "SUBSCRIPTION_CONTINUITY_LOST"
        } else {
            "SUBSCRIPTION_SOURCE_CLOSED"
        };
        f.source.tx.send(signal).unwrap();
        let response = next(&mut stream).await;
        assert_eq!(response.data, graphql_orm::async_graphql::Value::Null);
        assert_eq!(response.errors.len(), 1);
        assert_eq!(
            response.errors[0]
                .extensions
                .as_ref()
                .unwrap()
                .get("code")
                .unwrap()
                .to_string(),
            format!("\"{code}\"")
        );
        assert!(stream.next().await.is_none());
    }
    let mut stream = f.stream(LIVE).await;
    baseline(&f, &mut stream).await;
    let mut definition = definition();
    definition.collections[0].description = Some("new revision".into());
    let revised = definition.validate().unwrap();
    let collection = revised
        .resolve_collection(&CollectionId::new("records").unwrap())
        .unwrap();
    let field = revised.resolve_field(&collection, &fid("id")).unwrap();
    let key = revised
        .runtime_key(
            &collection,
            &[(field, RuntimeValue::String("visible".into()))],
        )
        .unwrap();
    f.source
        .tx
        .send(RuntimeLiveEvent::Changed {
            key,
            action: RuntimeMutationAction::Update,
            preimage: None,
        })
        .unwrap();
    let response = next(&mut stream).await;
    assert_eq!(response.data, graphql_orm::async_graphql::Value::Null);
    assert_eq!(
        response.errors[0]
            .extensions
            .as_ref()
            .unwrap()
            .get("code")
            .unwrap()
            .to_string(),
        "\"SUBSCRIPTION_CONTINUITY_LOST\""
    );
    assert!(stream.next().await.is_none());
}
#[test]
fn declarations_validate_roundtrip_and_reuse_existing_physical_column() {
    let enabled = definition().validate().unwrap();
    let mut plain = definition();
    plain.collections[0].node_identity = None;
    plain.collections[0].live_subscriptions = None;
    let serialized = serde_json::to_string(&plain).unwrap();
    assert!(!serialized.contains("node_identity"));
    assert!(!serialized.contains("live_subscriptions"));
    let plain = plain.validate().unwrap();
    assert_eq!(
        plain.canonical_bytes(),
        serde_json::from_str::<RuntimeSchema>(&serialized)
            .unwrap()
            .validate()
            .unwrap()
            .canonical_bytes()
    );
    assert_ne!(plain.fingerprint(), enabled.fingerprint());
    assert_ne!(
        plain.structural_fingerprint(),
        enabled.structural_fingerprint()
    );
    assert_eq!(
        plain
            .physical_schema::<Backend>(Default::default())
            .unwrap()
            .stable_hash(),
        enabled
            .physical_schema::<Backend>(Default::default())
            .unwrap()
            .stable_hash()
    );
    assert_eq!(
        enabled.fingerprint(),
        serde_json::from_str::<RuntimeSchema>(&serde_json::to_string(enabled.schema()).unwrap())
            .unwrap()
            .validate()
            .unwrap()
            .fingerprint()
    );
    let mut bad = definition();
    bad.collections[0].node_identity = None;
    assert!(bad.validate().is_err());
    let mut bad = definition();
    bad.collections[0]
        .live_subscriptions
        .as_mut()
        .unwrap()
        .subscription_key_limit = 0;
    assert!(bad.validate().is_err());
    let mut bad = definition();
    bad.collections[0]
        .node_identity
        .as_mut()
        .unwrap()
        .version_field = fid("title");
    assert!(bad.validate().is_err());
}

struct Write(RuntimeProjection);
impl RuntimeWriteAuthority<Backend> for Write {
    fn authorize_intent<'a>(
        &'a self,
        _: RuntimeWriteIntent<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeWriteGrant, RuntimeMutationError>> {
        Box::pin(async move { RuntimeWriteGrant::new(self.0.clone(), None) })
    }
    fn authorize_preimage<'a>(
        &'a self,
        _: RuntimePreimageCheck<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<(), RuntimeMutationError>> {
        Box::pin(async { Ok(()) })
    }
    fn authorize_result<'a>(
        &'a self,
        check: RuntimeResultCheck<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeReturnGrant, RuntimeMutationError>> {
        Box::pin(async move { Ok(RuntimeReturnGrant::new(check.intent.returning().cloned())) })
    }
}
impl Fixture {
    async fn write(
        &self,
        key: &str,
        title: &str,
        expected: Option<i64>,
        rollback: bool,
    ) -> Result<RuntimeMutationEffect, TransactionError> {
        let projection = self
            .schema
            .resolve_projection(
                &self.collection(),
                &[
                    self.field("id"),
                    self.field("title"),
                    self.field("tenant"),
                    self.field("rowVersion"),
                ],
            )
            .unwrap();
        let expected = expected.map(|version| {
            self.schema
                .runtime_compare(
                    &self.collection(),
                    &self.field("rowVersion"),
                    RuntimeScalarOperator::Eq,
                    RuntimeValue::Integer(version),
                    Default::default(),
                )
                .unwrap()
        });
        let key = self.key(key);
        let request = self
            .schema
            .runtime_update_request(
                key.clone(),
                &[(self.field("title"), RuntimeValue::String(title.into()))],
                expected,
                Some(projection.clone()),
                Default::default(),
            )
            .unwrap();
        let source = self.source.clone();
        let environment = self.environment.clone();
        self.db
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let effect = tx
                        .mutate_runtime(&environment, &request, &Write(projection))
                        .await
                        .map_err(graphql_orm::graphql::errors::OrmPublicError::from)?;
                    let event = RuntimeLiveEvent::from_effect(environment.schema(), &effect)
                        .map_err(graphql_orm::graphql::errors::OrmPublicError::from)?;
                    tx.defer(move |_| async move {
                        let _ = source.tx.send(event);
                        Ok::<(), std::convert::Infallible>(())
                    });
                    if rollback {
                        return Err(graphql_orm::graphql::errors::OrmPublicError::forbidden());
                    }
                    Ok(effect)
                })
            })
            .await
    }
}
#[tokio::test]
async fn committed_writes_advance_versions_cas_refuses_and_rollback_never_delivers() {
    let f = Fixture::new().await;
    let mut stream = f.stream(LIVE).await;
    baseline(&f, &mut stream).await;
    assert!(
        f.write("visible", "rolled back", Some(1), true)
            .await
            .is_err()
    );
    no_delivery(&mut stream).await;
    let row = f.image("visible", &["id", "rowVersion"]).await;
    assert_eq!(
        row.value(&f.field("rowVersion")).unwrap(),
        &RuntimeValue::Integer(1)
    );
    let effect = f
        .write("visible", "committed", Some(1), false)
        .await
        .unwrap();
    assert_eq!(
        effect.staged().value(&f.field("rowVersion")).unwrap(),
        &RuntimeValue::Integer(2)
    );
    let response = next(&mut stream).await;
    assert!(response.errors.is_empty(), "{response:?}");
    let value = response.data.into_json().unwrap();
    assert_eq!(value["recordChanged"]["record"]["title"], "committed");
    assert_eq!(value["recordChanged"]["record"]["RecordVersion"], 2);
    assert!(f.write("visible", "stale", Some(1), false).await.is_err());
    no_delivery(&mut stream).await;
    let effect = f
        .write("visible", "committed", Some(2), false)
        .await
        .unwrap();
    assert_eq!(
        effect.staged().value(&f.field("rowVersion")).unwrap(),
        &RuntimeValue::Integer(3)
    );
    assert!(next(&mut stream).await.errors.is_empty());
    let error = f
        .schema
        .runtime_update_request(
            f.key("visible"),
            &[(f.field("rowVersion"), RuntimeValue::Integer(100))],
            None,
            None,
            Default::default(),
        )
        .err()
        .unwrap();
    assert_eq!(error.code(), RuntimeMutationErrorCode::FieldDenied);
    graphql_orm::sqlx::query(
        "UPDATE runtime_live_records SET rowversion=9223372036854775807 WHERE id='visible'",
    )
    .execute(f.db.pool())
    .await
    .unwrap();
    assert!(f.write("visible", "overflow", None, false).await.is_err());
    no_delivery(&mut stream).await;
    let unchanged = f.image("visible", &["id", "title", "rowVersion"]).await;
    assert_eq!(
        unchanged.value(&f.field("title")).unwrap(),
        &RuntimeValue::String("committed".into())
    );
    assert_eq!(
        unchanged.value(&f.field("rowVersion")).unwrap(),
        &RuntimeValue::Integer(i64::MAX)
    );
}
#[tokio::test]
async fn committed_collection_events_keep_key_order() {
    let f = Fixture::new().await;
    let mut stream = f.stream("subscription { recordChanged(keys:[\"visible\",\"other\"]) { id record { id title } } }").await;
    baseline(&f, &mut stream).await;
    f.write("visible", "first", Some(1), false).await.unwrap();
    f.write("other", "second", Some(1), false).await.unwrap();
    for expected in ["visible", "other"] {
        let response = next(&mut stream).await;
        assert!(response.errors.is_empty(), "{response:?}");
        assert_eq!(
            response.data.into_json().unwrap()["recordChanged"]["id"],
            expected
        );
    }
    no_delivery(&mut stream).await;
}

#[tokio::test]
async fn composite_identity_and_same_id_in_another_tenant_stay_bound_to_the_complete_key() {
    let mut definition = definition();
    definition.collections[0].primary_key = vec![fid("tenant"), fid("id")];
    let f = Fixture::with_definition(definition).await;
    graphql_orm::sqlx::query(
        "INSERT INTO runtime_live_records (id,title,tenant) VALUES ('visible','foreign','south')",
    )
    .execute(f.db.pool())
    .await
    .unwrap();
    let mut stream = f.stream("subscription { recordChanged(keys:[{tenant:\"north\",id:\"visible\"}]) { id key { tenant id } record { id title } } }").await;
    baseline(&f, &mut stream).await;
    let foreign = f
        .schema
        .runtime_key(
            &f.collection(),
            &[
                (f.field("tenant"), RuntimeValue::String("south".into())),
                (f.field("id"), RuntimeValue::String("visible".into())),
            ],
        )
        .unwrap();
    f.source
        .tx
        .send(RuntimeLiveEvent::Changed {
            key: foreign,
            action: RuntimeMutationAction::Update,
            preimage: None,
        })
        .unwrap();
    no_delivery(&mut stream).await;
    for (tenant, visible) in [("north", true), ("south", false)] {
        let id = graphql_orm::graphql::federation::encode_node_id(
            "Record",
            vec![tenant.into(), "visible".into()],
        );
        let response = f
            .api
            .execute(f.request(&format!(
                "{{ node(NodeId:{}) {{ NodeId ... on Record {{ id }} }} }}",
                serde_json::to_string(&id.0).unwrap()
            )))
            .await;
        assert!(response.errors.is_empty(), "{response:?}");
        assert_eq!(
            !response.data.into_json().unwrap()["node"].is_null(),
            visible
        );
    }
}

#[tokio::test]
async fn graphql_transport_ws_uses_next_error_then_complete_without_private_extensions() {
    use graphql_orm::async_graphql::{
        Data,
        http::{WebSocket, WebSocketProtocols as Protocols},
    };
    let f = Fixture::new().await;
    for (signal, code) in [
        (
            RuntimeLiveEvent::ContinuityLost,
            "SUBSCRIPTION_CONTINUITY_LOST",
        ),
        (RuntimeLiveEvent::SourceClosed, "SUBSCRIPTION_SOURCE_CLOSED"),
    ] {
        let (input, receiver) = graphql_orm::futures::channel::mpsc::unbounded::<Vec<u8>>();
        let fingerprint = f.schema.fingerprint();
        let authority = f.authority.clone();
        let mut socket = WebSocket::new(f.api.clone(), receiver, Protocols::GraphQLWS)
            .on_connection_init(move |_| async move {
                let mut data = Data::default();
                data.insert(RuntimeGraphqlRequest::<Backend>::new(
                    fingerprint,
                    authority,
                    None,
                    Default::default(),
                ));
                Ok(data)
            });
        input
            .unbounded_send(
                serde_json::to_vec(&serde_json::json!({"type":"connection_init"})).unwrap(),
            )
            .unwrap();
        let ack: serde_json::Value =
            serde_json::from_str(&socket.next().await.unwrap().unwrap_text()).unwrap();
        assert_eq!(ack["type"], "connection_ack");
        input
            .unbounded_send(
                serde_json::to_vec(
                    &serde_json::json!({"type":"subscribe","id":"wire","payload":{"query": LIVE}}),
                )
                .unwrap(),
            )
            .unwrap();
        tokio::select! { value = socket.next() => panic!("unexpected registration frame: {value:?}"), () = f.source.subscribed.notified() => {} }
        f.emit("visible", RuntimeMutationAction::Update, None);
        let baseline: serde_json::Value =
            serde_json::from_str(&socket.next().await.unwrap().unwrap_text()).unwrap();
        assert_eq!(baseline["type"], "next");
        assert_eq!(
            baseline["payload"]["data"]["recordChanged"]["record"]["id"],
            "visible"
        );
        f.source.tx.send(signal).unwrap();
        let terminal: serde_json::Value =
            serde_json::from_str(&socket.next().await.unwrap().unwrap_text()).unwrap();
        assert_eq!(terminal["type"], "next");
        assert!(terminal["payload"]["data"].is_null());
        assert_eq!(terminal["payload"]["errors"].as_array().unwrap().len(), 1);
        assert_eq!(
            terminal["payload"]["errors"][0]["extensions"],
            serde_json::json!({"code":code})
        );
        let complete: serde_json::Value =
            serde_json::from_str(&socket.next().await.unwrap().unwrap_text()).unwrap();
        assert_eq!(complete, serde_json::json!({"id":"wire","type":"complete"}));
    }
}

#[derive(
    graphql_orm::GraphQLEntity,
    graphql_orm::GraphQLOperations,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql_entity(
    table = "runtime_live_derived",
    plural = "DerivedRecords",
    node_id,
    auth = "none"
)]
struct Record {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    title: String,
    tenant: String,
    #[graphql_orm(version = "managed", default = "0")]
    row_version: i64,
}
struct ParityQuery;
#[graphql_orm::async_graphql::Object]
impl ParityQuery {
    async fn record(&self) -> Record {
        Record {
            id: "visible".into(),
            title: "initial".into(),
            tenant: "north".into(),
            row_version: 1,
        }
    }
}
#[tokio::test]
#[cfg(not(any(
    feature = "field-case-pascal",
    feature = "field-case-snake",
    feature = "resolver-case-pascal",
    feature = "resolver-case-snake",
    feature = "argument-case-pascal",
    feature = "argument-case-snake"
)))]
async fn derived_and_runtime_share_identity_version_and_changed_client_shapes() {
    let f = Fixture::new().await;
    let derived = graphql_orm::async_graphql::Schema::build(
        ParityQuery,
        graphql_orm::async_graphql::EmptyMutation,
        RecordSubscriptions,
    )
    .data(f.db.clone())
    .finish();
    let response = derived
        .execute("{ record { NodeId RecordVersion id } }")
        .await;
    assert!(response.errors.is_empty(), "{response:?}");
    let expected = response.data.into_json().unwrap()["record"].clone();
    let response = f.api.execute(f.request("{ records(where:{id:{eq:\"visible\"}}) { edges { node { NodeId RecordVersion id } } } }")).await;
    assert!(response.errors.is_empty(), "{response:?}");
    assert_eq!(
        response.data.into_json().unwrap()["records"]["edges"][0]["node"],
        expected
    );
    for field in [
        "NodeId: ID!",
        "RecordVersion: Int!",
        "id: String!",
        "action: ChangeAction!",
        "changeKind: ChangeKind!",
        "record: Record",
        "keys: [String!]",
    ] {
        assert!(f.api.sdl().contains(field), "runtime lacks {field}");
        assert!(derived.sdl().contains(field), "derived lacks {field}");
    }
}

#[tokio::test]
async fn node_aliases_and_lists_share_global_admission_and_preserve_missing_nulls() {
    let f = Fixture::new().await;
    let a = graphql_orm::graphql::federation::encode_node_id("Record", vec!["visible".into()]);
    let b = graphql_orm::graphql::federation::encode_node_id("Record", vec!["other".into()]);
    let query = format!(
        "{{ a:node(NodeId:{}) {{ ...on Record {{ id }} }} b:node(NodeId:{}) {{ ...on Record {{ id }} }} records {{ totalCount }} }}",
        serde_json::to_string(&a.0).unwrap(),
        serde_json::to_string(&b.0).unwrap()
    );
    let response = f.api.execute(f.request(&query)).await;
    assert!(response.errors.is_empty(), "{response:?}");
    let data = response.data.into_json().unwrap();
    assert_eq!(data["a"]["id"], "visible");
    assert_eq!(data["b"]["id"], "other");
    assert_eq!(data["records"]["totalCount"], "2");
    f.authority.purposes.lock().unwrap().clear();
    let limits = RuntimeGraphqlLimits {
        max_statements: 1,
        ..Default::default()
    };
    let response = f
        .api
        .execute(
            Request::new(query).data(RuntimeGraphqlRequest::<Backend>::new(
                f.schema.fingerprint(),
                f.authority.clone(),
                None,
                limits,
            )),
        )
        .await;
    assert_eq!(response.data, graphql_orm::async_graphql::Value::Null);
    assert!(!response.errors.is_empty());
    assert!(
        f.authority.purposes.lock().unwrap().is_empty(),
        "admission precedes grants and I/O"
    );
}
#[tokio::test]
async fn repeated_event_field_aliases_are_bounded_before_any_payload_is_released() {
    let f = Fixture::new().await;
    let fields = (0..100)
        .map(|index| format!("a{index}:title"))
        .collect::<Vec<_>>()
        .join(" ");
    let query = format!(
        "subscription {{ recordChanged(keys:[\"visible\"]) {{ id record {{ id {fields} }} }} }}"
    );
    let limits = RuntimeGraphqlLimits {
        max_response_bytes: 10_000,
        ..Default::default()
    };
    let mut stream = f
        .api
        .execute_stream(
            Request::new(query).data(RuntimeGraphqlRequest::<Backend>::new(
                f.schema.fingerprint(),
                f.authority.clone(),
                None,
                limits,
            )),
        )
        .boxed();
    tokio::select! { response = stream.next() => panic!("unexpected registration frame: {response:?}"), () = f.source.subscribed.notified() => {} }
    f.emit("visible", RuntimeMutationAction::Update, None);
    let response = next(&mut stream).await;
    assert!(response.errors.is_empty(), "{response:?}");
    assert_eq!(
        response.data.into_json().unwrap()["recordChanged"]["record"]["a0"],
        "initial"
    );
    f.write("visible", &"large".repeat(100), Some(1), false)
        .await
        .unwrap();
    let response = next(&mut stream).await;
    assert_eq!(response.data, graphql_orm::async_graphql::Value::Null);
    assert_eq!(
        response.errors[0]
            .extensions
            .as_ref()
            .unwrap()
            .get("code")
            .unwrap()
            .to_string(),
        "\"SUBSCRIPTION_CONTINUITY_LOST\""
    );
    assert!(stream.next().await.is_none());
}
#[tokio::test]
async fn missing_subscribed_key_stays_quiet_after_positive_baseline() {
    let f = Fixture::new().await;
    let mut stream = f.stream("subscription { recordChanged(keys:[\"visible\",\"missing\"]) { id record { id title } } }").await;
    baseline(&f, &mut stream).await;
    f.emit("missing", RuntimeMutationAction::Update, None);
    no_delivery(&mut stream).await;
}

#[tokio::test]
async fn a_delete_event_reloads_a_recreated_row_instead_of_releasing_an_old_image() {
    let f = Fixture::new().await;
    let mut stream = f.stream(LIVE).await;
    baseline(&f, &mut stream).await;
    let image = f
        .image("visible", &["id", "title", "tenant", "rowVersion"])
        .await;
    graphql_orm::sqlx::query("DELETE FROM runtime_live_records WHERE id='visible'")
        .execute(f.db.pool())
        .await
        .unwrap();
    graphql_orm::sqlx::query("INSERT INTO runtime_live_records (id,title,tenant) VALUES ('visible','replacement','north')").execute(f.db.pool()).await.unwrap();
    f.authority.allow_delete.store(true, Ordering::SeqCst);
    f.emit("visible", RuntimeMutationAction::Delete, Some(image));
    let response = next(&mut stream).await;
    assert!(response.errors.is_empty(), "{response:?}");
    assert_eq!(
        response.data.into_json().unwrap()["recordChanged"]["record"]["title"],
        "replacement"
    );
}
#[tokio::test]
async fn native_profile_keeps_current_reads_but_never_releases_deleted_images() {
    let mut definition = definition();
    definition.collections[0]
        .live_subscriptions
        .as_mut()
        .unwrap()
        .deleted_images = false;
    let mut f = Fixture::with_definition(definition).await;
    let module = RuntimeGraphqlModule::compile(
        f.schema.clone(),
        RuntimeGraphqlOptions::default()
            .with_cursor_profile(RuntimeCursorProfile::Unprotected)
            .with_live_source(f.source.clone(), RuntimeLiveProfile::NativeRls),
    )
    .unwrap();
    f.api = RuntimeGraphqlComposer::new(f.db.clone(), "Query", Default::default())
        .unwrap()
        .install(module)
        .unwrap()
        .finish()
        .unwrap();
    let mut stream = f.stream(LIVE).await;
    baseline(&f, &mut stream).await;
    let image = f
        .image("visible", &["id", "title", "tenant", "rowVersion"])
        .await;
    graphql_orm::sqlx::query(
        "UPDATE runtime_live_records SET title='queued',rowversion=rowversion+1 WHERE id='visible'",
    )
    .execute(f.db.pool())
    .await
    .unwrap();
    let deleted = f
        .image("visible", &["id", "title", "tenant", "rowVersion"])
        .await;
    f.authority.allow_delete.store(true, Ordering::SeqCst);
    graphql_orm::sqlx::query("DELETE FROM runtime_live_records WHERE id='visible'")
        .execute(f.db.pool())
        .await
        .unwrap();
    f.emit("visible", RuntimeMutationAction::Update, Some(image));
    no_delivery(&mut stream).await;
    f.emit("visible", RuntimeMutationAction::Delete, Some(deleted));
    no_delivery(&mut stream).await;
}

#[tokio::test]
async fn numeric_uuid_and_boolean_node_keys_use_the_shared_canonical_wire_values() {
    for (kind, key, raw) in [
        (
            RuntimeValueKind::Integer,
            serde_json::json!(1),
            serde_json::json!("1"),
        ),
        (
            RuntimeValueKind::Uuid,
            serde_json::json!("11111111-1111-4111-8111-111111111111"),
            serde_json::json!("11111111-1111-4111-8111-111111111111"),
        ),
        (
            RuntimeValueKind::Boolean,
            serde_json::json!(true),
            serde_json::json!(true),
        ),
    ] {
        let mut definition = definition();
        definition.collections[0].fields[0].value_kind = kind;
        let f = Fixture::with_definition(definition).await;
        let id = graphql_orm::graphql::federation::encode_node_id("Record", vec![key.clone()]);
        let response = f
            .api
            .execute(f.request(&format!(
                "{{ node(NodeId:{}) {{ NodeId ...on Record {{ id }} }} }}",
                serde_json::to_string(&id.0).unwrap()
            )))
            .await;
        assert!(response.errors.is_empty(), "{response:?}");
        let data = response.data.into_json().unwrap();
        assert_eq!(data["node"]["id"], raw);
        assert_eq!(data["node"]["NodeId"], id.0);
        let mut stream = f
            .stream(&format!(
                "subscription {{ recordChanged(keys:[{key}]) {{ id record {{ id NodeId }} }} }}"
            ))
            .await;
        let value = match kind {
            RuntimeValueKind::Integer => RuntimeValue::Integer(1),
            RuntimeValueKind::Uuid => RuntimeValue::Uuid(
                graphql_orm::uuid::Uuid::parse_str(key.as_str().unwrap()).unwrap(),
            ),
            RuntimeValueKind::Boolean => RuntimeValue::Boolean(true),
            _ => unreachable!(),
        };
        f.source
            .tx
            .send(RuntimeLiveEvent::Changed {
                key: f
                    .schema
                    .runtime_key(&f.collection(), &[(f.field("id"), value)])
                    .unwrap(),
                action: RuntimeMutationAction::Update,
                preimage: None,
            })
            .unwrap();
        let response = next(&mut stream).await;
        assert!(response.errors.is_empty(), "{response:?}");
        let data = response.data.into_json().unwrap();
        assert_eq!(data["recordChanged"]["id"], key);
        assert_eq!(data["recordChanged"]["record"]["id"], raw);
    }
}
struct Producer {
    schema: Arc<ValidatedRuntimeSchema>,
    source: Arc<Source>,
}
impl RuntimeMutationHook<Backend> for Producer {
    fn before_commit<'a>(
        &'a self,
        tx: &'a mut MutationContext<'_, Backend>,
        effect: &'a RuntimeMutationEffect,
    ) -> BoxFuture<'a, Result<(), RuntimeMutationError>> {
        Box::pin(async move {
            let event = RuntimeLiveEvent::from_effect(&self.schema, effect)?;
            let source = self.source.clone();
            tx.defer(move |_| async move {
                let _ = source.tx.send(event);
                Ok::<(), std::convert::Infallible>(())
            });
            Ok(())
        })
    }
}
#[tokio::test]
async fn graphql_mutations_serve_identity_and_managed_versions_and_publish_after_commit() {
    let f = Fixture::new().await;
    let match_profile = if Backend::DIALECT == DatabaseBackend::Sqlite {
        RuntimeMatchProfile::SqliteBinaryCanonical
    } else {
        RuntimeMatchProfile::PostgresCUtf8
    };
    let module = RuntimeGraphqlModule::compile(
        f.schema.clone(),
        RuntimeGraphqlOptions::default()
            .with_cursor_profile(RuntimeCursorProfile::Unprotected)
            .with_mutations(Default::default())
            .with_live_source(
                f.source.clone(),
                RuntimeLiveProfile::ApplicationPolicy { match_profile },
            ),
    )
    .unwrap();
    let api = RuntimeGraphqlComposer::new(f.db.clone(), "Query", Default::default())
        .unwrap()
        .mutation_root("Mutation")
        .unwrap()
        .install(module)
        .unwrap()
        .finish()
        .unwrap();
    assert!(
        !api.sdl()
            .split("input CreateRecordInput {")
            .nth(1)
            .unwrap()
            .split('}')
            .next()
            .unwrap()
            .contains("rowVersion")
    );
    let projection = f
        .schema
        .resolve_projection(
            &f.collection(),
            &[
                f.field("id"),
                f.field("tenant"),
                f.field("title"),
                f.field("rowVersion"),
            ],
        )
        .unwrap();
    let request = |query| {
        Request::new(query)
            .variables(graphql_orm::async_graphql::Variables::from_json(
                serde_json::json!({"schema":f.schema.fingerprint().to_string()}),
            ))
            .data(
                RuntimeGraphqlRequest::<Backend>::new(
                    f.schema.fingerprint(),
                    f.authority.clone(),
                    None,
                    Default::default(),
                )
                .with_mutation_environment(f.environment.clone())
                .with_write_authority(Arc::new(Write(projection.clone())))
                .with_mutation_hook(Arc::new(Producer {
                    schema: f.schema.clone(),
                    source: f.source.clone(),
                })),
            )
    };
    let mut stream = f.stream("subscription { recordChanged(keys:[\"created\"]) { action id record { id title NodeId RecordVersion } } }").await;
    let (response, event) = tokio::join!(api.execute(request("mutation($schema:String!){createRecord(expectedSchema:$schema,input:{id:\"created\",title:\"fresh\",tenant:\"north\"}){record{id tenant NodeId RecordVersion}}}")), next(&mut stream));
    assert!(response.errors.is_empty(), "{response:?}");
    let data = response.data.into_json().unwrap();
    assert_eq!(data["createRecord"]["record"]["RecordVersion"], 1);
    assert_eq!(
        data["createRecord"]["record"]["NodeId"],
        graphql_orm::graphql::federation::encode_node_id("Record", vec!["created".into()]).0
    );
    assert!(event.errors.is_empty(), "{event:?}");
    assert_eq!(
        event.data.into_json().unwrap()["recordChanged"]["record"]["title"],
        "fresh"
    );
    let update = "mutation($schema:String!){updateRecord(expectedSchema:$schema,key:{id:\"created\"},input:{title:\"updated\"},expected:{rowVersion:{eq:\"1\"}}){record{id tenant NodeId RecordVersion}}}";
    let response = api.execute(request(update)).await;
    assert!(response.errors.is_empty(), "{response:?}");
    assert_eq!(
        response.data.into_json().unwrap()["updateRecord"]["record"]["RecordVersion"],
        2
    );
    assert!(next(&mut stream).await.errors.is_empty());
    assert!(!api.execute(request(update)).await.errors.is_empty());
    no_delivery(&mut stream).await;
}

#[tokio::test]
async fn revocation_and_row_restriction_changes_after_the_first_grant_never_release_prepared_data()
{
    let f = Fixture::new().await;
    let mut stream = f.stream(LIVE).await;
    baseline(&f, &mut stream).await;
    f.authority.revoke_after_grant.store(true, Ordering::SeqCst);
    f.emit("visible", RuntimeMutationAction::Update, None);
    assert!(
        timeout(Duration::from_secs(10), stream.next())
            .await
            .unwrap()
            .is_none()
    );
    f.authority.revoked.store(false, Ordering::SeqCst);
    let mut stream = f.stream(LIVE).await;
    baseline(&f, &mut stream).await;
    f.authority
        .move_scope_after_grant
        .store(true, Ordering::SeqCst);
    f.emit("visible", RuntimeMutationAction::Update, None);
    let response = next(&mut stream).await;
    assert_eq!(response.data, graphql_orm::async_graphql::Value::Null);
    assert_eq!(
        response.errors[0]
            .extensions
            .as_ref()
            .unwrap()
            .get("code")
            .unwrap()
            .to_string(),
        "\"SUBSCRIPTION_CONTINUITY_LOST\""
    );
    assert!(stream.next().await.is_none());
}
#[tokio::test]
async fn deleted_image_permission_is_rechecked_at_the_release_fence() {
    let f = Fixture::new().await;
    let mut stream = f.stream(LIVE).await;
    baseline(&f, &mut stream).await;
    let image = f
        .image("visible", &["id", "title", "tenant", "rowVersion"])
        .await;
    graphql_orm::sqlx::query("DELETE FROM runtime_live_records WHERE id='visible'")
        .execute(f.db.pool())
        .await
        .unwrap();
    f.authority.allow_delete.store(true, Ordering::SeqCst);
    f.authority
        .revoke_image_after_check
        .store(true, Ordering::SeqCst);
    f.emit("visible", RuntimeMutationAction::Delete, Some(image));
    no_delivery(&mut stream).await;
}

#[derive(Default)]
struct CursorProbe(std::sync::atomic::AtomicUsize);
impl RuntimeCursorProtector for CursorProbe {
    fn seal<'a>(
        &'a self,
        _: &'a RuntimeCursorContext,
        _: &'a str,
        limits: RuntimeCursorProtectionLimits,
    ) -> BoxFuture<'a, Result<RuntimeSealedCursor, RuntimeCursorProtectionError>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            // A test sentinel, never a production encryption implementation.
            RuntimeSealedCursor::new("probe", vec![17; 32], limits)
        })
    }
    fn open<'a>(
        &'a self,
        _: &'a RuntimeCursorContext,
        _: &'a str,
        _: &'a [u8],
        _: RuntimeCursorProtectionLimits,
    ) -> BoxFuture<'a, Result<String, RuntimeCursorProtectionError>> {
        Box::pin(async { Err(RuntimeCursorProtectionError::InvalidCursor) })
    }
}
#[tokio::test]
async fn nested_point_and_live_cursors_use_final_composer_protection() {
    let mut definition = definition();
    definition.collections[0]
        .live_subscriptions
        .as_mut()
        .unwrap()
        .deleted_images = false;
    definition.collections[0].relations.push(RuntimeRelation {
        id: RelationId::new("self").unwrap(),
        api_name: "related".into(),
        target: CollectionId::new("records").unwrap(),
        key_pairs: vec![RelationKeyPair {
            source: fid("id"),
            target: fid("id"),
        }],
        cardinality: RelationCardinality::Many,
        enforce_foreign_key: false,
        on_delete: None,
        join: None,
        description: None,
        classification: None,
        export: None,
    });
    let mut f = Fixture::with_definition(definition).await;
    let protector = Arc::new(CursorProbe::default());
    let initial_protector = Arc::new(CursorProbe::default());
    let module = RuntimeGraphqlModule::compile(
        f.schema.clone(),
        RuntimeGraphqlOptions::default()
            .with_relation_filters()
            .with_live_source(f.source.clone(), RuntimeLiveProfile::NativeRls),
    )
    .unwrap();
    f.api = RuntimeGraphqlComposer::new(f.db.clone(), "Query", Default::default())
        .unwrap()
        .cursor_protection(
            initial_protector.clone(),
            RuntimeCursorAudience::new("runtime-live-initial").unwrap(),
            Default::default(),
        )
        .unwrap()
        .install(module)
        .unwrap()
        .cursor_protection(
            protector.clone(),
            RuntimeCursorAudience::new("runtime-live-test").unwrap(),
            Default::default(),
        )
        .unwrap()
        .finish()
        .unwrap();
    let id = graphql_orm::graphql::federation::encode_node_id(
        "Record",
        vec![serde_json::json!("visible")],
    )
    .0;
    let simple = f
        .api
        .execute(f.request(&format!("{{ node(NodeId: \"{id}\") {{ NodeId }} }}")))
        .await;
    assert!(simple.errors.is_empty(), "{simple:?}");
    assert_ne!(simple.data, graphql_orm::async_graphql::Value::Null);
    assert_eq!(
        protector.0.load(Ordering::SeqCst),
        0,
        "point roots have no page cursor"
    );
    let scoped = |query: String| {
        Request::new(query).data(
            RuntimeGraphqlRequest::<Backend>::new(
                f.schema.fingerprint(),
                f.authority.clone(),
                None,
                Default::default(),
            )
            .with_cursor_scope("trusted-test"),
        )
    };
    let selection = r#"related(where:{related:{some:{id:{eq:"visible"}}}},first:1) { edges { cursor node { id } } }"#;
    let nested = f
        .api
        .execute(scoped(format!(
            "{{ node(NodeId: \"{id}\") {{ ... on Record {{ {selection} }} }} }}"
        )))
        .await;
    assert!(nested.errors.is_empty(), "{nested:?}");
    let after_node = protector.0.load(Ordering::SeqCst);
    assert!(
        after_node > 0,
        "nested node connections must seal their cursors"
    );
    let query = format!(
        "subscription {{ recordChanged(keys:[\"visible\"]) {{ id record {{ {selection} }} }} }}"
    );
    let mut stream = f.api.execute_stream(scoped(query.clone())).boxed();
    tokio::select! { response = stream.next() => panic!("unexpected registration frame {response:?}"), () = f.source.subscribed.notified() => {} }
    f.emit("visible", RuntimeMutationAction::Update, None);
    let response = next(&mut stream).await;
    assert!(response.errors.is_empty(), "{response:?}");
    assert_eq!(
        response.data.into_json().unwrap()["recordChanged"]["id"],
        "visible"
    );
    assert!(
        protector.0.load(Ordering::SeqCst) > after_node,
        "live nested cursors must use final composer protection"
    );
    let refused = f
        .api
        .execute_stream(f.request(&query))
        .next()
        .await
        .unwrap();
    assert!(!refused.errors.is_empty());
    assert_eq!(
        refused.data,
        graphql_orm::async_graphql::Value::Null,
        "a missing trusted cursor scope cannot deliver data"
    );
    assert_eq!(
        initial_protector.0.load(Ordering::SeqCst),
        0,
        "replaced protection must never serve a cursor"
    );
    let checks = f.authority.operation_checks.lock().unwrap();
    for origin in [
        RuntimeReadPurpose::NodeLookup,
        RuntimeReadPurpose::SubscriptionRegistration,
        RuntimeReadPurpose::SubscriptionDelivery,
    ] {
        assert!(
            checks.iter().any(|(purpose, operation)| matches!(
                purpose,
                RuntimeReadPurpose::RelationFilter(_)
            ) && *operation == origin),
            "relation-filter checks lost their originating purpose: {origin:?}"
        );
    }
}

#[tokio::test]
async fn opting_into_existing_version_storage_is_an_owned_migration_noop() {
    let f = Fixture::new().await;
    let mut legacy = f.schema.schema().clone();
    legacy.collections[0].node_identity = None;
    legacy.collections[0].live_subscriptions = None;
    let json = serde_json::to_string(&legacy).unwrap();
    let legacy = Arc::new(legacy.validate().unwrap());
    let ownership = ManagedTableSet::new(["runtime_live_records".into()]).unwrap();
    for (label, schema) in [("existing-column", &legacy), ("node-opt-in", &f.schema)] {
        let target = schema
            .physical_schema::<Backend>(Default::default())
            .unwrap();
        let plan =
            f.db.schema()
                .plan_owned_migration(
                    label,
                    "existing version column",
                    &target,
                    &ownership,
                    PlanOptions::strict(),
                )
                .await
                .unwrap();
        assert!(
            plan.steps().is_empty(),
            "opt-in must not add or rewrite physical columns: {plan:?}"
        );
    }
    let from_legacy_json = Arc::new(
        serde_json::from_str::<RuntimeSchema>(&json)
            .unwrap()
            .validate()
            .unwrap(),
    );
    assert_eq!(legacy.canonical_bytes(), from_legacy_json.canonical_bytes());
    let compose = |schema, options| {
        RuntimeGraphqlComposer::new(f.db.clone(), "Query", Default::default())
            .unwrap()
            .install(RuntimeGraphqlModule::compile(schema, options).unwrap())
            .unwrap()
            .finish_with_semantic_catalog()
            .unwrap()
    };
    let (before, before_catalog) = compose(
        legacy,
        RuntimeGraphqlOptions::default().with_cursor_profile(RuntimeCursorProfile::Unprotected),
    );
    // Merely wiring an available source cannot change a collection's opt-out artifacts.
    let (after, after_catalog) = compose(
        from_legacy_json,
        RuntimeGraphqlOptions::default()
            .with_cursor_profile(RuntimeCursorProfile::Unprotected)
            .with_live_source(f.source.clone(), RuntimeLiveProfile::NativeRls),
    );
    assert_eq!(before.sdl().as_bytes(), after.sdl().as_bytes());
    assert_eq!(
        serde_json::to_vec(&before_catalog).unwrap(),
        serde_json::to_vec(&after_catalog).unwrap()
    );
    assert!(!after.sdl().contains("interface Node"));
    assert!(!after.sdl().contains("RecordVersion"));
}

#[tokio::test]
async fn composite_numeric_live_keys_share_mutation_inputs_including_create_only() {
    let mut definition = definition();
    definition.collections[0].fields[0].value_kind = RuntimeValueKind::Integer;
    definition.collections[0].primary_key = vec![fid("tenant"), fid("id")];
    definition.collections[0]
        .live_subscriptions
        .as_mut()
        .unwrap()
        .deleted_images = false;
    let mut f = Fixture::with_definition(definition).await;
    for update_delete in [true, false] {
        let mut mutations = RuntimeGraphqlMutationOptions::default();
        mutations.update = update_delete;
        mutations.delete = update_delete;
        let module = RuntimeGraphqlModule::compile(
            f.schema.clone(),
            RuntimeGraphqlOptions::default()
                .with_cursor_profile(RuntimeCursorProfile::Unprotected)
                .with_mutations(mutations)
                .with_live_source(f.source.clone(), RuntimeLiveProfile::NativeRls),
        )
        .unwrap();
        f.api = RuntimeGraphqlComposer::new(f.db.clone(), "Query", Default::default())
            .unwrap()
            .mutation_root("Mutation")
            .unwrap()
            .install(module)
            .unwrap()
            .finish_with_semantic_catalog()
            .unwrap()
            .0;
        let query = "subscription { recordChanged(keys:[{tenant:\"north\",id:1}]) { id key { tenant id } record { id } } }";
        let mut stream = f.stream(query).await;
        let key = |id| {
            f.schema
                .runtime_key(
                    &f.collection(),
                    &[
                        (f.field("tenant"), RuntimeValue::String("north".into())),
                        (f.field("id"), RuntimeValue::Integer(id)),
                    ],
                )
                .unwrap()
        };
        f.source
            .tx
            .send(RuntimeLiveEvent::Changed {
                key: key(1),
                action: RuntimeMutationAction::Update,
                preimage: None,
            })
            .unwrap();
        let response = next(&mut stream).await;
        assert!(response.errors.is_empty(), "{response:?}");
        let data = response.data.into_json().unwrap();
        assert_eq!(data["recordChanged"]["key"]["id"], 1);
        assert_eq!(data["recordChanged"]["record"]["id"], "1");
        f.source
            .tx
            .send(RuntimeLiveEvent::Changed {
                key: key(2),
                action: RuntimeMutationAction::Update,
                preimage: None,
            })
            .unwrap();
        no_delivery(&mut stream).await;
    }
}

#[tokio::test]
async fn registration_admits_all_initial_key_reads_before_authority_or_io() {
    let f = Fixture::new().await;
    let mut stream = f.stream(LIVE).await;
    baseline(&f, &mut stream).await;
    f.authority.purposes.lock().unwrap().clear();
    let budget = RuntimeGraphqlLimits {
        max_statements: 1,
        ..Default::default()
    };
    let response = f
        .api
        .execute_stream(
            Request::new("subscription { recordChanged(keys:[\"visible\",\"other\"]) { id } }")
                .data(RuntimeGraphqlRequest::<Backend>::new(
                    f.schema.fingerprint(),
                    f.authority.clone(),
                    None,
                    budget,
                )),
        )
        .next()
        .await
        .unwrap();
    assert!(!response.errors.is_empty());
    assert_eq!(response.data, graphql_orm::async_graphql::Value::Null);
    assert!(f.authority.purposes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn broader_registration_policy_cannot_turn_a_hidden_row_into_a_removal_frame() {
    let f = Fixture::new().await;
    f.authority.registration_all.store(true, Ordering::SeqCst);
    for key in ["hidden", "missing"] {
        let mut stream = f.stream(&format!("subscription {{ recordChanged(keys:[\"visible\",\"{key}\"]) {{ action id record {{ id title }} }} }}")).await;
        baseline(&f, &mut stream).await;
        f.emit(key, RuntimeMutationAction::Update, None);
        no_delivery(&mut stream).await;
        f.authority.revoked.store(true, Ordering::SeqCst);
        f.emit(key, RuntimeMutationAction::Update, None);
        no_delivery(&mut stream).await;
        f.authority.revoked.store(false, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn queued_revision_or_close_during_registration_precedes_policy_refusal() {
    for source_closed in [false, true] {
        let f = Fixture::new().await;
        let mut working = f.stream(LIVE).await;
        baseline(&f, &mut working).await;
        f.authority.pause.store(true, Ordering::SeqCst);
        let mut stream = f.stream(LIVE).await;
        let task = tokio::spawn(async move {
            let response = next(&mut stream).await;
            (response, stream)
        });
        f.authority.entered.notified().await;
        f.authority.revoked.store(true, Ordering::SeqCst);
        let event = if source_closed {
            RuntimeLiveEvent::SourceClosed
        } else {
            let mut revision = f.schema.schema().clone();
            revision.collections[0].description = Some("new revision".into());
            let revision = revision.validate().unwrap();
            let collection = revision
                .resolve_collection(&CollectionId::new("records").unwrap())
                .unwrap();
            let id = revision.resolve_field(&collection, &fid("id")).unwrap();
            RuntimeLiveEvent::Changed {
                key: revision
                    .runtime_key(&collection, &[(id, RuntimeValue::String("visible".into()))])
                    .unwrap(),
                action: RuntimeMutationAction::Update,
                preimage: None,
            }
        };
        f.source.tx.send(event).unwrap();
        f.authority.resume.notify_one();
        let (response, mut stream) = task.await.unwrap();
        assert_eq!(response.data, graphql_orm::async_graphql::Value::Null);
        assert_eq!(response.errors.len(), 1);
        let code = if source_closed {
            "SUBSCRIPTION_SOURCE_CLOSED"
        } else {
            "SUBSCRIPTION_CONTINUITY_LOST"
        };
        assert_eq!(
            response.errors[0]
                .extensions
                .as_ref()
                .unwrap()
                .get("code")
                .unwrap(),
            &graphql_orm::async_graphql::Value::String(code.into())
        );
        assert!(
            timeout(Duration::from_secs(10), stream.next())
                .await
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn authorized_delete_queued_before_initial_visibility_is_not_lost() {
    let f = Fixture::new().await;
    let mut working = f.stream(LIVE).await;
    baseline(&f, &mut working).await;
    let image = f
        .image("visible", &["id", "title", "tenant", "rowVersion"])
        .await;
    f.authority.allow_delete.store(true, Ordering::SeqCst);
    f.authority.pause.store(true, Ordering::SeqCst);
    let mut stream = f.stream(LIVE).await;
    let task = tokio::spawn(async move { next(&mut stream).await });
    f.authority.entered.notified().await;
    graphql_orm::sqlx::query("DELETE FROM runtime_live_records WHERE id='visible'")
        .execute(f.db.pool())
        .await
        .unwrap();
    f.emit("visible", RuntimeMutationAction::Delete, Some(image));
    f.authority.resume.notify_one();
    let response = task.await.unwrap();
    assert!(response.errors.is_empty(), "{response:?}");
    let data = response.data.into_json().unwrap();
    assert_eq!(data["recordChanged"]["action"], "DELETED");
    assert_eq!(data["recordChanged"]["id"], "visible");
    assert!(data["recordChanged"]["record"].is_null());
}
