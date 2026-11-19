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

#[cfg(feature = "sqlite")]
schema_roots! { query_custom_ops: [], entities: [BindSource, BindTarget] }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [BindSource, BindTarget] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [BindSource, BindTarget] }

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
                "BindTarget" => ReadVisibility::Complete(flag(ReadPredicateOp::Eq, 1)),
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
        Box::pin(async { Ok(false) })
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
        (Database::new(pool), owned)
    };
    for statement in [
        format!("CREATE TABLE bind_sources (id {TEXT} NOT NULL PRIMARY KEY, flag INT NULL)"),
        format!(
            "CREATE TABLE bind_targets (id {TEXT} NOT NULL PRIMARY KEY, source_id {TEXT} NOT NULL, up {TEXT} NULL)"
        ),
        "INSERT INTO bind_sources (id, flag) VALUES ('s1', NULL), ('s2', 0), ('s3', 5), ('s4', 1)"
            .to_owned(),
        "INSERT INTO bind_targets (id, source_id, up) VALUES ('t1', 's1', NULL), ('t2', 's2', 't1'), ('t3', 's3', 't2')"
            .to_owned(),
    ] {
        execute(&db, &statement).await;
    }
    (db, owned)
}

fn schema(db: &Database<B>) -> async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot> {
    let mut db = db.clone();
    db.set_row_policy(Policy);
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
    let (db, _owned) = fixture().await;
    let schema = schema(&db);

    // Drift: the target table lacks `flag`. Every read fails.
    let mut admitted = Vec::new();
    for (index, (name, predicate, _)) in shapes().into_iter().enumerate() {
        // Each read path on its own, so one failing cannot hide another.
        if let Ok(rows) = repository_list(&db, &predicate).await {
            admitted.push(format!("{name}: repository list admitted {rows:?}"));
        }
        if let Ok(count) = repository_count(&db, &predicate).await {
            admitted.push(format!("{name}: repository count admitted {count}"));
        }
        SHAPE.store(index, Ordering::SeqCst);
        for query in graphql_reads() {
            if let Ok(rows) = graphql_admitted(&schema, &query).await {
                admitted.push(format!("{name}: {query} admitted {rows:?}"));
            }
        }
    }
    SHAPE.store(usize::MAX, Ordering::SeqCst);
    for query in target_policy_reads() {
        if let Ok(rows) = graphql_admitted(&schema, &query).await {
            admitted.push(format!("{query} admitted {rows:?}"));
        }
    }
    assert!(admitted.is_empty(), "{}", admitted.join("\n"));

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
    }
    SHAPE.store(usize::MAX, Ordering::SeqCst);
    let [filtered, ordered] = target_policy_reads();
    assert_eq!(graphql_admitted(&schema, &filtered).await.unwrap(), ["s2"]);
    assert_eq!(
        graphql_admitted(&schema, &ordered).await.unwrap(),
        ["s2", "s1", "s3", "s4"]
    );

    drop(schema);
    #[cfg(not(feature = "mssql"))]
    db.pool().close().await;
}
