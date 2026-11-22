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
//! Row aliases and closure names differ from every table and alias in
//! scope: table names starting with `__gom_` are refused, and a table of
//! that name mentioned in hand-written policy SQL moves the names the ORM
//! picks. Generated filters render their columns for the row they are
//! given, so a column called `__gom_self` is still that column, and
//! hand-written policy SQL is read with the backend's own lexical rules.

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
    /// A key the table lacks until its own control phase.
    link: Option<String>,
    #[filterable(type = "number")]
    flag: Option<i32>,
}

/// A tree of nodes for closures.
#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(table = "named_nodes", plural = "NamedNodes")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(backend = "postgres", table = "named_nodes", plural = "NamedNodes")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "named_nodes",
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

/// A target whose policy column is physically named like the row qualifier
/// hand-written filters use, beside a column named like a row alias.
#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(table = "marked_rows", plural = "MarkedRows")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(backend = "postgres", table = "marked_rows", plural = "MarkedRows")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "marked_rows",
        plural = "MarkedRows"
    )
)]
struct MarkedRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    source_id: String,
    #[graphql_orm(db_column = "__gom_self")]
    #[filterable(type = "number")]
    marker: Option<i32>,
    #[graphql_orm(db_column = "__gom_row_1")]
    #[filterable(type = "number")]
    decoy: Option<i32>,
}

/// The nodes a policy lists.
#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(table = "listed_nodes", plural = "ListedNodes")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(backend = "postgres", table = "listed_nodes", plural = "ListedNodes")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "listed_nodes",
        plural = "ListedNodes"
    )
)]
struct ListedNode {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
}

/// Declares a physical column with the name of SQLite's implicit row id.
#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(table = "oid_rows", plural = "OidRows")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(backend = "postgres", table = "oid_rows", plural = "OidRows")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "oid_rows",
        plural = "OidRows"
    )
)]
struct OidRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    oid: Option<i32>,
}

/// `OwnerId` declared without quotes, which PostgreSQL folds to `ownerid`,
/// beside a column declared as `"OwnerId"`.
#[cfg(feature = "postgres")]
#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "postgres", table = "case_rows", plural = "CaseRows")]
struct CaseRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    #[graphql_orm(db_column = "OwnerId")]
    folded: Option<i32>,
    #[graphql_orm(db_column = "\"OwnerId\"")]
    exact: Option<i32>,
}

/// Columns named like operators: `like` (reserved everywhere) and `glob`.
#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(table = "word_rows", plural = "WordRows")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(backend = "postgres", table = "word_rows", plural = "WordRows")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "word_rows",
        plural = "WordRows"
    )
)]
struct WordRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    #[graphql_orm(db_column = "like")]
    liked: Option<i32>,
    glob: Option<String>,
}

/// A filter whose `to_sql_conditions` answers differently on each call: the
/// first answer is `self.0`, every later one `self.1`.
struct Changing(
    (&'static str, Vec<SqlValue>),
    (&'static str, Vec<SqlValue>),
    std::sync::atomic::AtomicUsize,
);
impl DatabaseFilter for Changing {
    fn to_sql_conditions(&self) -> (Vec<String>, Vec<SqlValue>) {
        let call = self.2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (clause, values) = if call == 0 { &self.0 } else { &self.1 };
        (vec![(*clause).to_owned()], values.clone())
    }
    fn is_empty(&self) -> bool {
        false
    }
}
impl ReadVisibilityFilter for Changing {
    fn entity_type_id() -> std::any::TypeId {
        std::any::TypeId::of::<NamedNode>()
    }
    fn backend() -> DatabaseBackend {
        B::DIALECT
    }
    fn table_name() -> Option<&'static str> {
        Some(NamedNode::TABLE_NAME)
    }
}

/// A hand-built entity over a table `TABLE` names, with the node columns.
#[derive(Clone)]
struct HandBuilt<const TABLE: usize>;
impl<const TABLE: usize> FromSqlRow<B> for HandBuilt<TABLE> {
    fn from_row(_: &<B as OrmBackend>::Row) -> graphql_orm::Result<Self> {
        Ok(Self)
    }
}
const RESERVED_TABLES: [&str; 6] = [
    "__gom_closure_1",
    "__gom_row_1",
    "main.__GOM_Visibility_2",
    "\"__gom_closure_2_t\"",
    "__gom_self",
    // Not a generated name: an ordinary table.
    "__gom_orders",
];
impl<const TABLE: usize> DatabaseEntity for HandBuilt<TABLE> {
    const TABLE_NAME: &'static str = RESERVED_TABLES[TABLE];
    const PLURAL_NAME: &'static str = "HandBuilts";
    const PRIMARY_KEY: &'static str = "id";
    const DEFAULT_SORT: &'static str = "id";
    fn column_names() -> &'static [&'static str] {
        NamedNode::column_names()
    }
}
impl<const TABLE: usize> DatabaseSchema for HandBuilt<TABLE> {
    fn columns() -> &'static [ColumnDef] {
        <NamedNode as DatabaseSchema>::columns()
    }
    fn indexes() -> &'static [IndexDef] {
        &[]
    }
    fn composite_unique_indexes() -> &'static [&'static [&'static str]] {
        &[]
    }
}
impl<const TABLE: usize> EntityRelations for HandBuilt<TABLE> {}
impl<const TABLE: usize> DatabaseSearchSchema for HandBuilt<TABLE> {}
impl<const TABLE: usize> Entity for HandBuilt<TABLE> {
    fn entity_name() -> &'static str {
        "HandBuilt"
    }
    fn metadata() -> &'static EntityMetadata {
        NamedNode::metadata()
    }
}

/// A hand-written filter over `E`: one SQL clause. `BOUND` filters name
/// their table, so the clause may write the `__gom_self` row qualifier.
#[derive(Clone)]
struct Written<E, const BOUND: bool>(
    &'static str,
    Vec<SqlValue>,
    std::marker::PhantomData<fn() -> E>,
);
fn written<E: Entity + 'static>(clause: &'static str) -> Written<E, true> {
    Written(clause, Vec::new(), std::marker::PhantomData)
}
/// As [`written`], with the clause's bind values.
fn written_with<E: Entity + 'static>(
    clause: &'static str,
    values: Vec<SqlValue>,
) -> Written<E, true> {
    Written(clause, values, std::marker::PhantomData)
}
fn unbound<E: Entity + 'static>(clause: &'static str) -> Written<E, false> {
    Written(clause, Vec::new(), std::marker::PhantomData)
}
impl<E, const BOUND: bool> DatabaseFilter for Written<E, BOUND> {
    fn to_sql_conditions(&self) -> (Vec<String>, Vec<SqlValue>) {
        (vec![self.0.to_owned()], self.1.clone())
    }
    fn is_empty(&self) -> bool {
        false
    }
}
/// A hand-written filter over the nodes with several conditions, which
/// share one flat list of bind values. It keeps the default
/// `to_filter_expression`, which divides that list among the conditions.
#[derive(Clone)]
struct Conditions(Vec<&'static str>, Vec<SqlValue>);
impl DatabaseFilter for Conditions {
    fn to_sql_conditions(&self) -> (Vec<String>, Vec<SqlValue>) {
        (
            self.0.iter().map(|clause| (*clause).to_owned()).collect(),
            self.1.clone(),
        )
    }
    fn is_empty(&self) -> bool {
        false
    }
}
impl ReadVisibilityFilter for Conditions {
    fn entity_type_id() -> std::any::TypeId {
        std::any::TypeId::of::<NamedNode>()
    }
    fn backend() -> DatabaseBackend {
        B::DIALECT
    }
    fn table_name() -> Option<&'static str> {
        Some(NamedNode::TABLE_NAME)
    }
}

impl<E: Entity + 'static, const BOUND: bool> ReadVisibilityFilter for Written<E, BOUND> {
    fn entity_type_id() -> std::any::TypeId {
        std::any::TypeId::of::<E>()
    }
    fn backend() -> DatabaseBackend {
        B::DIALECT
    }
    fn table_name() -> Option<&'static str> {
        BOUND.then_some(E::TABLE_NAME)
    }
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

#[cfg(feature = "mssql")]
const ADD_LINK: &str = "ALTER TABLE shadow_rows ADD link NVARCHAR(64) NULL";
#[cfg(not(feature = "mssql"))]
const ADD_LINK: &str = "ALTER TABLE shadow_rows ADD COLUMN link TEXT NULL";

const KEYS: &[(&str, &str)] = &[("id", "source_id")];
const UP: &[(&str, &str)] = &[("up", "id")];
const LINK: &[(&str, &str)] = &[("link", "id")];

/// Whether `error` is the database refusing `column` because the row's
/// table does not have it, rather than any other failure.
fn missing_column(error: &str, column: &str) -> bool {
    #[cfg(feature = "sqlite")]
    let refused = error.contains("no such column") && error.contains(column);
    #[cfg(feature = "postgres")]
    let refused =
        (error.contains("42703") || error.contains("does not exist")) && error.contains(column);
    #[cfg(feature = "mssql")]
    let refused = error.contains("Invalid column name") && error.contains(column);
    refused
}

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

/// Source policies through relations nested over the target entity itself,
/// keyed by the target's `link`, each with the sources it admits once the
/// target has the column.
fn link_shapes() -> Vec<(&'static str, ReadPredicate, &'static [&'static str])> {
    let linked =
        |target| ReadPredicate::related::<B, ShadowTarget, ShadowTarget>(LINK, target).unwrap();
    let any = || ReadPredicate::is_not_null::<B, ShadowTarget>("id").unwrap();
    vec![
        (
            "one same-entity relation",
            related(linked(any())),
            &["a2", "a3"],
        ),
        (
            "two same-entity relations",
            related(linked(linked(any()))),
            &["a3"],
        ),
        (
            "same-entity not_related",
            related(
                ReadPredicate::not_related::<B, ShadowTarget, ShadowTarget>(LINK, linked(any()))
                    .unwrap(),
            ),
            // Relations are three-valued: b2's only linked row, b1, has no
            // link of its own, so whether b1 is linked on is UNKNOWN and b2
            // is not admitted; b3's linked row is.
            &[],
        ),
    ]
}

/// Sources (`flag`): a1 (1), a2 (NULL), a3 (0). Targets, created without
/// `flag` (its control value in brackets): b1 -> a1 [0], a root;
/// b2 -> a2 [1], up b1; b3 -> a3 [NULL], up b2. Each source's flag differs
/// from its target's, in nullness too for a2 and a3. The targets also lack
/// `link` (b2 -> b1, b3 -> b2 once added), which every source has.
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
        format!(
            "CREATE TABLE {SCHEMA}.shadow_rows (id {TEXT} NOT NULL PRIMARY KEY, flag INT NULL, link {TEXT} NULL)"
        ),
        format!(
            "CREATE TABLE shadow_rows (id {TEXT} NOT NULL PRIMARY KEY, source_id {TEXT} NOT NULL, up {TEXT} NULL)"
        ),
        format!(
            "INSERT INTO {SCHEMA}.shadow_rows (id, flag, link) VALUES ('a1', 1, 'b1'), ('a2', NULL, 'b1'), ('a3', 0, 'b1')"
        ),
        "INSERT INTO shadow_rows (id, source_id, up) VALUES ('b1', 'a1', NULL), ('b2', 'a2', 'b1'), ('b3', 'a3', 'b2')"
            .to_owned(),
    ] {
        execute(&db, &statement).await;
    }

    // Drift: the target table lacks `flag`. Every read fails, and fails
    // because the database refuses the missing column.
    let mut wrong = Vec::new();
    for (name, predicate, _) in shadow_shapes() {
        let (list, count) = reads::<ShadowSource>(&db, &predicate, |row| row.id).await;
        for (read, outcome) in [
            ("list", list.map(|rows| format!("{rows:?}"))),
            ("count", count.map(|count| count.to_string())),
        ] {
            match outcome {
                Ok(admitted) => wrong.push(format!("{name}: {read} admitted {admitted}")),
                Err(error) if !missing_column(&error, "flag") => {
                    wrong.push(format!("{name}: {read} failed otherwise: {error}"))
                }
                Err(_) => {}
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));

    // Control: the target's own values decide.
    execute(&db, ADD_FLAG).await;
    execute(&db, "UPDATE shadow_rows SET flag = 0 WHERE id = 'b1'").await;
    execute(&db, "UPDATE shadow_rows SET flag = 1 WHERE id = 'b2'").await;
    for (name, predicate, expected) in shadow_shapes() {
        let (list, count) = reads::<ShadowSource>(&db, &predicate, |row| row.id).await;
        assert_eq!(list.unwrap(), expected, "{name}");
        assert_eq!(count.unwrap(), expected.len() as i64, "{name}");
    }

    // Key drift: the target table lacks `link`, the key of relations nested
    // over the target entity itself. The source table has that column.
    let mut wrong = Vec::new();
    for (name, predicate, _) in link_shapes() {
        let (list, count) = reads::<ShadowSource>(&db, &predicate, |row| row.id).await;
        for (read, outcome) in [
            ("list", list.map(|rows| format!("{rows:?}"))),
            ("count", count.map(|count| count.to_string())),
        ] {
            match outcome {
                Ok(admitted) => wrong.push(format!("{name}: {read} admitted {admitted}")),
                Err(error) if !missing_column(&error, "link") => {
                    wrong.push(format!("{name}: {read} failed otherwise: {error}"))
                }
                Err(_) => {}
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    execute(&db, ADD_LINK).await;
    execute(&db, "UPDATE shadow_rows SET link = 'b1' WHERE id = 'b2'").await;
    execute(&db, "UPDATE shadow_rows SET link = 'b2' WHERE id = 'b3'").await;
    for (name, predicate, expected) in link_shapes() {
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

/// Every read predicate over a hand-built entity whose table carries a name
/// read predicates generate is refused, whichever side it is on.
fn reserved_table_is_refused<const TABLE: usize>() {
    let node = || ReadPredicate::is_not_null::<B, NamedNode>("id").unwrap();
    misconfigured(ReadPredicate::is_not_null::<B, HandBuilt<TABLE>>("id"));
    misconfigured(ReadPredicate::is_null::<B, HandBuilt<TABLE>>("id"));
    misconfigured(ReadPredicate::column::<B, HandBuilt<TABLE>>(
        "score",
        ReadPredicateOp::Eq,
        SqlValue::Int(1),
    ));
    misconfigured(ReadPredicate::from_filter::<B, _>(&written::<
        HandBuilt<TABLE>,
    >(
        "__gom_self.score = 1",
    )));
    // As the source of a relation, where it would be read unaliased.
    misconfigured(ReadPredicate::related::<B, HandBuilt<TABLE>, NamedNode>(
        UP,
        node(),
    ));
    misconfigured(ReadPredicate::not_related::<B, HandBuilt<TABLE>, NamedNode>(UP, node()));
    // A filter that names no table builds, since it never names the row.
    // Attaching its predicate to a statement over the entity is refused.
    let unnamed =
        ReadPredicate::from_filter::<B, _>(&unbound::<HandBuilt<TABLE>>("score = 1")).unwrap();
    misconfigured(
        EntityQuery::<HandBuilt<TABLE>>::new()
            .with_read_visibility(&ReadVisibility::Complete(unnamed))
            .map(|_| ()),
    );
}

#[test]
fn table_names_read_predicates_generate_are_refused() {
    reserved_table_is_refused::<0>();
    reserved_table_is_refused::<1>();
    reserved_table_is_refused::<2>();
    reserved_table_is_refused::<3>();
    reserved_table_is_refused::<4>();
    // Any other name starting with `__gom_` is an ordinary table: aliases
    // are allocated around it.
    type Orders = HandBuilt<5>;
    assert_eq!(Orders::TABLE_NAME, "__gom_orders");
    let scored = || {
        ReadPredicate::column::<B, Orders>("score", ReadPredicateOp::Eq, SqlValue::Int(1)).unwrap()
    };
    let written =
        ReadPredicate::from_filter::<B, _>(&written::<Orders>("__gom_self.score = 1")).unwrap();
    ReadPredicate::related::<B, Orders, Orders>(UP, scored().and(written).unwrap()).unwrap();
    ReadPredicate::not_related::<B, NamedNode, Orders>(UP, scored()).unwrap();
    let closure = ReadPredicate::related_closure::<B, Orders>(UP, scored()).unwrap();
    EntityQuery::<Orders>::new()
        .with_read_visibility(&ReadVisibility::Complete(closure))
        .unwrap();
}

/// Nodes (`up`, score): n1 (root, 5), n2 (n1, 1), n3 (n2, 5). Listed: n2.
async fn nodes() -> (Database<B>, Owned) {
    let (db, owned) = database().await;
    for statement in [
        format!(
            "CREATE TABLE named_nodes (id {TEXT} NOT NULL PRIMARY KEY, up {TEXT} NULL, score INT NULL)"
        ),
        "INSERT INTO named_nodes (id, up, score) VALUES ('n1', NULL, 5), ('n2', 'n1', 1), ('n3', 'n2', 5)"
            .to_owned(),
        format!("CREATE TABLE listed_nodes (id {TEXT} NOT NULL PRIMARY KEY)"),
        "INSERT INTO listed_nodes (id) VALUES ('n2')".to_owned(),
    ] {
        execute(&db, &statement).await;
    }
    (db, owned)
}

/// The node is listed: a policy over another table, built with `related`.
fn listed() -> ReadPredicate {
    ReadPredicate::related::<B, NamedNode, ListedNode>(
        &[("id", "id")],
        ReadPredicate::is_not_null::<B, ListedNode>("id").unwrap(),
    )
    .unwrap()
}

fn score_at_least(value: i64) -> ReadPredicate {
    ReadPredicate::column::<B, NamedNode>("score", ReadPredicateOp::Gte, SqlValue::Int(value))
        .unwrap()
}

/// A hand-written filter cannot read another table or name a row of its
/// own; the structured builders express the same policy.
#[tokio::test(flavor = "multi_thread")]
async fn policies_over_another_table_use_the_structured_builders() {
    for clause in [
        // An alias spelled like a generated row alias.
        "EXISTS (SELECT 1 FROM listed_nodes AS '__gom_row_1' WHERE '__gom_row_1'.id = __gom_self.id)",
        // An alias spelled like the root table, which would capture the
        // name the governed row has at the root of a statement.
        "EXISTS (SELECT 1 FROM listed_nodes AS named_nodes WHERE named_nodes.id = __gom_self.id)",
        // A table with the name of a closure's common table expression.
        "EXISTS (SELECT 1 FROM \"__gom_closure_1\" AS listed WHERE listed.id = __gom_self.id)",
        "__gom_self.id IN (SELECT id FROM listed_nodes)",
        "__gom_self.score >= 5 UNION SELECT 1",
    ] {
        misconfigured(ReadPredicate::from_filter::<B, _>(&written::<NamedNode>(
            clause,
        )));
        misconfigured(ReadPredicate::from_filter::<B, _>(&unbound::<NamedNode>(
            clause,
        )));
    }

    let (db, _owned) = nodes().await;
    // Some node on the way up is listed.
    let any = || ReadPredicate::related_closure::<B, NamedNode>(UP, listed()).unwrap();
    let mut shapes = vec![
        ("listed", listed(), &["n2"][..]),
        ("closure", any(), &["n2", "n3"]),
    ];
    // The SQLite and PostgreSQL lane also exercises two nested closures
    // around the generated relation target.
    if B::DIALECT != DatabaseBackend::Mssql {
        // No node on the way up is listed, inside a second closure: two
        // closures nested inside one another are the most allowed.
        let nested = ReadPredicate::related_closure::<B, NamedNode>(
            UP,
            ReadPredicate::related_closure_all::<B, NamedNode>(UP, listed().not().unwrap())
                .unwrap(),
        )
        .unwrap();
        misconfigured(ReadPredicate::related_closure::<B, NamedNode>(
            UP,
            nested.clone(),
        ));
        shapes.push(("nested closures", nested, &["n1", "n2", "n3"]));
    }
    for (name, predicate, expected) in shapes {
        let (list, count) = reads::<NamedNode>(&db, &predicate, |row| row.id).await;
        assert_eq!(list.unwrap(), expected, "{name}");
        assert_eq!(count.unwrap(), expected.len() as i64, "{name}");
    }
    #[cfg(not(feature = "mssql"))]
    db.pool().close().await;
}

/// Closures beside one another do not count as nested, whatever wraps them.
#[tokio::test(flavor = "multi_thread")]
async fn sibling_closures_are_not_nested_closures() {
    let (db, _owned) = nodes().await;
    let up = |target| ReadPredicate::related_closure::<B, NamedNode>(UP, target).unwrap();
    let all = |target| ReadPredicate::related_closure_all::<B, NamedNode>(UP, target).unwrap();
    // Three sibling closures, negated: every node has an ancestor-or-self
    // scoring 5 or more, so only the rest of the conjunction decides.
    let three = up(score_at_least(5))
        .and(all(score_at_least(1)))
        .unwrap()
        .and(up(score_at_least(9)))
        .unwrap();
    let negated = three.clone().not().unwrap();
    // Two closures nested inside one another are allowed, a third refused,
    // also under a negation.
    let nested = up(up(score_at_least(5)).not().unwrap());
    misconfigured(ReadPredicate::related_closure::<B, NamedNode>(
        UP,
        nested.clone(),
    ));
    misconfigured(ReadPredicate::related_closure_all::<B, NamedNode>(
        UP,
        nested.not().unwrap(),
    ));
    for (name, predicate, expected) in [
        ("three siblings", three, &[][..]),
        ("negated siblings", negated, &["n1", "n2", "n3"]),
    ] {
        let (list, count) = reads::<NamedNode>(&db, &predicate, |row| row.id).await;
        assert_eq!(list.unwrap(), expected, "{name}");
        assert_eq!(count.unwrap(), expected.len() as i64, "{name}");
    }
    #[cfg(not(feature = "mssql"))]
    db.pool().close().await;
}

/// Marked rows (`__gom_self`, `__gom_row_1`): m1 -> n1 (1, 0), m2 -> n2
/// (0, 1), m3 -> n3 (NULL, 1). The policy tests `__gom_self`; a row name
/// written over the column name would test `__gom_row_1` instead.
#[tokio::test(flavor = "multi_thread")]
async fn a_column_named_like_the_row_qualifier_is_still_that_column() {
    let (db, _owned) = nodes().await;
    #[cfg(feature = "mssql")]
    let columns = "[__gom_self] INT NULL, [__gom_row_1] INT NULL";
    #[cfg(not(feature = "mssql"))]
    let columns = "__gom_self INT NULL, __gom_row_1 INT NULL";
    execute(
        &db,
        &format!(
            "CREATE TABLE marked_rows (id {TEXT} NOT NULL PRIMARY KEY, source_id {TEXT} NOT NULL, {columns})"
        ),
    )
    .await;
    execute(
        &db,
        "INSERT INTO marked_rows (id, source_id, __gom_self, __gom_row_1) VALUES ('m1', 'n1', 1, 0), ('m2', 'n2', 0, 1), ('m3', 'n3', NULL, 1)",
    )
    .await;
    let marked = || {
        ReadPredicate::from_filter::<B, _>(&MarkedRowWhereInput {
            marker: Some(IntFilter {
                eq: Some(1),
                ..Default::default()
            }),
            ..Default::default()
        })
        .unwrap()
    };
    let (list, count) = reads::<MarkedRow>(&db, &marked(), |row| row.id).await;
    assert_eq!(list.unwrap(), ["m1"]);
    assert_eq!(count.unwrap(), 1);
    for (name, predicate, expected) in [
        (
            "related",
            ReadPredicate::related::<B, NamedNode, MarkedRow>(KEYS, marked()).unwrap(),
            &["n1"][..],
        ),
        (
            "not_related",
            ReadPredicate::not_related::<B, NamedNode, MarkedRow>(KEYS, marked()).unwrap(),
            &["n2"],
        ),
        (
            "related not",
            ReadPredicate::related::<B, NamedNode, MarkedRow>(KEYS, marked().not().unwrap())
                .unwrap(),
            &["n2"],
        ),
    ] {
        let (list, count) = reads::<NamedNode>(&db, &predicate, |row| row.id).await;
        assert_eq!(list.unwrap(), expected, "{name}");
        assert_eq!(count.unwrap(), expected.len() as i64, "{name}");
    }
    #[cfg(not(feature = "mssql"))]
    db.pool().close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn hand_written_filters_are_single_row_expressions() {
    let (db, _owned) = nodes().await;
    let high = || {
        ReadPredicate::from_filter::<B, _>(&written::<NamedNode>("__gom_self.score >= 5")).unwrap()
    };
    // Text in a string is not the qualifier, and a backslash in one is a
    // character on every backend, as the generated `ESCAPE '\'` needs.
    let quoted = ReadPredicate::from_filter::<B, _>(&written::<NamedNode>(
        "__gom_self.id <> '__gom_self.id' AND __gom_self.id <> 'a\\' AND __gom_self.score >= 5",
    ))
    .unwrap();
    // A filter that names no row is read as before at the root of a
    // statement, where no enclosing row exists.
    let unqualified =
        || ReadPredicate::from_filter::<B, _>(&unbound::<NamedNode>("score >= 5")).unwrap();
    // One column through the qualifier does not bind the other.
    let mixed = || {
        ReadPredicate::from_filter::<B, _>(&written::<NamedNode>(
            "__gom_self.score >= 5 AND up IS NOT NULL",
        ))
        .unwrap()
    };
    for (name, predicate, expected) in [
        ("qualified", high(), &["n1", "n3"][..]),
        ("string", quoted, &["n1", "n3"]),
        ("unqualified at the root", unqualified(), &["n1", "n3"]),
        ("mixed at the root", mixed(), &["n3"]),
        (
            "unqualified, negated and combined at the root",
            unqualified().not().unwrap().or(high()).unwrap(),
            &["n1", "n2", "n3"],
        ),
        (
            "qualified in a relation",
            ReadPredicate::related::<B, NamedNode, NamedNode>(UP, high()).unwrap(),
            &["n2"],
        ),
        (
            "qualified in a closure",
            ReadPredicate::related_closure_all::<B, NamedNode>(UP, high()).unwrap(),
            &["n1"],
        ),
    ] {
        let (list, count) = reads::<NamedNode>(&db, &predicate, |row| row.id).await;
        assert_eq!(list.unwrap(), expected, "{name}");
        assert_eq!(count.unwrap(), expected.len() as i64, "{name}");
    }

    // In a correlated subquery every column must go through the qualifier:
    // an unqualified column could resolve against an outer row.
    let targets: [&dyn Fn() -> ReadPredicate; 6] = [
        &unqualified,
        &mixed,
        &|| unqualified().not().unwrap(),
        &|| unqualified().and(score_at_least(1)).unwrap(),
        &|| score_at_least(1).or(mixed()).unwrap(),
        &|| {
            ReadPredicate::from_filter::<B, _>(&written::<NamedNode>(
                "named_nodes.score >= 5 AND __gom_self.up IS NULL",
            ))
            .unwrap()
        },
    ];
    for target in targets {
        misconfigured(ReadPredicate::related::<B, NamedNode, NamedNode>(
            UP,
            target(),
        ));
        misconfigured(ReadPredicate::not_related::<B, NamedNode, NamedNode>(
            UP,
            target(),
        ));
        misconfigured(ReadPredicate::related_closure::<B, NamedNode>(UP, target()));
        misconfigured(ReadPredicate::related_closure_all::<B, NamedNode>(
            UP,
            target(),
        ));
    }

    // The qualifier needs the filter's table.
    misconfigured(ReadPredicate::from_filter::<B, _>(&unbound::<NamedNode>(
        "__gom_self.score >= 5",
    )));
    for clause in [
        // Not the qualifier of a column: a column, or the whole row.
        "__gom_self = 1",
        "__gom_self.score >= 5 AND f(__gom_self) = 1",
        // A delimited spelling of the qualifier or of a generated alias.
        "\"__gom_self\".score >= 5",
        "\"__gom_row_1\".score >= 5",
        // Comments.
        "__gom_self.score >= 5 /* it's fine */",
        "__gom_self.score >= 5 -- it's fine\n AND __gom_self.up IS NULL",
        "__gom_self.score >= 5 -- to the end",
        // String continuation.
        "__gom_self.id <> 'a' 'b'",
        "__gom_self.id <> 'a'\n'b'",
        // A clause that would close the group it is composed into, or its
        // own, or end the statement.
        "__gom_self.score >= 5) OR (1 = 1",
        "(__gom_self.score >= 5",
        "__gom_self.score >= 5; DELETE",
        // A string where an identifier stands.
        "'named_nodes'.score >= 5",
        // Other statements' keywords, and whitespace only the database reads.
        "__gom_self.score IN (VALUES (5))",
        "__gom_self.score >= 5 AND\u{a0}__gom_self.up IS NULL",
        // Unclosed text.
        "__gom_self.score >= 5 AND __gom_self.id <> 'open",
        "__gom_self.score >= 5 AND \"open = 1",
    ] {
        misconfigured(ReadPredicate::from_filter::<B, _>(&written::<NamedNode>(
            clause,
        )));
    }
    // A malformed clause never reaches a composition: it cannot be built,
    // so it cannot widen a tenant predicate it would be combined with.
    let tenant = score_at_least(5);
    for malformed in ["1 = 1) OR (1 = 1", "1 = 0) OR ((1 = 1"] {
        misconfigured(
            ReadPredicate::from_filter::<B, _>(&written::<NamedNode>(malformed))
                .and_then(|malformed| tenant.clone().and(malformed)),
        );
        misconfigured(
            ReadPredicate::from_filter::<B, _>(&unbound::<NamedNode>(malformed))
                .and_then(|malformed| tenant.clone().and(malformed)),
        );
    }
    // A word that is no keyword of the dialect, or not one where it stands,
    // could name a column: accepted at the root, refused in a subquery.
    for clause in [
        "__gom_self.score >= 5 AND unknown = 0",
        "__gom_self.score >= 5 AND escape = 0",
        "__gom_self.score >= 5 AND __gom_self.up = end",
    ] {
        let predicate = ReadPredicate::from_filter::<B, _>(&written::<NamedNode>(clause)).unwrap();
        misconfigured(ReadPredicate::related::<B, NamedNode, NamedNode>(
            UP,
            predicate.clone(),
        ));
        misconfigured(ReadPredicate::related_closure::<B, NamedNode>(
            UP, predicate,
        ));
    }
    // A number may start with its dot, also after a keyword.
    let fraction = ReadPredicate::from_filter::<B, _>(&written::<NamedNode>(
        "__gom_self.score BETWEEN .5 AND 5.5 AND __gom_self.score > .25",
    ))
    .unwrap();
    let (list, _) = reads::<NamedNode>(
        &db,
        &ReadPredicate::related::<B, NamedNode, NamedNode>(UP, fraction).unwrap(),
        |row| row.id,
    )
    .await;
    assert_eq!(list.unwrap(), ["n2", "n3"]);

    // Placeholders: only the anonymous `?`, one per bind value, so that a
    // clause composes with the binds of the predicates around it.
    for (clause, values) in [
        ("__gom_self.score = ?1", vec![SqlValue::Int(5)]),
        ("__gom_self.score = $1", vec![SqlValue::Int(5)]),
        ("__gom_self.score = @p1", vec![SqlValue::Int(5)]),
        // `@name` is a variable on SQL Server and a parameter on SQLite; on
        // PostgreSQL `@` is an operator, so `$name` stands in there.
        (
            if B::DIALECT == DatabaseBackend::Postgres {
                "__gom_self.score = $other"
            } else {
                "__gom_self.score = @score"
            },
            vec![],
        ),
        ("__gom_self.score = $score", vec![]),
        // A placeholder with no value for it.
        ("__gom_self.score = ?", vec![]),
        (
            "__gom_self.score >= ? AND __gom_self.score <= ?",
            vec![SqlValue::Int(1)],
        ),
    ] {
        let built = ReadPredicate::from_filter::<B, _>(&written_with::<NamedNode>(clause, values))
            .and_then(|written| tenant.clone().and(written));
        misconfigured(built);
    }
    // With matching arity the clause's binds are numbered in order with
    // those around it.
    let ranged = ReadPredicate::from_filter::<B, _>(&written_with::<NamedNode>(
        "__gom_self.score >= ? AND __gom_self.score <= ?",
        vec![SqlValue::Int(2), SqlValue::Int(9)],
    ))
    .unwrap();
    let composed = score_at_least(1)
        .and(ranged)
        .unwrap()
        .and(score_at_least(5))
        .unwrap();
    let (sql, binds) = EntityQuery::<NamedNode>::new()
        .with_read_visibility(&ReadVisibility::Complete(composed.clone()))
        .unwrap()
        .build_delete_sql();
    assert_eq!(binds, [1, 2, 9, 5].map(SqlValue::Int).to_vec(), "{sql}");
    let numbered = |index: usize| match B::DIALECT {
        DatabaseBackend::Postgres => format!("${index}"),
        DatabaseBackend::Mssql => format!("@P{index}"),
        _ => "?".to_owned(),
    };
    let positions = (1..=4)
        .map(|index| {
            sql.find(&numbered(index))
                .unwrap_or_else(|| panic!("{sql}"))
        })
        .collect::<Vec<_>>();
    if B::DIALECT != DatabaseBackend::Sqlite {
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{sql}");
        assert!(!sql.contains(&numbered(5)) && !sql.contains('?'), "{sql}");
    } else {
        assert_eq!(sql.matches('?').count(), 4, "{sql}");
    }
    let (list, count) = reads::<NamedNode>(&db, &composed, |row| row.id).await;
    assert_eq!(list.unwrap(), ["n1", "n3"]);
    assert_eq!(count.unwrap(), 2);

    // Several conditions share one flat bind list, which the default
    // adapter divides by placeholder count. A surplus value would be
    // dropped silently and a missing one would shift the rest, so the
    // total is checked first.
    let conditions = || {
        vec![
            "__gom_self.score >= ?",
            "__gom_self.score <= ? AND __gom_self.id <> '?'",
        ]
    };
    for values in [
        // Surplus: the third value belongs to no placeholder.
        vec![SqlValue::Int(2), SqlValue::Int(9), SqlValue::Int(42)],
        // Deficit.
        vec![SqlValue::Int(2)],
        vec![],
    ] {
        let filter = Conditions(conditions(), values);
        misconfigured(ReadPredicate::from_filter::<B, _>(&filter));
        misconfigured(
            ReadPredicate::from_filter::<B, _>(&filter)
                .and_then(|written| tenant.clone().and(written)),
        );
    }
    let matching = ReadPredicate::from_filter::<B, _>(&Conditions(
        conditions(),
        vec![SqlValue::Int(2), SqlValue::Int(9)],
    ))
    .unwrap();
    let composed = score_at_least(1).and(matching).unwrap();
    let (sql, binds) = EntityQuery::<NamedNode>::new()
        .with_read_visibility(&ReadVisibility::Complete(composed.clone()))
        .unwrap()
        .build_delete_sql();
    assert_eq!(binds, [1, 2, 9].map(SqlValue::Int).to_vec(), "{sql}");
    if B::DIALECT != DatabaseBackend::Sqlite {
        let positions = (1..=3)
            .map(|index| {
                sql.find(&numbered(index))
                    .unwrap_or_else(|| panic!("{sql}"))
            })
            .collect::<Vec<_>>();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{sql}");
        assert!(!sql.contains(&numbered(4)), "{sql}");
    }
    let (list, count) = reads::<NamedNode>(&db, &composed, |row| row.id).await;
    assert_eq!(list.unwrap(), ["n1", "n3"]);
    assert_eq!(count.unwrap(), 2);
    // PostgreSQL: Unicode-escape strings and identifiers.
    for clause in ["__gom_self.id <> U&'\\0061'", "U&\"score\" >= 5"] {
        let predicate = ReadPredicate::from_filter::<B, _>(&written::<NamedNode>(clause));
        if B::DIALECT == DatabaseBackend::Postgres {
            misconfigured(predicate);
        }
    }
    #[cfg(not(feature = "mssql"))]
    db.pool().close().await;
}

/// Every SQLite table has an implicit row id named `rowid`, `_rowid_` and
/// `oid`. If a declared column of that name were missing from the table,
/// SQLite would read the row id instead of failing the statement, so there
/// the names are not policy columns even when declared. Elsewhere a
/// declared `oid` (r1: 7, r2: 1) is an ordinary column.
#[tokio::test(flavor = "multi_thread")]
async fn sqlite_implicit_row_ids_are_not_policy_columns() {
    let sqlite = B::DIALECT == DatabaseBackend::Sqlite;
    for column in ["rowid", "_rowid_", "oid", "ROWID"] {
        // Undeclared: refused on every backend.
        misconfigured(ReadPredicate::column::<B, NamedNode>(
            column,
            ReadPredicateOp::Eq,
            SqlValue::Int(1),
        ));
        misconfigured(ReadPredicate::is_null::<B, NamedNode>(column));
        misconfigured(ReadPredicate::related::<B, NamedNode, NamedNode>(
            &[(column, "id")],
            score_at_least(1),
        ));
        let written = ReadPredicate::from_filter::<B, _>(&written::<NamedNode>(match column {
            "rowid" => "__gom_self.rowid = 1",
            "_rowid_" => "__gom_self._rowid_ = 1",
            "oid" => "__gom_self.\"oid\" = 1",
            _ => "ROWID = 1",
        }));
        if sqlite {
            misconfigured(written);
        }
    }
    // Declared: still refused on SQLite, as a column, a null test and a key.
    let declared =
        || ReadPredicate::column::<B, OidRow>("oid", ReadPredicateOp::Eq, SqlValue::Int(7));
    let keyed = || {
        ReadPredicate::related::<B, OidRow, OidRow>(
            &[("oid", "oid")],
            ReadPredicate::is_not_null::<B, OidRow>("id").unwrap(),
        )
    };
    if sqlite {
        misconfigured(declared());
        misconfigured(ReadPredicate::is_not_null::<B, OidRow>("oid"));
        misconfigured(keyed());
        misconfigured(ReadPredicate::related_closure::<B, OidRow>(
            &[("oid", "id")],
            ReadPredicate::is_not_null::<B, OidRow>("id").unwrap(),
        ));
        return;
    }

    let (db, _owned) = database().await;
    execute(
        &db,
        &format!("CREATE TABLE oid_rows (id {TEXT} NOT NULL PRIMARY KEY, oid INT NULL)"),
    )
    .await;
    execute(
        &db,
        "INSERT INTO oid_rows (id, oid) VALUES ('r1', 7), ('r2', 1)",
    )
    .await;
    let related =
        ReadPredicate::related::<B, OidRow, OidRow>(&[("id", "id")], declared().unwrap()).unwrap();
    for predicate in [declared().unwrap(), related] {
        let (list, count) = reads::<OidRow>(&db, &predicate, |row| row.id).await;
        assert_eq!(list.unwrap(), ["r1"]);
        assert_eq!(count.unwrap(), 1);
    }
    let (list, _) = reads::<OidRow>(&db, &keyed().unwrap(), |row| row.id).await;
    assert_eq!(list.unwrap(), ["r1", "r2"]);
    #[cfg(not(feature = "mssql"))]
    db.pool().close().await;
}

/// PostgreSQL folds a column declared without quotes to lower case, and a
/// table may hold `"OwnerId"` beside it. Rows (`ownerid`, `"OwnerId"`):
/// c1 (1, 5), c2 (7, 7). Nodes score 5, 1, 5.
#[cfg(feature = "postgres")]
#[tokio::test(flavor = "multi_thread")]
async fn native_predicates_address_the_column_generated_ddl_creates() {
    let (db, _owned) = nodes().await;
    execute(
        &db,
        "CREATE TABLE case_rows (id TEXT NOT NULL PRIMARY KEY, OwnerId INT NULL, \"OwnerId\" INT NULL)",
    )
    .await;
    execute(
        &db,
        "INSERT INTO case_rows (id, ownerid, \"OwnerId\") VALUES ('c1', 1, 5), ('c2', 7, 7)",
    )
    .await;
    let count = |predicate: ReadPredicate| {
        let db = db.clone();
        async move {
            EntityQuery::<CaseRow>::new()
                .with_read_visibility(&ReadVisibility::Complete(predicate))
                .unwrap()
                .count(&db)
                .await
                .unwrap()
        }
    };
    let id = |id: &str| {
        ReadPredicate::column::<B, CaseRow>("id", ReadPredicateOp::Eq, SqlValue::String(id.into()))
            .unwrap()
    };
    let folded = |value| {
        ReadPredicate::column::<B, CaseRow>("OwnerId", ReadPredicateOp::Eq, SqlValue::Int(value))
            .unwrap()
    };
    let exact = |value| {
        ReadPredicate::column::<B, CaseRow>(
            "\"OwnerId\"",
            ReadPredicateOp::Eq,
            SqlValue::Int(value),
        )
        .unwrap()
    };
    // The unquoted declaration tests `ownerid`, the quoted one `"OwnerId"`.
    assert_eq!(count(folded(1).and(id("c1")).unwrap()).await, 1);
    assert_eq!(count(folded(5)).await, 0);
    assert_eq!(count(exact(5).and(id("c1")).unwrap()).await, 1);
    assert_eq!(count(exact(1)).await, 0);
    assert_eq!(
        count(ReadPredicate::is_not_null::<B, CaseRow>("OwnerId").unwrap()).await,
        2
    );
    // Relation keys and closure pointers address the same columns.
    let keyed = |column: &'static str| {
        ReadPredicate::related::<B, NamedNode, CaseRow>(
            &[("score", column)],
            ReadPredicate::is_not_null::<B, CaseRow>("id").unwrap(),
        )
        .unwrap()
    };
    for (name, predicate, expected) in [
        ("folded key", keyed("OwnerId"), &["n2"][..]),
        ("exact key", keyed("\"OwnerId\""), &["n1", "n3"]),
        (
            "folded source key",
            ReadPredicate::related::<B, CaseRow, CaseRow>(&[("OwnerId", "\"OwnerId\"")], id("c2"))
                .unwrap(),
            &[][..],
        ),
    ] {
        if name == "folded source key" {
            // c2's `ownerid` (7) equals c2's `"OwnerId"` (7); c1's (1) no row's.
            assert_eq!(count(predicate).await, 1, "{name}");
            continue;
        }
        let (list, _) = reads::<NamedNode>(&db, &predicate, |row| row.id).await;
        assert_eq!(list.unwrap(), expected, "{name}");
    }
    assert_eq!(
        count(
            ReadPredicate::related_closure::<B, CaseRow>(&[("OwnerId", "\"OwnerId\"")], id("c2"))
                .unwrap()
        )
        .await,
        1
    );
    db.pool().close().await;
}

/// Asserts a repository read was refused as misconfigured authorization.
fn refused<T: std::fmt::Debug>(result: graphql_orm::Result<T>, context: &str) {
    let error = result.expect_err(context);
    let code = graphql_orm::graphql::errors::graphql_error_from_sqlx(error)
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.get("code"))
        .map(ToString::to_string);
    assert_eq!(
        code.as_deref(),
        Some("\"AUTHORIZATION_MISCONFIGURED\""),
        "{context}"
    );
}

/// A query's clauses and bind values are flat lists that a statement
/// numbers together. A caller clause with more or fewer values than
/// placeholders would shift the policy's binds: `score >= ?` with `[1, 42]`
/// before the policy `score <= ?` with `[5]` would test `score <= 42`.
#[tokio::test(flavor = "multi_thread")]
async fn a_miscounted_caller_filter_never_shifts_the_policy_binds() {
    let (db, _owned) = nodes().await;
    let policy = || {
        ReadVisibility::Complete(
            ReadPredicate::from_filter::<B, _>(&written_with::<NamedNode>(
                "__gom_self.score <= ?",
                vec![SqlValue::Int(4)],
            ))
            .unwrap(),
        )
    };
    let query = |clauses: &[(&str, Vec<SqlValue>)], policy_first: bool| {
        let mut query = EntityQuery::<NamedNode>::new();
        if policy_first {
            query = query.with_read_visibility(&policy()).unwrap();
        }
        for (clause, values) in clauses {
            query = query.where_values(clause, values.clone());
        }
        if !policy_first {
            query = query.with_read_visibility(&policy()).unwrap();
        }
        query
    };
    let ints = |values: &[i64]| {
        values
            .iter()
            .copied()
            .map(SqlValue::Int)
            .collect::<Vec<_>>()
    };
    for (name, clauses) in [
        // The reported case: a surplus value before the policy.
        ("surplus", vec![("score >= ?", ints(&[1, 42]))]),
        ("deficit", vec![("score >= ? AND score <> ?", ints(&[1]))]),
        ("no value", vec![("score >= ?", ints(&[]))]),
        ("no placeholder", vec![("score >= 1", ints(&[42]))]),
        // A surplus and a deficit whose totals agree still do not pair up.
        (
            "offsetting",
            vec![
                ("score >= ?", ints(&[1, 42])),
                ("score <> ? AND score <> ?", ints(&[9])),
            ],
        ),
    ] {
        for policy_first in [false, true] {
            let query = query(&clauses, policy_first);
            let context = format!("{name} (policy first: {policy_first})");
            refused(query.try_build_delete_sql(), &context);
            refused(query.clone().fetch_all(&db).await, &context);
            refused(query.count(&db).await, &context);
        }
    }
    // The caller's lists are public. Whatever is done to them before and
    // after the policy is attached, the policy keeps its own bind value:
    // here the first clause is given a second value and a later clause
    // none, so the totals agree. The caller's clauses share the caller's
    // two values; the policy still tests `score <= 4`.
    let mut shifted = EntityQuery::<NamedNode>::new();
    shifted.where_clauses.push("score >= ?".to_owned());
    shifted.values.push(SqlValue::Int(1));
    shifted.values.push(SqlValue::String("n9".into()));
    let mut shifted = shifted.with_read_visibility(&policy()).unwrap();
    shifted.where_clauses.push("id <> ?".to_owned());
    let (sql, binds) = shifted.try_build_delete_sql().unwrap();
    assert_eq!(
        binds,
        vec![
            SqlValue::Int(1),
            SqlValue::String("n9".into()),
            SqlValue::Int(4)
        ],
        "{sql}"
    );
    let rows = shifted.clone().fetch_all(&db).await.unwrap();
    assert_eq!(
        rows.into_iter().map(|row| row.id).collect::<Vec<_>>(),
        ["n2"]
    );
    assert_eq!(shifted.count(&db).await.unwrap(), 1);
    // The same lists with one value too many for the caller's clauses are
    // refused: validated on their own, apart from the policy.
    let mut surplus = EntityQuery::<NamedNode>::new()
        .with_read_visibility(&policy())
        .unwrap();
    surplus.where_clauses.push("score >= ?".to_owned());
    surplus.values.extend([SqlValue::Int(1), SqlValue::Int(42)]);
    refused(surplus.try_build_delete_sql(), "direct surplus");
    refused(surplus.clone().fetch_all(&db).await, "direct surplus");

    // A placeholder the statement does not number itself cannot stand
    // beside a policy. SQLite sends `?1`, `:name`, `@name` and `$name` as
    // written and does not advance its anonymous counter past them, so the
    // policy's `?` would read the caller's argument. PostgreSQL and SQL
    // Server renumber every `$N` and `@pN`, as generated filters write
    // them; a named variable is refused there too.
    let named: &[&str] = match B::DIALECT {
        DatabaseBackend::Sqlite => &[
            "score >= ?1",
            "score >= :low",
            "score >= @low",
            "score >= $low",
        ],
        DatabaseBackend::Postgres => &["score >= $low"],
        _ => &["score >= @low"],
    };
    for clause in named {
        for policy_first in [false, true] {
            let query = query(&[(clause, ints(&[1]))], policy_first);
            let context = format!("{clause} (policy first: {policy_first})");
            refused(query.try_build_delete_sql(), &context);
            refused(query.clone().fetch_all(&db).await, &context);
            refused(query.count(&db).await, &context);
        }
    }
    // With no policy the caller's SQL is passed through as written: SQLite
    // binds the one value to both uses of `?1`.
    if B::DIALECT == DatabaseBackend::Sqlite {
        let unpoliced = EntityQuery::<NamedNode>::new()
            .where_values("score = ?1 OR score + 4 = ?1", ints(&[5]));
        let rows = unpoliced.clone().fetch_all(&db).await.unwrap();
        assert_eq!(
            rows.into_iter().map(|row| row.id).collect::<Vec<_>>(),
            ["n1", "n2", "n3"]
        );
        assert_eq!(unpoliced.count(&db).await.unwrap(), 3);
    }

    // Clauses that pair up render in order and return the policy's rows.
    for policy_first in [false, true] {
        let query = query(
            &[
                ("score >= ?", ints(&[1])),
                (
                    "score <> ? AND id <> ?",
                    vec![SqlValue::Int(9), SqlValue::String("n9".into())],
                ),
            ],
            policy_first,
        );
        let (_, binds) = query.try_build_delete_sql().unwrap();
        assert_eq!(binds.len(), 4);
        let rows = query.clone().fetch_all(&db).await.unwrap();
        assert_eq!(
            rows.into_iter().map(|row| row.id).collect::<Vec<_>>(),
            ["n2"]
        );
        assert_eq!(query.count(&db).await.unwrap(), 1);
    }
    #[cfg(not(feature = "mssql"))]
    db.pool().close().await;
}

/// A predicate is built from one answer of the filter: the SQL that was
/// validated is the SQL that is rendered, whatever the filter says later.
#[tokio::test(flavor = "multi_thread")]
async fn a_filter_is_read_once() {
    let (db, _owned) = nodes().await;
    let calls = |filter: &Changing| filter.2.load(std::sync::atomic::Ordering::SeqCst);
    // First a valid answer, then one that would test nothing.
    let filter = Changing(
        ("__gom_self.score >= ?", vec![SqlValue::Int(5)]),
        ("1 = 1", vec![]),
        Default::default(),
    );
    let predicate = ReadPredicate::from_filter::<B, _>(&filter).unwrap();
    assert_eq!(calls(&filter), 1);
    let (list, count) = reads::<NamedNode>(&db, &predicate, |row| row.id).await;
    assert_eq!(list.unwrap(), ["n1", "n3"]);
    assert_eq!(count.unwrap(), 2);
    assert_eq!(calls(&filter), 1);
    // First a miscounted answer, then a valid one: refused on the first.
    for first in [
        (
            "__gom_self.score >= ?",
            vec![SqlValue::Int(5), SqlValue::Int(42)],
        ),
        (
            "__gom_self.score >= ? AND __gom_self.score <= ?",
            vec![SqlValue::Int(5)],
        ),
        ("__gom_self.score >= ?", vec![]),
    ] {
        let filter = Changing(
            first,
            ("__gom_self.score >= ?", vec![SqlValue::Int(5)]),
            Default::default(),
        );
        misconfigured(ReadPredicate::from_filter::<B, _>(&filter));
        assert_eq!(calls(&filter), 1);
    }
    #[cfg(not(feature = "mssql"))]
    db.pool().close().await;
}

/// A column named like an operator is an identifier after its dot, and
/// reads as that column. Rows (`like`, `glob`): w1 (1, 'abc'), w2 (2, 'xyz').
#[tokio::test(flavor = "multi_thread")]
async fn columns_named_like_operators_are_read_as_columns() {
    let (db, _owned) = database().await;
    // SQL Server reads a reserved word as a name only in brackets.
    #[cfg(feature = "mssql")]
    let (like, liked) = ("[like]", "__gom_self.[like] = ?");
    #[cfg(not(feature = "mssql"))]
    let (like, liked) = ("\"like\"", "__gom_self.like = ?");
    execute(
        &db,
        &format!(
            "CREATE TABLE word_rows (id {TEXT} NOT NULL PRIMARY KEY, {like} INT NULL, glob {TEXT} NULL)"
        ),
    )
    .await;
    execute(
        &db,
        &format!(
            "INSERT INTO word_rows (id, {like}, glob) VALUES ('w1', 1, 'abc'), ('w2', 2, 'xyz')"
        ),
    )
    .await;
    let mut filters = vec![
        written_with::<WordRow>(liked, vec![SqlValue::Int(1)]),
        written_with::<WordRow>(
            "__gom_self.glob LIKE ? AND __gom_self.glob IN ('abc', ?)",
            vec![SqlValue::String("a%".into()), SqlValue::String("q".into())],
        ),
    ];
    if B::DIALECT == DatabaseBackend::Sqlite {
        filters.push(written_with::<WordRow>(
            "__gom_self.glob GLOB ?",
            vec![SqlValue::String("a*".into())],
        ));
    }
    for filter in filters {
        let predicate = ReadPredicate::from_filter::<B, _>(&filter).unwrap();
        let related =
            ReadPredicate::related::<B, WordRow, WordRow>(&[("id", "id")], predicate.clone())
                .unwrap();
        for predicate in [predicate, related] {
            let (list, count) = reads::<WordRow>(&db, &predicate, |row| row.id).await;
            assert_eq!(list.unwrap(), ["w1"], "{}", filter.0);
            assert_eq!(count.unwrap(), 1, "{}", filter.0);
        }
    }
    #[cfg(not(feature = "mssql"))]
    db.pool().close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn handwritten_closure_targets_keep_physical_column_names() {
    let (db, _owned) = nodes().await;
    let non_ascii = if B::DIALECT == DatabaseBackend::Mssql {
        "[ü]"
    } else {
        "\"ü\""
    };
    execute(
        &db,
        &format!("ALTER TABLE named_nodes ADD {non_ascii} BIGINT NULL"),
    )
    .await;
    execute(&db, &format!("UPDATE named_nodes SET {non_ascii} = CASE WHEN id = 'n1' THEN 5 WHEN id = 'n2' THEN 1 END")).await;
    let mut filters = vec![if B::DIALECT == DatabaseBackend::Mssql {
        "__gom_self.[ü] = ?"
    } else {
        "__gom_self.\"ü\" = ?"
    }];
    if B::DIALECT == DatabaseBackend::Postgres {
        execute(&db, "ALTER TABLE named_nodes ADD \"user\" BIGINT NULL").await;
        execute(&db, "UPDATE named_nodes SET \"user\" = \"ü\"").await;
        filters.extend(["__gom_self.USER = ?", "__gom_self.User = ?"]);
    }
    if B::DIALECT == DatabaseBackend::Mssql {
        execute(&db, "ALTER TABLE named_nodes ADD [user] BIGINT NULL").await;
        execute(&db, "UPDATE named_nodes SET [user] = [ü]").await;
        filters.push("__gom_self.[user] = ? OR __gom_self.[USER] = ?");
    }
    for filter in filters {
        let operands = filter.matches('?').count();
        let target = ReadPredicate::from_filter::<B, _>(&written_with::<NamedNode>(
            filter,
            vec![SqlValue::Int(5); operands],
        ))
        .unwrap();
        // Also execute PostgreSQL's explicit FALSE fallback. Double negation
        // restores the same target with a separately checked FALSE test.
        let explicit = B::DIALECT == DatabaseBackend::Postgres && filter.contains('ü');
        let target = if explicit {
            target.not().unwrap().not().unwrap()
        } else {
            target
        };
        let any = ReadPredicate::related_closure::<B, NamedNode>(UP, target.clone()).unwrap();
        let all = ReadPredicate::related_closure_all::<B, NamedNode>(UP, target).unwrap();
        let nested = ReadPredicate::related_closure::<B, NamedNode>(UP, all.clone()).unwrap();
        let query = EntityQuery::<NamedNode>::new()
            .with_read_visibility(&ReadVisibility::Complete(any.clone()))
            .unwrap();
        let (sql, binds) = query.build_delete_sql();
        assert!(!sql.contains("_origin"), "{filter}");
        assert_eq!(
            binds.len(),
            operands
                * if B::DIALECT == DatabaseBackend::Postgres && !explicit {
                    2
                } else {
                    4
                }
        );
        for (policy, expected) in [
            (any.clone(), &["n1", "n2", "n3"][..]),
            (all, &["n1"]),
            (any.not().unwrap(), &[]),
            (nested, &["n1", "n2", "n3"]),
        ] {
            let (list, count) = reads::<NamedNode>(&db, &policy, |row| row.id).await;
            assert_eq!(list.unwrap(), expected, "{filter}");
            assert_eq!(count.unwrap(), expected.len() as i64, "{filter}");
        }
    }
    #[cfg(not(feature = "mssql"))]
    db.pool().close().await;
}
