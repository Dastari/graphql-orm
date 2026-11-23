// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
//! Closure parity across backends. SQLite and PostgreSQL render the closure
//! as a recursive CTE; SQL Server unrolls it into a breadth-first search.
//! Each backend evaluates the same generated truth-table cases over the same
//! generated rows, and every admitted set must equal the one a reference
//! evaluator of the documented rules computes. So the backends admit
//! identical sets. The rows cover cycles, self-loops, chains longer than the
//! bound, unreachable and partially NULL pointers, roots, UNKNOWN targets and
//! non-unique (fan-out) composite keys. The cases cover both quantifiers,
//! `not()`, several bounds and nested closures.
//!
//! The run prints a fingerprint of every admitted set, which is the same on
//! every backend.

#[cfg(feature = "mssql")]
#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(table = "closure_parity_nodes", plural = "ClosureParityNodes")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "closure_parity_nodes",
        plural = "ClosureParityNodes"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "closure_parity_nodes",
        plural = "ClosureParityNodes"
    )
)]
struct ClosureParityNode {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: i64,
    parent_id: Option<i64>,
    score: Option<i64>,
    key_a: Option<i64>,
    key_b: Option<i64>,
    ptr_a: Option<i64>,
    ptr_b: Option<i64>,
    name: Option<String>,
    label: Option<String>,
}

/// Text pointers and keys under declared collations. `ptr`/`tag` pair a
/// case-insensitive pointer with a case-sensitive key; `ptr_ci`/`tag_ci`
/// are both case-insensitive.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(table = "closure_text_nodes", plural = "ClosureTextNodes")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "closure_text_nodes",
        plural = "ClosureTextNodes"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "closure_text_nodes",
        plural = "ClosureTextNodes"
    )
)]
struct ClosureTextNode {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: i64,
    parent_id: Option<i64>,
    ptr: Option<String>,
    tag: Option<String>,
    ptr_ci: Option<String>,
    tag_ci: Option<String>,
    score: Option<i64>,
}
#[cfg(feature = "sqlite")]
schema_roots! { query_custom_ops: [], entities: [ClosureParityNode, ClosureTextNode] }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [ClosureParityNode, ClosureTextNode] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [ClosureParityNode, ClosureTextNode] }

type B = DefaultBackend;
type Node = ClosureParityNode;

/// The key relations walked: a unique parent chain, a composite key that is
/// not unique, and a single non-unique key.
#[derive(Clone, Copy, Debug)]
enum Keys {
    Parent,
    Composite,
    FanOut,
}
impl Keys {
    fn columns(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Parent => &[("parent_id", "id")],
            Self::Composite => &[("ptr_a", "key_a"), ("ptr_b", "key_b")],
            Self::FanOut => &[("ptr_a", "key_a")],
        }
    }
    fn pointer(self, row: &Node) -> Vec<Option<i64>> {
        match self {
            Self::Parent => vec![row.parent_id],
            Self::Composite => vec![row.ptr_a, row.ptr_b],
            Self::FanOut => vec![row.ptr_a],
        }
    }
    fn key(self, row: &Node) -> Vec<Option<i64>> {
        match self {
            Self::Parent => vec![Some(row.id)],
            Self::Composite => vec![row.key_a, row.key_b],
            Self::FanOut => vec![row.key_a],
        }
    }
}

#[derive(Clone, Debug)]
enum Target {
    AtLeast(i64),
    Below(i64),
    ScoreIsNull,
    Not(Box<Target>),
    Or(Box<Target>, Box<Target>),
    Closure(ReadPredicateQuantifier, Keys, Box<Target>, u32),
}
use Target::{AtLeast, Below, Closure, Not, Or, ScoreIsNull};

impl Target {
    fn predicate(&self) -> ReadPredicate {
        let score = |op, value| {
            ReadPredicate::column::<B, Node>("score", op, SqlValue::Int(value)).unwrap()
        };
        match self {
            AtLeast(value) => score(ReadPredicateOp::Gte, *value),
            Below(value) => score(ReadPredicateOp::Lt, *value),
            ScoreIsNull => ReadPredicate::is_null::<B, Node>("score").unwrap(),
            Not(target) => target.predicate().not().unwrap(),
            Or(left, right) => left.predicate().or(right.predicate()).unwrap(),
            Closure(quantifier, keys, target, depth) => {
                ReadPredicate::related_closure_within::<B, Node>(
                    *quantifier,
                    keys.columns(),
                    target.predicate(),
                    *depth,
                )
                .unwrap()
            }
        }
    }

    /// The reference evaluator, per row: Kleene logic, `None` is UNKNOWN.
    fn truths(&self, rows: &[Node]) -> Vec<Option<bool>> {
        let each = |test: &dyn Fn(&Node) -> Option<bool>| rows.iter().map(test).collect();
        match self {
            AtLeast(value) => each(&|row| row.score.map(|score| score >= *value)),
            Below(value) => each(&|row| row.score.map(|score| score < *value)),
            ScoreIsNull => each(&|row| Some(row.score.is_none())),
            Not(target) => target
                .truths(rows)
                .into_iter()
                .map(|truth| truth.map(|truth| !truth))
                .collect(),
            Or(left, right) => left
                .truths(rows)
                .into_iter()
                .zip(right.truths(rows))
                .map(|pair| match pair {
                    (Some(true), _) | (_, Some(true)) => Some(true),
                    (Some(false), Some(false)) => Some(false),
                    _ => None,
                })
                .collect(),
            Closure(quantifier, keys, target, depth) => {
                let truths = target.truths(rows);
                (0..rows.len())
                    .map(|row| closure(rows, &truths, row, *quantifier, *keys, *depth))
                    .collect()
            }
        }
    }
}

/// The documented walk, level by level: the row itself at depth 0, then every
/// row whose key equals a pointer reached at the previous depth. `truths` is
/// the target's truth for each row.
fn closure(
    rows: &[Node],
    truths: &[Option<bool>],
    row: usize,
    quantifier: ReadPredicateQuantifier,
    keys: Keys,
    max_depth: u32,
) -> Option<bool> {
    use std::collections::{BTreeSet, HashMap};
    let complete = |pointer: &[Option<i64>]| pointer.iter().all(Option::is_some);
    // Rows by key; a key with a NULL column is never equal to a pointer.
    let mut named = HashMap::<Vec<Option<i64>>, Vec<usize>>::new();
    for (index, candidate) in rows.iter().enumerate() {
        named.entry(keys.key(candidate)).or_default().push(index);
    }
    let names = |pointer: &[Option<i64>]| {
        if complete(pointer) {
            named.get(pointer).cloned().unwrap_or_default()
        } else {
            Vec::new()
        }
    };
    // (pointer, truth) per depth, deduplicated as the CTE's UNION does.
    let mut levels = vec![BTreeSet::from([(keys.pointer(&rows[row]), truths[row])])];
    for _ in 1..=max_depth {
        let next = levels
            .last()
            .unwrap()
            .iter()
            .flat_map(|(pointer, _)| names(pointer))
            .map(|reached| (keys.pointer(&rows[reached]), truths[reached]))
            .collect::<BTreeSet<_>>();
        levels.push(next);
    }
    let below_bound = levels[..max_depth as usize]
        .iter()
        .flatten()
        .map(|(pointer, _)| pointer.clone())
        .collect::<BTreeSet<_>>();
    let incomplete = levels.iter().enumerate().any(|(depth, level)| {
        level.iter().any(|(pointer, _)| {
            let ambiguous = !complete(pointer) && pointer.iter().any(Option::is_some);
            let unreachable = complete(pointer) && names(pointer).is_empty();
            let cut =
                complete(pointer) && depth == max_depth as usize && !below_bound.contains(pointer);
            ambiguous || unreachable || cut
        })
    });
    let reached = levels
        .iter()
        .flatten()
        .map(|(_, truth)| *truth)
        .collect::<BTreeSet<_>>();
    match quantifier {
        ReadPredicateQuantifier::Any if reached.contains(&Some(true)) => Some(true),
        ReadPredicateQuantifier::All if reached.contains(&Some(false)) => Some(false),
        _ if incomplete || reached.contains(&None) => None,
        ReadPredicateQuantifier::Any => Some(false),
        ReadPredicateQuantifier::All => Some(true),
    }
}

/// A small deterministic generator (64-bit LCG), so every backend gets the
/// same rows.
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

/// 60 random rows, a 15-row chain (61 is its root) and a six-cycle (80-85).
fn rows() -> Vec<Node> {
    let mut random = Generator(779);
    let mut rows = Vec::new();
    let node = |id, parent_id, score, key: (i64, i64), pointer: (Option<i64>, Option<i64>)| Node {
        id,
        parent_id,
        score,
        key_a: Some(key.0),
        key_b: Some(key.1),
        ptr_a: pointer.0,
        ptr_b: pointer.1,
        name: (id % 3 != 0).then(|| if id % 2 == 0 { "abc" } else { "ABC" }.to_owned()),
        label: (id % 5 != 0).then(|| "abc ".to_owned()),
    };
    for id in 1..=85_i64 {
        let parent = match id {
            61 => None,
            62..=75 => Some(id - 1),
            76..=79 => Some(id - 15),
            80 => Some(85),
            81..=85 => Some(id - 1),
            _ => match random.below(100) {
                0..=11 => None,
                12..=17 => Some(id),
                18..=23 => Some(1000 + id),
                _ => Some(1 + random.below(60) as i64),
            },
        };
        let score = match random.below(10) {
            0 | 1 => None,
            2 | 3 => Some(1),
            _ => Some(5),
        };
        let key = (1 + random.below(5) as i64, 1 + random.below(3) as i64);
        let component = |random: &mut Generator| Some(1 + random.below(5) as i64);
        let pointer = match random.below(100) {
            0..=11 => (None, None),
            12..=15 => (component(&mut random), None),
            16..=19 => (None, component(&mut random)),
            20..=25 => (Some(99), Some(99)),
            _ => (component(&mut random), Some(1 + random.below(3) as i64)),
        };
        rows.push(node(id, parent, score, key, pointer));
    }
    rows
}

fn cases() -> Vec<(String, Target)> {
    use ReadPredicateQuantifier::{All, Any};
    let targets = [
        ("p", AtLeast(5)),
        ("fails", Below(5)),
        ("not(p)", Not(Box::new(AtLeast(5)))),
        ("p or null", Or(Box::new(AtLeast(5)), Box::new(ScoreIsNull))),
    ];
    let mut cases = Vec::new();
    for keys in [Keys::Parent, Keys::Composite, Keys::FanOut] {
        for quantifier in [Any, All] {
            // The default bound on the unique chain; shorter bounds, which
            // the fan-out keys reach in full, elsewhere.
            let depths = match keys {
                Keys::Parent => [1, 2, 3, 6, 64],
                Keys::Composite | Keys::FanOut => [1, 2, 3, 6, 12],
            };
            for (name, target) in &targets {
                for depth in depths {
                    let closure = Closure(quantifier, keys, Box::new(target.clone()), depth);
                    let label = format!("{keys:?} {quantifier:?}({name}) depth {depth}");
                    cases.push((format!("not {label}"), Not(Box::new(closure.clone()))));
                    cases.push((label, closure));
                }
            }
        }
    }
    let nested = [
        Closure(
            Any,
            Keys::Parent,
            Box::new(Closure(All, Keys::Parent, Box::new(AtLeast(5)), 3)),
            3,
        ),
        Closure(
            All,
            Keys::Composite,
            Box::new(Closure(Any, Keys::Parent, Box::new(Below(5)), 2)),
            2,
        ),
        Closure(
            All,
            Keys::FanOut,
            Box::new(Not(Box::new(Closure(
                Any,
                Keys::Composite,
                Box::new(ScoreIsNull),
                4,
            )))),
            6,
        ),
        Closure(
            Any,
            Keys::Parent,
            Box::new(Or(
                Box::new(Below(5)),
                Box::new(Closure(All, Keys::FanOut, Box::new(AtLeast(5)), 3)),
            )),
            64,
        ),
    ];
    for (index, closure) in nested.into_iter().enumerate() {
        cases.push((format!("nested {index}"), closure.clone()));
        cases.push((format!("not nested {index}"), Not(Box::new(closure))));
    }
    cases
}

#[cfg(feature = "sqlite")]
struct FixtureOwner;
#[cfg(feature = "sqlite")]
impl FixtureOwner {
    fn cleanup(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }
}
#[cfg(feature = "postgres")]
type FixtureOwner = owned_postgres::OwnedPostgres;
#[cfg(feature = "mssql")]
type FixtureOwner = owned_mssql::OwnedSqlServer;

async fn fixture() -> (Database, FixtureOwner) {
    #[cfg(feature = "sqlite")]
    let (db, owner) = (
        Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
            .await
            .unwrap(),
        FixtureOwner,
    );
    #[cfg(feature = "postgres")]
    let (db, owner) = {
        let owner = FixtureOwner::start("read-predicate-closure-parity").unwrap();
        let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .after_connect(|conn, _| {
                Box::pin(async move {
                    graphql_orm::sqlx::query("SET statement_timeout = '60s'")
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&owner.url)
            .await
            .unwrap();
        (Database::new(pool), owner)
    };
    #[cfg(feature = "mssql")]
    let (db, owner) = {
        let owner = FixtureOwner::start().await.unwrap();
        let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable_with(
            &owner.connection_string,
            graphql_orm::db::mssql::MssqlPoolOptions::default()
                .query_timeout(Some(std::time::Duration::from_secs(60))),
        )
        .await
        .unwrap();
        let db = Database::builder(pool)
            .schema_policy(SchemaPolicy::ExternalWritable)
            .build();
        (db, owner)
    };
    let execute = |sql: String| {
        let db = db.clone();
        async move {
            <B as WriteBackend>::execute_with_binds(db.pool(), &sql, &[])
                .await
                .unwrap();
        }
    };
    #[cfg(feature = "sqlite")]
    let text_columns = "name TEXT COLLATE NOCASE NULL, label TEXT COLLATE RTRIM NULL";
    #[cfg(feature = "postgres")]
    let text_columns = r#"name TEXT COLLATE "C" NULL, label TEXT COLLATE "POSIX" NULL"#;
    #[cfg(feature = "mssql")]
    let text_columns = "name NVARCHAR(16) COLLATE Latin1_General_100_CI_AS NULL, \
                        label NVARCHAR(16) COLLATE Latin1_General_100_CS_AS NULL";
    execute(format!(
        "CREATE TABLE closure_parity_nodes (id BIGINT PRIMARY KEY, parent_id BIGINT NULL, \
         score BIGINT NULL, key_a BIGINT NULL, key_b BIGINT NULL, ptr_a BIGINT NULL, \
         ptr_b BIGINT NULL, {text_columns})"
    ))
    .await;
    let literal = |value: Option<i64>| value.map_or("NULL".to_owned(), |value| value.to_string());
    // The generated fixture strings contain only ASCII letters and spaces.
    let text_literal =
        |value: Option<&str>| value.map_or_else(|| "NULL".to_owned(), |value| format!("'{value}'"));
    let values = rows()
        .iter()
        .map(|row| {
            format!(
                "({}, {}, {}, {}, {}, {}, {}, {}, {})",
                row.id,
                literal(row.parent_id),
                literal(row.score),
                literal(row.key_a),
                literal(row.key_b),
                literal(row.ptr_a),
                literal(row.ptr_b),
                text_literal(row.name.as_deref()),
                text_literal(row.label.as_deref()),
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    execute(format!(
        "INSERT INTO closure_parity_nodes (id, parent_id, score, key_a, key_b, ptr_a, ptr_b, name, label) VALUES {values}"
    ))
    .await;
    (db, owner)
}

#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "owned database lane"
)]
async fn every_backend_admits_the_reference_sets() {
    let (db, mut owner) = fixture().await;
    let rows = rows();
    let mut mismatches = Vec::new();
    let mut fingerprint = 0xcbf2_9ce4_8422_2325_u64;
    let cases = cases();
    for (name, target) in &cases {
        let expected = rows
            .iter()
            .zip(target.truths(&rows))
            .filter(|(_, truth)| *truth == Some(true))
            .map(|(row, _)| row.id)
            .collect::<Vec<_>>();
        let mut admitted = EntityQuery::<Node>::new()
            .with_read_visibility(&ReadVisibility::Complete(target.predicate()))
            .unwrap()
            .fetch_all(&db)
            .await
            .unwrap_or_else(|error| panic!("{name}: {error}"))
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>();
        admitted.sort();
        for id in admitted.iter().chain([&-1]) {
            fingerprint = (fingerprint ^ *id as u64).wrapping_mul(0x0100_0000_01b3);
        }
        if admitted != expected {
            mismatches.push(format!(
                "{name}: admitted {admitted:?}, expected {expected:?}"
            ));
        }
    }
    eprintln!(
        "closure parity on {:?}: {} cases, fingerprint {fingerprint:016x}",
        B::DIALECT,
        cases.len()
    );
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    // The cases are not vacuous: they admit and refuse rows in both forms.
    let admits = |target: &Target| target.truths(&rows).contains(&Some(true));
    assert!(cases.iter().filter(|(_, target)| admits(target)).count() > cases.len() / 2);
    owner.cleanup().unwrap();
}

/// The first SQLite closure subquery in `sql`, through its closing
/// parenthesis. Read statements bind every value, so the SQL has no string
/// literals.
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

/// SQLite renders every closure exactly as the original renderer does.
/// PostgreSQL and SQL Server take the compact rendering.
fn assert_nested_rendering(name: &str, original: &str, candidate: &str) {
    if B::DIALECT == DatabaseBackend::Sqlite {
        assert!(
            original.contains(closure_sql(candidate)),
            "{name}: SQLite keeps the original nested rendering"
        );
    } else {
        assert!(
            candidate.len() + 64 < original.len(),
            "{name}: compact rendering"
        );
    }
}

/// The last read statement.
#[derive(Clone, Default)]
struct SqlText(std::sync::Arc<std::sync::Mutex<String>>);
impl graphql_orm::graphql::orm::ReadQueryObserver for SqlText {
    fn on_read(&self, sql: &str, _: usize) {
        *self.0.lock().unwrap() = sql.to_owned();
    }
}

#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "owned database lane"
)]
async fn directly_nested_closures_agree_with_original_sql() {
    use ReadPredicateQuantifier::{All, Any};
    use std::time::Instant;

    let (db, mut owner) = fixture().await;
    let text = SqlText::default();
    let db = db.with_read_query_observer(text.clone());
    let query = |policy| {
        EntityQuery::<Node>::new()
            .with_read_visibility(&ReadVisibility::Complete(policy))
            .unwrap()
    };
    let read = |query: EntityQuery<Node>| {
        let db = &db;
        let text = &text;
        async move {
            let start = Instant::now();
            let rows = query.fetch_all(db).await.unwrap();
            let elapsed = start.elapsed();
            let sql = text.0.lock().unwrap().clone();
            let mut ids = rows.into_iter().map(|row| row.id).collect::<Vec<_>>();
            ids.sort_unstable();
            (ids, sql, elapsed)
        }
    };

    let text_eq = |column, value: &str| {
        ReadPredicate::column::<B, Node>(
            column,
            ReadPredicateOp::Eq,
            SqlValue::String(value.to_owned()),
        )
        .unwrap()
    };
    let score =
        ReadPredicate::column::<B, Node>("score", ReadPredicateOp::Eq, SqlValue::Float(5.0))
            .unwrap();
    let null = ReadPredicate::is_null::<B, Node>("name").unwrap();
    let score_is =
        |op, value| ReadPredicate::column::<B, Node>("score", op, SqlValue::Int(value)).unwrap();
    let nested = |outer, inner, target, outer_depth, inner_depth| {
        let closure = |quantifier, target, depth| {
            ReadPredicate::related_closure_within::<B, Node>(
                quantifier,
                Keys::Parent.columns(),
                target,
                depth,
            )
            .unwrap()
        };
        closure(outer, closure(inner, target, inner_depth), outer_depth)
    };
    let cases = [
        (
            "case folding",
            nested(All, All, text_eq("name", "abc"), 1, 1),
        ),
        (
            "trailing space",
            nested(All, All, text_eq("label", "abc"), 1, 1),
        ),
        (
            "case-sensitive label",
            nested(All, All, text_eq("label", "ABC "), 1, 1),
        ),
        (
            "float bind for integer column",
            nested(Any, Any, score.clone(), 1, 3),
        ),
        (
            "scalar negation",
            nested(All, Any, text_eq("name", "abc").not().unwrap(), 6, 1),
        ),
        ("null test", nested(Any, All, null.clone(), 1, 6)),
        (
            "disjunction",
            nested(
                Any,
                Any,
                text_eq("name", "abc").or(null.clone()).unwrap(),
                6,
                1,
            ),
        ),
        (
            "conjunction",
            nested(
                All,
                All,
                score
                    .and(ReadPredicate::is_not_null::<B, Node>("name").unwrap())
                    .unwrap(),
                6,
                6,
            ),
        ),
        // Two distinct binds in the nested leaf. Exchanging them would
        // admit the score-5 rows instead of the score-1 rows.
        (
            "two bound values",
            nested(
                Any,
                All,
                score_is(ReadPredicateOp::Gte, 1)
                    .and(score_is(ReadPredicateOp::Ne, 5))
                    .unwrap(),
                3,
                3,
            ),
        ),
        (
            "text and integer binds",
            nested(
                All,
                Any,
                text_eq("name", "abc")
                    .and(score_is(ReadPredicateOp::Lt, 5))
                    .unwrap(),
                6,
                3,
            ),
        ),
        (
            "complete cycle",
            nested(
                All,
                All,
                ReadPredicate::is_not_null::<B, Node>("id").unwrap(),
                6,
                6,
            ),
        ),
    ];
    let primary_key_present = ReadPredicate::is_not_null::<B, Node>("id").unwrap();
    for (name, candidate) in cases {
        let original = candidate.clone().and(primary_key_present.clone()).unwrap();
        let (before, before_sql, _) = read(query(original)).await;
        let (after, after_sql, _) = read(query(candidate.clone())).await;
        let (double_negated, _, _) = read(query(candidate.not().unwrap().not().unwrap())).await;
        assert_eq!(before, after, "{name}");
        assert_eq!(before, double_negated, "double negation of {name}");
        if name == "scalar negation" && B::DIALECT == DatabaseBackend::Postgres {
            // PostgreSQL uses the original renderer for explicit negation.
            assert!(before_sql.len().abs_diff(after_sql.len()) < 64, "{name}");
        } else {
            assert_nested_rendering(name, &before_sql, &after_sql);
        }
        // Row 61 has no parent, name 'ABC', and label 'abc '. Check an
        // observable difference between the declared collations.
        match name {
            "case folding" | "trailing space" => {
                assert_eq!(after.contains(&61), !cfg!(feature = "postgres"), "{name}");
            }
            "case-sensitive label" => assert!(!after.contains(&61), "{name}"),
            "complete cycle" => assert!((80..=85).all(|id| after.contains(&id)), "{name}"),
            "two bound values" => {
                // Scores are NULL, 1 or 5, so the leaf is `score < 5` and the
                // exchanged binds would give `score >= 5`.
                let reference = |leaf: Target| {
                    let target = Closure(
                        Any,
                        Keys::Parent,
                        Box::new(Closure(All, Keys::Parent, Box::new(leaf), 3)),
                        3,
                    );
                    let rows = rows();
                    rows.iter()
                        .zip(target.truths(&rows))
                        .filter(|(_, truth)| *truth == Some(true))
                        .map(|(row, _)| row.id)
                        .collect::<Vec<_>>()
                };
                assert_eq!(after, reference(Below(5)), "{name}");
                assert_ne!(after, reference(AtLeast(5)), "{name}");
            }
            _ => {}
        }
    }
    if B::DIALECT == DatabaseBackend::Postgres {
        let initial =
            ReadPredicate::column::<B, Node>("id", ReadPredicateOp::Ne, SqlValue::Int(61)).unwrap();
        let primary_key_missing = ReadPredicate::is_null::<B, Node>("id").unwrap();
        // Each alternation adds one level while preserving the target's
        // truth. At this boundary the extra compact scalar parentheses
        // exceed the budget and the original renderer still fits.
        let mut target = initial;
        for step in 0..248 {
            target = if step % 2 == 0 {
                target.and(primary_key_present.clone())
            } else {
                target.or(primary_key_missing.clone())
            }
            .unwrap();
        }
        let below_limit = nested(All, All, target.clone(), 1, 1);
        let fallback = nested(
            All,
            All,
            target.and(primary_key_present.clone()).unwrap(),
            1,
            1,
        );
        let original = fallback.clone().and(primary_key_present).unwrap();
        assert_eq!(query(below_limit.clone()).build_delete_sql().1.len(), 4);
        assert_eq!(
            query(fallback.clone()).build_delete_sql().1,
            vec![SqlValue::Int(61); 16]
        );
        let (before, _, _) = read(query(original)).await;
        let (after, _, _) = read(query(fallback)).await;
        let (below_limit, _, _) = read(query(below_limit)).await;
        assert_eq!(after, before, "compact cost fallback");
        assert_eq!(after, below_limit, "at the compact cost boundary");
        assert!(!after.is_empty() && !after.contains(&61));
    }

    for outer_keys in [Keys::Parent, Keys::Composite, Keys::FanOut] {
        for inner_keys in [Keys::Parent, Keys::Composite, Keys::FanOut] {
            for outer in [Any, All] {
                for inner in [Any, All] {
                    // Model three principals with different score thresholds.
                    for minimum in [1, 5, 6] {
                        let target = Closure(
                            outer,
                            outer_keys,
                            Box::new(Closure(inner, inner_keys, Box::new(AtLeast(minimum)), 3)),
                            3,
                        );
                        let candidate = target.predicate();
                        // AND selects the original composition renderer. The fixture's
                        // non-null primary key makes the additional check always true.
                        let original = candidate
                            .clone()
                            .and(ReadPredicate::is_not_null::<B, Node>("id").unwrap())
                            .unwrap();
                        let original = query(original);
                        let candidate = query(candidate);
                        let (before, before_sql, _) = read(original.clone()).await;
                        let (after, after_sql, _) = read(candidate.clone()).await;
                        assert_eq!(before, after, "{target:?}");
                        assert_nested_rendering(&format!("{target:?}"), &before_sql, &after_sql);
                        if matches!(
                            (outer_keys, inner_keys, outer, inner, minimum),
                            (Keys::Parent, Keys::Parent, Any, All, 5)
                        ) {
                            let mut before_times = Vec::new();
                            let mut after_times = Vec::new();
                            for _ in 0..3 {
                                before_times.push(read(original.clone()).await.2);
                                after_times.push(read(candidate.clone()).await.2);
                            }
                            before_times.sort_unstable();
                            after_times.sort_unstable();
                            eprintln!(
                                "{:?}: depth-3 any(all(score >= 5)), {} -> {} bytes, median {:?} -> {:?}",
                                B::DIALECT,
                                before_sql.len(),
                                after_sql.len(),
                                before_times[1],
                                after_times[1],
                            );
                        }
                    }
                }
            }
        }
    }

    owner.cleanup().unwrap();
}

/// Two rows reached at the same depth carry the pointers `'a'` and `'A'`
/// with different target truths. Under a case-insensitive pointer collation
/// they compare equal, so a walk that deduplicated them without their truth
/// could drop one before following it. `ptr`/`tag` pair a case-insensitive
/// pointer with a case-sensitive key, where each pointer names a different
/// row; `ptr_ci`/`tag_ci` are both case-insensitive, where each names both.
/// The original and candidate renderings must agree on every backend. The
/// SQL Server walk carries only pointers in both renderings, and PostgreSQL's
/// recursive tuple carries the truth in both.
#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "owned database lane"
)]
async fn collation_equal_text_pointers_agree_with_original_sql() {
    use ReadPredicateQuantifier::{All, Any};
    type Text = ClosureTextNode;

    let (db, mut owner) = fixture().await;
    let execute = |sql: &str| {
        let db = db.clone();
        let sql = sql.to_owned();
        async move {
            <B as WriteBackend>::execute_with_binds(db.pool(), &sql, &[])
                .await
                .unwrap();
        }
    };
    #[cfg(feature = "sqlite")]
    let columns = "ptr TEXT COLLATE NOCASE NULL, tag TEXT COLLATE BINARY NULL, \
                   ptr_ci TEXT COLLATE NOCASE NULL, tag_ci TEXT COLLATE NOCASE NULL";
    #[cfg(feature = "postgres")]
    let columns = {
        execute(
            "CREATE COLLATION closure_ci (provider = icu, \
             locale = 'und-u-ks-level2', deterministic = false)",
        )
        .await;
        r#"ptr TEXT COLLATE closure_ci NULL, tag TEXT COLLATE "C" NULL, ptr_ci TEXT COLLATE closure_ci NULL, tag_ci TEXT COLLATE closure_ci NULL"#
    };
    #[cfg(feature = "mssql")]
    let columns = "ptr NVARCHAR(8) COLLATE Latin1_General_100_CI_AS NULL, \
                   tag NVARCHAR(8) COLLATE Latin1_General_100_CS_AS NULL, \
                   ptr_ci NVARCHAR(8) COLLATE Latin1_General_100_CI_AS NULL, \
                   tag_ci NVARCHAR(8) COLLATE Latin1_General_100_CI_AS NULL";
    execute(&format!(
        "CREATE TABLE closure_text_nodes (id BIGINT PRIMARY KEY, parent_id BIGINT NULL, \
         {columns}, score BIGINT NULL)"
    ))
    .await;
    // Row 1 points at 'p'. Rows 2 and 3 share the key 'p' and point at 'a'
    // (score 0, FALSE) and 'A' (score NULL, UNKNOWN); only key 'A' has score
    // 1. Rows 6 to 10 mirror this with score 1 under the lower-case key, so
    // whichever collation-equal pointer a deduplicating walk kept, one group
    // would lose its TRUE row. No row has a parent.
    let rows: [(i64, Option<&str>, &str, Option<i64>); 10] = [
        (1, Some("p"), "g", Some(0)),
        (2, Some("a"), "p", Some(0)),
        (3, Some("A"), "p", None),
        (4, None, "a", Some(0)),
        (5, None, "A", Some(1)),
        (6, Some("q"), "h", Some(0)),
        (7, Some("b"), "q", None),
        (8, Some("B"), "q", Some(0)),
        (9, None, "b", Some(1)),
        (10, None, "B", Some(0)),
    ];
    let values = rows
        .iter()
        .map(|(id, pointer, key, score)| {
            let pointer = pointer.map_or_else(|| "NULL".to_owned(), |text| format!("'{text}'"));
            let score = score.map_or_else(|| "NULL".to_owned(), |score| score.to_string());
            format!("({id}, NULL, {pointer}, '{key}', {pointer}, '{key}', {score})")
        })
        .collect::<Vec<_>>()
        .join(", ");
    execute(&format!(
        "INSERT INTO closure_text_nodes (id, parent_id, ptr, tag, ptr_ci, tag_ci, score) \
         VALUES {values}"
    ))
    .await;

    let text = SqlText::default();
    let db = db.with_read_query_observer(text.clone());
    let read = |policy: ReadPredicate| {
        let db = &db;
        let text = &text;
        async move {
            let rows = EntityQuery::<Text>::new()
                .with_read_visibility(&ReadVisibility::Complete(policy))
                .unwrap()
                .fetch_all(db)
                .await;
            let sql = text.0.lock().unwrap().clone();
            let rows = rows.map(|rows| {
                let mut ids = rows.into_iter().map(|row| row.id).collect::<Vec<_>>();
                ids.sort_unstable();
                ids
            });
            (rows.map_err(|error| error.to_string()), sql)
        }
    };
    let score =
        |op, value| ReadPredicate::column::<B, Text>("score", op, SqlValue::Int(value)).unwrap();
    let nested = |outer, inner, keys: &[(&str, &str)], target| {
        let closure = |quantifier, keys: &[(&str, &str)], target| {
            ReadPredicate::related_closure_within::<B, Text>(quantifier, keys, target, 3).unwrap()
        };
        closure(outer, &[("parent_id", "id")], closure(inner, keys, target))
    };
    let primary_key_present = ReadPredicate::is_not_null::<B, Text>("id").unwrap();
    let mut outcomes = Vec::new();
    for (pair, keys) in [
        ("case-sensitive key", [("ptr", "tag")]),
        ("case-insensitive key", [("ptr_ci", "tag_ci")]),
    ] {
        let cases = [
            (
                "any",
                nested(Any, Any, &keys, score(ReadPredicateOp::Eq, 1)),
            ),
            (
                "all",
                nested(All, All, &keys, score(ReadPredicateOp::Ne, 1)),
            ),
            (
                "not any",
                nested(Any, Any, &keys, score(ReadPredicateOp::Eq, 1))
                    .not()
                    .unwrap(),
            ),
        ];
        for (quantifier, candidate) in cases {
            let name = format!("{pair}, {quantifier}");
            let original = candidate.clone().and(primary_key_present.clone()).unwrap();
            let (before, before_sql) = read(original).await;
            let (after, after_sql) = read(candidate).await;
            // A statement error is the same outcome for both renderings.
            assert_eq!(before.as_ref().ok(), after.as_ref().ok(), "{name}");
            assert_eq!(before.is_err(), after.is_err(), "{name}");
            // A negated closure keeps the original rendering.
            if after.is_ok() && quantifier != "not any" {
                assert_nested_rendering(&name, &before_sql, &after_sql);
            }
            outcomes.push((name, after.ok()));
        }
    }
    let admitted = |ids: &[i64]| Some(ids.to_vec());
    // PostgreSQL and SQL Server refuse to compare a pointer and key with
    // different collations, so both renderings fail the statement there.
    let case_sensitive = if B::DIALECT == DatabaseBackend::Sqlite {
        [
            admitted(&[1, 3, 5, 6, 7, 9]),
            admitted(&[2, 4, 8, 10]),
            admitted(&[2, 4, 8, 10]),
        ]
    } else {
        [None, None, None]
    };
    let case_insensitive = [
        admitted(&[1, 2, 3, 5, 6, 7, 8, 9]),
        admitted(&[4, 10]),
        admitted(&[4, 10]),
    ];
    let expected = case_sensitive
        .into_iter()
        .chain(case_insensitive)
        .collect::<Vec<_>>();
    assert_eq!(
        outcomes
            .into_iter()
            .map(|(_, outcome)| outcome)
            .collect::<Vec<_>>(),
        expected
    );
    owner.cleanup().unwrap();
}
