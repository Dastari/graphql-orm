#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
//! SQLite closures preserve the original verdict for collations, affinity
//! and fan-out with collation-equal pointers.
#[path = "support/closure_legacy.rs"]
mod closure_legacy;

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

async fn execute(db: &Database<SqliteBackend>, statements: &[&str]) {
    for sql in statements {
        <SqliteBackend as WriteBackend>::execute_with_binds(db.pool(), sql, &[])
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn closures_match_original_on_collation_and_affinity() {
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
    let closure = |quantifier, target| {
        ReadPredicate::related_closure_within::<SqliteBackend, ClosureCollationNode>(
            quantifier,
            &[("parent_id", "id")],
            target,
            3,
        )
        .unwrap()
    };
    let cases = |legacy| {
        let column = |name, op, comparison, value| {
            if legacy {
                closure_legacy::predicate::<SqliteBackend, ClosureCollationNode>(
                    name, comparison, value,
                )
            } else {
                ReadPredicate::column::<SqliteBackend, ClosureCollationNode>(name, op, value)
                    .unwrap()
            }
        };
        let name_is_abc = column(
            "name",
            ReadPredicateOp::Eq,
            "=",
            SqlValue::String("abc".into()),
        );
        let name_is_other = column(
            "name",
            ReadPredicateOp::Ne,
            "<>",
            SqlValue::String("abc".into()),
        );
        let code_is_other = column("code", ReadPredicateOp::Ne, "<>", SqlValue::Int(5));
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
        cases
    };
    let read = |policy| {
        let db = &db;
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
            ids
        }
    };
    for ((name, candidate, expected), (_, original, _)) in cases(false).into_iter().zip(cases(true))
    {
        let original_ids = read(original).await;
        let candidate_ids = read(candidate).await;
        assert_eq!(original_ids, expected, "{name}, original");
        assert_eq!(candidate_ids, original_ids, "{name}, candidate");
    }
}

/// Two rows reached at the same depth carry the pointers `'a'` and `'A'`,
/// equal under the pointer column's NOCASE collation, with different target
/// truths. The step compares through the BINARY key, so each names a
/// different row, and only `'A'` leads to a TRUE row. The original walk keeps
/// both pointers because its recursive tuple includes the truth. A walk that
/// deduplicated on `(pointer, depth)` alone followed only one of them: the
/// single-level and nested `Any` cases below then denied row 6.
#[tokio::test]
async fn closures_follow_collation_equal_pointers_like_the_original() {
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
    let fan_out = |quantifier, target| closure(quantifier, &[("ptr", "key")], target);
    let cases = |legacy| {
        let score = |op, comparison, value| {
            if legacy {
                closure_legacy::predicate::<SqliteBackend, ClosureFanOutNode>(
                    "score",
                    comparison,
                    SqlValue::Int(value),
                )
            } else {
                ReadPredicate::column::<SqliteBackend, ClosureFanOutNode>(
                    "score",
                    op,
                    SqlValue::Int(value),
                )
                .unwrap()
            }
        };
        let cases: [(&str, ReadPredicate, &[i64]); 6] = [
            (
                "single-level any reaches the case-sensitive key",
                fan_out(Any, score(ReadPredicateOp::Eq, "=", 1)),
                &[1, 3, 5, 6, 7, 9],
            ),
            // Row 6 reaches the UNKNOWN row 7 and, through 'b', the FALSE row
            // 9, so it is denied; rows 2, 4, 8 and 10 reach only TRUE rows.
            (
                "single-level all reaches the case-sensitive key",
                fan_out(All, score(ReadPredicateOp::Ne, "<>", 1)),
                &[2, 4, 8, 10],
            ),
            (
                "single-level negated any",
                fan_out(Any, score(ReadPredicateOp::Eq, "=", 1))
                    .not()
                    .unwrap(),
                &[2, 4, 8, 10],
            ),
            (
                "nested any reaches the case-sensitive key",
                nested(Any, Any, score(ReadPredicateOp::Eq, "=", 1)),
                &[1, 3, 5, 6, 7, 9],
            ),
            (
                "nested all reaches the case-sensitive key",
                nested(All, All, score(ReadPredicateOp::Ne, "<>", 1)),
                &[2, 4, 8, 10],
            ),
            (
                "nested negated any",
                nested(Any, Any, score(ReadPredicateOp::Eq, "=", 1))
                    .not()
                    .unwrap(),
                &[2, 4, 8, 10],
            ),
        ];
        cases
    };
    let read = |policy| {
        let db = &db;
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
            ids
        }
    };
    for ((name, candidate, expected), (_, original, _)) in cases(false).into_iter().zip(cases(true))
    {
        let original_ids = read(original).await;
        let candidate_ids = read(candidate).await;
        assert_eq!(original_ids, expected, "{name}, original");
        assert_eq!(candidate_ids, original_ids, "{name}, candidate");
    }
}

/// Generated tables over the same columns: pointers and keys drawn from
/// collation-equal letters, NULL pointers, fan-out keys and parent chains.
/// Compare both nesting orders against the original renderer.
#[tokio::test]
async fn generated_collation_fan_outs_match_the_original_renderer() {
    use ReadPredicateQuantifier::{All, Any};

    struct Generator(u64);
    impl Generator {
        fn below(&mut self, bound: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) % bound
        }
    }
    let letters = ["a", "A", "b", "B", "p"];
    let mut random = Generator(785);
    let mut admitting = 0;
    for _ in 0..150 {
        let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
            .await
            .unwrap();
        let rows = 4 + random.below(8);
        let values = (1..=rows)
            .map(|id| {
                let parent = match random.below(3) {
                    0 => "NULL".to_owned(),
                    _ => (1 + random.below(rows)).to_string(),
                };
                let pointer = match random.below(6) {
                    5 => "NULL".to_owned(),
                    index => format!("'{}'", letters[index as usize]),
                };
                let key = letters[random.below(5) as usize];
                let score = match random.below(3) {
                    0 => "NULL".to_owned(),
                    score => (score - 1).to_string(),
                };
                format!("({id}, {parent}, {pointer}, '{key}', {score})")
            })
            .collect::<Vec<_>>()
            .join(", ");
        execute(
            &db,
            &[
                "CREATE TABLE closure_fan_out_nodes (id INTEGER PRIMARY KEY, parent_id INTEGER NULL, \
                 ptr TEXT COLLATE NOCASE NULL, key TEXT COLLATE BINARY NULL, score INTEGER NULL)",
                &format!("INSERT INTO closure_fan_out_nodes VALUES {values}"),
            ],
        )
        .await;
        let text = SqlText::default();
        let db = db.with_read_query_observer(text.clone());
        let depth = 1 + random.below(3) as u32;
        let closure = |quantifier, keys: &[(&str, &str)], target| {
            ReadPredicate::related_closure_within::<SqliteBackend, ClosureFanOutNode>(
                quantifier, keys, target, depth,
            )
            .unwrap()
        };
        for quantifier in [Any, All] {
            for op in [ReadPredicateOp::Eq, ReadPredicateOp::Ne] {
                let target = ReadPredicate::column::<SqliteBackend, ClosureFanOutNode>(
                    "score",
                    op,
                    SqlValue::Int(1),
                )
                .unwrap();
                let original = closure_legacy::predicate::<SqliteBackend, ClosureFanOutNode>(
                    "score",
                    if op == ReadPredicateOp::Eq { "=" } else { "<>" },
                    SqlValue::Int(1),
                );
                let parent = &[("parent_id", "id")];
                let text_keys = &[("ptr", "key")];
                for (outer, inner) in [(parent, text_keys), (text_keys, parent)] {
                    let nested =
                        |target| closure(quantifier, outer, closure(quantifier, inner, target));
                    let original = nested(original.clone());
                    let candidate = nested(target.clone());
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
                    let (original_ids, original_sql) = read(original.clone()).await;
                    let (candidate_ids, candidate_sql) = read(candidate.clone()).await;
                    assert_eq!(candidate_ids, original_ids, "{values}");
                    assert!(candidate_sql.contains("__gom_closure_2_i"), "{values}");
                    assert!(!original_sql.contains("__gom_closure_2_i"), "{values}");
                    let (original_false, _) = read(original.not().unwrap()).await;
                    let (candidate_false, _) = read(candidate.not().unwrap()).await;
                    assert_eq!(candidate_false, original_false, "{values}");
                    admitting += usize::from(!original_ids.is_empty());
                }
            }
        }
    }
    // The generated cases admit rows, so the comparison is not vacuous.
    assert!(admitting > 300);
}
