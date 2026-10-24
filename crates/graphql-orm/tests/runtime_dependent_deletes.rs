//! Opt-in Cascade/SetNull dependents of runtime deletes. Each PostgreSQL case owns a
//! verified disposable server; SQLite uses a private temporary database file.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use futures::future::BoxFuture;
use graphql_orm::{graphql::errors::OrmPublicError, graphql::orm::*, prelude::*};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

const TABLES: [&str; 7] = [
    "dep_parents",
    "dep_children",
    "dep_grandchildren",
    "dep_notes",
    "dep_pins",
    "dep_nodes",
    "dep_marks",
];

/// Stable IDs are schema-wide, so each is prefixed with its collection.
fn field(collection: &str, name: &str, nullable: bool) -> RuntimeField {
    RuntimeField {
        description: None,
        classification: None,
        export: None,
        id: FieldId::new(format!("{collection}_{name}")).unwrap(),
        api_name: name.replace("_id", "Id"),
        physical_column: name.into(),
        value_kind: RuntimeValueKind::Integer,
        nullable,
        unique: false,
        filterable: true,
        sortable: true,
        generated: false,
        default: None,
        format: None,
    }
}
fn relation(
    collection: &str,
    name: &str,
    target: &str,
    source: &str,
    policy: DeletePolicy,
) -> RuntimeRelation {
    RuntimeRelation {
        description: None,
        classification: None,
        export: None,
        id: RelationId::new(format!("{collection}_{name}")).unwrap(),
        api_name: name.into(),
        target: CollectionId::new(target).unwrap(),
        key_pairs: vec![RelationKeyPair {
            source: FieldId::new(format!("{collection}_{source}")).unwrap(),
            target: FieldId::new(format!("{target}_id")).unwrap(),
        }],
        cardinality: RelationCardinality::One,
        enforce_foreign_key: true,
        on_delete: Some(policy),
    }
}
fn collection(
    id: &str,
    type_name: &str,
    fields: Vec<RuntimeField>,
    relations: Vec<RuntimeRelation>,
) -> RuntimeCollection {
    RuntimeCollection {
        description: None,
        classification: None,
        id: CollectionId::new(id).unwrap(),
        api_type_name: type_name.into(),
        api_plural_name: format!("{type_name}s"),
        physical_table: format!("dep_{id}"),
        primary_key: vec![FieldId::new(format!("{id}_id")).unwrap()],
        append_only: false,
        retention_purge: false,
        fields,
        relations,
        indexes: vec![],
        composite_unique: vec![],
        default_order: vec![],
    }
}
fn definition() -> RuntimeSchema {
    RuntimeSchema {
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![
            collection(
                "parents",
                "Parent",
                vec![field("parents", "id", false)],
                vec![],
            ),
            collection(
                "children",
                "Child",
                vec![
                    field("children", "id", false),
                    field("children", "parent_id", false),
                ],
                vec![relation(
                    "children",
                    "parent",
                    "parents",
                    "parent_id",
                    DeletePolicy::Cascade,
                )],
            ),
            collection(
                "grandchildren",
                "Grandchild",
                vec![
                    field("grandchildren", "id", false),
                    field("grandchildren", "child_id", false),
                ],
                vec![relation(
                    "grandchildren",
                    "child",
                    "children",
                    "child_id",
                    DeletePolicy::Cascade,
                )],
            ),
            collection(
                "notes",
                "Note",
                vec![
                    field("notes", "id", false),
                    field("notes", "parent_id", true),
                ],
                vec![relation(
                    "notes",
                    "parent",
                    "parents",
                    "parent_id",
                    DeletePolicy::SetNull,
                )],
            ),
            collection(
                "pins",
                "Pin",
                vec![
                    field("pins", "id", false),
                    field("pins", "parent_id", true),
                    field("pins", "child_id", true),
                ],
                vec![
                    relation(
                        "pins",
                        "parent",
                        "parents",
                        "parent_id",
                        DeletePolicy::Restrict,
                    ),
                    relation(
                        "pins",
                        "child",
                        "children",
                        "child_id",
                        DeletePolicy::Restrict,
                    ),
                ],
            ),
            // Reachable both directly and through a child: a diamond.
            collection(
                "marks",
                "Mark",
                vec![
                    field("marks", "id", false),
                    field("marks", "parent_id", true),
                    field("marks", "child_id", true),
                ],
                vec![
                    relation(
                        "marks",
                        "parent",
                        "parents",
                        "parent_id",
                        DeletePolicy::Cascade,
                    ),
                    relation(
                        "marks",
                        "child",
                        "children",
                        "child_id",
                        DeletePolicy::Cascade,
                    ),
                ],
            ),
            collection(
                "nodes",
                "Node",
                vec![
                    field("nodes", "id", false),
                    field("nodes", "parent_id", true),
                ],
                vec![relation(
                    "nodes",
                    "parent",
                    "nodes",
                    "parent_id",
                    DeletePolicy::Cascade,
                )],
            ),
        ],
    }
}

struct Fixture {
    db: Database<Backend>,
    environment: Arc<RuntimeMutationEnvironment>,
    url: String,
    #[cfg(feature = "postgres")]
    owned: owned_postgres::OwnedPostgres,
    #[cfg(feature = "sqlite")]
    path: std::path::PathBuf,
}
impl Fixture {
    async fn new() -> Self {
        Self::with_external(&[]).await
    }
    /// `external` runs after the owned migration, before the environment is built.
    async fn with_external(external: &[&str]) -> Self {
        #[cfg(feature = "sqlite")]
        let path = std::env::temp_dir().join(format!(
            "graphql-orm-dependent-deletes-{}.sqlite",
            graphql_orm::uuid::Uuid::new_v4().simple()
        ));
        #[cfg(feature = "sqlite")]
        let url = format!("sqlite://{}?mode=rwc", path.display());
        #[cfg(feature = "sqlite")]
        let db = Database::<Backend>::connect_sqlite_with_options(
            &url,
            graphql_orm::db::ConnectionOptions::default().max_connections(2),
        )
        .await
        .unwrap();
        #[cfg(feature = "postgres")]
        let owned = owned_postgres::OwnedPostgres::start("dependent-deletes").unwrap();
        #[cfg(feature = "postgres")]
        let url = owned.url.clone();
        #[cfg(feature = "postgres")]
        let db = Database::<Backend>::connect_postgres(&url).await.unwrap();
        let schema = Arc::new(definition().validate().unwrap());
        let target = schema
            .physical_schema::<Backend>(Default::default())
            .unwrap();
        let ownership = ManagedTableSet::new(TABLES.map(String::from)).unwrap();
        let plan = db
            .schema()
            .plan_owned_migration(
                "init",
                "dependent deletes",
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
        for sql in external {
            Backend::fetch_rows(db.pool(), sql, &[]).await.unwrap();
        }
        let environment = Arc::new(
            db.schema()
                .runtime_mutation_environment(schema, &target, &ownership)
                .await
                .unwrap(),
        );
        Self {
            db,
            environment,
            url,
            #[cfg(feature = "postgres")]
            owned,
            #[cfg(feature = "sqlite")]
            path,
        }
    }
    fn schema(&self) -> &ValidatedRuntimeSchema {
        self.environment.schema()
    }
    fn collection(&self, id: &str) -> RuntimeCollectionHandle {
        self.schema()
            .resolve_collection(&CollectionId::new(id).unwrap())
            .unwrap()
    }
    fn delete(
        &self,
        collection: &str,
        id: i64,
        dependents: Option<RuntimeDependentLimits>,
    ) -> RuntimeMutationRequest {
        let handle = self.collection(collection);
        let key_field = self
            .schema()
            .resolve_field(&handle, &FieldId::new(format!("{collection}_id")).unwrap())
            .unwrap();
        let key = self
            .schema()
            .runtime_key(&handle, &[(key_field, RuntimeValue::Integer(id))])
            .unwrap();
        let request = self
            .schema()
            .runtime_delete_request(key, None, None, Default::default())
            .unwrap();
        match dependents {
            Some(limits) => request.with_dependent_changes(limits).unwrap(),
            None => request,
        }
    }
    async fn sql(&self, sql: &str) {
        Backend::fetch_rows(self.db.pool(), sql, &[]).await.unwrap();
    }
    async fn count(&self, sql: &str) -> i64 {
        let rows = Backend::fetch_rows(self.db.pool(), sql, &[]).await.unwrap();
        Backend::try_get_i64(&rows[0], "n").unwrap()
    }
    /// Row counts of every table, so refusals can prove that nothing changed.
    async fn snapshot(&self) -> Vec<i64> {
        let mut counts = Vec::new();
        for table in TABLES {
            counts.push(
                self.count(&format!("SELECT COUNT(*) AS n FROM {table}"))
                    .await,
            );
        }
        counts.push(
            self.count("SELECT COUNT(*) AS n FROM dep_notes WHERE parent_id IS NULL")
                .await,
        );
        counts
    }
    async fn run(
        &self,
        request: RuntimeMutationRequest,
        authority: &Authority,
        hook: &Hook,
    ) -> Result<Vec<String>, TransactionError> {
        let environment = self.environment.clone();
        let authority = authority.clone();
        let hook = hook.clone();
        self.db
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let effect = tx
                        .mutate_runtime(&environment, &request, &authority)
                        .await
                        .map_err(OrmPublicError::from)?;
                    tx.run_runtime_before_commit(&hook, &effect)
                        .await
                        .map_err(OrmPublicError::from)?;
                    Ok(effect.dependents().iter().map(describe).collect())
                })
            })
            .await
    }
    async fn finish(self) {
        self.db.pool().close().await;
        #[cfg(feature = "postgres")]
        {
            let mut owned = self.owned;
            owned.cleanup().unwrap();
        }
        #[cfg(feature = "sqlite")]
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.path.display()));
        }
    }
    /// Standard data: parents 1 and 2; children 10, 11 of 1 and 12 of 2;
    /// grandchild 100 of 10; notes 50 (parent 1) and 51 (parent 2).
    async fn seed(&self) {
        for sql in [
            "INSERT INTO dep_parents (id) VALUES (1), (2)",
            "INSERT INTO dep_children (id, parent_id) VALUES (10, 1), (11, 1), (12, 2)",
            "INSERT INTO dep_grandchildren (id, child_id) VALUES (100, 10)",
            "INSERT INTO dep_notes (id, parent_id) VALUES (50, 1), (51, 2)",
        ] {
            self.sql(sql).await;
        }
    }
}

fn describe(effect: &RuntimeMutationEffect) -> String {
    let cause = effect.cause().expect("dependent cause");
    let key = match cause.referenced().fields()[0].1 {
        RuntimeValue::Integer(id) => id,
        _ => unreachable!(),
    };
    format!(
        "{:?}:{}:{}:{:?}<-{}:{}",
        effect.action(),
        effect.collection().id(),
        cause.relation().id(),
        cause.policy(),
        key,
        cause.depth()
    )
}

#[derive(Clone, Default)]
struct Pause {
    reached: Arc<Notify>,
    resume: Arc<Notify>,
}
#[derive(Clone)]
struct Authority {
    schema: Arc<ValidatedRuntimeSchema>,
    deny_preimage: Option<&'static str>,
    /// Grant predicate hiding one row of a collection from writes.
    hide: Option<(&'static str, i64)>,
    log: Arc<Mutex<Vec<String>>>,
    pause: Option<Pause>,
}
impl Authority {
    fn new(f: &Fixture) -> Self {
        Self {
            schema: f.environment.schema().clone(),
            deny_preimage: None,
            hide: None,
            log: Default::default(),
            pause: None,
        }
    }
    fn entry(&self, stage: &str, intent: RuntimeWriteIntent<'_>) {
        self.log.lock().unwrap().push(format!(
            "{stage}:{}:{}",
            intent.collection().id(),
            intent.cause().map_or(0, |cause| cause.depth())
        ));
    }
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}
impl RuntimeWriteAuthority<Backend> for Authority {
    fn authorize_intent<'a>(
        &'a self,
        check: RuntimeWriteIntent<'a>,
        tx: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeWriteGrant, RuntimeMutationError>> {
        Box::pin(async move {
            self.entry("intent", check);
            if let (Some(pause), None) = (&self.pause, check.cause()) {
                // Establish the transaction snapshot, then let a concurrent writer run.
                tx.fetch_rows("SELECT 1 AS one", &[]).await.unwrap();
                pause.reached.notify_one();
                pause.resume.notified().await;
            }
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
            let predicate = match self.hide {
                Some((collection, id)) if collection == handle.id().as_str() => Some(
                    self.schema
                        .runtime_compare(
                            handle,
                            &self
                                .schema
                                .resolve_field(
                                    handle,
                                    &FieldId::new(format!("{collection}_id")).unwrap(),
                                )
                                .unwrap(),
                            RuntimeScalarOperator::Ne,
                            RuntimeValue::Integer(id),
                            Default::default(),
                        )
                        .unwrap(),
                ),
                _ => None,
            };
            RuntimeWriteGrant::new(
                self.schema.resolve_projection(handle, &fields).unwrap(),
                predicate,
            )
        })
    }
    fn authorize_preimage<'a>(
        &'a self,
        check: RuntimePreimageCheck<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<(), RuntimeMutationError>> {
        Box::pin(async move {
            self.entry("preimage", check.intent);
            if self.deny_preimage == Some(check.intent.collection().id().as_str()) {
                return Err(RuntimeMutationError::new(RuntimeMutationErrorCode::Denied));
            }
            Ok(())
        })
    }
    fn authorize_result<'a>(
        &'a self,
        check: RuntimeResultCheck<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeReturnGrant, RuntimeMutationError>> {
        Box::pin(async move {
            self.entry("result", check.intent);
            Ok(RuntimeReturnGrant::new(check.intent.returning().cloned()))
        })
    }
}
#[derive(Clone, Default)]
struct Hook {
    seen: Arc<Mutex<Vec<String>>>,
    fail: bool,
}
impl RuntimeMutationHook<Backend> for Hook {
    fn before_commit<'a>(
        &'a self,
        _: &'a mut MutationContext<'_, Backend>,
        pending: &'a RuntimeMutationEffect,
    ) -> BoxFuture<'a, Result<(), RuntimeMutationError>> {
        Box::pin(async move {
            let mut seen = self.seen.lock().unwrap();
            seen.extend(pending.dependents().iter().map(describe));
            seen.push(format!(
                "{:?}:{}",
                pending.action(),
                pending.collection().id()
            ));
            if self.fail {
                return Err(RuntimeMutationError::new(
                    RuntimeMutationErrorCode::HookFailed,
                ));
            }
            Ok(())
        })
    }
}
fn code(error: &TransactionError) -> Option<&'static str> {
    error.public_error().runtime_mutation_code()
}

#[tokio::test]
async fn cascade_set_null_and_multi_level_dependents_are_authorized_listed_and_applied() {
    let f = Fixture::new().await;
    f.seed().await;
    let authority = Authority::new(&f);
    let hook = Hook::default();
    let dependents = f
        .run(
            f.delete("parents", 1, Some(RuntimeDependentLimits::default())),
            &authority,
            &hook,
        )
        .await
        .unwrap();
    // SetNull first, then every row after all rows referencing it.
    assert_eq!(dependents.len(), 4);
    assert_eq!(dependents[0], "Update:notes:notes_parent:SetNull<-1:1");
    let grandchild = dependents
        .iter()
        .position(|d| d.starts_with("Delete:grandchildren"))
        .unwrap();
    assert_eq!(
        dependents[grandchild],
        "Delete:grandchildren:grandchildren_child:Cascade<-10:2"
    );
    let children = dependents
        .iter()
        .enumerate()
        .filter(|(_, d)| d.starts_with("Delete:children:children_parent:Cascade<-1:1"))
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    assert_eq!(children.len(), 2);
    assert!(children.iter().any(|i| *i > grandchild));
    // Hooks see every dependent change and then the requested delete.
    let mut expected = dependents.clone();
    expected.push("Delete:parents".into());
    assert_eq!(*hook.seen.lock().unwrap(), expected);
    // Every intent and preimage check precedes the first write's result check.
    let log = authority.log();
    let first_result = log.iter().position(|e| e.starts_with("result:")).unwrap();
    assert_eq!(
        log[..first_result]
            .iter()
            .filter(|e| e.starts_with("preimage:"))
            .count(),
        5
    );
    assert_eq!(log.iter().filter(|e| e.starts_with("result:")).count(), 5);
    assert_eq!(f.snapshot().await, vec![1, 1, 0, 2, 0, 0, 0, 1]);
    assert_eq!(
        f.count("SELECT COUNT(*) AS n FROM dep_notes WHERE id = 51 AND parent_id = 2")
            .await,
        1
    );
    f.finish().await;
}

#[tokio::test]
async fn deletes_without_opt_in_keep_refusing_incoming_modifying_actions() {
    let f = Fixture::new().await;
    f.seed().await;
    let before = f.snapshot().await;
    let error = f
        .run(
            f.delete("parents", 2, None),
            &Authority::new(&f),
            &Hook::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("unsupported_cascade"));
    assert_eq!(f.snapshot().await, before);
    // Only deletes may opt in.
    let handle = f.collection("notes");
    let field = |name: &str| {
        f.schema()
            .resolve_field(&handle, &FieldId::new(format!("notes_{name}")).unwrap())
            .unwrap()
    };
    let key = f
        .schema()
        .runtime_key(&handle, &[(field("id"), RuntimeValue::Integer(50))])
        .unwrap();
    let update = f
        .schema()
        .runtime_update_request(
            key,
            &[(field("parent_id"), RuntimeValue::Null)],
            None,
            None,
            Default::default(),
        )
        .unwrap()
        .with_dependent_changes(RuntimeDependentLimits::default())
        .unwrap_err();
    assert_eq!(update.code(), RuntimeMutationErrorCode::InvalidInput);
    f.finish().await;
}

#[tokio::test]
async fn restrict_reference_refuses_before_any_dependent_authorization_or_write() {
    let f = Fixture::new().await;
    f.seed().await;
    f.sql("INSERT INTO dep_pins (id, parent_id) VALUES (70, 1)")
        .await;
    let before = f.snapshot().await;
    let authority = Authority::new(&f);
    let error = f
        .run(
            f.delete("parents", 1, Some(RuntimeDependentLimits::default())),
            &authority,
            &Hook::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("constraint_violation"));
    assert_eq!(error.commit_outcome(), TransactionCommitOutcome::RolledBack);
    assert_eq!(f.snapshot().await, before);
    assert_eq!(authority.log(), ["intent:parents:0", "preimage:parents:0"]);
    f.finish().await;
}

#[tokio::test]
async fn denied_dependent_row_refuses_the_whole_delete() {
    let f = Fixture::new().await;
    f.seed().await;
    let before = f.snapshot().await;
    let authority = Authority {
        deny_preimage: Some("grandchildren"),
        ..Authority::new(&f)
    };
    let error = f
        .run(
            f.delete("parents", 1, Some(RuntimeDependentLimits::default())),
            &authority,
            &Hook::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("denied"));
    assert_eq!(f.snapshot().await, before);
    assert!(!authority.log().iter().any(|e| e.starts_with("result:")));
    f.finish().await;
}

#[tokio::test]
async fn row_and_depth_limits_refuse_before_dependent_callbacks() {
    let f = Fixture::new().await;
    f.seed().await;
    let before = f.snapshot().await;
    for limits in [
        RuntimeDependentLimits::new(3, 8).unwrap(),
        RuntimeDependentLimits::new(256, 1).unwrap(),
    ] {
        let authority = Authority::new(&f);
        let error = f
            .run(
                f.delete("parents", 1, Some(limits)),
                &authority,
                &Hook::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(code(&error), Some("limit_exceeded"));
        assert_eq!(authority.log(), ["intent:parents:0", "preimage:parents:0"]);
        assert_eq!(f.snapshot().await, before);
    }
    // The exact bounds succeed.
    f.run(
        f.delete(
            "parents",
            1,
            Some(RuntimeDependentLimits::new(4, 2).unwrap()),
        ),
        &Authority::new(&f),
        &Hook::default(),
    )
    .await
    .unwrap();
    f.finish().await;
}

#[tokio::test]
async fn self_referencing_chains_cascade_and_data_cycles_refuse() {
    let f = Fixture::new().await;
    for sql in [
        "INSERT INTO dep_nodes (id, parent_id) VALUES (1, NULL)",
        "INSERT INTO dep_nodes (id, parent_id) VALUES (2, 1)",
        "INSERT INTO dep_nodes (id, parent_id) VALUES (3, 2)",
        "INSERT INTO dep_nodes (id, parent_id) VALUES (5, NULL)",
        "INSERT INTO dep_nodes (id, parent_id) VALUES (6, 5)",
        "UPDATE dep_nodes SET parent_id = 6 WHERE id = 5",
    ] {
        f.sql(sql).await;
    }
    let dependents = f
        .run(
            f.delete("nodes", 1, Some(RuntimeDependentLimits::default())),
            &Authority::new(&f),
            &Hook::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        dependents,
        [
            "Delete:nodes:nodes_parent:Cascade<-2:2",
            "Delete:nodes:nodes_parent:Cascade<-1:1"
        ]
    );
    assert_eq!(f.count("SELECT COUNT(*) AS n FROM dep_nodes").await, 2);
    let error = f
        .run(
            f.delete("nodes", 5, Some(RuntimeDependentLimits::default())),
            &Authority::new(&f),
            &Hook::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("unsupported_cascade"));
    assert_eq!(f.count("SELECT COUNT(*) AS n FROM dep_nodes").await, 2);
    f.finish().await;
}

#[tokio::test]
async fn hook_failure_rolls_back_every_dependent_change() {
    let f = Fixture::new().await;
    f.seed().await;
    let before = f.snapshot().await;
    let hook = Hook {
        fail: true,
        ..Default::default()
    };
    let error = f
        .run(
            f.delete("parents", 1, Some(RuntimeDependentLimits::default())),
            &Authority::new(&f),
            &hook,
        )
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("hook_failed"));
    assert_eq!(error.commit_outcome(), TransactionCommitOutcome::RolledBack);
    assert_eq!(hook.seen.lock().unwrap().len(), 5);
    assert_eq!(f.snapshot().await, before);
    f.finish().await;
}

#[tokio::test]
async fn concurrent_referencing_insert_cannot_escape_authorization() {
    let f = Fixture::new().await;
    f.sql("INSERT INTO dep_parents (id) VALUES (1)").await;
    let pause = Pause::default();
    let authority = Authority {
        pause: Some(pause.clone()),
        ..Authority::new(&f)
    };
    let hook = Hook::default();
    let delete = f.run(
        f.delete("parents", 1, Some(RuntimeDependentLimits::default())),
        &authority,
        &hook,
    );
    let url = f.url.clone();
    let writer = async move {
        pause.reached.notified().await;
        #[cfg(feature = "postgres")]
        let inserted = {
            let pool = graphql_orm::sqlx::PgPool::connect(&url).await.unwrap();
            let result =
                graphql_orm::sqlx::query("INSERT INTO dep_children (id, parent_id) VALUES (10, 1)")
                    .execute(&pool)
                    .await;
            pool.close().await;
            result
        };
        #[cfg(feature = "sqlite")]
        let inserted = {
            use std::str::FromStr;
            let options = graphql_orm::sqlx::sqlite::SqliteConnectOptions::from_str(&url)
                .unwrap()
                .foreign_keys(true)
                .busy_timeout(std::time::Duration::from_millis(200));
            let pool = graphql_orm::sqlx::SqlitePool::connect_with(options)
                .await
                .unwrap();
            let result =
                graphql_orm::sqlx::query("INSERT INTO dep_children (id, parent_id) VALUES (10, 1)")
                    .execute(&pool)
                    .await;
            pool.close().await;
            result
        };
        pause.resume.notify_one();
        inserted.is_ok()
    };
    let (deleted, inserted) = tokio::join!(delete, writer);
    #[cfg(feature = "postgres")]
    {
        // The unseen child committed after the delete's snapshot. The final delete
        // fails as a retryable serialization failure instead of cascading silently.
        assert!(inserted);
        assert_eq!(code(&deleted.unwrap_err()), Some("retryable_transaction"));
        assert_eq!(f.count("SELECT COUNT(*) AS n FROM dep_parents").await, 1);
        assert_eq!(f.count("SELECT COUNT(*) AS n FROM dep_children").await, 1);
    }
    #[cfg(feature = "sqlite")]
    {
        // The immediate write transaction excludes the concurrent writer entirely.
        assert!(!inserted);
        assert!(deleted.unwrap().is_empty());
        assert_eq!(f.count("SELECT COUNT(*) AS n FROM dep_parents").await, 0);
        assert_eq!(f.count("SELECT COUNT(*) AS n FROM dep_children").await, 0);
    }
    f.finish().await;
}

#[test]
fn dependent_limits_are_bounded_and_opaque() {
    assert!(
        RuntimeDependentLimits::new(RUNTIME_DEPENDENT_MAX_ROWS, RUNTIME_DEPENDENT_MAX_DEPTH)
            .is_ok()
    );
    for (rows, depth) in [
        (RUNTIME_DEPENDENT_MAX_ROWS + 1, 1),
        (usize::MAX, 1),
        (1, RUNTIME_DEPENDENT_MAX_DEPTH + 1),
    ] {
        assert_eq!(
            RuntimeDependentLimits::new(rows, depth).unwrap_err().code(),
            RuntimeMutationErrorCode::LimitExceeded
        );
    }
    let defaults = RuntimeDependentLimits::default();
    assert_eq!((defaults.max_rows(), defaults.max_depth()), (256, 8));
}

#[tokio::test]
async fn hidden_and_denied_dependents_refuse_the_whole_delete() {
    let f = Fixture::new().await;
    f.seed().await;
    let before = f.snapshot().await;
    // A cascaded row outside the write grant's predicate is a denial: the row
    // exists and is locked. The refusal reveals that some hidden dependent exists.
    let hidden = Authority {
        hide: Some(("children", 11)),
        ..Authority::new(&f)
    };
    let error = f
        .run(
            f.delete("parents", 1, Some(RuntimeDependentLimits::default())),
            &hidden,
            &Hook::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("denied"));
    // A denied SetNull dependent refuses too.
    let denied = Authority {
        deny_preimage: Some("notes"),
        ..Authority::new(&f)
    };
    let error = f
        .run(
            f.delete("parents", 1, Some(RuntimeDependentLimits::default())),
            &denied,
            &Hook::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("denied"));
    assert!(!denied.log().iter().any(|e| e.starts_with("result:")));
    assert_eq!(f.snapshot().await, before);
    f.finish().await;
}

#[tokio::test]
async fn restrict_on_a_cascaded_row_refuses_before_writes() {
    let f = Fixture::new().await;
    f.seed().await;
    f.sql("INSERT INTO dep_pins (id, parent_id, child_id) VALUES (70, NULL, 10)")
        .await;
    let before = f.snapshot().await;
    let authority = Authority::new(&f);
    let error = f
        .run(
            f.delete("parents", 1, Some(RuntimeDependentLimits::default())),
            &authority,
            &Hook::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("constraint_violation"));
    assert_eq!(authority.log(), ["intent:parents:0", "preimage:parents:0"]);
    assert_eq!(f.snapshot().await, before);
    f.finish().await;
}

#[tokio::test]
async fn modifying_foreign_keys_from_non_runtime_tables_refuse() {
    let f = Fixture::with_external(&[
        "CREATE TABLE dep_external (id BIGINT PRIMARY KEY, parent_id BIGINT REFERENCES dep_parents (id) ON DELETE CASCADE)",
    ])
    .await;
    f.seed().await;
    let error = f
        .run(
            f.delete("parents", 2, Some(RuntimeDependentLimits::default())),
            &Authority::new(&f),
            &Hook::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("unsupported_cascade"));
    assert_eq!(f.count("SELECT COUNT(*) AS n FROM dep_parents").await, 2);
    f.finish().await;
}

#[tokio::test]
async fn diamond_dependents_use_minimum_depth_and_a_deterministic_order() {
    let f = Fixture::new().await;
    f.seed().await;
    // Mark 1 references parent 2 directly and through child 12.
    f.sql("INSERT INTO dep_marks (id, parent_id, child_id) VALUES (1, 2, 12), (2, NULL, 12)")
        .await;
    for _ in 0..2 {
        let dependents = f
            .run(
                f.delete(
                    "parents",
                    2,
                    Some(RuntimeDependentLimits::new(256, 1).unwrap()),
                ),
                &Authority::new(&f),
                &Hook::default(),
            )
            .await;
        // Mark 2 is only reachable through child 12, at depth 2.
        assert_eq!(code(&dependents.unwrap_err()), Some("limit_exceeded"));
    }
    f.sql("DELETE FROM dep_marks WHERE id = 2").await;
    let dependents = f
        .run(
            f.delete(
                "parents",
                2,
                Some(RuntimeDependentLimits::new(256, 1).unwrap()),
            ),
            &Authority::new(&f),
            &Hook::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        dependents,
        [
            "Update:notes:notes_parent:SetNull<-2:1",
            "Delete:marks:marks_parent:Cascade<-2:1",
            "Delete:children:children_parent:Cascade<-2:1",
        ]
    );
    f.finish().await;
}

/// A concurrent writer holds an uncommitted change that references the parent
/// when the delete starts, then commits while the delete waits for it.
#[tokio::test]
async fn uncommitted_concurrent_references_cannot_escape_authorization() {
    for (label, sql) in [
        (
            "insert",
            "INSERT INTO dep_children (id, parent_id) VALUES (13, 1)",
        ),
        (
            "reparent",
            "UPDATE dep_children SET parent_id = 1 WHERE id = 12",
        ),
        (
            "set-null insert",
            "INSERT INTO dep_notes (id, parent_id) VALUES (52, 1)",
        ),
    ] {
        let f = Fixture::new().await;
        f.seed().await;
        let started = Arc::new(Notify::new());
        let url = f.url.clone();
        let writer = {
            let started = started.clone();
            async move {
                #[cfg(feature = "postgres")]
                let pool = graphql_orm::sqlx::PgPool::connect(&url).await.unwrap();
                #[cfg(feature = "sqlite")]
                let pool = {
                    use std::str::FromStr;
                    graphql_orm::sqlx::SqlitePool::connect_with(
                        graphql_orm::sqlx::sqlite::SqliteConnectOptions::from_str(&url)
                            .unwrap()
                            .foreign_keys(true),
                    )
                    .await
                    .unwrap()
                };
                #[cfg(feature = "postgres")]
                let mut tx = pool.begin().await.unwrap();
                #[cfg(feature = "sqlite")]
                let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
                graphql_orm::sqlx::query(sql)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
                started.notify_one();
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                tx.commit().await.unwrap();
                pool.close().await;
            }
        };
        let authority = Authority::new(&f);
        let hook = Hook::default();
        let delete = async {
            started.notified().await;
            f.run(
                f.delete("parents", 1, Some(RuntimeDependentLimits::default())),
                &authority,
                &hook,
            )
            .await
        };
        let ((), deleted) = tokio::join!(writer, delete);
        #[cfg(feature = "postgres")]
        {
            // The delete waits on the row lock; the committed change is outside its
            // snapshot, so the final delete's own action fails as retryable.
            assert_eq!(
                code(&deleted.unwrap_err()),
                Some("retryable_transaction"),
                "{label}"
            );
            assert_eq!(
                f.count("SELECT COUNT(*) AS n FROM dep_parents").await,
                2,
                "{label}"
            );
        }
        #[cfg(feature = "sqlite")]
        {
            // The immediate transaction starts after the writer commits and sees the
            // change, so the new dependent is authorized and listed like any other.
            let dependents = deleted.unwrap();
            let listed = match label {
                "insert" => "Delete:children:children_parent:Cascade<-1:1",
                "reparent" => "Delete:children:children_parent:Cascade<-1:1",
                _ => "Update:notes:notes_parent:SetNull<-1:1",
            };
            assert!(
                dependents.iter().any(|d| d == listed),
                "{label}: {dependents:?}"
            );
            assert_eq!(
                f.count("SELECT COUNT(*) AS n FROM dep_parents").await,
                1,
                "{label}"
            );
            assert_eq!(
                f.count("SELECT COUNT(*) AS n FROM dep_children WHERE parent_id = 1")
                    .await,
                0,
                "{label}"
            );
        }
        f.finish().await;
    }
}

#[cfg(feature = "runtime-graphql")]
mod graphql {
    use super::*;
    use graphql_orm::async_graphql::{Request, Variables, dynamic::Schema};
    use graphql_orm::graphql::runtime::*;
    use serde_json::{Value, json};

    struct Read;
    impl RuntimeReadAuthority for Read {
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

    fn api(f: &Fixture, dependents: Option<RuntimeDependentLimits>) -> Schema {
        let mut mutations = RuntimeGraphqlMutationOptions::default();
        // Parents have no non-key fields to patch.
        mutations.update = false;
        mutations.dependent_changes = dependents;
        let module = RuntimeGraphqlModule::compile(
            f.environment.schema().clone(),
            RuntimeGraphqlOptions::default()
                .with_cursor_profile(RuntimeCursorProfile::Unprotected)
                .with_mutations(mutations),
        )
        .unwrap();
        RuntimeGraphqlComposer::new(f.db.clone(), "Query", Default::default())
            .unwrap()
            .mutation_root("Mutation")
            .unwrap()
            .install(module)
            .unwrap()
            .finish()
            .unwrap()
    }

    async fn delete_parent(f: &Fixture, api: &Schema, hook: &Hook, id: i64) -> Value {
        let schema = f.environment.schema();
        let request = Request::new(
            r#"mutation($schema: String!, $key: ParentKeyInput!) {
                deleteParent(expectedSchema: $schema, key: $key) { record { id } }
            }"#,
        )
        .variables(Variables::from_json(
            json!({"schema": schema.fingerprint().to_string(), "key": {"id": id.to_string()}}),
        ))
        .data(
            RuntimeGraphqlRequest::<Backend>::new(
                schema.fingerprint(),
                Arc::new(Read),
                None,
                Default::default(),
            )
            .with_mutation_environment(f.environment.clone())
            .with_write_authority(Arc::new(Authority::new(f)))
            .with_mutation_hook(Arc::new(hook.clone())),
        );
        serde_json::to_value(api.execute(request).await).unwrap()
    }

    #[tokio::test]
    async fn generated_deletes_honour_dependents_only_when_opted_in() {
        let f = Fixture::new().await;
        f.seed().await;
        let before = f.snapshot().await;
        let hook = Hook::default();
        let refused = delete_parent(&f, &api(&f, None), &hook, 1).await;
        assert!(
            refused.to_string().contains("unsupported_cascade"),
            "{refused}"
        );
        assert_eq!(f.snapshot().await, before);

        let deleted = delete_parent(
            &f,
            &api(&f, Some(RuntimeDependentLimits::default())),
            &hook,
            1,
        )
        .await;
        assert_eq!(
            deleted["data"]["deleteParent"]["record"]["id"],
            json!("1"),
            "{deleted}"
        );
        assert_eq!(hook.seen.lock().unwrap().len(), 5);
        assert_eq!(f.snapshot().await, vec![1, 1, 0, 2, 0, 0, 0, 1]);
        f.finish().await;
    }
}
