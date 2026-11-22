// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
//! Read predicates over schema-qualified tables with literal dotted columns.
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
    graphql_entity(table = "main.qual_parents", plural = "QualParents")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "public.qual_parents",
        plural = "QualParents"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "dbo.qual_parents",
        plural = "QualParents"
    )
)]
struct QualParent {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: i64,
    #[cfg_attr(
        not(feature = "mssql"),
        graphql_orm(db_column = "\"qual_parents.owner\"")
    )]
    #[cfg_attr(feature = "mssql", graphql_orm(db_column = "[qual_parents.owner]"))]
    owner: Option<String>,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(table = "main.qual_children", plural = "QualChildren")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "public.qual_children",
        plural = "QualChildren"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "dbo.qual_children",
        plural = "QualChildren"
    )
)]
struct QualChild {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: i64,
    #[cfg_attr(
        not(feature = "mssql"),
        graphql_orm(db_column = "\"qual_children.parent_ref\"")
    )]
    #[cfg_attr(
        feature = "mssql",
        graphql_orm(db_column = "[qual_children.parent_ref]")
    )]
    parent_ref: Option<i64>,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(table = "main.qual_nodes", plural = "QualNodes")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "public.qual_nodes",
        plural = "QualNodes"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "dbo.qual_nodes",
        plural = "QualNodes"
    )
)]
struct QualNode {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: i64,
    #[cfg_attr(not(feature = "mssql"), graphql_orm(db_column = "\"qual_nodes.up\""))]
    #[cfg_attr(feature = "mssql", graphql_orm(db_column = "[qual_nodes.up]"))]
    up: Option<i64>,
    #[cfg_attr(
        not(feature = "mssql"),
        graphql_orm(db_column = "\"qual_nodes.score\"")
    )]
    #[cfg_attr(feature = "mssql", graphql_orm(db_column = "[qual_nodes.score]"))]
    score: Option<i64>,
}

#[cfg(feature = "sqlite")]
schema_roots! { query_custom_ops: [], entities: [QualParent, QualChild, QualNode] }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [QualParent, QualChild, QualNode] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [QualParent, QualChild, QualNode] }

type B = DefaultBackend;

#[cfg(not(feature = "mssql"))]
const OWNER: &str = "\"qual_parents.owner\"";
#[cfg(feature = "mssql")]
const OWNER: &str = "[qual_parents.owner]";
#[cfg(not(feature = "mssql"))]
const PARENT_REF: &str = "\"qual_children.parent_ref\"";
#[cfg(feature = "mssql")]
const PARENT_REF: &str = "[qual_children.parent_ref]";
#[cfg(not(feature = "mssql"))]
const UP: &str = "\"qual_nodes.up\"";
#[cfg(feature = "mssql")]
const UP: &str = "[qual_nodes.up]";
#[cfg(not(feature = "mssql"))]
const SCORE: &str = "\"qual_nodes.score\"";
#[cfg(feature = "mssql")]
const SCORE: &str = "[qual_nodes.score]";

#[cfg(feature = "sqlite")]
struct Owner;
#[cfg(feature = "sqlite")]
impl Owner {
    fn cleanup(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }
}
#[cfg(feature = "postgres")]
type Owner = owned_postgres::OwnedPostgres;
#[cfg(feature = "mssql")]
type Owner = owned_mssql::OwnedSqlServer;

async fn execute(db: &Database, sql: &str) {
    <B as WriteBackend>::execute_with_binds(db.pool(), sql, &[])
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"));
}

/// Parents: 1 alice, 2 bob, 3 NULL owner. Children: 10 -> 1, 11 -> 2,
/// 12 -> 3, 13 -> NULL, 14 -> 99 (no such parent). Nodes (`up -> id`,
/// score in brackets): 1 [5] root, 2 [5] -> 1, 3 [1] -> 2, 4 [5] -> 3.
async fn fixture() -> (Database, Owner) {
    #[cfg(feature = "sqlite")]
    let (db, owner) = (
        Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
            .await
            .unwrap(),
        Owner,
    );
    #[cfg(feature = "postgres")]
    let (db, owner) = {
        let owner = Owner::start("qualified-read-predicates").unwrap();
        let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&owner.url)
            .await
            .unwrap();
        (Database::new(pool), owner)
    };
    #[cfg(feature = "mssql")]
    let (db, owner) = {
        let owner = Owner::start().await.unwrap();
        let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(
            &owner.connection_string,
        )
        .await
        .unwrap();
        pool.install_restore_marker().await.unwrap();
        (
            Database::builder(pool)
                .schema_policy(SchemaPolicy::ExternalWritable)
                .build(),
            owner,
        )
    };
    let schema = if cfg!(feature = "sqlite") {
        "main"
    } else if cfg!(feature = "postgres") {
        "public"
    } else {
        "dbo"
    };
    let target = SchemaModel::from_entities(&[
        QualParent::metadata(),
        QualChild::metadata(),
        QualNode::metadata(),
    ]);
    let plan = build_migration_plan(B::DIALECT, &SchemaModel::from_entities(&[]), &target);
    for statement in &plan.statements {
        execute(&db, statement).await;
    }
    execute(
        &db,
        &format!("INSERT INTO {schema}.qual_parents (id, {OWNER}) VALUES (1, 'alice'), (2, 'bob'), (3, NULL)"),
    )
    .await;
    execute(
        &db,
        &format!("INSERT INTO {schema}.qual_children (id, {PARENT_REF}) VALUES (10, 1), (11, 2), (12, 3), (13, NULL), (14, 99)"),
    )
    .await;
    execute(
        &db,
        &format!("INSERT INTO {schema}.qual_nodes (id, {UP}, {SCORE}) VALUES (1, NULL, 5), (2, 1, 5), (3, 2, 1), (4, 3, 5)"),
    )
    .await;
    (db, owner)
}

/// The ids of `ids` that `predicate` admits, read through `count` one id at
/// a time, with the total checked against the same predicate.
async fn admitted<T>(db: &Database, ids: &[i64], predicate: ReadPredicate) -> Vec<i64>
where
    T: DatabaseEntity + Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    let visibility = ReadVisibility::Complete(predicate.clone());
    let mut admitted = Vec::new();
    for id in ids {
        let only = predicate
            .clone()
            .and(
                ReadPredicate::column::<B, T>("id", ReadPredicateOp::Eq, SqlValue::Int(*id))
                    .unwrap(),
            )
            .unwrap();
        let count = EntityQuery::<T>::new()
            .with_read_visibility(&ReadVisibility::Complete(only))
            .unwrap()
            .count(db)
            .await
            .unwrap();
        if count == 1 {
            admitted.push(*id);
        }
    }
    let total = EntityQuery::<T>::new()
        .with_read_visibility(&visibility)
        .unwrap()
        .count(db)
        .await
        .unwrap();
    assert_eq!(total as usize, admitted.len(), "{admitted:?}");
    admitted
}

fn owned_by_alice() -> ReadPredicate {
    ReadPredicate::column::<B, QualParent>(
        OWNER,
        ReadPredicateOp::Eq,
        SqlValue::String("alice".into()),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn dotted_columns_decode_and_admit_exactly_the_right_rows() {
    let (db, mut owner) = fixture().await;
    let rows = QualParent::query(db.pool()).fetch_all().await.unwrap();
    assert_eq!(
        rows.iter()
            .find(|row| row.id == 1)
            .unwrap()
            .owner
            .as_deref(),
        Some("alice")
    );
    let rows = QualChild::query(db.pool()).fetch_all().await.unwrap();
    assert_eq!(
        rows.iter().find(|row| row.id == 11).unwrap().parent_ref,
        Some(2)
    );
    let updated = QualParent::update_by_id(
        &db,
        &1,
        UpdateQualParentInput {
            owner: Some(Some("alice".into())),
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(updated.owner.as_deref(), Some("alice"));
    let children = [10, 11, 12, 13, 14];
    let keys: &[(&str, &str)] = &[(PARENT_REF, "id")];

    // A predicate on the qualified column itself.
    let direct =
        ReadPredicate::column::<B, QualChild>(PARENT_REF, ReadPredicateOp::Eq, SqlValue::Int(2))
            .unwrap();
    assert_eq!(admitted::<QualChild>(&db, &children, direct).await, [11]);
    let null = ReadPredicate::is_null::<B, QualChild>(PARENT_REF).unwrap();
    assert_eq!(admitted::<QualChild>(&db, &children, null).await, [13]);
    let not_alice = ReadPredicate::column::<B, QualParent>(
        OWNER,
        ReadPredicateOp::Ne,
        SqlValue::String("alice".into()),
    )
    .unwrap();
    assert_eq!(
        admitted::<QualParent>(&db, &[1, 2, 3], not_alice).await,
        [2]
    );

    // `related`: the child's parent is owned by alice.
    let related =
        ReadPredicate::related::<B, QualChild, QualParent>(keys, owned_by_alice()).unwrap();
    assert_eq!(
        admitted::<QualChild>(&db, &children, related.clone()).await,
        [10]
    );
    // Its negation is three-valued (#770): a NULL key (13) and a parent whose
    // owner is NULL (12) are UNKNOWN and not admitted; a parent owned by bob
    // (11) and a key that names no parent (14) are FALSE, so the negation
    // admits them.
    assert_eq!(
        admitted::<QualChild>(&db, &children, related.not().unwrap()).await,
        [11, 14]
    );
    // `not_related` is exactly that negation.
    let not_related =
        ReadPredicate::not_related::<B, QualChild, QualParent>(keys, owned_by_alice()).unwrap();
    assert_eq!(
        admitted::<QualChild>(&db, &children, not_related).await,
        [11, 14]
    );

    // Closures, including the SQL Server breadth-first form.
    {
        let nodes = [1, 2, 3, 4];
        let up: &[(&str, &str)] = &[(UP, "id")];
        let score = |op, value| {
            ReadPredicate::column::<B, QualNode>(SCORE, op, SqlValue::Int(value)).unwrap()
        };
        // Every reached row (the row itself included) has score >= 5.
        let all =
            ReadPredicate::related_closure_all::<B, QualNode>(up, score(ReadPredicateOp::Gte, 5))
                .unwrap();
        assert_eq!(admitted::<QualNode>(&db, &nodes, all).await, [1, 2]);
        // Some reached row has score < 5.
        let any = ReadPredicate::related_closure::<B, QualNode>(up, score(ReadPredicateOp::Lt, 5))
            .unwrap();
        assert_eq!(admitted::<QualNode>(&db, &nodes, any).await, [3, 4]);
    }
    #[cfg(not(feature = "mssql"))]
    db.pool().close().await;
    #[cfg(feature = "mssql")]
    drop(db);
    owner.cleanup().unwrap();
}
