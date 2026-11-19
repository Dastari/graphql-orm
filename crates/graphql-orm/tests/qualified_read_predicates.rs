// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
//! Read predicates over a schema-qualified table and table-qualified
//! physical columns (#782 round 3).
//!
//! A declared `db_column` such as `"qual_children"."parent_ref"` names the
//! column `parent_ref`. Relation, negated-relation and closure predicates
//! reference it through an alias or the source table, so they must render
//! the column part, never the whole path after a prefix (which reads as
//! `schema.table.column`) or a literal-quoted path (which SQLite reads as a
//! string constant). A wrong column in a read predicate is an authorization
//! bug, so every case checks exactly which rows are admitted.

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
    #[graphql_orm(db_column = "\"qual_parents\".\"owner\"")]
    owner: Option<String>,
    /// Declared with another table's qualifier: never usable in a predicate.
    #[graphql_orm(db_column = "\"qual_children\".\"id\"")]
    stray: Option<i64>,
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
    #[graphql_orm(db_column = "\"qual_children\".\"parent_ref\"")]
    parent_ref: Option<i64>,
    /// Declared with another table's qualifier: never usable in a predicate.
    #[graphql_orm(db_column = "\"qual_parents\".\"owner\"")]
    stray: Option<String>,
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
    #[graphql_orm(db_column = "\"qual_nodes\".\"up\"")]
    up: Option<i64>,
    #[graphql_orm(db_column = "\"qual_nodes\".\"score\"")]
    score: Option<i64>,
}

#[cfg(feature = "sqlite")]
schema_roots! { query_custom_ops: [], entities: [QualParent, QualChild, QualNode] }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [QualParent, QualChild, QualNode] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [QualParent, QualChild, QualNode] }

type B = DefaultBackend;

const OWNER: &str = "\"qual_parents\".\"owner\"";
const PARENT_REF: &str = "\"qual_children\".\"parent_ref\"";
const UP: &str = "\"qual_nodes\".\"up\"";
const SCORE: &str = "\"qual_nodes\".\"score\"";

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
        (Database::new(pool), owner)
    };
    #[cfg(feature = "sqlite")]
    let (schema, text) = ("main", "TEXT");
    #[cfg(feature = "postgres")]
    let (schema, text) = ("public", "TEXT");
    #[cfg(feature = "mssql")]
    let (schema, text) = ("dbo", "nvarchar(50)");
    execute(
        &db,
        &format!(
            "CREATE TABLE {schema}.qual_parents (id BIGINT NOT NULL PRIMARY KEY, owner {text} NULL)"
        ),
    )
    .await;
    execute(
        &db,
        &format!("CREATE TABLE {schema}.qual_children (id BIGINT NOT NULL PRIMARY KEY, parent_ref BIGINT NULL)"),
    )
    .await;
    execute(
        &db,
        &format!("CREATE TABLE {schema}.qual_nodes (id BIGINT NOT NULL PRIMARY KEY, up BIGINT NULL, score BIGINT NULL)"),
    )
    .await;
    execute(
        &db,
        &format!("INSERT INTO {schema}.qual_parents (id, owner) VALUES (1, 'alice'), (2, 'bob'), (3, NULL)"),
    )
    .await;
    execute(
        &db,
        &format!("INSERT INTO {schema}.qual_children (id, parent_ref) VALUES (10, 1), (11, 2), (12, 3), (13, NULL), (14, 99)"),
    )
    .await;
    execute(
        &db,
        &format!("INSERT INTO {schema}.qual_nodes (id, up, score) VALUES (1, NULL, 5), (2, 1, 5), (3, 2, 1), (4, 3, 5)"),
    )
    .await;
    (db, owner)
}

/// The ids of `ids` that `predicate` admits, read through `count` one id at
/// a time (rows with a table-qualified column are not decoded here), with
/// the total checked against the same predicate.
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

const CHILD_STRAY: &str = "\"qual_parents\".\"owner\"";
const PARENT_STRAY: &str = "\"qual_children\".\"id\"";

fn misconfigured(result: async_graphql::Result<ReadPredicate>) {
    let error = result.expect_err("the predicate must be refused");
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

/// A declared column whose qualifier names another table is refused in
/// every predicate position, rather than having the qualifier stripped: in a
/// correlated subquery it could otherwise bind to a different row.
#[test]
fn qualifiers_naming_another_table_are_refused() {
    let text = || SqlValue::String("alice".into());
    misconfigured(ReadPredicate::column::<B, QualChild>(
        CHILD_STRAY,
        ReadPredicateOp::Eq,
        text(),
    ));
    misconfigured(ReadPredicate::is_null::<B, QualChild>(CHILD_STRAY));
    misconfigured(ReadPredicate::is_not_null::<B, QualChild>(CHILD_STRAY));
    // Relation source and destination keys.
    misconfigured(ReadPredicate::related::<B, QualChild, QualParent>(
        &[(CHILD_STRAY, "id")],
        owned_by_alice(),
    ));
    misconfigured(ReadPredicate::related::<B, QualChild, QualParent>(
        &[(PARENT_REF, PARENT_STRAY)],
        owned_by_alice(),
    ));
    misconfigured(ReadPredicate::not_related::<B, QualChild, QualParent>(
        &[(CHILD_STRAY, "id")],
        owned_by_alice(),
    ));
    misconfigured(ReadPredicate::not_related::<B, QualChild, QualParent>(
        &[(PARENT_REF, PARENT_STRAY)],
        owned_by_alice(),
    ));
    // Closure keys (SQL Server refuses every closure with the same code).
    let any_child =
        ReadPredicate::column::<B, QualChild>("id", ReadPredicateOp::Gte, SqlValue::Int(0))
            .unwrap();
    misconfigured(ReadPredicate::related_closure::<B, QualChild>(
        &[(CHILD_STRAY, "id")],
        any_child.clone(),
    ));
    misconfigured(ReadPredicate::related_closure_all::<B, QualChild>(
        &[(PARENT_REF, CHILD_STRAY)],
        any_child,
    ));
    // The matching qualifier on the same entities is accepted.
    ReadPredicate::related::<B, QualChild, QualParent>(&[(PARENT_REF, "id")], owned_by_alice())
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn qualified_columns_admit_exactly_the_right_rows() {
    let (db, mut owner) = fixture().await;
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
    // Its two-valued negation (`NOT EXISTS`).
    assert_eq!(
        admitted::<QualChild>(&db, &children, related.not().unwrap()).await,
        [11, 12, 13, 14]
    );
    // `not_related` is three-valued: a NULL key (13) and a parent whose owner
    // is NULL (12) are UNKNOWN and not admitted; a parent owned by bob (11)
    // and a key that names no parent (14) are TRUE.
    let not_related =
        ReadPredicate::not_related::<B, QualChild, QualParent>(keys, owned_by_alice()).unwrap();
    assert_eq!(
        admitted::<QualChild>(&db, &children, not_related).await,
        [11, 14]
    );

    #[cfg(not(feature = "mssql"))]
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
    #[cfg(feature = "mssql")]
    {
        // SQL Server refuses closures when the policy is built, qualified
        // columns or not (see `read_predicate_closure`).
        let refused = ReadPredicate::related_closure::<B, QualNode>(
            &[(UP, "id")],
            ReadPredicate::column::<B, QualNode>(SCORE, ReadPredicateOp::Lt, SqlValue::Int(5))
                .unwrap(),
        );
        assert!(refused.is_err());
        let _ = SCORE;
    }
    #[cfg(not(feature = "mssql"))]
    db.pool().close().await;
    #[cfg(feature = "mssql")]
    drop(db);
    owner.cleanup().unwrap();
}
