#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
//! Nested closure parity for SQLite column collation and affinity.
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "probe_nodes", plural = "ProbeNodes")]
struct ProbeNode {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: i64,
    parent_id: Option<i64>,
    name: Option<String>,
    code: Option<String>,
}
schema_roots! { query_custom_ops: [], entities: [ProbeNode] }

type B = DefaultBackend;
type Node = ProbeNode;

#[derive(Clone, Default)]
struct LastSql(std::sync::Arc<std::sync::Mutex<String>>);
impl graphql_orm::graphql::orm::ReadQueryObserver for LastSql {
    fn on_read(&self, sql: &str, _: usize) {
        *self.0.lock().unwrap() = sql.to_owned();
    }
}

#[tokio::test]
async fn nested_compact_matches_original_on_collation_and_affinity() {
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    for sql in [
        "CREATE TABLE probe_nodes (id INTEGER PRIMARY KEY, parent_id INTEGER NULL, \
         name TEXT COLLATE NOCASE NULL, code TEXT NULL)",
        "INSERT INTO probe_nodes VALUES (1, NULL, 'ABC', '5'), (2, 1, 'x', '6'), (3, 2, 'y', '7')",
    ] {
        <B as WriteBackend>::execute_with_binds(db.pool(), sql, &[])
            .await
            .unwrap();
    }
    let last = LastSql::default();
    let db = db.with_read_query_observer(last.clone());
    let parent: &[(&str, &str)] = &[("parent_id", "id")];
    let cases: Vec<(&str, Box<dyn Fn() -> ReadPredicate>)> = vec![
        (
            "single all(name <> 'abc')",
            Box::new(|| {
                ReadPredicate::related_closure_within::<B, Node>(
                    ReadPredicateQuantifier::All,
                    parent,
                    ReadPredicate::column::<B, Node>(
                        "name",
                        ReadPredicateOp::Ne,
                        SqlValue::String("abc".into()),
                    )
                    .unwrap(),
                    3,
                )
                .unwrap()
            }),
        ),
        (
            "nested all(all(name <> 'abc'))",
            Box::new(|| {
                let inner = ReadPredicate::related_closure_within::<B, Node>(
                    ReadPredicateQuantifier::All,
                    parent,
                    ReadPredicate::column::<B, Node>(
                        "name",
                        ReadPredicateOp::Ne,
                        SqlValue::String("abc".into()),
                    )
                    .unwrap(),
                    3,
                )
                .unwrap();
                ReadPredicate::related_closure_within::<B, Node>(
                    ReadPredicateQuantifier::All,
                    parent,
                    inner,
                    3,
                )
                .unwrap()
            }),
        ),
        (
            "nested any(any(name = 'abc'))",
            Box::new(|| {
                let inner = ReadPredicate::related_closure_within::<B, Node>(
                    ReadPredicateQuantifier::Any,
                    parent,
                    ReadPredicate::column::<B, Node>(
                        "name",
                        ReadPredicateOp::Eq,
                        SqlValue::String("abc".into()),
                    )
                    .unwrap(),
                    3,
                )
                .unwrap();
                ReadPredicate::related_closure_within::<B, Node>(
                    ReadPredicateQuantifier::Any,
                    parent,
                    inner,
                    3,
                )
                .unwrap()
            }),
        ),
        (
            "nested all(all(code <> 5 as Int))",
            Box::new(|| {
                let inner = ReadPredicate::related_closure_within::<B, Node>(
                    ReadPredicateQuantifier::All,
                    parent,
                    ReadPredicate::column::<B, Node>("code", ReadPredicateOp::Ne, SqlValue::Int(5))
                        .unwrap(),
                    3,
                )
                .unwrap();
                ReadPredicate::related_closure_within::<B, Node>(
                    ReadPredicateQuantifier::All,
                    parent,
                    inner,
                    3,
                )
                .unwrap()
            }),
        ),
    ];
    let mut mismatches = Vec::new();
    for (name, make) in &cases {
        let read = |policy: ReadPredicate| {
            let db = &db;
            let last = &last;
            async move {
                let mut ids = EntityQuery::<Node>::new()
                    .with_read_visibility(&ReadVisibility::Complete(policy))
                    .unwrap()
                    .fetch_all(db)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|row| row.id)
                    .collect::<Vec<_>>();
                ids.sort_unstable();
                (ids, last.0.lock().unwrap().clone())
            }
        };
        let original = make()
            .and(ReadPredicate::is_not_null::<B, Node>("id").unwrap())
            .unwrap();
        let (before, _) = read(original).await;
        let (after, sql) = read(make()).await;
        eprintln!("{name}: original {before:?}, compact {after:?}");
        if before != after {
            eprintln!("  compact SQL: {sql}");
            mismatches.push(format!("{name}: original {before:?}, compact {after:?}"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
