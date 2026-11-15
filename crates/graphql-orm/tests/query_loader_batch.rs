//! Batched nested relations: the target's default order (#245), relation
//! targets without a hand-written loader trait (#285), and one outcome per
//! key group in a shared batch (#444). Static filter operand limits (#455).

// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]

use graphql_orm::async_graphql::dataloader::{DataLoader, Loader};
use graphql_orm::async_graphql::{Context, SimpleObject};
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::graphql::loaders::{
    CompositeRelationQueryKey, FederationEntityKey, FederationKeyLoader, IsolatedLoad, RelationKey,
    RelationKeyPartKind, RelationLoader,
};
use graphql_orm::graphql::orm::{query_count, reset_query_count};
use graphql_orm::prelude::*;
use graphql_orm::{DefaultBackend, graphql::orm::FilterExpression};
use std::{
    any::Any,
    sync::{Arc, Mutex},
    time::Duration,
};
#[cfg(feature = "mssql")]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

/// The relation target. Its default order is a numeric position, which
/// disagrees with the order of its text primary key (`a-10` sorts before
/// `a-2`). It has no `BatchLoadEntity` implementation.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "batch_steps",
        plural = "BatchSteps",
        auth = "none",
        default_sort = "position ASC"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "batch_steps",
        plural = "BatchSteps",
        auth = "none",
        default_sort = "position ASC"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "batch_steps",
        plural = "BatchSteps",
        auth = "none",
        schema_policy = "external_writable",
        default_sort = "[position] ASC"
    )
)]
struct Step {
    #[primary_key]
    #[sortable]
    #[filterable(type = "string")]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[filterable(type = "string")]
    list_id: String,
    #[sortable]
    #[filterable(type = "number")]
    position: i32,
    #[filterable(type = "string")]
    label: String,
    /// Text here, and one stored value that is not text (`NULL`, or bytes
    /// that are not UTF-8 on SQLite): that row fails to decode, which is how
    /// one key group's statement is made to fail.
    note: String,
}

#[derive(
    GraphQLEntity,
    GraphQLOperations,
    GraphQLRelations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "batch_lists",
        plural = "BatchLists",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "batch_lists",
        plural = "BatchLists",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "batch_lists",
        plural = "BatchLists",
        auth = "none",
        schema_policy = "external_writable"
    )
)]
#[graphql(complex)]
struct List {
    #[primary_key]
    #[sortable]
    #[filterable(type = "string")]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[graphql(skip)]
    #[relation(
        target = "Step",
        from = "id",
        to = "list_id",
        multiple,
        emit_fk = false
    )]
    steps: Vec<Step>,
}

schema_roots! { auth: "none", query_custom_ops: [], entities: [List, Step], }

// One test measures a process-global ORM counter; the tests take turns so
// another test's statements are not attributed to it.
static QUERY_COUNT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    /// Every row is visible: the relation batches.
    Open,
    /// Hidden rows are excluded in SQL: the relation batches.
    Predicate,
    /// Hidden rows are excluded by a callback: the relation reads directly.
    Callback,
}

#[derive(Clone)]
struct Policy(Arc<Mutex<Mode>>);
impl RowPolicy for Policy {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            let mode = *self.0.lock().unwrap();
            if entity != "Step" {
                return Ok(ReadVisibility::Unrestricted);
            }
            Ok(match mode {
                Mode::Open => ReadVisibility::Unrestricted,
                Mode::Predicate => {
                    let filter = StepWhereInput {
                        label: Some(StringFilter {
                            ne: Some("hidden".into()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    };
                    ReadVisibility::Complete(ReadPredicate::from_filter::<DefaultBackend, _>(
                        &filter,
                    )?)
                }
                Mode::Callback => ReadVisibility::CallbackOnly,
            })
        })
    }
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if entity != "Step" || *self.0.lock().unwrap() != Mode::Callback {
                return Ok(true);
            }
            Ok(row
                .downcast_ref::<Step>()
                .is_some_and(|step| step.label != "hidden"))
        })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}
impl EntityPolicy for Policy {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

const PARENTS: [&str; 3] = ["a", "b", "c"];
const POSITIONS: i32 = 12;

/// One step row: `(id, list, position, label)`.
type Row = (String, &'static str, i32, &'static str);

/// Three parents with twelve numeric positions each. Parent `a` also has two
/// rows that tie on a position (ordered by the primary key), and every parent
/// has one hidden row in the middle.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for parent in PARENTS {
        for position in 0..POSITIONS {
            rows.push((format!("{parent}-{position}"), parent, position, "shown"));
        }
        rows.push((format!("{parent}-hidden"), parent, 3, "hidden"));
    }
    rows.push(("a-5-tie".into(), "a", 5, "shown"));
    rows.push(("a-05-tie".into(), "a", 5, "shown"));
    rows
}

/// Ids of one parent's steps in the declared default order, with the primary
/// key as the final tie-break, or in descending position when `descending`.
fn expected(parent: &str, hidden: bool, descending: bool) -> Vec<String> {
    let mut rows = rows()
        .into_iter()
        .filter(|row| row.1 == parent && (hidden || row.3 != "hidden"))
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        let position = left.2.cmp(&right.2);
        let position = if descending {
            position.reverse()
        } else {
            position
        };
        position.then_with(|| left.0.cmp(&right.0))
    });
    rows.into_iter().map(|row| row.0).collect()
}

struct Fixture {
    db: Database,
    mode: Arc<Mutex<Mode>>,
    #[cfg(feature = "mssql")]
    server: owned_mssql::OwnedSqlServer,
}

impl Fixture {
    async fn start(name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let _ = name;
        #[cfg(feature = "sqlite")]
        let mut db = Database::connect_sqlite("sqlite::memory:").await?;
        #[cfg(feature = "postgres")]
        let mut db = Database::new(owned_postgres::OwnedPostgres::pool(name, 2).await?);
        #[cfg(feature = "mssql")]
        let server = owned_mssql::OwnedSqlServer::start().await?;
        #[cfg(feature = "mssql")]
        let mut db = Database::<MssqlBackend>::builder(
            graphql_orm::db::mssql::MssqlPool::with_max_connections_external_writable(
                graphql_orm::tiberius::Config::from_ado_string(&server.connection_string)?,
                2,
            ),
        )
        .schema_policy(SchemaPolicy::ExternalWritable)
        .build();
        for statement in [
            "CREATE TABLE batch_lists (id VARCHAR(64) PRIMARY KEY)",
            "CREATE TABLE batch_steps (id VARCHAR(64) PRIMARY KEY, list_id VARCHAR(64) NOT NULL, \
             position INTEGER NOT NULL, label VARCHAR(64) NOT NULL, note VARCHAR(64) NULL)",
            "INSERT INTO batch_lists VALUES ('a')",
            "INSERT INTO batch_lists VALUES ('b')",
            "INSERT INTO batch_lists VALUES ('c')",
            "INSERT INTO batch_lists VALUES ('empty')",
            "INSERT INTO batch_lists VALUES ('broken')",
            // The row that cannot be decoded belongs to its own parent.
            #[cfg(feature = "sqlite")]
            "INSERT INTO batch_steps VALUES ('broken-0', 'broken', 0, 'broken', x'ff')",
            #[cfg(not(feature = "sqlite"))]
            "INSERT INTO batch_steps VALUES ('broken-0', 'broken', 0, 'broken', NULL)",
        ] {
            sql(&db, statement).await?;
        }
        for (id, list, position, label) in rows() {
            sql(
                &db,
                &format!(
                    "INSERT INTO batch_steps VALUES ('{id}', '{list}', {position}, '{label}', 'n')"
                ),
            )
            .await?;
        }
        db = db.with_authorized_scan_config(AuthorizedScanConfig::new([17; 32], 8, 512));
        let mode = Arc::new(Mutex::new(Mode::Open));
        db.set_row_policy(Policy(mode.clone()));
        db.set_entity_policy(Policy(mode.clone()));
        Ok(Self {
            db,
            mode,
            #[cfg(feature = "mssql")]
            server,
        })
    }

    fn finish(self) -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(feature = "mssql")]
        {
            let mut server = self.server;
            server.cleanup()?;
        }
        Ok(())
    }
}

async fn sql(db: &Database, statement: &str) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(not(feature = "mssql"))]
    graphql_orm::sqlx::query(statement)
        .execute(db.pool())
        .await?;
    #[cfg(feature = "mssql")]
    db.pool().fetch_rows(statement, &[]).await?;
    Ok(())
}

/// The generated roots with no `DataLoader` registered: every relation reads
/// through the direct path.
fn unbatched_schema(db: &Database) -> AppSchema {
    async_graphql::Schema::build(
        QueryRoot::default(),
        MutationRoot::default(),
        SubscriptionRoot::default(),
    )
    .data(db.clone())
    .finish()
}

fn steps_query(arguments: &str) -> String {
    format!(
        "{{ batchLists(where: {{ id: {{ ne: \"broken\" }} }}) {{ edges {{ node {{ id \
         steps{arguments} {{ edges {{ node {{ id position }} }} \
         pageInfo {{ totalCount hasNextPage hasPreviousPage }} }} }} }} }} }}"
    )
}

/// Per parent: the step ids, total count and page flags of one response.
fn pages(
    response: async_graphql::Response,
) -> std::collections::BTreeMap<String, (Vec<String>, i64, bool, bool)> {
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    data["batchLists"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| {
            let node = &edge["node"];
            let steps = &node["steps"];
            (
                node["id"].as_str().unwrap().to_string(),
                (
                    steps["edges"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|edge| edge["node"]["id"].as_str().unwrap().to_string())
                        .collect(),
                    steps["pageInfo"]["totalCount"].as_i64().unwrap(),
                    steps["pageInfo"]["hasNextPage"].as_bool().unwrap(),
                    steps["pageInfo"]["hasPreviousPage"].as_bool().unwrap(),
                ),
            )
        })
        .collect()
}

#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires owned disposable database"
)]
async fn batched_relations_use_the_target_default_order() -> Result<(), Box<dyn std::error::Error>>
{
    let _turn = QUERY_COUNT_LOCK.lock().await;
    let fixture = Fixture::start("relation-default-order").await?;
    let batched = schema_builder(fixture.db.clone()).finish();
    let unbatched = unbatched_schema(&fixture.db);

    for mode in [Mode::Open, Mode::Predicate, Mode::Callback] {
        *fixture.mode.lock().unwrap() = mode;
        let hidden = mode == Mode::Open;
        for (order, descending) in [("", false), ("orderBy: { position: DESC }", true)] {
            // Unpaged, then three pages of five with a partial last page.
            let windows = [
                None,
                Some((5, 0)),
                Some((5, 5)),
                Some((5, 10)),
                Some((5, 15)),
            ];
            for window in windows {
                let page = window
                    .map(|(limit, offset)| format!("page: {{ limit: {limit}, offset: {offset} }}"));
                let arguments = [Some(order.to_string()), page]
                    .into_iter()
                    .flatten()
                    .filter(|argument| !argument.is_empty())
                    .collect::<Vec<_>>()
                    .join(", ");
                let arguments = if arguments.is_empty() {
                    String::new()
                } else {
                    format!("({arguments})")
                };
                let query = steps_query(&arguments);

                reset_query_count();
                let from_loader = pages(batched.execute(query.clone()).await);
                let batched_statements = query_count();
                let direct = pages(unbatched.execute(query.clone()).await);
                assert_eq!(
                    from_loader, direct,
                    "{mode:?} `{arguments}`: batched and direct reads agree"
                );
                if mode != Mode::Callback {
                    // One parent list, and one count and one row statement
                    // for every parent's steps together.
                    assert!(
                        batched_statements <= 3,
                        "{mode:?} `{arguments}`: {batched_statements} statements"
                    );
                }

                for parent in PARENTS.into_iter().chain(["empty"]) {
                    let all = expected(parent, hidden, descending);
                    let (limit, offset) = window.unwrap_or((usize::MAX, 0));
                    let want = all
                        .iter()
                        .skip(offset)
                        .take(limit)
                        .cloned()
                        .collect::<Vec<_>>();
                    let (ids, total, has_next, has_previous) = &from_loader[parent];
                    assert_eq!(ids, &want, "{mode:?} `{arguments}` parent {parent}");
                    assert_eq!(*total, all.len() as i64, "{mode:?} `{arguments}` {parent}");
                    assert_eq!(
                        *has_next,
                        offset.saturating_add(want.len()) < all.len(),
                        "{mode:?} `{arguments}` {parent}"
                    );
                    assert_eq!(*has_previous, offset > 0, "{mode:?} `{arguments}` {parent}");
                }
            }
        }
    }

    // The declared order is numeric: position 2 comes before position 10,
    // although the primary key `a-10` sorts before `a-2`.
    *fixture.mode.lock().unwrap() = Mode::Open;
    let first = pages(batched.execute(steps_query("(page: { limit: 4 })")).await);
    assert_eq!(first["a"].0, ["a-0", "a-1", "a-2", "a-3"]);
    assert_eq!(first["b"].0, ["b-0", "b-1", "b-2", "b-3"]);
    fixture.finish()
}

fn step_key(parent: &str, filter: Option<(&str, FilterExpression)>) -> CompositeRelationQueryKey {
    let values = vec![SqlValue::String(parent.to_string())];
    CompositeRelationQueryKey {
        relation: "List::steps",
        parent_key: RelationKey::from_sql_values(&values),
        parent_values: values,
        fk_columns: vec!["list_id"],
        key_part_kinds: vec![RelationKeyPartKind::String],
        where_signature: filter.as_ref().map(|(signature, _)| signature.to_string()),
        order_signature: None,
        page_signature: None,
        filter: filter.map(|(_, filter)| filter),
        sorts: Vec::new(),
        pagination: None,
        auth_context: None,
    }
}

#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires owned disposable database"
)]
async fn a_failing_key_group_fails_only_its_own_keys() -> Result<(), Box<dyn std::error::Error>> {
    let _turn = QUERY_COUNT_LOCK.lock().await;
    let fixture = Fixture::start("relation-group-isolation").await?;
    let loader = RelationLoader::<Step, DefaultBackend>::new(fixture.db.clone());

    // Three groups in one loader call. The second selects a row that cannot
    // be decoded; the third names a column that does not exist.
    let shown =
        FilterExpression::trusted_fragment("label = ?", vec![SqlValue::String("shown".into())]);
    let good = step_key("a", Some(("shown", shown)));
    let undecodable = step_key("broken", None);
    let invalid = step_key(
        "b",
        Some((
            "invalid",
            FilterExpression::trusted_fragment("no_such_column = ?", vec![SqlValue::Int(1)]),
        )),
    );
    let keys = [good.clone(), undecodable.clone(), invalid.clone()].map(IsolatedLoad);
    let loaded = loader.load(&keys).await?;
    let ids = loaded[&IsolatedLoad(good.clone())]
        .as_ref()
        .map_err(String::clone)?
        .entities
        .iter()
        .map(|step| step.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(ids, expected("a", false, false));
    assert!(loaded[&IsolatedLoad(undecodable.clone())].is_err());
    assert!(loaded[&IsolatedLoad(invalid.clone())].is_err());

    // Keys that share their signature strings but not the filter they carry
    // are different queries: each is run with its own filter (#541).
    let labelled = |label: &str| {
        step_key(
            "a",
            Some((
                "same signature",
                FilterExpression::trusted_fragment(
                    "label = ?",
                    vec![SqlValue::String(label.to_string())],
                ),
            )),
        )
    };
    let (shown_key, hidden_key) = (labelled("shown"), labelled("hidden"));
    assert_ne!(shown_key, hidden_key);
    let loaded = loader
        .load(&[shown_key.clone(), hidden_key.clone()].map(IsolatedLoad))
        .await?;
    let count = |key: &CompositeRelationQueryKey| {
        loaded[&IsolatedLoad(key.clone())]
            .as_ref()
            .map(|result| result.entities.len())
            .map_err(String::clone)
    };
    assert_eq!(count(&shown_key)?, expected("a", false, false).len());
    assert_eq!(count(&hidden_key)?, 1);

    // A relation keyed by the column its target orders by default (a to-one
    // relation to a primary key): the default order adds no second term for
    // that column, which SQL Server would refuse.
    let lists = RelationLoader::<List, DefaultBackend>::new(fixture.db.clone());
    let list_key = |id: &str| {
        let values = vec![SqlValue::String(id.to_string())];
        CompositeRelationQueryKey {
            relation: "Step::list",
            parent_key: RelationKey::from_sql_values(&values),
            parent_values: values,
            fk_columns: vec![<List as DatabaseEntity>::PRIMARY_KEY],
            key_part_kinds: vec![RelationKeyPartKind::String],
            where_signature: None,
            order_signature: None,
            page_signature: None,
            filter: None,
            sorts: Vec::new(),
            pagination: None,
            auth_context: None,
        }
    };
    let loaded = lists
        .load(&[list_key("a"), list_key("b")].map(IsolatedLoad))
        .await?;
    for id in ["a", "b"] {
        let result = loaded[&IsolatedLoad(list_key(id))]
            .as_ref()
            .map_err(String::clone)?;
        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.entities[0].id, id);
    }

    // The plain key keeps its all-or-nothing contract: its value type has no
    // place for a per-key error.
    assert!(
        Loader::<CompositeRelationQueryKey>::load(&loader, &[good.clone(), invalid])
            .await
            .is_err()
    );

    // Federation keys follow the same rule.
    let federation = FederationKeyLoader::<Step, DefaultBackend>::new(fixture.db.clone());
    let entity_key = |column: &'static str, id: &str| {
        let values = vec![SqlValue::String(id.to_string())];
        FederationEntityKey {
            key: column,
            columns: vec![column],
            key_part_kinds: vec![RelationKeyPartKind::String],
            representation: RelationKey::from_projected_parts(
                vec![id.to_string()],
                &[RelationKeyPartKind::String],
            ),
            values,
            auth_context: None,
        }
    };
    let found = entity_key("id", "a-7");
    let missing_column = entity_key("no_such_column", "a-7");
    let loaded = federation
        .load(&[found.clone(), missing_column.clone()].map(IsolatedLoad))
        .await?;
    let entity = loaded[&IsolatedLoad(found)]
        .as_ref()
        .map_err(String::clone)?
        .as_ref()
        .expect("the key resolves");
    assert_eq!(entity.position, 7);
    assert!(loaded[&IsolatedLoad(missing_column)].is_err());

    // Two concurrent requests through one schema-wide loader. The long delay
    // puts both in the same batch; the failing one must not fail the other.
    let schema = schema_builder(fixture.db.clone())
        .data(
            DataLoader::new(
                RelationLoader::<Step, DefaultBackend>::new(fixture.db.clone()),
                tokio::spawn,
            )
            .delay(Duration::from_millis(250)),
        )
        .finish();
    let request = |list: &str, limit: i32| {
        schema.execute(format!(
            "{{ list(id: \"{list}\") {{ steps(page: {{ limit: {limit} }}) \
             {{ edges {{ node {{ id }} }} }} }} }}"
        ))
    };
    let (healthy, failing) = tokio::join!(request("a", 2), request("broken", 3));
    assert!(!failing.errors.is_empty(), "{failing:?}");
    assert!(healthy.errors.is_empty(), "{:?}", healthy.errors);
    let healthy = healthy.data.into_json()?;
    assert_eq!(healthy["list"]["steps"]["edges"][1]["node"]["id"], "a-1");
    fixture.finish()
}

#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires owned disposable database"
)]
async fn filter_lists_and_patterns_are_bounded() -> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::filters::{MAX_FILTER_LIST_OPERANDS, MAX_FILTER_PATTERN_LENGTH};
    let _turn = QUERY_COUNT_LOCK.lock().await;
    let fixture = Fixture::start("filter-operand-limits").await?;
    let schema = schema_builder(fixture.db.clone()).finish();
    let code = |response: &async_graphql::Response| {
        response.errors.first().and_then(|error| {
            error
                .extensions
                .as_ref()
                .and_then(|extensions| extensions.get("code"))
                .map(|code| code.to_string())
        })
    };
    let check = |response: async_graphql::Response, over: bool, what: &str| {
        if over {
            assert_eq!(
                code(&response).as_deref(),
                Some("\"INVALID_INPUT\""),
                "{what}: {response:?}"
            );
        } else {
            assert!(response.errors.is_empty(), "{what}: {:?}", response.errors);
        }
    };

    for over in [false, true] {
        let size = MAX_FILTER_LIST_OPERANDS + usize::from(over);
        // A list at the limit runs as one statement on every backend,
        // together with the statement's other bound values.
        let mut ids = vec!["\"a-7\"".to_string(); size - 1];
        ids.push("\"a-8\"".into());
        let ids = ids.join(", ");
        let positions = vec!["7"; size].join(", ");
        for filter in [
            format!("id: {{ inList: [{ids}] }}"),
            format!("id: {{ notIn: [{ids}] }}, listId: {{ eq: \"a\" }}"),
            format!("position: {{ inList: [{positions}] }}, listId: {{ eq: \"a\" }}"),
            format!("position: {{ notIn: [{positions}] }}, listId: {{ eq: \"a\" }}"),
            // Nested boolean composition is bounded too.
            format!("or: [{{ not: {{ id: {{ inList: [{ids}] }} }} }}]"),
        ] {
            let response = schema
                .execute(format!(
                    "{{ batchSteps(where: {{ {filter} }}, page: {{ limit: 1 }}) \
                     {{ pageInfo {{ totalCount }} }} }}"
                ))
                .await;
            check(response, over, &filter[..40]);
        }

        let length = MAX_FILTER_PATTERN_LENGTH + usize::from(over);
        // Every character is one the `LIKE` escape doubles.
        let pattern = "%".repeat(length);
        for operator in ["contains", "startsWith", "endsWith"] {
            let response = schema
                .execute(format!(
                    "{{ batchSteps(where: {{ label: {{ {operator}: \"{pattern}\" }} }}) \
                     {{ pageInfo {{ totalCount }} }} }}"
                ))
                .await;
            check(response, over, operator);
        }

        // Rust callers get the same refusal from the query itself.
        let rust = EntityQuery::<Step>::new()
            .filter(&StepWhereInput {
                id: Some(StringFilter {
                    in_list: Some(vec!["a-7".to_string(); size]),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .fetch_all(&fixture.db)
            .await;
        assert_eq!(rust.is_err(), over);
    }

    // The limits hold for filters built in Rust, whatever their element type.
    let over = MAX_FILTER_LIST_OPERANDS + 1;
    assert!(
        IntFilter {
            not_in: Some(vec![1; over]),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        UuidFilter {
            in_list: Some(vec![graphql_orm::uuid::Uuid::nil(); over]),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        BytesFilter {
            not_in: Some(vec![vec![1]; over]),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        StringFilter {
            similar: Some(SimilarityInput {
                value: "x".repeat(MAX_FILTER_PATTERN_LENGTH + 1)
            }),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    // The pattern limit counts UTF-16 code units, as SQL Server does.
    let astral = "\u{1F600}".repeat(MAX_FILTER_PATTERN_LENGTH / 2);
    let within = StringFilter {
        contains: Some(astral.clone()),
        ..Default::default()
    };
    assert!(within.validate().is_ok());
    let beyond = StringFilter {
        contains: Some(format!("{astral}x")),
        ..Default::default()
    };
    assert!(beyond.validate().is_err());
    fixture.finish()
}
