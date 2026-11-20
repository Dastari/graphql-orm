// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
//! Read predicates bind every column they render to their own row (#796).
//!
//! Relation, `not_related` and closure predicates evaluate the target's
//! predicate inside a correlated subquery. An unqualified column there that
//! the target table does not physically have (schema drift, or a migration
//! not yet applied) resolves against an outer row instead, on every backend,
//! so the policy would be evaluated against the wrong row. Here the target
//! table lacks `flag` while the source table has a `flag` column: every read
//! path must fail rather than admit rows. Once the column exists, the same
//! shapes admit exactly the rows the target's own values select.

#[path = "support/graphql_case.rs"]
mod graphql_case;
#[cfg(feature = "mssql")]
#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
use graphql_case::{argument_name as a, field_name as f, resolver_name as r};
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::prelude::*;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        table = "bind_sources",
        plural = "BindSources",
        aggregate = true,
        keyset = "id asc",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "bind_sources",
        plural = "BindSources",
        aggregate = true,
        keyset = "id asc",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "bind_sources",
        plural = "BindSources",
        aggregate = true,
        keyset = "id asc",
        default_sort = "id ASC"
    )
)]
struct BindSource {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    #[filterable(type = "number")]
    flag: Option<i32>,
    #[relation(
        target = "BindTarget",
        from = "id",
        to = "source_id",
        multiple,
        filter,
        emit_fk = false,
        order_aggregate(name = "TargetCount", aggregate = "count")
    )]
    targets: Vec<BindTarget>,
}

/// Declares `flag`, which the physical table lacks until the control phase.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(table = "bind_targets", plural = "BindTargets")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(backend = "postgres", table = "bind_targets", plural = "BindTargets")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "bind_targets",
        plural = "BindTargets"
    )
)]
struct BindTarget {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    source_id: String,
    up: Option<String>,
    #[filterable(type = "number")]
    flag: Option<i32>,
}

/// Reads its source through a relation loader, under the source's policy.
#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    async_graphql::SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(table = "bind_owners", plural = "BindOwners", default_sort = "id ASC")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "bind_owners",
        plural = "BindOwners",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "bind_owners",
        plural = "BindOwners",
        default_sort = "id ASC"
    )
)]
struct BindOwner {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    source_id: String,
    #[graphql(skip)]
    #[relation(target = "BindSource", from = "source_id", to = "id", emit_fk = false)]
    source: Option<BindSource>,
}

#[cfg(feature = "sqlite")]
schema_roots! { query_custom_ops: [], entities: [BindSource, BindTarget, BindOwner] }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [BindSource, BindTarget, BindOwner] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [BindSource, BindTarget, BindOwner] }

type B = DefaultBackend;

#[cfg(feature = "sqlite")]
type Owned = ();
#[cfg(feature = "postgres")]
type Owned = owned_postgres::OwnedPostgres;
#[cfg(feature = "mssql")]
type Owned = owned_mssql::OwnedSqlServer;

#[cfg(feature = "mssql")]
const TEXT: &str = "NVARCHAR(64)";
#[cfg(not(feature = "mssql"))]
const TEXT: &str = "TEXT";
#[cfg(feature = "mssql")]
const ADD_FLAG: &str = "ALTER TABLE bind_targets ADD flag INT NULL";
#[cfg(not(feature = "mssql"))]
const ADD_FLAG: &str = "ALTER TABLE bind_targets ADD COLUMN flag INT NULL";

#[cfg(feature = "sqlite")]
const TIMESTAMP: &str = "TEXT";
#[cfg(feature = "postgres")]
const TIMESTAMP: &str = "TIMESTAMP";
#[cfg(feature = "mssql")]
const TIMESTAMP: &str = "DATETIME2";

const KEYS: &[(&str, &str)] = &[("id", "source_id")];
const UP: &[(&str, &str)] = &[("up", "id")];
const SOURCES: [&str; 4] = ["s1", "s2", "s3", "s4"];

fn flag(op: ReadPredicateOp, value: i64) -> ReadPredicate {
    ReadPredicate::column::<B, BindTarget>("flag", op, SqlValue::Int(value)).unwrap()
}

fn related(target: ReadPredicate) -> ReadPredicate {
    ReadPredicate::related::<B, BindSource, BindTarget>(KEYS, target).unwrap()
}

/// Source policies whose only `flag` is the target's, each with the sources
/// it admits once the target has the column (see [`fixture`]).
fn shapes() -> Vec<(&'static str, ReadPredicate, &'static [&'static str])> {
    let eq = || flag(ReadPredicateOp::Eq, 1);
    vec![
        ("related column", related(eq()), &["s2"]),
        (
            "related is_null",
            related(ReadPredicate::is_null::<B, BindTarget>("flag").unwrap()),
            &["s3"],
        ),
        (
            "related is_not_null",
            related(ReadPredicate::is_not_null::<B, BindTarget>("flag").unwrap()),
            &["s1", "s2"],
        ),
        (
            "not exists",
            related(eq()).not().unwrap(),
            &["s1", "s3", "s4"],
        ),
        (
            "not_related",
            ReadPredicate::not_related::<B, BindSource, BindTarget>(KEYS, eq()).unwrap(),
            &["s1", "s4"],
        ),
        (
            "related closure",
            related(ReadPredicate::related_closure::<B, BindTarget>(UP, eq()).unwrap()),
            &["s2", "s3"],
        ),
        (
            "related closure_all",
            related(
                ReadPredicate::related_closure_all::<B, BindTarget>(
                    UP,
                    flag(ReadPredicateOp::Gte, 0),
                )
                .unwrap(),
            ),
            &["s1", "s2"],
        ),
        (
            "related from_filter",
            related(
                ReadPredicate::from_filter::<B, _>(&BindTargetWhereInput {
                    flag: Some(IntFilter {
                        eq: Some(1),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .unwrap(),
            ),
            &["s2"],
        ),
    ]
}

/// The target's policy as a hand-written filter: every column through the
/// row qualifier, compared with a constant that needs no row.
#[derive(Clone)]
struct UnexpiredFlagged;
impl DatabaseFilter for UnexpiredFlagged {
    fn to_sql_conditions(&self) -> (Vec<String>, Vec<SqlValue>) {
        (
            vec!["__gom_self.flag = ? AND __gom_self.expires_at > CURRENT_TIMESTAMP".to_owned()],
            vec![SqlValue::Int(1)],
        )
    }
    fn is_empty(&self) -> bool {
        false
    }
}
impl ReadVisibilityFilter for UnexpiredFlagged {
    fn entity_type_id() -> std::any::TypeId {
        std::any::TypeId::of::<BindTarget>()
    }
    fn backend() -> DatabaseBackend {
        B::DIALECT
    }
    fn table_name() -> Option<&'static str> {
        Some(BindTarget::TABLE_NAME)
    }
}

/// Whether the target's policy is the hand-written [`UnexpiredFlagged`].
static TARGET_WRITTEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The source policy in force: an index into [`shapes`], or `usize::MAX`
/// for unrestricted sources.
static SHAPE: AtomicUsize = AtomicUsize::new(usize::MAX);

struct Policy;
impl RowPolicy<B> for Policy {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<B>,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            Ok(match entity {
                "BindSource" => match shapes().into_iter().nth(SHAPE.load(Ordering::SeqCst)) {
                    Some((_, predicate, _)) => ReadVisibility::Complete(predicate),
                    None => ReadVisibility::Unrestricted,
                },
                "BindTarget" if TARGET_WRITTEN.load(Ordering::SeqCst) => {
                    ReadVisibility::Complete(ReadPredicate::from_filter::<B, _>(&UnexpiredFlagged)?)
                }
                "BindTarget" => ReadVisibility::Complete(flag(ReadPredicateOp::Eq, 1)),
                "BindOwner" => ReadVisibility::Unrestricted,
                _ => ReadVisibility::CallbackOnly,
            })
        })
    }
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<B>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<B>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

async fn execute(db: &Database<B>, sql: &str) {
    <B as WriteBackend>::execute_with_binds(db.pool(), sql, &[])
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"));
}

/// Sources (`flag`): s1 (NULL), s2 (0), s3 (5), s4 (1). Targets, created
/// without `flag` (the value it gets in the control phase in brackets):
/// t1 -> s1 [0], a root; t2 -> s2 [1], up t1; t3 -> s3 [NULL], up t2.
/// s4 has no target. Each source's flag differs from its target's, in
/// nullness too for s1 and s3, so a predicate bound to the source row would
/// admit different rows under every shape, the NULL tests included.
async fn fixture() -> (Database<B>, Owned) {
    #[cfg(feature = "sqlite")]
    let (db, owned) = (
        Database::<B>::connect_sqlite("sqlite::memory:")
            .await
            .unwrap(),
        (),
    );
    #[cfg(feature = "postgres")]
    let (db, owned) = {
        let owned = Owned::start("read-predicate-row-binding").unwrap();
        let db = Database::<B>::connect_postgres(&owned.url).await.unwrap();
        (db, owned)
    };
    #[cfg(feature = "mssql")]
    let (db, owned) = {
        let owned = Owned::start().await.unwrap();
        let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(
            &owned.connection_string,
        )
        .await
        .unwrap();
        // Writable, for the bulk writes below.
        let db = Database::builder(pool)
            .schema_policy(SchemaPolicy::ExternalWritable)
            .build();
        (db, owned)
    };
    for statement in [
        format!("CREATE TABLE bind_sources (id {TEXT} NOT NULL PRIMARY KEY, flag INT NULL)"),
        format!(
            "CREATE TABLE bind_targets (id {TEXT} NOT NULL PRIMARY KEY, source_id {TEXT} NOT NULL, up {TEXT} NULL, expires_at {TIMESTAMP} NOT NULL DEFAULT '9999-01-01 00:00:00')"
        ),
        "INSERT INTO bind_sources (id, flag) VALUES ('s1', NULL), ('s2', 0), ('s3', 5), ('s4', 1)"
            .to_owned(),
        "INSERT INTO bind_targets (id, source_id, up) VALUES ('t1', 's1', NULL), ('t2', 's2', 't1'), ('t3', 's3', 't2')"
            .to_owned(),
        format!("CREATE TABLE bind_owners (id {TEXT} NOT NULL PRIMARY KEY, source_id {TEXT} NOT NULL)"),
        "INSERT INTO bind_owners (id, source_id) VALUES ('o1', 's1'), ('o2', 's2'), ('o3', 's3'), ('o4', 's4')"
            .to_owned(),
    ] {
        execute(&db, &statement).await;
    }
    (db, owned)
}

/// `db` with the row policy installed, and bulk writes narrowed to the rows
/// the policy lets the caller read.
fn policed(db: &Database<B>) -> Database<B> {
    let mut db = db.clone().with_narrow_bulk_writes_to_read_visibility(true);
    db.set_row_policy(Policy);
    db
}

fn schema(db: &Database<B>) -> async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot> {
    let db = policed(db);
    async_graphql::Schema::build(
        QueryRoot::default(),
        MutationRoot::default(),
        SubscriptionRoot::default(),
    )
    .data(db)
    .data(AuthSubject::builder("reader").build())
    .finish()
}

fn ids(connection: &Value) -> Vec<String> {
    connection["edges"]
        .as_array()
        .unwrap_or_else(|| panic!("connection edges: {connection}"))
        .iter()
        .map(|edge| edge["node"][f("id")].as_str().unwrap().to_owned())
        .collect()
}

/// The GraphQL reads of the sources: a page with its total, a keyset page
/// with its total, and a point read of each source.
fn graphql_reads() -> Vec<String> {
    let selection = format!(
        "edges {{ node {{ {} }} }} pageInfo {{ totalCount }}",
        f("id")
    );
    let mut reads = vec![
        format!(
            "{{ rows: {}({}: {{limit: 10}}) {{ {selection} }} }}",
            r("bindSources"),
            a("page")
        ),
        format!(
            "{{ rows: {}({}: {{limit: 10, includeTotalCount: true}}) {{ {selection} }} }}",
            r("bindSourcesKeyset"),
            a("page")
        ),
    ];
    reads.extend(SOURCES.iter().map(|id| {
        format!(
            "{{ row: {}({}: {id:?}) {{ {} }} }}",
            r("bindSource"),
            a("id"),
            f("id")
        )
    }));
    reads
}

/// The sources one GraphQL read admits, or its errors.
async fn graphql_admitted(
    schema: &async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot>,
    query: &str,
) -> Result<Vec<String>, String> {
    let response = schema.execute(query).await;
    if !response.errors.is_empty() {
        return Err(format!("{:?}", response.errors));
    }
    let data = response.data.into_json().unwrap();
    Ok(if let Some(row) = data.get("row") {
        row.get(f("id"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .into_iter()
            .collect()
    } else {
        let rows = &data["rows"];
        let ids = ids(rows);
        assert_eq!(rows["pageInfo"]["totalCount"], ids.len(), "{query}");
        ids
    })
}

/// The sources the repository list admits under `predicate`.
async fn repository_list(
    db: &Database<B>,
    predicate: &ReadPredicate,
) -> Result<Vec<String>, String> {
    let rows = EntityQuery::<BindSource>::new()
        .with_read_visibility(&ReadVisibility::Complete(predicate.clone()))
        .map_err(|error| format!("{error:?}"))?
        .fetch_all(db)
        .await
        .map_err(|error| format!("{error:?}"))?;
    let mut ids = rows.into_iter().map(|row| row.id).collect::<Vec<_>>();
    ids.sort();
    Ok(ids)
}

/// How many sources the repository count admits under `predicate`.
async fn repository_count(db: &Database<B>, predicate: &ReadPredicate) -> Result<i64, String> {
    EntityQuery::<BindSource>::new()
        .with_read_visibility(&ReadVisibility::Complete(predicate.clone()))
        .map_err(|error| format!("{error:?}"))?
        .count(db)
        .await
        .map_err(|error| format!("{error:?}"))
}

/// Whether `error` is the database refusing `flag` because the row's table
/// does not have it, rather than any other failure.
fn missing_flag(error: &str) -> bool {
    #[cfg(feature = "sqlite")]
    let refused = error.contains("no such column") && error.contains("flag");
    #[cfg(feature = "postgres")]
    let refused =
        (error.contains("42703") || error.contains("does not exist")) && error.contains("flag");
    #[cfg(feature = "mssql")]
    let refused = error.contains("Invalid column name") && error.contains("flag");
    refused
}

/// The server-side diagnostics of the errors GraphQL reports as sanitized
/// codes: the `detail` of each `graphql_orm::internal_error` event.
static DIAGNOSTICS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

struct Diagnostics;
impl tracing::Subscriber for Diagnostics {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == "graphql_orm::internal_error"
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Detail(Option<String>);
        impl tracing::field::Visit for Detail {
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                if field.name() == "detail" {
                    self.0 = Some(value.to_owned());
                }
            }
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "detail" {
                    self.0 = Some(format!("{value:?}"));
                }
            }
        }
        let mut detail = Detail(None);
        event.record(&mut detail);
        DIAGNOSTICS.lock().unwrap().extend(detail.0);
    }
}

/// `outcome` of a GraphQL operation, a failure followed by the server-side
/// diagnostics recorded while it ran.
fn with_cause(outcome: Result<String, String>) -> Result<String, String> {
    let causes = std::mem::take(&mut *DIAGNOSTICS.lock().unwrap());
    outcome.map_err(|error| format!("{error} caused by {causes:?}"))
}

/// Whether a GraphQL read failed with the sanitized internal error, and its
/// server-side diagnostic is the database refusing the missing column.
fn database_refusal(errors: &str) -> bool {
    errors.contains("\"INTERNAL_ERROR\"") && missing_flag(errors)
}

/// How many sources the aggregate root's row count admits under the policy
/// in force.
async fn aggregate_count(db: &Database<B>) -> Result<i64, String> {
    let rows = BindSource::aggregate(&policed(db))
        .count_rows()
        .map_err(|error| format!("{error:?}"))?
        .fetch()
        .await
        .map_err(|error| format!("{error:?}"))?;
    match rows.as_slice() {
        [row] => match row.metrics[0].value {
            AggregateValue::Count(count) => Ok(count),
            ref other => Err(format!("unexpected metric {other:?}")),
        },
        other => Err(format!("unexpected rows {other:?}")),
    }
}

/// The sources the relation loader returns for the four owners, which are
/// unrestricted themselves, or the errors of their `source` fields.
async fn loaded_sources(
    schema: &async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot>,
) -> Result<Vec<String>, String> {
    let response = schema
        .execute(format!(
            "{{ rows: {}({}: {{limit: 10}}) {{ edges {{ node {{ {} source {{ {} }} }} }} }} }}",
            r("bindOwners"),
            a("page"),
            f("id"),
            f("id")
        ))
        .await;
    let data = response.data.into_json().unwrap();
    let edges = data["rows"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let loaded = edges
        .iter()
        .filter_map(|edge| edge["node"]["source"][f("id")].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    if response.errors.is_empty() {
        assert_eq!(edges.len(), SOURCES.len(), "{data}");
        return Ok(loaded);
    }
    // A failed load returns no source at all.
    assert!(loaded.is_empty(), "{data}");
    assert_eq!(
        response.errors.len(),
        SOURCES.len(),
        "{:?}",
        response.errors
    );
    Err(format!("{:?}", response.errors))
}

/// One bulk write of the source `s3` (`flag = 5`), narrowed to the rows the
/// policy in force admits: an update that keeps its value, or a delete. It
/// returns how many rows the write selected, or its error code.
async fn bulk_write(
    schema: &async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot>,
    delete: bool,
) -> Result<i64, String> {
    let (field, arguments, count) = if delete {
        (
            r("deleteBindSources"),
            format!("{}: {{ {}: {{ eq: 5 }} }}", a("where"), f("flag")),
            "deletedCount",
        )
    } else {
        (
            r("updateBindSources"),
            format!(
                "{}: {{ {}: {{ eq: 5 }} }}, {}: {{ {}: 5 }}",
                a("where"),
                f("flag"),
                a("input"),
                f("flag")
            ),
            "affectedCount",
        )
    };
    let response = schema
        .execute(format!(
            "mutation {{ write: {field}({arguments}) {{ success errorCode {count} }} }}"
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let write = &data["write"];
    if write["success"] == true {
        Ok(write[count].as_i64().unwrap())
    } else {
        assert_eq!(write[count], 0, "{write}");
        Err(write["errorCode"].to_string())
    }
}

/// The sources as stored, whatever the policy.
async fn stored_sources(db: &Database<B>) -> Vec<(String, Option<i32>)> {
    let mut rows = EntityQuery::<BindSource>::new()
        .fetch_all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|row| (row.id, row.flag))
        .collect::<Vec<_>>();
    rows.sort();
    rows
}

/// Generated reads of the sources that evaluate the target's own policy
/// (`flag = 1`) in a subquery, with unrestricted sources: a related-row
/// filter and a relation-count ordering.
fn target_policy_reads() -> [String; 2] {
    [
        format!(
            "{{ rows: {}({}: {{ {}: [{{ {}: {{ ne: \"none\" }} }}] }}, {}: {{limit: 10}}) {{ edges {{ node {{ {} }} }} pageInfo {{ totalCount }} }} }}",
            r("bindSources"),
            a("where"),
            f("targets"),
            f("id"),
            a("page"),
            f("id"),
        ),
        format!(
            "{{ rows: {}({}: [{{ TargetCount: DESC }}], {}: {{limit: 10}}) {{ edges {{ node {{ {} }} }} pageInfo {{ totalCount }} }} }}",
            r("bindSources"),
            a("orderBy"),
            a("page"),
            f("id"),
        ),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn policy_columns_bind_to_their_own_row() {
    // Record the server-side diagnostics behind sanitized GraphQL errors.
    // SAFETY: this binary's only test sets the variable before it starts
    // any other thread that reads the environment.
    unsafe { std::env::set_var("GRAPHQL_ORM_LOG_INTERNAL_ERRORS", "1") };
    tracing::subscriber::set_global_default(Diagnostics).unwrap();
    let (db, _owned) = fixture().await;
    let schema = schema(&db);

    // Drift: the target table lacks `flag`. Every read fails, and fails
    // because the database refuses the missing column: repository reads
    // carry its error, GraphQL reads the sanitized internal error.
    let stored = stored_sources(&db).await;
    let mut wrong = Vec::new();
    let mut check =
        |read: String, outcome: Result<String, String>, refused: fn(&str) -> bool| match outcome {
            Ok(admitted) => wrong.push(format!("{read} admitted {admitted}")),
            Err(error) if !refused(&error) => {
                wrong.push(format!("{read} failed otherwise: {error}"))
            }
            Err(_) => {}
        };
    let shown = |rows: Vec<String>| format!("{rows:?}");
    for (index, (name, predicate, _)) in shapes().into_iter().enumerate() {
        // Each read path on its own, so one failing cannot hide another.
        check(
            format!("{name}: repository list"),
            repository_list(&db, &predicate).await.map(shown),
            missing_flag,
        );
        check(
            format!("{name}: repository count"),
            repository_count(&db, &predicate)
                .await
                .map(|count| count.to_string()),
            missing_flag,
        );
        SHAPE.store(index, Ordering::SeqCst);
        for query in graphql_reads() {
            DIAGNOSTICS.lock().unwrap().clear();
            check(
                format!("{name}: {query}"),
                with_cause(graphql_admitted(&schema, &query).await.map(shown)),
                database_refusal,
            );
        }
        // The aggregate root, a relation loader and the locking read that
        // selects a bulk write's targets render the same policy.
        check(
            format!("{name}: aggregate"),
            aggregate_count(&db).await.map(|count| count.to_string()),
            missing_flag,
        );
        DIAGNOSTICS.lock().unwrap().clear();
        check(
            format!("{name}: relation loader"),
            with_cause(loaded_sources(&schema).await.map(shown)),
            database_refusal,
        );
        for delete in [false, true] {
            DIAGNOSTICS.lock().unwrap().clear();
            check(
                format!("{name}: bulk write (delete: {delete})"),
                with_cause(
                    bulk_write(&schema, delete)
                        .await
                        .map(|count| count.to_string()),
                ),
                |outcome| outcome.starts_with("\"INTERNAL_ERROR\" ") && missing_flag(outcome),
            );
        }
    }
    SHAPE.store(usize::MAX, Ordering::SeqCst);
    for query in target_policy_reads() {
        DIAGNOSTICS.lock().unwrap().clear();
        check(
            query.clone(),
            with_cause(graphql_admitted(&schema, &query).await.map(shown)),
            database_refusal,
        );
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    // No refused bulk write changed a row.
    assert_eq!(stored_sources(&db).await, stored);

    // Control: with the column in place, each shape admits exactly the rows
    // its target values select, on every read path.
    execute(&db, ADD_FLAG).await;
    execute(&db, "UPDATE bind_targets SET flag = 0 WHERE id = 't1'").await;
    execute(&db, "UPDATE bind_targets SET flag = 1 WHERE id = 't2'").await;
    for (index, (name, predicate, expected)) in shapes().into_iter().enumerate() {
        assert_eq!(
            repository_list(&db, &predicate).await.unwrap(),
            expected,
            "{name}"
        );
        assert_eq!(
            repository_count(&db, &predicate).await.unwrap(),
            expected.len() as i64,
            "{name}"
        );
        SHAPE.store(index, Ordering::SeqCst);
        let mut point = Vec::new();
        for query in graphql_reads() {
            let admitted = graphql_admitted(&schema, &query)
                .await
                .unwrap_or_else(|error| panic!("{name}: {query}: {error}"));
            if query.contains("row:") {
                point.extend(admitted);
            } else {
                assert_eq!(admitted, expected, "{name}: {query}");
            }
        }
        assert_eq!(point, expected, "{name}: point reads");
        assert_eq!(
            aggregate_count(&db).await.unwrap(),
            expected.len() as i64,
            "{name}: aggregate"
        );
        assert_eq!(
            loaded_sources(&schema).await.unwrap(),
            expected,
            "{name}: relation loader"
        );
        // The bulk update selects `s3` only where the policy admits it.
        assert_eq!(
            bulk_write(&schema, false).await.unwrap(),
            i64::from(expected.contains(&"s3")),
            "{name}: bulk update"
        );
    }
    // A bulk delete leaves a hidden row in place and removes a visible one.
    assert_eq!(stored_sources(&db).await, stored);
    let admits_s3 = |admits: bool| {
        shapes()
            .iter()
            .position(|(_, _, expected)| expected.contains(&"s3") == admits)
            .unwrap()
    };
    SHAPE.store(admits_s3(false), Ordering::SeqCst);
    assert_eq!(bulk_write(&schema, true).await.unwrap(), 0);
    assert_eq!(stored_sources(&db).await, stored);
    SHAPE.store(admits_s3(true), Ordering::SeqCst);
    assert_eq!(bulk_write(&schema, true).await.unwrap(), 1);
    assert_eq!(stored_sources(&db).await.len(), stored.len() - 1);
    execute(&db, "INSERT INTO bind_sources (id, flag) VALUES ('s3', 5)").await;
    SHAPE.store(usize::MAX, Ordering::SeqCst);
    let [filtered, ordered] = target_policy_reads();
    assert_eq!(graphql_admitted(&schema, &filtered).await.unwrap(), ["s2"]);
    assert_eq!(
        graphql_admitted(&schema, &ordered).await.unwrap(),
        ["s2", "s1", "s3", "s4"]
    );

    // The same two reads with the target's policy written by hand: bound
    // through the qualifier, it is accepted in the relation's and the
    // count's subquery and admits the same rows.
    TARGET_WRITTEN.store(true, Ordering::SeqCst);
    assert_eq!(graphql_admitted(&schema, &filtered).await.unwrap(), ["s2"]);
    assert_eq!(
        graphql_admitted(&schema, &ordered).await.unwrap(),
        ["s2", "s1", "s3", "s4"]
    );
    TARGET_WRITTEN.store(false, Ordering::SeqCst);

    drop(schema);
    #[cfg(not(feature = "mssql"))]
    db.pool().close().await;
}

/// A generated related-row filter and a relation-count ordering carry their
/// target's read policy, and its bind values, inside the source query's own
/// clause and ordering term. A query that holds one is checked as a policed
/// query even with no policy of its own: caller SQL may not shift those
/// binds. Targets here have `flag` from the start.
#[tokio::test(flavor = "multi_thread")]
async fn a_policy_inside_a_filter_or_ordering_polices_the_query() {
    let (db, _owned) = fixture().await;
    execute(&db, ADD_FLAG).await;
    execute(&db, "UPDATE bind_targets SET flag = 1 WHERE id = 't2'").await;
    let text = |value: &str| SqlValue::String(value.into());
    // A prepared related-row filter: targets the request may read
    // (`flag = ?` with 1), as the resolver stores it after authorization.
    let related = || {
        let mut filter = BindSourceWhereInput {
            targets: Some(vec![]),
            ..Default::default()
        };
        filter.__gom_related.insert(
            f("targets").into(),
            (
                vec![
                    "EXISTS (SELECT 1 FROM bind_targets WHERE bind_targets.source_id = bind_sources.id AND bind_targets.flag = ?)"
                        .into(),
                ],
                vec![SqlValue::Int(1)],
            ),
        );
        filter
    };
    // A prepared relation-count ordering under the same policy.
    let counted = || {
        let mut order = BindSourceOrderByInput {
            target_count: Some(OrderDirection::Desc),
            ..Default::default()
        };
        order.__gom_relation_counts.insert(
            "TargetCount".to_owned(),
            (
                "(SELECT COUNT(*) FROM bind_targets WHERE bind_targets.source_id = bind_sources.id AND bind_targets.flag = ?)"
                    .to_owned(),
                vec![SqlValue::Int(1)],
            ),
        );
        order
    };
    let ids = |rows: Vec<BindSource>| rows.into_iter().map(|row| row.id).collect::<Vec<_>>();
    // A placeholder the statement does not number itself, per dialect.
    let named = match B::DIALECT {
        DatabaseBackend::Sqlite => "id <> ?1",
        DatabaseBackend::Postgres => "id <> $other",
        _ => "id <> @other",
    };
    let refused = |result: graphql_orm::Result<Vec<BindSource>>, context: &str| {
        let error = result.expect_err(context);
        let code = graphql_orm::graphql::errors::graphql_error_from_sqlx(error)
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get("code"))
            .map(ToString::to_string);
        assert_eq!(
            code.as_deref(),
            Some("\"AUTHORIZATION_MISCONFIGURED\""),
            "{context}"
        );
    };

    // The related-row filter: with anonymous placeholders the caller's
    // value and the policy's each reach their own clause.
    let filtered = |clause: &str, values: Vec<SqlValue>| {
        EntityQuery::<BindSource>::new()
            .where_values(clause, values)
            .filter(&related())
    };
    assert_eq!(
        ids(filtered("id <> ?", vec![text("s9")])
            .fetch_all(&db)
            .await
            .unwrap()),
        ["s2"]
    );
    assert!(
        ids(filtered("id <> ?", vec![text("s2")])
            .fetch_all(&db)
            .await
            .unwrap())
        .is_empty()
    );
    refused(
        filtered(named, vec![text("s9")]).fetch_all(&db).await,
        "related filter, named placeholder",
    );
    refused(
        filtered("id <> ?", vec![text("s9"), SqlValue::Int(0)])
            .fetch_all(&db)
            .await,
        "related filter, surplus value",
    );

    // The count ordering: `s2` has the one counted target.
    let ordered = |clause: &str, values: Vec<SqlValue>| {
        EntityQuery::<BindSource>::new()
            .where_values(clause, values)
            .order_by(&counted())
    };
    assert_eq!(
        ids(ordered("id <> ?", vec![text("s4")])
            .fetch_all(&db)
            .await
            .unwrap())[0],
        "s2"
    );
    refused(
        ordered(named, vec![text("s4")]).fetch_all(&db).await,
        "count ordering, named placeholder",
    );
    refused(
        ordered("id <> ?", vec![text("s4"), SqlValue::Int(0)])
            .fetch_all(&db)
            .await,
        "count ordering, surplus value",
    );
    // An ordering term whose own binds do not pair up is refused too.
    let mut lopsided = counted();
    lopsided
        .__gom_relation_counts
        .get_mut("TargetCount")
        .unwrap()
        .1
        .push(SqlValue::Int(9));
    refused(
        EntityQuery::<BindSource>::new()
            .order_by(&lopsided)
            .fetch_all(&db)
            .await,
        "count ordering, lopsided term",
    );

    // With no policy carried, the caller's SQL is passed through as written.
    if B::DIALECT == DatabaseBackend::Sqlite {
        let rows = EntityQuery::<BindSource>::new()
            .where_values("id = ?1 OR id = ?1", vec![text("s4")])
            .fetch_all(&db)
            .await
            .unwrap();
        assert_eq!(ids(rows), ["s4"]);
    }
    #[cfg(not(feature = "mssql"))]
    db.pool().close().await;
}
