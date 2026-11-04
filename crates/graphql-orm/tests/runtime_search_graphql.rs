//! Runtime GraphQL `search` on a list connection: ranking, scores, readiness,
//! host authorization of every searchable field, and hidden rows.
#![cfg(all(
    feature = "runtime-graphql",
    any(
        all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
        all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
    )
))]

use futures::future::BoxFuture;
use graphql_orm::async_graphql::{Request, Variables};
use graphql_orm::graphql::filters::SearchMode;
use graphql_orm::{
    db::Database,
    graphql::{orm::*, runtime::*},
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;

fn fid(value: &str) -> FieldId {
    FieldId::new(value).unwrap()
}

/// A search member in the configured field case, as the SDL spells it.
fn member(camel: &str) -> String {
    use convert_case::{Case, Casing};
    if cfg!(feature = "field-case-pascal") {
        camel.to_case(Case::Pascal)
    } else if cfg!(feature = "field-case-snake") {
        camel.to_case(Case::Snake)
    } else {
        camel.to_string()
    }
}

fn field(name: &str, kind: RuntimeValueKind) -> RuntimeField {
    RuntimeField {
        id: fid(&format!("records.{name}")),
        api_name: name.into(),
        physical_column: name.into(),
        value_kind: kind,
        nullable: false,
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
    tokenized(false, None)
}

/// The searchable definition with the given document tokenization.
fn tokenized(remove_diacritics: bool, min_token_len: Option<u8>) -> RuntimeSchema {
    let entry = |name: &str, weight| RuntimeSearchField {
        field: fid(&format!("records.{name}")),
        weight,
        json_path: None,
    };
    RuntimeSchema {
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![RuntimeCollection {
            id: CollectionId::new("records").unwrap(),
            api_type_name: "Record".into(),
            api_plural_name: "Records".into(),
            physical_table: "gql_search_records".into(),
            primary_key: vec![fid("records.id")],
            append_only: false,
            retention_purge: false,
            fields: vec![
                field("id", RuntimeValueKind::Integer),
                field("tenant", RuntimeValueKind::String),
                field("title", RuntimeValueKind::String),
                field("body", RuntimeValueKind::String),
            ],
            relations: vec![],
            indexes: vec![],
            composite_unique: vec![],
            default_order: vec![],
            description: None,
            classification: None,
            label_fields: vec![],
            search: Some(RuntimeSearch {
                fields: vec![
                    entry("title", RuntimeSearchWeight::A),
                    entry("body", RuntimeSearchWeight::C),
                ],
                language: None,
                tokenizer: None,
                remove_diacritics,
                min_token_len,
            }),
        }],
    }
}

/// Grants one tenant and records whether a search was authorized.
#[derive(Default)]
struct Authority {
    deny_body_search: AtomicBool,
    saw_search: AtomicBool,
    /// A host allowlist of modes that withholds `WEB`.
    deny_web: AtomicBool,
    /// The search reported by the last check, as (mode, minScore, ranked).
    last_search: Mutex<Option<(SearchMode, bool, bool)>>,
}

impl RuntimeReadAuthority for Authority {
    fn authorize<'a>(
        &'a self,
        check: RuntimeReadCheck<'a>,
    ) -> BoxFuture<'a, Result<RuntimeReadGrant, RuntimeGraphqlError>> {
        Box::pin(async move {
            let searched = check
                .filter_operations
                .iter()
                .filter(|(_, op)| *op == "search")
                .map(|(field, _)| field.id().as_str().to_owned())
                .collect::<Vec<_>>();
            // The check reports the search directly, beside its field operations.
            assert_eq!(check.search.is_some(), !searched.is_empty());
            *self.last_search.lock().unwrap() = check
                .search
                .map(|search| (search.mode, search.min_score, search.ranked));
            if check
                .search
                .is_some_and(|search| search.mode == SearchMode::Web)
                && self.deny_web.load(Ordering::SeqCst)
            {
                return Err(RuntimeGraphqlError::denied());
            }
            if !searched.is_empty() {
                assert_eq!(searched, ["records.title", "records.body"]);
                self.saw_search.store(true, Ordering::SeqCst);
                if self.deny_body_search.load(Ordering::SeqCst) {
                    return Err(RuntimeGraphqlError::denied());
                }
            }
            let tenant = check
                .schema
                .resolve_field(check.collection, &fid("records.tenant"))
                .unwrap();
            let predicate = check
                .schema
                .runtime_compare(
                    check.collection,
                    &tenant,
                    RuntimeScalarOperator::Eq,
                    RuntimeValue::String("north".into()),
                    RuntimeQueryLimits::default(),
                )
                .unwrap();
            Ok(RuntimeReadGrant::new(Some(predicate)))
        })
    }
}

#[tokio::test]
async fn graphql_search_ranks_scores_waits_for_readiness_and_hides_rows() {
    #[cfg(feature = "postgres")]
    let owned = owned_postgres::OwnedPostgres::start("runtime-search-graphql").unwrap();
    #[cfg(feature = "postgres")]
    let database = Database::<Backend>::connect_postgres(&owned.url)
        .await
        .unwrap();
    #[cfg(feature = "sqlite")]
    let database = Database::<Backend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();

    let schema = Arc::new(definition().validate().unwrap());
    let target = schema
        .physical_schema::<Backend>(Default::default())
        .unwrap();
    let ownership = ManagedTableSet::new(["gql_search_records".into()]).unwrap();
    let plan = database
        .schema()
        .plan_owned_migration("init", "search", &target, &ownership, PlanOptions::strict())
        .await
        .unwrap();
    // Leave the index building, as a host that runs the backfill itself.
    database
        .schema()
        .apply_runtime_migration(
            &plan,
            &schema,
            ApplyOptions::default(),
            RuntimeSearchApplyOptions::deferred(),
        )
        .await
        .unwrap();
    graphql_orm::sqlx::query(
        "INSERT INTO gql_search_records (id, tenant, title, body) VALUES \
         (1, 'north', 'Harbour Bridge', 'steel arch'), \
         (2, 'north', 'Opera House', 'near the bridge'), \
         (3, 'south', 'Bridge bridge bridge', 'bridge bridge'), \
         (4, 'north', 'Museum', 'galleries')",
    )
    .execute(database.pool())
    .await
    .unwrap();

    let module = RuntimeGraphqlModule::compile(
        schema.clone(),
        RuntimeGraphqlOptions::default().with_cursor_profile(RuntimeCursorProfile::Unprotected),
    )
    .unwrap();
    let api = RuntimeGraphqlComposer::new(database.clone(), "Query", Default::default())
        .unwrap()
        .install(module)
        .unwrap()
        .finish()
        .unwrap();
    let sdl = api.sdl();
    assert!(sdl.contains("search: RuntimeSearchInput"), "{sdl}");
    assert!(
        sdl.contains(&format!("{}: Float", member("score"))),
        "{sdl}"
    );
    assert!(sdl.contains("input RuntimeSearchInput"), "{sdl}");

    let authority = Arc::new(Authority::default());
    let run = |query: &str| {
        Request::new(query).variables(Variables::default()).data(
            RuntimeGraphqlRequest::<Backend>::new(
                schema.fingerprint(),
                authority.clone(),
                None,
                RuntimeGraphqlLimits::default(),
            ),
        )
    };
    let (score, query) = (member("score"), member("query"));
    let searched = format!(
        "{{ records(search: {{ {query}: \"bridge\" }}) {{ totalCount edges {{ {score} node {{ id title }} }} }} }}"
    );
    let searched = searched.as_str();

    // An index still being built refuses search instead of answering partially.
    let building = api.execute(run(searched)).await;
    let code = building.errors[0]
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.get("code"))
        .map(ToString::to_string);
    assert_eq!(
        code.as_deref(),
        Some("\"search_index_not_ready\""),
        "{building:?}"
    );

    let progress = database
        .schema()
        .backfill_runtime_search(
            &schema,
            &CollectionId::new("records").unwrap(),
            Default::default(),
        )
        .await
        .unwrap();
    assert!(progress.ready);

    let ranked = api.execute(run(searched)).await;
    assert!(ranked.errors.is_empty(), "{:?}", ranked.errors);
    let ranked = ranked.data.into_json().unwrap();
    let edges = ranked["records"]["edges"].as_array().unwrap();
    // The south record matches best but is hidden by the grant: it never appears,
    // and the count does not reveal it.
    assert_eq!(
        edges
            .iter()
            .map(|edge| edge["node"]["id"].clone())
            .collect::<Vec<_>>(),
        ["1", "2"]
    );
    assert_eq!(ranked["records"]["totalCount"], "2");
    assert!(edges.iter().all(|edge| edge[&score].as_f64().is_some()));
    assert!(edges[0][&score].as_f64() >= edges[1][&score].as_f64());
    assert!(authority.saw_search.load(Ordering::SeqCst));
    assert_eq!(
        *authority.last_search.lock().unwrap(),
        Some((SearchMode::Plain, false, true))
    );

    // A host may withhold a mode, here WEB, which SQLite evaluates like PLAIN.
    let (mode, min_score) = (member("mode"), member("minScore"));
    let web = format!(
        "{{ records(search: {{ {query}: \"bridge\", {mode}: WEB }}) {{ edges {{ node {{ id }} }} }} }}"
    );
    assert!(api.execute(run(&web)).await.errors.is_empty());
    authority.deny_web.store(true, Ordering::SeqCst);
    let withheld = api.execute(run(&web)).await;
    assert_eq!(
        withheld.errors[0]
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get("code"))
            .map(ToString::to_string)
            .as_deref(),
        Some("\"authorization_denied\""),
        "{withheld:?}"
    );
    let phrase = api
        .execute(run(&format!(
            "{{ records(search: {{ {query}: \"harbour bridge\", {mode}: PHRASE, {min_score}: 0.0 }}) {{ edges {{ node {{ id }} }} }} }}"
        )))
        .await;
    assert!(phrase.errors.is_empty(), "{:?}", phrase.errors);
    assert_eq!(
        *authority.last_search.lock().unwrap(),
        Some((SearchMode::Phrase, true, true))
    );

    // An explicit order wins over relevance.
    let ordered = api
        .execute(run(&format!(
            "{{ records(search: {{ {query}: \"bridge\" }}, orderBy: [{{ field: title, direction: ASC }}]) {{ edges {{ node {{ title }} }} }} }}"
        )))
        .await;
    assert!(ordered.errors.is_empty(), "{:?}", ordered.errors);
    let titles = ordered.data.into_json().unwrap()["records"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| edge["node"]["title"].clone())
        .collect::<Vec<_>>();
    assert_eq!(titles, ["Harbour Bridge", "Opera House"]);
    assert_eq!(
        *authority.last_search.lock().unwrap(),
        Some((SearchMode::Plain, false, false)),
        "an explicit order is not ranked"
    );

    // Without search, edges have a null score.
    let plain = api
        .execute(run(&format!(
            "{{ records {{ edges {{ {score} node {{ id }} }} }} }}"
        )))
        .await;
    assert!(plain.errors.is_empty(), "{:?}", plain.errors);
    assert_eq!(*authority.last_search.lock().unwrap(), None);
    assert!(
        plain.data.into_json().unwrap()["records"]["edges"]
            .as_array()
            .unwrap()
            .iter()
            .all(|edge| edge[&score].is_null())
    );

    // A host that may not let this caller search a field refuses the request
    // with the ordinary denial, before any read.
    authority.deny_body_search.store(true, Ordering::SeqCst);
    let denied = api.execute(run(searched)).await;
    let code = denied.errors[0]
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.get("code"))
        .map(ToString::to_string);
    assert_eq!(
        code.as_deref(),
        Some("\"authorization_denied\""),
        "{denied:?}"
    );
    #[cfg(feature = "postgres")]
    drop(owned);
}

/// Plan the migration to `schema` and apply it with the backfill left to the
/// host, returning the plan.
async fn migrate_deferred(
    database: &Database<Backend>,
    version: &str,
    schema: &ValidatedRuntimeSchema,
) -> OwnedPlannedMigration {
    let target = schema
        .physical_schema::<Backend>(Default::default())
        .unwrap();
    let ownership = ManagedTableSet::new(["gql_search_records".into()]).unwrap();
    let plan = database
        .schema()
        .plan_owned_migration(
            version,
            "search",
            &target,
            &ownership,
            PlanOptions::strict(),
        )
        .await
        .unwrap();
    database
        .schema()
        .apply_runtime_migration(
            &plan,
            schema,
            ApplyOptions::default(),
            RuntimeSearchApplyOptions::deferred(),
        )
        .await
        .unwrap();
    plan
}

/// Whether the plan recreates search storage, which then needs a rebuild.
fn rebuilds_search(plan: &OwnedPlannedMigration) -> bool {
    plan.steps()
        .iter()
        .any(|step| step.reason().contains("requires an explicit rebuild"))
}

/// Search `records` for `term` through runtime GraphQL, returning the matching
/// ids or the error code.
async fn search_ids(
    database: &Database<Backend>,
    schema: &Arc<ValidatedRuntimeSchema>,
    term: &str,
) -> Result<Vec<String>, String> {
    let module = RuntimeGraphqlModule::compile(
        schema.clone(),
        RuntimeGraphqlOptions::default().with_cursor_profile(RuntimeCursorProfile::Unprotected),
    )
    .unwrap();
    let api = RuntimeGraphqlComposer::new(database.clone(), "Query", Default::default())
        .unwrap()
        .install(module)
        .unwrap()
        .finish()
        .unwrap();
    let authority = Arc::new(Authority::default());
    let query = format!(
        "{{ records(search: {{ {}: \"{term}\" }}, orderBy: [{{ field: id, direction: ASC }}]) {{ edges {{ node {{ id }} }} }} }}",
        member("query")
    );
    let response = api
        .execute(
            Request::new(query).data(RuntimeGraphqlRequest::<Backend>::new(
                schema.fingerprint(),
                authority,
                None,
                RuntimeGraphqlLimits::default(),
            )),
        )
        .await;
    if let Some(error) = response.errors.first() {
        return Err(error
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get("code"))
            .map(ToString::to_string)
            .unwrap_or_else(|| error.message.clone()));
    }
    Ok(response.data.into_json().unwrap()["records"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| edge["node"]["id"].as_str().unwrap().to_owned())
        .collect())
}

#[tokio::test]
async fn changing_document_tokenization_rebuilds_the_index() {
    #[cfg(feature = "postgres")]
    let owned = owned_postgres::OwnedPostgres::start("runtime-search-rebuild").unwrap();
    #[cfg(feature = "postgres")]
    let database = Database::<Backend>::connect_postgres(&owned.url)
        .await
        .unwrap();
    #[cfg(feature = "sqlite")]
    let database = Database::<Backend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();

    let plain = Arc::new(definition().validate().unwrap());
    migrate_deferred(&database, "init", &plain).await;
    graphql_orm::sqlx::query(
        "INSERT INTO gql_search_records (id, tenant, title, body) VALUES \
         (1, 'north', 'Te Reo Māori', 'language'), \
         (2, 'north', 'Maori carving', 'art')",
    )
    .execute(database.pool())
    .await
    .unwrap();
    assert!(
        database
            .schema()
            .backfill_runtime_search(
                &plain,
                &CollectionId::new("records").unwrap(),
                Default::default()
            )
            .await
            .unwrap()
            .ready
    );
    // PostgreSQL keeps accents without `remove_diacritics`. SQLite's default
    // `unicode61` tokenizer already folds most of them.
    #[cfg(feature = "postgres")]
    assert_eq!(search_ids(&database, &plain, "maori").await.unwrap(), ["2"]);

    // Both settings change how documents or queries are tokenized, so each is
    // a recreation that needs a rebuild, like fields, weights and language.
    let target = |schema: &ValidatedRuntimeSchema| {
        schema
            .physical_schema::<Backend>(Default::default())
            .unwrap()
    };
    let ownership = ManagedTableSet::new(["gql_search_records".into()]).unwrap();
    let shorter = tokenized(false, Some(3)).validate().unwrap();
    let plan = database
        .schema()
        .plan_owned_migration(
            "shorter",
            "search",
            &target(&shorter),
            &ownership,
            PlanOptions::strict(),
        )
        .await
        .unwrap();
    assert!(rebuilds_search(&plan), "min_token_len plans a rebuild");

    let folded = Arc::new(tokenized(true, None).validate().unwrap());
    let plan = migrate_deferred(&database, "folded", &folded).await;
    assert!(rebuilds_search(&plan), "remove_diacritics plans a rebuild");
    // The recreated index is building, so search is refused until the rebuild.
    assert_eq!(
        search_ids(&database, &folded, "maori").await,
        Err("\"search_index_not_ready\"".to_owned())
    );
    let progress = database
        .schema()
        .backfill_runtime_search(
            &folded,
            &CollectionId::new("records").unwrap(),
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!((progress.processed, progress.ready), (2, true));
    for term in ["maori", "Māori", "MĀORI"] {
        assert_eq!(
            search_ids(&database, &folded, term).await.unwrap(),
            ["1", "2"],
            "{term}"
        );
    }
    #[cfg(feature = "postgres")]
    drop(owned);
}
