#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
//! Nested closure parity for SQLite column collation and affinity.
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
schema_roots! { query_custom_ops: [], entities: [ClosureCollationNode] }

#[tokio::test]
async fn nested_compact_matches_original_on_collation_and_affinity() {
    use ReadPredicateQuantifier::{All, Any};

    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    for sql in [
        "CREATE TABLE closure_collation_nodes (id INTEGER PRIMARY KEY, parent_id INTEGER NULL, \
         name TEXT COLLATE NOCASE NULL, code TEXT NULL)",
        "INSERT INTO closure_collation_nodes VALUES \
         (1, NULL, 'ABC', '5'), (2, 1, 'x', '6'), (3, 2, 'y', '7')",
    ] {
        <SqliteBackend as WriteBackend>::execute_with_binds(db.pool(), sql, &[])
            .await
            .unwrap();
    }
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
    for (name, candidate, expected) in cases {
        // Composition uses the original renderer. The primary key is present
        // on every row, so this check leaves the closure's truth unchanged.
        let original = candidate
            .clone()
            .and(ReadPredicate::is_not_null::<SqliteBackend, ClosureCollationNode>("id").unwrap())
            .unwrap();
        assert_eq!(read(original).await, expected, "{name}, original");
        assert_eq!(read(candidate).await, expected, "{name}, compact");
    }
}
