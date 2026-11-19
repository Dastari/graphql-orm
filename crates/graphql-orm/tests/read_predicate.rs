// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]

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
        table = "predicate_records",
        plural = "PredicateRecords",
        keyset = "rank asc, id asc",
        aggregate = true
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "predicate_records",
        plural = "PredicateRecords",
        keyset = "rank asc, id asc",
        aggregate = true
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "predicate_records",
        plural = "PredicateRecords",
        keyset = "rank asc, id asc",
        aggregate = true
    )
)]
struct PredicateRecord {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[sortable]
    #[filterable(type = "number")]
    rank: i64,
    #[graphql_orm(read = false, filter = false, order = false, subscribe = false)]
    secret: Option<i64>,
    label: String,
}
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(table = "predicate_children", plural = "PredicateChildren")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "predicate_children",
        plural = "PredicateChildren"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "predicate_children",
        plural = "PredicateChildren"
    )
)]
struct PredicateChild {
    #[primary_key]
    #[sortable]
    id: graphql_orm::uuid::Uuid,
    parent_rank: Option<i64>,
    #[filterable(type = "number")]
    flag: Option<i64>,
}
#[cfg(feature = "sqlite")]
schema_roots! { query_custom_ops: [], entities: [PredicateRecord, PredicateChild] }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [PredicateRecord, PredicateChild] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [PredicateRecord, PredicateChild] }

fn column(name: &str, value: i64) -> async_graphql::Result<ReadPredicate> {
    ReadPredicate::column::<DefaultBackend, PredicateRecord>(
        name,
        ReadPredicateOp::Eq,
        SqlValue::Int(value),
    )
}
#[test]
fn validates_private_columns_without_public_filter_exposure() {
    assert!(column("secret) OR 1=1 --", 1).is_err());
    assert!(column("missing", 1).is_err());
    assert!(column("secret", 1).is_ok());
    let query = EntityQuery::<PredicateRecord>::new()
        .with_read_visibility(&ReadVisibility::Complete(column("secret", 918273).unwrap()))
        .unwrap();
    let (sql, binds) = query.build_delete_sql();
    assert!(!sql.contains("918273"));
    assert_eq!(binds, vec![SqlValue::Int(918273)]);
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

async fn fixture() -> Result<(Database, FixtureOwner), Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    {
        Ok((
            Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?,
            FixtureOwner,
        ))
    }
    #[cfg(feature = "postgres")]
    {
        let owner = FixtureOwner::start("field-write-denial")?;
        let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&owner.url)
            .await?;
        Ok((Database::new(pool), owner))
    }
    #[cfg(feature = "mssql")]
    {
        let owner = FixtureOwner::start().await?;
        let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(
            &owner.connection_string,
        )
        .await?;
        let mut db = Database::builder(pool)
            .schema_policy(SchemaPolicy::ExternalWritable)
            .build();
        db.set_entity_policy(AllowEntityPolicy);
        Ok((db, owner))
    }
}
async fn execute(db: &Database, sql: &str) -> graphql_orm::Result<()> {
    <DefaultBackend as WriteBackend>::execute_with_binds(db.pool(), sql, &[]).await?;
    Ok(())
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
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
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
#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "owned database lane"
)]
async fn negation_preserves_null_and_relation_truth_tables() {
    let (mut db, mut owner) = fixture().await.unwrap();
    let sdl = schema_builder(db.clone()).finish().sdl();
    let input = sdl
        .split("input PredicateRecordWhereInput {")
        .nth(1)
        .unwrap()
        .split('}')
        .next()
        .unwrap();
    assert!(!input.contains("secret"), "{input}");
    let id_type = match DefaultBackend::DIALECT {
        DatabaseBackend::Sqlite => "TEXT",
        DatabaseBackend::Postgres => "UUID",
        DatabaseBackend::Mssql => "UNIQUEIDENTIFIER",
        other => panic!("unsupported test backend: {other:?}"),
    };
    execute(&db, &format!("CREATE TABLE predicate_records (id {id_type} PRIMARY KEY, rank BIGINT NOT NULL, secret BIGINT NULL, label VARCHAR(40) NOT NULL)")).await.unwrap();
    execute(&db, "INSERT INTO predicate_records VALUES ('00000000-0000-0000-0000-000000000001', 1, NULL, 'null'), ('00000000-0000-0000-0000-000000000002', 2, 1, 'match'), ('00000000-0000-0000-0000-000000000003', 3, 2, 'other')").await.unwrap();
    let query = EntityQuery::<PredicateRecord>::new()
        .with_read_visibility(&ReadVisibility::Complete(
            column("secret", 1).unwrap().not().unwrap(),
        ))
        .unwrap();
    let rows = query.fetch_all(&db).await.unwrap();
    assert_eq!(rows.iter().map(|r| r.rank).collect::<Vec<_>>(), vec![3]);
    assert_eq!(query.count(&db).await.unwrap(), 1);
    // A relation from secret to rank: NULL has no related row, 1 is matching,
    // 2 exists but does not match. Absent OR matching must admit only 1 and 2.
    let exists = || {
        ReadPredicate::related::<DefaultBackend, PredicateRecord, PredicateRecord>(
            &[("secret", "rank")],
            column("rank", 0).unwrap().not().unwrap(),
        )
        .unwrap()
    };
    let matches = ReadPredicate::related::<DefaultBackend, PredicateRecord, PredicateRecord>(
        &[("secret", "rank")],
        column("rank", 1).unwrap(),
    )
    .unwrap();
    let allowed = exists().not().unwrap().or(matches).unwrap();
    let query = EntityQuery::<PredicateRecord>::new()
        .with_read_visibility(&ReadVisibility::Complete(allowed.clone()))
        .unwrap();
    let mut ids = query
        .fetch_all(&db)
        .await
        .unwrap()
        .iter()
        .map(|r| r.rank)
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, vec![1, 2]);
    assert_eq!(query.count(&db).await.unwrap(), 2);
    // The related row for source 2 has a NULL secret: it must not match.
    let nullable_match =
        ReadPredicate::related::<DefaultBackend, PredicateRecord, PredicateRecord>(
            &[("secret", "rank")],
            column("secret", 1).unwrap(),
        )
        .unwrap();
    let nullable_allowed = exists().not().unwrap().or(nullable_match).unwrap();
    let mut ids = EntityQuery::<PredicateRecord>::new()
        .with_read_visibility(&ReadVisibility::Complete(nullable_allowed))
        .unwrap()
        .fetch_all(&db)
        .await
        .unwrap()
        .iter()
        .map(|r| r.rank)
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, vec![1, 3]);
    db.set_row_policy(PredicatePolicy(allowed));
    let page = EntityQuery::<PredicateRecord>::new()
        .fetch_authorized_keyset(
            &db,
            None,
            None,
            KeysetPageInput {
                limit: Some(1),
                include_total_count: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(page.edges[0].node.rank, 1);
    assert_eq!(page.page_info.total_count, Some(2));
    assert!(page.page_info.has_next_page);
    let groups = PredicateRecord::aggregate(&db)
        .count_rows()
        .unwrap()
        .fetch()
        .await
        .unwrap();
    assert_eq!(groups[0].metrics[0].value, AggregateValue::Count(2));
    owner.cleanup().unwrap();
}

fn compare(name: &str, op: ReadPredicateOp, value: i64) -> ReadPredicate {
    ReadPredicate::column::<DefaultBackend, PredicateRecord>(name, op, SqlValue::Int(value))
        .unwrap()
}
fn flag(value: i64) -> ReadPredicate {
    ReadPredicate::column::<DefaultBackend, PredicateChild>(
        "flag",
        ReadPredicateOp::Eq,
        SqlValue::Int(value),
    )
    .unwrap()
}
fn not(predicate: ReadPredicate) -> ReadPredicate {
    predicate.not().unwrap()
}
fn related(key: &str, target: ReadPredicate) -> ReadPredicate {
    ReadPredicate::related::<DefaultBackend, PredicateRecord, PredicateChild>(
        &[(key, "parent_rank")],
        target,
    )
    .unwrap()
}
fn not_related(key: &str, target: ReadPredicate) -> ReadPredicate {
    ReadPredicate::not_related::<DefaultBackend, PredicateRecord, PredicateChild>(
        &[(key, "parent_rank")],
        target,
    )
    .unwrap()
}
fn secret_is_null() -> ReadPredicate {
    ReadPredicate::is_null::<DefaultBackend, PredicateRecord>("secret").unwrap()
}
fn misconfigured(result: async_graphql::Result<ReadPredicate>) {
    let error = result.expect_err("negating a match-nothing term must be refused");
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
fn empty_rank_list() -> ReadPredicate {
    ReadPredicate::from_filter::<DefaultBackend, _>(&PredicateRecordWhereInput {
        rank: Some(IntFilter {
            in_list: Some(Vec::new()),
            ..Default::default()
        }),
        ..Default::default()
    })
    .unwrap()
}
fn empty_flag_list() -> ReadPredicate {
    ReadPredicate::from_filter::<DefaultBackend, _>(&PredicateChildWhereInput {
        flag: Some(IntFilter {
            in_list: Some(Vec::new()),
            ..Default::default()
        }),
        ..Default::default()
    })
    .unwrap()
}

/// Placeholders in rendered order, normalized to their 1-based position.
fn placeholder_positions(sql: &str) -> Vec<usize> {
    match DefaultBackend::DIALECT {
        DatabaseBackend::Sqlite => (1..=sql.matches('?').count()).collect(),
        DatabaseBackend::Postgres => sql
            .split('$')
            .skip(1)
            .map(|rest| {
                rest.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
            })
            .filter(|digits| !digits.is_empty())
            .map(|digits| digits.parse().unwrap())
            .collect(),
        DatabaseBackend::Mssql => sql
            .split("@P")
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
fn nested_negation_renders_parenthesized_trees_in_bind_order() {
    // not( not(secret = 11) AND rank = 12  OR  not_related(flag = 13 AND flag = 14) )
    let tree = not(not(compare("secret", ReadPredicateOp::Eq, 11))
        .and(compare("rank", ReadPredicateOp::Eq, 12))
        .unwrap()
        .or(not_related("rank", flag(13).and(flag(14)).unwrap()))
        .unwrap());
    let (sql, binds) = EntityQuery::<PredicateRecord>::new()
        .with_read_visibility(&ReadVisibility::Complete(tree))
        .unwrap()
        .build_delete_sql();
    assert_eq!(
        binds,
        [11, 12, 13, 14, 13, 14].map(SqlValue::Int).to_vec(),
        "{sql}"
    );
    assert_eq!(placeholder_positions(&sql), vec![1, 2, 3, 4, 5, 6], "{sql}");
    let q = |name: &str| DefaultBackend::DIALECT.quote_identifier(name);
    assert!(sql.contains("NOT (((NOT ("), "{sql}");
    assert!(
        sql.contains(&format!("{} IS NULL THEN NULL WHEN EXISTS", q("rank"))),
        "{sql}"
    );
    assert!(sql.contains("THEN 0 ELSE 1 END = 1"), "{sql}");
    assert!(sql.contains("THEN NULL ELSE 1 END) = 1"), "{sql}");
    // Double negation keeps both NOTs and their parentheses.
    let (sql, binds) = EntityQuery::<PredicateRecord>::new()
        .with_read_visibility(&ReadVisibility::Complete(not(not(compare(
            "rank",
            ReadPredicateOp::Eq,
            7,
        )))))
        .unwrap()
        .build_delete_sql();
    assert_eq!(binds, vec![SqlValue::Int(7)]);
    // The column is bound to its own table (#796).
    assert!(
        sql.contains(&format!(
            "NOT (NOT ({}.{} = ",
            q("predicate_records"),
            q("rank")
        )),
        "{sql}"
    );
}

#[test]
fn negating_an_empty_list_is_refused_at_construction() {
    // The positive empty list is a deny; its negation would grant every row.
    misconfigured(empty_rank_list().not());
    misconfigured(
        empty_rank_list()
            .or(compare("rank", ReadPredicateOp::Eq, 1))
            .unwrap()
            .not(),
    );
    misconfigured(
        compare("rank", ReadPredicateOp::Eq, 1)
            .and(empty_rank_list())
            .unwrap()
            .not(),
    );
    misconfigured(related("rank", empty_flag_list()).not());
    misconfigured(ReadPredicate::not_related::<
        DefaultBackend,
        PredicateRecord,
        PredicateChild,
    >(&[("rank", "parent_rank")], empty_flag_list()));
    misconfigured(ReadPredicate::not_related::<
        DefaultBackend,
        PredicateRecord,
        PredicateChild,
    >(
        &[("rank", "parent_rank")],
        flag(1).or(empty_flag_list()).unwrap(),
    ));
    // Unknown columns, a wrong target entity and empty keys fail closed.
    assert!(ReadPredicate::is_null::<DefaultBackend, PredicateRecord>("missing").is_err());
    assert!(
        ReadPredicate::is_not_null::<DefaultBackend, PredicateRecord>("secret) OR (1").is_err()
    );
    assert!(
        ReadPredicate::not_related::<DefaultBackend, PredicateRecord, PredicateChild>(
            &[("rank", "parent_rank")],
            compare("rank", ReadPredicateOp::Eq, 1),
        )
        .is_err()
    );
    assert!(
        ReadPredicate::not_related::<DefaultBackend, PredicateRecord, PredicateChild>(&[], flag(1))
            .is_err()
    );
    assert!(
        ReadPredicate::not_related::<DefaultBackend, PredicateRecord, PredicateChild>(
            &[("missing", "parent_rank")],
            flag(1),
        )
        .is_err()
    );
}

async fn visible_ranks(db: &Database, predicate: ReadPredicate) -> Vec<i64> {
    let query = EntityQuery::<PredicateRecord>::new()
        .with_read_visibility(&ReadVisibility::Complete(predicate))
        .unwrap();
    let mut ranks = query
        .fetch_all(db)
        .await
        .unwrap()
        .iter()
        .map(|row| row.rank)
        .collect::<Vec<_>>();
    ranks.sort();
    assert_eq!(query.count(db).await.unwrap() as usize, ranks.len());
    ranks
}

async fn negation_fixture() -> (Database, FixtureOwner) {
    let (db, owner) = fixture().await.unwrap();
    let id_type = match DefaultBackend::DIALECT {
        DatabaseBackend::Sqlite => "TEXT",
        DatabaseBackend::Postgres => "UUID",
        DatabaseBackend::Mssql => "UNIQUEIDENTIFIER",
        other => panic!("unsupported test backend: {other:?}"),
    };
    execute(&db, &format!("CREATE TABLE predicate_records (id {id_type} PRIMARY KEY, rank BIGINT NOT NULL, secret BIGINT NULL, label VARCHAR(40) NOT NULL)")).await.unwrap();
    execute(&db, &format!("CREATE TABLE predicate_children (id {id_type} PRIMARY KEY, parent_rank BIGINT NULL, flag BIGINT NULL)")).await.unwrap();
    // rank: secret
    execute(&db, "INSERT INTO predicate_records VALUES ('00000000-0000-0000-0000-000000000001', 1, NULL, 'a'), ('00000000-0000-0000-0000-000000000002', 2, 1, 'b'), ('00000000-0000-0000-0000-000000000003', 3, 2, 'c'), ('00000000-0000-0000-0000-000000000004', 4, 3, 'd')").await.unwrap();
    // parent_rank: flag. Rank 1 has a matching child, 2 a non-matching one,
    // 3 an UNKNOWN and a non-matching one, 4 none; one child has no parent.
    execute(&db, "INSERT INTO predicate_children VALUES ('00000000-0000-0000-0000-0000000000a1', 1, 1), ('00000000-0000-0000-0000-0000000000a2', 2, 0), ('00000000-0000-0000-0000-0000000000a3', 3, NULL), ('00000000-0000-0000-0000-0000000000a4', 3, 0), ('00000000-0000-0000-0000-0000000000a5', NULL, 1)").await.unwrap();
    (db, owner)
}

#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "owned database lane"
)]
async fn negation_truth_tables_with_nulls() {
    let (db, mut owner) = negation_fixture().await;
    let eq_secret = || compare("secret", ReadPredicateOp::Eq, 1);
    let cases: Vec<(&str, ReadPredicate, Vec<i64>)> = vec![
        ("secret = 1", eq_secret(), vec![2]),
        ("not(secret = 1)", not(eq_secret()), vec![3, 4]),
        ("not(not(secret = 1))", not(not(eq_secret())), vec![2]),
        (
            "not(not(not(secret = 1)))",
            not(not(not(eq_secret()))),
            vec![3, 4],
        ),
        (
            "not(secret <> 1)",
            not(compare("secret", ReadPredicateOp::Ne, 1)),
            vec![2],
        ),
        (
            "not(secret > 1)",
            not(compare("secret", ReadPredicateOp::Gt, 1)),
            vec![2],
        ),
        (
            "not(secret <= 1)",
            not(compare("secret", ReadPredicateOp::Lte, 1)),
            vec![3, 4],
        ),
        (
            "not(secret = 1) or secret is null",
            not(eq_secret()).or(secret_is_null()).unwrap(),
            vec![1, 3, 4],
        ),
        ("not(secret is null)", not(secret_is_null()), vec![2, 3, 4]),
        (
            "secret is not null",
            ReadPredicate::is_not_null::<DefaultBackend, PredicateRecord>("secret").unwrap(),
            vec![2, 3, 4],
        ),
        (
            "not(secret = 1 or rank = 3)",
            not(eq_secret()
                .or(compare("rank", ReadPredicateOp::Eq, 3))
                .unwrap()),
            vec![4],
        ),
        // FALSE whatever the NULL holds: rank 1 is admitted because rank <> 2.
        (
            "not(secret = 1 and rank = 2)",
            not(eq_secret()
                .and(compare("rank", ReadPredicateOp::Eq, 2))
                .unwrap()),
            vec![1, 3, 4],
        ),
        (
            "not(secret = 1) and not(rank = 4)",
            not(eq_secret())
                .and(not(compare("rank", ReadPredicateOp::Eq, 4)))
                .unwrap(),
            vec![3],
        ),
        (
            "not(not(secret = 1) or rank = 1)",
            not(not(eq_secret())
                .or(compare("rank", ReadPredicateOp::Eq, 1))
                .unwrap()),
            vec![2],
        ),
        // Relations keyed by the non-null rank.
        ("related(flag = 1)", related("rank", flag(1)), vec![1]),
        (
            "not_related(flag = 1)",
            not_related("rank", flag(1)),
            vec![2, 4],
        ),
        (
            "not(not_related(flag = 1))",
            not(not_related("rank", flag(1))),
            vec![1],
        ),
        (
            "not(not(not_related(flag = 1)))",
            not(not(not_related("rank", flag(1)))),
            vec![2, 4],
        ),
        // Two-valued NOT EXISTS: the UNKNOWN child of rank 3 does not block.
        (
            "not(related(flag = 1))",
            not(related("rank", flag(1))),
            vec![2, 3, 4],
        ),
        (
            "not_related(flag = 0)",
            not_related("rank", flag(0)),
            vec![1, 4],
        ),
        (
            "not_related(flag = 1 or flag = 0)",
            not_related("rank", flag(1).or(flag(0)).unwrap()),
            vec![4],
        ),
        (
            "not_related(not(flag = 1))",
            not_related("rank", not(flag(1))),
            vec![1, 4],
        ),
        (
            "not_related(flag = 1) and not(secret = 3)",
            not_related("rank", flag(1))
                .and(not(compare("secret", ReadPredicateOp::Eq, 3)))
                .unwrap(),
            vec![2],
        ),
        (
            "not_related(flag = 1) or rank = 3",
            not_related("rank", flag(1))
                .or(compare("rank", ReadPredicateOp::Eq, 3))
                .unwrap(),
            vec![2, 3, 4],
        ),
        // Relations keyed by the nullable secret: a NULL key is UNKNOWN.
        (
            "related(secret; flag = 1)",
            related("secret", flag(1)),
            vec![2],
        ),
        (
            "not_related(secret; flag = 1)",
            not_related("secret", flag(1)),
            vec![3],
        ),
        (
            "not(not_related(secret; flag = 1))",
            not(not_related("secret", flag(1))),
            vec![2],
        ),
        (
            "not_related(secret; flag = 1) or secret is null",
            not_related("secret", flag(1)).or(secret_is_null()).unwrap(),
            vec![1, 3],
        ),
        (
            "not(related(secret; flag = 1))",
            not(related("secret", flag(1))),
            vec![1, 3, 4],
        ),
    ];
    for (name, predicate, expected) in cases {
        assert_eq!(visible_ranks(&db, predicate).await, expected, "{name}");
    }
    owner.cleanup().unwrap();
}

#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "owned database lane"
)]
async fn keyset_and_connection_pages_follow_negated_policies() {
    let (mut db, mut owner) = negation_fixture().await;
    for (predicate, expected) in [
        (not_related("rank", flag(1)), vec![2, 4]),
        (
            not(compare("secret", ReadPredicateOp::Eq, 2))
                .or(not(not_related("secret", flag(1))))
                .unwrap(),
            vec![2, 4],
        ),
        (not(compare("rank", ReadPredicateOp::Eq, 3)), vec![1, 2, 4]),
    ] {
        assert_eq!(visible_ranks(&db, predicate.clone()).await, expected);
        db.set_row_policy(PredicatePolicy(predicate));
        let (mut ranks, mut after) = (Vec::new(), None);
        loop {
            let page = EntityQuery::<PredicateRecord>::new()
                .fetch_authorized_keyset(
                    &db,
                    None,
                    None,
                    KeysetPageInput {
                        after: after.take(),
                        limit: Some(1),
                        include_total_count: true,
                    },
                )
                .await
                .unwrap();
            assert_eq!(page.page_info.total_count, Some(expected.len() as i64));
            ranks.extend(page.edges.iter().map(|edge| edge.node.rank));
            if !page.page_info.has_next_page {
                break;
            }
            after = page.page_info.end_cursor;
            assert!(ranks.len() <= expected.len(), "{ranks:?}");
        }
        assert_eq!(ranks, expected);
        let (mut ranks, mut after) = (Vec::new(), None);
        loop {
            let page = EntityQuery::<PredicateRecord>::new()
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
            ranks.extend(page.edges.iter().map(|edge| edge.node.rank));
            if !page.page_info.has_next_page {
                break;
            }
            after = page.page_info.end_cursor;
            assert!(ranks.len() <= expected.len(), "{ranks:?}");
        }
        assert_eq!(ranks, expected);
    }
    owner.cleanup().unwrap();
}
