#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
//! SQLite renders nested closures in the original form. These cases cover
//! column collation, affinity and collation-equal pointers.
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "closure_collation_nodes", plural = "ClosureCollationNodes")]
struct ClosureCollationNode {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: i64,
    parent_id: Option<i64>,
    name: Option<String>,
    code: Option<String>,
}

/// A text pointer with a case-insensitive collation naming a case-sensitive
/// key, with several rows per key.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "closure_fan_out_nodes", plural = "ClosureFanOutNodes")]
struct ClosureFanOutNode {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: i64,
    parent_id: Option<i64>,
    ptr: Option<String>,
    key: Option<String>,
    score: Option<i64>,
}
schema_roots! { query_custom_ops: [], entities: [ClosureCollationNode, ClosureFanOutNode] }

/// The last read statement.
#[derive(Clone, Default)]
struct SqlText(std::sync::Arc<std::sync::Mutex<String>>);
impl graphql_orm::graphql::orm::ReadQueryObserver for SqlText {
    fn on_read(&self, sql: &str, _: usize) {
        *self.0.lock().unwrap() = sql.to_owned();
    }
}

/// The first closure subquery in `sql`, through its closing parenthesis.
/// Read statements bind every value, so the SQL has no string literals.
fn closure_sql(sql: &str) -> &str {
    let start = sql.find("(WITH RECURSIVE").expect("closure subquery");
    let mut depth = 0_usize;
    for (offset, character) in sql[start..].char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &sql[start..=start + offset];
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced closure subquery");
}

async fn execute(db: &Database<SqliteBackend>, statements: &[&str]) {
    for sql in statements {
        <SqliteBackend as WriteBackend>::execute_with_binds(db.pool(), sql, &[])
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn nested_compact_matches_original_on_collation_and_affinity() {
    use ReadPredicateQuantifier::{All, Any};

    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    execute(
        &db,
        &[
            "CREATE TABLE closure_collation_nodes (id INTEGER PRIMARY KEY, parent_id INTEGER NULL, \
             name TEXT COLLATE NOCASE NULL, code TEXT NULL)",
            "INSERT INTO closure_collation_nodes VALUES \
             (1, NULL, 'ABC', '5'), (2, 1, 'x', '6'), (3, 2, 'y', '7')",
        ],
    )
    .await;
    let text = SqlText::default();
    let db = db.with_read_query_observer(text.clone());
    let column = |name, op, value| {
        ReadPredicate::column::<SqliteBackend, ClosureCollationNode>(name, op, value).unwrap()
    };
    let closure = |quantifier, target| {
        ReadPredicate::related_closure_within::<SqliteBackend, ClosureCollationNode>(
            quantifier,
            &[("parent_id", "id")],
            target,
            3,
        )
        .unwrap()
    };
    let name_is_abc = column("name", ReadPredicateOp::Eq, SqlValue::String("abc".into()));
    let name_is_other = column("name", ReadPredicateOp::Ne, SqlValue::String("abc".into()));
    let code_is_other = column("code", ReadPredicateOp::Ne, SqlValue::Int(5));
    let cases: [(&str, ReadPredicate, &[i64]); 4] = [
        (
            "single-level collation",
            closure(All, name_is_other.clone()),
            &[],
        ),
        (
            "nested collation, all",
            closure(All, closure(All, name_is_other)),
            &[],
        ),
        (
            "nested collation, any",
            closure(Any, closure(Any, name_is_abc)),
            &[1, 2, 3],
        ),
        (
            "nested text affinity",
            closure(All, closure(All, code_is_other)),
            &[],
        ),
    ];
    let read = |policy| {
        let db = &db;
        let text = &text;
        async move {
            let mut ids = EntityQuery::<ClosureCollationNode>::new()
                .with_read_visibility(&ReadVisibility::Complete(policy))
                .unwrap()
                .fetch_all(db)
                .await
                .unwrap()
                .into_iter()
                .map(|row| row.id)
                .collect::<Vec<_>>();
            ids.sort_unstable();
            (ids, text.0.lock().unwrap().clone())
        }
    };
    for (name, candidate, expected) in cases {
        // Composition uses the original renderer. The primary key is present
        // on every row, so this check leaves the closure's truth unchanged.
        let original = candidate
            .clone()
            .and(ReadPredicate::is_not_null::<SqliteBackend, ClosureCollationNode>("id").unwrap())
            .unwrap();
        let (original_ids, original_sql) = read(original).await;
        let (candidate_ids, candidate_sql) = read(candidate).await;
        assert_eq!(original_ids, expected, "{name}, original");
        assert_eq!(candidate_ids, expected, "{name}, candidate");
        // SQLite renders a nested closure exactly as the original renderer
        // does. A single-level closure keeps its compact form.
        assert_eq!(
            original_sql.contains(closure_sql(&candidate_sql)),
            name.starts_with("nested"),
            "{name}, rendering"
        );
    }
}

/// Two rows reached at the same depth carry the pointers `'a'` and `'A'`,
/// equal under the pointer column's NOCASE collation, with different target
/// truths. The step compares through the BINARY key, so each names a
/// different row, and only `'A'` leads to a TRUE row. The original walk keeps
/// both pointers because its recursive tuple includes the truth. A walk that
/// deduplicated on `(pointer, depth)` alone would follow only one of them.
#[tokio::test]
async fn nested_closure_follows_collation_equal_pointers_like_the_original() {
    use ReadPredicateQuantifier::{All, Any};

    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    execute(
        &db,
        &[
            "CREATE TABLE closure_fan_out_nodes (id INTEGER PRIMARY KEY, parent_id INTEGER NULL, \
             ptr TEXT COLLATE NOCASE NULL, key TEXT COLLATE BINARY NULL, score INTEGER NULL)",
            // Row 1 points at 'p'. Rows 2 and 3 share the key 'p' and point
            // at 'a' (score 0, FALSE) and 'A' (score NULL, UNKNOWN). Only the
            // row with key 'A' has score 1. Rows 6 to 10 mirror this with the
            // TRUE row under the lower-case key, so whichever of two
            // collation-equal pointers a deduplicating walk kept, one group
            // would lose its TRUE row.
            "INSERT INTO closure_fan_out_nodes VALUES \
             (1, NULL, 'p', 'g', 0), (2, NULL, 'a', 'p', 0), (3, NULL, 'A', 'p', NULL), \
             (4, NULL, NULL, 'a', 0), (5, NULL, NULL, 'A', 1), \
             (6, NULL, 'q', 'h', 0), (7, NULL, 'b', 'q', NULL), (8, NULL, 'B', 'q', 0), \
             (9, NULL, NULL, 'b', 1), (10, NULL, NULL, 'B', 0)",
        ],
    )
    .await;
    let text = SqlText::default();
    let db = db.with_read_query_observer(text.clone());
    let score = |op, value| {
        ReadPredicate::column::<SqliteBackend, ClosureFanOutNode>("score", op, SqlValue::Int(value))
            .unwrap()
    };
    let closure = |quantifier, keys: &[(&str, &str)], target| {
        ReadPredicate::related_closure_within::<SqliteBackend, ClosureFanOutNode>(
            quantifier, keys, target, 3,
        )
        .unwrap()
    };
    // No row has a parent, so the outer closure's verdict is the inner
    // closure's verdict for the governed row itself.
    let nested = |outer, inner, target| {
        closure(
            outer,
            &[("parent_id", "id")],
            closure(inner, &[("ptr", "key")], target),
        )
    };
    let cases: [(&str, ReadPredicate, &[i64]); 3] = [
        (
            "any reaches the case-sensitive key",
            nested(Any, Any, score(ReadPredicateOp::Eq, 1)),
            &[1, 3, 5, 6, 7, 9],
        ),
        (
            "all reaches the case-sensitive key",
            nested(All, All, score(ReadPredicateOp::Ne, 1)),
            &[2, 4, 8, 10],
        ),
        (
            "negated any",
            nested(Any, Any, score(ReadPredicateOp::Eq, 1))
                .not()
                .unwrap(),
            &[2, 4, 8, 10],
        ),
    ];
    let read = |policy| {
        let db = &db;
        let text = &text;
        async move {
            let mut ids = EntityQuery::<ClosureFanOutNode>::new()
                .with_read_visibility(&ReadVisibility::Complete(policy))
                .unwrap()
                .fetch_all(db)
                .await
                .unwrap()
                .into_iter()
                .map(|row| row.id)
                .collect::<Vec<_>>();
            ids.sort_unstable();
            (ids, text.0.lock().unwrap().clone())
        }
    };
    for (name, candidate, expected) in cases {
        let original = candidate
            .clone()
            .and(ReadPredicate::is_not_null::<SqliteBackend, ClosureFanOutNode>("id").unwrap())
            .unwrap();
        let (original_ids, original_sql) = read(original).await;
        let (candidate_ids, candidate_sql) = read(candidate).await;
        assert_eq!(original_ids, expected, "{name}, original");
        assert_eq!(candidate_ids, original_ids, "{name}, candidate");
        assert!(
            original_sql.contains(closure_sql(&candidate_sql)),
            "{name}, rendering"
        );
    }
}
