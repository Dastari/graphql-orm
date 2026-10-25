//! Server-managed runtime timestamps (`RuntimeField::auto_timestamp`). Each PostgreSQL
//! case owns a verified disposable server; SQLite uses a private temporary database file.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use futures::future::BoxFuture;
use graphql_orm::{graphql::errors::OrmPublicError, graphql::orm::*, prelude::*};
use std::sync::Arc;
#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

const TABLES: [&str; 4] = ["ats_parents", "ats_notes", "ats_tags", "ats_note_tags"];

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
    }
}
fn stamp(collection: &str, name: &str, auto: RuntimeAutoTimestamp) -> RuntimeField {
    RuntimeField {
        auto_timestamp: Some(auto),
        ..field(collection, name, RuntimeValueKind::DateTime, false)
    }
}
fn collection(id: &str, type_name: &str, fields: Vec<RuntimeField>) -> RuntimeCollection {
    RuntimeCollection {
        description: None,
        classification: None,
        id: cid(id),
        api_type_name: type_name.into(),
        api_plural_name: format!("{type_name}s"),
        physical_table: format!("ats_{id}s"),
        primary_key: vec![fid(&format!("{id}.id"))],
        append_only: false,
        retention_purge: false,
        fields,
        relations: vec![],
        indexes: vec![],
        composite_unique: vec![],
        default_order: vec![],
        label_fields: Vec::new(),
    }
}
/// Notes carry both timestamps, a set-null parent and many-to-many tags.
fn definition() -> RuntimeSchema {
    // Sortable keys give runtime GraphQL its deterministic keyset order.
    let id = |c: &str| RuntimeField {
        sortable: true,
        filterable: true,
        ..field(c, "id", RuntimeValueKind::Integer, false)
    };
    // A writable member keeps runtime GraphQL update inputs non-empty.
    let label = |c: &str| field(c, "label", RuntimeValueKind::String, true);
    let parent = collection("parent", "Parent", vec![id("parent"), label("parent")]);
    let tag = collection("tag", "Tag", vec![id("tag"), label("tag")]);
    let mut note = collection(
        "note",
        "Note",
        vec![
            id("note"),
            field("note", "title", RuntimeValueKind::String, false),
            field("note", "parentId", RuntimeValueKind::Integer, true),
            stamp("note", "createdAt", RuntimeAutoTimestamp::OnCreate),
            stamp("note", "updatedAt", RuntimeAutoTimestamp::OnCreateAndUpdate),
        ],
    );
    note.relations = vec![
        RuntimeRelation {
            description: None,
            classification: None,
            export: None,
            id: RelationId::new("note.parent").unwrap(),
            api_name: "parent".into(),
            target: cid("parent"),
            key_pairs: vec![RelationKeyPair {
                source: fid("note.parentId"),
                target: fid("parent.id"),
            }],
            cardinality: RelationCardinality::One,
            enforce_foreign_key: true,
            on_delete: Some(DeletePolicy::SetNull),
            join: None,
        },
        RuntimeRelation {
            description: None,
            classification: None,
            export: None,
            id: RelationId::new("note.tags").unwrap(),
            api_name: "tags".into(),
            target: cid("tag"),
            key_pairs: vec![],
            cardinality: RelationCardinality::Many,
            enforce_foreign_key: false,
            on_delete: None,
            join: Some(RuntimeJoinTable {
                physical_table: "ats_note_tags".into(),
                source: vec![RuntimeJoinColumn {
                    field: fid("note.id"),
                    column: "note_id".into(),
                }],
                target: vec![RuntimeJoinColumn {
                    field: fid("tag.id"),
                    column: "tag_id".into(),
                }],
            }),
        },
    ];
    RuntimeSchema {
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![parent, note, tag],
    }
}

fn codes(schema: RuntimeSchema) -> Vec<RuntimeSchemaDiagnosticCode> {
    schema
        .validate()
        .unwrap_err()
        .into_diagnostics()
        .into_iter()
        .map(|d| d.code)
        .collect()
}
fn detail(schema: RuntimeSchema) -> Vec<(Option<String>, RuntimeAutoTimestampProblem)> {
    schema
        .validate()
        .unwrap_err()
        .into_diagnostics()
        .into_iter()
        .filter(|d| d.code == RuntimeSchemaDiagnosticCode::InvalidAutoTimestamp)
        .map(|d| match d.detail {
            Some(RuntimeSchemaDiagnosticDetail::AutoTimestamp(problem)) => (d.subject, problem),
            other => panic!("unexpected detail {other:?}"),
        })
        .collect()
}

#[test]
fn validation_accepts_datetime_fields_and_rejects_every_invalid_combination() {
    // Valid: both kinds, nullable or not, with no default or `CurrentTimestamp`.
    let mut valid = definition();
    let mut nullable = stamp("note", "seenAt", RuntimeAutoTimestamp::OnCreateAndUpdate);
    nullable.nullable = true;
    nullable.default = Some(RuntimeDefault::CurrentTimestamp);
    valid.collections[1].fields.push(nullable);
    valid.validate().unwrap();

    use RuntimeAutoTimestampProblem as P;
    let mut bad = definition();
    let note = &mut bad.collections[1];
    note.fields[1].auto_timestamp = Some(RuntimeAutoTimestamp::OnCreate); // string title
    note.fields[3].generated = true; // createdAt
    note.fields[4].default = Some(
        RuntimeDefault::datetime(&RuntimeDateTime::parse_exact("2026-01-01T00:00:00Z").unwrap())
            .unwrap(),
    ); // updatedAt
    let mut found = detail(bad);
    found.sort();
    assert_eq!(
        found,
        vec![
            (Some("note.createdAt".into()), P::Generated),
            (Some("note.title".into()), P::NotDateTime),
            (Some("note.updatedAt".into()), P::LiteralDefault),
        ]
    );

    // Primary keys and relation keys on either side are key fields.
    let mut keyed = definition();
    keyed.collections[0].fields[0] = RuntimeField {
        auto_timestamp: Some(RuntimeAutoTimestamp::OnCreate),
        ..field("parent", "id", RuntimeValueKind::DateTime, false)
    };
    keyed.collections[1].fields[2] = RuntimeField {
        auto_timestamp: Some(RuntimeAutoTimestamp::OnCreateAndUpdate),
        ..field("note", "parentId", RuntimeValueKind::DateTime, true)
    };
    // The DateTime keys only add the expected relation/join mismatch diagnostics.
    assert!(codes(keyed.clone()).contains(&RuntimeSchemaDiagnosticCode::InvalidAutoTimestamp));
    let mut found = detail(keyed);
    found.sort();
    assert_eq!(
        found,
        vec![
            (Some("note.parentId".into()), P::KeyField),
            (Some("parent.id".into()), P::KeyField),
        ]
    );
    // Diagnostics name the problem, never a value.
    assert_eq!(P::KeyField.as_str(), "key_field");
}

#[test]
fn serialization_and_identity_include_the_flag_only_when_declared() {
    let with = definition().validate().unwrap();
    let mut plain = definition();
    for field in &mut plain.collections[1].fields {
        field.auto_timestamp = None;
    }
    let json = serde_json::to_string(&plain).unwrap();
    assert!(!json.contains("auto_timestamp"));
    let plain = plain.validate().unwrap();
    assert!(
        !String::from_utf8(plain.canonical_bytes().to_vec())
            .unwrap()
            .contains("auto_timestamp")
    );

    let json = serde_json::to_value(with.schema()).unwrap();
    let fields = &json["collections"][1]["fields"];
    assert_eq!(fields[3]["auto_timestamp"], "on_create");
    assert_eq!(fields[4]["auto_timestamp"], "on_create_and_update");
    let reread: RuntimeSchema = serde_json::from_value(json.clone()).unwrap();
    let reread = reread.validate().unwrap();
    assert_eq!(reread.fingerprint(), with.fingerprint());
    assert_eq!(
        reread.structural_fingerprint(),
        with.structural_fingerprint()
    );
    let canonical = String::from_utf8(with.canonical_bytes().to_vec()).unwrap();
    assert!(canonical.contains("|default=none|auto_timestamp=on_create_and_update"));
    // Write semantics: both the ID-bearing and the structural identity change.
    assert_ne!(with.fingerprint(), plain.fingerprint());
    assert_ne!(
        with.structural_fingerprint(),
        plain.structural_fingerprint()
    );
    assert_eq!(with.format_fingerprint(), plain.format_fingerprint());
    // The two kinds are distinct identities.
    let mut swapped = with.schema().clone();
    swapped.collections[1].fields[3].auto_timestamp = Some(RuntimeAutoTimestamp::OnCreateAndUpdate);
    assert_ne!(
        swapped.validate().unwrap().fingerprint(),
        with.fingerprint()
    );

    let mut unknown = json;
    unknown["collections"][1]["fields"][3]["auto_timestamp"] = "on_read".into();
    assert!(serde_json::from_value::<RuntimeSchema>(unknown).is_err());
}

#[test]
fn requests_refuse_supplied_timestamps_and_do_not_require_them() {
    let schema = definition().validate().unwrap();
    let note = schema.resolve_collection(&cid("note")).unwrap();
    let f = |name: &str| schema.resolve_field(&note, &fid(name)).unwrap();
    assert_eq!(
        f("note.updatedAt").auto_timestamp(),
        Some(RuntimeAutoTimestamp::OnCreateAndUpdate)
    );
    assert_eq!(f("note.title").auto_timestamp(), None);
    let now = RuntimeValue::DateTime(RuntimeDateTime::parse("2026-01-01T00:00:00Z").unwrap());
    let base = [
        (f("note.id"), RuntimeValue::Integer(1)),
        (f("note.title"), RuntimeValue::String("a".into())),
    ];
    // Non-null timestamps are not missing_required on create.
    schema
        .runtime_create_request(&note, &base, None, Default::default())
        .unwrap();
    for stamped in ["note.createdAt", "note.updatedAt"] {
        let mut fields = base.to_vec();
        fields.push((f(stamped), now.clone()));
        let error = schema
            .runtime_create_request(&note, &fields, None, Default::default())
            .unwrap_err();
        assert_eq!(error.code(), RuntimeMutationErrorCode::FieldDenied);
        assert!(!format!("{error:?}{error}").contains("2026"));
        let key = schema
            .runtime_key(&note, &[(f("note.id"), RuntimeValue::Integer(1))])
            .unwrap();
        for value in [now.clone(), RuntimeValue::Null] {
            let error = schema
                .runtime_update_request(
                    key.clone(),
                    &[(f(stamped), value)],
                    None,
                    None,
                    Default::default(),
                )
                .unwrap_err();
            assert_eq!(error.code(), RuntimeMutationErrorCode::FieldDenied);
        }
    }
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
        let environment = Arc::new(migrate(&db, definition(), "init").await);
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
    fn note(&self) -> RuntimeCollectionHandle {
        self.schema().resolve_collection(&cid("note")).unwrap()
    }
    fn field(&self, name: &str) -> RuntimeFieldHandle {
        self.schema()
            .resolve_field(&self.note(), &fid(name))
            .unwrap()
    }
    fn all(&self) -> RuntimeProjection {
        let fields = [
            "note.id",
            "note.title",
            "note.parentId",
            "note.createdAt",
            "note.updatedAt",
        ]
        .map(|name| self.field(name));
        self.schema()
            .resolve_projection(&self.note(), &fields)
            .unwrap()
    }
    fn key(&self, id: i64) -> RuntimeKey {
        self.schema()
            .runtime_key(
                &self.note(),
                &[(self.field("note.id"), RuntimeValue::Integer(id))],
            )
            .unwrap()
    }
    fn create(&self, id: i64, parent: Option<i64>) -> RuntimeMutationRequest {
        self.schema()
            .runtime_create_request(
                &self.note(),
                &[
                    (self.field("note.id"), RuntimeValue::Integer(id)),
                    (self.field("note.title"), RuntimeValue::String("a".into())),
                    (
                        self.field("note.parentId"),
                        parent.map_or(RuntimeValue::Null, RuntimeValue::Integer),
                    ),
                ],
                Some(self.all()),
                Default::default(),
            )
            .unwrap()
    }
    fn retitle(&self, id: i64, title: &str) -> RuntimeMutationRequest {
        self.schema()
            .runtime_update_request(
                self.key(id),
                &[(self.field("note.title"), RuntimeValue::String(title.into()))],
                None,
                Some(self.all()),
                Default::default(),
            )
            .unwrap()
    }
    async fn run(&self, request: RuntimeMutationRequest) -> RuntimeMutationEffect {
        self.try_run(request, false).await.unwrap()
    }
    /// With `rollback`, the callback fails after the mutation succeeded.
    async fn try_run(
        &self,
        request: RuntimeMutationRequest,
        rollback: bool,
    ) -> Result<RuntimeMutationEffect, TransactionError> {
        let environment = self.environment.clone();
        let authority = Authority(self.environment.schema().clone());
        self.db
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let effect = tx
                        .mutate_runtime(&environment, &request, &authority)
                        .await
                        .map_err(OrmPublicError::from)?;
                    if rollback {
                        return Err(OrmPublicError::from(RuntimeMutationError::new(
                            RuntimeMutationErrorCode::HookFailed,
                        )));
                    }
                    Ok(effect)
                })
            })
            .await
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
async fn migrate(
    db: &Database<Backend>,
    definition: RuntimeSchema,
    label: &str,
) -> RuntimeMutationEnvironment {
    migrate_steps(db, definition, label).await.1
}
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
            "auto timestamps",
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

/// Approves every write of this isolated fixture, including link changes.
#[derive(Clone)]
struct Authority(Arc<ValidatedRuntimeSchema>);
impl RuntimeWriteAuthority<Backend> for Authority {
    fn authorize_intent<'a>(
        &'a self,
        check: RuntimeWriteIntent<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeWriteGrant, RuntimeMutationError>> {
        Box::pin(async move {
            let handle = check.collection();
            let fields = self
                .0
                .schema()
                .collections
                .iter()
                .find(|c| c.id == *handle.id())
                .unwrap()
                .fields
                .iter()
                .map(|f| self.0.resolve_field(handle, &f.id).unwrap())
                .collect::<Vec<_>>();
            RuntimeWriteGrant::new(self.0.resolve_projection(handle, &fields).unwrap(), None)
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

fn stamp_of(record: &RuntimeRecord, field: &RuntimeFieldHandle) -> RuntimeDateTime {
    match record.value(field).unwrap() {
        RuntimeValue::DateTime(value) => value.clone(),
        other => panic!("expected a stored date-time, got {other:?}"),
    }
}

/// Distinct clock readings on both backends (SQLite stores milliseconds).
async fn tick() {
    tokio::time::sleep(std::time::Duration::from_millis(15)).await;
}

#[tokio::test]
async fn create_sets_both_and_every_row_update_refreshes_only_on_create_and_update() {
    let f = Fixture::new("auto-timestamps-writes").await;
    let (created, updated) = (f.field("note.createdAt"), f.field("note.updatedAt"));
    let effect = f.run(f.create(1, None)).await;
    let record = effect.record().unwrap();
    let (c0, u0) = (stamp_of(record, &created), stamp_of(record, &updated));
    // One statement, one clock reading; canonical runtime encoding.
    assert_eq!(c0, u0);
    let text = c0.as_str();
    assert!(
        text.len() == 27 && text.ends_with('Z') && text.as_bytes()[10] == b'T',
        "{text}"
    );

    tick().await;
    let effect = f.run(f.retitle(1, "b")).await;
    let record = effect.record().unwrap();
    let (c1, u1) = (stamp_of(record, &created), stamp_of(record, &updated));
    assert_eq!(c1, c0);
    assert!(u1 > u0, "{u1:?} > {u0:?}");
    assert_eq!(stamp_of(effect.staged(), &updated), u1);

    // An update that changes no value still refreshes the timestamp.
    tick().await;
    let effect = f.run(f.retitle(1, "b")).await;
    let u2 = stamp_of(effect.record().unwrap(), &updated);
    assert!(u2 > u1);
    assert_eq!(stamp_of(effect.record().unwrap(), &created), c0);

    // A rolled-back update leaves the stored value unchanged.
    tick().await;
    let error = f.try_run(f.retitle(1, "c"), true).await.unwrap_err();
    assert_eq!(
        error.public_error().runtime_mutation_code(),
        Some("hook_failed")
    );
    tick().await;
    let effect = f.run(f.retitle(1, "d")).await;
    let record = effect.record().unwrap();
    assert!(stamp_of(record, &updated) > u2);
    assert_eq!(stamp_of(record, &created), c0);
    assert_eq!(
        f.count("SELECT COUNT(*) AS n FROM ats_notes WHERE title = 'c'")
            .await,
        0
    );
    f.finish().await;
}

#[tokio::test]
async fn rollback_leaves_values_unchanged() {
    let f = Fixture::new("auto-timestamps-rollback").await;
    f.run(f.create(1, None)).await;
    tick().await;
    f.try_run(f.retitle(1, "rolled back"), true)
        .await
        .unwrap_err();
    assert_eq!(
        f.count("SELECT COUNT(*) AS n FROM ats_notes WHERE updatedat = createdat AND title = 'a'")
            .await,
        1
    );
    f.finish().await;
}

#[tokio::test]
async fn set_null_dependents_refresh_and_link_only_updates_do_not() {
    let f = Fixture::new("auto-timestamps-dependents").await;
    let updated = f.field("note.updatedAt");
    f.sql("INSERT INTO ats_parents (id) VALUES (1)").await;
    f.sql("INSERT INTO ats_tags (id) VALUES (10)").await;
    let u0 = stamp_of(
        f.run(f.create(1, Some(1))).await.record().unwrap(),
        &updated,
    );

    // A standalone link mutation writes no row field, so the row is not updated.
    tick().await;
    let tags = f
        .schema()
        .resolve_relation(&f.note(), &RelationId::new("note.tags").unwrap())
        .unwrap();
    let tag = f.schema().resolve_collection(&cid("tag")).unwrap();
    let tag_key = f
        .schema()
        .runtime_key(
            &tag,
            &[(
                f.schema().resolve_field(&tag, &fid("tag.id")).unwrap(),
                RuntimeValue::Integer(10),
            )],
        )
        .unwrap();
    let link = f
        .schema()
        .runtime_link_request(
            &tags,
            f.key(1),
            RuntimeLinkChange::Replace(vec![tag_key]),
            Some(f.all()),
            10,
            Default::default(),
        )
        .unwrap();
    let effect = f.run(link).await;
    assert_eq!(effect.links()[0].added().len(), 1);
    assert_eq!(stamp_of(effect.record().unwrap(), &updated), u0);
    assert_eq!(
        f.count("SELECT COUNT(*) AS n FROM ats_notes WHERE updatedat = createdat")
            .await,
        1
    );

    // Deleting the parent nulls the note's key through the ORM: the row changed.
    tick().await;
    let parent = f.schema().resolve_collection(&cid("parent")).unwrap();
    let key = f
        .schema()
        .runtime_key(
            &parent,
            &[(
                f.schema()
                    .resolve_field(&parent, &fid("parent.id"))
                    .unwrap(),
                RuntimeValue::Integer(1),
            )],
        )
        .unwrap();
    let delete = f
        .schema()
        .runtime_delete_request(key, None, None, Default::default())
        .unwrap()
        .with_dependent_changes(RuntimeDependentLimits::default())
        .unwrap();
    let effect = f.run(delete).await;
    let dependent = &effect.dependents()[0];
    assert_eq!(dependent.action(), RuntimeMutationAction::Update);
    let staged = dependent.staged();
    assert_eq!(
        staged.value(&f.field("note.parentId")).unwrap_err().code(),
        RuntimeRecordErrorCode::NullValue
    );
    assert!(stamp_of(staged, &updated) > u0);
    assert_eq!(
        f.count(
            "SELECT COUNT(*) AS n FROM ats_notes WHERE updatedat > createdat AND parentid IS NULL"
        )
        .await,
        1
    );
    f.finish().await;
}

#[tokio::test]
async fn adding_auto_columns_backfills_rows_and_toggling_is_catalog_only_when_physical_matches() {
    let f = Fixture::new("auto-timestamps-migration").await;
    // Existing rows, then a revision that adds a third, required automatic timestamp.
    f.run(f.create(1, None)).await;
    f.run(f.create(2, None)).await;
    let mut added = definition();
    added.collections[1].fields.push(stamp(
        "note",
        "touchedAt",
        RuntimeAutoTimestamp::OnCreateAndUpdate,
    ));
    let (steps, environment) = migrate_steps(&f.db, added.clone(), "add").await;
    assert!(!steps.is_empty());
    assert_eq!(
        f.count("SELECT COUNT(*) AS n FROM ats_notes WHERE touchedat IS NOT NULL")
            .await,
        2
    );
    drop(environment);

    // Turning the flag off on a column that keeps an explicit CurrentTimestamp
    // default leaves the physical target unchanged: a catalog-only revision.
    let mut off = added.clone();
    let touched = off.collections[1].fields.last_mut().unwrap();
    touched.auto_timestamp = None;
    touched.default = Some(RuntimeDefault::CurrentTimestamp);
    let (steps, _) = migrate_steps(&f.db, off.clone(), "flag-off").await;
    assert!(steps.is_empty(), "{steps:?}");
    // And back on.
    let mut on = off.clone();
    on.collections[1].fields.last_mut().unwrap().auto_timestamp =
        Some(RuntimeAutoTimestamp::OnCreate);
    let (steps, _) = migrate_steps(&f.db, on, "flag-on").await;
    assert!(steps.is_empty(), "{steps:?}");

    // Enabling it on a column without a default changes the physical default: an
    // explicit migration step, never a silent catalog flip.
    let mut plain = off.clone();
    plain.collections[1].fields.last_mut().unwrap().default = None;
    migrate_steps(&f.db, plain.clone(), "no-default").await;
    let mut enabled = plain;
    enabled.collections[1]
        .fields
        .last_mut()
        .unwrap()
        .auto_timestamp = Some(RuntimeAutoTimestamp::OnCreateAndUpdate);
    let (steps, environment) = migrate_steps(&f.db, enabled, "enable").await;
    assert!(!steps.is_empty());
    // The certified revision writes through the new target.
    let schema = environment.schema().clone();
    let environment = Arc::new(environment);
    let note = schema.resolve_collection(&cid("note")).unwrap();
    let touched = schema.resolve_field(&note, &fid("note.touchedAt")).unwrap();
    let key = schema
        .runtime_key(
            &note,
            &[(
                schema.resolve_field(&note, &fid("note.id")).unwrap(),
                RuntimeValue::Integer(1),
            )],
        )
        .unwrap();
    let request = schema
        .runtime_update_request(
            key,
            &[(
                schema.resolve_field(&note, &fid("note.title")).unwrap(),
                RuntimeValue::String("z".into()),
            )],
            None,
            Some(
                schema
                    .resolve_projection(&note, std::slice::from_ref(&touched))
                    .unwrap(),
            ),
            Default::default(),
        )
        .unwrap();
    let authority = Authority(schema.clone());
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
    stamp_of(effect.record().unwrap(), &touched);
    f.finish().await;
}

#[cfg(feature = "runtime-graphql")]
#[tokio::test]
async fn graphql_inputs_omit_automatic_timestamps_and_refuse_supplied_values() {
    use graphql_orm::graphql::runtime::*;
    let f = Fixture::new("auto-timestamps-graphql").await;
    let module = RuntimeGraphqlModule::compile(
        Arc::new(f.schema().clone()),
        RuntimeGraphqlOptions::default()
            .with_cursor_profile(RuntimeCursorProfile::Unprotected)
            .with_mutations(Default::default()),
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
    let block = |name: &str| {
        let start = sdl.find(name).unwrap_or_else(|| panic!("{name} in {sdl}"));
        let end = start + sdl[start..].find('}').unwrap();
        sdl[start..end].to_owned()
    };
    // The record type still exposes both timestamps as output.
    let note = block("type Note ");
    assert!(
        note.contains("createdAt: RuntimeDateTime!")
            && note.contains("updatedAt: RuntimeDateTime!")
    );
    for input in ["input CreateNoteInput ", "input UpdateNoteInput "] {
        let input = block(input);
        assert!(input.contains("title"), "{input}");
        assert!(
            !input.contains("createdAt") && !input.contains("updatedAt"),
            "{input}"
        );
    }
    // A supplied value is refused before any write, without echoing it.
    let response = api
        .execute(format!(
            "mutation {{ createNote(expectedSchema: \"{}\", input: {{id: 1, title: \"a\", createdAt: \"2026-01-01T00:00:00Z\"}}) {{ record {{ id }} }} }}",
            f.schema().fingerprint()
        ))
        .await;
    assert!(!response.errors.is_empty());
    assert_eq!(
        response.errors[0].message, "invalid_graphql_input",
        "{:?}",
        response.errors
    );
    assert_eq!(f.count("SELECT COUNT(*) AS n FROM ats_notes").await, 0);
    f.finish().await;
}
