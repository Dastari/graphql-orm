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
        let schema = Arc::new(definition.validate().unwrap());
        let target = schema
            .physical_schema::<Backend>(Default::default())
            .unwrap();
        let ownership = ManagedTableSet::new(["search_records".into()]).unwrap();
        let plan = self
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
        self.run(update).await.unwrap();
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
