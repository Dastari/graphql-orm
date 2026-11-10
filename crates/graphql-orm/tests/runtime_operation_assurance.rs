//! Operation assurance on runtime GraphQL roots.
//!
//! Runtime create/update/delete/link mutations, host mutation fields and
//! runtime query roots enforce the installed `AssuranceEnforcement` with their
//! own root coordinate before any write. A registry miss applies the configured
//! interactive mutation default, and strict classification refuses to finish a
//! composition whose mutation roots are not all registered.
#![cfg(all(
    feature = "runtime-graphql",
    feature = "sqlite",
    not(any(feature = "postgres", feature = "mssql"))
))]

use futures::future::BoxFuture;
use graphql_orm::async_graphql::{
    self, ErrorExtensions, Request, Variables,
    dynamic::{FieldFuture, FieldValue, Schema, TypeRef},
};
use graphql_orm::graphql::assurance::*;
use graphql_orm::graphql::runtime::*;
use graphql_orm::{graphql::orm::*, prelude::*};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

type Backend = SqliteBackend;

const POLICY: &str = "interactive.recent-auth";

fn cid(s: &str) -> CollectionId {
    CollectionId::new(s).unwrap()
}
fn fid(s: &str) -> FieldId {
    FieldId::new(s).unwrap()
}
fn field(id: &str, name: &str, kind: RuntimeValueKind) -> RuntimeField {
    RuntimeField {
        description: None,
        classification: None,
        export: None,
        id: fid(id),
        api_name: name.into(),
        physical_column: name.to_lowercase(),
        value_kind: kind,
        nullable: false,
        unique: false,
        filterable: true,
        sortable: true,
        generated: false,
        default: None,
        format: None,
        auto_timestamp: None,
        auto_actor: None,
    }
}
fn collection(id: &str, name: &str, plural: &str) -> RuntimeCollection {
    RuntimeCollection {
        node_identity: None,
        live_subscriptions: None,
        description: None,
        classification: None,
        id: cid(id),
        api_type_name: name.into(),
        api_plural_name: plural.into(),
        physical_table: format!("ra_{id}"),
        primary_key: vec![fid(&format!("{id}.id"))],
        append_only: false,
        retention_purge: false,
        fields: vec![
            field(&format!("{id}.id"), "id", RuntimeValueKind::Integer),
            field(&format!("{id}.label"), "label", RuntimeValueKind::String),
        ],
        relations: vec![],
        indexes: vec![],
        composite_unique: vec![],
        default_order: vec![],
        label_fields: Vec::new(),
        search: None,
    }
}
/// Posts link to tags through `ra_post_tags`, so the module has every runtime
/// mutation root: `createPost`, `updatePost`, `deletePost` and `updatePostTagsLinks`.
fn definition() -> RuntimeSchema {
    let mut post = collection("post", "Post", "Posts");
    let tag = collection("tag", "Tag", "Tags");
    post.relations.push(RuntimeRelation {
        description: None,
        classification: None,
        export: None,
        id: RelationId::new("tags").unwrap(),
        api_name: "tags".into(),
        target: cid("tag"),
        key_pairs: vec![],
        cardinality: RelationCardinality::Many,
        enforce_foreign_key: false,
        on_delete: None,
        join: Some(RuntimeJoinTable {
            physical_table: "ra_post_tags".into(),
            source: vec![RuntimeJoinColumn {
                field: fid("post.id"),
                column: "post_id".into(),
            }],
            target: vec![RuntimeJoinColumn {
                field: fid("tag.id"),
                column: "tag_id".into(),
            }],
        }),
    });
    RuntimeSchema {
        repository_only: false,
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![post, tag],
    }
}

/// Grants every field; counts every write authorization it is asked for.
#[derive(Clone)]
struct Writes {
    schema: Arc<ValidatedRuntimeSchema>,
    intents: Arc<AtomicUsize>,
}
impl RuntimeWriteAuthority<Backend> for Writes {
    fn authorize_intent<'a>(
        &'a self,
        check: RuntimeWriteIntent<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeWriteGrant, RuntimeMutationError>> {
        Box::pin(async move {
            self.intents.fetch_add(1, Ordering::SeqCst);
            let handle = check.collection();
            let fields = self
                .schema
                .schema()
                .collections
                .iter()
                .find(|c| c.id == *handle.id())
                .unwrap()
                .fields
                .iter()
                .map(|f| self.schema.resolve_field(handle, &f.id).unwrap())
                .collect::<Vec<_>>();
            RuntimeWriteGrant::new(
                self.schema.resolve_projection(handle, &fields).unwrap(),
                None,
            )
        })
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
    fn link_target_visibility<'a>(
        &'a self,
        _: RuntimeLinkVisibilityCheck<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<Option<RuntimePredicate>, RuntimeMutationError>> {
        Box::pin(async { Ok(None) })
    }
    fn authorize_links<'a>(
        &'a self,
        _: RuntimeLinkCheck<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<(), RuntimeMutationError>> {
        Box::pin(async { Ok(()) })
    }
}

struct Reads;
impl RuntimeReadAuthority for Reads {
    fn authorize_mutation_result<'a>(
        &'a self,
        _check: RuntimeMutationOutputCheck<'a>,
    ) -> BoxFuture<'a, Result<(), RuntimeGraphqlError>> {
        Box::pin(async { Ok(()) })
    }
    fn authorize<'a>(
        &'a self,
        _check: RuntimeReadCheck<'a>,
    ) -> BoxFuture<'a, Result<RuntimeReadGrant, RuntimeGraphqlError>> {
        Box::pin(async { Ok(RuntimeReadGrant::new(None)) })
    }
}

/// Denies every requirement with `STEP_UP_REQUIRED`, recording the policy.
#[derive(Clone, Debug, Default)]
struct StepUp {
    policies: Arc<Mutex<Vec<String>>>,
}
impl AssuranceRequirementEvaluator for StepUp {
    fn enforce(
        &self,
        _ctx: &async_graphql::Context<'_>,
        actor_class: AssuranceActorClass,
        policy_id: &str,
    ) -> async_graphql::Result<()> {
        assert_eq!(actor_class, AssuranceActorClass::Interactive);
        self.policies.lock().unwrap().push(policy_id.to_owned());
        Err(
            async_graphql::Error::new("additional authentication is required")
                .extend_with(|_, extensions| extensions.set("code", "STEP_UP_REQUIRED")),
        )
    }
}

struct Fixture {
    path: std::path::PathBuf,
    db: Database<Backend>,
    environment: Arc<RuntimeMutationEnvironment>,
    intents: Arc<AtomicUsize>,
    host_calls: Arc<AtomicUsize>,
}
impl Fixture {
    async fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "graphql-orm-runtime-assurance-{}.sqlite",
            graphql_orm::uuid::Uuid::new_v4().simple()
        ));
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let db = Database::<Backend>::connect_sqlite_with_options(
            &url,
            graphql_orm::db::ConnectionOptions::default().max_connections(2),
        )
        .await
        .unwrap();
        let schema = Arc::new(definition().validate().unwrap());
        let target = schema
            .physical_schema::<Backend>(Default::default())
            .unwrap();
        let ownership = ManagedTableSet::new(schema.owned_table_names()).unwrap();
        let plan = db
            .schema()
            .plan_owned_migration(
                "init",
                "runtime assurance",
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
                .runtime_mutation_environment(schema, &target, &ownership)
                .await
                .unwrap(),
        );
        for sql in [
            "INSERT INTO ra_post (id, label) VALUES (1, 'first')",
            "INSERT INTO ra_tag (id, label) VALUES (10, 'red')",
        ] {
            Backend::fetch_rows(db.pool(), sql, &[]).await.unwrap();
        }
        Self {
            path,
            db,
            environment,
            intents: Default::default(),
            host_calls: Default::default(),
        }
    }
    fn schema(&self) -> &Arc<ValidatedRuntimeSchema> {
        self.environment.schema()
    }
    fn module(&self) -> RuntimeGraphqlModule {
        RuntimeGraphqlModule::compile(
            self.schema().clone(),
            RuntimeGraphqlOptions::default()
                .with_cursor_profile(RuntimeCursorProfile::Unprotected)
                .with_mutations(Default::default()),
        )
        .unwrap()
    }
    fn host_field(&self) -> RuntimeHostField {
        let calls = self.host_calls.clone();
        RuntimeHostField::new(
            "hostAction",
            TypeRef::named_nn(TypeRef::BOOLEAN),
            move |_| {
                let calls = calls.clone();
                FieldFuture::new(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(Some(FieldValue::value(true)))
                })
            },
        )
    }
    fn composer(&self) -> RuntimeGraphqlComposer<Backend> {
        RuntimeGraphqlComposer::new(self.db.clone(), "Query", Default::default())
            .unwrap()
            .mutation_root("Mutation")
            .unwrap()
            .install(self.module())
            .unwrap()
            .mutation_field(self.host_field())
            .unwrap()
    }
    async fn run(&self, api: &Schema, query: &str) -> Value {
        let schema = self.schema();
        let response = api
            .execute(
                Request::new(query)
                    .variables(Variables::from_json(
                        json!({"schema": schema.fingerprint().to_string()}),
                    ))
                    .data(
                        RuntimeGraphqlRequest::<Backend>::new(
                            schema.fingerprint(),
                            Arc::new(Reads),
                            None,
                            Default::default(),
                        )
                        .with_mutation_environment(self.environment.clone())
                        .with_write_authority(Arc::new(Writes {
                            schema: schema.clone(),
                            intents: self.intents.clone(),
                        })),
                    ),
            )
            .await;
        serde_json::to_value(&response).unwrap()
    }
    async fn count(&self, table: &str) -> i64 {
        let rows = Backend::fetch_rows(
            self.db.pool(),
            &format!("SELECT COUNT(*) AS n FROM {table}"),
            &[],
        )
        .await
        .unwrap();
        Backend::try_get_i64(&rows[0], "n").unwrap()
    }
    async fn label(&self) -> Option<String> {
        let rows = Backend::fetch_rows(
            self.db.pool(),
            "SELECT label FROM ra_post WHERE id = 1",
            &[],
        )
        .await
        .unwrap();
        rows.first()
            .map(|row| Backend::try_get_string(row, "label").unwrap())
    }
    async fn finish(self) {
        self.db.pool().close().await;
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.path.display()));
        }
    }
}

const MUTATIONS: &[(&str, &str)] = &[
    (
        "createPost",
        r#"mutation($schema: String!) { createPost(expectedSchema: $schema, input: {id: "2", label: "new"}) { record { id } } }"#,
    ),
    (
        "updatePost",
        r#"mutation($schema: String!) { updatePost(expectedSchema: $schema, key: {id: "1"}, input: {label: "changed"}) { record { id } } }"#,
    ),
    (
        "deletePost",
        r#"mutation($schema: String!) { deletePost(expectedSchema: $schema, key: {id: "1"}) { record { id } } }"#,
    ),
    (
        "updatePostTagsLinks",
        r#"mutation($schema: String!) { updatePostTagsLinks(expectedSchema: $schema, key: {id: "1"}, add: [{id: "10"}]) { record { id } } }"#,
    ),
    ("hostAction", "mutation { hostAction }"),
];

fn step_up_code(response: &Value) -> Option<&str> {
    response["errors"][0]["extensions"]["code"].as_str()
}

async fn assert_nothing_written(f: &Fixture, response: &Value, root: &str) {
    assert_eq!(
        step_up_code(response),
        Some("STEP_UP_REQUIRED"),
        "{root}: {response}"
    );
    assert_eq!(
        f.intents.load(Ordering::SeqCst),
        0,
        "{root} reached write authority"
    );
    assert_eq!(
        f.host_calls.load(Ordering::SeqCst),
        0,
        "{root} ran the host resolver"
    );
    assert_eq!(f.count("ra_post").await, 1, "{root} changed rows");
    assert_eq!(f.count("ra_post_tags").await, 0, "{root} changed links");
    assert_eq!(
        f.label().await.as_deref(),
        Some("first"),
        "{root} changed a row"
    );
}

/// A default interactive mutation policy covers runtime mutation roots even
/// when the registry was seeded only from its (here empty) static catalogue.
#[tokio::test]
async fn unregistered_runtime_mutations_apply_the_default_policy_before_any_write() {
    let f = Fixture::new().await;
    let evaluator = StepUp::default();
    let registry = OperationAssuranceRegistry::builder(
        &GraphqlOperationCatalog::compose(std::iter::empty()),
        AssuranceSchemaConfig::legacy()
            .with_default_interactive_mutation_policy(POLICY)
            .unwrap(),
    )
    .build()
    .unwrap();
    let api = f
        .composer()
        .data(AssuranceEnforcement::new(
            Arc::new(registry),
            Arc::new(evaluator.clone()),
        ))
        .finish()
        .unwrap();
    for (root, query) in MUTATIONS {
        let response = f.run(&api, query).await;
        assert_nothing_written(&f, &response, root).await;
    }
    assert_eq!(evaluator.policies.lock().unwrap().len(), MUTATIONS.len());
    assert!(
        evaluator
            .policies
            .lock()
            .unwrap()
            .iter()
            .all(|p| p == POLICY)
    );
    f.finish().await;
}

#[tokio::test]
async fn registered_runtime_roots_enforce_their_own_coordinates() {
    let f = Fixture::new().await;
    let evaluator = StepUp::default();
    let composer = f.composer();
    let mut builder = OperationAssuranceRegistryBuilder::new(
        AssuranceSchemaConfig::legacy()
            .with_default_interactive_mutation_policy(POLICY)
            .unwrap()
            .with_strict_mutation_classification(true),
    );
    composer
        .register_assurance_operations(&mut builder)
        .unwrap();
    builder
        .require(GraphqlOperationKind::Query, "posts", "read.recent-auth")
        .unwrap()
        .exempt(
            GraphqlOperationKind::Mutation,
            "hostAction",
            "host action is idempotent and harmless",
        )
        .unwrap();
    let registry = builder.build().unwrap();
    registry.audit().assert_complete();
    let manifest = registry.manifest();
    for (root, _) in MUTATIONS {
        let entry = manifest
            .operations
            .iter()
            .find(|entry| entry.field_name == *root)
            .unwrap_or_else(|| panic!("{root} is not registered"));
        assert_eq!(entry.operation_kind, GraphqlOperationKind::Mutation);
        let origin = if *root == "hostAction" {
            OperationAssuranceOrigin::Custom
        } else {
            OperationAssuranceOrigin::Runtime
        };
        assert_eq!(entry.origin, origin, "{root}");
    }
    assert!(
        manifest
            .operations
            .iter()
            .any(|entry| entry.field_coordinate == "Query.tags")
    );

    let api = composer
        .data(AssuranceEnforcement::new(
            Arc::new(registry),
            Arc::new(evaluator.clone()),
        ))
        .finish()
        .unwrap();
    for (root, query) in &MUTATIONS[..4] {
        let response = f.run(&api, query).await;
        assert_nothing_written(&f, &response, root).await;
    }
    // An explicit exemption runs; a required runtime query root is refused.
    let exempt = f.run(&api, "mutation { hostAction }").await;
    assert!(exempt.get("errors").is_none(), "{exempt}");
    assert_eq!(f.host_calls.load(Ordering::SeqCst), 1);
    let read = f.run(&api, "{ posts { edges { node { id } } } }").await;
    assert_eq!(step_up_code(&read), Some("STEP_UP_REQUIRED"), "{read}");
    assert_eq!(read["data"], Value::Null, "{read}");
    // An unclassified runtime query root keeps its legacy behaviour.
    let tags = f.run(&api, "{ tags { edges { node { label } } } }").await;
    assert!(tags.get("errors").is_none(), "{tags}");
    assert_eq!(
        evaluator.policies.lock().unwrap().as_slice(),
        [POLICY, POLICY, POLICY, POLICY, "read.recent-auth"]
    );
    f.finish().await;
}

#[tokio::test]
async fn strict_classification_refuses_unregistered_runtime_mutation_roots() {
    let f = Fixture::new().await;
    let strict = AssuranceSchemaConfig::legacy()
        .with_default_interactive_mutation_policy(POLICY)
        .unwrap()
        .with_strict_mutation_classification(true);
    // A strict registry that never saw the runtime roots cannot finish.
    let registry = OperationAssuranceRegistryBuilder::new(strict.clone())
        .build()
        .unwrap();
    let unregistered = f
        .composer()
        .data(AssuranceEnforcement::new(
            Arc::new(registry.clone()),
            Arc::new(StepUp::default()),
        ))
        .finish()
        .unwrap_err();
    assert_eq!(unregistered.code(), "assurance_unclassified");
    let missing = registry
        .ensure_mutation_roots_registered(MUTATIONS.iter().map(|(root, _)| *root))
        .unwrap_err();
    let AssuranceRegistryError::UnregisteredMutations(missing) = missing else {
        panic!("unexpected error");
    };
    assert_eq!(missing.len(), MUTATIONS.len());
    assert!(
        missing
            .iter()
            .any(|coordinate| coordinate.to_string() == "Mutation.updatePostTagsLinks")
    );

    // A registered host field needs an explicit exemption: the default only
    // covers interactive roots, so a service host field stays unclassified.
    let composer = f.composer();
    let mut builder = OperationAssuranceRegistryBuilder::new(strict);
    composer
        .register_assurance_operations(&mut builder)
        .unwrap();
    builder
        .set_actor_class(
            GraphqlOperationKind::Mutation,
            "hostAction",
            AssuranceActorClass::Service,
        )
        .unwrap();
    assert!(matches!(
        builder.clone().build(),
        Err(AssuranceRegistryError::IncompleteMutations(_))
    ));
    builder
        .exempt(
            GraphqlOperationKind::Mutation,
            "hostAction",
            "service host action",
        )
        .unwrap();
    let registry = builder.build().unwrap();
    composer
        .data(AssuranceEnforcement::new(
            Arc::new(registry),
            Arc::new(StepUp::default()),
        ))
        .finish()
        .unwrap();
    f.finish().await;
}
