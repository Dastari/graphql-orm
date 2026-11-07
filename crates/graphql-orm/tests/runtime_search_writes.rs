//! Runtime collections with search keep their search documents in the same
//! transaction as every create, update and delete.
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
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

fn fid(value: &str) -> FieldId {
    FieldId::new(value).unwrap()
}

fn field(id: &str, kind: RuntimeValueKind, nullable: bool) -> RuntimeField {
    RuntimeField {
        id: fid(id),
        api_name: id.into(),
        physical_column: id.into(),
        value_kind: kind,
        nullable,
        unique: false,
        filterable: true,
        sortable: true,
        generated: false,
        default: None,
        format: None,
        description: None,
        classification: None,
        export: None,
        auto_timestamp: None,
        auto_actor: None,
    }
}

fn definition() -> RuntimeSchema {
    with_search(true, None)
}

fn with_search(enabled: bool, language: Option<&str>) -> RuntimeSchema {
    let entry = |id: &str, weight, json_path: Option<&str>| RuntimeSearchField {
        field: fid(id),
        weight,
        json_path: json_path.map(str::to_string),
    };
    RuntimeSchema {
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![RuntimeCollection {
            node_identity: None,
            live_subscriptions: None,
            id: CollectionId::new("records").unwrap(),
            api_type_name: "Record".into(),
            api_plural_name: "Records".into(),
            physical_table: "search_records".into(),
            primary_key: vec![fid("id")],
            append_only: false,
            retention_purge: false,
            fields: vec![
                field("id", RuntimeValueKind::Integer, false),
                field("title", RuntimeValueKind::String, false),
                field("body", RuntimeValueKind::String, true),
                field("meta", RuntimeValueKind::Json, true),
            ],
            relations: vec![],
            indexes: vec![],
            composite_unique: vec![],
            default_order: vec![],
            description: None,
            classification: None,
            label_fields: vec![],
            search: enabled.then(|| RuntimeSearch {
                fields: vec![
                    entry("title", RuntimeSearchWeight::A, None),
                    entry("meta", RuntimeSearchWeight::B, Some("$.summary")),
                    entry("body", RuntimeSearchWeight::C, None),
                ],
                language: language.map(str::to_string),
                tokenizer: None,
                remove_diacritics: false,
                min_token_len: None,
            }),
        }],
    }
}

struct Allow(RuntimeProjection);

impl RuntimeWriteAuthority<Backend> for Allow {
    fn authorize_intent<'a>(
        &'a self,
        _check: RuntimeWriteIntent<'a>,
        _tx: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeWriteGrant, RuntimeMutationError>> {
        Box::pin(async move { RuntimeWriteGrant::new(self.0.clone(), None) })
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

struct Harness {
    db: Database<Backend>,
    environment: Arc<RuntimeMutationEnvironment>,
    /// Keeps the owned container alive for the harness.
    #[cfg(feature = "postgres")]
    #[allow(dead_code)]
    owned: owned_postgres::OwnedPostgres,
}

impl Harness {
    async fn new() -> Self {
        Self::with_schema(definition()).await
    }

    async fn with_schema(definition: RuntimeSchema) -> Self {
        #[cfg(feature = "sqlite")]
        let db = Database::<Backend>::connect_sqlite("sqlite::memory:")
            .await
            .unwrap();
        #[cfg(feature = "postgres")]
        let owned = owned_postgres::OwnedPostgres::start("runtime-search-writes").unwrap();
        #[cfg(feature = "postgres")]
        let db = Database::<Backend>::connect_postgres(&owned.url)
            .await
            .unwrap();
        let schema = Arc::new(definition.validate().unwrap());
        let target = schema
            .physical_schema::<Backend>(Default::default())
            .unwrap();
        let ownership = ManagedTableSet::new(["search_records".into()]).unwrap();
        let plan = db
            .schema()
            .plan_owned_migration(
                "init",
                "runtime search",
                &target,
                &ownership,
                PlanOptions::strict(),
            )
            .await
            .unwrap();
        db.schema()
            .apply_runtime_migration(&plan, &schema, ApplyOptions::default(), Default::default())
            .await
            .unwrap();
        let environment = Arc::new(
            db.schema()
                .runtime_mutation_environment(schema, &target, &ownership)
                .await
                .unwrap(),
        );
        Self {
            db,
            environment,
            #[cfg(feature = "postgres")]
            owned,
        }
    }

    fn schema(&self) -> &ValidatedRuntimeSchema {
        self.environment.schema()
    }

    fn collection(&self) -> RuntimeCollectionHandle {
        self.schema()
            .resolve_collection(&CollectionId::new("records").unwrap())
            .unwrap()
    }

    fn field(&self, name: &str) -> RuntimeFieldHandle {
        self.schema()
            .resolve_field(&self.collection(), &fid(name))
            .unwrap()
    }

    fn projection(&self) -> RuntimeProjection {
        self.schema()
            .resolve_projection(
                &self.collection(),
                &["id", "title", "body", "meta"].map(|name| self.field(name)),
            )
            .unwrap()
    }

    fn key(&self, id: i64) -> RuntimeKey {
        self.schema()
            .runtime_key(
                &self.collection(),
                &[(self.field("id"), RuntimeValue::Integer(id))],
            )
            .unwrap()
    }

    async fn run(
        &self,
        request: RuntimeMutationRequest,
    ) -> Result<RuntimeMutationEffect, TransactionError> {
        let environment = self.environment.clone();
        let authority = Allow(self.projection());
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

    async fn create(
        &self,
        id: i64,
        title: &str,
        body: &str,
        summary: &str,
    ) -> Result<RuntimeMutationEffect, TransactionError> {
        let request = self
            .schema()
            .runtime_create_request(
                &self.collection(),
                &[
                    (self.field("id"), RuntimeValue::Integer(id)),
                    (self.field("title"), RuntimeValue::String(title.into())),
                    (self.field("body"), RuntimeValue::String(body.into())),
                    (
                        self.field("meta"),
                        RuntimeValue::Json(
                            serde_json::json!({"summary": summary, "other": "ignored"}),
                        ),
                    ),
                ],
                Some(self.projection()),
                Default::default(),
            )
            .unwrap();
        self.run(request).await
    }

    /// The stored document text per weight, or none when the row has no document.
    async fn document(&self, id: i64) -> Option<(String, String, String)> {
        #[cfg(feature = "sqlite")]
        let sql = "SELECT weight_a AS a, weight_b AS b, weight_c AS c FROM __graphql_orm_fts_search_records WHERE entity_pk = ?";
        #[cfg(feature = "postgres")]
        let sql = "SELECT document_text AS a, '' AS b, '' AS c FROM __graphql_orm_search_search_records WHERE entity_pk = $1";
        let rows = Backend::fetch_rows(self.db.pool(), sql, &[SqlValue::String(id.to_string())])
            .await
            .unwrap();
        rows.first().map(|row| {
            (
                Backend::try_get_string(row, "a").unwrap(),
                Backend::try_get_string(row, "b").unwrap(),
                Backend::try_get_string(row, "c").unwrap(),
            )
        })
    }

    async fn count(&self) -> i64 {
        let rows = Backend::fetch_rows(
            self.db.pool(),
            "SELECT COUNT(*) AS n FROM search_records",
            &[],
        )
        .await
        .unwrap();
        Backend::try_get_i64(&rows[0], "n").unwrap()
    }
}

#[tokio::test]
async fn create_update_and_delete_keep_the_search_document_in_step() {
    let harness = Harness::new().await;
    harness
        .create(1, "Harbour Bridge", "steel arch", "Sydney landmark")
        .await
        .unwrap();
    let (a, b, c) = harness.document(1).await.expect("created document");
    #[cfg(feature = "sqlite")]
    {
        assert_eq!(a, "Harbour Bridge");
        assert_eq!(b, "Sydney landmark");
        assert_eq!(c, "steel arch");
    }
    #[cfg(feature = "postgres")]
    {
        let _ = (b, c);
        assert!(
            a.contains("Harbour Bridge") && a.contains("Sydney landmark") && !a.contains("ignored"),
            "{a}"
        );
    }

    let update = harness
        .schema()
        .runtime_update_request(
            harness.key(1),
            &[(
                harness.field("title"),
                RuntimeValue::String("Opera House".into()),
            )],
            None,
            Some(harness.projection()),
            Default::default(),
        )
        .unwrap();
    harness.run(update).await.unwrap();
    let (a, ..) = harness.document(1).await.expect("updated document");
    assert!(a.contains("Opera House") && !a.contains("Harbour"), "{a}");

    let delete = harness
        .schema()
        .runtime_delete_request(
            harness.key(1),
            None,
            Some(harness.projection()),
            Default::default(),
        )
        .unwrap();
    harness.run(delete).await.unwrap();
    assert!(harness.document(1).await.is_none());
    assert_eq!(harness.count().await, 0);
}

#[tokio::test]
async fn a_failed_search_document_write_rolls_back_the_row() {
    let harness = Harness::new().await;
    #[cfg(feature = "sqlite")]
    let drop = "DROP TABLE __graphql_orm_fts_search_records";
    #[cfg(feature = "postgres")]
    let drop = "DROP TABLE __graphql_orm_search_search_records";
    <Backend as WriteBackend>::execute_with_binds(harness.db.pool(), drop, &[])
        .await
        .unwrap();
    assert!(
        harness
            .create(1, "Harbour Bridge", "steel arch", "Sydney landmark")
            .await
            .is_err()
    );
    assert_eq!(
        harness.count().await,
        0,
        "the row write rolled back with its document"
    );
}

impl Harness {
    /// Migrate to `definition`, replacing the mutation environment.
    async fn migrate(&mut self, definition: RuntimeSchema, search: RuntimeSearchApplyOptions) {
        self.migrate_as("search", definition, search).await;
    }

    /// Migrate to `definition` under migration `version`.
    async fn migrate_as(
        &mut self,
        version: &str,
        definition: RuntimeSchema,
        search: RuntimeSearchApplyOptions,
    ) {
        let schema = Arc::new(definition.validate().unwrap());
        let target = schema
            .physical_schema::<Backend>(Default::default())
            .unwrap();
        let ownership = ManagedTableSet::new(["search_records".into()]).unwrap();
        let plan = self
            .db
            .schema()
            .plan_owned_migration(
                version,
                "add search",
                &target,
                &ownership,
                PlanOptions::strict(),
            )
            .await
            .unwrap();
        self.db
            .schema()
            .apply_runtime_migration(&plan, &schema, ApplyOptions::default(), search)
            .await
            .unwrap();
        self.environment = Arc::new(
            self.db
                .schema()
                .runtime_mutation_environment(schema, &target, &ownership)
                .await
                .unwrap(),
        );
    }

    fn records(&self) -> CollectionId {
        CollectionId::new("records").unwrap()
    }

    async fn ready(&self) -> bool {
        self.db
            .schema()
            .runtime_search_ready(self.schema(), &self.records())
            .await
            .unwrap()
    }

    async fn backfill(
        &self,
        options: RuntimeSearchBackfillOptions,
    ) -> RuntimeSearchBackfillProgress {
        self.db
            .schema()
            .backfill_runtime_search(self.schema(), &self.records(), options)
            .await
            .unwrap()
    }

    async fn retitle(&self, id: i64, title: &str) {
        self.try_retitle(id, title).await.unwrap();
    }

    async fn try_retitle(&self, id: i64, title: &str) -> Result<(), TransactionError> {
        let update = self
            .schema()
            .runtime_update_request(
                self.key(id),
                &[(self.field("title"), RuntimeValue::String(title.into()))],
                None,
                Some(self.projection()),
                Default::default(),
            )
            .unwrap();
        self.run(update).await.map(drop)
    }

    async fn try_backfill(
        &self,
        options: RuntimeSearchBackfillOptions,
    ) -> Result<RuntimeSearchBackfillProgress, RuntimeMigrationError> {
        self.db
            .schema()
            .backfill_runtime_search(self.schema(), &self.records(), options)
            .await
    }

    /// The index's metadata, including its readiness and resume point.
    async fn metadata(&self) -> serde_json::Value {
        #[cfg(feature = "sqlite")]
        let sql = "SELECT config_json AS c FROM __graphql_orm_search_metadata WHERE table_name = ?";
        #[cfg(feature = "postgres")]
        let sql = "SELECT config_json::text AS c FROM __graphql_orm_search_metadata WHERE table_name = $1";
        let rows = Backend::fetch_rows(
            self.db.pool(),
            sql,
            &[SqlValue::String("search_records".into())],
        )
        .await
        .unwrap();
        serde_json::from_str(&Backend::try_get_string(&rows[0], "c").unwrap()).unwrap()
    }

    async fn execute(&self, sql: &str) {
        <Backend as WriteBackend>::execute_with_binds(self.db.pool(), sql, &[])
            .await
            .unwrap();
    }
}

/// A batch that fails partway through rolls back as a whole: its earlier
/// documents are not kept, the resume point does not advance and the index
/// stays `building`. Resuming after the fault indexes the rest.
#[tokio::test]
async fn a_failure_inside_a_batch_rolls_it_back_and_keeps_the_resume_point() {
    let mut harness = Harness::with_schema(with_search(false, None)).await;
    for id in 1..=5 {
        harness
            .create(id, &format!("title {id}"), "body", "summary")
            .await
            .unwrap();
    }
    harness
        .migrate(definition(), RuntimeSearchApplyOptions::deferred())
        .await;
    // Fail the document write of row 4, the second row of the second batch.
    #[cfg(feature = "sqlite")]
    harness
        .execute(
            "CREATE TRIGGER gom_test_fault BEFORE INSERT ON __graphql_orm_fts_search_records_content \
             WHEN NEW.c0 = '4' BEGIN SELECT RAISE(ABORT, 'injected fault'); END",
        )
        .await;
    #[cfg(feature = "postgres")]
    {
        harness
            .execute(
                "CREATE FUNCTION gom_test_fault() RETURNS trigger LANGUAGE plpgsql AS $$ \
                 BEGIN IF NEW.entity_pk = '4' THEN RAISE EXCEPTION 'injected fault'; END IF; \
                 RETURN NEW; END $$",
            )
            .await;
        harness
            .execute(
                "CREATE TRIGGER gom_test_fault BEFORE INSERT ON __graphql_orm_search_search_records \
                 FOR EACH ROW EXECUTE FUNCTION gom_test_fault()",
            )
            .await;
    }

    let options = RuntimeSearchBackfillOptions::default().with_batch_size(2);
    assert!(harness.try_backfill(options).await.is_err());
    assert!(harness.document(1).await.is_some() && harness.document(2).await.is_some());
    assert!(
        harness.document(3).await.is_none(),
        "row 3 was written in the failed batch and rolled back with it"
    );
    let metadata = harness.metadata().await;
    assert_eq!(metadata["readiness"], "building");
    assert_eq!(metadata["backfill_after"], "2", "{metadata}");
    assert!(!harness.ready().await);

    #[cfg(feature = "sqlite")]
    harness.execute("DROP TRIGGER gom_test_fault").await;
    #[cfg(feature = "postgres")]
    harness
        .execute("DROP TRIGGER gom_test_fault ON __graphql_orm_search_search_records")
        .await;
    let resumed = harness.backfill(options).await;
    assert_eq!((resumed.processed, resumed.ready), (3, true));
    for id in 1..=5 {
        assert!(harness.document(id).await.is_some(), "{id}");
    }
    assert!(harness.metadata().await.get("backfill_after").is_none());
}

/// Wait until some session of this database matches `condition` in
/// `pg_stat_activity`.
#[cfg(feature = "postgres")]
async fn wait_for_session(harness: &Harness, condition: &str) {
    let sql = format!(
        "SELECT COUNT(*) AS n FROM pg_stat_activity WHERE datname = current_database() AND {condition}"
    );
    for _ in 0..500 {
        let rows = Backend::fetch_rows(harness.db.pool(), &sql, &[])
            .await
            .unwrap();
        if Backend::try_get_i64(&rows[0], "n").unwrap() > 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("no session matched {condition}");
}

/// A writer that runs while a batch holds its rows waits for the batch instead
/// of racing it. Runtime writes are serializable, so the waiting writer then
/// fails as retryable with nothing written, and the host's retry writes the
/// row and its document. No document is ever left stale.
#[cfg(feature = "postgres")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_writer_concurrent_with_a_running_batch_waits_and_leaves_no_stale_document() {
    let mut harness = Harness::with_schema(with_search(false, None)).await;
    for id in 1..=3 {
        harness
            .create(id, &format!("title {id}"), "body", "summary")
            .await
            .unwrap();
    }
    harness
        .migrate(definition(), RuntimeSearchApplyOptions::deferred())
        .await;
    // Hold the batch open after it has read and locked rows 1 to 3: it pauses
    // while writing the first document.
    harness
        .execute(
            "CREATE FUNCTION gom_test_pause() RETURNS trigger LANGUAGE plpgsql AS $$ \
             BEGIN IF NEW.entity_pk = '1' THEN PERFORM pg_sleep(2); END IF; \
             RETURN NEW; END $$",
        )
        .await;
    harness
        .execute(
            "CREATE TRIGGER gom_test_pause BEFORE INSERT ON __graphql_orm_search_search_records \
             FOR EACH ROW EXECUTE FUNCTION gom_test_pause()",
        )
        .await;

    let backfill = harness.try_backfill(RuntimeSearchBackfillOptions::default());
    let writer = async {
        // Start only once the batch is running, then require that the writer
        // really waits on the batch's row lock.
        wait_for_session(&harness, "wait_event = 'PgSleep'").await;
        let (written, ()) = tokio::join!(
            harness.try_retitle(2, "renamed two"),
            wait_for_session(&harness, "wait_event_type = 'Lock'"),
        );
        written
    };
    let (progress, written) = tokio::join!(backfill, writer);
    let progress = progress.unwrap();
    assert!(progress.ready, "{progress:?}");
    assert!(
        matches!(written, Err(TransactionError::Retryable(_))),
        "{written:?}"
    );
    // The batch committed the documents of the rows it read; the failed writer
    // left both its row and its document unchanged.
    let titles = |renamed: &'static str| [(1, "title 1"), (2, renamed), (3, "title 3")];
    for (id, title) in titles("title 2") {
        let (text, ..) = harness.document(id).await.expect("document");
        assert!(text.contains(title), "{id}: {text}");
    }

    harness.try_retitle(2, "renamed two").await.unwrap();
    for (id, title) in titles("renamed two") {
        let (text, ..) = harness.document(id).await.expect("document");
        assert!(text.contains(title), "{id}: {text}");
    }
}

/// A collection that gains search keeps its rows; a deferred backfill stopped
/// partway stays `building`, and resuming it indexes the rest and marks it ready.
#[tokio::test]
async fn a_resumed_backfill_completes_and_writes_meanwhile_stay_consistent() {
    let mut harness = Harness::with_schema(with_search(false, None)).await;
    for id in 1..=5 {
        harness
            .create(id, &format!("title {id}"), "body", "summary")
            .await
            .unwrap();
    }
    harness
        .migrate(definition(), RuntimeSearchApplyOptions::deferred())
        .await;
    assert!(
        !harness.ready().await,
        "a deferred backfill leaves the index building"
    );
    assert!(harness.document(1).await.is_none());

    // One bounded batch, as a host job interrupted after its first batch.
    let options = RuntimeSearchBackfillOptions::default().with_batch_size(2);
    let partial = harness.backfill(options.with_max_batches(1)).await;
    assert_eq!(
        (partial.processed, partial.remaining, partial.ready),
        (2, 3, false)
    );
    assert!(!harness.ready().await);
    assert!(harness.document(2).await.is_some() && harness.document(3).await.is_none());

    // Writes during the backfill maintain their own documents, on both sides of
    // the resume point.
    harness.retitle(1, "renamed one").await;
    harness.retitle(4, "renamed four").await;
    harness
        .create(6, "title 6", "body", "summary")
        .await
        .unwrap();

    let finished = harness.backfill(options).await;
    assert!(finished.ready && finished.remaining == 0, "{finished:?}");
    assert_eq!(finished.processed, 4);
    assert!(harness.ready().await);
    for (id, title) in [
        (1, "renamed one"),
        (2, "title 2"),
        (3, "title 3"),
        (4, "renamed four"),
        (5, "title 5"),
        (6, "title 6"),
    ] {
        let (text, ..) = harness.document(id).await.expect("document");
        assert!(text.contains(title), "{id}: {text}");
    }
    // A finished backfill is a no-op.
    let again = harness.backfill(options).await;
    assert_eq!((again.processed, again.ready), (0, true));
}

#[tokio::test]
async fn the_apply_time_backfill_indexes_existing_rows_before_apply_returns() {
    let mut harness = Harness::with_schema(with_search(false, None)).await;
    for id in 1..=3 {
        harness
            .create(id, &format!("title {id}"), "body", "summary")
            .await
            .unwrap();
    }
    harness
        .migrate(
            definition(),
            RuntimeSearchApplyOptions::default()
                .with_backfill(RuntimeSearchBackfillOptions::default().with_batch_size(2)),
        )
        .await;
    assert!(harness.ready().await);
    for id in 1..=3 {
        assert!(harness.document(id).await.is_some(), "{id}");
    }
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn an_unknown_postgresql_text_search_configuration_fails_at_plan_time() {
    let harness = Harness::with_schema(with_search(false, None)).await;
    let schema = Arc::new(
        with_search(true, Some("no_such_config"))
            .validate()
            .unwrap(),
    );
    let target = schema
        .physical_schema::<Backend>(Default::default())
        .unwrap();
    let ownership = ManagedTableSet::new(["search_records".into()]).unwrap();
    let error = harness
        .db
        .schema()
        .plan_owned_migration(
            "search",
            "add search",
            &target,
            &ownership,
            PlanOptions::strict(),
        )
        .await
        .unwrap_err();
    let RuntimeMigrationError::Diagnostics(diagnostics) = error else {
        panic!("expected diagnostics, got {error:?}");
    };
    assert_eq!(
        diagnostics.0[0].code,
        RuntimeMigrationDiagnosticCode::UnknownSearchLanguage
    );
    assert_eq!(diagnostics.0[0].subject.as_deref(), Some("no_such_config"));
}

impl Harness {
    async fn search(
        &self,
        query: &str,
        rank: bool,
        min_score: Option<f64>,
        predicate: Option<RuntimePredicate>,
        page: RuntimePageRequest,
    ) -> Result<RuntimeConnection, RuntimeQueryError> {
        let schema = self.schema();
        let collection = self.collection();
        let order = schema
            .runtime_order(&collection, None, Default::default())
            .unwrap();
        let search = schema.runtime_search_request(
            &collection,
            graphql_orm::graphql::filters::SearchInput {
                query: query.into(),
                mode: None,
                min_score,
            },
            rank,
            RuntimeSearchLimits::default(),
        )?;
        let request = schema
            .runtime_read_request(
                &collection,
                &self.projection(),
                predicate,
                order,
                page,
                true,
                Default::default(),
            )
            .unwrap()
            .with_search(search)
            .unwrap();
        self.db.execute_runtime_read(&request, None).await
    }

    fn ids(&self, connection: &RuntimeConnection) -> Vec<i64> {
        let id = self.field("id");
        connection
            .edges
            .iter()
            .map(|edge| match edge.node.value(&id) {
                Ok(RuntimeValue::Integer(id)) => *id,
                other => panic!("unexpected id {other:?}"),
            })
            .collect()
    }
}

#[tokio::test]
async fn search_ranks_by_weight_filters_before_ranking_and_binds_its_cursors() {
    let harness = Harness::new().await;
    // "bridge" appears in the title (A), in the JSON summary (B) or in the body (C).
    harness
        .create(1, "steel tower", "a bridge nearby", "harbour")
        .await
        .unwrap();
    harness
        .create(2, "Harbour Bridge", "steel arch", "landmark")
        .await
        .unwrap();
    harness
        .create(3, "ferry wharf", "busy", "next to the bridge")
        .await
        .unwrap();
    harness
        .create(4, "museum", "art", "galleries")
        .await
        .unwrap();

    let ranked = harness
        .search(
            "bridge",
            true,
            None,
            None,
            RuntimePageRequest::first(10, None),
        )
        .await
        .unwrap();
    assert_eq!(harness.ids(&ranked), [2, 3, 1], "A ranks above B above C");
    assert_eq!(ranked.total_count, Some(3));
    assert!(ranked.edges.iter().all(|edge| edge.score.is_some()));
    let scores = ranked
        .edges
        .iter()
        .map(|edge| edge.score.unwrap().0)
        .collect::<Vec<_>>();
    assert!(
        scores.windows(2).all(|pair| pair[0] >= pair[1]),
        "{scores:?}"
    );

    // A filter applies before ranking and counting.
    let not_two = harness
        .schema()
        .runtime_compare(
            &harness.collection(),
            &harness.field("id"),
            RuntimeScalarOperator::Ne,
            RuntimeValue::Integer(2),
            Default::default(),
        )
        .unwrap();
    let filtered = harness
        .search(
            "bridge",
            true,
            None,
            Some(not_two),
            RuntimePageRequest::first(10, None),
        )
        .await
        .unwrap();
    assert_eq!(harness.ids(&filtered), [3, 1]);
    assert_eq!(filtered.total_count, Some(2));

    // Relevance pages with a stable cursor.
    let first = harness
        .search(
            "bridge",
            true,
            None,
            None,
            RuntimePageRequest::first(2, None),
        )
        .await
        .unwrap();
    assert_eq!(harness.ids(&first), [2, 3]);
    assert!(first.page_info.has_next_page);
    let after = first.page_info.end_cursor.clone();
    let second = harness
        .search(
            "bridge",
            true,
            None,
            None,
            RuntimePageRequest::first(2, after.clone()),
        )
        .await
        .unwrap();
    assert_eq!(harness.ids(&second), [1]);
    // A cursor built for one search is refused for another.
    let other = harness
        .search(
            "steel",
            true,
            None,
            None,
            RuntimePageRequest::first(2, after),
        )
        .await
        .unwrap_err();
    assert_eq!(other.code(), RuntimeQueryErrorCode::CursorInvalid);

    // Without ranking, results follow the read's own order (the primary key).
    let unranked = harness
        .search(
            "bridge",
            false,
            None,
            None,
            RuntimePageRequest::first(10, None),
        )
        .await
        .unwrap();
    assert_eq!(harness.ids(&unranked), [1, 2, 3]);

    // minScore drops weaker matches, and counts follow.
    let top = ranked.edges[0].score.unwrap().0;
    let strongest = harness
        .search(
            "bridge",
            true,
            Some(top),
            None,
            RuntimePageRequest::first(10, None),
        )
        .await
        .unwrap();
    assert_eq!(harness.ids(&strongest), [2]);
    assert_eq!(strongest.total_count, Some(1));

    // Bounds and empty inputs are refused before any query.
    let long = "word ".repeat(40);
    assert_eq!(
        harness
            .search(&long, true, None, None, RuntimePageRequest::first(1, None))
            .await
            .unwrap_err()
            .code(),
        RuntimeQueryErrorCode::ResourceLimit
    );
    assert_eq!(
        harness
            .search("  ", true, None, None, RuntimePageRequest::first(1, None))
            .await
            .unwrap_err()
            .code(),
        RuntimeQueryErrorCode::InvalidSearch
    );
}

#[tokio::test]
async fn remove_diacritics_matches_accented_and_plain_forms() {
    let mut definition = definition();
    definition.collections[0]
        .search
        .as_mut()
        .unwrap()
        .remove_diacritics = true;
    let harness = Harness::with_schema(definition).await;
    harness
        .create(1, "Te Reo Māori", "language", "Aotearoa")
        .await
        .unwrap();
    harness
        .create(2, "Maori carving", "art", "whakairo")
        .await
        .unwrap();
    harness
        .create(3, "Café Society", "history", "Wellington")
        .await
        .unwrap();
    for (query, expected) in [
        ("maori", vec![1, 2]),
        ("Māori", vec![1, 2]),
        ("cafe", vec![3]),
        ("café", vec![3]),
    ] {
        let found = harness
            .search(
                query,
                false,
                None,
                None,
                RuntimePageRequest::first(10, None),
            )
            .await
            .unwrap();
        assert_eq!(harness.ids(&found), expected, "{query}");
    }
}

impl Harness {
    /// A relevance-ordered search under a host grant that hides every row with
    /// `id >= 1000`, as `(ids, scores, totalCount)`.
    async fn granted(&self, query: &str, min_score: Option<f64>) -> (Vec<i64>, Vec<f64>, i64) {
        let grant = self
            .schema()
            .runtime_compare(
                &self.collection(),
                &self.field("id"),
                RuntimeScalarOperator::Lt,
                RuntimeValue::Integer(HIDDEN_FROM),
                Default::default(),
            )
            .unwrap();
        let found = self
            .search(
                query,
                true,
                min_score,
                Some(grant),
                RuntimePageRequest::first(50, None),
            )
            .await
            .unwrap();
        let scores = found
            .edges
            .iter()
            .map(|edge| edge.score.expect("ranked edges carry a score").0)
            .collect();
        (
            self.ids(&found),
            scores,
            found.total_count.expect("count requested"),
        )
    }

    /// The number of stored search documents.
    async fn document_count(&self) -> i64 {
        #[cfg(feature = "sqlite")]
        let sql = "SELECT COUNT(*) AS n FROM __graphql_orm_fts_search_records";
        #[cfg(feature = "postgres")]
        let sql = "SELECT COUNT(*) AS n FROM __graphql_orm_search_search_records";
        let rows = Backend::fetch_rows(self.db.pool(), sql, &[]).await.unwrap();
        Backend::try_get_i64(&rows[0], "n").unwrap()
    }
}

/// Rows with this key or above are hidden by the grant in [`Harness::granted`].
const HIDDEN_FROM: i64 = 1000;

/// The weighted ranking fixture: "bridge" once in the title (A), the JSON
/// summary (B) or the body (C) of a visible row, plus a row without it.
async fn weighted_fixture(harness: &Harness) {
    for (id, title, body, summary) in [
        (1, "steel tower", "a bridge nearby", "harbour"),
        (2, "Harbour Bridge", "steel arch", "landmark"),
        (3, "ferry wharf", "busy", "next to the bridge"),
        (4, "museum", "art", "galleries"),
    ] {
        harness.create(id, title, body, summary).await.unwrap();
    }
}

/// A visible row's score, the order of visible rows, `minScore` results and
/// `totalCount` are the same whether 0, 1 or 1,000 rows hidden by the grant
/// contain the query term. Each backend scores a row from its own document
/// only (SQLite weighted match counts, PostgreSQL `ts_rank_cd`), so hidden
/// rows cannot be inferred from visible scores or order.
#[tokio::test]
async fn hidden_rows_do_not_change_visible_scores_order_or_min_score() {
    let harness = Harness::new().await;
    weighted_fixture(&harness).await;

    let (ids, scores, total) = harness.granted("bridge", None).await;
    assert_eq!(ids, [2, 3, 1], "A ranks above B above C");
    assert_eq!(total, 3);
    assert!(
        scores.windows(2).all(|pair| pair[0] > pair[1]),
        "{scores:?}"
    );
    // A threshold between the B and C scores keeps the A and B rows.
    let threshold = (scores[1] + scores[2]) / 2.0;
    let baseline = (
        harness.granted("bridge", None).await,
        harness.granted("bridge", Some(threshold)).await,
        harness.granted("harbour bridge", None).await,
    );
    assert_eq!((baseline.1).0, [2, 3]);
    assert_eq!((baseline.1).2, 2);

    // One hidden row, then 1,000 of them, matching the term in every weight and
    // with varied lengths and frequencies, so any corpus statistic would move.
    let mut hidden = HIDDEN_FROM;
    for count in [1, 999] {
        for _ in 0..count {
            let repeat = 1 + usize::try_from(hidden % 7).unwrap();
            harness
                .create(
                    hidden,
                    &"bridge ".repeat(repeat),
                    &format!("harbour {}", "bridge span ".repeat(repeat * 3)),
                    "bridge",
                )
                .await
                .unwrap();
            hidden += 1;
        }
        let with_hidden = (
            harness.granted("bridge", None).await,
            harness.granted("bridge", Some(threshold)).await,
            harness.granted("harbour bridge", None).await,
        );
        assert_eq!(
            with_hidden,
            baseline,
            "{} hidden rows",
            hidden - HIDDEN_FROM
        );
    }
    assert_eq!(hidden - HIDDEN_FROM, 1000);
    // Removing hidden rows and their documents changes nothing either.
    harness
        .execute("DELETE FROM search_records WHERE id >= 1500")
        .await;
    #[cfg(feature = "sqlite")]
    harness
        .execute(
            "DELETE FROM __graphql_orm_fts_search_records WHERE CAST(entity_pk AS INTEGER) >= 1500",
        )
        .await;
    #[cfg(feature = "postgres")]
    harness
        .execute("DELETE FROM __graphql_orm_search_search_records WHERE CAST(entity_pk AS BIGINT) >= 1500")
        .await;
    assert_eq!(
        (
            harness.granted("bridge", None).await,
            harness.granted("bridge", Some(threshold)).await,
            harness.granted("harbour bridge", None).await,
        ),
        baseline
    );
}

/// The weighted fixture keeps its order on both backends, including for a
/// multi-term query and a row matching in two weights.
#[tokio::test]
async fn the_weighted_ranking_fixture_keeps_its_order() {
    let harness = Harness::new().await;
    weighted_fixture(&harness).await;
    // Title (A) and body (C) both match.
    harness
        .create(5, "Bridge Street", "the bridge", "shops")
        .await
        .unwrap();
    let (ids, scores, _) = harness.granted("bridge", None).await;
    assert_eq!(ids, [5, 2, 3, 1], "{scores:?}");
    let (ids, scores, _) = harness.granted("harbour bridge", None).await;
    // Both terms in the title rank above both terms in B and C.
    assert_eq!(ids, [2, 1], "{scores:?}");
}

fn stale_codes(error: RuntimeMigrationError) -> Vec<(RuntimeMigrationDiagnosticCode, String)> {
    let RuntimeMigrationError::Diagnostics(diagnostics) = error else {
        panic!("expected diagnostics, got {error:?}");
    };
    diagnostics
        .0
        .into_iter()
        .map(|diagnostic| {
            assert_eq!(
                diagnostic.collection.as_ref().map(CollectionId::as_str),
                Some("records")
            );
            (diagnostic.code, diagnostic.subject.unwrap_or_default())
        })
        .collect()
}

/// A backfill given a schema whose search declaration differs from the stored
/// index is refused and writes nothing; the current schema still completes and
/// resumes.
#[tokio::test]
async fn a_backfill_with_a_stale_schema_is_refused_and_writes_nothing() {
    let mut harness = Harness::with_schema(with_search(false, None)).await;
    for id in 1..=5 {
        harness
            .create(id, &format!("title {id}"), "body text", "summary")
            .await
            .unwrap();
    }
    let stale = definition().validate().unwrap();
    harness
        .migrate(definition(), RuntimeSearchApplyOptions::deferred())
        .await;
    let options = RuntimeSearchBackfillOptions::default().with_batch_size(2);
    harness.backfill(options.with_max_batches(1)).await;

    // The index is dropped and recreated for another field set while the old
    // declaration is still held by a host job.
    let mut changed = definition();
    changed.collections[0]
        .search
        .as_mut()
        .unwrap()
        .fields
        .remove(2);
    harness
        .migrate_as("recreate", changed, RuntimeSearchApplyOptions::deferred())
        .await;
    let before = (
        harness.metadata().await,
        harness.document_count().await,
        harness.ready().await,
    );
    assert!(!before.2);
    let refused = harness
        .db
        .schema()
        .backfill_runtime_search(&stale, &harness.records(), options)
        .await
        .unwrap_err();
    assert_eq!(
        stale_codes(refused),
        [(
            RuntimeMigrationDiagnosticCode::SearchConfigurationMismatch,
            "fields".to_string()
        )]
    );
    let after = (
        harness.metadata().await,
        harness.document_count().await,
        harness.ready().await,
    );
    assert_eq!(after, before, "nothing written");

    // The current schema still completes, in resumable batches.
    let partial = harness.backfill(options.with_max_batches(1)).await;
    assert_eq!(
        (partial.processed, partial.remaining, partial.ready),
        (2, 3, false)
    );
    let finished = harness.backfill(options).await;
    assert_eq!((finished.processed, finished.ready), (3, true));
    // Documents follow the current declaration, which no longer indexes the body.
    let (text, _, _body) = harness.document(5).await.expect("document");
    assert!(text.contains("title 5"), "{text}");
    #[cfg(feature = "sqlite")]
    assert!(_body.is_empty(), "{_body}");
    #[cfg(feature = "postgres")]
    assert!(!text.contains("body"), "{text}");

    // A ready index refuses a stale schema too, and stays ready.
    let ready = harness.metadata().await;
    let refused = harness
        .db
        .schema()
        .backfill_runtime_search(&stale, &harness.records(), options)
        .await
        .unwrap_err();
    assert_eq!(
        stale_codes(refused)[0].0,
        RuntimeMigrationDiagnosticCode::SearchConfigurationMismatch
    );
    assert_eq!(harness.metadata().await, ready);
    assert!(harness.ready().await);

    // Every part of the declaration is fenced, each reported by name.
    let mut variants = Vec::new();
    let mut folded = harness.schema().schema().clone();
    folded.collections[0]
        .search
        .as_mut()
        .unwrap()
        .remove_diacritics = true;
    variants.push((folded, "tokenizer"));
    let mut language = harness.schema().schema().clone();
    language.collections[0].search.as_mut().unwrap().language = Some("simple".into());
    variants.push((language, "language"));
    let mut shortest = harness.schema().schema().clone();
    shortest.collections[0]
        .search
        .as_mut()
        .unwrap()
        .min_token_len = Some(3);
    variants.push((shortest, "min_token_len"));
    let mut reweighted = harness.schema().schema().clone();
    reweighted.collections[0].search.as_mut().unwrap().fields[0].weight = RuntimeSearchWeight::B;
    variants.push((reweighted, "fields"));
    let mut repathed = harness.schema().schema().clone();
    repathed.collections[0].search.as_mut().unwrap().fields[1].json_path = Some("$.other".into());
    variants.push((repathed, "fields"));
    for (variant, part) in variants {
        let refused = harness
            .db
            .schema()
            .backfill_runtime_search(&variant.validate().unwrap(), &harness.records(), options)
            .await
            .unwrap_err();
        assert_eq!(
            stale_codes(refused),
            [(
                RuntimeMigrationDiagnosticCode::SearchConfigurationMismatch,
                part.to_string()
            )]
        );
    }
    assert_eq!(harness.metadata().await, ready);
}

/// `porter` stemming keeps accent folding with `remove_diacritics`.
#[tokio::test]
async fn porter_with_remove_diacritics_stems_and_folds_accents() {
    let mut definition = definition();
    let search = definition.collections[0].search.as_mut().unwrap();
    search.tokenizer = Some(RuntimeSearchTokenizer::Porter);
    search.remove_diacritics = true;
    let harness = Harness::with_schema(definition).await;
    harness
        .create(1, "Māori carvings", "art", "whakairo")
        .await
        .unwrap();
    harness
        .create(2, "Café Society", "history", "Wellington")
        .await
        .unwrap();
    harness
        .create(3, "Bridges", "steel", "harbour")
        .await
        .unwrap();
    for (query, expected) in [
        ("maori carving", vec![1]),
        ("Māori", vec![1]),
        ("cafe", vec![2]),
        ("café", vec![2]),
        ("bridge", vec![3]),
    ] {
        let found = harness
            .search(
                query,
                false,
                None,
                None,
                RuntimePageRequest::first(10, None),
            )
            .await
            .unwrap();
        assert_eq!(harness.ids(&found), expected, "{query}");
    }
}
