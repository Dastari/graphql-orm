// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
//! Read predicates bind each row through an alias no other scope uses (#796).
//!
//! A table qualifier is not enough on SQLite: when a qualified column is
//! missing from the inner table, SQLite keeps searching outer scopes for a
//! table exposing the same name. Here the source and target tables share a
//! name in two schemas, the target lacks `flag` and the source has it. Every
//! read must fail rather than evaluate the target's policy on the source row.
//!
//! Quoted table names that look like internal names must not change how a
//! predicate is measured or rendered, and a table named like a row alias is
//! refused rather than allowed to stand in for one.

#[cfg(feature = "mssql")]
#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
use graphql_orm::prelude::*;

/// The source, in a second schema, with the same table name as the target.
#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(table = "aux.shadow_rows", plural = "ShadowSources")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "other.shadow_rows",
        plural = "ShadowSources"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "other.shadow_rows",
        plural = "ShadowSources"
    )
)]
struct ShadowSource {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    #[filterable(type = "number")]
    flag: Option<i32>,
}

/// The target, in the default schema. Its table lacks `flag` until the
/// control phase.
#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(table = "shadow_rows", plural = "ShadowTargets")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(backend = "postgres", table = "shadow_rows", plural = "ShadowTargets")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "shadow_rows",
        plural = "ShadowTargets"
    )
)]
struct ShadowTarget {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    source_id: String,
    up: Option<String>,
    #[filterable(type = "number")]
    flag: Option<i32>,
}

/// A table whose quoted name looks like a closure's internal name.
#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(table = "__gom_closure_3", plural = "NamedNodes")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(backend = "postgres", table = "__gom_closure_3", plural = "NamedNodes")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "__gom_closure_3",
        plural = "NamedNodes"
    )
)]
struct NamedNode {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    up: Option<String>,
    score: Option<i32>,
}

/// A table named like a relation's row alias.
#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(table = "__gom_row_1", plural = "AliasRows")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(backend = "postgres", table = "__gom_row_1", plural = "AliasRows")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "__gom_row_1",
        plural = "AliasRows"
    )
)]
struct AliasRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    up: Option<String>,
}

type B = DefaultBackend;

#[cfg(feature = "sqlite")]
type Owned = ();
#[cfg(feature = "postgres")]
type Owned = owned_postgres::OwnedPostgres;
#[cfg(feature = "mssql")]
type Owned = owned_mssql::OwnedSqlServer;

#[cfg(feature = "mssql")]
const TEXT: &str = "NVARCHAR(64)";
#[cfg(not(feature = "mssql"))]
const TEXT: &str = "TEXT";
#[cfg(feature = "sqlite")]
const SCHEMA: &str = "aux";
#[cfg(not(feature = "sqlite"))]
const SCHEMA: &str = "other";
#[cfg(feature = "mssql")]
const ADD_FLAG: &str = "ALTER TABLE shadow_rows ADD flag INT NULL";
#[cfg(not(feature = "mssql"))]
const ADD_FLAG: &str = "ALTER TABLE shadow_rows ADD COLUMN flag INT NULL";

const KEYS: &[(&str, &str)] = &[("id", "source_id")];
const UP: &[(&str, &str)] = &[("up", "id")];

async fn execute(db: &Database<B>, sql: &str) {
    <B as WriteBackend>::execute_with_binds(db.pool(), sql, &[])
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"));
}

async fn database() -> (Database<B>, Owned) {
    #[cfg(feature = "sqlite")]
    let (db, owned) = (
        // One connection, so the attached schema stays with it.
        Database::<B>::connect_sqlite("sqlite::memory:")
            .await
            .unwrap(),
        (),
    );
    #[cfg(feature = "postgres")]
    let (db, owned) = {
        let owned = Owned::start("read-predicate-row-scopes").unwrap();
        let db = Database::<B>::connect_postgres(&owned.url).await.unwrap();
        (db, owned)
    };
    #[cfg(feature = "mssql")]
    let (db, owned) = {
        let owned = Owned::start().await.unwrap();
        let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(
            &owned.connection_string,
        )
        .await
        .unwrap();
        (Database::new(pool), owned)
    };
    (db, owned)
}

/// The ids `predicate` admits through the repository list, and its count,
/// each read on its own.
async fn reads<T>(
    db: &Database<B>,
    predicate: &ReadPredicate,
    id: fn(T) -> String,
) -> (Result<Vec<String>, String>, Result<i64, String>)
where
    T: DatabaseEntity + Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    let visibility = ReadVisibility::Complete(predicate.clone());
    let list = match EntityQuery::<T>::new().with_read_visibility(&visibility) {
        Ok(query) => query
            .fetch_all(db)
            .await
            .map(|rows| {
                let mut ids = rows.into_iter().map(id).collect::<Vec<_>>();
                ids.sort();
                ids
            })
            .map_err(|error| format!("{error:?}")),
        Err(error) => Err(format!("{error:?}")),
    };
    let count = match EntityQuery::<T>::new().with_read_visibility(&visibility) {
        Ok(query) => query.count(db).await.map_err(|error| format!("{error:?}")),
        Err(error) => Err(format!("{error:?}")),
    };
    (list, count)
}

fn target_flag(op: ReadPredicateOp, value: i64) -> ReadPredicate {
    ReadPredicate::column::<B, ShadowTarget>("flag", op, SqlValue::Int(value)).unwrap()
}

fn related(target: ReadPredicate) -> ReadPredicate {
    ReadPredicate::related::<B, ShadowSource, ShadowTarget>(KEYS, target).unwrap()
}

/// Source policies over the target's `flag`, each with the sources it
/// admits once the target has the column.
fn shadow_shapes() -> Vec<(&'static str, ReadPredicate, &'static [&'static str])> {
    let eq = || target_flag(ReadPredicateOp::Eq, 1);
    vec![
        ("related column", related(eq()), &["a2"]),
        (
            "related is_null",
            related(ReadPredicate::is_null::<B, ShadowTarget>("flag").unwrap()),
            &["a3"],
        ),
        (
            "not_related",
            ReadPredicate::not_related::<B, ShadowSource, ShadowTarget>(KEYS, eq()).unwrap(),
            &["a1"],
        ),
        (
            "related closure",
            related(ReadPredicate::related_closure::<B, ShadowTarget>(UP, eq()).unwrap()),
            &["a2", "a3"],
        ),
        (
            "related from_filter",
            related(
                ReadPredicate::from_filter::<B, _>(&ShadowTargetWhereInput {
                    flag: Some(IntFilter {
                        eq: Some(1),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .unwrap(),
            ),
            &["a2"],
        ),
    ]
}

/// Sources (`flag`): a1 (1), a2 (NULL), a3 (0). Targets, created without
/// `flag` (its control value in brackets): b1 -> a1 [0], a root;
/// b2 -> a2 [1], up b1; b3 -> a3 [NULL], up b2. Each source's flag differs
/// from its target's, in nullness too for a2 and a3.
#[tokio::test(flavor = "multi_thread")]
async fn a_same_named_outer_table_never_supplies_a_policy_column() {
    let (db, _owned) = database().await;
    #[cfg(feature = "sqlite")]
    execute(&db, "ATTACH DATABASE ':memory:' AS aux").await;
    #[cfg(feature = "postgres")]
    execute(&db, "CREATE SCHEMA other").await;
    // SQL Server runs CREATE SCHEMA only as the first statement of a batch.
    #[cfg(feature = "mssql")]
    execute(&db, "EXEC('CREATE SCHEMA other')").await;
    for statement in [
        format!("CREATE TABLE {SCHEMA}.shadow_rows (id {TEXT} NOT NULL PRIMARY KEY, flag INT NULL)"),
        format!(
            "CREATE TABLE shadow_rows (id {TEXT} NOT NULL PRIMARY KEY, source_id {TEXT} NOT NULL, up {TEXT} NULL)"
        ),
        format!(
            "INSERT INTO {SCHEMA}.shadow_rows (id, flag) VALUES ('a1', 1), ('a2', NULL), ('a3', 0)"
        ),
        "INSERT INTO shadow_rows (id, source_id, up) VALUES ('b1', 'a1', NULL), ('b2', 'a2', 'b1'), ('b3', 'a3', 'b2')"
            .to_owned(),
    ] {
        execute(&db, &statement).await;
    }

    // Drift: the target table lacks `flag`. Every read fails.
    let mut admitted = Vec::new();
    for (name, predicate, _) in shadow_shapes() {
        let (list, count) = reads::<ShadowSource>(&db, &predicate, |row| row.id).await;
        if let Ok(rows) = list {
            admitted.push(format!("{name}: list admitted {rows:?}"));
        }
        if let Ok(count) = count {
            admitted.push(format!("{name}: count admitted {count}"));
        }
    }
    assert!(admitted.is_empty(), "{}", admitted.join("\n"));

    // Control: the target's own values decide.
    execute(&db, ADD_FLAG).await;
    execute(&db, "UPDATE shadow_rows SET flag = 0 WHERE id = 'b1'").await;
    execute(&db, "UPDATE shadow_rows SET flag = 1 WHERE id = 'b2'").await;
    for (name, predicate, expected) in shadow_shapes() {
        let (list, count) = reads::<ShadowSource>(&db, &predicate, |row| row.id).await;
        assert_eq!(list.unwrap(), expected, "{name}");
        assert_eq!(count.unwrap(), expected.len() as i64, "{name}");
    }
    #[cfg(not(feature = "mssql"))]
    db.pool().close().await;
}

fn misconfigured<T: std::fmt::Debug>(result: async_graphql::Result<T>) {
    let error = result.expect_err("expected AUTHORIZATION_MISCONFIGURED");
    let code = error
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.get("code"))
        .map(ToString::to_string);
    assert_eq!(
        code.as_deref(),
        Some("\"AUTHORIZATION_MISCONFIGURED\""),
        "{error:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn quoted_names_that_look_internal_do_not_change_predicates() {
    // A quoted `__gom_closure_3` is a table name, not a closure: it neither
    // counts towards the closure nesting limit nor numbers the next closure.
    let score = |op, value| {
        ReadPredicate::column::<B, NamedNode>("score", op, SqlValue::Int(value)).unwrap()
    };
    let has_parent = ReadPredicate::related::<B, NamedNode, NamedNode>(
        UP,
        ReadPredicate::is_not_null::<B, NamedNode>("id").unwrap(),
    )
    .unwrap();
    let low =
        ReadPredicate::related_closure::<B, NamedNode>(UP, score(ReadPredicateOp::Lt, 5)).unwrap();
    // Two closures nested inside one another are the most allowed.
    let nested = ReadPredicate::related_closure::<B, NamedNode>(
        UP,
        ReadPredicate::related_closure_all::<B, NamedNode>(UP, score(ReadPredicateOp::Gte, 5))
            .unwrap(),
    )
    .unwrap();
    misconfigured(ReadPredicate::related_closure::<B, NamedNode>(
        UP,
        nested.clone(),
    ));

    // A table named like a row alias could stand in for one: relations and
    // closures over it are refused, while plain column tests still build.
    ReadPredicate::is_not_null::<B, AliasRow>("id").unwrap();
    misconfigured(ReadPredicate::related::<B, AliasRow, AliasRow>(
        UP,
        ReadPredicate::is_not_null::<B, AliasRow>("id").unwrap(),
    ));
    misconfigured(ReadPredicate::related_closure::<B, AliasRow>(
        UP,
        ReadPredicate::is_not_null::<B, AliasRow>("id").unwrap(),
    ));

    // Nodes (`up`, score): n1 (root, 5), n2 (n1, 1), n3 (n2, 5).
    let (db, _owned) = database().await;
    #[cfg(feature = "mssql")]
    let table = "[__gom_closure_3]";
    #[cfg(not(feature = "mssql"))]
    let table = "\"__gom_closure_3\"";
    execute(
        &db,
        &format!(
            "CREATE TABLE {table} (id {TEXT} NOT NULL PRIMARY KEY, up {TEXT} NULL, score INT NULL)"
        ),
    )
    .await;
    execute(
        &db,
        &format!(
            "INSERT INTO {table} (id, up, score) VALUES ('n1', NULL, 5), ('n2', 'n1', 1), ('n3', 'n2', 5)"
        ),
    )
    .await;
    for (name, predicate, expected) in [
        ("column", score(ReadPredicateOp::Gte, 5), &["n1", "n3"][..]),
        ("related", has_parent, &["n2", "n3"]),
        ("closure", low, &["n2", "n3"]),
        ("nested closures", nested, &["n1", "n2", "n3"]),
    ] {
        let (list, count) = reads::<NamedNode>(&db, &predicate, |row| row.id).await;
        assert_eq!(list.unwrap(), expected, "{name}");
        assert_eq!(count.unwrap(), expected.len() as i64, "{name}");
    }
    #[cfg(not(feature = "mssql"))]
    db.pool().close().await;
}
