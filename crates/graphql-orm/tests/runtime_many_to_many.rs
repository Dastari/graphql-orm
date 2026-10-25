//! Runtime many-to-many relations: IR, planning, link writes, deletes and
//! batched GraphQL reads. Each PostgreSQL case owns a verified disposable server.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use futures::future::BoxFuture;
use graphql_orm::{graphql::errors::OrmPublicError, graphql::orm::*, prelude::*};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

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
    }
}
fn collection(id: &str, name: &str, plural: &str) -> RuntimeCollection {
    RuntimeCollection {
        description: None,
        classification: None,
        id: cid(id),
        api_type_name: name.into(),
        api_plural_name: plural.into(),
        physical_table: format!("mm_{id}"),
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
    }
}
fn join(
    id: &str,
    target: &str,
    table: &str,
    source: (&str, &str),
    target_side: (&str, &str),
) -> RuntimeRelation {
    RuntimeRelation {
        description: None,
        classification: None,
        export: None,
        id: RelationId::new(id).unwrap(),
        api_name: id.into(),
        target: cid(target),
        key_pairs: vec![],
        cardinality: RelationCardinality::Many,
        enforce_foreign_key: false,
        on_delete: None,
        join: Some(RuntimeJoinTable {
            physical_table: table.into(),
            source: vec![RuntimeJoinColumn {
                field: fid(source.0),
                column: source.1.into(),
            }],
            target: vec![RuntimeJoinColumn {
                field: fid(target_side.0),
                column: target_side.1.into(),
            }],
        }),
    }
}
/// Posts and tags linked both ways through `mm_post_tags`; nodes link to nodes.
fn definition() -> RuntimeSchema {
    let mut post = collection("post", "Post", "Posts");
    let mut tag = collection("tag", "Tag", "Tags");
    let mut node = collection("node", "Node", "Nodes");
    post.relations.push(join(
        "tags",
        "tag",
        "mm_post_tags",
        ("post.id", "post_id"),
        ("tag.id", "tag_id"),
    ));
    tag.relations.push(join(
        "posts",
        "post",
        "mm_post_tags",
        ("tag.id", "tag_id"),
        ("post.id", "post_id"),
    ));
    node.relations.push(join(
        "links",
        "node",
        "mm_node_links",
        ("node.id", "from_id"),
        ("node.id", "to_id"),
    ));
    RuntimeSchema {
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![post, tag, node],
    }
}

#[test]
fn join_relations_are_validated_fingerprinted_and_planned() {
    use RuntimeSchemaDiagnosticCode as C;
    let schema = definition().validate().unwrap();
    assert_eq!(
        schema.owned_table_names(),
        [
            "mm_post",
            "mm_tag",
            "mm_node",
            "mm_post_tags",
            "mm_node_links"
        ]
    );
    let bytes = String::from_utf8(schema.canonical_bytes()).unwrap();
    assert!(
        bytes.contains("|join=mm_post_tags|join_source=post.id=post_id|join_target=tag.id=tag_id")
    );
    let mut renamed = definition();
    renamed.collections[0].relations[0]
        .join
        .as_mut()
        .unwrap()
        .physical_table = "mm_other".into();
    renamed.collections[1].relations[0]
        .join
        .as_mut()
        .unwrap()
        .physical_table = "mm_other".into();
    let renamed = renamed.validate().unwrap();
    assert_ne!(renamed.fingerprint(), schema.fingerprint());
    assert_ne!(
        renamed.structural_fingerprint(),
        schema.structural_fingerprint()
    );

    let codes = |schema: RuntimeSchema| {
        schema
            .validate()
            .unwrap_err()
            .into_diagnostics()
            .into_iter()
            .map(|d| d.code)
            .collect::<Vec<_>>()
    };
    // Key pairs, a foreign key or one cardinality are not join shapes.
    let mut shaped = definition();
    shaped.collections[2].relations[0].cardinality = RelationCardinality::One;
    assert_eq!(codes(shaped), [C::InvalidJoinTable]);
    // Columns must cover the primary key.
    let mut uncovered = definition();
    uncovered.collections[2].relations[0]
        .join
        .as_mut()
        .unwrap()
        .target[0]
        .field = fid("node.label");
    assert_eq!(codes(uncovered), [C::InvalidJoinTable]);
    // Names must not collide with collection tables, and columns must be distinct.
    let mut collision = definition();
    collision.collections[2].relations[0]
        .join
        .as_mut()
        .unwrap()
        .physical_table = "mm_post".into();
    assert_eq!(codes(collision), [C::InvalidJoinTable]);
    let mut columns = definition();
    columns.collections[2].relations[0]
        .join
        .as_mut()
        .unwrap()
        .target[0]
        .column = "from_id".into();
    assert_eq!(codes(columns), [C::InvalidJoinTable]);
    // Sharing requires exact inverses.
    let mut shared = definition();
    shared.collections[1].relations[0]
        .join
        .as_mut()
        .unwrap()
        .source[0]
        .column = "other".into();
    assert_eq!(codes(shared), [C::InvalidJoinTable, C::InvalidJoinTable]);
    // Serialization omits the member for ordinary relations.
    let mut plain = definition();
    plain.collections[2].relations.clear();
    let value = serde_json::to_value(&plain).unwrap();
    assert!(
        value["collections"][0]["relations"][0]
            .get("join")
            .is_some()
    );
    let restored: RuntimeSchema = serde_json::from_value(value).unwrap();
    assert_eq!(restored, plain);

    let target = schema
        .physical_schema::<Backend>(Default::default())
        .unwrap();
    let link = target
        .tables()
        .iter()
        .find(|t| t.table_name() == "mm_post_tags")
        .unwrap();
    assert_eq!(link.primary_keys(), ["post_id", "tag_id"]);
    assert_eq!(link.foreign_keys().len(), 2);
    assert!(
        link.foreign_keys()
            .iter()
            .all(|fk| fk.on_delete == DeletePolicy::Cascade)
    );
    assert!(
        link.indexes()
            .iter()
            .any(|index| index.columns == ["tag_id".to_string()])
    );
}

#[derive(Clone)]
struct Authority {
    schema: Arc<ValidatedRuntimeSchema>,
    allow_links: bool,
    log: Arc<Mutex<Vec<String>>>,
    /// Pause inside `authorize_links` so a concurrent writer can run.
    pause: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    /// Hide `secret` link targets through `link_target_visibility`.
    hide_secret_targets: bool,
    /// Hide source rows with this label through the write grant.
    hidden_source: Option<&'static str>,
    /// Deny link mutations naming any of these target ids.
    deny_targets: Vec<i64>,
    /// Requested targets seen by `authorize_links`.
    requested: Arc<Mutex<Vec<String>>>,
}
impl Authority {
    fn label_ne(&self, handle: &RuntimeCollectionHandle, label: &str) -> RuntimePredicate {
        let field = self
            .schema
            .resolve_field(handle, &fid(&format!("{}.label", handle.id().as_str())))
            .unwrap();
        self.schema
            .runtime_compare(
                handle,
                &field,
                RuntimeScalarOperator::Ne,
                RuntimeValue::String(label.into()),
                Default::default(),
            )
            .unwrap()
    }
}
/// Integer and text keys print as-is; UUID keys by their last four hex digits.
fn show(key: &RuntimeKey) -> String {
    match &key.fields()[0].1 {
        RuntimeValue::Integer(id) => id.to_string(),
        RuntimeValue::String(text) => text.clone(),
        RuntimeValue::Uuid(id) => id.simple().to_string()[28..].to_string(),
        _ => unreachable!(),
    }
}
fn describe(link: &RuntimeLink) -> String {
    format!("{}-{}", show(link.source()), show(link.target()))
}
impl RuntimeWriteAuthority<Backend> for Authority {
    fn authorize_intent<'a>(
        &'a self,
        check: RuntimeWriteIntent<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeWriteGrant, RuntimeMutationError>> {
        Box::pin(async move {
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
                self.hidden_source.map(|label| self.label_ne(handle, label)),
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
        check: RuntimeLinkVisibilityCheck<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<Option<RuntimePredicate>, RuntimeMutationError>> {
        Box::pin(async move {
            Ok(self
                .hide_secret_targets
                .then(|| self.label_ne(check.relation.target(), "secret")))
        })
    }
    fn authorize_links<'a>(
        &'a self,
        check: RuntimeLinkCheck<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<(), RuntimeMutationError>> {
        Box::pin(async move {
            let ids = |keys: &[RuntimeKey]| keys.iter().map(show).collect::<Vec<_>>().join(",");
            self.requested.lock().unwrap().push(format!(
                "+[{}]-[{}]",
                ids(check.requested_add),
                ids(check.requested_remove)
            ));
            if check
                .requested_add
                .iter()
                .chain(check.requested_remove)
                .any(|k| self.deny_targets.iter().any(|id| id.to_string() == show(k)))
            {
                return Err(RuntimeMutationError::new(RuntimeMutationErrorCode::Denied));
            }
            self.log.lock().unwrap().push(format!(
                "{}:+[{}]-[{}]",
                check.relation.id(),
                check
                    .added
                    .iter()
                    .map(describe)
                    .collect::<Vec<_>>()
                    .join(","),
                check
                    .removed
                    .iter()
                    .map(describe)
                    .collect::<Vec<_>>()
                    .join(","),
            ));
            if let Some((reached, resume)) = &self.pause {
                reached.notify_one();
                resume.notified().await;
            }
            if self.allow_links {
                Ok(())
            } else {
                Err(RuntimeMutationError::new(RuntimeMutationErrorCode::Denied))
            }
        })
    }
}
/// The engine's default refusal: authorities that do not opt in to links.
struct NoLinks(Authority);
impl RuntimeWriteAuthority<Backend> for NoLinks {
    fn authorize_intent<'a>(
        &'a self,
        check: RuntimeWriteIntent<'a>,
        tx: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeWriteGrant, RuntimeMutationError>> {
        self.0.authorize_intent(check, tx)
    }
    fn authorize_preimage<'a>(
        &'a self,
        check: RuntimePreimageCheck<'a>,
        tx: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<(), RuntimeMutationError>> {
        self.0.authorize_preimage(check, tx)
    }
    fn authorize_result<'a>(
        &'a self,
        check: RuntimeResultCheck<'a>,
        tx: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeReturnGrant, RuntimeMutationError>> {
        self.0.authorize_result(check, tx)
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
            for effect in pending.links() {
                self.seen.lock().unwrap().push(format!(
                    "{}:+{}-{}",
                    effect.relation().id(),
                    effect.added().len(),
                    effect.removed().len()
                ));
            }
            if self.fail {
                return Err(RuntimeMutationError::new(
                    RuntimeMutationErrorCode::HookFailed,
                ));
            }
            Ok(())
        })
    }
}

struct Fixture {
    url: String,
    #[cfg(feature = "sqlite")]
    path: std::path::PathBuf,
    db: Database<Backend>,
    environment: Arc<RuntimeMutationEnvironment>,
    target: OwnedSchemaModel,
    ownership: ManagedTableSet,
    #[cfg(feature = "postgres")]
    owned: owned_postgres::OwnedPostgres,
}
impl Fixture {
    async fn new() -> Self {
        Self::with_definition(
            definition(),
            &[
                "INSERT INTO mm_post (id, label) VALUES (1, 'first'), (2, 'second')",
                "INSERT INTO mm_tag (id, label) VALUES (10, 'red'), (11, 'green'), (12, 'blue'), (13, 'secret')",
                "INSERT INTO mm_node (id, label) VALUES (1, 'a'), (2, 'b')",
            ],
        )
        .await
    }
    async fn with_definition(definition: RuntimeSchema, seed: &[&str]) -> Self {
        #[cfg(feature = "sqlite")]
        let path = std::env::temp_dir().join(format!(
            "graphql-orm-many-to-many-{}.sqlite",
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
        let owned = owned_postgres::OwnedPostgres::start("many-to-many").unwrap();
        #[cfg(feature = "postgres")]
        let url = owned.url.clone();
        #[cfg(feature = "postgres")]
        let db = Database::<Backend>::connect_postgres(&url).await.unwrap();
        let schema = Arc::new(definition.validate().unwrap());
        let target = schema
            .physical_schema::<Backend>(Default::default())
            .unwrap();
        let ownership = ManagedTableSet::new(schema.owned_table_names()).unwrap();
        let plan = db
            .schema()
            .plan_owned_migration(
                "init",
                "many to many",
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
        for sql in seed {
            Backend::fetch_rows(db.pool(), sql, &[]).await.unwrap();
        }
        Self {
            url,
            #[cfg(feature = "sqlite")]
            path,
            db,
            environment,
            target,
            ownership,
            #[cfg(feature = "postgres")]
            owned,
        }
    }
    fn schema(&self) -> &Arc<ValidatedRuntimeSchema> {
        self.environment.schema()
    }
    fn authority(&self, allow_links: bool) -> Authority {
        Authority {
            schema: self.schema().clone(),
            allow_links,
            log: Default::default(),
            pause: None,
            hide_secret_targets: false,
            hidden_source: None,
            deny_targets: Vec::new(),
            requested: Default::default(),
        }
    }
    fn key(&self, collection: &str, id: i64) -> RuntimeKey {
        self.key_of(collection, RuntimeValue::Integer(id))
    }
    fn key_of(&self, collection: &str, id: RuntimeValue) -> RuntimeKey {
        let handle = self.schema().resolve_collection(&cid(collection)).unwrap();
        let field = self
            .schema()
            .resolve_field(&handle, &fid(&format!("{collection}.id")))
            .unwrap();
        self.schema().runtime_key(&handle, &[(field, id)]).unwrap()
    }
    fn relation(&self, collection: &str, id: &str) -> RuntimeRelationHandle {
        let handle = self.schema().resolve_collection(&cid(collection)).unwrap();
        self.schema()
            .resolve_relation(&handle, &RelationId::new(id).unwrap())
            .unwrap()
    }
    fn link(&self, post: i64, change: RuntimeLinkChange, max: usize) -> RuntimeMutationRequest {
        self.schema()
            .runtime_link_request(
                &self.relation("post", "tags"),
                self.key("post", post),
                change,
                None,
                max,
                Default::default(),
            )
            .unwrap()
    }
    fn tags(&self, ids: &[i64]) -> Vec<RuntimeKey> {
        ids.iter().map(|id| self.key("tag", *id)).collect()
    }
    async fn run(
        &self,
        request: RuntimeMutationRequest,
        authority: Arc<dyn RuntimeWriteAuthority<Backend>>,
        hook: &Hook,
    ) -> Result<Vec<String>, TransactionError> {
        let environment = self.environment.clone();
        let hook = hook.clone();
        self.db
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let effect = tx
                        .mutate_runtime(&environment, &request, authority.as_ref())
                        .await
                        .map_err(OrmPublicError::from)?;
                    tx.run_runtime_before_commit(&hook, &effect)
                        .await
                        .map_err(OrmPublicError::from)?;
                    Ok(effect
                        .links()
                        .iter()
                        .flat_map(|e| {
                            e.added()
                                .iter()
                                .map(|l| format!("+{}", describe(l)))
                                .chain(e.removed().iter().map(|l| format!("-{}", describe(l))))
                                .collect::<Vec<_>>()
                        })
                        .collect())
                })
            })
            .await
    }
    async fn links(&self, table: &str) -> Vec<(i64, i64)> {
        let (a, b) = if table == "mm_post_tags" {
            ("post_id", "tag_id")
        } else {
            ("from_id", "to_id")
        };
        let rows = Backend::fetch_rows(
            self.db.pool(),
            &format!("SELECT {a} AS a, {b} AS b FROM {table} ORDER BY {a}, {b}"),
            &[],
        )
        .await
        .unwrap();
        rows.iter()
            .map(|row| {
                (
                    Backend::try_get_i64(row, "a").unwrap(),
                    Backend::try_get_i64(row, "b").unwrap(),
                )
            })
            .collect()
    }
    async fn finish(self) {
        self.db.pool().close().await;
        #[cfg(feature = "sqlite")]
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.path.display()));
        }
        #[cfg(feature = "postgres")]
        {
            let mut owned = self.owned;
            owned.cleanup().unwrap();
        }
    }
}
fn code(error: &TransactionError) -> Option<&'static str> {
    error.public_error().runtime_mutation_code()
}

#[tokio::test]
async fn link_sets_replace_add_remove_with_authority_and_hooks() {
    let f = Fixture::new().await;
    let authority = f.authority(true);
    let hook = Hook::default();
    let replace = |ids: &[i64]| RuntimeLinkChange::Replace(f.tags(ids));
    assert_eq!(
        f.run(
            f.link(1, replace(&[10, 11]), 10),
            Arc::new(authority.clone()),
            &hook
        )
        .await
        .unwrap(),
        ["+1-10", "+1-11"]
    );
    assert_eq!(f.links("mm_post_tags").await, [(1, 10), (1, 11)]);
    // Replace computes the exact delta; unchanged links are not rewritten.
    assert_eq!(
        f.run(
            f.link(1, replace(&[11, 12]), 10),
            Arc::new(authority.clone()),
            &hook
        )
        .await
        .unwrap(),
        ["+1-12", "-1-10"]
    );
    // Delta: adding an existing link or removing an absent one changes nothing.
    let delta = RuntimeLinkChange::Delta {
        add: f.tags(&[11, 13]),
        remove: f.tags(&[12, 10]),
    };
    assert_eq!(
        f.run(f.link(1, delta, 10), Arc::new(authority.clone()), &hook)
            .await
            .unwrap(),
        ["+1-13", "-1-12"]
    );
    assert_eq!(f.links("mm_post_tags").await, [(1, 11), (1, 13)]);
    assert_eq!(
        *authority.log.lock().unwrap(),
        [
            "tags:+[1-10,1-11]-[]",
            "tags:+[1-12]-[1-10]",
            "tags:+[1-13]-[1-12]"
        ]
    );
    assert_eq!(
        *hook.seen.lock().unwrap(),
        ["tags:+2-0", "tags:+1-1", "tags:+1-1"]
    );

    // Duplicate targets and over-limit requests refuse before any I/O.
    assert!(
        f.schema()
            .runtime_link_request(
                &f.relation("post", "tags"),
                f.key("post", 1),
                RuntimeLinkChange::Delta {
                    add: f.tags(&[10]),
                    remove: f.tags(&[10]),
                },
                None,
                10,
                Default::default(),
            )
            .is_err()
    );
    assert_eq!(
        f.schema()
            .runtime_link_request(
                &f.relation("post", "tags"),
                f.key("post", 1),
                replace(&[10, 11, 12]),
                None,
                2,
                Default::default(),
            )
            .unwrap_err()
            .code(),
        RuntimeMutationErrorCode::LimitExceeded
    );
    // Current links beyond the bound refuse instead of reading an unbounded set.
    let error = f
        .run(
            f.link(1, replace(&[10]), 1),
            Arc::new(authority.clone()),
            &hook,
        )
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("limit_exceeded"));
    // A missing target is refused before the host is asked or anything is written.
    let before = authority.log.lock().unwrap().len();
    let error = f
        .run(
            f.link(2, replace(&[10, 99]), 10),
            Arc::new(authority.clone()),
            &hook,
        )
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("denied"));
    assert_eq!(authority.log.lock().unwrap().len(), before);
    assert_eq!(f.links("mm_post_tags").await, [(1, 11), (1, 13)]);
    // Hosts that do not opt in to links deny them.
    let error = f
        .run(
            f.link(2, replace(&[10]), 10),
            Arc::new(NoLinks(f.authority(true))),
            &hook,
        )
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("denied"));
    // A failing hook rolls back the link writes.
    let failing = Hook {
        fail: true,
        ..Default::default()
    };
    assert!(
        f.run(
            f.link(2, replace(&[10]), 10),
            Arc::new(authority.clone()),
            &failing
        )
        .await
        .is_err()
    );
    assert_eq!(f.links("mm_post_tags").await, [(1, 11), (1, 13)]);
    f.finish().await;
}

#[tokio::test]
async fn deleting_either_side_removes_its_links_after_authorization() {
    let f = Fixture::new().await;
    let authority = f.authority(true);
    let hook = Hook::default();
    f.run(
        f.link(1, RuntimeLinkChange::Replace(f.tags(&[10, 11])), 10),
        Arc::new(authority.clone()),
        &hook,
    )
    .await
    .unwrap();
    f.run(
        f.link(2, RuntimeLinkChange::Replace(f.tags(&[11])), 10),
        Arc::new(authority.clone()),
        &hook,
    )
    .await
    .unwrap();
    let delete = |collection: &str, id: i64| {
        f.schema()
            .runtime_delete_request(f.key(collection, id), None, None, Default::default())
            .unwrap()
    };
    // Without link authority the delete refuses and nothing changes.
    let error = f
        .run(
            delete("tag", 11),
            Arc::new(NoLinks(f.authority(true))),
            &hook,
        )
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("denied"));
    assert_eq!(f.links("mm_post_tags").await, [(1, 10), (1, 11), (2, 11)]);
    // Target side: links are reported from the declaring relation's perspective.
    assert_eq!(
        f.run(delete("tag", 11), Arc::new(authority.clone()), &hook)
            .await
            .unwrap(),
        ["-1-11", "-2-11"]
    );
    // Source side.
    assert_eq!(
        f.run(delete("post", 1), Arc::new(authority.clone()), &hook)
            .await
            .unwrap(),
        ["-1-10"]
    );
    assert!(f.links("mm_post_tags").await.is_empty());
    // A bounded delete refuses when a row has more links than allowed.
    f.run(
        f.link(2, RuntimeLinkChange::Replace(f.tags(&[10, 12])), 10),
        Arc::new(authority.clone()),
        &hook,
    )
    .await
    .unwrap();
    let error = f
        .run(
            delete("post", 2).with_max_delete_links(1),
            Arc::new(authority.clone()),
            &hook,
        )
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("limit_exceeded"));
    assert_eq!(f.links("mm_post_tags").await, [(2, 10), (2, 12)]);

    // Self-relations remove both directions of a node's links once.
    let nodes = f.relation("node", "links");
    for (from, to) in [(1, 2), (2, 1), (1, 1)] {
        let request = f
            .schema()
            .runtime_link_request(
                &nodes,
                f.key("node", from),
                RuntimeLinkChange::Delta {
                    add: vec![f.key("node", to)],
                    remove: vec![],
                },
                None,
                10,
                Default::default(),
            )
            .unwrap();
        f.run(request, Arc::new(authority.clone()), &hook)
            .await
            .unwrap();
    }
    assert_eq!(
        f.run(delete("node", 1), Arc::new(authority.clone()), &hook)
            .await
            .unwrap(),
        ["-1-1", "-1-2", "-2-1"]
    );
    assert!(f.links("mm_node_links").await.is_empty());
    f.finish().await;
}

#[tokio::test]
async fn a_link_inserted_during_a_delete_cannot_escape_authorization() {
    let f = Fixture::new().await;
    let reached = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let authority = Authority {
        pause: Some((reached.clone(), resume.clone())),
        ..f.authority(true)
    };
    f.run(
        f.link(1, RuntimeLinkChange::Replace(f.tags(&[11])), 10),
        Arc::new(f.authority(true)),
        &Hook::default(),
    )
    .await
    .unwrap();
    let hook = Hook::default();
    let delete = f.run(
        f.schema()
            .runtime_delete_request(f.key("tag", 11), None, None, Default::default())
            .unwrap(),
        Arc::new(authority),
        &hook,
    );
    let url = f.url.clone();
    let writer = async move {
        reached.notified().await;
        let insert = "INSERT INTO mm_post_tags (post_id, tag_id) VALUES (2, 11)";
        #[cfg(feature = "postgres")]
        let pool = graphql_orm::sqlx::PgPool::connect(&url).await.unwrap();
        #[cfg(feature = "sqlite")]
        let pool = {
            use std::str::FromStr;
            graphql_orm::sqlx::SqlitePool::connect_with(
                graphql_orm::sqlx::sqlite::SqliteConnectOptions::from_str(&url)
                    .unwrap()
                    .foreign_keys(true)
                    .busy_timeout(std::time::Duration::from_millis(200)),
            )
            .await
            .unwrap()
        };
        // PostgreSQL: the insert waits on the locked tag row. SQLite: the
        // immediate write transaction excludes it.
        let pending = graphql_orm::sqlx::query(insert).execute(&pool);
        tokio::pin!(pending);
        let early = tokio::time::timeout(std::time::Duration::from_millis(300), &mut pending).await;
        resume.notify_one();
        let inserted = match early {
            Ok(result) => result.is_ok(),
            Err(_) => pending.await.is_ok(),
        };
        pool.close().await;
        inserted
    };
    let (deleted, inserted) = tokio::join!(delete, writer);
    assert_eq!(deleted.unwrap(), ["-1-11"]);
    // The concurrent link never committed against the deleted tag.
    assert!(!inserted);
    assert!(f.links("mm_post_tags").await.is_empty());
    f.finish().await;
}

#[tokio::test]
async fn removing_a_join_relation_is_a_destructive_plan_and_adding_it_is_additive() {
    let f = Fixture::new().await;
    let mut without = definition();
    for c in &mut without.collections {
        c.relations
            .retain(|r| r.id.as_str() != "tags" && r.id.as_str() != "posts");
    }
    let without = without.validate().unwrap();
    let target = without
        .physical_schema::<Backend>(Default::default())
        .unwrap();
    let removed =
        f.db.schema()
            .plan_owned_migration(
                "drop",
                "drop links",
                &target,
                &f.ownership,
                PlanOptions::strict(),
            )
            .await
            .unwrap();
    let step = removed
        .steps()
        .iter()
        .find(|s| s.table_name() == Some("mm_post_tags"))
        .unwrap();
    assert_eq!(step.risk(), MigrationRisk::Destructive);
    assert!(
        f.db.schema()
            .apply_owned_migration(&removed, ApplyOptions::default())
            .await
            .is_err()
    );
    f.db.schema()
        .apply_owned_migration(
            &removed,
            ApplyOptions {
                allow_destructive: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let added =
        f.db.schema()
            .plan_owned_migration(
                "add",
                "add links",
                &f.target,
                &f.ownership,
                PlanOptions::strict(),
            )
            .await
            .unwrap();
    assert!(
        added
            .steps()
            .iter()
            .all(|s| s.risk() == MigrationRisk::Additive)
    );
    f.db.schema()
        .apply_owned_migration(&added, ApplyOptions::default())
        .await
        .unwrap();
    let replanned =
        f.db.schema()
            .plan_owned_migration(
                "noop",
                "no-op",
                &f.target,
                &f.ownership,
                PlanOptions::strict(),
            )
            .await
            .unwrap();
    assert!(replanned.steps().is_empty());
    f.finish().await;
}

#[tokio::test]
async fn hidden_link_targets_are_scoped_out_and_reveal_nothing() {
    let f = Fixture::new().await;
    let hook = Hook::default();
    f.run(
        f.link(1, RuntimeLinkChange::Replace(f.tags(&[10, 13])), 10),
        Arc::new(f.authority(true)),
        &hook,
    )
    .await
    .unwrap();
    let delta = |add: &[i64], remove: &[i64]| RuntimeLinkChange::Delta {
        add: f.tags(add),
        remove: f.tags(remove),
    };
    // The host decides from the requested targets, so denying visible tag 10
    // fails identically whether post 1 links it or post 2 does not, and the
    // host sees the same request in both cases.
    let guarded = Authority {
        hide_secret_targets: true,
        deny_targets: vec![10],
        ..f.authority(true)
    };
    for change in [
        delta(&[10], &[]),
        delta(&[], &[10]),
        RuntimeLinkChange::Replace(f.tags(&[10, 11])),
    ] {
        let mut outcomes = Vec::new();
        for post in [1, 2] {
            let error = f
                .run(
                    f.link(post, change.clone(), 10),
                    Arc::new(guarded.clone()),
                    &hook,
                )
                .await
                .unwrap_err();
            assert_eq!(code(&error), Some("denied"));
            outcomes.push(error.to_string());
        }
        assert_eq!(outcomes[0], outcomes[1]);
    }
    {
        let requested = guarded.requested.lock().unwrap();
        assert_eq!(requested.len(), 6);
        for pair in requested.chunks(2) {
            assert_eq!(pair[0], pair[1]);
        }
    }
    assert_eq!(f.links("mm_post_tags").await, [(1, 10), (1, 13)]);

    let scoped = Authority {
        hide_secret_targets: true,
        ..f.authority(true)
    };
    // Replace removes only visible links; the hidden link is preserved and
    // does not count toward the link-key bound.
    assert_eq!(
        f.run(
            f.link(1, RuntimeLinkChange::Replace(f.tags(&[11])), 1),
            Arc::new(scoped.clone()),
            &hook
        )
        .await
        .unwrap(),
        ["+1-11", "-1-10"]
    );
    assert_eq!(f.links("mm_post_tags").await, [(1, 11), (1, 13)]);
    f.finish().await;
}

#[tokio::test]
async fn hidden_and_missing_link_targets_are_refused_uniformly() {
    let f = Fixture::new().await;
    f.run(
        f.link(1, RuntimeLinkChange::Replace(f.tags(&[10, 13])), 10),
        Arc::new(f.authority(true)),
        &Hook::default(),
    )
    .await
    .unwrap();
    let delta = |add: &[i64], remove: &[i64]| RuntimeLinkChange::Delta {
        add: f.tags(add),
        remove: f.tags(remove),
    };
    // This host would allow any link; the ORM still refuses hidden tag 13
    // (linked to post 1, unlinked from post 2) and missing tag 99 identically,
    // before `authorize_links`, hooks or any write.
    for hide_secret_targets in [true, false] {
        let permissive = Authority {
            hide_secret_targets,
            ..f.authority(true)
        };
        let hook = Hook::default();
        let mut cases = vec![(2, 99), (1, 99)];
        if hide_secret_targets {
            cases.extend([(1, 13), (2, 13)]);
        }
        for change in [|t: i64| (vec![t], vec![]), |t: i64| (vec![], vec![t])]
            .into_iter()
            .map(Some)
            .chain([None])
        {
            let mut outcomes = BTreeSet::new();
            for (post, target) in &cases {
                let change = match change {
                    Some(f_) => {
                        let (add, remove) = f_(*target);
                        delta(&add, &remove)
                    }
                    None => RuntimeLinkChange::Replace(f.tags(&[11, *target])),
                };
                let error = f
                    .run(
                        f.link(*post, change, 10),
                        Arc::new(permissive.clone()),
                        &hook,
                    )
                    .await
                    .unwrap_err();
                outcomes.insert((code(&error), error.to_string()));
            }
            assert_eq!(outcomes.len(), 1, "{outcomes:?}");
            assert_eq!(outcomes.first().unwrap().0, Some("denied"));
        }
        assert!(permissive.log.lock().unwrap().is_empty());
        assert!(permissive.requested.lock().unwrap().is_empty());
        assert!(hook.seen.lock().unwrap().is_empty());
        assert_eq!(f.links("mm_post_tags").await, [(1, 10), (1, 13)]);
    }
    f.finish().await;
}

/// Documents keyed by UUID linked both ways to words keyed by text.
fn keyed_definition() -> RuntimeSchema {
    let mut doc = collection("doc", "Doc", "Docs");
    doc.fields[0].value_kind = RuntimeValueKind::Uuid;
    let mut word = collection("word", "Word", "Words");
    word.fields[0].value_kind = RuntimeValueKind::String;
    doc.relations.push(join(
        "words",
        "word",
        "mm_doc_words",
        ("doc.id", "doc_id"),
        ("word.id", "word_id"),
    ));
    word.relations.push(join(
        "docs",
        "doc",
        "mm_doc_words",
        ("word.id", "word_id"),
        ("doc.id", "doc_id"),
    ));
    RuntimeSchema {
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![doc, word],
    }
}

#[tokio::test]
async fn uuid_and_text_keys_match_existing_links_exactly() {
    let f = Fixture::with_definition(keyed_definition(), &[]).await;
    let authority = f.authority(true);
    let hook = Hook::default();
    let doc = |n: u128| RuntimeValue::Uuid(graphql_orm::uuid::Uuid::from_u128(n));
    let word = |w: &str| RuntimeValue::String(w.into());
    for (collection, id) in [
        ("doc", doc(0xa1)),
        ("doc", doc(0xb2)),
        ("word", word("alpha")),
        ("word", word("Beta")),
        ("word", word("gamma")),
    ] {
        let handle = f.schema().resolve_collection(&cid(collection)).unwrap();
        let field = |name: &str| {
            f.schema()
                .resolve_field(&handle, &fid(&format!("{collection}.{name}")))
                .unwrap()
        };
        let request = f
            .schema()
            .runtime_create_request(
                &handle,
                &[
                    (field("id"), id),
                    (field("label"), RuntimeValue::String("x".into())),
                ],
                None,
                Default::default(),
            )
            .unwrap();
        f.run(request, Arc::new(authority.clone()), &hook)
            .await
            .unwrap();
    }
    let words = |ws: &[&str]| {
        ws.iter()
            .map(|w| f.key_of("word", word(w)))
            .collect::<Vec<_>>()
    };
    let link = |d: u128, change: RuntimeLinkChange| {
        f.schema().runtime_link_request(
            &f.relation("doc", "words"),
            f.key_of("doc", doc(d)),
            change,
            None,
            10,
            Default::default(),
        )
    };
    let run = |request: RuntimeMutationRequest| f.run(request, Arc::new(authority.clone()), &hook);
    assert_eq!(
        run(link(0xa1, RuntimeLinkChange::Replace(words(&["alpha", "Beta"]))).unwrap())
            .await
            .unwrap(),
        ["+00a1-alpha", "+00a1-Beta"]
    );
    // Existing links are recognised by key value and never re-added.
    assert_eq!(
        run(link(0xa1, RuntimeLinkChange::Replace(words(&["Beta", "gamma"]))).unwrap())
            .await
            .unwrap(),
        ["+00a1-gamma", "-00a1-alpha"]
    );
    assert_eq!(
        run(link(
            0xa1,
            RuntimeLinkChange::Delta {
                add: words(&["Beta"]),
                remove: words(&["gamma", "alpha"]),
            }
        )
        .unwrap())
        .await
        .unwrap(),
        ["-00a1-gamma"]
    );
    // Text keys match exactly: a differently cased key names a missing row.
    let error = run(link(
        0xa1,
        RuntimeLinkChange::Delta {
            add: words(&["beta"]),
            remove: vec![],
        },
    )
    .unwrap())
    .await
    .unwrap_err();
    assert_eq!(code(&error), Some("denied"));
    assert_eq!(
        link(0xa1, RuntimeLinkChange::Replace(words(&["Beta", "Beta"])))
            .unwrap_err()
            .code(),
        RuntimeMutationErrorCode::InvalidInput
    );
    // The inverse side matches UUID targets the same way.
    let inverse = f
        .schema()
        .runtime_link_request(
            &f.relation("word", "docs"),
            f.key_of("word", word("Beta")),
            RuntimeLinkChange::Delta {
                add: vec![f.key_of("doc", doc(0xa1)), f.key_of("doc", doc(0xb2))],
                remove: vec![],
            },
            None,
            10,
            Default::default(),
        )
        .unwrap();
    assert_eq!(run(inverse).await.unwrap(), ["+Beta-00b2"]);
    // Deleting a text-keyed row removes its links, described from the declaring side.
    let delete = f
        .schema()
        .runtime_delete_request(
            f.key_of("word", word("Beta")),
            None,
            None,
            Default::default(),
        )
        .unwrap();
    assert_eq!(run(delete).await.unwrap(), ["-00a1-Beta", "-00b2-Beta"]);
    let rows = Backend::fetch_rows(f.db.pool(), "SELECT COUNT(*) AS n FROM mm_doc_words", &[])
        .await
        .unwrap();
    assert_eq!(Backend::try_get_i64(&rows[0], "n").unwrap(), 0);
    f.finish().await;
}

#[tokio::test]
async fn link_writes_on_a_hidden_source_are_not_found() {
    let f = Fixture::new().await;
    let hidden = Authority {
        hidden_source: Some("second"),
        ..f.authority(true)
    };
    let error = f
        .run(
            f.link(2, RuntimeLinkChange::Replace(f.tags(&[10])), 10),
            Arc::new(hidden.clone()),
            &Hook::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("not_found"));
    assert!(hidden.requested.lock().unwrap().is_empty());
    assert!(f.links("mm_post_tags").await.is_empty());
    f.finish().await;
}

#[test]
fn append_only_sources_refuse_link_writes() {
    let mut definition = definition();
    definition.collections[0].append_only = true;
    let schema = definition.validate().unwrap();
    let post = schema.resolve_collection(&cid("post")).unwrap();
    let id = schema.resolve_field(&post, &fid("post.id")).unwrap();
    let key = schema
        .runtime_key(&post, &[(id, RuntimeValue::Integer(1))])
        .unwrap();
    let relation = schema
        .resolve_relation(&post, &RelationId::new("tags").unwrap())
        .unwrap();
    assert_eq!(
        schema
            .runtime_link_request(
                &relation,
                key,
                RuntimeLinkChange::Replace(Vec::new()),
                None,
                10,
                Default::default(),
            )
            .unwrap_err()
            .code(),
        RuntimeMutationErrorCode::AppendOnly
    );
}

#[tokio::test]
async fn concurrent_adds_from_both_sides_store_one_link() {
    let f = Fixture::new().await;
    let reached = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let paused = Authority {
        pause: Some((reached.clone(), resume.clone())),
        ..f.authority(true)
    };
    let hook = Hook::default();
    let forward = f.run(
        f.link(
            1,
            RuntimeLinkChange::Delta {
                add: f.tags(&[10]),
                remove: vec![],
            },
            10,
        ),
        Arc::new(paused),
        &hook,
    );
    let inverse = f
        .schema()
        .runtime_link_request(
            &f.relation("tag", "posts"),
            f.key("tag", 10),
            RuntimeLinkChange::Delta {
                add: vec![f.key("post", 1)],
                remove: vec![],
            },
            None,
            10,
            Default::default(),
        )
        .unwrap();
    let backward = async {
        // The forward writer has read no current link and is paused.
        reached.notified().await;
        let pending = f.run(inverse, Arc::new(f.authority(true)), &hook);
        tokio::pin!(pending);
        let early = tokio::time::timeout(std::time::Duration::from_millis(300), &mut pending).await;
        resume.notify_one();
        match early {
            Ok(result) => result,
            Err(_) => pending.await,
        }
    };
    let (forward, backward) = tokio::join!(forward, backward);
    // Either writer may observe the other's link (no-op) or, on PostgreSQL,
    // a retryable serialization failure; neither duplicates nor errors otherwise.
    for result in [&forward, &backward] {
        if let Err(error) = result {
            assert_eq!(code(error), Some("retryable_transaction"), "{error}");
        }
    }
    assert!(forward.is_ok() || backward.is_ok());
    assert_eq!(f.links("mm_post_tags").await, [(1, 10)]);
    f.finish().await;
}

impl Fixture {
    /// Plan `next`, refuse it without destructive approval, apply it with
    /// approval, and require an empty replan.
    async fn migrate_destructively(
        &self,
        version: &str,
        next: RuntimeSchema,
        extra_tables: &[&str],
    ) -> Result<(), RuntimeMigrationError> {
        let next = next.validate().unwrap();
        let target = next.physical_schema::<Backend>(Default::default()).unwrap();
        let mut names = self.schema().owned_table_names();
        names.extend(extra_tables.iter().map(|t| t.to_string()));
        let ownership = ManagedTableSet::new(names).unwrap();
        let schema = self.db.schema();
        let planned = schema
            .plan_owned_migration(version, version, &target, &ownership, PlanOptions::strict())
            .await
            .unwrap();
        assert!(
            planned
                .steps()
                .iter()
                .any(|s| s.risk() == MigrationRisk::Destructive)
        );
        assert!(
            self.db
                .schema()
                .apply_owned_migration(&planned, ApplyOptions::default())
                .await
                .is_err()
        );
        self.db
            .schema()
            .apply_owned_migration(
                &planned,
                ApplyOptions {
                    allow_destructive: true,
                    ..Default::default()
                },
            )
            .await?;
        let replanned = schema
            .plan_owned_migration(
                "replan",
                "replan",
                &target,
                &ownership,
                PlanOptions::strict(),
            )
            .await
            .unwrap();
        assert!(replanned.steps().is_empty(), "{:?}", replanned.steps());
        Ok(())
    }
    async fn tables(&self) -> Vec<String> {
        #[cfg(feature = "sqlite")]
        let sql = "SELECT name AS name FROM sqlite_master WHERE type = 'table' AND name LIKE 'mm_%' ORDER BY name";
        #[cfg(feature = "postgres")]
        let sql = "SELECT tablename::text AS name FROM pg_tables WHERE schemaname = current_schema() AND tablename LIKE 'mm_%' ORDER BY tablename";
        Backend::fetch_rows(self.db.pool(), sql, &[])
            .await
            .unwrap()
            .iter()
            .map(|row| Backend::try_get_string(row, "name").unwrap())
            .collect()
    }
}

#[tokio::test]
async fn removing_a_linked_collection_drops_its_link_table() {
    let f = Fixture::new().await;
    f.run(
        f.link(1, RuntimeLinkChange::Replace(f.tags(&[10])), 10),
        Arc::new(f.authority(true)),
        &Hook::default(),
    )
    .await
    .unwrap();
    let mut next = definition();
    next.collections.retain(|c| c.id.as_str() != "tag");
    for c in &mut next.collections {
        c.relations.retain(|r| r.target.as_str() != "tag");
    }
    f.migrate_destructively("drop-tag", next, &[])
        .await
        .unwrap();
    assert_eq!(f.tables().await, ["mm_node", "mm_node_links", "mm_post"]);
    f.finish().await;
}

#[tokio::test]
async fn renaming_physical_tables_is_a_destructive_recreate() {
    let f = Fixture::new().await;
    f.run(
        f.link(1, RuntimeLinkChange::Replace(f.tags(&[10])), 10),
        Arc::new(f.authority(true)),
        &Hook::default(),
    )
    .await
    .unwrap();
    // A target collection's physical table: the planner has no rename, so the
    // table is dropped and re-created empty and the link table's foreign key
    // (a SQLite table rebuild) follows the new name. Existing links would
    // dangle, so the migration fails its foreign-key check and rolls back.
    let mut next = definition();
    next.collections[1].physical_table = "mm_tag_v2".into();
    assert!(
        f.migrate_destructively("rename-tag", next.clone(), &["mm_tag_v2"])
            .await
            .is_err()
    );
    assert_eq!(
        f.tables().await,
        [
            "mm_node",
            "mm_node_links",
            "mm_post",
            "mm_post_tags",
            "mm_tag"
        ]
    );
    assert_eq!(f.links("mm_post_tags").await, [(1, 10)]);
    f.run(
        f.link(1, RuntimeLinkChange::Replace(Vec::new()), 10),
        Arc::new(f.authority(true)),
        &Hook::default(),
    )
    .await
    .unwrap();
    f.migrate_destructively("rename-tag-unlinked", next.clone(), &["mm_tag_v2"])
        .await
        .unwrap();
    // A link table's physical name: the links are dropped with the old table.
    for c in &mut next.collections {
        for r in &mut c.relations {
            if let Some(join) = &mut r.join
                && join.physical_table == "mm_post_tags"
            {
                join.physical_table = "mm_post_tags_v2".into();
            }
        }
    }
    f.migrate_destructively(
        "rename-links",
        next.clone(),
        &["mm_tag_v2", "mm_post_tags_v2"],
    )
    .await
    .unwrap();
    assert_eq!(
        f.tables().await,
        [
            "mm_node",
            "mm_node_links",
            "mm_post",
            "mm_post_tags_v2",
            "mm_tag_v2"
        ]
    );
    // The re-created link table enforces its foreign key against the renamed target.
    for sql in [
        "INSERT INTO mm_tag_v2 (id, label) VALUES (10, 'red')",
        "INSERT INTO mm_post_tags_v2 (post_id, tag_id) VALUES (1, 10)",
    ] {
        Backend::fetch_rows(f.db.pool(), sql, &[]).await.unwrap();
    }
    assert!(
        Backend::fetch_rows(
            f.db.pool(),
            "INSERT INTO mm_post_tags_v2 (post_id, tag_id) VALUES (1, 99)",
            &[]
        )
        .await
        .is_err()
    );
    f.finish().await;
}

#[tokio::test]
async fn creates_and_updates_carry_link_changes_in_one_transaction() {
    use RuntimeMutationErrorCode as E;
    let f = Fixture::new().await;
    let schema = f.schema().clone();
    let post = schema.resolve_collection(&cid("post")).unwrap();
    let field = |name: &str| {
        schema
            .resolve_field(&post, &fid(&format!("post.{name}")))
            .unwrap()
    };
    let tags = f.relation("post", "tags");
    let count = |sql: &'static str| {
        let db = f.db.clone();
        async move {
            let rows = Backend::fetch_rows(db.pool(), sql, &[]).await.unwrap();
            Backend::try_get_i64(&rows[0], "n").unwrap()
        }
    };
    let create = |id: i64, label: &str| {
        schema
            .runtime_create_request(
                &post,
                &[
                    (field("id"), RuntimeValue::Integer(id)),
                    (field("label"), RuntimeValue::String(label.into())),
                ],
                None,
                Default::default(),
            )
            .unwrap()
    };
    let relabel = |id: i64, label: &str| {
        schema
            .runtime_update_request(
                f.key("post", id),
                &[(field("label"), RuntimeValue::String(label.into()))],
                None,
                None,
                Default::default(),
            )
            .unwrap()
    };

    // A create binds its links to the generated row, in the same transaction and effect.
    let request = create(3, "third")
        .with_link_change(&tags, RuntimeLinkChange::Replace(f.tags(&[10, 11])), 10)
        .unwrap();
    assert_eq!(request.attached_link_changes().count(), 1);
    let applied = f
        .run(request, Arc::new(f.authority(true)), &Hook::default())
        .await
        .unwrap();
    assert_eq!(applied, ["+3-10", "+3-11"]);
    assert_eq!(f.links("mm_post_tags").await, [(3, 10), (3, 11)]);

    // An update writes fields and a delta together.
    let request = relabel(3, "3rd")
        .with_link_change(
            &tags,
            RuntimeLinkChange::Delta {
                add: f.tags(&[12]),
                remove: f.tags(&[10]),
            },
            10,
        )
        .unwrap();
    f.run(request, Arc::new(f.authority(true)), &Hook::default())
        .await
        .unwrap();
    assert_eq!(f.links("mm_post_tags").await, [(3, 11), (3, 12)]);
    assert_eq!(
        count("SELECT COUNT(*) AS n FROM mm_post WHERE id = 3 AND label = '3rd'").await,
        1
    );

    // A link refusal rolls the field write back, and so does a missing target.
    let request = relabel(3, "rolled back")
        .with_link_change(&tags, RuntimeLinkChange::Replace(f.tags(&[10])), 10)
        .unwrap();
    let error = f
        .run(request, Arc::new(f.authority(false)), &Hook::default())
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("denied"));
    let request = create(4, "fourth")
        .with_link_change(&tags, RuntimeLinkChange::Replace(f.tags(&[99])), 10)
        .unwrap();
    let error = f
        .run(request, Arc::new(f.authority(true)), &Hook::default())
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("denied"));
    assert_eq!(
        count("SELECT COUNT(*) AS n FROM mm_post WHERE id = 3 AND label = '3rd'").await,
        1
    );
    assert_eq!(
        count("SELECT COUNT(*) AS n FROM mm_post WHERE id = 4").await,
        0
    );
    assert_eq!(f.links("mm_post_tags").await, [(3, 11), (3, 12)]);

    // Shapes are refused before any I/O: a delete, a repeated relation, another collection's
    // relation, and the key bound.
    let delete = schema
        .runtime_delete_request(f.key("post", 3), None, None, Default::default())
        .unwrap();
    let refused = |result: Result<RuntimeMutationRequest, RuntimeMutationError>| {
        result.err().map(|e| e.code())
    };
    assert_eq!(
        refused(delete.with_link_change(&tags, RuntimeLinkChange::Replace(vec![]), 10)),
        Some(E::InvalidInput)
    );
    let once = create(5, "five")
        .with_link_change(&tags, RuntimeLinkChange::Replace(vec![]), 10)
        .unwrap();
    assert_eq!(
        refused(once.with_link_change(&tags, RuntimeLinkChange::Replace(vec![]), 10)),
        Some(E::InvalidInput)
    );
    assert_eq!(
        refused(create(5, "five").with_link_change(
            &f.relation("node", "links"),
            RuntimeLinkChange::Replace(vec![]),
            10
        )),
        Some(E::InvalidInput)
    );
    assert_eq!(
        refused(create(5, "five").with_link_change(
            &tags,
            RuntimeLinkChange::Replace(f.tags(&[10, 11])),
            1
        )),
        Some(E::LimitExceeded)
    );
    f.finish().await;
}

#[cfg(feature = "runtime-graphql")]
mod graphql {
    use super::*;
    use graphql_orm::async_graphql::{Request, Variables, dynamic::Schema};
    use graphql_orm::graphql::runtime::*;
    use serde_json::{Value, json};

    /// Every layer hides the `secret` label.
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
            check: RuntimeReadCheck<'a>,
        ) -> BoxFuture<'a, Result<RuntimeReadGrant, RuntimeGraphqlError>> {
            Box::pin(async move {
                let collection = check.collection.id().as_str();
                let label = check
                    .schema
                    .resolve_field(check.collection, &fid(&format!("{collection}.label")))
                    .unwrap();
                Ok(RuntimeReadGrant::new(Some(
                    check
                        .schema
                        .runtime_compare(
                            check.collection,
                            &label,
                            RuntimeScalarOperator::Ne,
                            RuntimeValue::String("secret".into()),
                            Default::default(),
                        )
                        .unwrap(),
                )))
            })
        }
    }
    fn api(f: &Fixture) -> Schema {
        let module = RuntimeGraphqlModule::compile(
            f.schema().clone(),
            RuntimeGraphqlOptions::default()
                .with_cursor_profile(RuntimeCursorProfile::Unprotected)
                .with_mutations(Default::default()),
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
    async fn run(f: &Fixture, api: &Schema, query: &str) -> Value {
        run_as(f, api, query, f.authority(true)).await
    }
    async fn run_as(f: &Fixture, api: &Schema, query: &str, authority: Authority) -> Value {
        let schema = f.schema();
        let response = api
            .execute(
                Request::new(query)
                    .variables(Variables::from_json(
                        json!({"schema": schema.fingerprint().to_string()}),
                    ))
                    .data(
                        RuntimeGraphqlRequest::<Backend>::new(
                            schema.fingerprint(),
                            Arc::new(Read),
                            None,
                            Default::default(),
                        )
                        .with_mutation_environment(f.environment.clone())
                        .with_write_authority(Arc::new(authority)),
                    ),
            )
            .await;
        serde_json::to_value(&response).unwrap()
    }

    #[tokio::test]
    async fn graphql_link_mutations_and_batched_join_reads() {
        let f = Fixture::new().await;
        let api = api(&f);
        let sdl = api.sdl();
        assert!(sdl.contains("updatePostTagsLinks("), "{sdl}");
        assert!(sdl.contains("): TagConnection!"), "{sdl}");
        // The link mutation is a described generated root of the semantic catalogue.
        let (_, catalog) = RuntimeGraphqlComposer::new(f.db.clone(), "Query", Default::default())
            .unwrap()
            .mutation_root("Mutation")
            .unwrap()
            .install(
                RuntimeGraphqlModule::compile(
                    f.schema().clone(),
                    RuntimeGraphqlOptions::default()
                        .with_cursor_profile(RuntimeCursorProfile::Unprotected)
                        .with_mutations(Default::default()),
                )
                .unwrap(),
            )
            .unwrap()
            .finish_with_semantic_catalog()
            .unwrap();
        assert!(
            catalog
                .operations
                .iter()
                .any(|operation| operation.field_name == "updatePostTagsLinks")
        );

        let set = run(
            &f,
            &api,
            r#"mutation($schema: String!) { updatePostTagsLinks(expectedSchema: $schema, key: {id: "1"}, set: [{id: "10"}, {id: "11"}, {id: "13"}]) { record { id label } } }"#,
        )
        .await;
        assert!(set.get("errors").is_none(), "{set}");
        assert_eq!(
            set["data"]["updatePostTagsLinks"]["record"]["label"],
            "first"
        );
        let add = run(
            &f,
            &api,
            r#"mutation($schema: String!) { updatePostTagsLinks(expectedSchema: $schema, key: {id: "2"}, add: [{id: "11"}, {id: "12"}]) { record { id } } }"#,
        )
        .await;
        assert!(add.get("errors").is_none(), "{add}");
        // `set` cannot be combined with `add`/`remove`.
        let mixed = run(
            &f,
            &api,
            r#"mutation($schema: String!) { updatePostTagsLinks(expectedSchema: $schema, key: {id: "2"}, set: [], add: [{id: "11"}]) { record { id } } }"#,
        )
        .await;
        assert!(mixed.get("errors").is_some(), "{mixed}");

        // One batched layer per relation; hidden targets are omitted; filters,
        // ordering, pagination and counts apply to the target layer.
        let read = run(
            &f,
            &api,
            r#"{ posts(orderBy: [{field: id}]) { edges { node { id tags(orderBy: [{field: label, direction: DESC}], first: 2) { totalCount edges { node { label } } pageInfo { hasNextPage } } } } } }"#,
        )
        .await;
        assert!(read.get("errors").is_none(), "{read}");
        let posts = read["data"]["posts"]["edges"].as_array().unwrap();
        assert_eq!(posts[0]["node"]["tags"]["totalCount"], "2");
        assert_eq!(
            posts[0]["node"]["tags"]["edges"],
            json!([{"node": {"label": "red"}}, {"node": {"label": "green"}}])
        );
        assert_eq!(posts[1]["node"]["tags"]["totalCount"], "2");
        assert_eq!(
            posts[1]["node"]["tags"]["edges"],
            json!([{"node": {"label": "green"}}, {"node": {"label": "blue"}}])
        );
        let inverse = run(
            &f,
            &api,
            r#"{ tags(orderBy: [{field: id}], where: {label: {eq: "green"}}) { edges { node { posts(orderBy: [{field: id}]) { edges { node { id } } } } } } }"#,
        )
        .await;
        assert_eq!(
            inverse["data"]["tags"]["edges"][0]["node"]["posts"]["edges"],
            json!([{"node": {"id": "1"}}, {"node": {"id": "2"}}]),
            "{inverse}"
        );
        let filtered = run(
            &f,
            &api,
            r#"{ posts(where: {id: {eq: "1"}}) { edges { node { tags(where: {label: {startsWith: "g"}}) { edges { node { label } } } } } } }"#,
        )
        .await;
        assert_eq!(
            filtered["data"]["posts"]["edges"][0]["node"]["tags"]["edges"],
            json!([{"node": {"label": "green"}}]),
            "{filtered}"
        );
        f.finish().await;
    }

    #[tokio::test]
    async fn graphql_writes_take_opt_in_links() {
        let f = Fixture::new().await;
        // Off by default: no `links` argument.
        assert!(!api(&f).sdl().contains("links: PostLinksInput"));
        let mut options = RuntimeGraphqlMutationOptions::default();
        options.links_in_writes = true;
        let module = RuntimeGraphqlModule::compile(
            f.schema().clone(),
            RuntimeGraphqlOptions::default()
                .with_cursor_profile(RuntimeCursorProfile::Unprotected)
                .with_mutations(options),
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
        let sdl = api.sdl();
        assert!(sdl.contains("links: PostLinksInput"), "{sdl}");
        assert!(sdl.contains("input PostTagsLinkChangeInput"), "{sdl}");
        let created = run(
            &f,
            &api,
            r#"mutation($schema: String!) { createPost(expectedSchema: $schema, input: {id: "5", label: "five"}, links: {tags: {set: [{id: "10"}, {id: "12"}]}}) { record { id label } } }"#,
        )
        .await;
        assert!(created.get("errors").is_none(), "{created}");
        assert_eq!(created["data"]["createPost"]["record"]["label"], "five");
        let updated = run(
            &f,
            &api,
            r#"mutation($schema: String!) { updatePost(expectedSchema: $schema, key: {id: "5"}, input: {label: "5"}, links: {tags: {add: [{id: "11"}], remove: [{id: "10"}]}}) { record { label } } }"#,
        )
        .await;
        assert!(updated.get("errors").is_none(), "{updated}");
        assert_eq!(f.links("mm_post_tags").await, [(5, 11), (5, 12)]);
        // A refused link write leaves the row as it was.
        let refused = run_as(
            &f,
            &api,
            r#"mutation($schema: String!) { updatePost(expectedSchema: $schema, key: {id: "5"}, input: {label: "nope"}, links: {tags: {set: [{id: "10"}]}}) { record { label } } }"#,
            f.authority(false),
        )
        .await;
        assert!(refused.get("errors").is_some(), "{refused}");
        let read = run(
            &f,
            &api,
            r#"{ posts(where: {id: {eq: "5"}}) { edges { node { label } } } }"#,
        )
        .await;
        assert_eq!(
            read["data"]["posts"]["edges"][0]["node"]["label"], "5",
            "{read}"
        );
        assert_eq!(f.links("mm_post_tags").await, [(5, 11), (5, 12)]);
        f.finish().await;
    }

    #[tokio::test]
    async fn graphql_denied_link_writes_report_denied() {
        let f = Fixture::new().await;
        let api = api(&f);
        let denied = run_as(
            &f,
            &api,
            r#"mutation($schema: String!) { updatePostTagsLinks(expectedSchema: $schema, key: {id: "1"}, add: [{id: "10"}]) { record { id } } }"#,
            f.authority(false),
        )
        .await;
        assert_eq!(
            denied["errors"][0]["extensions"]["code"], "denied",
            "{denied}"
        );
        assert!(f.links("mm_post_tags").await.is_empty());
        f.finish().await;
    }
}
