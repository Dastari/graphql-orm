//! Runtime GraphQL for formatted fields: plain-scalar formats keep identical
//! SDL; reviewed projectors expose host output objects after authorization;
//! mutation hooks stage related typed writes atomically.
//! PostgreSQL cases own a verified disposable server.
#![cfg(all(
    feature = "runtime-graphql",
    any(
        all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
        all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
    )
))]
use futures::future::BoxFuture;
use graphql_orm::async_graphql::{Request, Value, Variables, dynamic::Schema};
use graphql_orm::{
    graphql::errors::OrmPublicError,
    graphql::{orm::*, runtime::*},
    prelude::*,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

const FILE_REF: &str = "example.file_ref";
const EMAIL: &str = "example.email";

/// Host-owned claim journal staged by the mutation hook in the same transaction.
#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(table = "format_file_claims", plural = "FormatFileClaims")]
struct FileClaim {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    file: String,
    profile: i64,
}

fn namespace(value: &str) -> RuntimeFormatNamespace {
    RuntimeFormatNamespace::new(value).unwrap()
}

fn format(value: &str, version: u16) -> RuntimeFieldFormat {
    RuntimeFieldFormat::new(namespace(value), version, RuntimeFormatPayload::empty()).unwrap()
}

fn field(
    id: &str,
    kind: RuntimeValueKind,
    nullable: bool,
    format: Option<RuntimeFieldFormat>,
) -> RuntimeField {
    RuntimeField {
        description: None,
        classification: None,
        export: None,
        id: FieldId::new(id).unwrap(),
        api_name: id.into(),
        physical_column: id.to_ascii_lowercase(),
        value_kind: kind,
        nullable,
        unique: false,
        filterable: true,
        sortable: true,
        generated: false,
        default: None,
        format,
        auto_timestamp: None,
        auto_actor: None,
    }
}

fn definition(formats: bool) -> RuntimeSchema {
    let file = formats.then(|| format(FILE_REF, 1));
    RuntimeSchema {
        repository_only: false,
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![RuntimeCollection {
            node_identity: None,
            live_subscriptions: None,
            description: None,
            classification: None,
            id: CollectionId::new("profiles").unwrap(),
            api_type_name: "Profile".into(),
            api_plural_name: "Profiles".into(),
            physical_table: "format_profiles".into(),
            primary_key: vec![FieldId::new("id").unwrap()],
            append_only: false,
            retention_purge: false,
            fields: vec![
                field("id", RuntimeValueKind::Integer, false, None),
                field("name", RuntimeValueKind::String, false, None),
                field(
                    "email",
                    RuntimeValueKind::String,
                    true,
                    formats.then(|| format(EMAIL, 1)),
                ),
                field("avatar", RuntimeValueKind::Uuid, true, file.clone()),
                field("banner", RuntimeValueKind::Uuid, false, file),
                field("coach", RuntimeValueKind::Integer, true, None),
            ],
            // Each profile's mentor and the profiles it mentors.
            relations: vec![
                relation("mentor", "coach", "id", RelationCardinality::One),
                relation("mentees", "id", "coach", RelationCardinality::Many),
            ],
            indexes: vec![],
            composite_unique: vec![],
            default_order: vec![],
            label_fields: Vec::new(),
            search: None,
        }],
    }
}

fn relation(
    name: &str,
    source: &str,
    target: &str,
    cardinality: RelationCardinality,
) -> RuntimeRelation {
    RuntimeRelation {
        description: None,
        classification: None,
        export: None,
        id: RelationId::new(name).unwrap(),
        api_name: name.into(),
        target: CollectionId::new("profiles").unwrap(),
        key_pairs: vec![RelationKeyPair {
            source: FieldId::new(source).unwrap(),
            target: FieldId::new(target).unwrap(),
        }],
        cardinality,
        enforce_foreign_key: false,
        on_delete: None,
        join: None,
    }
}

fn uuid(n: u8) -> uuid::Uuid {
    uuid::Uuid::from_bytes([n; 16])
}

/// One recorded projector call: values, selected fields and purpose.
type SeenCall = (Vec<RuntimeValue>, Vec<String>, RuntimeProjectionPurpose);

/// Example projector: resolves stored file IDs against a host catalogue.
struct FileProjector {
    contract: RuntimeProjectionContract,
    files: BTreeMap<uuid::Uuid, (String, i64)>,
    calls: AtomicUsize,
    seen: Mutex<Vec<SeenCall>>,
    behaviour: Mutex<Behaviour>,
}
#[derive(Clone, Copy, PartialEq)]
enum Behaviour {
    Normal,
    Fail,
    Deny,
    Hang,
    WrongKind,
    Inject,
}
impl FileProjector {
    fn new(may_be_absent: bool) -> Arc<Self> {
        Arc::new(Self {
            contract: RuntimeProjectionContract {
                namespace: namespace(FILE_REF),
                versions: 1..=1,
                value_kinds: BTreeSet::from([RuntimeValueKind::Uuid]),
                object: RuntimeProjectedObject::new(
                    "ExampleFile",
                    vec![
                        RuntimeProjectedField::new("id", RuntimeValueKind::Uuid, false),
                        RuntimeProjectedField::new("name", RuntimeValueKind::String, false),
                        RuntimeProjectedField::new("size", RuntimeValueKind::Integer, true),
                    ],
                )
                .unwrap(),
                may_be_absent,
            },
            files: BTreeMap::from([
                (uuid(1), ("one.png".into(), 10)),
                (uuid(2), ("two.png".into(), 20)),
            ]),
            calls: AtomicUsize::new(0),
            seen: Mutex::new(Vec::new()),
            behaviour: Mutex::new(Behaviour::Normal),
        })
    }
    fn set(&self, behaviour: Behaviour) {
        *self.behaviour.lock().unwrap() = behaviour;
    }
}
impl RuntimeFieldProjector for FileProjector {
    fn contract(&self) -> &RuntimeProjectionContract {
        &self.contract
    }
    fn resolve<'a>(
        &'a self,
        batch: RuntimeProjectionBatch<'a>,
    ) -> BoxFuture<'a, Result<RuntimeProjectionResults, RuntimeProjectionError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.seen.lock().unwrap().push((
                batch.values.to_vec(),
                batch.selected.iter().map(|s| s.to_string()).collect(),
                batch.purpose,
            ));
            let behaviour = *self.behaviour.lock().unwrap();
            match behaviour {
                Behaviour::Fail => return Err(RuntimeProjectionError::Unavailable),
                Behaviour::Deny => return Err(RuntimeProjectionError::Denied),
                Behaviour::Hang => std::future::pending::<()>().await,
                _ => {}
            }
            let mut results = RuntimeProjectionResults::new();
            for value in batch.values {
                let RuntimeValue::Uuid(id) = value else {
                    return Err(RuntimeProjectionError::Invalid);
                };
                results.insert(
                    value.clone(),
                    match self.files.get(id) {
                        Some((name, size)) => RuntimeProjected::Object(BTreeMap::from([
                            ("id".into(), value.clone()),
                            (
                                "name".into(),
                                if behaviour == Behaviour::WrongKind {
                                    RuntimeValue::Integer(1)
                                } else {
                                    RuntimeValue::String(name.clone())
                                },
                            ),
                            ("size".into(), RuntimeValue::Integer(*size)),
                        ])),
                        None => RuntimeProjected::Absent,
                    },
                );
            }
            if behaviour == Behaviour::Inject {
                results.insert(RuntimeValue::Uuid(uuid(9)), RuntimeProjected::Absent);
            }
            Ok(results)
        })
    }
}

struct Read {
    deny_avatar: AtomicBool,
    projections: Mutex<Vec<(String, Vec<String>)>>,
}
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
            for selection in &check.projections {
                assert!(check.projection.contains(selection.field));
                self.projections
                    .lock()
                    .unwrap()
                    .push((selection.field.id().to_string(), selection.fields.to_vec()));
            }
            if self.deny_avatar.load(Ordering::SeqCst)
                && check.projection.iter().any(|f| f.id().as_str() == "avatar")
            {
                return Err(RuntimeGraphqlError::denied());
            }
            Ok(RuntimeReadGrant::new(None))
        })
    }
}

struct Write {
    projection: RuntimeProjection,
}
impl RuntimeWriteAuthority<Backend> for Write {
    fn authorize_intent<'a>(
        &'a self,
        _check: RuntimeWriteIntent<'a>,
        _tx: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeWriteGrant, RuntimeMutationError>> {
        Box::pin(async move { RuntimeWriteGrant::new(self.projection.clone(), None) })
    }
    fn authorize_preimage<'a>(
        &'a self,
        _check: RuntimePreimageCheck<'a>,
        _tx: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<(), RuntimeMutationError>> {
        Box::pin(async { Ok(()) })
    }
    fn authorize_result<'a>(
        &'a self,
        check: RuntimeResultCheck<'a>,
        _tx: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeReturnGrant, RuntimeMutationError>> {
        Box::pin(async move { Ok(RuntimeReturnGrant::new(check.intent.returning().cloned())) })
    }
}

/// Stages a claim for the stored banner file in the original transaction; a
/// conflicting claim fails the whole mutation through the claim primary key.
struct ClaimHook {
    fail_after_claim: bool,
    id: RuntimeFieldHandle,
    banner: RuntimeFieldHandle,
}
impl RuntimeMutationHook<Backend> for ClaimHook {
    fn before_commit<'a>(
        &'a self,
        tx: &'a mut MutationContext<'_, Backend>,
        pending: &'a RuntimeMutationEffect,
    ) -> BoxFuture<'a, Result<(), RuntimeMutationError>> {
        Box::pin(async move {
            // The staged image is authoritative even when nothing is returned.
            let file = pending.staged().uuid(&self.banner)?;
            let profile = pending.staged().integer(&self.id)?;
            tx.insert::<FileClaim>(CreateFileClaimInput {
                file: file.to_string(),
                profile,
            })
            .await
            .map_err(|e| {
                RuntimeMutationError::new(RuntimeMutationErrorCode::Conflict).with_source(e)
            })?;
            if self.fail_after_claim {
                return Err(RuntimeMutationError::new(
                    RuntimeMutationErrorCode::HookFailed,
                ));
            }
            Ok(())
        })
    }
}

struct Fixture {
    db: Database<Backend>,
    schema: Arc<ValidatedRuntimeSchema>,
    environment: Arc<RuntimeMutationEnvironment>,
    #[cfg(feature = "postgres")]
    owned: owned_postgres::OwnedPostgres,
}
impl Fixture {
    async fn new() -> Self {
        #[cfg(feature = "sqlite")]
        let db = Database::<Backend>::connect_sqlite_with_options(
            "sqlite::memory:",
            graphql_orm::db::ConnectionOptions::default().max_connections(1),
        )
        .await
        .unwrap();
        #[cfg(feature = "postgres")]
        let owned = owned_postgres::OwnedPostgres::start("graphql-formats").unwrap();
        #[cfg(feature = "postgres")]
        let db = {
            let db = Database::<Backend>::connect_postgres(&owned.url)
                .await
                .unwrap();
            let name: String = sqlx::query_scalar("SELECT current_database()")
                .fetch_one(db.pool())
                .await
                .unwrap();
            assert!(owned.url.ends_with(&format!("/{name}")));
            db
        };
        let schema = Arc::new(definition(true).validate().unwrap());
        let target = schema
            .physical_schema::<Backend>(Default::default())
            .unwrap()
            .with_static_entities(&[FileClaim::metadata()])
            .unwrap();
        let ownership =
            ManagedTableSet::new(["format_profiles".into(), "format_file_claims".into()]).unwrap();
        let plan = db
            .schema()
            .plan_owned_migration("v1", "formats", &target, &ownership, PlanOptions::strict())
            .await
            .unwrap();
        db.schema()
            .apply_owned_migration(&plan, Default::default())
            .await
            .unwrap();
        let environment = Arc::new(
            db.schema()
                .runtime_mutation_environment(schema.clone(), &target, &ownership)
                .await
                .unwrap(),
        );
        for (id, name, email, avatar, banner) in [
            (1, "ada", Some("ada@example.test"), Some(uuid(1)), uuid(2)),
            (2, "bob", None, None, uuid(1)),
            (3, "cy", None, Some(uuid(7)), uuid(2)),
        ] {
            sqlx::query(&format!(
                "INSERT INTO format_profiles (id, name, email, avatar, banner) VALUES ({id}, '{name}', {}, {}, '{}')",
                email.map_or("NULL".into(), |e| format!("'{e}'")),
                avatar.map_or("NULL".into(), |a| format!("'{a}'")),
                banner
            ))
            .execute(db.pool())
            .await
            .unwrap();
        }
        // Ada mentors Bob and Cy; Ada has no mentor.
        sqlx::query("UPDATE format_profiles SET coach = 1 WHERE id IN (2, 3)")
            .execute(db.pool())
            .await
            .unwrap();
        Self {
            db,
            schema,
            environment,
            #[cfg(feature = "postgres")]
            owned,
        }
    }
    fn options(&self, projector: &Arc<FileProjector>) -> RuntimeGraphqlOptions {
        RuntimeGraphqlOptions::default()
            .with_cursor_profile(RuntimeCursorProfile::Unprotected)
            .with_mutations(Default::default())
            .project_field(FieldId::new("avatar").unwrap(), projector.clone())
            .project_field(FieldId::new("banner").unwrap(), projector.clone())
            .scalar_format(namespace(EMAIL), 1..=1)
    }
    fn api(&self, options: RuntimeGraphqlOptions) -> Result<Schema, RuntimeGraphqlDiagnostics> {
        let module = RuntimeGraphqlModule::compile(self.schema.clone(), options)?;
        Ok(
            RuntimeGraphqlComposer::new(self.db.clone(), "Query", Default::default())?
                .mutation_root("Mutation")?
                .install(module)?
                .finish()?,
        )
    }
    fn request(&self, read: &Arc<Read>, query: &str, budget: RuntimeGraphqlLimits) -> Request {
        let collection = self
            .schema
            .resolve_collection(&CollectionId::new("profiles").unwrap())
            .unwrap();
        let fields = ["id", "name", "email", "avatar", "banner"].map(|id| {
            self.schema
                .resolve_field(&collection, &FieldId::new(id).unwrap())
                .unwrap()
        });
        let projection = self
            .schema
            .resolve_projection(&collection, &fields)
            .unwrap();
        Request::new(query)
            .variables(Variables::from_json(serde_json::json!({
                "schema": self.schema.fingerprint().to_string()
            })))
            .data(
                RuntimeGraphqlRequest::<Backend>::new(
                    self.schema.fingerprint(),
                    read.clone(),
                    None,
                    budget,
                )
                .with_mutation_environment(self.environment.clone())
                .with_write_authority(Arc::new(Write { projection }))
                .with_mutation_hook(Arc::new(self.hook(false)))
                .with_projection_context(Arc::new("host-request-data")),
            )
    }
    fn hook(&self, fail_after_claim: bool) -> ClaimHook {
        let collection = self
            .schema
            .resolve_collection(&CollectionId::new("profiles").unwrap())
            .unwrap();
        let field = |id: &str| {
            self.schema
                .resolve_field(&collection, &FieldId::new(id).unwrap())
                .unwrap()
        };
        ClaimHook {
            fail_after_claim,
            id: field("id"),
            banner: field("banner"),
        }
    }
    async fn claims(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM format_file_claims")
            .fetch_one(self.db.pool())
            .await
            .unwrap()
    }
    async fn finish(self) {
        self.db.pool().close().await;
        #[cfg(feature = "postgres")]
        {
            let mut owned = self.owned;
            owned.cleanup().unwrap();
        }
    }
}

fn read() -> Arc<Read> {
    Arc::new(Read {
        deny_avatar: AtomicBool::new(false),
        projections: Mutex::new(Vec::new()),
    })
}

fn codes(response: &graphql_orm::async_graphql::Response) -> Vec<String> {
    response.errors.iter().map(|e| e.message.clone()).collect()
}

#[tokio::test]
async fn plain_scalar_formats_keep_identical_sdl_and_expose_descriptors() {
    let f = Fixture::new().await;
    let plain_schema = Arc::new(definition(false).validate().unwrap());
    let plain = RuntimeGraphqlComposer::new(f.db.clone(), "Query", Default::default())
        .unwrap()
        .mutation_root("Mutation")
        .unwrap()
        .install(
            RuntimeGraphqlModule::compile(
                plain_schema,
                RuntimeGraphqlOptions::default()
                    .with_cursor_profile(RuntimeCursorProfile::Unprotected)
                    .with_mutations(Default::default()),
            )
            .unwrap(),
        )
        .unwrap()
        .finish()
        .unwrap()
        .sdl();
    // Serve every format as a plain scalar: SDL is byte-identical.
    let scalar = f
        .api(
            RuntimeGraphqlOptions::default()
                .with_cursor_profile(RuntimeCursorProfile::Unprotected)
                .with_mutations(Default::default())
                .scalar_format(namespace(EMAIL), 1..=1)
                .scalar_format(namespace(FILE_REF), 1..=3),
        )
        .unwrap()
        .sdl();
    assert_eq!(scalar, plain);
    let collection = f
        .schema
        .resolve_collection(&CollectionId::new("profiles").unwrap())
        .unwrap();
    let email = f
        .schema
        .resolve_field(&collection, &FieldId::new("email").unwrap())
        .unwrap();
    assert_eq!(email.format(), Some(&format(EMAIL, 1)));
    let name = f
        .schema
        .resolve_field(&collection, &FieldId::new("name").unwrap())
        .unwrap();
    assert_eq!(name.format(), None);
    f.finish().await;
}

#[tokio::test]
async fn projected_objects_honour_selection_aliases_fragments_null_and_absence() {
    let f = Fixture::new().await;
    let projector = FileProjector::new(true);
    let api = f.api(f.options(&projector)).unwrap();
    let sdl = api.sdl();
    assert!(sdl.contains("avatar: ExampleFile\n"), "{sdl}");
    // Absence is allowed, so even the required stored banner outputs nullable.
    assert!(sdl.contains("banner: ExampleFile\n"), "{sdl}");
    assert!(sdl.contains("email: String\n"), "{sdl}");
    assert!(sdl.contains("type ExampleFile {"), "{sdl}");
    assert!(
        sdl.contains("avatar: RuntimeUuid\n"),
        "inputs stay scalar: {sdl}"
    );
    assert!(
        sdl.contains("avatar: RuntimeUuidFilter"),
        "filters stay scalar: {sdl}"
    );
    let read = read();
    let response = api
        .execute(f.request(
            &read,
            r#"{ profiles(orderBy:[{field:id}], where:{banner:{eq:"02020202-0202-0202-0202-020202020202"}}) {
                edges { node { id email picture: avatar { ...File __typename } avatar { size } banner { name } } }
            } }
            fragment File on ExampleFile { name label: name }"#,
            Default::default(),
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap(),
        serde_json::json!({"profiles": {"edges": [
            {"node": {"id": "1", "email": "ada@example.test",
                "picture": {"name": "one.png", "label": "one.png", "__typename": "ExampleFile"},
                "avatar": {"size": "10"}, "banner": {"name": "two.png"}}},
            {"node": {"id": "3", "email": null, "picture": null, "avatar": null,
                "banner": {"name": "two.png"}}}
        ]}})
    );
    // One batched call per projected field per layer, with distinct values and
    // the union of selected object fields; the read authority saw both projections.
    let seen = projector.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert!(seen.iter().any(|(values, selected, purpose)| values
        == &vec![RuntimeValue::Uuid(uuid(1)), RuntimeValue::Uuid(uuid(7))]
        && selected == &vec!["name".to_owned(), "size".to_owned()]
        && *purpose == RuntimeProjectionPurpose::Read));
    assert!(seen.iter().any(
        |(values, selected, _)| values == &vec![RuntimeValue::Uuid(uuid(2))]
            && selected == &vec!["name".to_owned()]
    ));
    assert_eq!(read.projections.lock().unwrap().len(), 2);
    f.finish().await;
}

#[tokio::test]
async fn denied_fields_failures_budgets_and_cancellation_are_value_free() {
    let f = Fixture::new().await;
    let projector = FileProjector::new(false);
    let api = f.api(f.options(&projector)).unwrap();
    let sdl = api.sdl();
    assert!(
        sdl.contains("banner: ExampleFile!") && sdl.contains("avatar: ExampleFile\n"),
        "{sdl}"
    );
    let query = "{ profiles(orderBy:[{field:id}]) { edges { node { avatar { id name } } } } }";
    // Denied stored field: the projector is never called and nothing leaks.
    let read_denied = read();
    read_denied.deny_avatar.store(true, Ordering::SeqCst);
    let denied = api
        .execute(f.request(&read_denied, query, Default::default()))
        .await;
    assert_eq!(codes(&denied), ["authorization_denied"]);
    assert_eq!(projector.calls.load(Ordering::SeqCst), 0);
    // Absence is not allowed by this contract (row 3 references an unknown file).
    let absent = api
        .execute(f.request(&read(), query, Default::default()))
        .await;
    assert_eq!(codes(&absent), ["projection_failed"]);
    let only_known =
        "{ profiles(where:{id:{lt:\"3\"}}) { edges { node { avatar { id name } } } } }";
    for (behaviour, code) in [
        (Behaviour::Fail, "projection_failed"),
        (Behaviour::Deny, "projection_denied"),
        (Behaviour::WrongKind, "projection_failed"),
        (Behaviour::Inject, "projection_failed"),
        (Behaviour::Hang, "projection_timeout"),
    ] {
        projector.set(behaviour);
        let budget = RuntimeGraphqlLimits {
            projection: RuntimeProjectionLimits {
                call_timeout: Duration::from_millis(50),
                ..Default::default()
            },
            ..Default::default()
        };
        let response = api.execute(f.request(&read(), only_known, budget)).await;
        assert_eq!(codes(&response), [code]);
        let text = format!("{:?}", response.errors);
        assert!(
            !text.contains("one.png") && !text.contains("01010101"),
            "{text}"
        );
    }
    projector.set(Behaviour::Normal);
    let ok = api
        .execute(f.request(&read(), only_known, Default::default()))
        .await;
    assert!(ok.errors.is_empty(), "{:?}", ok.errors);
    // Call and value budgets are admitted before any I/O.
    let calls = projector.calls.load(Ordering::SeqCst);
    for budget in [
        RuntimeProjectionLimits {
            max_calls: 1,
            ..Default::default()
        },
        RuntimeProjectionLimits {
            max_values_per_call: 1,
            ..Default::default()
        },
    ] {
        let response = api
            .execute(f.request(
                &read(),
                "{ profiles(first:2) { edges { node { avatar { id } banner { id } } } } }",
                RuntimeGraphqlLimits {
                    projection: budget,
                    ..Default::default()
                },
            ))
            .await;
        assert_eq!(codes(&response), ["cost_exceeded"]);
    }
    assert_eq!(projector.calls.load(Ordering::SeqCst), calls);
    // Invalid subselections are rejected by validation/planning.
    let invalid = api
        .execute(f.request(
            &read(),
            "{ profiles { edges { node { avatar { missing } } } } }",
            Default::default(),
        ))
        .await;
    assert!(!invalid.errors.is_empty());
    f.finish().await;
}

#[test]
fn compilation_rejects_missing_unsupported_and_colliding_projections() {
    let schema = Arc::new(definition(true).validate().unwrap());
    let projector = FileProjector::new(true);
    let compile = |options: RuntimeGraphqlOptions| {
        RuntimeGraphqlModule::compile(schema.clone(), options)
            .err()
            .map(|d| d.0[0].code())
    };
    let base =
        || RuntimeGraphqlOptions::default().with_cursor_profile(RuntimeCursorProfile::Unprotected);
    // Unsupported descriptor / missing adapter.
    assert_eq!(compile(base()), Some("missing_projection"));
    assert_eq!(
        compile(
            base()
                .scalar_format(namespace(FILE_REF), 2..=3)
                .scalar_format(namespace(EMAIL), 1..=1)
        ),
        Some("missing_projection")
    );
    // Incompatible namespace/version/kind and unformatted or unknown fields.
    let email_projector = || base().scalar_format(namespace(FILE_REF), 1..=1);
    assert_eq!(
        compile(email_projector().project_field(FieldId::new("email").unwrap(), projector.clone())),
        Some("incompatible_projection")
    );
    assert_eq!(
        compile(
            email_projector()
                .scalar_format(namespace(EMAIL), 1..=1)
                .project_field(FieldId::new("name").unwrap(), projector.clone())
        ),
        Some("invalid_composition")
    );
    assert_eq!(
        compile(
            email_projector()
                .scalar_format(namespace(EMAIL), 1..=1)
                .project_field(FieldId::new("nope").unwrap(), projector.clone())
        ),
        Some("invalid_composition")
    );
    // Primary-key and relation-key fields stay scalar in v1.
    let mut keyed = definition(true);
    keyed.collections[0].fields[0].format = Some(format(FILE_REF, 1));
    keyed.collections[0].fields[0].value_kind = RuntimeValueKind::Uuid;
    // The mentor relations join `coach` to `id`: keep their key kinds equal.
    keyed.collections[0].fields[5].value_kind = RuntimeValueKind::Uuid;
    let keyed = Arc::new(keyed.validate().unwrap());
    assert_eq!(
        RuntimeGraphqlModule::compile(
            keyed,
            base()
                .scalar_format(namespace(EMAIL), 1..=1)
                .project_field(FieldId::new("id").unwrap(), projector.clone())
                .project_field(FieldId::new("avatar").unwrap(), projector.clone())
                .project_field(FieldId::new("banner").unwrap(), projector.clone())
        )
        .err()
        .map(|d| d.0[0].code()),
        Some("unsupported_capability")
    );
    // A projected object may not shadow a generated type.
    let colliding = Arc::new(FileProjector {
        contract: RuntimeProjectionContract {
            object: RuntimeProjectedObject::new(
                "ProfileConnection",
                vec![RuntimeProjectedField::new(
                    "id",
                    RuntimeValueKind::Uuid,
                    false,
                )],
            )
            .unwrap(),
            ..projector.contract.clone()
        },
        files: BTreeMap::new(),
        calls: AtomicUsize::new(0),
        seen: Mutex::new(Vec::new()),
        behaviour: Mutex::new(Behaviour::Normal),
    });
    assert_eq!(
        compile(
            base()
                .scalar_format(namespace(EMAIL), 1..=1)
                .project_field(FieldId::new("avatar").unwrap(), colliding.clone())
                .project_field(FieldId::new("banner").unwrap(), colliding)
        ),
        Some("name_collision")
    );
    assert_eq!(
        RuntimeProjectedObject::new(
            "bad",
            vec![RuntimeProjectedField::new(
                "id",
                RuntimeValueKind::Uuid,
                false
            )]
        )
        .err()
        .map(|e| e.code()),
        Some("invalid_composition")
    );
}

#[tokio::test]
async fn mutation_outputs_project_after_commit_and_hooks_stage_claims_atomically() {
    let f = Fixture::new().await;
    let projector = FileProjector::new(true);
    let api = f.api(f.options(&projector)).unwrap();
    let read = read();
    let create = |id: i64, banner: &str| {
        format!(
            r#"mutation($schema:String!){{ createProfile(expectedSchema:$schema,
                input:{{id:"{id}", name:"new", banner:"{banner}"}}) {{
                record {{ id banner {{ name }} avatar {{ id }} }} }} }}"#
        )
    };
    let first = api
        .execute(f.request(
            &read,
            &create(10, "01010101-0101-0101-0101-010101010101"),
            Default::default(),
        ))
        .await;
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    assert_eq!(
        first.data.into_json().unwrap(),
        serde_json::json!({"createProfile": {"record": {
            "id": "10", "banner": {"name": "one.png"}, "avatar": null
        }}})
    );
    let mutation_call = projector.seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        mutation_call.2,
        RuntimeProjectionPurpose::MutationResult {
            action: RuntimeMutationAction::Create,
            kind: RuntimeMutationRecordKind::CommittedResult,
        }
    );
    assert_eq!(f.claims().await, 1);
    // A second claim of the same file conflicts inside the transaction: the profile
    // row and the claim both roll back, and no projection is resolved.
    let calls = projector.calls.load(Ordering::SeqCst);
    let conflict = api
        .execute(f.request(
            &read,
            &create(11, "01010101-0101-0101-0101-010101010101"),
            Default::default(),
        ))
        .await;
    assert_eq!(codes(&conflict), ["conflict"]);
    assert_eq!(f.claims().await, 1);
    let profiles: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM format_profiles WHERE id = 11")
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    assert_eq!(profiles, 0);
    assert_eq!(projector.calls.load(Ordering::SeqCst), calls);
    // A projection failure after commit reports the committed outcome without values.
    projector.set(Behaviour::Fail);
    let committed = api
        .execute(f.request(
            &read,
            &create(12, "02020202-0202-0202-0202-020202020202"),
            Default::default(),
        ))
        .await;
    assert_eq!(codes(&committed), ["mutation_payload_failed"]);
    assert_eq!(
        committed.errors[0]
            .extensions
            .as_ref()
            .and_then(|e| e.get("outcome")),
        Some(&Value::from("committed"))
    );
    assert_eq!(f.claims().await, 2);
    f.finish().await;
}

#[tokio::test]
async fn failing_hooks_roll_back_staged_claims_and_concurrent_claims_cannot_both_commit() {
    let f = Fixture::new().await;
    let collection = f
        .schema
        .resolve_collection(&CollectionId::new("profiles").unwrap())
        .unwrap();
    let field = |id: &str| {
        f.schema
            .resolve_field(&collection, &FieldId::new(id).unwrap())
            .unwrap()
    };
    let projection = f
        .schema
        .resolve_projection(&collection, &[field("id"), field("name"), field("banner")])
        .unwrap();
    let request = |id: i64| {
        f.schema
            .runtime_create_request(
                &collection,
                &[
                    (field("id"), RuntimeValue::Integer(id)),
                    (field("name"), RuntimeValue::String("claim".into())),
                    (field("banner"), RuntimeValue::Uuid(uuid(5))),
                ],
                None,
                Default::default(),
            )
            .unwrap()
    };
    let run = |request: RuntimeMutationRequest, fail: bool| {
        let environment = f.environment.clone();
        let authority = Write {
            projection: projection.clone(),
        };
        let hook = f.hook(fail);
        let db = f.db.clone();
        async move {
            db.transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let effect = tx
                        .mutate_runtime(&environment, &request, &authority)
                        .await
                        .map_err(OrmPublicError::from)?;
                    // Staged image carries the stored input even without a return projection.
                    assert!(effect.record().is_none());
                    assert!(effect.preimage().is_none());
                    tx.run_runtime_before_commit(&hook, &effect)
                        .await
                        .map_err(OrmPublicError::from)?;
                    Ok(())
                })
            })
            .await
        }
    };
    assert!(run(request(20), true).await.is_err());
    assert_eq!(f.claims().await, 0);
    let (a, b) = tokio::join!(run(request(21), false), run(request(22), false));
    assert_eq!(
        usize::from(a.is_ok()) + usize::from(b.is_ok()),
        1,
        "{a:?} {b:?}"
    );
    assert_eq!(f.claims().await, 1);
    let rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM format_profiles WHERE id IN (20, 21, 22)")
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert_eq!(rows, 1);
    f.finish().await;
}

/// The response budget charges a projected object once for every record that
/// carries it, not once per distinct stored value, so a page that repeats one
/// value is refused before the next projector runs (#528).
#[tokio::test]
async fn projection_budget_counts_every_record_that_shares_a_value() {
    let f = Fixture::new().await;
    let projector = FileProjector::new(true);
    let api = f.api(f.options(&projector)).unwrap();
    let read = read();
    // Banners: two.png (rows 1 and 3) and one.png (row 2). The smallest budget
    // for the banners alone pays for three copies; counted once per distinct
    // value, one byte less would leave a whole copy to spare.
    let banners = smallest_budget(
        &api,
        &f,
        &read,
        "{ profiles { edges { node { banner { name } } } } }",
    )
    .await;
    let query = "{ profiles { edges { node { banner { name } avatar { name } } } } }";
    let before = projector.calls.load(Ordering::SeqCst);
    let response = api
        .execute(f.request(
            &read,
            query,
            RuntimeGraphqlLimits {
                max_response_bytes: banners - 1,
                ..Default::default()
            },
        ))
        .await;
    assert_eq!(codes(&response), ["cost_exceeded"]);
    assert_eq!(
        projector.calls.load(Ordering::SeqCst) - before,
        1,
        "the avatar projector ran after the banners exceeded the budget"
    );
    f.finish().await;
}

/// The smallest response budget that serves `query`.
async fn smallest_budget(api: &Schema, f: &Fixture, read: &Arc<Read>, query: &str) -> usize {
    let run = async |budget: usize| {
        let limits = RuntimeGraphqlLimits {
            max_response_bytes: budget,
            ..Default::default()
        };
        api.execute(f.request(read, query, limits)).await
    };
    let maximum = RuntimeGraphqlLimits::default().max_response_bytes;
    let served = run(maximum).await;
    assert!(served.errors.is_empty(), "{query}: {:?}", served.errors);
    let (mut low, mut high) = (1usize, maximum);
    while low < high {
        let middle = low + (high - low) / 2;
        if run(middle).await.errors.is_empty() {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    low
}

/// Mutation roots of one request share the response budget, projections
/// included: at the smallest budget that serves both roots everything is
/// returned, and one byte less refuses the second root's payload, not the
/// first's. Both mutations commit either way (#528).
#[tokio::test]
async fn mutation_roots_share_the_response_budget_with_their_projections() {
    let f = Fixture::new().await;
    // Large projected names, so the committed payloads, not the planning
    // estimate, decide which budgets are enough.
    let mut projector = Arc::try_unwrap(FileProjector::new(true)).ok().unwrap();
    projector.files = BTreeMap::from([
        (uuid(1), ("o".repeat(2_000), 10)),
        (uuid(2), ("t".repeat(2_000), 20)),
    ]);
    let projector = Arc::new(projector);
    let api = f.api(f.options(&projector)).unwrap();
    let read = read();
    const BOTH: &str = r#"mutation($schema:String!){
        a: updateProfile(expectedSchema:$schema, key:{id:"1"}, input:{name:"a"}) { record { banner { name } } }
        b: updateProfile(expectedSchema:$schema, key:{id:"2"}, input:{name:"b"}) { record { banner { name } } }
    }"#;
    let run = async |budget: usize| {
        // Every write claims its banner; clear the claims between requests.
        sqlx::query("DELETE FROM format_file_claims")
            .execute(f.db.pool())
            .await
            .unwrap();
        let limits = RuntimeGraphqlLimits {
            max_response_bytes: budget,
            ..Default::default()
        };
        api.execute(f.request(&read, BOTH, limits)).await
    };
    let served = run(RuntimeGraphqlLimits::default().max_response_bytes).await;
    assert!(served.errors.is_empty(), "{:?}", served.errors);
    // The smallest budget that serves both roots.
    let (mut low, mut high) = (1usize, RuntimeGraphqlLimits::default().max_response_bytes);
    while low < high {
        let middle = low + (high - low) / 2;
        if run(middle).await.errors.is_empty() {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    // Count the projected objects copied into responses from here on.
    let copies = Arc::new(AtomicUsize::new(0));
    let _copies = tracing::subscriber::set_default(Events(PROJECTION_COPIES, copies.clone()));
    let exact = run(low).await;
    assert!(exact.errors.is_empty(), "{:?}", exact.errors);
    assert_eq!(
        copies.swap(0, Ordering::SeqCst),
        2,
        "both banners are copied"
    );
    sqlx::query("UPDATE format_profiles SET name = 'reset' WHERE id IN (1, 2)")
        .execute(f.db.pool())
        .await
        .unwrap();
    let short = run(low - 1).await;
    assert_eq!(
        codes(&short),
        ["mutation_payload_failed"],
        "{:?}",
        short.errors
    );
    // Root `b`'s banner, the one over the remainder, is refused before it is
    // copied: only root `a`'s is.
    assert_eq!(
        copies.load(Ordering::SeqCst),
        1,
        "the refused banner was copied"
    );
    // The first root was served; the second, the one over the remainder, is refused.
    // Root `b` is on line 3 of the operation.
    assert_eq!(
        short.errors[0]
            .locations
            .iter()
            .map(|pos| pos.line)
            .collect::<Vec<_>>(),
        [3],
        "{:?}",
        short.errors
    );
    let names: Vec<String> =
        sqlx::query_scalar("SELECT name FROM format_profiles WHERE id IN (1, 2) ORDER BY id")
            .fetch_all(f.db.pool())
            .await
            .unwrap();
    assert_eq!(names, ["a", "b"], "both mutations committed");
    f.finish().await;
}

/// A read's whole response structure is reserved before its projected
/// objects are copied: connection objects, edges, cursors and page fields;
/// each parent's relation key; the null of a to-one relation without a
/// related record; and every to-many connection, empty ones included. At the
/// smallest budget that serves the query its last projection is copied; one
/// byte less refuses before that copy, so the running total and the final
/// measure count the same bytes (#528).
#[tokio::test]
async fn read_structure_is_reserved_before_projections_are_copied() {
    let f = Fixture::new().await;
    let projector = FileProjector::new(true);
    let api = f.api(f.options(&projector)).unwrap();
    let read = read();
    // Each query with the copy events at its smallest budget and at one byte
    // less: the last projection is the one refused.
    for (query, at_exact, at_short) in [
        // Root connection overhead, after the only projection.
        (
            "{ profiles(orderBy:[{field:id}]) { totalCount pageInfo { hasNextPage endCursor } \
            edges { cursor node { banner { name } } } } }",
            1,
            0,
        ),
        // To-one: Ada's mentor is null; Bob's and Cy's is Ada.
        (
            "{ profiles(orderBy:[{field:id}]) { edges { node { id \
            mentor { banner { name } } } } } }",
            1,
            0,
        ),
        // To-many: Ada mentors two; Bob's and Cy's connections are empty.
        (
            "{ profiles(orderBy:[{field:id}]) { edges { node { id \
            mentees(first:5) { totalCount pageInfo { endCursor } edges { cursor node { banner { name } } } } } } } }",
            1,
            0,
        ),
        // Relations below projected parents (#704): the mentor keys, Ada's
        // null and the related records are reserved before the parents'
        // banners are copied.
        (
            "{ profiles(orderBy:[{field:id}]) { edges { node { banner { name } mentor { id } } } } }",
            1,
            0,
        ),
        // The same for to-many connections, empty ones included.
        (
            "{ profiles(orderBy:[{field:id}]) { edges { node { banner { name } \
            mentees(first:5) { totalCount pageInfo { endCursor } edges { cursor node { id name } } } } } } }",
            1,
            0,
        ),
        // Two levels below a projected parent.
        (
            "{ profiles(orderBy:[{field:id}]) { edges { node { banner { name } \
            mentor { name mentees(first:5) { edges { cursor node { id } } } } } } } }",
            1,
            0,
        ),
        // Projections on both layers: parents first, so only the related
        // records' projection is refused.
        (
            "{ profiles(orderBy:[{field:id}]) { edges { node { banner { name } \
            mentor { id banner { name } } } } } }",
            2,
            1,
        ),
    ] {
        let run = async |budget: usize| {
            let limits = RuntimeGraphqlLimits {
                max_response_bytes: budget,
                ..Default::default()
            };
            api.execute(f.request(&read, query, limits)).await
        };
        let served = run(RuntimeGraphqlLimits::default().max_response_bytes).await;
        let low = smallest_budget(&api, &f, &read, query).await;
        let copies = Arc::new(AtomicUsize::new(0));
        let _copies = tracing::subscriber::set_default(Events(PROJECTION_COPIES, copies.clone()));
        let exact = run(low).await;
        assert!(exact.errors.is_empty(), "{query}: {:?}", exact.errors);
        assert_eq!(exact.data, served.data, "{query}");
        assert_eq!(copies.swap(0, Ordering::SeqCst), at_exact, "{query}");
        let short = run(low - 1).await;
        assert_eq!(codes(&short), ["cost_exceeded"], "{query}");
        assert_eq!(
            copies.load(Ordering::SeqCst),
            at_short,
            "{query}: projected objects were copied before the response was refused"
        );
    }
    f.finish().await;
}

/// A committed record's relation layers are read and reserved before the
/// record's own projected objects are copied into the payload: one byte
/// below the smallest budget refuses with no copy. The mutation stays
/// committed (#704).
#[tokio::test]
async fn mutation_relation_structure_is_reserved_before_projections_are_copied() {
    let f = Fixture::new().await;
    // Large projected names, so the committed payloads, not the planning
    // estimate, decide which budgets are enough.
    let mut projector = Arc::try_unwrap(FileProjector::new(true)).ok().unwrap();
    projector.files = BTreeMap::from([
        (uuid(1), ("o".repeat(2_000), 10)),
        (uuid(2), ("t".repeat(2_000), 20)),
    ]);
    let projector = Arc::new(projector);
    let api = f.api(f.options(&projector)).unwrap();
    let read = read();
    for (query, at_exact, at_short) in [
        // Bob's mentor is Ada.
        (
            r#"mutation($schema:String!){ updateProfile(expectedSchema:$schema, key:{id:"2"}, input:{name:"b"}) { record { banner { name } mentor { id name } } } }"#,
            1,
            0,
        ),
        // Ada has no mentor: the null is reserved first too.
        (
            r#"mutation($schema:String!){ updateProfile(expectedSchema:$schema, key:{id:"1"}, input:{name:"a"}) { record { banner { name } mentor { id } } } }"#,
            1,
            0,
        ),
        // The record's own projection is copied before the related one.
        (
            r#"mutation($schema:String!){ updateProfile(expectedSchema:$schema, key:{id:"2"}, input:{name:"b"}) { record { banner { name } mentor { banner { name } } } } }"#,
            2,
            1,
        ),
    ] {
        let run = async |budget: usize| {
            // Every write claims its banner; clear the claims between requests.
            sqlx::query("DELETE FROM format_file_claims")
                .execute(f.db.pool())
                .await
                .unwrap();
            let limits = RuntimeGraphqlLimits {
                max_response_bytes: budget,
                ..Default::default()
            };
            api.execute(f.request(&read, query, limits)).await
        };
        let maximum = RuntimeGraphqlLimits::default().max_response_bytes;
        let served = run(maximum).await;
        assert!(served.errors.is_empty(), "{query}: {:?}", served.errors);
        let (mut low, mut high) = (1usize, maximum);
        while low < high {
            let middle = low + (high - low) / 2;
            if run(middle).await.errors.is_empty() {
                high = middle;
            } else {
                low = middle + 1;
            }
        }
        let copies = Arc::new(AtomicUsize::new(0));
        let _copies = tracing::subscriber::set_default(Events(PROJECTION_COPIES, copies.clone()));
        let exact = run(low).await;
        assert!(exact.errors.is_empty(), "{query}: {:?}", exact.errors);
        assert_eq!(exact.data, served.data, "{query}");
        assert_eq!(copies.swap(0, Ordering::SeqCst), at_exact, "{query}");
        let short = run(low - 1).await;
        assert_eq!(codes(&short), ["mutation_payload_failed"], "{query}");
        assert_eq!(
            copies.load(Ordering::SeqCst),
            at_short,
            "{query}: projected objects were copied before the payload was refused"
        );
    }
    f.finish().await;
}

/// A read copies no record it does not have to: the executor borrows the
/// records it read and the read layers move them, so a large stored value is
/// allocated once, served or refused (#704). The one copy left is of a
/// related record that several parents of one batch share, because each
/// parent's result owns its record.
#[tokio::test]
async fn reads_clone_no_root_or_related_record() {
    let f = Fixture::new().await;
    let projector = FileProjector::new(true);
    let api = f.api(f.options(&projector)).unwrap();
    let read = read();
    let clones = Arc::new(AtomicUsize::new(0));
    for (query, shared) in [
        // Root records, to-one and to-many related records, two levels deep.
        // Ada and Bob: no two parents share a related record.
        (
            "{ profiles(first:2, orderBy:[{field:id}]) { edges { node { id name email banner { name } \
            mentor { id name banner { name } mentees(first:2) { edges { node { id name } } } } \
            mentees(first:2) { totalCount edges { cursor node { id name } } } } } } }",
            0,
        ),
        // Bob and Cy share their mentor, Ada: one copy for the second parent.
        (
            "{ profiles(first:3, orderBy:[{field:id}]) { edges { node { id mentor { id name } } } } }",
            1,
        ),
    ] {
        let low = smallest_budget(&api, &f, &read, query).await;
        let _clones = tracing::subscriber::set_default(Events(RECORD_CLONES, clones.clone()));
        for budget in [RuntimeGraphqlLimits::default().max_response_bytes, low - 1] {
            let limits = RuntimeGraphqlLimits {
                max_response_bytes: budget,
                ..Default::default()
            };
            let response = api.execute(f.request(&read, query, limits)).await;
            assert_eq!(response.errors.is_empty(), budget >= low, "{budget}");
            assert_eq!(
                clones.swap(0, Ordering::SeqCst),
                shared,
                "{query}: records cloned at budget {budget}"
            );
        }
    }
    let _clones = tracing::subscriber::set_default(Events(RECORD_CLONES, clones.clone()));
    // The counter sees clones: one is counted here.
    let collection = f
        .schema
        .resolve_collection(&CollectionId::new("profiles").unwrap())
        .unwrap();
    let limits = RuntimeQueryLimits::default();
    let id = f
        .schema
        .resolve_field(&collection, &FieldId::new("id").unwrap())
        .unwrap();
    let request = f
        .schema
        .runtime_read_request(
            &collection,
            &f.schema.resolve_projection(&collection, &[id]).unwrap(),
            None,
            f.schema.runtime_order(&collection, None, limits).unwrap(),
            RuntimePageRequest::first(1, None),
            false,
            limits,
        )
        .unwrap();
    let page = f.db.execute_runtime_read(&request, None).await.unwrap();
    clones.store(0, Ordering::SeqCst);
    let _copy = page.edges[0].node.clone();
    assert_eq!(clones.load(Ordering::SeqCst), 1);
    f.finish().await;
}

/// One event per projection call that copied objects into a response.
const PROJECTION_COPIES: &str = "graphql_orm::runtime_projection";
/// One event per cloned [`RuntimeRecord`].
const RECORD_CLONES: &str = "graphql_orm::runtime_record";

/// Counts the trace events of one target.
struct Events(&'static str, Arc<AtomicUsize>);
impl tracing::Subscriber for Events {
    // Ask per event: other tests' threads register and drop their own
    // dispatchers, so a cached callsite interest could miss this one's events.
    fn register_callsite(
        &self,
        _: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::sometimes()
    }
    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        Some(tracing::level_filters::LevelFilter::TRACE)
    }
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == self.0
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if event.metadata().target() == self.0 {
            self.1.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

#[tokio::test]
async fn projected_runtime_inputs_compile_public_scalar_capabilities() {
    use graphql_orm_ai_tool_profiles::{
        AiGraphqlQueryCapabilityCatalog, AiGraphqlQueryCapabilityLimits, GraphqlExecutionTargetId,
    };
    let f = Fixture::new().await;
    let projector = FileProjector::new(false);
    let module = RuntimeGraphqlModule::compile(f.schema.clone(), f.options(&projector)).unwrap();
    let (api, catalog) = RuntimeGraphqlComposer::new(f.db.clone(), "Query", Default::default())
        .unwrap()
        .mutation_root("Mutation")
        .unwrap()
        .install(module)
        .unwrap()
        .finish_with_semantic_catalog()
        .unwrap();
    let result = AiGraphqlQueryCapabilityCatalog::compile(
        "runtime-projections",
        GraphqlExecutionTargetId::parse("runtime.projections").unwrap(),
        &api.sdl(),
        &catalog,
        AiGraphqlQueryCapabilityLimits::default(),
    )
    .and_then(|capabilities| {
        let capability = capabilities.capabilities().next().unwrap();
        let described = capability.compact_argument_schema().to_string();
        assert!(!described.contains("\"avatar\""), "{described}");
        assert!(!described.contains("\"banner\""), "{described}");
        capability.compile_compact(serde_json::json!({
            "arguments": {"where": {"name": {"eq": "public"}},
                "orderBy": [{"field": "name", "direction": "ASC"}]},
            "maximumItems": 1, "selections": ["name"],
            "relationshipArguments": {}, "relationshipMaximumItems": {}
        }))
    });
    let invalid_sdl = api.sdl().replacen(
        "enum ProfileOrderField {",
        "enum ProfileOrderField {\n HOST_SELECTOR",
        1,
    );
    assert_ne!(invalid_sdl, api.sdl());
    let invalid = AiGraphqlQueryCapabilityCatalog::compile(
        "runtime-projections-invalid",
        GraphqlExecutionTargetId::parse("runtime.projections.invalid").unwrap(),
        &invalid_sdl,
        &catalog,
        AiGraphqlQueryCapabilityLimits::default(),
    );
    f.finish().await;
    result.expect("runtime-emitted projected inputs retain public scalar filtering and ordering");
    assert!(
        invalid.is_err(),
        "runtime order fields require canonical scalar or opaque projected members"
    );
}
