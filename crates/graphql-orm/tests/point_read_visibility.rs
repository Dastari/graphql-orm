// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(feature = "sqlite")]
//! Generated point reads (get by key, `NodeId` and Federation `_entities`)
//! apply the same row authority as generated list reads: the request's
//! `RowPolicy::read_visibility` SQL predicate, plus `can_read_row` only when
//! that decision requires residual checks.

use graphql_orm::prelude::*;
use serde_json::Value;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[path = "support/graphql_case.rs"]
mod graphql_case;
use graphql_case::{argument_name, field_name, resolver_name};

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "point_docs",
    plural = "PointDocs",
    default_sort = "id ASC",
    node_id,
    federation_key
)]
struct PointDoc {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    owner: String,
    #[filterable(type = "string")]
    title: String,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "keyed_docs",
    plural = "KeyedDocs",
    default_sort = "id ASC",
    read_policy = "keyed.read",
    node_id,
    federation_key
)]
struct KeyedDoc {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    title: String,
}

schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [PointDoc, KeyedDoc],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// SQL is the whole decision; the row callback allows everything, as a
    /// host relying on the predicate alone would write it.
    Complete,
    /// SQL narrows by owner; the row callback also hides "hidden" titles.
    Prefilter,
    /// `read_visibility` fails with a raw internal error.
    FailVisibility,
    /// `Prefilter`, and `can_read_row` fails with a raw internal error.
    FailCallback,
    /// `Prefilter`; resolving the mutation-surface decision stands in for a
    /// concurrent write that hides the row after the commit.
    ConcurrentHide,
    /// As `ConcurrentHide`, but the concurrent write renames the row.
    ConcurrentRename,
}

/// A provider diagnostic that must never reach a client.
const RAW_PROVIDER_ERROR: &str = "relation \"tenant_secret\" does not exist on host db-7";

/// A database failure a provider propagated with `?`.
fn raw_provider_error() -> async_graphql::Error {
    async_graphql::Error::from(sqlx::Error::Protocol(RAW_PROVIDER_ERROR.into()))
}

#[derive(Clone)]
struct OwnerPolicy {
    mode: Mode,
    callbacks: Arc<AtomicUsize>,
}

fn subject_id(ctx: Option<&async_graphql::Context<'_>>) -> String {
    ctx.and_then(|ctx| ctx.auth_subject_opt())
        .map(|subject| subject.id)
        .unwrap_or_default()
}

impl RowPolicy for OwnerPolicy {
    fn read_visibility<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a Database,
        entity_name: &'static str,
        _: Option<&'static str>,
        surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            if entity_name != "PointDoc" {
                return Ok(ReadVisibility::Unrestricted);
            }
            if self.mode == Mode::FailVisibility {
                return Err(raw_provider_error());
            }
            let concurrent_title = match self.mode {
                Mode::ConcurrentHide => Some("hidden"),
                Mode::ConcurrentRename => Some("concurrent"),
                _ => None,
            };
            if let Some(title) = concurrent_title
                && surface == EntityAccessSurface::GraphqlMutation
            {
                sqlx::query("UPDATE point_docs SET title = ? WHERE id = 'a1'")
                    .bind(title)
                    .execute(db.pool())
                    .await?;
            }
            // Repository reads stand in for a trusted job scoped to one owner.
            let owner = if surface == EntityAccessSurface::Repository {
                "alice".to_string()
            } else {
                subject_id(ctx)
            };
            let filter = PointDocWhereInput {
                owner: Some(StringFilter {
                    eq: Some(owner),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let predicate = ReadPredicate::from_filter::<SqliteBackend, _>(&filter)?;
            Ok(match self.mode {
                Mode::Complete => ReadVisibility::Complete(predicate),
                _ => ReadVisibility::Prefilter(predicate),
            })
        })
    }

    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            self.callbacks.fetch_add(1, Ordering::Relaxed);
            if self.mode == Mode::FailCallback {
                return Err(raw_provider_error());
            }
            Ok(match row.downcast_ref::<PointDoc>() {
                Some(row) => self.mode == Mode::Complete || row.title != "hidden",
                None => true,
            })
        })
    }

    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

struct AllowEntities;

impl EntityPolicy for AllowEntities {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

async fn seeded_database() -> Database {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE point_docs (id TEXT PRIMARY KEY, owner TEXT NOT NULL, title TEXT NOT NULL)",
    )
    .execute(database.pool())
    .await
    .unwrap();
    sqlx::query("CREATE TABLE keyed_docs (id TEXT PRIMARY KEY, title TEXT NOT NULL)")
        .execute(database.pool())
        .await
        .unwrap();
    for (id, owner, title) in [
        ("a1", "alice", "first"),
        ("a2", "alice", "hidden"),
        ("b1", "bob", "other"),
    ] {
        sqlx::query("INSERT INTO point_docs (id, owner, title) VALUES (?, ?, ?)")
            .bind(id)
            .bind(owner)
            .bind(title)
            .execute(database.pool())
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO keyed_docs (id, title) VALUES ('k1', 'keyed')")
        .execute(database.pool())
        .await
        .unwrap();
    database
}

async fn policy_database(mode: Mode) -> (Database, Arc<AtomicUsize>) {
    let mut database = seeded_database().await;
    let callbacks = Arc::new(AtomicUsize::new(0));
    database.set_row_policy(OwnerPolicy {
        mode,
        callbacks: callbacks.clone(),
    });
    (database, callbacks)
}

async fn execute(db: &Database, user: &str, query: String) -> async_graphql::Response {
    schema_builder(db.clone())
        .finish()
        .execute(async_graphql::Request::new(query).data(AuthSubject::new(user)))
        .await
}

async fn data(db: &Database, user: &str, query: String) -> Value {
    let response = execute(db, user, query).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().unwrap()
}

fn get_query(id: &str) -> String {
    format!(
        "{{ {}({}: {id:?}) {{ {} }} }}",
        resolver_name("pointDoc"),
        argument_name("id"),
        field_name("title"),
    )
}

fn list_ids(json: &Value) -> Vec<String> {
    json[resolver_name("pointDocs")][field_name("edges")]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| {
            edge[field_name("node")][field_name("id")]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect()
}

fn list_query_for(root: &str, field: &str) -> String {
    format!(
        "{{ {} {{ {} {{ {} {{ {} }} }} }} }}",
        resolver_name(root),
        field_name("edges"),
        field_name("node"),
        field_name(field),
    )
}

fn list_query() -> String {
    list_query_for("pointDocs", "id")
}

fn node_id_of(typename: &str, id: &str) -> String {
    graphql_orm::graphql::federation::encode_node_id(typename, vec![serde_json::json!(id)]).0
}

fn entities_by_key_query_of(typename: &str, id: &str) -> String {
    format!(
        "{{ _entities(representations: [{{ __typename: {typename:?}, {}: {id:?} }}]) {{ ... on {typename} {{ {} }} }} }}",
        field_name("id"),
        field_name("title"),
    )
}

fn entities_by_node_id_query_of(typename: &str, id: &str) -> String {
    format!(
        "{{ _entities(representations: [{{ __typename: {typename:?}, NodeId: {:?} }}]) {{ ... on {typename} {{ {} }} }} }}",
        node_id_of(typename, id),
        field_name("title"),
    )
}

fn entities_by_key_query(id: &str) -> String {
    entities_by_key_query_of("PointDoc", id)
}

fn entities_by_node_id_query(id: &str) -> String {
    entities_by_node_id_query_of("PointDoc", id)
}

/// Builds one single-row query for a key.
type QueryFor = fn(&str) -> String;

/// The error messages and the released value of a single-row read.
async fn outcome(db: &Database, query: String, root: &str) -> (Vec<String>, Value) {
    let response = execute(db, "alice", query).await;
    let messages = response
        .errors
        .iter()
        .map(|error| error.message.clone())
        .collect::<Vec<_>>();
    let json = response.data.into_json().unwrap_or(Value::Null);
    let value = if root == "_entities" {
        json["_entities"][0].clone()
    } else {
        json[root].clone()
    };
    (messages, value)
}

/// Reads `id` through every generated point-read surface and returns the
/// title each one released.
async fn point_reads(db: &Database, user: &str, id: &str) -> [Value; 3] {
    let title = field_name("title");
    let by_key = data(db, user, get_query(id)).await[resolver_name("pointDoc")].clone();
    let by_entity_key = data(db, user, entities_by_key_query(id)).await["_entities"][0].clone();
    let by_node_id = data(db, user, entities_by_node_id_query(id)).await["_entities"][0].clone();
    [
        by_key.get(title).cloned().unwrap_or(Value::Null),
        by_entity_key.get(title).cloned().unwrap_or(Value::Null),
        by_node_id.get(title).cloned().unwrap_or(Value::Null),
    ]
}

#[tokio::test]
async fn complete_predicate_governs_point_reads_like_lists() {
    let (db, callbacks) = policy_database(Mode::Complete).await;

    let listed = list_ids(&data(&db, "alice", list_query()).await);
    assert_eq!(listed, ["a1", "a2"]);

    // Outside the SQL predicate: hidden from every point read, although the
    // row callback would allow it.
    assert_eq!(
        point_reads(&db, "alice", "b1").await,
        [Value::Null, Value::Null, Value::Null]
    );
    let first = Value::from("first");
    assert_eq!(
        point_reads(&db, "alice", "a1").await,
        [first.clone(), first.clone(), first]
    );
    // A complete decision is sufficient: no row callback runs.
    assert_eq!(callbacks.load(Ordering::Relaxed), 0);

    assert_eq!(list_ids(&data(&db, "bob", list_query()).await), ["b1"]);
    let other = Value::from("other");
    assert_eq!(
        point_reads(&db, "bob", "b1").await,
        [other.clone(), other.clone(), other]
    );
    assert_eq!(
        point_reads(&db, "bob", "a1").await,
        [Value::Null, Value::Null, Value::Null]
    );
}

#[tokio::test]
async fn prefilter_predicate_and_row_callback_both_apply_to_point_reads() {
    let (db, callbacks) = policy_database(Mode::Prefilter).await;

    // Excluded by SQL even though the callback would allow it.
    assert_eq!(
        point_reads(&db, "alice", "b1").await,
        [Value::Null, Value::Null, Value::Null]
    );
    // Matched by SQL but refused by the residual callback.
    assert_eq!(
        point_reads(&db, "alice", "a2").await,
        [Value::Null, Value::Null, Value::Null]
    );
    let first = Value::from("first");
    assert_eq!(
        point_reads(&db, "alice", "a1").await,
        [first.clone(), first.clone(), first]
    );
    assert!(callbacks.load(Ordering::Relaxed) > 0);
}

#[tokio::test]
async fn strict_mode_without_a_row_provider_refuses_policy_keyed_lists_and_point_reads() {
    for mode in [
        AuthorizationMode::ExplicitPolicyForAllExposedOperations,
        AuthorizationMode::DeclaredPoliciesRequired,
    ] {
        let mut db = seeded_database().await;
        db.set_entity_policy(AllowEntities);
        db.set_authorization_mode(mode);

        // Lists defer to `can_read_row` too; without an authorized scan
        // configuration the list refuses rather than release any row.
        let list = execute(&db, "alice", list_query_for("keyedDocs", "title")).await;
        assert!(
            list.errors
                .iter()
                .any(|error| error.message == "authorization is misconfigured"),
            "{mode:?}: a policy-keyed list released rows without a row provider: {:?}",
            list
        );

        // On every single-row surface an existing and a missing key answer
        // alike, so the response is no existence oracle.
        let expected: Vec<String> = if mode == AuthorizationMode::DeclaredPoliciesRequired {
            vec!["authorization is misconfigured".to_string()]
        } else {
            Vec::new()
        };
        let surfaces: [(&str, QueryFor); 3] = [
            (resolver_name("keyedDoc"), |id| {
                format!(
                    "{{ {}({}: {id:?}) {{ {} }} }}",
                    resolver_name("keyedDoc"),
                    argument_name("id"),
                    field_name("title"),
                )
            }),
            ("_entities", |id| entities_by_key_query_of("KeyedDoc", id)),
            ("_entities", |id| {
                entities_by_node_id_query_of("KeyedDoc", id)
            }),
        ];
        for (root, query) in surfaces {
            let existing = outcome(&db, query("k1"), root).await;
            let missing = outcome(&db, query("missing"), root).await;
            assert!(
                existing.1.is_null(),
                "{mode:?} {}: released {:?}",
                query("k1"),
                existing.1
            );
            assert_eq!(existing.0, missing.0, "{mode:?} {}", query("k1"));
            assert_eq!(existing.0, expected, "{mode:?} {}", query("k1"));
        }
    }

    // The compatibility mode keeps its permissive behaviour on both paths.
    let mut db = seeded_database().await;
    db.set_entity_policy(AllowEntities);
    db.set_authorization_mode(AuthorizationMode::LegacyPermissive);
    let json = data(
        &db,
        "alice",
        format!(
            "{{ {}({}: \"k1\") {{ {} }} }}",
            resolver_name("keyedDoc"),
            argument_name("id"),
            field_name("title"),
        ),
    )
    .await;
    assert_eq!(
        json[resolver_name("keyedDoc")][field_name("title")],
        "keyed"
    );
}

#[tokio::test]
async fn provider_errors_on_point_reads_are_sanitized() {
    for mode in [Mode::FailVisibility, Mode::FailCallback] {
        let (db, _) = policy_database(mode).await;
        // A failing decision fails missing keys too; the residual callback
        // only runs for an existing row.
        let ids: &[&str] = if mode == Mode::FailVisibility {
            &["a1", "missing"]
        } else {
            &["a1"]
        };
        for query in ids.iter().flat_map(|id| {
            [
                get_query(id),
                entities_by_key_query(id),
                entities_by_node_id_query(id),
            ]
        }) {
            let response = execute(&db, "alice", query.clone()).await;
            assert!(!response.errors.is_empty(), "{query}: expected a refusal");
            let rendered = format!("{:?}", response.errors);
            assert!(
                !rendered.contains("tenant_secret") && !rendered.contains("db-7"),
                "{query}: provider diagnostic reached the client: {rendered}"
            );
        }
    }
}

fn update_query(id: &str) -> String {
    format!(
        "mutation {{ {}({}: {id:?}, {}: {{ {}: \"renamed\" }}) {{ {} {} {{ {} }} }} }}",
        resolver_name("updatePointDoc"),
        argument_name("id"),
        argument_name("input"),
        field_name("title"),
        field_name("success"),
        field_name("pointDoc"),
        field_name("title"),
    )
}

#[tokio::test]
async fn mutation_output_is_withheld_outside_the_read_predicate() {
    let (db, callbacks) = policy_database(Mode::Complete).await;

    // Writable but outside alice's complete predicate: the write commits and
    // the row is withheld, as a list read would hide it.
    let json = data(&db, "alice", update_query("b1")).await;
    let payload = &json[resolver_name("updatePointDoc")];
    assert_eq!(payload[field_name("success")], true, "{json}");
    assert!(payload[field_name("pointDoc")].is_null(), "{json}");
    let stored: (String,) = sqlx::query_as("SELECT title FROM point_docs WHERE id = 'b1'")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(stored.0, "renamed");

    let json = data(&db, "alice", update_query("a1")).await;
    assert_eq!(
        json[resolver_name("updatePointDoc")][field_name("pointDoc")][field_name("title")],
        "renamed"
    );
    // A complete decision is sufficient for the payload too.
    assert_eq!(callbacks.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn repository_point_reads_apply_the_repository_predicate() {
    let (db, callbacks) = policy_database(Mode::Complete).await;
    assert!(
        PointDoc::find_by_id(&db, &"a1".to_string())
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        PointDoc::find_by_id(&db, &"b1".to_string())
            .await
            .unwrap()
            .is_none()
    );
    // Repository reads keep calling the row callback for every row.
    assert_eq!(callbacks.load(Ordering::Relaxed), 1);
}

struct FailingEntities;

impl EntityPolicy for FailingEntities {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Err(raw_provider_error()) })
    }
}

#[tokio::test]
async fn entity_policy_errors_on_point_reads_are_sanitized() {
    let (mut db, _) = policy_database(Mode::Complete).await;
    db.set_entity_policy(FailingEntities);
    for query in [
        get_query("a1"),
        entities_by_key_query("a1"),
        entities_by_node_id_query("a1"),
    ] {
        let response = execute(&db, "alice", query.clone()).await;
        assert!(!response.errors.is_empty(), "{query}: expected a refusal");
        let rendered = format!("{:?}", response.errors);
        assert!(
            !rendered.contains("tenant_secret") && !rendered.contains("db-7"),
            "{query}: provider diagnostic reached the client: {rendered}"
        );
    }
}

#[tokio::test]
async fn mutation_output_checks_and_releases_the_reread_row() {
    // The row is changed after the commit and before the release check. The
    // payload carries the re-read row, and the callback judges that row.
    let (db, _) = policy_database(Mode::ConcurrentRename).await;
    let json = data(&db, "alice", update_query("a1")).await;
    assert_eq!(
        json[resolver_name("updatePointDoc")][field_name("pointDoc")][field_name("title")],
        "concurrent",
        "{json}"
    );

    let (db, _) = policy_database(Mode::ConcurrentHide).await;
    let json = data(&db, "alice", update_query("a1")).await;
    let payload = &json[resolver_name("updatePointDoc")];
    assert_eq!(payload[field_name("success")], true, "{json}");
    assert!(payload[field_name("pointDoc")].is_null(), "{json}");
}
