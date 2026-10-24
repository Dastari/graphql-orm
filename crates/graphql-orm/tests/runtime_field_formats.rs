//! Semantic field format descriptors: IR validation, canonical identity,
//! physical stability and descriptor-only revisions without DDL.
//! PostgreSQL cases own a verified disposable server.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use graphql_orm::{graphql::orm::*, prelude::*};
use std::collections::BTreeMap;

#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

fn format(namespace: &str, version: u16) -> RuntimeFieldFormat {
    let payload = RuntimeFormatPayload::new(BTreeMap::from([(
        "accept".to_owned(),
        RuntimeFormatValue::List(vec![RuntimeFormatValue::String("image".into())]),
    )]))
    .unwrap();
    RuntimeFieldFormat::new(
        RuntimeFormatNamespace::new(namespace).unwrap(),
        version,
        payload,
    )
    .unwrap()
}

fn field(id: &str, kind: RuntimeValueKind, nullable: bool) -> RuntimeField {
    RuntimeField {
        description: None,
        classification: None,
        export: None,
        id: FieldId::new(id).unwrap(),
        api_name: id.into(),
        physical_column: id.into(),
        value_kind: kind,
        nullable,
        unique: false,
        filterable: true,
        sortable: false,
        generated: false,
        default: None,
        format: None,
    }
}

fn schema(avatar: Option<RuntimeFieldFormat>) -> RuntimeSchema {
    let mut avatar_field = field("avatar", RuntimeValueKind::Uuid, true);
    avatar_field.format = avatar;
    RuntimeSchema {
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![RuntimeCollection {
            description: None,
            classification: None,
            id: CollectionId::new("profiles").unwrap(),
            api_type_name: "Profile".into(),
            api_plural_name: "Profiles".into(),
            physical_table: "format_profiles".into(),
            primary_key: vec![FieldId::new("id").unwrap()],
            append_only: false,
            retention_purge: false,
            fields: vec![field("id", RuntimeValueKind::Integer, false), avatar_field],
            relations: vec![],
            indexes: vec![],
            composite_unique: vec![],
            default_order: vec![],
            label_fields: Vec::new(),
        }],
    }
}

#[test]
fn absent_descriptors_keep_serialized_and_canonical_bytes() {
    let plain = schema(None).validate().unwrap();
    let json = serde_json::to_string(plain.schema()).unwrap();
    assert!(!json.contains("format\""), "{json}");
    assert!(
        !String::from_utf8(plain.canonical_bytes())
            .unwrap()
            .contains("format=")
    );
    let formatted = schema(Some(format("example.file_ref", 1)))
        .validate()
        .unwrap();
    assert_eq!(
        plain.format_fingerprint(),
        schema(None).validate().unwrap().format_fingerprint()
    );
    assert_ne!(plain.format_fingerprint(), formatted.format_fingerprint());
}

#[test]
fn descriptors_change_semantic_identity_never_structure_or_physical_shape() {
    let plain = schema(None).validate().unwrap();
    let v1 = schema(Some(format("example.file_ref", 1)))
        .validate()
        .unwrap();
    let v2 = schema(Some(format("example.file_ref", 2)))
        .validate()
        .unwrap();
    for changed in [&v1, &v2] {
        assert_ne!(plain.fingerprint(), changed.fingerprint());
        assert_eq!(
            plain.structural_fingerprint(),
            changed.structural_fingerprint()
        );
        assert_eq!(
            plain
                .physical_schema::<Backend>(Default::default())
                .unwrap()
                .stable_hash(),
            changed
                .physical_schema::<Backend>(Default::default())
                .unwrap()
                .stable_hash()
        );
    }
    assert_ne!(v1.fingerprint(), v2.fingerprint());
    let canonical = String::from_utf8(v1.canonical_bytes()).unwrap();
    assert!(
        canonical.contains(r#"|format="example.file_ref@1:{\"accept\":[\"image\"]}""#),
        "{canonical}"
    );
    // Serde round trips preserve the descriptor and the identity.
    let text = serde_json::to_string(v1.schema()).unwrap();
    assert!(text.contains(
        r#""format":{"namespace":"example.file_ref","version":1,"payload":{"accept":["image"]}}"#
    ));
    let reloaded: RuntimeSchema = serde_json::from_str(&text).unwrap();
    assert_eq!(reloaded.validate().unwrap().fingerprint(), v1.fingerprint());
    // Handles expose the descriptor and bind to the descriptor-bearing identity.
    let collection = v1
        .resolve_collection(&CollectionId::new("profiles").unwrap())
        .unwrap();
    let avatar = v1
        .resolve_field(&collection, &FieldId::new("avatar").unwrap())
        .unwrap();
    assert_eq!(avatar.format(), Some(&format("example.file_ref", 1)));
    assert_eq!(avatar.schema_fingerprint(), &v1.fingerprint());
    assert!(plain.resolve_field(&collection, avatar.id()).is_err());
}

#[test]
fn literal_default_diagnostics_are_structured_and_value_free() {
    let mut definition = schema(None);
    let secret = "SECRET-LITERAL-\u{1F512}";
    for (kind, literal, problem) in [
        (
            RuntimeValueKind::Uuid,
            secret,
            RuntimeDefaultProblem::Malformed,
        ),
        (
            RuntimeValueKind::Json,
            "{\"b\":1,\"a\":2}",
            RuntimeDefaultProblem::NonCanonical,
        ),
        (
            RuntimeValueKind::Bytes,
            "AQJ=",
            RuntimeDefaultProblem::NonCanonical,
        ),
        (
            RuntimeValueKind::DateTime,
            "2026-02-30T00:00:00.000000Z",
            RuntimeDefaultProblem::ImpossibleDateTime,
        ),
        (
            RuntimeValueKind::Float,
            "inf",
            RuntimeDefaultProblem::Nonfinite,
        ),
        (
            RuntimeValueKind::Integer,
            "9223372036854775808",
            RuntimeDefaultProblem::OutOfRange,
        ),
    ] {
        let mut bad = field("bad", kind, true);
        bad.default = Some(RuntimeDefault::Literal(literal.into()));
        definition.collections[0].fields.push(bad);
        let diagnostics = definition.clone().validate().unwrap_err();
        let diagnostic = &diagnostics.diagnostics()[0];
        assert_eq!(diagnostic.code, RuntimeSchemaDiagnosticCode::InvalidDefault);
        assert_eq!(
            diagnostic.detail,
            Some(RuntimeSchemaDiagnosticDetail::Default(problem))
        );
        let rendered = format!(
            "{diagnostics} {}",
            serde_json::to_string(&diagnostics).unwrap()
        );
        assert!(!rendered.contains(literal), "{rendered}");
        definition.collections[0].fields.pop();
    }
    let mut stamped = field("bad", RuntimeValueKind::Uuid, true);
    stamped.default = Some(RuntimeDefault::CurrentTimestamp);
    definition.collections[0].fields.push(stamped);
    let diagnostics = definition.validate().unwrap_err();
    assert_eq!(
        diagnostics.diagnostics()[0].detail,
        Some(RuntimeSchemaDiagnosticDetail::Default(
            RuntimeDefaultProblem::KindMismatch
        ))
    );
}

#[test]
fn every_descriptor_bound_is_rejected_on_deserialization() {
    let base = serde_json::to_value(schema(Some(format("example.file_ref", 1)))).unwrap();
    for (patch, expected) in [
        (
            serde_json::json!({"namespace": "BAD", "version": 1}),
            "invalid_namespace",
        ),
        (
            serde_json::json!({"namespace": "graphql_orm.file", "version": 1}),
            "reserved_namespace",
        ),
        (
            serde_json::json!({"namespace": "example.file", "version": 0}),
            "nonzero",
        ),
        (
            serde_json::json!({"namespace": "example.file", "version": 1, "payload": {"a": 0.5}}),
            "float_in_payload",
        ),
        (
            serde_json::json!({"namespace": "example.file", "version": 1, "payload": [1]}),
            "invalid_payload",
        ),
        (
            serde_json::json!({"namespace": "example.file", "version": 1, "extra": true}),
            "unknown field",
        ),
    ] {
        let mut value = base.clone();
        value["collections"][0]["fields"][1]["format"] = patch;
        let error = serde_json::from_value::<RuntimeSchema>(value)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains(expected), "{expected}: {error}");
        assert!(!error.contains("BAD"), "{error}");
    }
}

struct Store {
    #[cfg(feature = "postgres")]
    owned: owned_postgres::OwnedPostgres,
    #[cfg(feature = "sqlite")]
    path: std::path::PathBuf,
}
impl Store {
    fn new() -> Self {
        Self {
            #[cfg(feature = "postgres")]
            owned: owned_postgres::OwnedPostgres::start("field-formats").unwrap(),
            #[cfg(feature = "sqlite")]
            path: std::env::temp_dir().join(format!(
                "graphql-orm-field-formats-{}.db",
                uuid::Uuid::new_v4().simple()
            )),
        }
    }
    async fn open(&self) -> Database<Backend> {
        #[cfg(feature = "sqlite")]
        return Database::<Backend>::connect_sqlite(&format!(
            "sqlite://{}?mode=rwc",
            self.path.display()
        ))
        .await
        .unwrap();
        #[cfg(feature = "postgres")]
        {
            let db = Database::<Backend>::connect_postgres(&self.owned.url)
                .await
                .unwrap();
            let name: String = sqlx::query_scalar("SELECT current_database()")
                .fetch_one(db.pool())
                .await
                .unwrap();
            assert!(self.owned.url.ends_with(&format!("/{name}")));
            db
        }
    }
    fn finish(self) {
        #[cfg(feature = "sqlite")]
        std::fs::remove_file(&self.path).unwrap();
        #[cfg(feature = "postgres")]
        {
            let mut owned = self.owned;
            owned.cleanup().unwrap();
        }
    }
}

async fn plan(
    db: &Database<Backend>,
    schema: &ValidatedRuntimeSchema,
    version: &str,
) -> OwnedPlannedMigration {
    let target = schema
        .physical_schema::<Backend>(Default::default())
        .unwrap();
    let ownership = ManagedTableSet::new(["format_profiles".into()]).unwrap();
    db.schema()
        .plan_owned_migration(
            version,
            "formats",
            &target,
            &ownership,
            PlanOptions::strict(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn descriptor_only_revisions_need_no_ddl_even_after_reopen() {
    let store = Store::new();
    let db = store.open().await;
    let plain = schema(None).validate().unwrap();
    let initial = plan(&db, &plain, "v1").await;
    db.schema()
        .apply_owned_migration(&initial, ApplyOptions::default())
        .await
        .unwrap();
    let formatted = schema(Some(format("example.file_ref", 1)))
        .validate()
        .unwrap();
    let revision = plan(&db, &formatted, "v2").await;
    assert!(revision.steps().is_empty() && revision.statements().is_empty());
    assert_eq!(revision.target_schema_hash(), initial.target_schema_hash());
    db.pool().close().await;

    // A persisted descriptor survives reload; adding a nullable formatted field is
    // an ordinary AddColumn and later descriptor edits remain DDL-free.
    let db = store.open().await;
    let reloaded: RuntimeSchema =
        serde_json::from_str(&serde_json::to_string(formatted.schema()).unwrap()).unwrap();
    let reloaded = reloaded.validate().unwrap();
    assert_eq!(reloaded.fingerprint(), formatted.fingerprint());
    assert!(plan(&db, &reloaded, "v3").await.steps().is_empty());
    let mut grown = reloaded.into_schema();
    let mut banner = field("banner", RuntimeValueKind::Uuid, true);
    banner.filterable = false;
    banner.format = Some(format("example.file_ref", 1));
    grown.collections[0].fields.push(banner);
    let grown = grown.validate().unwrap();
    let add = plan(&db, &grown, "v4").await;
    assert_eq!(add.steps().len(), 1, "{:?}", add.statements());
    db.schema()
        .apply_owned_migration(&add, ApplyOptions::default())
        .await
        .unwrap();
    db.pool().close().await;
    let db = store.open().await;
    let mut edited = grown.schema().clone();
    edited.collections[0].fields[2].format = Some(format("example.file_ref", 2));
    let edited = edited.validate().unwrap();
    assert!(plan(&db, &edited, "v5").await.steps().is_empty());
    let target = edited
        .physical_schema::<Backend>(Default::default())
        .unwrap();
    let environment = db
        .schema()
        .runtime_mutation_environment(
            std::sync::Arc::new(edited.clone()),
            &target,
            &ManagedTableSet::new(["format_profiles".into()]).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(environment.schema().fingerprint(), edited.fingerprint());
    db.pool().close().await;
    store.finish();
}
