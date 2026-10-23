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

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "composite_events",
    plural = "CompositeEvents",
    repository_mutations = true
)]
struct CompositeEventRow {
    #[primary_key]
    #[sortable]
    tenant: String,
    #[primary_key]
    code: String,
    label: String,
    #[graphql(skip)]
    #[relation(target = "ReadonlyEventProjection", from = ["tenant", "code"], to = ["tenant", "code"], propagate_change = "up", emit_fk = false)]
    projection: Option<ReadonlyEventProjection>,
}
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "composite_events",
    plural = "ReadonlyEventProjections",
    schema_policy = "external_read_only"
)]
struct ReadonlyEventProjection {
    #[primary_key]
    #[sortable]
    tenant: String,
    #[primary_key]
    code: String,
    #[graphql_orm(read_policy = "label.read")]
    label: Option<String>,
}
schema_roots! { query_custom_ops: [], entities: [CompositeEventRow, ReadonlyEventProjection], generated_mutations: "none", }

#[derive(Clone)]
struct Policy {
    source: Arc<AtomicBool>,
    target: Arc<AtomicBool>,
    key: Arc<AtomicBool>,
}
impl EntityPolicy for Policy {
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
impl RowPolicy for Policy {
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(if entity == "CompositeEventRow" {
                self.source.load(Ordering::SeqCst)
            } else {
                self.target.load(Ordering::SeqCst)
            })
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
    fn can_read_repository_field<'a>(
        &'a self,
        _: Option<AccessContext<'a>>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        field: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(field != "code" || self.key.load(Ordering::SeqCst)) })
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
async fn setup() -> (Database, Policy) {
    let mut db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    sqlx::query("CREATE TABLE composite_events (tenant TEXT NOT NULL, code TEXT NOT NULL, label TEXT NOT NULL, PRIMARY KEY(tenant,code))").execute(db.pool()).await.unwrap();
    let policy = Policy {
        source: Arc::new(AtomicBool::new(true)),
        target: Arc::new(AtomicBool::new(true)),
        key: Arc::new(AtomicBool::new(true)),
    };
    db.set_entity_policy(policy.clone());
    db.set_row_policy(policy.clone());
    db.set_field_policy(policy.clone());
    (db, policy)
}
fn input(code: &str) -> CreateCompositeEventRowInput {
    CreateCompositeEventRowInput {
        tenant: "same-tenant".into(),
        code: code.into(),
        label: code.into(),
    }
}
async fn registered<S: Stream<Item = Response> + Unpin>(stream: &mut S) {
    poll_fn(|cx| match stream.poll_next_unpin(cx) {
        Poll::Pending => Poll::Ready(()),
        other => panic!("unexpected initial event {other:?}"),
    })
    .await;
}
const DIRECT: &str = "subscription { compositeEventRowChanged(key: {tenant: \"same-tenant\", code: \"one\"}) { id key {tenant code} changeKind compositeEventRow {label} } }";
const PROJECTED: &str = "subscription { readonlyEventProjectionChanged(key: {tenant: \"same-tenant\", code: \"one\"}) { id key {tenant code} sourceId changeKind readonlyEventProjection {label} } }";
#[tokio::test]
async fn complete_key_filter_and_readonly_projection_share_only_the_exact_identity() {
    let (db, _) = setup().await;
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("tester"))
        .finish();
    let mut direct = Box::pin(schema.execute_stream(DIRECT));
    let mut projection = Box::pin(schema.execute_stream(PROJECTED));
    registered(&mut direct).await;
    registered(&mut projection).await;
    CompositeEventRow::insert(&db, input("two")).await.unwrap();
    CompositeEventRow::insert(&db, input("one")).await.unwrap();
    let a = timeout(Duration::from_secs(2), direct.next())
        .await
        .unwrap()
        .unwrap();
    let b = timeout(Duration::from_secs(2), projection.next())
        .await
        .unwrap()
        .unwrap();
    assert!(a.errors.is_empty(), "{:?}", a.errors);
    assert!(b.errors.is_empty(), "{:?}", b.errors);
    let a = a.data.into_json().unwrap();
    let b = b.data.into_json().unwrap();
    assert_eq!(a["compositeEventRowChanged"]["key"]["code"], "one");
    assert_eq!(b["readonlyEventProjectionChanged"]["key"]["code"], "one");
    assert_eq!(
        a["compositeEventRowChanged"]["id"],
        b["readonlyEventProjectionChanged"]["sourceId"]
    );
    assert_eq!(
        b["readonlyEventProjectionChanged"]["changeKind"],
        "PROPAGATED"
    );
    assert!(
        timeout(Duration::from_millis(100), direct.next())
            .await
            .is_err()
    );
    assert!(
        timeout(Duration::from_millis(100), projection.next())
            .await
            .is_err()
    );
}
#[tokio::test]
async fn projected_release_revalidates_source_target_and_all_key_fields() {
    for denied in ["source", "target", "key"] {
        let (db, policy) = setup().await;
        let schema = schema_builder(db.clone())
            .data(AuthSubject::new("tester"))
            .finish();
        let mut stream = Box::pin(schema.execute_stream(PROJECTED));
        registered(&mut stream).await;
        match denied {
            "source" => policy.source.store(false, Ordering::SeqCst),
            "target" => policy.target.store(false, Ordering::SeqCst),
            _ => policy.key.store(false, Ordering::SeqCst),
        }
        CompositeEventRow::insert(&db, input("one")).await.unwrap();
        assert!(
            timeout(Duration::from_millis(100), stream.next())
                .await
                .is_err(),
            "released {denied}"
        );
    }
}
#[tokio::test]
async fn rollback_does_not_emit_composite_or_readonly_projection_events() {
    let (db, _) = setup().await;
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("tester"))
        .finish();
    let mut stream = Box::pin(schema.execute_stream(PROJECTED));
    registered(&mut stream).await;
    let result: Result<(), TransactionError> = db
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                tx.insert::<CompositeEventRow>(input("one")).await?;
                Err(OrmPublicError::forbidden())
            })
        })
        .await;
    assert!(result.is_err());
    assert!(
        timeout(Duration::from_millis(100), stream.next())
            .await
            .is_err()
    );
}
#[tokio::test]
async fn deleted_composite_source_preimages_do_not_release_direct_or_readonly_events() {
    let (db, _) = setup().await;
    CompositeEventRow::insert(&db, input("one")).await.unwrap();
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("tester"))
        .finish();
    let mut direct = Box::pin(schema.execute_stream(DIRECT));
    let mut projection = Box::pin(schema.execute_stream(PROJECTED));
    registered(&mut direct).await;
    registered(&mut projection).await;
    CompositeEventRow::delete_by_key(
        &db,
        &CompositeEventRowKey {
            tenant: "same-tenant".into(),
            code: "one".into(),
        },
    )
    .await
    .unwrap();
    assert!(
        timeout(Duration::from_millis(100), direct.next())
            .await
            .is_err(),
        "deleted direct preimage lacks current row authority"
    );
    assert!(
        timeout(Duration::from_millis(100), projection.next())
            .await
            .is_err(),
        "deleted source cannot establish current projection provenance"
    );
    CompositeEventRow::insert(&db, input("one")).await.unwrap();
    for (response, field, kind) in [
        (
            timeout(Duration::from_secs(2), direct.next())
                .await
                .unwrap()
                .unwrap(),
            "compositeEventRowChanged",
            "DIRECT",
        ),
        (
            timeout(Duration::from_secs(2), projection.next())
                .await
                .unwrap()
                .unwrap(),
            "readonlyEventProjectionChanged",
            "PROPAGATED",
        ),
    ] {
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let value = response.data.into_json().unwrap();
        assert_eq!(value[field]["key"]["tenant"], "same-tenant");
        assert_eq!(value[field]["key"]["code"], "one");
        assert_eq!(value[field]["changeKind"], kind);
    }
}

#[tokio::test]
async fn generated_catalogue_matches_readonly_subscription_sdl_without_mutations() {
    let (db, _) = setup().await;
    let schema = schema_builder(db).data(AuthSubject::new("tester")).finish();
    let sdl = schema.sdl();
    assert!(sdl.contains("readonlyEventProjectionChanged"));
    assert!(sdl.contains("ReadonlyEventProjectionKeyInput"));
    assert!(!sdl.contains("createReadonlyEventProjection"));
    assert!(!sdl.contains("type MutationRoot"));
    let catalog = graphql_orm_operation_catalog();
    assert!(
        catalog
            .operations()
            .iter()
            .any(|op| op.field_name() == "readonlyEventProjectionChanged")
    );
    assert!(
        !catalog
            .operations()
            .iter()
            .any(|op| op.kind() == GraphqlOperationKind::Mutation)
    );
    let mut invalid = Box::pin(schema.execute_stream(Request::new(
        "subscription { readonlyEventProjectionChanged(key: {tenant: \"same-tenant\"}) { id } }",
    )));
    let invalid = timeout(Duration::from_secs(2), invalid.next())
        .await
        .unwrap()
        .unwrap();
    assert!(!invalid.errors.is_empty());
}

#[tokio::test]
async fn raw_readonly_observer_events_cannot_enter_generated_delivery() {
    let (db, _) = setup().await;
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("tester"))
        .finish();
    let mut stream = Box::pin(schema.execute_stream(PROJECTED));
    registered(&mut stream).await;
    db.emit_event(ReadonlyEventProjectionChangedEvent {
        id: "untrusted".into(),
        key: ReadonlyEventProjectionKey {
            tenant: "same-tenant".into(),
            code: "one".into(),
        },
        action: ChangeAction::Updated,
        change_kind: ChangeKind::Direct,
        source_entity: None,
        source_id: None,
        path: vec![],
        entity: None,
    });
    CompositeEventRow::insert(&db, input("one")).await.unwrap();
    let response = timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let value = response.data.into_json().unwrap();
    assert_eq!(
        value["readonlyEventProjectionChanged"]["changeKind"],
        "PROPAGATED"
    );
    assert_ne!(value["readonlyEventProjectionChanged"]["id"], "untrusted");
}

mod readonly_root {
    use super::{
        ReadonlyEventProjection, ReadonlyEventProjectionQueries,
        ReadonlyEventProjectionSubscriptions,
    };
    use graphql_orm::prelude::*;
    schema_roots! { schema_policy: "external_read_only", query_custom_ops: [], entities: [ReadonlyEventProjection], }
    #[tokio::test]
    async fn schema_readonly_policy_keeps_generated_subscription_and_empty_mutation() {
        let (db, _) = super::setup().await;
        let schema = schema_builder(db).finish();
        let sdl = schema.sdl();
        assert!(sdl.contains("readonlyEventProjectionChanged"));
        assert!(!sdl.contains("createReadonlyEventProjection"));
        assert!(
            graphql_orm_operation_catalog()
                .operations()
                .iter()
                .any(|operation| operation.kind() == GraphqlOperationKind::Subscription)
        );
        assert!(
            !graphql_orm_operation_catalog()
                .operations()
                .iter()
                .any(|operation| operation.kind() == GraphqlOperationKind::Mutation)
        );
    }
}

#[derive(Clone)]
struct RevokeDuringSerialization(Arc<AtomicBool>, Arc<std::sync::atomic::AtomicUsize>);
impl async_graphql::extensions::ExtensionFactory for RevokeDuringSerialization {
    fn create(&self) -> Arc<dyn async_graphql::extensions::Extension> {
        Arc::new(self.clone())
    }
}
#[async_graphql::async_trait::async_trait]
impl async_graphql::extensions::Extension for RevokeDuringSerialization {
    async fn resolve(
        &self,
        ctx: &async_graphql::extensions::ExtensionContext<'_>,
        info: async_graphql::extensions::ResolveInfo<'_>,
        next: async_graphql::extensions::NextResolve<'_>,
    ) -> async_graphql::ServerResult<Option<async_graphql::Value>> {
        if info.parent_type == "ReadonlyEventProjection"
            && info.name == "label"
            && self.1.fetch_add(1, Ordering::SeqCst) == 0
        {
            self.0.store(true, Ordering::SeqCst);
        }
        next.run(ctx, info).await
    }
}
struct DenyLabelAfterPreflight(Arc<AtomicBool>, Arc<std::sync::atomic::AtomicUsize>, bool);
impl FieldPolicy for DenyLabelAfterPreflight {
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
            let selected = entity == "ReadonlyEventProjection" && field == "label";
            if selected {
                self.1.fetch_add(1, Ordering::SeqCst);
            }
            let denied = selected && self.0.load(Ordering::SeqCst);
            if denied && self.2 {
                return Err(async_graphql::Error::new("PRIVATE_POLICY_DIAGNOSTIC"));
            }
            Ok(!denied)
        })
    }
    fn can_read_repository_field<'a>(
        &'a self,
        _: Option<AccessContext<'a>>,
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
#[tokio::test]
async fn late_nullable_scalar_refusal_emits_no_frame_and_does_not_poison_the_next_event() {
    for policy_error in [false, true] {
        let (mut db, _) = setup().await;
        let revoked = Arc::new(AtomicBool::new(false));
        let checks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let serializations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        db.set_field_policy(DenyLabelAfterPreflight(
            revoked.clone(),
            checks.clone(),
            policy_error,
        ));
        let schema = schema_builder(db.clone())
            .extension(RevokeDuringSerialization(
                revoked.clone(),
                serializations.clone(),
            ))
            .data(AuthSubject::new("tester"))
            .finish();
        assert!(
            schema.sdl().contains("label: String\n"),
            "exercise a nullable scalar"
        );
        let query = "subscription { readonlyEventProjectionChanged { id key {tenant code} sourceId changeKind readonlyEventProjection {label} } }";
        let mut stream = Box::pin(schema.execute_stream(query));
        registered(&mut stream).await;
        CompositeEventRow::insert(&db, input("one")).await.unwrap();
        assert!(
            timeout(Duration::from_millis(100), stream.next())
                .await
                .is_err(),
            "late refusal must not release even an error frame"
        );
        assert!(
            checks.load(Ordering::SeqCst) >= 2,
            "preflight and serialization both checked the field"
        );
        assert_eq!(
            serializations.load(Ordering::SeqCst),
            1,
            "revocation must occur in real GraphQL serialization, after preflight"
        );
        revoked.store(false, Ordering::SeqCst);
        CompositeEventRow::insert(&db, input("two")).await.unwrap();
        let response = timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert!(
            response.errors.is_empty(),
            "private diagnostics from a discarded event escaped: {:?}",
            response.errors
        );
        let value = response.data.into_json().unwrap();
        assert_eq!(
            value["readonlyEventProjectionChanged"]["key"]["code"],
            "two"
        );
        assert_eq!(
            value["readonlyEventProjectionChanged"]["readonlyEventProjection"]["label"],
            "two"
        );
        assert_eq!(serializations.load(Ordering::SeqCst), 2);
    }
}

#[derive(Clone)]
struct RevokeAfterSuccessfulSerialization {
    authority: Arc<AtomicBool>,
    revoked_value: bool,
    serializations: Arc<std::sync::atomic::AtomicUsize>,
}
impl async_graphql::extensions::ExtensionFactory for RevokeAfterSuccessfulSerialization {
    fn create(&self) -> Arc<dyn async_graphql::extensions::Extension> {
        Arc::new(self.clone())
    }
}
#[async_graphql::async_trait::async_trait]
impl async_graphql::extensions::Extension for RevokeAfterSuccessfulSerialization {
    async fn resolve(
        &self,
        ctx: &async_graphql::extensions::ExtensionContext<'_>,
        info: async_graphql::extensions::ResolveInfo<'_>,
        next: async_graphql::extensions::NextResolve<'_>,
    ) -> async_graphql::ServerResult<Option<async_graphql::Value>> {
        let revoke = info.parent_type == "ReadonlyEventProjection"
            && info.name == "label"
            && self.serializations.fetch_add(1, Ordering::SeqCst) == 0;
        let value = next.run(ctx, info).await;
        if revoke {
            assert!(
                value.is_ok(),
                "the selected scalar must serialize successfully before revocation"
            );
            self.authority.store(self.revoked_value, Ordering::SeqCst);
        }
        value
    }
}

#[tokio::test]
async fn successful_serialization_cannot_outlive_source_target_key_or_field_authority() {
    for denied in ["source", "target", "key", "field", "field_error"] {
        let (mut db, policy) = setup().await;
        let field_revoked = Arc::new(AtomicBool::new(false));
        let field_checks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (authority, revoked_value) = match denied {
            "source" => (policy.source.clone(), false),
            "target" => (policy.target.clone(), false),
            "key" => (policy.key.clone(), false),
            _ => {
                db.set_field_policy(DenyLabelAfterPreflight(
                    field_revoked.clone(),
                    field_checks.clone(),
                    denied == "field_error",
                ));
                (field_revoked, true)
            }
        };
        let serializations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let schema = schema_builder(db.clone())
            .extension(RevokeAfterSuccessfulSerialization {
                authority: authority.clone(),
                revoked_value,
                serializations: serializations.clone(),
            })
            .data(AuthSubject::new("tester"))
            .finish();
        let mut stream = Box::pin(schema.execute_stream(PROJECTED));
        registered(&mut stream).await;
        CompositeEventRow::insert(&db, input("one")).await.unwrap();
        assert!(
            timeout(Duration::from_millis(100), stream.next())
                .await
                .is_err(),
            "successful serialization released a frame after {denied} authority was revoked"
        );
        assert_eq!(
            serializations.load(Ordering::SeqCst),
            1,
            "exercise real successful scalar serialization"
        );
        if denied.starts_with("field") {
            assert!(
                field_checks.load(Ordering::SeqCst) >= 3,
                "check field authority before, during, and after successful serialization"
            );
        }
        authority.store(!revoked_value, Ordering::SeqCst);
        CompositeEventRow::update_by_key(
            &db,
            &CompositeEventRowKey {
                tenant: "same-tenant".into(),
                code: "one".into(),
            },
            UpdateCompositeEventRowInput {
                label: Some("after".into()),
            },
        )
        .await
        .unwrap();
        let response = timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert!(
            response.errors.is_empty(),
            "discarded private failure poisoned the next event: {:?}",
            response.errors
        );
        let value = response.data.into_json().unwrap();
        assert_eq!(
            value["readonlyEventProjectionChanged"]["key"]["code"],
            "one"
        );
        assert_eq!(
            value["readonlyEventProjectionChanged"]["readonlyEventProjection"]["label"],
            "after"
        );
        assert_eq!(serializations.load(Ordering::SeqCst), 2);
    }
}

mod composite_cycles {
    use super::{Duration, StreamExt, registered, timeout};
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        table = "composite_propagation_nodes",
        plural = "CompositePropagationNodes",
        repository_mutations = true
    )]
    struct CompositePropagationNode {
        #[primary_key]
        #[sortable]
        tenant: String,
        #[primary_key]
        code: String,
        parent_tenant: Option<String>,
        parent_code: Option<String>,
        label: String,
        #[graphql(skip)]
        #[relation(target = "CompositePropagationNode", from = ["parent_tenant", "parent_code"], to = ["tenant", "code"], propagate_change = "up", emit_fk = false)]
        parent: Option<Box<CompositePropagationNode>>,
    }
    schema_roots! { query_custom_ops: [], entities: [CompositePropagationNode], generated_mutations: "none", }

    async fn setup() -> Database {
        let mut db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
        let policy = super::Policy {
            source: std::sync::Arc::new(super::AtomicBool::new(true)),
            target: std::sync::Arc::new(super::AtomicBool::new(true)),
            key: std::sync::Arc::new(super::AtomicBool::new(true)),
        };
        db.set_entity_policy(policy.clone());
        db.set_row_policy(policy.clone());
        db.set_field_policy(policy);
        sqlx::query("CREATE TABLE composite_propagation_nodes (tenant TEXT NOT NULL, code TEXT NOT NULL, parent_tenant TEXT, parent_code TEXT, label TEXT NOT NULL, PRIMARY KEY(tenant,code))").execute(db.pool()).await.unwrap();
        db
    }
    async fn seed(db: &Database, code: &str, parent: Option<&str>) {
        sqlx::query("INSERT INTO composite_propagation_nodes (tenant,code,parent_tenant,parent_code,label) VALUES (?,?,?,?,?)")
            .bind("shared-tenant").bind(code).bind(parent.map(|_| "shared-tenant"))
            .bind(parent).bind("before").execute(db.pool()).await.unwrap();
    }
    async fn update(db: &Database, code: &str) {
        CompositePropagationNode::update_by_key(
            db,
            &CompositePropagationNodeKey {
                tenant: "shared-tenant".into(),
                code: code.into(),
            },
            UpdateCompositePropagationNodeInput {
                label: Some("after".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .expect("source row");
    }
    const QUERY: &str =
        "subscription { compositePropagationNodeChanged { key {tenant code} path changeKind } }";

    #[tokio::test]
    async fn composite_cycle_does_not_reemit_the_direct_source() {
        let db = setup().await;
        seed(&db, "one", Some("two")).await;
        seed(&db, "two", Some("one")).await;
        let schema = schema_builder(db.clone())
            .data(AuthSubject::new("tester"))
            .finish();
        let mut stream = Box::pin(schema.execute_stream(QUERY));
        registered(&mut stream).await;
        update(&db, "one").await;
        for (code, kind, depth) in [("one", "DIRECT", 0), ("two", "PROPAGATED", 1)] {
            let response = timeout(Duration::from_secs(2), stream.next())
                .await
                .unwrap()
                .unwrap();
            assert!(response.errors.is_empty(), "{:?}", response.errors);
            let value = response.data.into_json().unwrap();
            let event = &value["compositePropagationNodeChanged"];
            assert_eq!(event["key"]["tenant"], "shared-tenant");
            assert_eq!(event["key"]["code"], code);
            assert_eq!(event["changeKind"], kind);
            assert_eq!(event["path"].as_array().unwrap().len(), depth);
        }
        assert!(
            timeout(Duration::from_millis(100), stream.next())
                .await
                .is_err(),
            "cycle re-emitted an already notified complete key"
        );
    }

    #[tokio::test]
    async fn composite_chain_stops_at_sixteen_authorized_hops() {
        let db = setup().await;
        for index in 0..40 {
            let parent = (index > 0).then(|| format!("node-{}", index - 1));
            seed(&db, &format!("node-{index}"), parent.as_deref()).await;
        }
        let schema = schema_builder(db.clone())
            .data(AuthSubject::new("tester"))
            .finish();
        let mut stream = Box::pin(schema.execute_stream(QUERY));
        registered(&mut stream).await;
        update(&db, "node-39").await;
        for depth in 0..=16 {
            let response = timeout(Duration::from_secs(2), stream.next())
                .await
                .unwrap()
                .unwrap();
            assert!(response.errors.is_empty(), "{:?}", response.errors);
            let value = response.data.into_json().unwrap();
            let event = &value["compositePropagationNodeChanged"];
            assert_eq!(event["key"]["tenant"], "shared-tenant");
            assert_eq!(event["key"]["code"], format!("node-{}", 39 - depth));
            assert_eq!(event["path"].as_array().unwrap().len(), depth);
        }
        assert!(
            timeout(Duration::from_millis(100), stream.next())
                .await
                .is_err(),
            "composite propagation exceeded its hop bound"
        );
    }
}
