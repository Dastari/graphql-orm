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
}
#[cfg(feature = "sqlite")]
schema_roots! { query_custom_ops: [], entities: [ClosureParityNode] }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [ClosureParityNode] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [ClosureParityNode] }

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
    execute(
        "CREATE TABLE closure_parity_nodes (id BIGINT PRIMARY KEY, parent_id BIGINT NULL, \
         score BIGINT NULL, key_a BIGINT NULL, key_b BIGINT NULL, ptr_a BIGINT NULL, \
         ptr_b BIGINT NULL)"
            .to_owned(),
    )
    .await;
    let literal = |value: Option<i64>| value.map_or("NULL".to_owned(), |value| value.to_string());
    let values = rows()
        .iter()
        .map(|row| {
            format!(
                "({}, {}, {}, {}, {}, {}, {})",
                row.id,
                literal(row.parent_id),
                literal(row.score),
                literal(row.key_a),
                literal(row.key_b),
                literal(row.ptr_a),
                literal(row.ptr_b)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    execute(format!(
        "INSERT INTO closure_parity_nodes (id, parent_id, score, key_a, key_b, ptr_a, ptr_b) VALUES {values}"
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
