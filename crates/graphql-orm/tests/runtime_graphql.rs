#![cfg(all(
    feature = "runtime-graphql",
    any(feature = "sqlite", feature = "postgres")
))]

use chacha20poly1305::{
    ChaCha20Poly1305, KeyInit,
    aead::{Aead, Payload},
};
use futures::future::BoxFuture;
use graphql_orm::async_graphql::{
    self, Request, Variables,
    dynamic::{Field, FieldFuture, FieldValue, Object, TypeRef},
};
use graphql_orm::{
    db::Database,
    graphql::{orm::*, runtime::*},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(all(feature = "sqlite", not(feature = "postgres")))]
type Backend = SqliteBackend;

static QUERY_COUNTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct DeferredDirective;
#[async_trait::async_trait]
impl async_graphql::CustomDirective for DeferredDirective {}
#[async_graphql::Directive(location = "Field")]
fn deferred_directive() -> impl async_graphql::CustomDirective {
    panic!("unsupported factory must never be invoked");
    #[allow(unreachable_code)]
    DeferredDirective
}

struct ReplaceSelection;
impl async_graphql::extensions::ExtensionFactory for ReplaceSelection {
    fn create(&self) -> Arc<dyn async_graphql::extensions::Extension> {
        Arc::new(Self)
    }
}
#[async_trait::async_trait]
impl async_graphql::extensions::Extension for ReplaceSelection {
    async fn parse_query(
        &self,
        context: &async_graphql::extensions::ExtensionContext<'_>,
        query: &str,
        variables: &Variables,
        next: async_graphql::extensions::NextParseQuery<'_>,
    ) -> async_graphql::ServerResult<async_graphql::parser::types::ExecutableDocument> {
        let _ = next.run(context, query, variables).await?;
        Ok(async_graphql::parser::parse_query("{parents{edges{node{tenant}}}}").unwrap())
    }
}

struct TestAead;
impl RuntimeCursorProtector for TestAead {
    fn seal<'a>(
        &'a self,
        context: &'a RuntimeCursorContext,
        plaintext: &'a str,
        limits: RuntimeCursorProtectionLimits,
    ) -> BoxFuture<'a, Result<RuntimeSealedCursor, RuntimeCursorProtectionError>> {
        Box::pin(async move {
            let cipher = ChaCha20Poly1305::new(&[23; 32].into());
            let nonce = graphql_orm::uuid::Uuid::new_v4();
            let nonce = &nonce.as_bytes()[..12];
            let mut bytes = nonce.to_vec();
            bytes.extend(
                cipher
                    .encrypt(
                        nonce.into(),
                        Payload {
                            msg: plaintext.as_bytes(),
                            aad: context.associated_data(),
                        },
                    )
                    .map_err(|_| RuntimeCursorProtectionError::InvalidCursor)?,
            );
            RuntimeSealedCursor::new("test-key", bytes, limits)
        })
    }
    fn open<'a>(
        &'a self,
        context: &'a RuntimeCursorContext,
        key: &'a str,
        bytes: &'a [u8],
        _: RuntimeCursorProtectionLimits,
    ) -> BoxFuture<'a, Result<String, RuntimeCursorProtectionError>> {
        Box::pin(async move {
            if key != "test-key" {
                return Err(RuntimeCursorProtectionError::CursorUnavailable);
            }
            if bytes.len() < 28 {
                return Err(RuntimeCursorProtectionError::InvalidCursor);
            }
            let cipher = ChaCha20Poly1305::new(&[23; 32].into());
            let value = cipher
                .decrypt(
                    (&bytes[..12]).into(),
                    Payload {
                        msg: &bytes[12..],
                        aad: context.associated_data(),
                    },
                )
                .map_err(|_| RuntimeCursorProtectionError::InvalidCursor)?;
            String::from_utf8(value).map_err(|_| RuntimeCursorProtectionError::InvalidCursor)
        })
    }
}
fn cid(s: &str) -> CollectionId {
    CollectionId::new(s).unwrap()
}
fn fid(s: &str) -> FieldId {
    FieldId::new(s).unwrap()
}
fn field(id: &str, name: &str, kind: RuntimeValueKind, nullable: bool) -> RuntimeField {
    RuntimeField {
        id: fid(id),
        api_name: name.into(),
        physical_column: name.to_lowercase(),
        value_kind: kind,
        nullable,
        unique: name == "id",
        filterable: true,
        sortable: kind != RuntimeValueKind::Json,
        generated: false,
        default: None,
    }
}
fn schema() -> Arc<ValidatedRuntimeSchema> {
    let mut parent = RuntimeCollection {
        id: cid("parent"),
        api_type_name: "Parent".into(),
        api_plural_name: "Parents".into(),
        physical_table: "gql_parents".into(),
        primary_key: vec![fid("parent.id")],
        append_only: false,
        retention_purge: false,
        fields: vec![
            field("parent.id", "id", RuntimeValueKind::Integer, false),
            field("parent.tenant", "tenant", RuntimeValueKind::String, false),
            field("parent.label", "label", RuntimeValueKind::String, false),
            field("parent.required", "required", RuntimeValueKind::Json, false),
            field("parent.document", "document", RuntimeValueKind::Json, true),
        ],
        relations: vec![],
        indexes: vec![],
        composite_unique: vec![],
        default_order: vec![],
    };
    let mut child = RuntimeCollection {
        id: cid("child"),
        api_type_name: "Child".into(),
        api_plural_name: "Children".into(),
        physical_table: "gql_children".into(),
        primary_key: vec![fid("child.id")],
        append_only: false,
        retention_purge: false,
        fields: vec![
            field("child.id", "id", RuntimeValueKind::Integer, false),
            field("child.tenant", "tenant", RuntimeValueKind::String, false),
            field("child.owner", "owner", RuntimeValueKind::Integer, false),
            field("child.label", "label", RuntimeValueKind::String, false),
        ],
        relations: vec![],
        indexes: vec![],
        composite_unique: vec![],
        default_order: vec![],
    };
    child.relations.push(RuntimeRelation {
        id: RelationId::new("parent").unwrap(),
        api_name: "parent".into(),
        target: parent.id.clone(),
        key_pairs: vec![
            RelationKeyPair {
                source: fid("child.owner"),
                target: fid("parent.id"),
            },
            RelationKeyPair {
                source: fid("child.tenant"),
                target: fid("parent.tenant"),
            },
        ],
        cardinality: RelationCardinality::One,
        enforce_foreign_key: false,
        on_delete: None,
    });
    parent.relations.push(RuntimeRelation {
        id: RelationId::new("children").unwrap(),
        api_name: "children".into(),
        target: child.id.clone(),
        key_pairs: vec![
            RelationKeyPair {
                source: fid("parent.id"),
                target: fid("child.owner"),
            },
            RelationKeyPair {
                source: fid("parent.tenant"),
                target: fid("child.tenant"),
            },
        ],
        cardinality: RelationCardinality::Many,
        enforce_foreign_key: false,
        on_delete: None,
    });
    Arc::new(
        RuntimeSchema {
            format_version: 1,
            collections: vec![parent, child],
        }
        .validate()
        .unwrap(),
    )
}
struct Authority {
    deny_child: AtomicBool,
    deny_count: AtomicBool,
    calls: AtomicUsize,
    tenant: &'static str,
}
impl RuntimeReadAuthority for Authority {
    fn authorize<'a>(
        &'a self,
        check: RuntimeReadCheck<'a>,
    ) -> BoxFuture<'a, Result<RuntimeReadGrant, RuntimeGraphqlError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if (self.deny_child.load(Ordering::SeqCst) && check.relation.is_some())
                || (self.deny_count.load(Ordering::SeqCst) && check.include_count)
            {
                return Err(RuntimeGraphqlError::denied());
            }
            // Internal keys may be used for deterministic order; never project tenant.
            if check
                .projection
                .iter()
                .any(|f| f.id().as_str().ends_with("tenant"))
            {
                return Err(RuntimeGraphqlError::denied());
            }
            if check
                .filter_operations
                .iter()
                .any(|(field, op)| field.id().as_str().ends_with("tenant") || *op == "contains")
                || check
                    .order
                    .terms()
                    .iter()
                    .any(|term| term.field().id().as_str().ends_with("tenant"))
            {
                return Err(RuntimeGraphqlError::denied());
            }
            if check.requested_order.iter().any(|term| {
                term.field.id().as_str().ends_with(".id") && !check.projection.contains(&term.field)
            }) {
                return Err(RuntimeGraphqlError::denied());
            }
            let tenant = check
                .schema
                .resolve_field(
                    check.collection,
                    &fid(&format!("{}.tenant", check.collection.id().as_str())),
                )
                .unwrap();
            let predicate = check
                .schema
                .runtime_compare(
                    check.collection,
                    &tenant,
                    RuntimeScalarOperator::Eq,
                    RuntimeValue::String(self.tenant.into()),
                    RuntimeQueryLimits::default(),
                )
                .unwrap();
            Ok(RuntimeReadGrant::new(Some(predicate)))
        })
    }
}
fn request(
    schema: &ValidatedRuntimeSchema,
    authority: Arc<Authority>,
    query: &str,
    variables: serde_json::Value,
) -> Request {
    Request::new(query)
        .variables(Variables::from_json(variables))
        .data(
            RuntimeGraphqlRequest::<Backend>::new(
                schema.fingerprint(),
                authority,
                None,
                RuntimeGraphqlLimits::default(),
            )
            .with_cursor_scope("trusted-north"),
        )
}
#[tokio::test]
async fn authorized_reads_counts_batched_relations_and_confidential_resume() {
    let _query_counts = QUERY_COUNTS.lock().await;
    #[cfg(feature = "postgres")]
    let mut owned = owned_postgres::OwnedPostgres::start("runtime-graphql-c").unwrap();
    #[cfg(feature = "postgres")]
    let database = Database::connect_postgres(&owned.url).await.unwrap();
    #[cfg(all(feature = "sqlite", not(feature = "postgres")))]
    let database = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    #[cfg(feature = "postgres")]
    let json_type = "JSONB";
    #[cfg(not(feature = "postgres"))]
    let json_type = "TEXT";
    graphql_orm::sqlx::query(&format!("CREATE TABLE gql_parents (id BIGINT PRIMARY KEY, tenant TEXT NOT NULL, label TEXT NOT NULL, document {json_type}, required {json_type} NOT NULL DEFAULT 'null')")).execute(database.pool()).await.unwrap();
    graphql_orm::sqlx::query("CREATE TABLE gql_children (id BIGINT PRIMARY KEY, tenant TEXT NOT NULL, owner BIGINT NOT NULL, label TEXT NOT NULL)").execute(database.pool()).await.unwrap();
    graphql_orm::sqlx::query("INSERT INTO gql_parents(id,tenant,label,document) VALUES (1,'north','first','null'),(2,'north','second',NULL),(3,'south','private',NULL)").execute(database.pool()).await.unwrap();
    graphql_orm::sqlx::query("INSERT INTO gql_children VALUES (11,'north',1,'a'),(12,'north',1,'b'),(21,'north',2,'c'),(22,'north',2,'d'),(31,'south',1,'private'),(41,'north',99,'orphan')").execute(database.pool()).await.unwrap();
    let runtime = schema();
    let module = RuntimeGraphqlModule::compile(runtime.clone(), Default::default()).unwrap();
    let api = RuntimeGraphqlComposer::new(database.clone(), "Query", Default::default())
        .unwrap()
        .cursor_protection(
            Arc::new(TestAead),
            RuntimeCursorAudience::new("synthetic-tests").unwrap(),
            Default::default(),
        )
        .unwrap()
        .query_field(RuntimeHostField::new(
            "health",
            TypeRef::named_nn("Boolean"),
            |_| FieldFuture::new(async { Ok(Some(FieldValue::value(true))) }),
        ))
        .unwrap()
        .install(module)
        .unwrap()
        .finish()
        .unwrap();
    let authority = Arc::new(Authority {
        deny_child: AtomicBool::new(false),
        deny_count: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
        tenant: "north",
    });
    reset_query_count();
    let query = "{ health parents(first:2) { edges { cursor node { id label document required children(first:1) { edges { cursor node { label } } totalCount pageInfo { startCursor endCursor hasNextPage } } } } totalCount pageInfo { startCursor endCursor hasNextPage } } }";
    let response = api
        .execute(request(
            &runtime,
            authority.clone(),
            query,
            serde_json::json!({}),
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data["parents"]["totalCount"], "2");
    assert_eq!(data["parents"]["edges"][0]["node"]["document"], "null");
    assert!(data["parents"]["edges"][1]["node"]["document"].is_null());
    assert_eq!(data["parents"]["edges"][0]["node"]["required"], "null");
    assert_eq!(
        data["parents"]["edges"][0]["node"]["children"]["totalCount"],
        "2"
    );
    assert_eq!(
        query_count(),
        4,
        "one parent read/count and one batched child read/count"
    );
    let cursor = data["parents"]["edges"][0]["cursor"].as_str().unwrap();
    assert!(cursor.starts_with("gormgqlc1.test-key."));
    assert_eq!(data["parents"]["pageInfo"]["startCursor"], cursor);
    let forward = api
        .execute(request(
            &runtime,
            authority.clone(),
            "query($cursor:String!){ parents(first:1,after:$cursor){edges{node{id}}totalCount}}",
            serde_json::json!({"cursor":cursor}),
        ))
        .await;
    assert!(forward.errors.is_empty(), "{:?}", forward.errors);
    assert_eq!(
        forward.data.into_json().unwrap()["parents"]["edges"][0]["node"]["id"],
        "2"
    );
    let second_cursor = data["parents"]["edges"][1]["cursor"].as_str().unwrap();
    let backward = api
        .execute(request(
            &runtime,
            authority.clone(),
            "query($cursor:String!){parents(last:1,before:$cursor){edges{node{id}}}}",
            serde_json::json!({"cursor":second_cursor}),
        ))
        .await;
    assert!(backward.errors.is_empty(), "{:?}", backward.errors);
    assert_eq!(
        backward.data.into_json().unwrap()["parents"]["edges"][0]["node"]["id"],
        "1"
    );
    let nested = &data["parents"]["edges"][0]["node"]["children"]["edges"][0]["cursor"];
    let resumed = api.execute(request(&runtime, authority.clone(), "query($cursor:String!){parents(where:{id:{eq:\"1\"}}){edges{node{children(first:1,after:$cursor){edges{node{label}}}}}}}", serde_json::json!({"cursor":nested}))).await;
    assert!(resumed.errors.is_empty(), "{:?}", resumed.errors);
    assert_eq!(
        resumed.data.into_json().unwrap()["parents"]["edges"][0]["node"]["children"]["edges"][0]["node"]
            ["label"],
        "b"
    );
    reset_query_count();
    let wrong_parent = api.execute(request(&runtime, authority.clone(), "query($cursor:String!){parents(where:{id:{eq:\"2\"}}){edges{node{children(first:1,after:$cursor){edges{node{label}}}}}}}", serde_json::json!({"cursor":nested}))).await;
    assert!(!wrong_parent.errors.is_empty());
    assert_eq!(
        query_count(),
        1,
        "parent anchor acquired but child query denied"
    );
    authority.deny_child.store(true, Ordering::SeqCst);
    reset_query_count();
    let denied = api
        .execute(request(
            &runtime,
            authority.clone(),
            query,
            serde_json::json!({}),
        ))
        .await;
    assert!(!denied.errors.is_empty());
    assert_eq!(
        query_count(),
        0,
        "whole subtree authorization before parent I/O"
    );
    authority.deny_child.store(false, Ordering::SeqCst);
    authority.deny_count.store(true, Ordering::SeqCst);
    reset_query_count();
    let denied = api.execute(request(&runtime, authority.clone(), "{parents{... Counts edges{node{label}}}} fragment Counts on ParentConnection { totalCount }", serde_json::json!({}))).await;
    assert!(!denied.errors.is_empty());
    assert_eq!(query_count(), 0);
    let allowed = api
        .execute(request(
            &runtime,
            authority.clone(),
            "query($count:Boolean!){parents{totalCount @include(if:$count) edges{node{label}}}}",
            serde_json::json!({"count":false}),
        ))
        .await;
    assert!(allowed.errors.is_empty(), "{:?}", allowed.errors);
    assert_eq!(query_count(), 1);
    reset_query_count();
    let denied = api
        .execute(request(
            &runtime,
            authority.clone(),
            "{parents{edges{node{secret:tenant}}}}",
            serde_json::json!({}),
        ))
        .await;
    assert!(!denied.errors.is_empty());
    assert_eq!(query_count(), 0);
    for query in [
        "{parents(where:{tenant:{eq:\"north\"}}){edges{node{label}}}}",
        "{parents(orderBy:{field:tenant}){edges{node{label}}}}",
        "{parents(orderBy:{field:id}){edges{node{label}}}}",
        "{parents(where:{label:{contains:\"first\"}}){edges{node{label}}}}",
    ] {
        assert!(
            !api.execute(request(
                &runtime,
                authority.clone(),
                query,
                serde_json::json!({})
            ))
            .await
            .errors
            .is_empty()
        );
        assert_eq!(query_count(), 0);
    }
    let raw = api
        .execute(request(
            &runtime,
            authority.clone(),
            "{parents(after:\"gormrq1.raw\"){edges{node{label}}}}",
            serde_json::json!({}),
        ))
        .await;
    assert!(!raw.errors.is_empty());
    assert_eq!(query_count(), 0);
    let absent = api.execute("{parents{edges{node{label}}}}").await;
    assert!(!absent.errors.is_empty());
    assert_eq!(query_count(), 0);
    authority.deny_count.store(false, Ordering::SeqCst);
    reset_query_count();
    let deep = api
        .execute(request(
            &runtime,
            authority.clone(),
            "{parents{edges{node{children(first:1){edges{node{parent{label}}}}}}}}",
            serde_json::json!({}),
        ))
        .await;
    assert!(deep.errors.is_empty(), "{:?}", deep.errors);
    assert_eq!(query_count(), 3);
    assert_eq!(
        deep.data.into_json().unwrap()["parents"]["edges"][1]["node"]["children"]["edges"][0]["node"]
            ["parent"]["label"],
        "second"
    );
    let missing = api
        .execute(request(
            &runtime,
            authority.clone(),
            "{children(where:{id:{eq:\"41\"}}){edges{node{label parent{label}}}}}",
            serde_json::json!({}),
        ))
        .await;
    assert!(missing.errors.is_empty(), "{:?}", missing.errors);
    assert!(missing.data.into_json().unwrap()["children"]["edges"][0]["node"]["parent"].is_null());
    // Variable enum strings and GraphQL singleton-list coercion use the same typed order.
    let ordered = api.execute(request(&runtime, authority.clone(),
        "query($order:[ParentOrderInput!]){parents(orderBy:$order,where:{and:{not:{label:{eq:\"absent\"}}}}){edges{node{id}}}}",
        serde_json::json!({"order":{"field":"id","direction":"DESC","nulls":"LAST"}}))).await;
    assert!(ordered.errors.is_empty(), "{:?}", ordered.errors);
    assert_eq!(
        ordered.data.into_json().unwrap()["parents"]["edges"][0]["node"]["id"],
        "2"
    );
    // A later invalid alias cursor must be rejected before an earlier alias reads.
    reset_query_count();
    let invalid_alias = api
        .execute(request(
            &runtime,
            authority.clone(),
            "query($c:String!){a:parents{edges{node{id}}} b:parents(after:$c){edges{node{id}}}}",
            serde_json::json!({"c":format!("{}A", cursor)}),
        ))
        .await;
    assert!(!invalid_alias.errors.is_empty());
    assert_eq!(query_count(), 0);
    let wrong_order = api.execute(request(&runtime, authority.clone(),
        "query($c:String!){parents(after:$c,orderBy:{field:id,direction:DESC}){edges{node{id}}}}",
        serde_json::json!({"c":cursor}))).await;
    assert!(!wrong_order.errors.is_empty());
    assert_eq!(query_count(), 0);
    let scope_request = Request::new("query($c:String!){parents(after:$c){edges{node{id}}}}")
        .variables(Variables::from_json(serde_json::json!({"c":cursor})))
        .data(
            RuntimeGraphqlRequest::<Backend>::new(
                runtime.fingerprint(),
                authority.clone(),
                None,
                Default::default(),
            )
            .with_cursor_scope("revoked-partition"),
        );
    assert!(!api.execute(scope_request).await.errors.is_empty());
    assert_eq!(query_count(), 0);
    let mut newer = runtime.schema().clone();
    newer.collections[0].append_only = true;
    let newer = Arc::new(newer.validate().unwrap());
    let newer_api = RuntimeGraphqlComposer::new(database.clone(), "Query", Default::default())
        .unwrap()
        .cursor_protection(
            Arc::new(TestAead),
            RuntimeCursorAudience::new("synthetic-tests").unwrap(),
            Default::default(),
        )
        .unwrap()
        .install(RuntimeGraphqlModule::compile(newer.clone(), Default::default()).unwrap())
        .unwrap()
        .finish()
        .unwrap();
    let stale_cursor = newer_api
        .execute(request(
            &newer,
            authority.clone(),
            "query($c:String!){parents(after:$c){edges{node{id}}}}",
            serde_json::json!({"c":cursor}),
        ))
        .await;
    assert!(!stale_cursor.errors.is_empty());
    assert_eq!(query_count(), 0);
    let other_audience = RuntimeGraphqlComposer::new(database.clone(), "Query", Default::default())
        .unwrap()
        .cursor_protection(
            Arc::new(TestAead),
            RuntimeCursorAudience::new("different-audience").unwrap(),
            Default::default(),
        )
        .unwrap()
        .install(RuntimeGraphqlModule::compile(runtime.clone(), Default::default()).unwrap())
        .unwrap()
        .finish()
        .unwrap();
    let wrong_audience = other_audience
        .execute(request(
            &runtime,
            authority.clone(),
            "query($c:String!){parents(after:$c){edges{node{id}}}}",
            serde_json::json!({"c":cursor}),
        ))
        .await;
    assert!(!wrong_audience.errors.is_empty());
    assert_eq!(query_count(), 0);
    let south = Arc::new(Authority {
        deny_child: AtomicBool::new(false),
        deny_count: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
        tenant: "south",
    });
    let other = api
        .execute(request(
            &runtime,
            south.clone(),
            "{parents{edges{node{label}}totalCount}}",
            serde_json::json!({}),
        ))
        .await;
    assert!(other.errors.is_empty());
    let other = other.data.into_json().unwrap();
    assert_eq!(other["parents"]["totalCount"], "1");
    assert_eq!(other["parents"]["edges"][0]["node"]["label"], "private");
    // Identical trusted cursor partition does not restore old predicates/authority.
    let changed = api
        .execute(request(
            &runtime,
            south,
            "query($c:String!){parents(after:$c){edges{node{label}}totalCount}}",
            serde_json::json!({"c":cursor}),
        ))
        .await;
    assert!(changed.errors.is_empty());
    assert_eq!(
        changed.data.into_json().unwrap()["parents"]["totalCount"],
        "1"
    );
    reset_query_count();
    let limited = RuntimeGraphqlLimits {
        max_statements: 1,
        ..Default::default()
    };
    let limited = Request::new("{parents{edges{node{label}}totalCount}}").data(
        RuntimeGraphqlRequest::<Backend>::new(
            runtime.fingerprint(),
            authority.clone(),
            None,
            limited,
        )
        .with_cursor_scope("trusted-north"),
    );
    assert!(!api.execute(limited).await.errors.is_empty());
    assert_eq!(query_count(), 0);
    let mut changed_schema = runtime.schema().clone();
    changed_schema.collections[0].append_only = true;
    let stale = Request::new("{parents{edges{node{label}}}}").data(
        RuntimeGraphqlRequest::<Backend>::new(
            changed_schema.validate().unwrap().fingerprint(),
            authority.clone(),
            None,
            Default::default(),
        )
        .with_cursor_scope("trusted-north"),
    );
    assert!(!api.execute(stale).await.errors.is_empty());
    assert_eq!(query_count(), 0);
    let invalid_value = api
        .execute(request(
            &runtime,
            authority,
            "{parents(where:{id:{eq:\"secret-invalid-value\"}}){edges{node{id}}}}",
            serde_json::json!({}),
        ))
        .await;
    assert!(!invalid_value.errors.is_empty());
    assert!(!format!("{:?}", invalid_value.errors).contains("secret-invalid-value"));
    assert_eq!(query_count(), 0);
    let composer = || {
        RuntimeGraphqlComposer::new(database.clone(), "Query", Default::default())
            .unwrap()
            .cursor_protection(
                Arc::new(TestAead),
                RuntimeCursorAudience::new("collisions").unwrap(),
                Default::default(),
            )
            .unwrap()
    };
    let module = || RuntimeGraphqlModule::compile(runtime.clone(), Default::default()).unwrap();
    let colliding = composer()
        .register(
            Object::new("Parent")
                .field(Field::new("ok", TypeRef::named("Boolean"), |_| {
                    FieldFuture::new(async { Ok(Some(Value::Boolean(true))) })
                }))
                .into(),
        )
        .unwrap()
        .install(module());
    assert!(
        matches!(colliding, Err(ref diagnostics) if diagnostics.0[0].code() == "name_collision")
    );
    assert!(
        composer()
            .install(module())
            .unwrap()
            .register(Object::new("Parent").into())
            .is_err()
    );
    assert!(
        composer()
            .register(async_graphql::dynamic::Scalar::new("String").into())
            .is_err()
    );
    assert!(
        composer()
            .register(async_graphql::dynamic::Scalar::new("RuntimeJson").into())
            .unwrap()
            .install(module())
            .is_err()
    );
    assert!(
        composer()
            .install(module())
            .unwrap()
            .query_field(RuntimeHostField::new(
                "parents",
                TypeRef::named("Boolean"),
                |_| FieldFuture::new(async { Ok(Some(Value::Boolean(true))) })
            ))
            .is_err()
    );
    assert!(
        RuntimeGraphqlComposer::new(database.clone(), "Query", Default::default())
            .unwrap()
            .install(module())
            .is_err()
    );
    assert!(matches!(
        composer().directive(deferred_directive),
        Err(error) if error.code() == "unsupported_capability"
    ));
    use async_graphql::dynamic::{Subscription, SubscriptionField, SubscriptionFieldFuture};
    use futures::StreamExt;
    let subscriptions = || {
        Subscription::new("HostSubscription").field(SubscriptionField::new(
            "pulse",
            TypeRef::named_nn("Boolean"),
            |_| {
                SubscriptionFieldFuture::new(async {
                    Ok(futures::stream::iter([Ok(FieldValue::value(true))]))
                })
            },
        ))
    };
    assert!(
        composer()
            .subscription_root(Subscription::new("Query"))
            .is_err()
    );
    assert!(
        composer()
            .subscription_root(subscriptions())
            .unwrap()
            .subscription_root(subscriptions())
            .is_err()
    );
    let composed = composer()
        .install(module())
        .unwrap()
        .subscription_root(subscriptions())
        .unwrap()
        .limit_directives(1)
        .finish()
        .unwrap();
    let response = composed
        .execute_stream("subscription { pulse }")
        .next()
        .await
        .unwrap();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(response.data, async_graphql::value!({"pulse":true}));
    reset_query_count();
    let skipped = composed
        .execute("{ parents @skip(if:true) { edges { node { tenant } } } }")
        .await;
    assert!(skipped.errors.is_empty(), "{:?}", skipped.errors);
    assert_eq!(query_count(), 0);
    let limited = composed
        .execute("{ parents @skip(if:true) @include(if:false) { edges { node { label } } } }")
        .await;
    assert!(!limited.errors.is_empty());
    assert_eq!(query_count(), 0);
    let forged = composer()
        .query_field(RuntimeHostField::new(
            "forged",
            TypeRef::named("Parent"),
            |_| {
                FieldFuture::new(async {
                    Ok(Some(FieldValue::owned_any(
                        async_graphql::value!({"label":"private-forged-value"}),
                    )))
                })
            },
        ))
        .unwrap()
        .install(module())
        .unwrap()
        .finish()
        .unwrap();
    reset_query_count();
    let forged = forged.execute("{forged{label}}").await;
    assert!(!forged.errors.is_empty());
    assert!(!format!("{:?}", forged.errors).contains("private-forged-value"));
    assert_eq!(query_count(), 0);
    let guarded = composer()
        .extension(ReplaceSelection)
        .install(module())
        .unwrap()
        .finish()
        .unwrap();
    let guarded_authority = Arc::new(Authority {
        deny_child: AtomicBool::new(false),
        deny_count: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
        tenant: "north",
    });
    reset_query_count();
    let changed_selection = guarded
        .execute(request(
            &runtime,
            guarded_authority,
            "{parents{edges{node{label}}}}",
            serde_json::json!({}),
        ))
        .await;
    assert!(!changed_selection.errors.is_empty());
    assert_eq!(
        query_count(),
        0,
        "host parse extensions cannot bypass final selection preflight"
    );
    let unprotected = RuntimeGraphqlModule::compile(
        runtime.clone(),
        RuntimeGraphqlOptions::default().with_cursor_profile(RuntimeCursorProfile::Unprotected),
    )
    .unwrap();
    let unprotected = RuntimeGraphqlComposer::new(database, "Query", Default::default())
        .unwrap()
        .install(unprotected)
        .unwrap()
        .finish()
        .unwrap();
    let authority = Arc::new(Authority {
        deny_child: AtomicBool::new(false),
        deny_count: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
        tenant: "north",
    });
    let raw_response = unprotected
        .execute(request(
            &runtime,
            authority,
            "{parents(first:1){edges{cursor node{label}}}}",
            serde_json::json!({}),
        ))
        .await;
    assert!(raw_response.errors.is_empty());
    assert!(
        raw_response.data.into_json().unwrap()["parents"]["edges"][0]["cursor"]
            .as_str()
            .unwrap()
            .starts_with("gormrq1")
    );
    #[cfg(feature = "postgres")]
    owned.cleanup().unwrap();
}
use async_graphql::Value;

#[cfg(feature = "postgres")]
#[tokio::test]
async fn postgres_rls_context_is_propagated_to_read_count_and_relation_layers() {
    let _query_counts = QUERY_COUNTS.lock().await;
    struct Public;
    impl RuntimeReadAuthority for Public {
        fn authorize<'a>(
            &'a self,
            _: RuntimeReadCheck<'a>,
        ) -> BoxFuture<'a, Result<RuntimeReadGrant, RuntimeGraphqlError>> {
            Box::pin(async { Ok(RuntimeReadGrant::new(None)) })
        }
    }
    let mut owned = owned_postgres::OwnedPostgres::start("runtime-gql-rls").unwrap();
    let owner = Database::connect_postgres(&owned.url).await.unwrap();
    for sql in [
        "CREATE TABLE gql_parents(id BIGINT PRIMARY KEY,tenant TEXT NOT NULL,label TEXT NOT NULL,document JSONB,required JSONB NOT NULL DEFAULT 'null')",
        "CREATE TABLE gql_children(id BIGINT PRIMARY KEY,tenant TEXT NOT NULL,owner BIGINT NOT NULL,label TEXT NOT NULL)",
        "INSERT INTO gql_parents(id,tenant,label,document) VALUES(1,'north','n',NULL),(2,'south','s',NULL)",
        "INSERT INTO gql_children VALUES(11,'north',1,'n-child'),(21,'south',2,'s-child')",
        "CREATE ROLE runtime_gql_reader NOLOGIN",
        "GRANT SELECT ON gql_parents,gql_children TO runtime_gql_reader",
        "ALTER TABLE gql_parents ENABLE ROW LEVEL SECURITY",
        "ALTER TABLE gql_children ENABLE ROW LEVEL SECURITY",
        "CREATE POLICY parents_tenant ON gql_parents USING(tenant=current_setting('app.tenant_id',true))",
        "CREATE POLICY children_tenant ON gql_children USING(tenant=current_setting('app.tenant_id',true))",
    ] {
        graphql_orm::sqlx::query(sql)
            .execute(owner.pool())
            .await
            .unwrap();
    }
    let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|connection, _| {
            Box::pin(async move {
                graphql_orm::sqlx::query("SET ROLE runtime_gql_reader")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&owned.url)
        .await
        .unwrap();
    let runtime = schema();
    let api = RuntimeGraphqlComposer::new(
        Database::<PostgresBackend>::new(pool.clone()),
        "Query",
        Default::default(),
    )
    .unwrap()
    .cursor_protection(
        Arc::new(TestAead),
        RuntimeCursorAudience::new("rls-test").unwrap(),
        Default::default(),
    )
    .unwrap()
    .install(RuntimeGraphqlModule::compile(runtime.clone(), Default::default()).unwrap())
    .unwrap()
    .finish()
    .unwrap();
    for (tenant, label) in [
        (Some("north"), Some("n")),
        (Some("south"), Some("s")),
        (None, None),
    ] {
        let auth = tenant.map(|tenant| DbAuthContext {
            tenant_id: Some(tenant.into()),
            ..Default::default()
        });
        let response = api.execute(Request::new("{parents{edges{node{label children{edges{node{label}}totalCount}}}totalCount}}")
            .data(RuntimeGraphqlRequest::<PostgresBackend>::new(runtime.fingerprint(), Arc::new(Public), auth, Default::default()).with_cursor_scope(tenant.unwrap_or("anonymous")))).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json().unwrap();
        if let Some(label) = label {
            assert_eq!(data["parents"]["totalCount"], "1");
            assert_eq!(data["parents"]["edges"][0]["node"]["label"], label);
            assert_eq!(
                data["parents"]["edges"][0]["node"]["children"]["totalCount"],
                "1"
            );
            assert_eq!(
                data["parents"]["edges"][0]["node"]["children"]["edges"][0]["node"]["label"],
                format!("{label}-child")
            );
        } else {
            assert_eq!(data["parents"]["totalCount"], "0");
        }
    }
    pool.close().await;
    owner.pool().close().await;
    owned.cleanup().unwrap();
}
