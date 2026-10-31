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
use graphql_orm::{
    db::Database,
    graphql::{orm::*, runtime::*},
};
use std::sync::{
    Arc,
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
                remove_diacritics: false,
                min_token_len: None,
            }),
        }],
    }
}

/// Grants one tenant and records whether a search was authorized.
struct Authority {
    deny_body_search: AtomicBool,
    saw_search: AtomicBool,
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

    let authority = Arc::new(Authority {
        deny_body_search: AtomicBool::new(false),
        saw_search: AtomicBool::new(false),
    });
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

    // Without search, edges have a null score.
    let plain = api
        .execute(run(&format!(
            "{{ records {{ edges {{ {score} node {{ id }} }} }} }}"
        )))
        .await;
    assert!(plain.errors.is_empty(), "{:?}", plain.errors);
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
