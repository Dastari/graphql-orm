// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
//! Transitive-closure read predicates (`related_closure`,
//! `related_closure_all`, `related_closure_within`): three-valued truth
//! tables, cycles, the depth bound, composite and fan-out keys, nesting, bind
//! order, refusals, pages and a performance sanity check.

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
    graphql_entity(
        table = "closure_nodes",
        plural = "ClosureNodes",
        keyset = "id asc",
        aggregate = true
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "closure_nodes",
        plural = "ClosureNodes",
        keyset = "id asc",
        aggregate = true
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "closure_nodes",
        plural = "ClosureNodes",
        keyset = "id asc",
        aggregate = true
    )
)]
struct ClosureNode {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: i64,
    parent_id: Option<i64>,
    #[filterable(type = "number")]
    score: Option<i64>,
    key_a: Option<i64>,
    key_b: Option<i64>,
    ptr_a: Option<i64>,
    ptr_b: Option<i64>,
    /// A column named like the SQL Server closure's summary columns, so a
    /// target over it must still bind to the row being authorized.
    #[graphql_orm(db_column = "__gom_max")]
    shadow: Option<i64>,
}
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(table = "closure_marks", plural = "ClosureMarks")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(backend = "postgres", table = "closure_marks", plural = "ClosureMarks")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "closure_marks",
        plural = "ClosureMarks"
    )
)]
struct ClosureMark {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: i64,
    node_id: i64,
    flag: Option<i64>,
}
#[cfg(feature = "sqlite")]
schema_roots! { query_custom_ops: [], entities: [ClosureNode, ClosureMark] }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [ClosureNode, ClosureMark] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [ClosureNode, ClosureMark] }

type B = DefaultBackend;
const PARENT: &[(&str, &str)] = &[("parent_id", "id")];
fn misconfigured(result: async_graphql::Result<ReadPredicate>) {
    let error = result.expect_err("the closure must be refused");
    assert_eq!(
        error
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get("code"))
            .map(ToString::to_string)
            .as_deref(),
        Some("\"AUTHORIZATION_MISCONFIGURED\""),
        "{error:?}"
    );
}

/// The largest hop bound this backend accepts.
fn max_depth() -> u32 {
    if B::DIALECT == DatabaseBackend::Mssql {
        READ_PREDICATE_CLOSURE_MSSQL_MAX_DEPTH
    } else {
        READ_PREDICATE_CLOSURE_MAX_DEPTH
    }
}

#[cfg(feature = "mssql")]
#[test]
fn sql_server_bounds_the_unrolled_walk() {
    // SQL Server unrolls the walk into one nested level per hop, so its bound
    // is lower; a larger one is refused rather than evaluated in memory.
    let target =
        ReadPredicate::column::<B, ClosureNode>("score", ReadPredicateOp::Gte, SqlValue::Int(5))
            .unwrap();
    const { assert!(READ_PREDICATE_CLOSURE_MSSQL_MAX_DEPTH >= READ_PREDICATE_CLOSURE_DEFAULT_DEPTH) };
    ReadPredicate::related_closure::<B, ClosureNode>(PARENT, target.clone()).unwrap();
    ReadPredicate::related_closure_all::<B, ClosureNode>(PARENT, target.clone()).unwrap();
    for quantifier in [ReadPredicateQuantifier::Any, ReadPredicateQuantifier::All] {
        ReadPredicate::related_closure_within::<B, ClosureNode>(
            quantifier,
            PARENT,
            target.clone(),
            READ_PREDICATE_CLOSURE_MSSQL_MAX_DEPTH,
        )
        .unwrap();
        for depth in [
            READ_PREDICATE_CLOSURE_MSSQL_MAX_DEPTH + 1,
            READ_PREDICATE_CLOSURE_MAX_DEPTH,
        ] {
            misconfigured(ReadPredicate::related_closure_within::<B, ClosureNode>(
                quantifier,
                PARENT,
                target.clone(),
                depth,
            ));
        }
    }
}

#[cfg(feature = "mssql")]
#[tokio::test]
#[ignore = "owned database lane"]
async fn sql_server_rejects_a_common_table_expression_inside_a_subquery() {
    // The reason SQL Server unrolls the walk instead of using the recursive
    // CTE the other backends render.
    let mut owner = owned_mssql::OwnedSqlServer::start().await.unwrap();
    let pool =
        graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(&owner.connection_string)
            .await
            .unwrap();
    let result = <B as WriteBackend>::execute_with_binds(
        &pool,
        "SELECT 1 AS x WHERE EXISTS (WITH c (n) AS (SELECT 1) SELECT n FROM c)",
        &[],
    )
    .await;
    assert!(result.is_err(), "{result:?}");
    owner.cleanup().unwrap();
}

mod portable {
    use super::*;

    const COMPOSITE: &[(&str, &str)] = &[("ptr_a", "key_a"), ("ptr_b", "key_b")];
    const FAN_OUT: &[(&str, &str)] = &[("ptr_a", "key_a")];

    fn score(op: ReadPredicateOp, value: i64) -> ReadPredicate {
        ReadPredicate::column::<B, ClosureNode>("score", op, SqlValue::Int(value)).unwrap()
    }
    /// `score >= 5`: TRUE for 5, FALSE for 1, UNKNOWN for NULL.
    fn passes() -> ReadPredicate {
        score(ReadPredicateOp::Gte, 5)
    }
    fn id_at_least(value: i64) -> ReadPredicate {
        ReadPredicate::column::<B, ClosureNode>("id", ReadPredicateOp::Gte, SqlValue::Int(value))
            .unwrap()
    }
    fn id_below(value: i64) -> ReadPredicate {
        ReadPredicate::column::<B, ClosureNode>("id", ReadPredicateOp::Lt, SqlValue::Int(value))
            .unwrap()
    }
    fn not(predicate: ReadPredicate) -> ReadPredicate {
        predicate.not().unwrap()
    }
    fn and(left: ReadPredicate, right: ReadPredicate) -> ReadPredicate {
        left.and(right).unwrap()
    }
    fn any(keys: &[(&str, &str)], target: ReadPredicate) -> ReadPredicate {
        ReadPredicate::related_closure::<B, ClosureNode>(keys, target).unwrap()
    }
    fn all(keys: &[(&str, &str)], target: ReadPredicate) -> ReadPredicate {
        ReadPredicate::related_closure_all::<B, ClosureNode>(keys, target).unwrap()
    }
    fn within(
        quantifier: ReadPredicateQuantifier,
        target: ReadPredicate,
        max_depth: u32,
    ) -> ReadPredicate {
        ReadPredicate::related_closure_within::<B, ClosureNode>(
            quantifier, PARENT, target, max_depth,
        )
        .unwrap()
    }

    #[cfg(feature = "sqlite")]
    pub(super) struct FixtureOwner;
    #[cfg(feature = "sqlite")]
    impl FixtureOwner {
        pub(super) fn cleanup(&mut self) -> Result<(), Box<dyn std::error::Error>> {
            Ok(())
        }
    }
    #[cfg(feature = "postgres")]
    pub(super) type FixtureOwner = owned_postgres::OwnedPostgres;
    #[cfg(feature = "mssql")]
    pub(super) type FixtureOwner = owned_mssql::OwnedSqlServer;

    #[cfg(feature = "mssql")]
    struct AllowEntityPolicy;
    #[cfg(feature = "mssql")]
    impl EntityPolicy for AllowEntityPolicy {
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

    async fn empty_fixture() -> (Database, FixtureOwner) {
        #[cfg(feature = "sqlite")]
        let (db, owner) = (
            Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
                .await
                .unwrap(),
            FixtureOwner,
        );
        #[cfg(feature = "postgres")]
        let (db, owner) = {
            let owner = FixtureOwner::start("read-predicate-closure").unwrap();
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
            let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(
                &owner.connection_string,
            )
            .await
            .unwrap();
            let mut db = Database::builder(pool)
                .schema_policy(SchemaPolicy::ExternalWritable)
                .build();
            db.set_entity_policy(AllowEntityPolicy);
            (db, owner)
        };
        execute(&db, "CREATE TABLE closure_nodes (id BIGINT PRIMARY KEY, parent_id BIGINT NULL, score BIGINT NULL, key_a BIGINT NULL, key_b BIGINT NULL, ptr_a BIGINT NULL, ptr_b BIGINT NULL, __gom_max BIGINT NULL)").await;
        execute(&db, "CREATE TABLE closure_marks (id BIGINT PRIMARY KEY, node_id BIGINT NOT NULL, flag BIGINT NULL)").await;
        (db, owner)
    }
    async fn execute(db: &Database, sql: &str) {
        <B as WriteBackend>::execute_with_binds(db.pool(), sql, &[])
            .await
            .unwrap();
    }

    /// Parent chains (`parent_id -> id`, score in brackets, `-` is NULL):
    ///
    /// - 1 [5] root; 2 [5] -> 1; 3 [1] -> 2; 4 [5] -> 3
    /// - 5 [5] <-> 6 [5] (a two-cycle); 7 [5] -> 7 (a self-loop)
    /// - 8 [5] -> 99, which does not exist (an unreachable step)
    /// - 9 [-] -> 1; 10 [5] -> 9; 11 [1] -> 10
    /// - 12 [1] <-> 13 [-]; 14 [1] root
    ///
    /// Composite pointers (`(ptr_a, ptr_b) -> (key_a, key_b)`):
    ///
    /// - 200 (1,1) [5] root; 201 (1,2) [5] -> (1,1)
    /// - 202 (1,3) [5] -> (1,-), partially NULL (an ambiguous step)
    /// - 203 (1,4) [5] -> (1,3); 204 (2,1) [1] -> (1,2); 205 (3,1) [5] -> (1,-)
    ///
    /// Marks: node 1 has flag 1, node 9 has flag NULL.
    async fn fixture() -> (Database, FixtureOwner) {
        let (db, owner) = empty_fixture().await;
        execute(&db, "INSERT INTO closure_nodes (id, parent_id, score) VALUES (1, NULL, 5), (2, 1, 5), (3, 2, 1), (4, 3, 5), (5, 6, 5), (6, 5, 5), (7, 7, 5), (8, 99, 5), (9, 1, NULL), (10, 9, 5), (11, 10, 1), (12, 13, 1), (13, 12, NULL), (14, NULL, 1)").await;
        execute(&db, "INSERT INTO closure_nodes (id, score, key_a, key_b, ptr_a, ptr_b) VALUES (200, 5, 1, 1, NULL, NULL), (201, 5, 1, 2, 1, 1), (202, 5, 1, 3, 1, NULL), (203, 5, 1, 4, 1, 3), (204, 1, 2, 1, 1, 2), (205, 5, 3, 1, 1, NULL)").await;
        execute(
            &db,
            "INSERT INTO closure_marks VALUES (1, 1, 1), (2, 9, NULL)",
        )
        .await;
        (db, owner)
    }

    async fn visible(db: &Database, predicate: ReadPredicate) -> Vec<i64> {
        let query = EntityQuery::<ClosureNode>::new()
            .with_read_visibility(&ReadVisibility::Complete(predicate))
            .unwrap();
        let mut ids = query
            .fetch_all(db)
            .await
            .unwrap()
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>();
        ids.sort();
        assert_eq!(query.count(db).await.unwrap() as usize, ids.len());
        ids
    }
    fn chains(predicate: ReadPredicate) -> ReadPredicate {
        and(predicate, id_below(100))
    }
    fn composite_rows(predicate: ReadPredicate) -> ReadPredicate {
        and(predicate, id_at_least(200))
    }

    #[tokio::test]
    #[cfg_attr(
        any(feature = "postgres", feature = "mssql"),
        ignore = "owned database lane"
    )]
    async fn closure_truth_tables_with_nulls_unknowns_and_cycles() {
        let (db, mut owner) = fixture().await;
        let mark = |flag: i64| {
            ReadPredicate::related::<B, ClosureNode, ClosureMark>(
                &[("id", "node_id")],
                ReadPredicate::column::<B, ClosureMark>(
                    "flag",
                    ReadPredicateOp::Eq,
                    SqlValue::Int(flag),
                )
                .unwrap(),
            )
            .unwrap()
        };
        let cases: Vec<(&str, ReadPredicate, Vec<i64>)> = vec![
            // Existential: TRUE when some reached row passes.
            ("any(p)", chains(any(PARENT, passes())), (1..=11).collect()),
            // FALSE only for a complete walk with every row FALSE: 12 and 13
            // reach an UNKNOWN, so only 14 is admitted.
            ("not(any(p))", chains(not(any(PARENT, passes()))), vec![14]),
            (
                "not(not(any(p)))",
                chains(not(not(any(PARENT, passes())))),
                (1..=11).collect(),
            ),
            // Universal: chains, a two-cycle and a self-loop pass. 8 (an
            // unreachable step), 9 (UNKNOWN itself) and 10 (an UNKNOWN
            // ancestor) are UNKNOWN; 3, 4, 11-14 reach a FALSE row.
            ("all(p)", chains(all(PARENT, passes())), vec![1, 2, 5, 6, 7]),
            (
                "not(all(p))",
                chains(not(all(PARENT, passes()))),
                vec![3, 4, 11, 12, 13, 14],
            ),
            // Duality: all(p) == not(any(not(p))).
            (
                "not(any(not(p)))",
                chains(not(any(PARENT, not(passes())))),
                vec![1, 2, 5, 6, 7],
            ),
            (
                "not(all(not(p)))",
                chains(not(all(PARENT, not(passes())))),
                (1..=11).collect(),
            ),
            // "Some reachable row fails": FALSE rows reached, UNKNOWN ignored.
            (
                "any(score < 5)",
                chains(any(PARENT, score(ReadPredicateOp::Lt, 5))),
                vec![3, 4, 11, 12, 13, 14],
            ),
            // Explicitly admitting NULL scores turns the UNKNOWNs into TRUE.
            (
                "all(p or score is null)",
                chains(all(
                    PARENT,
                    passes()
                        .or(ReadPredicate::is_null::<B, ClosureNode>("score").unwrap())
                        .unwrap(),
                )),
                vec![1, 2, 5, 6, 7, 9, 10],
            ),
            // A closure inside and/or.
            (
                "all(p) or score < 5",
                chains(
                    all(PARENT, passes())
                        .or(score(ReadPredicateOp::Lt, 5))
                        .unwrap(),
                ),
                vec![1, 2, 3, 5, 6, 7, 11, 12, 14],
            ),
            (
                "any(p) and not(all(p))",
                chains(and(any(PARENT, passes()), not(all(PARENT, passes())))),
                vec![3, 4, 11],
            ),
            // A relation target is evaluated on each reached row: rows whose
            // chain reaches node 1 have a matching mark.
            (
                "any(related(mark flag = 1))",
                chains(any(PARENT, mark(1))),
                vec![1, 2, 3, 4, 9, 10, 11],
            ),
            (
                "all(not_related(mark flag = 1))",
                chains(all(
                    PARENT,
                    ReadPredicate::not_related::<B, ClosureNode, ClosureMark>(
                        &[("id", "node_id")],
                        ReadPredicate::column::<B, ClosureMark>(
                            "flag",
                            ReadPredicateOp::Eq,
                            SqlValue::Int(1),
                        )
                        .unwrap(),
                    )
                    .unwrap(),
                )),
                vec![5, 6, 7, 12, 13, 14],
            ),
            // A closure of a closure: some reached row whose whole chain
            // passes. 8 is UNKNOWN; 12-14 are FALSE.
            (
                "any(all(p))",
                chains(any(PARENT, all(PARENT, passes()))),
                vec![1, 2, 3, 4, 5, 6, 7, 9, 10, 11],
            ),
            (
                "not(any(all(p)))",
                chains(not(any(PARENT, all(PARENT, passes())))),
                vec![12, 13, 14],
            ),
            // Composite pointers: a partially NULL pointer is ambiguous.
            (
                "composite all(p)",
                composite_rows(all(COMPOSITE, passes())),
                vec![200, 201],
            ),
            (
                "composite not(all(p))",
                composite_rows(not(all(COMPOSITE, passes()))),
                vec![204],
            ),
            (
                "composite not(any(score < 5))",
                composite_rows(not(any(COMPOSITE, score(ReadPredicateOp::Lt, 5)))),
                vec![200, 201],
            ),
            (
                "composite any(score < 5)",
                composite_rows(any(COMPOSITE, score(ReadPredicateOp::Lt, 5))),
                vec![204],
            ),
            // A non-unique key fans out to every matching row: ptr_a = 1
            // reaches 200-203, which all pass and point back into the group.
            (
                "fan-out all(p)",
                composite_rows(all(FAN_OUT, passes())),
                vec![200, 201, 202, 203, 205],
            ),
            (
                "fan-out not(all(p))",
                composite_rows(not(all(FAN_OUT, passes()))),
                vec![204],
            ),
        ];
        for (name, predicate, expected) in cases {
            assert_eq!(visible(&db, predicate).await, expected, "{name}");
        }
        owner.cleanup().unwrap();
    }

    /// The deepest `(SELECT ...)` nesting in `sql`, skipping quoted names.
    fn subquery_nesting(sql: &str) -> u32 {
        let b = sql.as_bytes();
        let (mut open, mut d, mut m, mut i) = (Vec::new(), 0u32, 0u32, 0);
        while i < b.len() {
            match b[i] {
                q @ (b'\'' | b'"' | b'[') => {
                    let c = if q == b'[' { b']' } else { q };
                    i += 1;
                    while i < b.len() && b[i] != c {
                        i += 1;
                    }
                }
                b'(' => {
                    let r = sql[i + 1..].trim_start().as_bytes();
                    let sub = r.len() >= 6 && r[..6].eq_ignore_ascii_case(b"SELECT");
                    if sub {
                        d += 1;
                        m = m.max(d);
                    }
                    open.push(sub);
                }
                b')' if open.pop() == Some(true) => d -= 1,
                _ => {}
            }
            i += 1;
        }
        m
    }

    fn related_to_itself(target: ReadPredicate) -> ReadPredicate {
        ReadPredicate::related::<B, ClosureNode, ClosureNode>(&[("id", "id")], target).unwrap()
    }
    fn rendered(predicate: &ReadPredicate) -> String {
        EntityQuery::<ClosureNode>::new()
            .with_read_visibility(&ReadVisibility::Complete(predicate.clone()))
            .unwrap()
            .build_delete_sql()
            .0
    }

    #[test]
    fn closures_nest_at_most_two_levels_on_every_backend() {
        assert_eq!(READ_PREDICATE_CLOSURE_MAX_NESTING, 2);
        // Two levels build, directly or through relations, negations and or.
        let two = any(PARENT, all(PARENT, passes()));
        let through = any(
            PARENT,
            related_to_itself(all(PARENT, passes()).not().unwrap())
                .or(passes())
                .unwrap(),
        );
        // A third level is refused however it is reached.
        for target in [two.clone(), through.clone(), related_to_itself(two.clone())] {
            for quantifier in [ReadPredicateQuantifier::Any, ReadPredicateQuantifier::All] {
                misconfigured(ReadPredicate::related_closure_within::<B, ClosureNode>(
                    quantifier,
                    PARENT,
                    target.clone(),
                    2,
                ));
            }
        }
        // Combining two-level closures side by side is not nesting.
        two.or(through).unwrap();
    }

    #[test]
    fn rendered_size_is_budgeted_before_rendering_on_every_backend() {
        // Each not_related inlines its target twice: the doubling stops at
        // the budget, and no predicate that built renders past it.
        let mut predicate = passes();
        let mut built = 0;
        loop {
            match ReadPredicate::not_related::<B, ClosureNode, ClosureNode>(
                &[("id", "id")],
                predicate.clone(),
            ) {
                Ok(next) => {
                    predicate = next;
                    built += 1;
                    assert!(rendered(&predicate).len() <= READ_PREDICATE_MAX_RENDERED_BYTES + 1024);
                    assert!(built < 32, "the budget never refused");
                }
                Err(error) => {
                    misconfigured(Err(error));
                    break;
                }
            }
        }
        // Each level adds two nested CASE expressions: the CASE limit stops
        // the chain at five levels, before its doubled target grows large.
        // SQL Server's 16-relation total stops it at three.
        assert_eq!(READ_PREDICATE_MAX_CASE_NESTING, 10);
        assert_eq!(
            built,
            if B::DIALECT == DatabaseBackend::Mssql {
                3
            } else {
                5
            }
        );
        // A fold of one kind is refused at the byte budget, and the last
        // fold that builds renders within it.
        let term = || ReadPredicate::is_not_null::<B, ClosureNode>("id").unwrap();
        let mut fold = term();
        // About 25 bytes a term: grow without copying well short of the
        // budget, then step to the refusal.
        for _ in 0..40_000 {
            fold = fold.and(term()).unwrap();
        }
        loop {
            match fold.clone().and(term()) {
                Ok(next) => fold = next,
                Err(error) => {
                    misconfigured(Err(error));
                    break;
                }
            }
        }
        let clause = rendered(&fold);
        assert!(
            clause.len() <= READ_PREDICATE_MAX_RENDERED_BYTES + 1024,
            "{}",
            clause.len()
        );
        assert!(
            clause.len() > READ_PREDICATE_MAX_RENDERED_BYTES * 9 / 10,
            "{}",
            clause.len()
        );
        // Breadth counts too: `or` of large closures stops at the budget.
        let large = any(PARENT, related_to_itself(all(PARENT, passes())));
        let mut wide = large.clone();
        let refused = (0..200).find_map(|_| match wide.clone().or(large.clone()) {
            Ok(next) => {
                wide = next;
                None
            }
            Err(error) => Some(error),
        });
        misconfigured(Err(refused.expect("the budget never refused a wide policy")));
        assert!(rendered(&wide).len() <= READ_PREDICATE_MAX_RENDERED_BYTES + 1024);
    }

    /// A 64-hop closure inside as many relations as SQL Server's nesting
    /// budget allows, and the one more that is refused there.
    fn deepest_wrapped_closure(target: ReadPredicate) -> (ReadPredicate, usize) {
        let mut predicate = target;
        for wrappers in 0..40 {
            match ReadPredicate::related::<B, ClosureNode, ClosureNode>(
                &[("id", "id")],
                predicate.clone(),
            ) {
                Ok(next) => predicate = next,
                Err(error) => {
                    misconfigured(Err(error));
                    return (predicate, wrappers);
                }
            }
        }
        (predicate, 40)
    }

    #[test]
    fn statement_nesting_is_budgeted_on_sql_server() {
        let two_levels = any(PARENT, related_to_itself(all(PARENT, passes())));
        // A closure at the default bound uses 67 subquery levels, a closure
        // inside a closure 73, and each relation around them two more.
        for (target, room) in [(all(PARENT, passes()), 4), (two_levels, 1)] {
            let (deepest, wrappers) = deepest_wrapped_closure(target);
            let nesting = subquery_nesting(&rendered(&deepest));
            if B::DIALECT == DatabaseBackend::Mssql {
                // SQL Server refuses about 96 subquery levels at run time;
                // the budget refuses past 75, or past its weighted units,
                // when the policy is built.
                assert_eq!(READ_PREDICATE_MSSQL_MAX_NESTING, 75);
                assert!(nesting <= READ_PREDICATE_MSSQL_MAX_NESTING, "{nesting}");
                assert_eq!(wrappers, room);
            } else {
                // No subquery budget off SQL Server: the relation nesting
                // limit stops it, counting the closure's own relations.
                assert!((28..=32).contains(&wrappers), "{wrappers}");
                assert_eq!(READ_PREDICATE_MAX_RELATION_NESTING, 32);
            }
        }
    }

    /// Apply `step` until it is refused; the last predicate that built and
    /// how many steps it took.
    fn grow(
        mut predicate: ReadPredicate,
        step: impl Fn(ReadPredicate, usize) -> async_graphql::Result<ReadPredicate>,
    ) -> (ReadPredicate, usize) {
        for steps in 0..100_000 {
            match step(predicate.clone(), steps) {
                Ok(next) => predicate = next,
                Err(error) => {
                    misconfigured(Err(error));
                    return (predicate, steps);
                }
            }
        }
        panic!("the expression budget never refused");
    }
    fn negations(predicate: ReadPredicate) -> (ReadPredicate, usize) {
        grow(predicate, |predicate, _| predicate.not())
    }
    fn alternations(predicate: ReadPredicate) -> (ReadPredicate, usize) {
        grow(predicate, |predicate, steps| {
            if steps % 2 == 0 {
                predicate.and(score(ReadPredicateOp::Lt, 9))
            } else {
                predicate.or(id_below(3))
            }
        })
    }

    #[test]
    fn expression_depth_is_budgeted_on_every_backend() {
        assert_eq!(READ_PREDICATE_MAX_EXPRESSION_DEPTH, 256);
        // Each not() and each alternation of and/or adds one level.
        assert_eq!(negations(passes()).1, 256);
        assert_eq!(alternations(passes()).1, 256);
        // A fold of one kind stays one level deep, however long.
        let mut fold = passes();
        for _ in 0..20_000 {
            fold = fold
                .or(ReadPredicate::is_null::<B, ClosureNode>("id").unwrap())
                .unwrap();
        }
        fold.not().unwrap();
        // On SQL Server a negation weighs more next to subqueries: around a
        // closure inside four relations, only a few fit.
        let (wrapped, _) = deepest_wrapped_closure(all(PARENT, passes()));
        let (_, around) = negations(wrapped);
        if B::DIALECT == DatabaseBackend::Mssql {
            assert!(around < 50, "{around}");
        } else {
            assert!(around > 100, "{around}");
        }
    }

    #[tokio::test]
    #[cfg_attr(
        any(feature = "postgres", feature = "mssql"),
        ignore = "owned database lane"
    )]
    async fn a_fifty_thousand_term_or_fold_builds_flat_and_runs() {
        let (mut db, mut owner) = fixture().await;
        let mut wide = ReadPredicate::is_not_null::<B, ClosureNode>("score").unwrap();
        for _ in 0..50_000 {
            wide = wide
                .or(ReadPredicate::is_null::<B, ClosureNode>("id").unwrap())
                .unwrap();
        }
        let expected = vec![
            1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 14, 200, 201, 202, 203, 204, 205,
        ];
        assert_eq!(visible(&db, wide.clone()).await, expected);
        db.set_row_policy(PredicatePolicy(wide));
        let page = EntityQuery::<ClosureNode>::new()
            .fetch_authorized_keyset(
                &db,
                None,
                None,
                KeysetPageInput {
                    after: None,
                    limit: Some(2),
                    include_total_count: true,
                },
            )
            .await
            .unwrap();
        assert_eq!(page.page_info.total_count, Some(expected.len() as i64));
        owner.cleanup().unwrap();
    }

    #[tokio::test]
    #[cfg_attr(
        any(feature = "postgres", feature = "mssql"),
        ignore = "owned database lane"
    )]
    async fn the_largest_policies_that_build_run_on_every_read_path() {
        let (mut db, mut owner) = fixture().await;
        let two_levels = any(PARENT, related_to_itself(all(PARENT, passes())));
        let mut shapes = vec![
            (all(PARENT, passes()), vec![1, 2, 5, 6, 7]),
            (two_levels.clone(), vec![1, 2, 3, 4, 5, 6, 7, 9, 10, 11]),
        ];
        if B::DIALECT == DatabaseBackend::Mssql {
            // The deepest nesting the SQL Server budget admits.
            for (target, expected) in shapes.clone() {
                shapes.push((deepest_wrapped_closure(target).0, expected));
            }
        }
        let mut shapes = shapes
            .into_iter()
            .map(|(policy, expected)| (chains(policy), Some(expected)))
            .collect::<Vec<_>>();
        // The deepest expressions that build, alone and around the deepest
        // closure nesting: an even number of negations is the identity.
        let (wrapped, _) = deepest_wrapped_closure(all(PARENT, passes()));
        for (base, expected) in [
            (
                passes(),
                vec![1, 2, 4, 5, 6, 7, 8, 10, 200, 201, 202, 203, 205],
            ),
            (chains(wrapped), vec![1, 2, 5, 6, 7]),
        ] {
            let (negated, count) = negations(base.clone());
            shapes.push((negated, (count % 2 == 0).then_some(expected)));
            shapes.push((alternations(base).0, None));
        }
        for (policy, expected) in shapes {
            let ids = visible(&db, policy.clone()).await;
            if let Some(expected) = &expected {
                assert_eq!(&ids, expected);
            }
            let expected = ids;
            db.set_row_policy(PredicatePolicy(policy));
            let page = EntityQuery::<ClosureNode>::new()
                .fetch_authorized_keyset(
                    &db,
                    None,
                    None,
                    KeysetPageInput {
                        after: None,
                        limit: Some(2),
                        include_total_count: true,
                    },
                )
                .await
                .unwrap();
            assert_eq!(page.page_info.total_count, Some(expected.len() as i64));
            let connection = EntityQuery::<ClosureNode>::new()
                .fetch_authorized_keyset_connection(
                    &db,
                    None,
                    None,
                    KeysetConnectionInput {
                        first: Some(1),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            assert_eq!(
                connection.edges.first().map(|edge| edge.node.id),
                expected.first().copied()
            );
            let groups = ClosureNode::aggregate(&db)
                .count_rows()
                .unwrap()
                .fetch()
                .await
                .unwrap();
            assert_eq!(
                groups[0].metrics[0].value,
                AggregateValue::Count(expected.len() as i64)
            );
        }
        owner.cleanup().unwrap();
    }

    #[tokio::test]
    #[cfg_attr(
        any(feature = "postgres", feature = "mssql"),
        ignore = "owned database lane"
    )]
    async fn a_target_column_named_like_internal_columns_binds_to_the_row() {
        // `__gom_max` is also a summary column of the SQL Server closure.
        let (db, mut owner) = fixture().await;
        execute(&db, "UPDATE closure_nodes SET __gom_max = score").await;
        let shadow = || {
            ReadPredicate::column::<B, ClosureNode>(
                "__gom_max",
                ReadPredicateOp::Gte,
                SqlValue::Int(5),
            )
            .unwrap()
        };
        assert_eq!(
            visible(&db, chains(all(PARENT, shadow()))).await,
            [1, 2, 5, 6, 7]
        );
        assert_eq!(visible(&db, chains(not(any(PARENT, shadow())))).await, [14]);
        owner.cleanup().unwrap();
    }

    #[tokio::test]
    #[cfg_attr(
        any(feature = "postgres", feature = "mssql"),
        ignore = "owned database lane"
    )]
    async fn the_depth_bound_fails_closed() {
        let (db, mut owner) = empty_fixture().await;
        // 100 [1] is the root of 101..=110 [5]; 120 [5] <-> 121 [5].
        let mut rows = vec!["(100, NULL, 1)".to_owned()];
        rows.extend((101..=110).map(|id| format!("({id}, {}, 5)", id - 1)));
        rows.extend(["(120, 121, 5)".to_owned(), "(121, 120, 5)".to_owned()]);
        execute(
            &db,
            &format!(
                "INSERT INTO closure_nodes (id, parent_id, score) VALUES {}",
                rows.join(", ")
            ),
        )
        .await;
        use ReadPredicateQuantifier::{All, Any};
        let only = |id: i64, predicate: ReadPredicate| {
            and(
                predicate,
                ReadPredicate::column::<B, ClosureNode>(
                    "id",
                    ReadPredicateOp::Eq,
                    SqlValue::Int(id),
                )
                .unwrap(),
            )
        };
        let fails = || score(ReadPredicateOp::Lt, 5);
        // 110 is ten hops from the failing root.
        for (depth, all_admits, not_all_admits, any_fails, not_any_fails) in [
            (3, false, false, false, false),
            (9, false, false, false, false),
            (10, false, true, true, false),
            (64, false, true, true, false),
        ] {
            let case = |name: &str| format!("{name} at depth {depth}");
            assert_eq!(
                visible(&db, only(110, within(All, passes(), depth))).await == [110],
                all_admits,
                "{}",
                case("all(p)")
            );
            assert_eq!(
                visible(&db, only(110, not(within(All, passes(), depth)))).await == [110],
                not_all_admits,
                "{}",
                case("not(all(p))")
            );
            assert_eq!(
                visible(&db, only(110, within(Any, fails(), depth))).await == [110],
                any_fails,
                "{}",
                case("any(fails)")
            );
            assert_eq!(
                visible(&db, only(110, not(within(Any, fails(), depth)))).await == [110],
                not_any_fails,
                "{}",
                case("not(any(fails))")
            );
        }
        // A passing chain cut short by the bound is not admitted by the
        // universal form, nor by the negated existential form.
        for depth in [1, 4] {
            assert!(
                visible(&db, only(105, within(All, passes(), depth)))
                    .await
                    .is_empty()
            );
            assert!(
                visible(&db, only(105, not(within(Any, fails(), depth))))
                    .await
                    .is_empty()
            );
            // Existential matches within the bound still admit.
            assert_eq!(
                visible(&db, only(105, within(Any, passes(), depth))).await,
                vec![105]
            );
        }
        // A cycle is complete once the bound covers its length.
        assert!(
            visible(&db, only(120, within(All, passes(), 1)))
                .await
                .is_empty()
        );
        for depth in [2, 3, max_depth()] {
            assert_eq!(
                visible(&db, only(120, within(All, passes(), depth))).await,
                vec![120],
                "cycle at depth {depth}"
            );
        }
        owner.cleanup().unwrap();
    }

    #[tokio::test]
    #[cfg_attr(
        any(feature = "postgres", feature = "mssql"),
        ignore = "owned database lane"
    )]
    async fn keyset_and_connection_pages_follow_closure_policies() {
        let (mut db, mut owner) = fixture().await;
        for (predicate, expected) in [
            (chains(all(PARENT, passes())), vec![1, 2, 5, 6, 7]),
            (
                chains(not(all(PARENT, passes()))),
                vec![3, 4, 11, 12, 13, 14],
            ),
            (chains(not(any(PARENT, passes()))), vec![14]),
            (
                any(PARENT, score(ReadPredicateOp::Lt, 5))
                    .or(all(COMPOSITE, passes()))
                    .unwrap(),
                // Rows 1-14 are roots of the composite walk.
                vec![1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 200, 201, 204],
            ),
        ] {
            assert_eq!(visible(&db, predicate.clone()).await, expected);
            db.set_row_policy(PredicatePolicy(predicate));
            let (mut ids, mut after) = (Vec::new(), None);
            loop {
                let page = EntityQuery::<ClosureNode>::new()
                    .fetch_authorized_keyset(
                        &db,
                        None,
                        None,
                        KeysetPageInput {
                            after: after.take(),
                            limit: Some(2),
                            include_total_count: true,
                        },
                    )
                    .await
                    .unwrap();
                assert_eq!(page.page_info.total_count, Some(expected.len() as i64));
                ids.extend(page.edges.iter().map(|edge| edge.node.id));
                if !page.page_info.has_next_page {
                    break;
                }
                after = page.page_info.end_cursor;
                assert!(ids.len() <= expected.len(), "{ids:?}");
            }
            assert_eq!(ids, expected);
            let (mut ids, mut after) = (Vec::new(), None);
            loop {
                let page = EntityQuery::<ClosureNode>::new()
                    .fetch_authorized_keyset_connection(
                        &db,
                        None,
                        None,
                        KeysetConnectionInput {
                            after: after.take(),
                            first: Some(1),
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
                ids.extend(page.edges.iter().map(|edge| edge.node.id));
                if !page.page_info.has_next_page {
                    break;
                }
                after = page.page_info.end_cursor;
                assert!(ids.len() <= expected.len(), "{ids:?}");
            }
            assert_eq!(ids, expected);
            let groups = ClosureNode::aggregate(&db)
                .count_rows()
                .unwrap()
                .fetch()
                .await
                .unwrap();
            assert_eq!(
                groups[0].metrics[0].value,
                AggregateValue::Count(expected.len() as i64)
            );
        }
        owner.cleanup().unwrap();
    }

    #[tokio::test]
    #[cfg_attr(
        any(feature = "postgres", feature = "mssql"),
        ignore = "owned database lane"
    )]
    async fn a_long_chain_over_thousands_of_rows_stays_bounded() {
        let (db, mut owner) = empty_fixture().await;
        // One chain of 1,000 rows (1 is the root) and 2,000 rows in chains
        // of five, rooted at 1001, 1006, ...
        let mut rows = vec!["(1, NULL, 5)".to_owned()];
        rows.extend((2..=1000).map(|id| format!("({id}, {}, 5)", id - 1)));
        for id in 1001..=3000_i64 {
            let parent = if (id - 1001) % 5 == 0 {
                "NULL".to_owned()
            } else {
                (id - 1).to_string()
            };
            let score = if id % 97 == 0 { 1 } else { 5 };
            rows.push(format!("({id}, {parent}, {score})"));
        }
        for chunk in rows.chunks(500) {
            execute(
                &db,
                &format!(
                    "INSERT INTO closure_nodes (id, parent_id, score) VALUES {}",
                    chunk.join(", ")
                ),
            )
            .await;
        }
        let started = std::time::Instant::now();
        // The backend's largest bound: 1,024 hops, or 64 on SQL Server. The
        // long chain's rows within the bound of its root are admitted.
        let depth = max_depth();
        let long_chain = 1000.min(depth as usize + 1);
        let policy = within(ReadPredicateQuantifier::All, passes(), depth);
        let query = EntityQuery::<ClosureNode>::new()
            .with_read_visibility(&ReadVisibility::Complete(policy))
            .unwrap();
        // Every row of the long chain passes; in the short chains a row fails
        // when it or an ancestor has id % 97 == 0.
        let expected = long_chain
            + (1001..=3000_i64)
                .filter(|id| {
                    let root = id - (id - 1001) % 5;
                    (root..=*id).all(|ancestor| ancestor % 97 != 0)
                })
                .count();
        assert_eq!(query.count(&db).await.unwrap() as usize, expected);
        // The deepest rows of the long chain walk to the bound (999 hops when
        // it is 1,024).
        let page = EntityQuery::<ClosureNode>::new()
            .with_read_visibility(&ReadVisibility::Complete(and(
                within(ReadPredicateQuantifier::All, passes(), depth),
                id_below(1001),
            )))
            .unwrap()
            .fetch_all(&db)
            .await
            .unwrap();
        assert_eq!(page.len(), long_chain);
        // The default bound cuts the long chain short: its deep rows are not
        // admitted by the universal form.
        assert_eq!(
            visible(&db, and(all(PARENT, passes()), id_below(1001))).await,
            (1..=65).collect::<Vec<_>>()
        );
        let elapsed = started.elapsed();
        eprintln!("closure reads over 3,000 rows took {elapsed:?}");
        assert!(
            elapsed < std::time::Duration::from_secs(60),
            "closure reads took {elapsed:?}"
        );
        // The SQL Server plan for the default bound: the table is scanned once
        // for the rows being authorized, and every hop and the target lookup
        // is a correlated nested-loops seek on the key; nothing walks or joins
        // the whole table per hop.
        #[cfg(feature = "mssql")]
        {
            let (sql, binds) = EntityQuery::<ClosureNode>::new()
                .with_read_visibility(&ReadVisibility::Complete(all(PARENT, passes())))
                .unwrap()
                .build_delete_sql();
            let sql = sql.replacen("DELETE FROM", "SELECT COUNT_BIG(*) AS c FROM", 1);
            let plan = actual_plan(&owner.connection_string, &sql, &binds).await;
            let operators = |name: &str| plan.matches(&format!("PhysicalOp=\"{name}\"")).count();
            assert_eq!(
                operators("Clustered Index Scan") + operators("Table Scan"),
                1
            );
            assert!(
                operators("Clustered Index Seek") > READ_PREDICATE_CLOSURE_DEFAULT_DEPTH as usize
            );
            assert_eq!(operators("Hash Match") + operators("Merge Join"), 0);
        }
        owner.cleanup().unwrap();
    }

    /// The actual plan SQL Server runs `sql` with.
    #[cfg(feature = "mssql")]
    async fn actual_plan(connection_string: &str, sql: &str, binds: &[SqlValue]) -> String {
        use graphql_orm::tokio_util::compat::TokioAsyncWriteCompatExt;
        let config = graphql_orm::tiberius::Config::from_ado_string(connection_string).unwrap();
        let tcp = tokio::net::TcpStream::connect(config.get_addr())
            .await
            .unwrap();
        tcp.set_nodelay(true).unwrap();
        let mut client = graphql_orm::tiberius::Client::connect(config, tcp.compat_write())
            .await
            .unwrap();
        client
            .simple_query("SET STATISTICS XML ON")
            .await
            .unwrap()
            .into_results()
            .await
            .unwrap();
        let values = binds
            .iter()
            .map(|value| match value {
                SqlValue::Int(value) => *value,
                other => panic!("unexpected bind {other:?}"),
            })
            .collect::<Vec<_>>();
        let params = values
            .iter()
            .map(|value| value as &dyn graphql_orm::tiberius::ToSql)
            .collect::<Vec<_>>();
        let results = client
            .query(sql, &params)
            .await
            .unwrap()
            .into_results()
            .await
            .unwrap();
        results
            .iter()
            .flatten()
            .filter_map(|row| row.try_get::<&str, _>(0).ok().flatten())
            .find(|text| text.contains("ShowPlanXML"))
            .unwrap_or_else(|| panic!("no plan: {} result sets", results.len()))
            .to_owned()
    }

    struct PredicatePolicy(ReadPredicate);
    impl RowPolicy for PredicatePolicy {
        fn read_visibility<'a>(
            &'a self,
            _: Option<&'a async_graphql::Context<'_>>,
            _: &'a Database,
            _: &'static str,
            _: Option<&'static str>,
            _: EntityAccessSurface,
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>>
        {
            Box::pin(async { Ok(ReadVisibility::Complete(self.0.clone())) })
        }
        fn can_read_row<'a>(
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

    /// Placeholders in rendered order, normalized to their 1-based position.
    fn placeholder_positions(sql: &str) -> Vec<usize> {
        match B::DIALECT {
            DatabaseBackend::Sqlite => (1..=sql.matches('?').count()).collect(),
            DatabaseBackend::Postgres | DatabaseBackend::Mssql => sql
                .split(if B::DIALECT == DatabaseBackend::Mssql {
                    "@P"
                } else {
                    "$"
                })
                .skip(1)
                .map(|rest| {
                    rest.chars()
                        .take_while(char::is_ascii_digit)
                        .collect::<String>()
                })
                .filter(|digits| !digits.is_empty())
                .map(|digits| digits.parse().unwrap())
                .collect(),
            other => panic!("unsupported test backend: {other:?}"),
        }
    }

    #[test]
    fn closures_render_recursive_ctes_in_bind_order() {
        // not(score = 11) AND all(score >= 12 AND score <> 13) OR id = 14
        let tree = not(score(ReadPredicateOp::Eq, 11))
            .and(all(
                PARENT,
                and(
                    score(ReadPredicateOp::Gte, 12),
                    score(ReadPredicateOp::Ne, 13),
                ),
            ))
            .unwrap()
            .or(ReadPredicate::column::<B, ClosureNode>(
                "id",
                ReadPredicateOp::Eq,
                SqlValue::Int(14),
            )
            .unwrap())
            .unwrap();
        let (sql, binds) = EntityQuery::<ClosureNode>::new()
            .with_read_visibility(&ReadVisibility::Complete(tree))
            .unwrap()
            .build_delete_sql();
        assert_eq!(
            binds,
            [11, 12, 13, 12, 13, 12, 13, 12, 13, 14]
                .map(SqlValue::Int)
                .to_vec(),
            "{sql}"
        );
        assert_eq!(placeholder_positions(&sql), (1..=10).collect::<Vec<_>>());
        if B::DIALECT == DatabaseBackend::Mssql {
            // One nested level and OUTER APPLY per hop, and no CTE.
            assert!(!sql.contains("WITH"), "{sql}");
            assert_eq!(sql.matches("OUTER APPLY (SELECT DISTINCT").count(), 64);
            assert!(sql.contains(") AS __gom_closure_1_l63 "), "{sql}");
            assert!(sql.contains(") AS __gom_closure_1_g "), "{sql}");
            assert!(!sql.contains("__gom_closure_1_l64"), "{sql}");
        } else {
            assert!(sql.contains("(WITH RECURSIVE __gom_closure_1 ("), "{sql}");
        }
        assert!(sql.contains("__gom_depth < 64"), "{sql}");
        // A nested closure gets its own CTE name.
        let (sql, binds) = EntityQuery::<ClosureNode>::new()
            .with_read_visibility(&ReadVisibility::Complete(any(
                PARENT,
                all(PARENT, score(ReadPredicateOp::Eq, 21)),
            )))
            .unwrap()
            .build_delete_sql();
        assert_eq!(binds.len(), 16, "{sql}");
        assert!(sql.contains("__gom_closure_1") && sql.contains("__gom_closure_2"));
        // Composite keys join every column.
        let (sql, _) = EntityQuery::<ClosureNode>::new()
            .with_read_visibility(&ReadVisibility::Complete(all(COMPOSITE, passes())))
            .unwrap()
            .build_delete_sql();
        assert!(sql.contains("__gom_p1"), "{sql}");
        // Negating a valid closure is not mistaken for a match-nothing term.
        not(any(PARENT, passes()));
        not(all(COMPOSITE, passes()));
    }

    fn empty_score_list() -> ReadPredicate {
        ReadPredicate::from_filter::<B, _>(&ClosureNodeWhereInput {
            score: Some(IntFilter {
                in_list: Some(Vec::new()),
                ..Default::default()
            }),
            ..Default::default()
        })
        .unwrap()
    }

    #[tokio::test]
    #[cfg_attr(
        any(feature = "postgres", feature = "mssql"),
        ignore = "owned database lane"
    )]
    async fn closure_refusals() {
        use ReadPredicateQuantifier::{All, Any};
        let closure = |quantifier, keys: &[(&str, &str)], target, depth| {
            ReadPredicate::related_closure_within::<B, ClosureNode>(quantifier, keys, target, depth)
        };
        for quantifier in [Any, All] {
            misconfigured(closure(quantifier, PARENT, passes(), 0));
            misconfigured(closure(quantifier, PARENT, passes(), max_depth() + 1));
            misconfigured(closure(quantifier, &[], passes(), 4));
            misconfigured(closure(quantifier, &[("missing", "id")], passes(), 4));
            misconfigured(closure(
                quantifier,
                &[("parent_id", "id) OR (1")],
                passes(),
                4,
            ));
            // The target must be for the same entity.
            misconfigured(closure(
                quantifier,
                PARENT,
                ReadPredicate::column::<B, ClosureMark>(
                    "flag",
                    ReadPredicateOp::Eq,
                    SqlValue::Int(1),
                )
                .unwrap(),
                4,
            ));
            // A match-nothing target is a plain deny; negating it is refused.
            let positive = closure(quantifier, PARENT, empty_score_list(), 4).unwrap();
            misconfigured(positive.clone().not());
            misconfigured(positive.clone().or(passes()).unwrap().not());
            misconfigured(
                closure(
                    quantifier,
                    PARENT,
                    passes().or(empty_score_list()).unwrap(),
                    4,
                )
                .unwrap()
                .not(),
            );
            let (db, mut owner) = fixture().await;
            assert!(visible(&db, positive).await.is_empty());
            owner.cleanup().unwrap();
        }
    }
}
