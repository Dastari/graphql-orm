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
#[cfg(feature = "sqlite")]
schema_roots! { query_custom_ops: [], entities: [PredicateRecord] }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [PredicateRecord] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [PredicateRecord] }

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
    // A relation from secret to rank: NULL is UNKNOWN (#770), 1 is matching,
    // 2 exists but does not match. Absent OR matching admits only 2: the NULL
    // key stays UNKNOWN on both sides of the OR.
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
    assert_eq!(ids, vec![2]);
    assert_eq!(query.count(&db).await.unwrap(), 1);
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
    assert_eq!(ids, vec![3]);
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
    assert_eq!(page.edges[0].node.rank, 2);
    assert_eq!(page.page_info.total_count, Some(1));
    assert!(!page.page_info.has_next_page);
    let groups = PredicateRecord::aggregate(&db)
        .count_rows()
        .unwrap()
        .fetch()
        .await
        .unwrap();
    assert_eq!(groups[0].metrics[0].value, AggregateValue::Count(1));
    owner.cleanup().unwrap();
}

/// `related(P)` is three-valued (#770): UNKNOWN when a source key is NULL or
/// when no related row has `P` TRUE but one has it UNKNOWN. Positive results
/// are unchanged, and a negation, also one reaching the relation through
/// `and`/`or`, never admits a row on either UNKNOWN.
#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "owned database lane"
)]
async fn related_negation_fails_closed_on_null_keys_and_unknown_targets() {
    let (db, mut owner) = fixture().await.unwrap();
    let id_type = match DefaultBackend::DIALECT {
        DatabaseBackend::Sqlite => "TEXT",
        DatabaseBackend::Postgres => "UUID",
        DatabaseBackend::Mssql => "UNIQUEIDENTIFIER",
        other => panic!("unsupported test backend: {other:?}"),
    };
    execute(&db, &format!("CREATE TABLE predicate_records (id {id_type} PRIMARY KEY, rank BIGINT NOT NULL, secret BIGINT NULL, label VARCHAR(40) NOT NULL)")).await.unwrap();
    // secret -> rank. Row 1 (rank 1) has a NULL key. Row 2 points at rank 1,
    // whose secret is NULL. Row 3 points at rank 2 (secret 1). Row 4 points at
    // a rank with no row. Rows 5 and 6 share rank 5: one NULL key, one
    // pointing at nothing. Row 7 (rank 6) points at rank 5, whose two rows
    // have secrets NULL and 7: one UNKNOWN and one FALSE for `secret = 1`.
    // Row 8 (rank 8) points at itself.
    execute(&db, "INSERT INTO predicate_records VALUES ('00000000-0000-0000-0000-000000000001', 1, NULL, 'null'), ('00000000-0000-0000-0000-000000000002', 2, 1, 'unknown'), ('00000000-0000-0000-0000-000000000003', 3, 2, 'match'), ('00000000-0000-0000-0000-000000000004', 4, 9, 'absent'), ('00000000-0000-0000-0000-000000000005', 5, NULL, 'dup'), ('00000000-0000-0000-0000-000000000006', 5, 7, 'dup'), ('00000000-0000-0000-0000-000000000007', 6, 5, 'mixed'), ('00000000-0000-0000-0000-000000000008', 8, 8, 'self')").await.unwrap();
    let related = |value| {
        ReadPredicate::related::<DefaultBackend, PredicateRecord, PredicateRecord>(
            &[("secret", "rank")],
            column("secret", value).unwrap(),
        )
        .unwrap()
    };
    let ranks = |predicate: ReadPredicate| {
        let db = db.clone();
        async move {
            let query = EntityQuery::<PredicateRecord>::new()
                .with_read_visibility(&ReadVisibility::Complete(predicate))
                .unwrap();
            let mut ranks = query
                .fetch_all(&db)
                .await
                .unwrap()
                .iter()
                .map(|r| r.rank)
                .collect::<Vec<_>>();
            ranks.sort();
            assert_eq!(query.count(&db).await.unwrap(), ranks.len() as i64);
            ranks
        }
    };
    // Positive: only the row whose related row has secret = 1 TRUE.
    assert_eq!(ranks(related(1)).await, vec![3]);
    // Negation: NULL keys (rows 1, 5), the UNKNOWN-only relation (row 2) and
    // the UNKNOWN+FALSE relation (row 7) are not admitted; FALSE relations
    // (rows 4, 6, 8) are.
    assert_eq!(ranks(related(1).not().unwrap()).await, vec![4, 5, 8]);
    // Double negation is the identity.
    assert_eq!(
        ranks(related(1).not().unwrap().not().unwrap()).await,
        vec![3]
    );
    // Row 3's related row has `secret = 2` FALSE, row 2's has it UNKNOWN.
    assert_eq!(ranks(related(2)).await, Vec::<i64>::new());
    assert_eq!(ranks(related(2).not().unwrap()).await, vec![3, 4, 5, 8]);
    // A negation reaching the relation through `or` / `and` is pushed down:
    // `not(r OR rank = 4)` is FALSE-of-r AND rank <> 4.
    assert_eq!(
        ranks(
            related(1)
                .or(column("rank", 4).unwrap())
                .unwrap()
                .not()
                .unwrap()
        )
        .await,
        vec![5, 8]
    );
    // `not(r AND rank = 3)` is FALSE-of-r OR rank <> 3: the UNKNOWN relation
    // rows are admitted only through the second term.
    assert_eq!(
        ranks(
            related(1)
                .and(column("rank", 3).unwrap())
                .unwrap()
                .not()
                .unwrap()
        )
        .await,
        vec![1, 2, 4, 5, 5, 6, 8]
    );
    // ... and back again.
    assert_eq!(
        ranks(
            related(1)
                .and(column("rank", 3).unwrap())
                .unwrap()
                .not()
                .unwrap()
                .not()
                .unwrap()
        )
        .await,
        vec![3]
    );
    // A relation under a relation: rows with a related row whose own
    // `related(1).not()` is TRUE. Row 7 relates to rows 5 (UNKNOWN) and 6
    // (TRUE); row 8 to itself (TRUE). Rows 2 and 3 relate only to UNKNOWN
    // rows (1 and 2), so they stay UNKNOWN.
    let nested = ReadPredicate::related::<DefaultBackend, PredicateRecord, PredicateRecord>(
        &[("secret", "rank")],
        related(1).not().unwrap(),
    )
    .unwrap();
    assert_eq!(ranks(nested.clone()).await, vec![6, 8]);
    // Its negation admits only FALSE: rows 4 and 6, which relate to nothing.
    assert_eq!(ranks(nested.not().unwrap()).await, vec![4, 5]);
    // A composite key with a partial NULL is UNKNOWN: rows 1 and 5 never
    // pass the negation. Row 8 is its own related row.
    let composite = || {
        ReadPredicate::related::<DefaultBackend, PredicateRecord, PredicateRecord>(
            &[("secret", "rank"), ("label", "label")],
            ReadPredicate::column::<DefaultBackend, PredicateRecord>(
                "rank",
                ReadPredicateOp::Gte,
                SqlValue::Int(0),
            )
            .unwrap(),
        )
        .unwrap()
    };
    assert_eq!(ranks(composite()).await, vec![8]);
    assert_eq!(ranks(composite().not().unwrap()).await, vec![2, 3, 4, 5, 6]);
    assert_eq!(
        ranks(composite().not().unwrap().not().unwrap()).await,
        vec![8]
    );
    // A generated-filter target renders backend placeholders and binds in
    // order beside other bound terms.
    let filtered = || {
        ReadPredicate::related::<DefaultBackend, PredicateRecord, PredicateRecord>(
            &[("secret", "rank")],
            ReadPredicate::from_filter::<DefaultBackend, _>(&PredicateRecordWhereInput {
                rank: Some(graphql_orm::graphql::filters::IntFilter {
                    eq: Some(2),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap()
    };
    assert_eq!(
        ranks(column("rank", 1).unwrap().or(filtered()).unwrap()).await,
        vec![1, 3]
    );
    assert_eq!(
        ranks(
            column("rank", 4)
                .unwrap()
                .not()
                .unwrap()
                .and(filtered().not().unwrap())
                .unwrap()
        )
        .await,
        vec![2, 5, 6, 8]
    );
    owner.cleanup().unwrap();
}

/// The plan shape of the relation renderings (#770 review): the positive form
/// is one correlated `EXISTS` and the negation one key test plus one
/// `NOT EXISTS` (an anti-join on PostgreSQL), each binding the target's values
/// once. Neither renders a CASE over two `EXISTS` probes.
#[test]
fn relation_renderings_are_single_correlated_subqueries() {
    let related = || {
        ReadPredicate::related::<DefaultBackend, PredicateRecord, PredicateRecord>(
            &[("secret", "rank")],
            column("secret", 1).unwrap(),
        )
        .unwrap()
    };
    let rendered = |predicate: ReadPredicate| {
        let query = EntityQuery::<PredicateRecord>::new()
            .with_read_visibility(&ReadVisibility::Complete(predicate))
            .unwrap();
        (query.where_clauses.join(" AND "), query.values.len())
    };
    let (positive, binds) = rendered(related());
    assert_eq!(positive.matches("EXISTS (").count(), 1, "{positive}");
    assert!(
        !positive.contains("CASE") && !positive.contains("NOT "),
        "{positive}"
    );
    assert_eq!(binds, 1);
    let (negative, binds) = rendered(related().not().unwrap());
    assert_eq!(negative.matches("EXISTS (").count(), 1, "{negative}");
    assert!(
        negative.contains("IS NOT NULL AND NOT EXISTS ("),
        "{negative}"
    );
    assert!(!negative.contains("THEN NULL"), "{negative}");
    assert_eq!(binds, 1);
}
