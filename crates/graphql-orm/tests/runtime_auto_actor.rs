//! Host-supplied actor stamps (`RuntimeField::auto_actor`). Each PostgreSQL case owns a
//! verified disposable server; SQLite uses a private temporary database file.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use futures::future::BoxFuture;
use graphql_orm::{graphql::errors::OrmPublicError, graphql::orm::*, prelude::*};
use std::sync::{Arc, Mutex};
#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

const TABLES: [&str; 4] = ["aac_parents", "aac_principals", "aac_docs", "aac_entrys"];

fn fid(id: &str) -> FieldId {
    FieldId::new(id).unwrap()
}
fn cid(id: &str) -> CollectionId {
    CollectionId::new(id).unwrap()
}
fn field(collection: &str, name: &str, kind: RuntimeValueKind, nullable: bool) -> RuntimeField {
    RuntimeField {
        description: None,
        classification: None,
        export: None,
        id: fid(&format!("{collection}.{name}")),
        api_name: name.into(),
        physical_column: name.to_ascii_lowercase(),
        value_kind: kind,
        nullable,
        unique: false,
        filterable: false,
        sortable: false,
        generated: false,
        default: None,
        format: None,
        auto_timestamp: None,
        auto_actor: None,
    }
}
fn actor(collection: &str, name: &str, kind: RuntimeValueKind) -> RuntimeField {
    RuntimeField {
        auto_actor: Some(RuntimeAutoActor::OnCreate),
        ..field(collection, name, kind, true)
    }
}
fn collection(id: &str, type_name: &str, fields: Vec<RuntimeField>) -> RuntimeCollection {
    RuntimeCollection {
        description: None,
        classification: None,
        id: cid(id),
        api_type_name: type_name.into(),
        api_plural_name: format!("{type_name}s"),
        physical_table: format!("aac_{id}s"),
        primary_key: vec![fid(&format!("{id}.id"))],
        append_only: false,
        retention_purge: false,
        fields,
        relations: vec![],
        indexes: vec![],
        composite_unique: vec![],
        default_order: vec![],
        label_fields: Vec::new(),
        search: None,
    }
}
fn set_null(id: &str, api: &str, target: &str, source: &str, target_key: &str) -> RuntimeRelation {
    RuntimeRelation {
        description: None,
        classification: None,
        export: None,
        id: RelationId::new(id).unwrap(),
        api_name: api.into(),
        target: cid(target),
        key_pairs: vec![RelationKeyPair {
            source: fid(source),
            target: fid(target_key),
        }],
        cardinality: RelationCardinality::One,
        enforce_foreign_key: true,
        on_delete: Some(DeletePolicy::SetNull),
        join: None,
    }
}
/// Docs carry two Uuid actor stamps; `auditorId` is also a SetNull key to a principal.
/// Entries carry one String actor stamp.
fn definition() -> RuntimeSchema {
    // Sortable keys give runtime GraphQL its deterministic keyset order.
    let id = |c: &str, kind| RuntimeField {
        sortable: true,
        filterable: true,
        ..field(c, "id", kind, false)
    };
    let label = |c: &str| field(c, "label", RuntimeValueKind::String, true);
    let parent = collection(
        "parent",
        "Parent",
        vec![id("parent", RuntimeValueKind::Integer), label("parent")],
    );
    let principal = collection(
        "principal",
        "Principal",
        vec![id("principal", RuntimeValueKind::Uuid), label("principal")],
    );
    let mut doc = collection(
        "doc",
        "Doc",
        vec![
            id("doc", RuntimeValueKind::Integer),
            field("doc", "title", RuntimeValueKind::String, true),
            field("doc", "parentId", RuntimeValueKind::Integer, true),
            actor("doc", "authorId", RuntimeValueKind::Uuid),
            actor("doc", "auditorId", RuntimeValueKind::Uuid),
        ],
    );
    doc.relations = vec![
        set_null(
            "doc.parent",
            "parent",
            "parent",
            "doc.parentId",
            "parent.id",
        ),
        set_null(
            "doc.auditor",
            "auditor",
            "principal",
            "doc.auditorId",
            "principal.id",
        ),
    ];
    let entry = collection(
        "entry",
        "Entry",
        vec![
            id("entry", RuntimeValueKind::Integer),
            label("entry"),
            actor("entry", "actorName", RuntimeValueKind::String),
        ],
    );
    RuntimeSchema {
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![parent, principal, doc, entry],
    }
}
const DOC: usize = 2;

fn problems(schema: RuntimeSchema) -> Vec<(Option<String>, RuntimeAutoActorProblem)> {
    schema
        .validate()
        .unwrap_err()
        .into_diagnostics()
        .into_iter()
        .filter(|d| d.code == RuntimeSchemaDiagnosticCode::InvalidAutoActor)
        .map(|d| match d.detail {
            Some(RuntimeSchemaDiagnosticDetail::AutoActor(problem)) => (d.subject, problem),
            other => panic!("unexpected detail {other:?}"),
        })
        .collect()
}

#[test]
fn validation_accepts_uuid_and_string_fields_and_rejects_each_invalid_shape() {
    // Valid: nullable Uuid and String stamps, including one that is a relation key.
    definition().validate().unwrap();
    assert_eq!(
        RuntimeSchemaDiagnosticCode::InvalidAutoActor.as_str(),
        "invalid_auto_actor"
    );

    use RuntimeAutoActorProblem as P;
    type Case = (&'static str, fn(&mut RuntimeField), RuntimeAutoActorProblem);
    let cases: [Case; 6] = [
        (
            "unsupported_kind",
            |f| f.value_kind = RuntimeValueKind::Integer,
            P::UnsupportedKind,
        ),
        ("not_nullable", |f| f.nullable = false, P::NotNullable),
        ("generated", |f| f.generated = true, P::Generated),
        (
            "has_default",
            |f| {
                f.default = Some(RuntimeDefault::Literal(
                    "6a2f41a3-c54c-fce8-32d2-0324e1c32e22".into(),
                ))
            },
            P::HasDefault,
        ),
        (
            "auto_timestamp",
            |f| f.auto_timestamp = Some(RuntimeAutoTimestamp::OnCreate),
            P::AutoTimestamp,
        ),
        ("key_field", |_| {}, P::KeyField),
    ];
    for (name, mutate, problem) in cases {
        let mut bad = definition();
        let doc = &mut bad.collections[DOC];
        if problem == P::KeyField {
            // Turn the primary key itself into an actor stamp.
            doc.fields[0].auto_actor = Some(RuntimeAutoActor::OnCreate);
            doc.fields[0].value_kind = RuntimeValueKind::Uuid;
        } else {
            mutate(&mut doc.fields[3]);
        }
        let subject = if problem == P::KeyField {
            "doc.id"
        } else {
            "doc.authorId"
        };
        assert_eq!(
            problems(bad),
            vec![(Some(subject.into()), problem)],
            "{name}"
        );
        assert_eq!(problem.as_str(), name);
    }
}

#[test]
fn serialization_and_identity_include_the_flag_only_when_declared() {
    let with = definition().validate().unwrap();
    let mut plain = definition();
    for field in plain.collections.iter_mut().flat_map(|c| &mut c.fields) {
        field.auto_actor = None;
    }
    let json = serde_json::to_string(&plain).unwrap();
    assert!(!json.contains("auto_actor"));
    let plain = plain.validate().unwrap();
    let plain_canonical = String::from_utf8(plain.canonical_bytes().to_vec()).unwrap();
    assert!(!plain_canonical.contains("auto_actor"));

    let json = serde_json::to_value(with.schema()).unwrap();
    assert_eq!(
        json["collections"][DOC]["fields"][3]["auto_actor"],
        "on_create"
    );
    assert!(
        json["collections"][DOC]["fields"][2]
            .get("auto_actor")
            .is_none()
    );
    let reread: RuntimeSchema = serde_json::from_value(json.clone()).unwrap();
    let reread = reread.validate().unwrap();
    assert_eq!(reread.fingerprint(), with.fingerprint());
    let canonical = String::from_utf8(with.canonical_bytes().to_vec()).unwrap();
    assert!(canonical.contains("|generated=false|default=none|auto_actor=on_create"));
    // Removing the member restores the exact earlier canonical line.
    assert_eq!(
        canonical.replace("|auto_actor=on_create", ""),
        plain_canonical
    );
    // Write semantics: both the ID-bearing and the structural identity change.
    assert_ne!(with.fingerprint(), plain.fingerprint());
    assert_ne!(
        with.structural_fingerprint(),
        plain.structural_fingerprint()
    );
    assert_eq!(with.format_fingerprint(), plain.format_fingerprint());

    let mut unknown = json;
    unknown["collections"][DOC]["fields"][3]["auto_actor"] = "on_update".into();
    assert!(serde_json::from_value::<RuntimeSchema>(unknown).is_err());
}

#[test]
fn requests_accept_actor_fields_until_the_grant_decides() {
    let schema = definition().validate().unwrap();
    let doc = schema.resolve_collection(&cid("doc")).unwrap();
    let f = |name: &str| schema.resolve_field(&doc, &fid(name)).unwrap();
    assert_eq!(
        f("doc.authorId").auto_actor(),
        Some(RuntimeAutoActor::OnCreate)
    );
    assert_eq!(f("doc.title").auto_actor(), None);
    let raw = schema.schema().collections[DOC].fields.clone();
    assert!(!raw[3].is_client_writable() && raw[1].is_client_writable());
    let value = RuntimeValue::Uuid(graphql_orm::uuid::Uuid::new_v4());
    let id = (f("doc.id"), RuntimeValue::Integer(1));
    // Construction accepts a supplied actor (value or explicit null); execution
    // refuses it unless the grant permits assignment.
    for supplied in [value, RuntimeValue::Null] {
        schema
            .runtime_create_request(
                &doc,
                &[id.clone(), (f("doc.authorId"), supplied.clone())],
                None,
                Default::default(),
            )
            .unwrap();
        let key = schema.runtime_key(&doc, std::slice::from_ref(&id)).unwrap();
        schema
            .runtime_update_request(
                key,
                &[(f("doc.authorId"), supplied)],
                None,
                None,
                Default::default(),
            )
            .unwrap();
    }
    // Values are still type-checked at construction.
    assert_eq!(
        schema
            .runtime_create_request(
                &doc,
                &[id, (f("doc.authorId"), RuntimeValue::String("x".into()))],
                None,
                Default::default(),
            )
            .unwrap_err()
            .code(),
        RuntimeMutationErrorCode::InvalidInput
    );
}

struct Fixture {
    db: Database<Backend>,
    environment: Arc<RuntimeMutationEnvironment>,
    #[cfg(feature = "postgres")]
    owned: owned_postgres::OwnedPostgres,
    #[cfg(feature = "sqlite")]
    path: std::path::PathBuf,
}
impl Fixture {
    async fn new(label: &str) -> Self {
        #[cfg(feature = "sqlite")]
        let path = std::env::temp_dir().join(format!(
            "graphql-orm-{label}-{}.sqlite",
            graphql_orm::uuid::Uuid::new_v4().simple()
        ));
        #[cfg(feature = "sqlite")]
        let db = Database::<Backend>::connect_sqlite_with_options(
            &format!("sqlite://{}?mode=rwc", path.display()),
            graphql_orm::db::ConnectionOptions::default().max_connections(2),
        )
        .await
        .unwrap();
        #[cfg(feature = "postgres")]
        let owned = owned_postgres::OwnedPostgres::start(label).unwrap();
        #[cfg(feature = "postgres")]
        let db = Database::<Backend>::connect_postgres(&owned.url)
            .await
            .unwrap();
        let environment = Arc::new(migrate_steps(&db, definition(), "init").await.1);
        Self {
            db,
            environment,
            #[cfg(feature = "postgres")]
            owned,
            #[cfg(feature = "sqlite")]
            path,
        }
    }
    fn schema(&self) -> &ValidatedRuntimeSchema {
        self.environment.schema()
    }
    fn handle(&self, collection: &str) -> RuntimeCollectionHandle {
        self.schema().resolve_collection(&cid(collection)).unwrap()
    }
    fn field(&self, name: &str) -> RuntimeFieldHandle {
        let collection = name.split('.').next().unwrap();
        self.schema()
            .resolve_field(&self.handle(collection), &fid(name))
            .unwrap()
    }
    fn all(&self, collection: &str) -> RuntimeProjection {
        let c = self
            .schema()
            .schema()
            .collections
            .iter()
            .find(|c| c.id == cid(collection))
            .unwrap();
        let fields = c
            .fields
            .iter()
            .map(|f| self.field(f.id.as_str()))
            .collect::<Vec<_>>();
        self.schema()
            .resolve_projection(&self.handle(collection), &fields)
            .unwrap()
    }
    fn key(&self, collection: &str, value: RuntimeValue) -> RuntimeKey {
        self.schema()
            .runtime_key(
                &self.handle(collection),
                &[(self.field(&format!("{collection}.id")), value)],
            )
            .unwrap()
    }
    fn doc(&self, id: i64, extra: &[(&str, RuntimeValue)]) -> RuntimeMutationRequest {
        let mut fields = vec![
            (self.field("doc.id"), RuntimeValue::Integer(id)),
            (self.field("doc.title"), RuntimeValue::String("a".into())),
        ];
        fields.extend(extra.iter().map(|(n, v)| (self.field(n), v.clone())));
        self.schema()
            .runtime_create_request(
                &self.handle("doc"),
                &fields,
                Some(self.all("doc")),
                Default::default(),
            )
            .unwrap()
    }
    fn update(&self, id: i64, fields: &[(&str, RuntimeValue)]) -> RuntimeMutationRequest {
        let fields = fields
            .iter()
            .map(|(n, v)| (self.field(n), v.clone()))
            .collect::<Vec<_>>();
        self.schema()
            .runtime_update_request(
                self.key("doc", RuntimeValue::Integer(id)),
                &fields,
                None,
                Some(self.all("doc")),
                Default::default(),
            )
            .unwrap()
    }
    async fn run(
        &self,
        request: RuntimeMutationRequest,
        authority: Authority,
    ) -> Result<RuntimeMutationEffect, TransactionError> {
        let environment = self.environment.clone();
        self.db
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    tx.mutate_runtime(&environment, &request, &authority)
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .await
    }
    fn authority(&self) -> Authority {
        Authority {
            schema: self.environment.schema().clone(),
            actor: None,
            assignment: false,
            narrow: false,
            staged: Arc::default(),
        }
    }
    async fn sql(&self, sql: &str) {
        Backend::fetch_rows(self.db.pool(), sql, &[]).await.unwrap();
    }
    async fn count(&self, sql: &str) -> i64 {
        let rows = Backend::fetch_rows(self.db.pool(), sql, &[]).await.unwrap();
        Backend::try_get_i64(&rows[0], "n").unwrap()
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
}

/// Plan, apply, prove the re-plan is a no-op and certify the target.
async fn migrate_steps(
    db: &Database<Backend>,
    definition: RuntimeSchema,
    label: &str,
) -> (Vec<String>, RuntimeMutationEnvironment) {
    let schema = Arc::new(definition.validate().unwrap());
    let target = schema
        .physical_schema::<Backend>(Default::default())
        .unwrap();
    let ownership = ManagedTableSet::new(TABLES.map(String::from)).unwrap();
    let plan = db
        .schema()
        .plan_owned_migration(
            label,
            "actor stamps",
            &target,
            &ownership,
            PlanOptions::strict(),
        )
        .await
        .unwrap();
    let steps = plan
        .steps()
        .iter()
        .map(|step| format!("{step:?}"))
        .collect();
    db.schema()
        .apply_owned_migration(&plan, ApplyOptions::default())
        .await
        .unwrap();
    let noop = db
        .schema()
        .plan_owned_migration(
            &format!("{label}-noop"),
            "unchanged",
            &target,
            &ownership,
            PlanOptions::strict(),
        )
        .await
        .unwrap();
    assert!(noop.steps().is_empty(), "{noop:?}");
    let environment = db
        .schema()
        .runtime_mutation_environment(schema, &target, &ownership)
        .await
        .unwrap();
    (steps, environment)
}

/// Approves every write of this isolated fixture. The grant carries `actor` and,
/// with `assignment`, permits requests to supply actor stamps. With `narrow` it
/// projects only the primary key. Every result check's staged record is kept.
#[derive(Clone)]
struct Authority {
    schema: Arc<ValidatedRuntimeSchema>,
    actor: Option<RuntimeValue>,
    assignment: bool,
    narrow: bool,
    staged: Arc<Mutex<Vec<RuntimeRecord>>>,
}
impl Authority {
    fn acting(mut self, actor: RuntimeValue) -> Self {
        self.actor = Some(actor);
        self
    }
    fn assigning(mut self) -> Self {
        self.assignment = true;
        self
    }
    fn narrow(mut self) -> Self {
        self.narrow = true;
        self
    }
    fn last_staged(&self) -> RuntimeRecord {
        self.staged.lock().unwrap().last().unwrap().clone()
    }
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
                .filter(|f| !self.narrow || f.api_name == "id")
                .map(|f| self.schema.resolve_field(handle, &f.id).unwrap())
                .collect::<Vec<_>>();
            let mut grant = RuntimeWriteGrant::new(
                self.schema.resolve_projection(handle, &fields).unwrap(),
                None,
            )?;
            if let Some(actor) = &self.actor {
                grant = grant.with_actor(actor.clone());
            }
            if self.assignment {
                grant = grant.with_actor_assignment();
            }
            Ok(grant)
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
        self.staged.lock().unwrap().push(check.record.clone());
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

/// The stored value, with `Null` for SQL NULL.
fn value_of(record: &RuntimeRecord, field: &RuntimeFieldHandle) -> RuntimeValue {
    match record.state(field).unwrap() {
        RuntimeFieldState::Null => RuntimeValue::Null,
        RuntimeFieldState::Value(value) => value.clone(),
        RuntimeFieldState::Unloaded => panic!("actor stamp not loaded"),
    }
}
fn uuid() -> RuntimeValue {
    RuntimeValue::Uuid(graphql_orm::uuid::Uuid::new_v4())
}
fn code(error: TransactionError) -> Option<String> {
    error
        .public_error()
        .runtime_mutation_code()
        .map(str::to_owned)
}
fn principal_sql(value: &RuntimeValue) -> String {
    let RuntimeValue::Uuid(id) = value else {
        unreachable!()
    };
    format!("INSERT INTO aac_principals (id) VALUES ('{id}')")
}

#[tokio::test]
async fn create_stamps_the_grant_actor_into_every_unsupplied_actor_field() {
    let f = Fixture::new("auto-actor-create").await;
    let (author, auditor) = (f.field("doc.authorId"), f.field("doc.auditorId"));
    let a = uuid();
    f.sql(&principal_sql(&a)).await;

    // Both stamps get the actor, also under a key-only grant with no return
    // projection: the result check sees them in the staged image.
    let authority = f.authority().acting(a.clone()).narrow();
    let create = f
        .schema()
        .runtime_create_request(
            &f.handle("doc"),
            &[(f.field("doc.id"), RuntimeValue::Integer(1))],
            None,
            Default::default(),
        )
        .unwrap();
    let effect = f.run(create, authority.clone()).await.unwrap();
    assert!(effect.record().is_none());
    assert_eq!(value_of(effect.staged(), &author), a);
    assert_eq!(value_of(effect.staged(), &auditor), a);
    let checked = authority.last_staged();
    assert_eq!(value_of(&checked, &author), a);
    assert_eq!(value_of(&checked, &auditor), a);

    // Persisted, and returned when projected.
    let effect = f
        .run(f.doc(2, &[]), f.authority().acting(a.clone()))
        .await
        .unwrap();
    assert_eq!(value_of(effect.record().unwrap(), &author), a);
    assert_eq!(
        f.count("SELECT COUNT(*) AS n FROM aac_docs WHERE authorid IS NOT NULL AND auditorid IS NOT NULL")
            .await,
        2
    );

    // Without an actor the stamps are NULL.
    let effect = f.run(f.doc(3, &[]), f.authority()).await.unwrap();
    let record = effect.record().unwrap();
    assert_eq!(value_of(record, &author), RuntimeValue::Null);
    assert_eq!(value_of(record, &auditor), RuntimeValue::Null);

    // An actor of the wrong kind is refused before any write.
    let error = f
        .run(
            f.doc(4, &[]),
            f.authority().acting(RuntimeValue::String("x".into())),
        )
        .await
        .unwrap_err();
    assert_eq!(code(error).as_deref(), Some("invalid_input"));
    assert_eq!(f.count("SELECT COUNT(*) AS n FROM aac_docs").await, 3);

    // A String stamp takes a String actor; a Uuid actor does not fit it.
    let entry = |id: i64| {
        f.schema()
            .runtime_create_request(
                &f.handle("entry"),
                &[(f.field("entry.id"), RuntimeValue::Integer(id))],
                Some(f.all("entry")),
                Default::default(),
            )
            .unwrap()
    };
    let effect = f
        .run(
            entry(1),
            f.authority().acting(RuntimeValue::String("alice".into())),
        )
        .await
        .unwrap();
    assert_eq!(
        value_of(effect.record().unwrap(), &f.field("entry.actorName")),
        RuntimeValue::String("alice".into())
    );
    let error = f
        .run(entry(2), f.authority().acting(uuid()))
        .await
        .unwrap_err();
    assert_eq!(code(error).as_deref(), Some("invalid_input"));
    assert_eq!(f.count("SELECT COUNT(*) AS n FROM aac_entrys").await, 1);
    f.finish().await;
}

#[tokio::test]
async fn supplied_actors_without_assignment_are_field_denied_before_any_write() {
    let f = Fixture::new("auto-actor-denied").await;
    let a = uuid();
    f.sql(&principal_sql(&a)).await;
    let author = f.field("doc.authorId");
    for supplied in [uuid(), RuntimeValue::Null] {
        let error = f
            .run(
                f.doc(1, &[("doc.authorId", supplied.clone())]),
                f.authority().acting(a.clone()),
            )
            .await
            .unwrap_err();
        assert_eq!(code(error).as_deref(), Some("field_denied"));
    }
    assert_eq!(f.count("SELECT COUNT(*) AS n FROM aac_docs").await, 0);

    let created = f
        .run(f.doc(1, &[]), f.authority().acting(a.clone()))
        .await
        .unwrap();
    assert_eq!(value_of(created.record().unwrap(), &author), a);
    for supplied in [uuid(), RuntimeValue::Null] {
        let error = f
            .run(
                f.update(
                    1,
                    &[
                        ("doc.title", RuntimeValue::String("changed".into())),
                        ("doc.authorId", supplied),
                    ],
                ),
                f.authority().acting(a.clone()),
            )
            .await
            .unwrap_err();
        assert_eq!(code(error).as_deref(), Some("field_denied"));
    }
    // Nothing was written: the title and the stamp are unchanged.
    let effect = f
        .run(
            f.update(1, &[("doc.title", RuntimeValue::String("a".into()))]),
            f.authority(),
        )
        .await
        .unwrap();
    assert_eq!(value_of(effect.record().unwrap(), &author), a);
    assert_eq!(
        f.count("SELECT COUNT(*) AS n FROM aac_docs WHERE title = 'changed'")
            .await,
        0
    );
    f.finish().await;
}

#[tokio::test]
async fn assignment_permits_supplied_actors_and_updates_never_restamp() {
    let f = Fixture::new("auto-actor-assign").await;
    let (author, auditor) = (f.field("doc.authorId"), f.field("doc.auditorId"));
    let (a, b, c, z) = (uuid(), uuid(), uuid(), uuid());
    f.sql(&principal_sql(&a)).await;

    // A supplied value overrides the actor; the other stamp still gets the actor.
    let effect = f
        .run(
            f.doc(1, &[("doc.authorId", b.clone())]),
            f.authority().acting(a.clone()).assigning(),
        )
        .await
        .unwrap();
    let record = effect.record().unwrap();
    assert_eq!(value_of(record, &author), b);
    assert_eq!(value_of(record, &auditor), a);

    // An assignment-permitted update changes it.
    let effect = f
        .run(
            f.update(1, &[("doc.authorId", c.clone())]),
            f.authority().acting(a.clone()).assigning(),
        )
        .await
        .unwrap();
    assert_eq!(value_of(effect.record().unwrap(), &author), c);
    assert_eq!(value_of(effect.staged(), &author), c);
    assert_eq!(value_of(effect.preimage().unwrap(), &author), b);

    // Updates that do not supply it leave it unchanged, with or without
    // assignment, whoever the acting principal is.
    for authority in [
        f.authority().acting(a.clone()).assigning(),
        f.authority().acting(z.clone()),
        f.authority(),
    ] {
        let effect = f
            .run(
                f.update(1, &[("doc.title", RuntimeValue::String("b".into()))]),
                authority,
            )
            .await
            .unwrap();
        let record = effect.record().unwrap();
        assert_eq!(value_of(record, &author), c);
        assert_eq!(value_of(record, &auditor), a);
    }
    // An explicit null is an assignment too.
    let effect = f
        .run(
            f.update(1, &[("doc.authorId", RuntimeValue::Null)]),
            f.authority().assigning(),
        )
        .await
        .unwrap();
    assert_eq!(
        value_of(effect.record().unwrap(), &author),
        RuntimeValue::Null
    );
    f.finish().await;
}

#[tokio::test]
async fn dependent_set_null_updates_keep_actor_stamps_except_the_cleared_key() {
    let f = Fixture::new("auto-actor-dependents").await;
    let (author, auditor) = (f.field("doc.authorId"), f.field("doc.auditorId"));
    let a = uuid();
    f.sql(&principal_sql(&a)).await;
    f.sql("INSERT INTO aac_parents (id) VALUES (1)").await;
    f.run(
        f.doc(1, &[("doc.parentId", RuntimeValue::Integer(1))]),
        f.authority().acting(a.clone()),
    )
    .await
    .unwrap();

    let delete = |collection: &str, key: RuntimeValue| {
        f.schema()
            .runtime_delete_request(f.key(collection, key), None, None, Default::default())
            .unwrap()
            .with_dependent_changes(RuntimeDependentLimits::default())
            .unwrap()
    };
    // Deleting the parent nulls `parentId` under another acting principal; the
    // actor stamps are untouched.
    let effect = f
        .run(
            delete("parent", RuntimeValue::Integer(1)),
            f.authority().acting(uuid()),
        )
        .await
        .unwrap();
    let staged = effect.dependents()[0].staged();
    assert_eq!(
        value_of(staged, &f.field("doc.parentId")),
        RuntimeValue::Null
    );
    assert_eq!(value_of(staged, &author), a);
    assert_eq!(value_of(staged, &auditor), a);

    // An actor stamp that is itself a SetNull key is cleared by the referential
    // action, without assignment permission; the other stamp is untouched.
    let effect = f
        .run(delete("principal", a.clone()), f.authority())
        .await
        .unwrap();
    let staged = effect.dependents()[0].staged();
    assert_eq!(value_of(staged, &auditor), RuntimeValue::Null);
    assert_eq!(value_of(staged, &author), a);
    assert_eq!(
        f.count(
            "SELECT COUNT(*) AS n FROM aac_docs WHERE auditorid IS NULL AND authorid IS NOT NULL"
        )
        .await,
        1
    );
    f.finish().await;
}

#[tokio::test]
async fn adding_an_actor_column_keeps_rows_null_and_toggling_it_is_catalog_only() {
    let f = Fixture::new("auto-actor-migration").await;
    f.run(f.doc(1, &[]), f.authority()).await.unwrap();
    f.run(f.doc(2, &[]), f.authority()).await.unwrap();

    // A new actor column is a plain nullable column: existing rows stay NULL.
    let mut added = definition();
    added.collections[DOC]
        .fields
        .push(actor("doc", "editorId", RuntimeValueKind::Uuid));
    let (steps, _) = migrate_steps(&f.db, added.clone(), "add").await;
    assert_eq!(steps.len(), 1, "{steps:?}");
    assert!(
        steps[0].contains("AddColumn")
            && steps[0].contains("nullable: true")
            && steps[0].contains("default: None"),
        "{steps:?}"
    );
    assert_eq!(
        f.count("SELECT COUNT(*) AS n FROM aac_docs WHERE editorid IS NULL")
            .await,
        2
    );

    // Switching the flag off and on again changes no physical definition.
    let mut off = added.clone();
    off.collections[DOC].fields.last_mut().unwrap().auto_actor = None;
    let (steps, _) = migrate_steps(&f.db, off.clone(), "flag-off").await;
    assert!(steps.is_empty(), "{steps:?}");
    let (steps, environment) = migrate_steps(&f.db, added, "flag-on").await;
    assert!(steps.is_empty(), "{steps:?}");

    // The certified revision stamps the new column; existing rows keep NULL.
    let schema = environment.schema().clone();
    let environment = Arc::new(environment);
    let doc = schema.resolve_collection(&cid("doc")).unwrap();
    let editor = schema.resolve_field(&doc, &fid("doc.editorId")).unwrap();
    let request = schema
        .runtime_create_request(
            &doc,
            &[(
                schema.resolve_field(&doc, &fid("doc.id")).unwrap(),
                RuntimeValue::Integer(3),
            )],
            Some(
                schema
                    .resolve_projection(&doc, std::slice::from_ref(&editor))
                    .unwrap(),
            ),
            Default::default(),
        )
        .unwrap();
    let a = uuid();
    f.sql(&principal_sql(&a)).await;
    let authority = Authority {
        schema: schema.clone(),
        ..f.authority()
    }
    .acting(a.clone());
    let effect =
        f.db.transaction(TransactionMode::StateMachine, move |tx| {
            Box::pin(async move {
                tx.mutate_runtime(&environment, &request, &authority)
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await
        .unwrap();
    assert_eq!(value_of(effect.record().unwrap(), &editor), a);
    assert_eq!(
        f.count("SELECT COUNT(*) AS n FROM aac_docs WHERE editorid IS NULL")
            .await,
        2
    );
    f.finish().await;
}

#[cfg(feature = "runtime-graphql")]
#[tokio::test]
async fn graphql_omits_actor_inputs_by_default_and_offers_them_on_opt_in() {
    use graphql_orm::async_graphql::{Request, Variables};
    use graphql_orm::graphql::runtime::*;
    struct Read;
    impl RuntimeReadAuthority for Read {
        fn authorize_mutation_result<'a>(
            &'a self,
            _: RuntimeMutationOutputCheck<'a>,
        ) -> BoxFuture<'a, Result<(), RuntimeGraphqlError>> {
            Box::pin(async { Ok(()) })
        }
        fn authorize<'a>(
            &'a self,
            _: RuntimeReadCheck<'a>,
        ) -> BoxFuture<'a, Result<RuntimeReadGrant, RuntimeGraphqlError>> {
            Box::pin(async { Ok(RuntimeReadGrant::new(None)) })
        }
    }
    let f = Fixture::new("auto-actor-graphql").await;
    let api = |actor_inputs: bool| {
        let mut options = RuntimeGraphqlMutationOptions::default();
        options.actor_inputs = actor_inputs;
        let module = RuntimeGraphqlModule::compile(
            Arc::new(f.schema().clone()),
            RuntimeGraphqlOptions::default()
                .with_cursor_profile(RuntimeCursorProfile::Unprotected)
                .with_mutations(options),
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
    };
    let block = |sdl: &str, name: &str| {
        let start = sdl.find(name).unwrap_or_else(|| panic!("{name} in {sdl}"));
        let end = start + sdl[start..].find('}').unwrap();
        sdl[start..end].to_owned()
    };

    // Default: the stamps are output only.
    let closed = api(false);
    let sdl = closed.sdl();
    let doc = block(&sdl, "type Doc ");
    assert!(
        doc.contains("authorId:") && doc.contains("auditorId:"),
        "{doc}"
    );
    for input in ["input CreateDocInput ", "input UpdateDocInput "] {
        let input = block(&sdl, input);
        assert!(input.contains("title"), "{input}");
        assert!(
            !input.contains("authorId") && !input.contains("auditorId"),
            "{input}"
        );
    }
    // Only `id` and `label` are writable on entries; `actorName` is not input.
    assert!(!block(&sdl, "input CreateEntryInput ").contains("actorName"));

    // Opt-in: optional inputs on both create and update.
    let open = api(true);
    let sdl = open.sdl();
    for input in ["input CreateDocInput ", "input UpdateDocInput "] {
        let input = block(&sdl, input);
        for name in ["authorId", "auditorId"] {
            let line = input
                .lines()
                .find(|line| line.trim_start().starts_with(&format!("{name}:")))
                .unwrap_or_else(|| panic!("{name} in {input}"));
            assert!(!line.trim_end().ends_with('!'), "{line}");
        }
    }

    let a = uuid();
    let RuntimeValue::Uuid(a_id) = &a else {
        unreachable!()
    };
    f.sql(&principal_sql(&a)).await;
    let execute =
        |api: &graphql_orm::async_graphql::dynamic::Schema, query: &str, authority: Authority| {
            let request = Request::new(query)
                .variables(Variables::from_json(
                    serde_json::json!({"schema": f.schema().fingerprint().to_string()}),
                ))
                .data(
                    RuntimeGraphqlRequest::<Backend>::new(
                        f.schema().fingerprint(),
                        Arc::new(Read),
                        None,
                        Default::default(),
                    )
                    .with_mutation_environment(f.environment.clone())
                    .with_write_authority(Arc::new(authority)),
                );
            let api = api.clone();
            async move { api.execute(request).await }
        };
    // The grant's actor survives the GraphQL restriction of the grant.
    let response = execute(
        &closed,
        "mutation($schema: String!) { createDoc(expectedSchema: $schema, input: {id: \"1\", title: \"a\"}) { record { authorId auditorId } } }",
        f.authority().acting(a.clone()),
    )
    .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data["createDoc"]["record"]["authorId"], a_id.to_string());
    assert_eq!(data["createDoc"]["record"]["auditorId"], a_id.to_string());

    // A supplied actor needs assignment permission even when the input exists.
    let b = graphql_orm::uuid::Uuid::new_v4();
    let supplied = format!(
        "mutation($schema: String!) {{ createDoc(expectedSchema: $schema, input: {{id: \"2\", title: \"b\", authorId: \"{b}\"}}) {{ record {{ authorId auditorId }} }} }}"
    );
    let response = execute(&open, &supplied, f.authority().acting(a.clone())).await;
    assert_eq!(response.errors.len(), 1, "{:?}", response.errors);
    assert_eq!(response.errors[0].message, "field_denied");
    assert!(!format!("{:?}", response.errors).contains(&b.to_string()));
    assert_eq!(f.count("SELECT COUNT(*) AS n FROM aac_docs").await, 1);

    let response = execute(
        &open,
        &supplied,
        f.authority().acting(a.clone()).assigning(),
    )
    .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data["createDoc"]["record"]["authorId"], b.to_string());
    assert_eq!(data["createDoc"]["record"]["auditorId"], a_id.to_string());

    // Without the opt-in the input field does not exist.
    let response = execute(
        &closed,
        &supplied.replace("id: \"2\"", "id: \"3\""),
        f.authority().assigning(),
    )
    .await;
    assert_eq!(response.errors.len(), 1, "{:?}", response.errors);
    assert_eq!(f.count("SELECT COUNT(*) AS n FROM aac_docs").await, 2);
    f.finish().await;
}
