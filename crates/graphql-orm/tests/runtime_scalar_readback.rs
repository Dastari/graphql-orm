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
/// Every field is NOT NULL, so a field error would null the whole connection.
const STRICT_TABLE: &str = "readback_stricts";

fn field(
    collection: &str,
    name: &str,
    kind: RuntimeValueKind,
    nullable: bool,
    key: bool,
) -> RuntimeField {
    RuntimeField {
        description: None,
        classification: None,
        export: None,
        id: FieldId::new(format!("{collection}.{name}")).unwrap(),
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
fn collection(
    id: &str,
    type_name: &str,
    table: &str,
    fields: Vec<RuntimeField>,
) -> RuntimeCollection {
    RuntimeCollection {
        node_identity: None,
        live_subscriptions: None,
        description: None,
        classification: None,
        id: CollectionId::new(id).unwrap(),
        api_type_name: type_name.into(),
        api_plural_name: format!("{type_name}s"),
        physical_table: table.into(),
        primary_key: vec![FieldId::new(format!("{id}.id")).unwrap()],
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
fn definition() -> RuntimeSchema {
    RuntimeSchema {
        repository_only: false,
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![
            collection(
                "sample",
                "Sample",
                TABLE,
                vec![
                    field("sample", "id", RuntimeValueKind::Integer, false, true),
                    field("sample", "at", RuntimeValueKind::DateTime, true, false),
                    field("sample", "document", RuntimeValueKind::Json, true, false),
                ],
            ),
            collection(
                "strict",
                "Strict",
                STRICT_TABLE,
                vec![
                    field("strict", "id", RuntimeValueKind::Integer, false, true),
                    // Sortable, so a page can be ordered and resumed by it.
                    field("strict", "at", RuntimeValueKind::DateTime, false, true),
                    field("strict", "document", RuntimeValueKind::Json, false, false),
                ],
            ),
        ],
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
        let ownership = ManagedTableSet::new([TABLE.to_owned(), STRICT_TABLE.to_owned()]).unwrap();
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
    /// One page of the NOT NULL collection, ordered by its date-time.
    async fn strict_page(&self, after: Option<&str>) -> serde_json::Value {
        self.execute(
            "query($after: String) { stricts(first: 1, after: $after, \
               orderBy: [{field: at, direction: ASC}, {field: id, direction: ASC}]) { \
               edges { node { id at document } } pageInfo { hasNextPage endCursor } } }",
            serde_json::json!({ "after": after }),
        )
        .await
    }
    async fn sql(&self, sql: &str) {
        Backend::fetch_rows(self.db.pool(), sql, &[]).await.unwrap();
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
/// PostgreSQL `jsonb` returns `1e16` as `10000000000000000`, which serde_json
/// reads back as an integer and prints in full.
fn integral_document(count: usize) -> String {
    format!("[{}]", vec!["1e16"; count].join(","))
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
        // About 20 KiB of input returns from PostgreSQL as about 74 KiB.
        serde_json::json!({"id": "6", "document": integral_document(4095)}),
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
    let response = f
        .create(serde_json::json!({"id": "5", "document": integral_document(3000)}))
        .await;
    assert_eq!(response["errors"], serde_json::json!([]), "{response}");
    let page = f.list().await;
    assert_eq!(page["errors"], serde_json::json!([]), "{page}");
    let edges = page["data"]["samples"]["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 5, "{page}");
    for (edge, (id, _, at)) in edges.iter().zip(expected) {
        assert_eq!(edge["node"]["id"], serde_json::json!(id));
        assert_eq!(edge["node"]["at"], serde_json::json!(at));
    }
    assert_eq!(edges[3]["node"]["document"], serde_json::json!(canonical));
    // SQLite returns `1e+16`; PostgreSQL returns `10000000000000000`. Either
    // text is the same numbers, and either can be sent back as input.
    let read_back = edges[4]["node"]["document"].as_str().unwrap().to_owned();
    let numbers = serde_json::from_str::<Vec<serde_json::Value>>(&read_back).unwrap();
    assert_eq!(numbers.len(), 3000);
    assert!(
        numbers.iter().all(|n| n.as_f64() == Some(1e16)),
        "{read_back:.80}"
    );
    let response = f
        .create(serde_json::json!({"id": "6", "document": read_back}))
        .await;
    assert_eq!(response["errors"], serde_json::json!([]), "{response}");
    f.finish().await;
}

/// Earlier releases, or host code, could store a date-time outside 0001 to
/// 9999 or a JSON document longer than the input bound. Output renders such
/// values as stored, even in NOT NULL fields where a field error would null
/// the whole connection, and a page resumes across them by cursor.
#[tokio::test]
async fn stored_values_beyond_input_bounds_are_returned_and_paged_as_stored() {
    let f = Fixture::new("scalar-readback-stored").await;
    // About 70 KB, beyond the default 64 KiB input bound.
    let long = format!("[\"{}\"]", "x".repeat(70_000));
    #[cfg(feature = "sqlite")]
    let rows = [
        (
            "1",
            "2026-01-01T00:00:00.000000Z",
            "2026-01-01T00:00:00.000000Z",
        ),
        (
            "2",
            "+10000-01-01T00:00:00.000000Z",
            "+10000-01-01T00:00:00.000000Z",
        ),
        (
            "3",
            "0000-12-31T23:00:00.000000Z",
            "0000-12-31T23:00:00.000000Z",
        ),
        (
            "4",
            "9999-12-31T23:59:60.000000Z",
            "9999-12-31T23:59:60.000000Z",
        ),
    ];
    // PostgreSQL refuses year 0000 and stores the final leap second of 9999
    // as year 10000, which its reader returns as an extended year.
    #[cfg(feature = "postgres")]
    let rows = [
        ("1", "2026-01-01 00:00:00+00", "2026-01-01T00:00:00.000000Z"),
        (
            "2",
            "10000-01-01 00:00:00+00",
            "+10000-01-01T00:00:00.000000Z",
        ),
        ("4", "9999-12-31T23:59:60Z", "+10000-01-01T00:00:00.000000Z"),
    ];
    for (id, stored, _) in &rows {
        f.sql(&format!(
            "INSERT INTO {STRICT_TABLE} (id, at, document) VALUES ({id}, '{stored}', '{long}')"
        ))
        .await;
    }
    let page = f
        .execute(
            "{ stricts { edges { node { id at document } } } }",
            serde_json::json!({}),
        )
        .await;
    assert_eq!(
        page["errors"],
        serde_json::json!([]),
        "{:.300}",
        page.to_string()
    );
    let edges = page["data"]["stricts"]["edges"].as_array().unwrap();
    assert_eq!(edges.len(), rows.len());
    for (edge, (id, _, at)) in edges.iter().zip(&rows) {
        assert_eq!(edge["node"]["id"], serde_json::json!(id));
        assert_eq!(edge["node"]["at"], serde_json::json!(at));
        assert_eq!(edge["node"]["document"], serde_json::json!(long));
    }
    // Resume one row at a time; each cursor carries the stored date-time.
    let (mut after, mut seen) = (None::<String>, Vec::new());
    loop {
        let page = f.strict_page(after.as_deref()).await;
        assert_eq!(
            page["errors"],
            serde_json::json!([]),
            "{:.300}",
            page.to_string()
        );
        let connection = &page["data"]["stricts"];
        for edge in connection["edges"].as_array().unwrap() {
            seen.push(edge["node"]["id"].as_str().unwrap().to_owned());
        }
        if connection["pageInfo"]["hasNextPage"] != serde_json::json!(true) {
            break;
        }
        after = Some(
            connection["pageInfo"]["endCursor"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
        assert!(seen.len() <= rows.len(), "{seen:?}");
    }
    seen.sort();
    let mut expected = rows
        .iter()
        .map(|(id, _, _)| id.to_string())
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(seen, expected);
    f.finish().await;
}

/// A deserialized out-of-range date-time can be read and compared, but runtime
/// mutations refuse to write it.
#[test]
fn deserialized_out_of_range_date_times_are_never_written() {
    let schema = definition().validate().unwrap();
    let sample = schema
        .resolve_collection(&CollectionId::new("sample").unwrap())
        .unwrap();
    let field = |name: &str| {
        schema
            .resolve_field(&sample, &FieldId::new(format!("sample.{name}")).unwrap())
            .unwrap()
    };
    for stored in [
        "+10000-01-01T00:00:00.000000Z",
        "0000-12-31T23:00:00.000000Z",
        "9999-12-31T23:59:60.000000Z",
    ] {
        let at: RuntimeDateTime = serde_json::from_str(&format!("\"{stored}\"")).unwrap();
        let values = [
            (field("id"), RuntimeValue::Integer(1)),
            (field("at"), RuntimeValue::DateTime(at.clone())),
        ];
        let error = schema
            .runtime_create_request(&sample, &values, None, Default::default())
            .unwrap_err();
        assert_eq!(
            error.code(),
            RuntimeMutationErrorCode::InvalidInput,
            "{stored}"
        );
        let key = schema
            .runtime_key(&sample, &[(field("id"), RuntimeValue::Integer(1))])
            .unwrap();
        let error = schema
            .runtime_update_request(
                key,
                &[(field("at"), RuntimeValue::DateTime(at))],
                None,
                None,
                Default::default(),
            )
            .unwrap_err();
        assert_eq!(
            error.code(),
            RuntimeMutationErrorCode::InvalidInput,
            "{stored}"
        );
    }
}
