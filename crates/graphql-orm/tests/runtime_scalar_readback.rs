//! Runtime GraphQL accepts a date-time or JSON value only when its canonical
//! form can be stored and read back. Rounding and re-serialization can lengthen
//! a value after its input text was checked, and a stored value that cannot be
//! read breaks every page that contains its row. PostgreSQL cases own a verified
//! disposable server; SQLite uses a private temporary database file.
#![cfg(all(
    feature = "runtime-graphql",
    any(
        all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
        all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
    )
))]
use futures::future::BoxFuture;
use graphql_orm::async_graphql::{Request, Variables, dynamic::Schema};
use graphql_orm::{
    graphql::{orm::*, runtime::*},
    prelude::*,
};
use std::sync::Arc;
#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

const TABLE: &str = "readback_samples";

fn field(name: &str, kind: RuntimeValueKind, nullable: bool, key: bool) -> RuntimeField {
    RuntimeField {
        description: None,
        classification: None,
        export: None,
        id: FieldId::new(format!("sample.{name}")).unwrap(),
        api_name: name.into(),
        physical_column: name.into(),
        value_kind: kind,
        nullable,
        unique: false,
        // A sortable key gives runtime GraphQL its deterministic keyset order.
        filterable: key,
        sortable: key,
        generated: false,
        default: None,
        format: None,
        auto_timestamp: None,
        auto_actor: None,
    }
}
fn definition() -> RuntimeSchema {
    RuntimeSchema {
        repository_only: false,
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![RuntimeCollection {
            node_identity: None,
            live_subscriptions: None,
            description: None,
            classification: None,
            id: CollectionId::new("sample").unwrap(),
            api_type_name: "Sample".into(),
            api_plural_name: "Samples".into(),
            physical_table: TABLE.into(),
            primary_key: vec![FieldId::new("sample.id").unwrap()],
            append_only: false,
            retention_purge: false,
            fields: vec![
                field("id", RuntimeValueKind::Integer, false, true),
                field("at", RuntimeValueKind::DateTime, true, false),
                field("document", RuntimeValueKind::Json, true, false),
            ],
            relations: vec![],
            indexes: vec![],
            composite_unique: vec![],
            default_order: vec![],
            label_fields: Vec::new(),
            search: None,
        }],
    }
}

/// Approves every read and write of this isolated fixture.
struct Open(Arc<ValidatedRuntimeSchema>);
impl RuntimeReadAuthority for Open {
    fn authorize<'a>(
        &'a self,
        _: RuntimeReadCheck<'a>,
    ) -> BoxFuture<'a, Result<RuntimeReadGrant, RuntimeGraphqlError>> {
        Box::pin(async { Ok(RuntimeReadGrant::new(None)) })
    }
    fn authorize_mutation_result<'a>(
        &'a self,
        _: RuntimeMutationOutputCheck<'a>,
    ) -> BoxFuture<'a, Result<(), RuntimeGraphqlError>> {
        Box::pin(async { Ok(()) })
    }
}
impl RuntimeWriteAuthority<Backend> for Open {
    fn authorize_intent<'a>(
        &'a self,
        check: RuntimeWriteIntent<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeWriteGrant, RuntimeMutationError>> {
        Box::pin(async move {
            let collection = check.collection();
            let fields = ["sample.id", "sample.at", "sample.document"]
                .map(|id| {
                    self.0
                        .resolve_field(collection, &FieldId::new(id).unwrap())
                        .unwrap()
                })
                .to_vec();
            RuntimeWriteGrant::new(
                self.0.resolve_projection(collection, &fields).unwrap(),
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
}

struct Fixture {
    db: Database<Backend>,
    environment: Arc<RuntimeMutationEnvironment>,
    api: Schema,
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
        let schema = Arc::new(definition().validate().unwrap());
        let target = schema
            .physical_schema::<Backend>(Default::default())
            .unwrap();
        let ownership = ManagedTableSet::new([TABLE.to_owned()]).unwrap();
        let plan = db
            .schema()
            .plan_owned_migration(
                "init",
                "readback",
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
        let module = RuntimeGraphqlModule::compile(
            schema,
            RuntimeGraphqlOptions::default()
                .with_cursor_profile(RuntimeCursorProfile::Unprotected)
                .with_mutations(Default::default()),
        )
        .unwrap();
        let api = RuntimeGraphqlComposer::new(db.clone(), "Query", Default::default())
            .unwrap()
            .mutation_root("Mutation")
            .unwrap()
            .install(module)
            .unwrap()
            .finish()
            .unwrap();
        Self {
            db,
            environment,
            api,
            #[cfg(feature = "postgres")]
            owned,
            #[cfg(feature = "sqlite")]
            path,
        }
    }
    async fn execute(&self, query: &str, variables: serde_json::Value) -> serde_json::Value {
        let schema = self.environment.schema().clone();
        let response = self
            .api
            .execute(
                Request::new(query)
                    .variables(Variables::from_json(variables))
                    .data(
                        RuntimeGraphqlRequest::<Backend>::new(
                            schema.fingerprint(),
                            Arc::new(Open(schema.clone())),
                            None,
                            Default::default(),
                        )
                        .with_mutation_environment(self.environment.clone())
                        .with_write_authority(Arc::new(Open(schema))),
                    ),
            )
            .await;
        serde_json::json!({
            "errors": response.errors.iter().map(|e| e.message.clone()).collect::<Vec<_>>(),
            "data": response.data.into_json().unwrap(),
        })
    }
    async fn create(&self, input: serde_json::Value) -> serde_json::Value {
        self.execute(
            "mutation($schema: String!, $input: CreateSampleInput!) { \
               createSample(expectedSchema: $schema, input: $input) { record { id at document } } }",
            serde_json::json!({
                "schema": self.environment.schema().fingerprint().to_string(),
                "input": input,
            }),
        )
        .await
    }
    async fn list(&self) -> serde_json::Value {
        self.execute(
            "{ samples { edges { node { id at document } } } }",
            serde_json::json!({}),
        )
        .await
    }
    async fn count(&self) -> i64 {
        let rows = Backend::fetch_rows(
            self.db.pool(),
            &format!("SELECT COUNT(*) AS n FROM {TABLE}"),
            &[],
        )
        .await
        .unwrap();
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

/// `1e15` is re-serialized as `1000000000000000.0`: `count` copies fit the
/// default 64 KiB input bound but not necessarily the same output bound.
fn expanding_document(count: usize) -> String {
    format!("[{}]", vec!["1e15"; count].join(","))
}

#[tokio::test]
async fn values_that_cannot_be_read_back_are_rejected_before_any_write() {
    let f = Fixture::new("scalar-readback-rejected").await;
    for input in [
        // Rounding to microseconds carries into year 10000.
        serde_json::json!({"id": "1", "at": "9999-12-31T23:59:59.9999995Z"}),
        // Offset normalization moves the value into year 0000.
        serde_json::json!({"id": "2", "at": "0001-01-01T00:00:00+01:00"}),
        serde_json::json!({"id": "3", "at": "9999-12-31T23:30:00-01:00"}),
        // PostgreSQL stores this leap second as year 10000.
        serde_json::json!({"id": "5", "at": "9999-12-31T23:59:60Z"}),
        // About 20 KiB of input becomes about 76 KiB of output.
        serde_json::json!({"id": "4", "document": expanding_document(4000)}),
    ] {
        let response = f.create(input.clone()).await;
        assert_eq!(
            response["errors"],
            serde_json::json!(["invalid_graphql_input"]),
            "{input}: {response}"
        );
    }
    assert_eq!(f.count().await, 0);
    let page = f.list().await;
    assert_eq!(page["errors"], serde_json::json!([]), "{page}");
    assert_eq!(page["data"]["samples"]["edges"], serde_json::json!([]));
    f.finish().await;
}

#[tokio::test]
async fn boundary_values_are_stored_and_read_back() {
    let f = Fixture::new("scalar-readback-boundaries").await;
    let document = expanding_document(3000);
    let canonical =
        serde_json::to_string(&serde_json::from_str::<serde_json::Value>(&document).unwrap())
            .unwrap();
    let expected = [
        ("1", "0001-01-01T00:00:00Z", "0001-01-01T00:00:00.000000Z"),
        (
            "2",
            "9999-12-31T23:59:59.9999994Z",
            "9999-12-31T23:59:59.999999Z",
        ),
        (
            "3",
            "9999-12-31T22:59:59.999999-01:00",
            "9999-12-31T23:59:59.999999Z",
        ),
    ];
    for (id, input, _) in expected {
        let response = f.create(serde_json::json!({"id": id, "at": input})).await;
        assert_eq!(response["errors"], serde_json::json!([]), "{response}");
    }
    let response = f
        .create(serde_json::json!({"id": "4", "document": document}))
        .await;
    assert_eq!(response["errors"], serde_json::json!([]), "{response}");
    assert_eq!(
        response["data"]["createSample"]["record"]["document"],
        serde_json::json!(canonical)
    );
    let page = f.list().await;
    assert_eq!(page["errors"], serde_json::json!([]), "{page}");
    let edges = page["data"]["samples"]["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 4, "{page}");
    for (edge, (id, _, at)) in edges.iter().zip(expected) {
        assert_eq!(edge["node"]["id"], serde_json::json!(id));
        assert_eq!(edge["node"]["at"], serde_json::json!(at));
    }
    assert_eq!(edges[3]["node"]["document"], serde_json::json!(canonical));
    f.finish().await;
}

/// Earlier releases could store a date-time outside 0001 to 9999 as SQLite
/// text, including a signed extended year that strict RFC 3339 cannot parse.
/// Such a row no longer fails the whole page: other rows and fields are
/// returned, and only the out-of-range nullable field reports an error,
/// because runtime GraphQL output must also be valid scalar input.
#[cfg(feature = "sqlite")]
#[tokio::test]
async fn previously_stored_out_of_range_years_fail_only_their_field_on_sqlite() {
    let f = Fixture::new("scalar-readback-legacy").await;
    Backend::fetch_rows(
        f.db.pool(),
        &format!(
            "INSERT INTO {TABLE} (id, at, document) VALUES \
             (1, '+10000-01-01T00:00:00.000000Z', '[1]'), \
             (2, '0000-12-31T23:00:00.000000Z', '[2]'), \
             (3, '2026-01-01T00:00:00.000000Z', '[3]')"
        ),
        &[],
    )
    .await
    .unwrap();
    let page = f.list().await;
    assert_eq!(page["errors"].as_array().unwrap().len(), 2, "{page}");
    let edges = page["data"]["samples"]["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 3, "{page}");
    for (edge, (id, document)) in edges.iter().zip([("1", "[1]"), ("2", "[2]"), ("3", "[3]")]) {
        assert_eq!(edge["node"]["id"], serde_json::json!(id));
        assert_eq!(edge["node"]["document"], serde_json::json!(document));
    }
    assert_eq!(edges[0]["node"]["at"], serde_json::Value::Null);
    assert_eq!(edges[1]["node"]["at"], serde_json::Value::Null);
    assert_eq!(
        edges[2]["node"]["at"],
        serde_json::json!("2026-01-01T00:00:00.000000Z")
    );
    f.finish().await;
}
