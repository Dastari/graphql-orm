//! SQLite closure rendering goldens from before ORM 0.200.0.
//!
//! `tests/goldens/closure_sqlite_pre_0_200_0.txt` was captured by running
//! this exact file against `a8b92c270^1`, the parent of the #841 merge, whose
//! SQLite closures use the original recursive walk. Each entry pins the full
//! statement text and the ordered bind values. Standalone closures still
//! reproduce these byte for byte. The nested examples retain the original SQL
//! for reference while the parity tests compare their verdicts.
//!
//! The file compiles unchanged against both commits. Set
//! `GRAPHQL_ORM_CAPTURE_GOLDENS=1` only when deliberately recapturing at the
//! pinned historical commit.
#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::prelude::*;
use std::fmt::Write as _;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "closure_golden_nodes", plural = "ClosureGoldenNodes")]
struct ClosureGoldenNode {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: i64,
    parent_id: Option<i64>,
    ptr: Option<String>,
    key: Option<String>,
    ptr_b: Option<i64>,
    key_b: Option<i64>,
    score: Option<i64>,
}
schema_roots! { query_custom_ops: [], entities: [ClosureGoldenNode] }

type B = SqliteBackend;
type Node = ClosureGoldenNode;

/// Each shape over each key relation: an integer parent chain, a text
/// pointer and key, and a composite text and integer key.
fn report() -> String {
    use ReadPredicateQuantifier::{All, Any};
    let score =
        |op, value| ReadPredicate::column::<B, Node>("score", op, SqlValue::Int(value)).unwrap();
    let score_is_null = || ReadPredicate::is_null::<B, Node>("score").unwrap();
    let key_sets: [(&str, &[(&str, &str)]); 3] = [
        ("parent", &[("parent_id", "id")]),
        ("text", &[("ptr", "key")]),
        ("composite", &[("ptr", "key"), ("ptr_b", "key_b")]),
    ];
    let mut report = String::new();
    for (key_name, keys) in key_sets {
        let closure = |quantifier, keys: &[(&str, &str)], target, depth| {
            ReadPredicate::related_closure_within::<B, Node>(quantifier, keys, target, depth)
                .unwrap()
        };
        let shapes = [
            (
                "any(score = 1)",
                closure(Any, keys, score(ReadPredicateOp::Eq, 1), 3),
            ),
            (
                "all(score >= 2 and score <> 5)",
                closure(
                    All,
                    keys,
                    score(ReadPredicateOp::Gte, 2)
                        .and(score(ReadPredicateOp::Ne, 5))
                        .unwrap(),
                    3,
                ),
            ),
            (
                "not(all(not(score = 1)))",
                closure(All, keys, score(ReadPredicateOp::Eq, 1).not().unwrap(), 3)
                    .not()
                    .unwrap(),
            ),
            (
                "any over parents(all(score < 3 or score is null))",
                closure(
                    Any,
                    &[("parent_id", "id")],
                    closure(
                        All,
                        keys,
                        score(ReadPredicateOp::Lt, 3).or(score_is_null()).unwrap(),
                        2,
                    ),
                    2,
                ),
            ),
            (
                "any(score = 1) and score = 9",
                closure(Any, keys, score(ReadPredicateOp::Eq, 1), 3)
                    .and(score(ReadPredicateOp::Eq, 9))
                    .unwrap(),
            ),
            (
                "all(score is null) or score = 7",
                closure(All, keys, score_is_null(), 3)
                    .or(score(ReadPredicateOp::Eq, 7))
                    .unwrap(),
            ),
        ];
        for (shape, policy) in shapes {
            let (sql, binds) = EntityQuery::<Node>::new()
                .with_read_visibility(&ReadVisibility::Complete(policy))
                .unwrap()
                .build_delete_sql();
            writeln!(report, "## {key_name}: {shape}\n{sql}\n{binds:?}\n").unwrap();
        }
    }
    report
}

#[test]
fn sqlite_standalone_closures_render_as_before_0_200_0() {
    let path = format!(
        "{}/tests/goldens/closure_sqlite_pre_0_200_0.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    let report = report();
    if std::env::var_os("GRAPHQL_ORM_CAPTURE_GOLDENS").is_some() {
        std::fs::write(&path, &report).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap();
    // Compare entry by entry so a failure names the shape.
    let entries = |text: &str| {
        text.split("\n\n")
            .filter(|entry| !entry.trim().is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let (actual, expected) = (entries(&report), entries(&expected));
    assert_eq!(actual.len(), expected.len());
    assert_eq!(actual.len(), 18);
    for (actual, expected) in actual.iter().zip(&expected) {
        let header = expected.lines().next().unwrap();
        assert_eq!(actual.lines().next(), Some(header));
        if header.contains("any over parents") {
            assert!(actual.contains("__gom_closure_2_i"), "{actual}");
        } else {
            assert_eq!(actual, expected);
        }
    }
}
