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

/// A parent with a composite key. The same `id` exists under two tenants.
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
        table = "batch_boards",
        plural = "BatchBoards",
        auth = "none",
        schema_policy = "external_read_only"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "batch_boards",
        plural = "BatchBoards",
        auth = "none",
        schema_policy = "external_read_only"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "batch_boards",
        plural = "BatchBoards",
        auth = "none",
        schema_policy = "external_read_only"
    )
)]
#[graphql(complex)]
struct Board {
    #[primary_key]
    #[sortable]
    #[filterable(type = "string")]
    tenant: String,
    #[primary_key]
    #[sortable]
    #[filterable(type = "string")]
    id: String,
    #[graphql(skip)]
    #[relation(
        target = "Card",
        from = ["tenant", "id"],
        to = ["tenant", "board_id"],
        multiple,
        emit_fk = false
    )]
    cards: Vec<Card>,
}

/// A composite-keyed relation target ordered by a numeric position by
/// default. Card ids repeat across tenants, and positions run in opposite
/// directions in the two tenants, so a result that mixed tenants or fell back
/// to key order would not match.
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
        table = "batch_cards",
        plural = "BatchCards",
        auth = "none",
        schema_policy = "external_read_only",
        default_sort = "position ASC"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "batch_cards",
        plural = "BatchCards",
        auth = "none",
        schema_policy = "external_read_only",
        default_sort = "position ASC"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "batch_cards",
        plural = "BatchCards",
        auth = "none",
        schema_policy = "external_read_only",
        default_sort = "[position] ASC"
    )
)]
#[graphql(complex)]
struct Card {
    #[primary_key]
    #[sortable]
    #[filterable(type = "string")]
    tenant: String,
    #[primary_key]
    #[sortable]
    #[filterable(type = "string")]
    id: String,
    board_id: String,
    /// `tenant/id`, the single-column key the notes relation uses.
    uid: String,
    #[sortable]
    #[filterable(type = "number")]
    position: i32,
    #[filterable(type = "string")]
    label: String,
    /// Ordering by this count needs the request, so it is never batched.
    #[graphql(skip)]
    #[relation(
        target = "Note",
        from = "uid",
        to = "card_uid",
        multiple,
        emit_fk = false,
        order_aggregate(name = "NoteCount", aggregate = "count")
    )]
    notes: Vec<Note>,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "batch_notes",
        plural = "BatchNotes",
        auth = "none",
        schema_policy = "external_read_only"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "batch_notes",
        plural = "BatchNotes",
        auth = "none",
        schema_policy = "external_read_only"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "batch_notes",
        plural = "BatchNotes",
        auth = "none",
        schema_policy = "external_read_only"
    )
)]
struct Note {
    #[primary_key]
    #[sortable]
    id: String,
    card_uid: String,
}

/// Who a request acts for. Requests without one follow the fixture's mode.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Role {
    /// Reads every card.
    Admin,
    /// Hidden cards are excluded in SQL.
    Member,
    /// Hidden cards are excluded by a callback, so the relation is not batched.
    Auditor,
    /// Reads the visible cards of tenant `t1` only.
    TenantOne,
    /// May not read cards at all.
    Outsider,
}

schema_roots! { auth: "none", query_custom_ops: [], entities: [List, Step, Board, Card, Note], }

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

/// How one entity's rows are exposed to one request.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Exposure {
    All,
    /// Hidden rows excluded by a SQL predicate; `Some(tenant)` adds a tenant.
    Predicate(Option<&'static str>),
    Callback,
    Denied,
}

#[derive(Clone)]
struct Policy(Arc<Mutex<Mode>>);
impl Policy {
    /// Steps follow the fixture's mode. Cards follow the request's role, or
    /// the mode when the request carries none.
    fn exposure(&self, ctx: Option<&Context<'_>>, entity: &str) -> Exposure {
        let by_mode = match *self.0.lock().unwrap() {
            Mode::Open => Exposure::All,
            Mode::Predicate => Exposure::Predicate(None),
            Mode::Callback => Exposure::Callback,
        };
        match entity {
            "Step" => by_mode,
            "Card" => match ctx.and_then(|ctx| ctx.data_opt::<Role>()) {
                None => by_mode,
                Some(Role::Admin) => Exposure::All,
                Some(Role::Member) => Exposure::Predicate(None),
                Some(Role::Auditor) => Exposure::Callback,
                Some(Role::TenantOne) => Exposure::Predicate(Some("t1")),
                Some(Role::Outsider) => Exposure::Denied,
            },
            _ => Exposure::All,
        }
    }
}
impl RowPolicy for Policy {
    fn read_visibility<'a>(
        &'a self,
        ctx: Option<&'a Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            let shown = || {
                Some(StringFilter {
                    ne: Some("hidden".into()),
                    ..Default::default()
                })
            };
            Ok(match (self.exposure(ctx, entity), entity) {
                (Exposure::Predicate(_), "Step") => ReadVisibility::Complete(
                    ReadPredicate::from_filter::<DefaultBackend, _>(&StepWhereInput {
                        label: shown(),
                        ..Default::default()
                    })?,
                ),
                (Exposure::Predicate(tenant), "Card") => ReadVisibility::Complete(
                    ReadPredicate::from_filter::<DefaultBackend, _>(&CardWhereInput {
                        label: shown(),
                        tenant: tenant.map(|tenant| StringFilter {
                            eq: Some(tenant.into()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })?,
                ),
                (Exposure::Callback, _) => ReadVisibility::CallbackOnly,
                _ => ReadVisibility::Unrestricted,
            })
        })
    }
    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if self.exposure(ctx, entity) != Exposure::Callback {
                return Ok(true);
            }
            let label = row
                .downcast_ref::<Step>()
                .map(|step| step.label.as_str())
                .or_else(|| row.downcast_ref::<Card>().map(|card| card.label.as_str()));
            Ok(label.is_some_and(|label| label != "hidden"))
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
        ctx: Option<&'a Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(self.exposure(ctx, entity) != Exposure::Denied) })
    }
}

const TENANTS: [&str; 2] = ["t1", "t2"];
const BOARDS: [&str; 2] = ["b1", "b2"];

/// One card: its key, parent, position, label and how many notes it has.
#[derive(Clone, Debug)]
struct CardRow {
    tenant: &'static str,
    id: String,
    board: &'static str,
    position: i32,
    label: &'static str,
    notes: usize,
}

/// Every board has twelve cards whose ids repeat in both tenants. Positions
/// ascend with the id in `t1` and descend in `t2`. Each board also has a
/// hidden card in the middle, and `t1/b1` has two more cards that tie on a
/// position. Note counts are distinct within a board, unrelated to position.
fn cards() -> Vec<CardRow> {
    let mut cards = Vec::new();
    for tenant in TENANTS {
        for board in BOARDS {
            for n in 0..POSITIONS {
                cards.push(CardRow {
                    tenant,
                    id: format!("{board}-{n}"),
                    board,
                    position: if tenant == "t1" { n } else { POSITIONS - 1 - n },
                    label: "shown",
                    notes: ((n * 5) % POSITIONS) as usize + 1,
                });
            }
            cards.push(CardRow {
                tenant,
                id: format!("{board}-hidden"),
                board,
                position: 3,
                label: "hidden",
                notes: 20,
            });
        }
    }
    for (id, notes) in [("b1-5-tie", 13), ("b1-05-tie", 14)] {
        cards.push(CardRow {
            tenant: "t1",
            id: id.into(),
            board: "b1",
            position: 5,
            label: "shown",
            notes,
        });
    }
    cards
}

/// How a `cards` selection is ordered.
#[derive(Clone, Copy, Debug, PartialEq)]
enum CardOrder {
    /// No `orderBy`: the target's declared default order.
    Default,
    PositionDescending,
    /// Context-dependent: counts only the notes the request may read.
    NoteCountDescending,
}

impl CardOrder {
    fn argument(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::PositionDescending => "orderBy: { position: DESC }",
            Self::NoteCountDescending => "orderBy: { NoteCount: DESC }",
        }
    }
}

/// The card ids one role reads under one board, in the requested order with
/// the complete primary key as the final tie-break.
fn expected_cards(role: Role, tenant: &str, board: &str, order: CardOrder) -> Vec<String> {
    let mut cards = cards()
        .into_iter()
        .filter(|card| card.tenant == tenant && card.board == board)
        .filter(|card| match role {
            Role::Admin => true,
            Role::Member | Role::Auditor => card.label != "hidden",
            Role::TenantOne => card.label != "hidden" && card.tenant == "t1",
            Role::Outsider => false,
        })
        .collect::<Vec<_>>();
    cards.sort_by(|left, right| {
        let primary = match order {
            CardOrder::Default => left.position.cmp(&right.position),
            CardOrder::PositionDescending => right.position.cmp(&left.position),
            CardOrder::NoteCountDescending => right.notes.cmp(&left.notes),
        };
        primary.then_with(|| left.id.cmp(&right.id))
    });
    cards.into_iter().map(|card| card.id).collect()
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
        for statement in [
            "CREATE TABLE batch_boards (tenant VARCHAR(64) NOT NULL, id VARCHAR(64) NOT NULL, \
             PRIMARY KEY (tenant, id))",
            "CREATE TABLE batch_cards (tenant VARCHAR(64) NOT NULL, id VARCHAR(64) NOT NULL, \
             board_id VARCHAR(64) NOT NULL, uid VARCHAR(64) NOT NULL, position INTEGER NOT NULL, \
             label VARCHAR(64) NOT NULL, PRIMARY KEY (tenant, id))",
            "CREATE TABLE batch_notes (id VARCHAR(64) PRIMARY KEY, card_uid VARCHAR(64) NOT NULL)",
            "INSERT INTO batch_boards VALUES ('t1', 'b1'), ('t1', 'b2'), ('t2', 'b1'), ('t2', 'b2'), \
             ('t1', 'empty')",
        ] {
            sql(&db, statement).await?;
        }
        let cards = cards();
        let card_values = cards
            .iter()
            .map(|card| {
                format!(
                    "('{}', '{}', '{}', '{}/{}', {}, '{}')",
                    card.tenant,
                    card.id,
                    card.board,
                    card.tenant,
                    card.id,
                    card.position,
                    card.label
                )
            })
            .collect::<Vec<_>>();
        sql(
            &db,
            &format!("INSERT INTO batch_cards VALUES {}", card_values.join(", ")),
        )
        .await?;
        let note_values = cards
            .iter()
            .flat_map(|card| {
                (0..card.notes).map(move |n| {
                    format!(
                        "('{}/{}#{n}', '{}/{}')",
                        card.tenant, card.id, card.tenant, card.id
                    )
                })
            })
            .collect::<Vec<_>>();
        // SQL Server accepts 1,000 rows in one `VALUES` list.
        for chunk in note_values.chunks(500) {
            sql(
                &db,
                &format!("INSERT INTO batch_notes VALUES {}", chunk.join(", ")),
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

fn cards_query(order: CardOrder, window: Option<(usize, usize)>) -> String {
    let page =
        window.map(|(limit, offset)| format!("page: {{ limit: {limit}, offset: {offset} }}"));
    let arguments = [Some(order.argument().to_string()), page]
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
    format!(
        "{{ batchBoards {{ edges {{ node {{ tenant id cards{arguments} \
         {{ edges {{ node {{ id tenant position }} }} \
         pageInfo {{ totalCount hasNextPage hasPreviousPage }} }} }} }} }} }}"
    )
}

/// Per `tenant/board`: card ids, total count and page flags. Every returned
/// card must belong to the tenant of the board it is listed under.
fn card_pages(
    response: async_graphql::Response,
) -> std::collections::BTreeMap<String, (Vec<String>, i64, bool, bool)> {
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    data["batchBoards"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| {
            let node = &edge["node"];
            let cards = &node["cards"];
            let ids = cards["edges"]
                .as_array()
                .unwrap()
                .iter()
                .map(|edge| {
                    assert_eq!(edge["node"]["tenant"], node["tenant"], "cross-tenant row");
                    edge["node"]["id"].as_str().unwrap().to_string()
                })
                .collect();
            (
                format!(
                    "{}/{}",
                    node["tenant"].as_str().unwrap(),
                    node["id"].as_str().unwrap()
                ),
                (
                    ids,
                    cards["pageInfo"]["totalCount"].as_i64().unwrap(),
                    cards["pageInfo"]["hasNextPage"].as_bool().unwrap(),
                    cards["pageInfo"]["hasPreviousPage"].as_bool().unwrap(),
                ),
            )
        })
        .collect()
}

/// What one role must read for one order and window, per `tenant/board`.
fn expected_card_pages(
    role: Role,
    order: CardOrder,
    window: Option<(usize, usize)>,
) -> std::collections::BTreeMap<String, (Vec<String>, i64, bool, bool)> {
    let mut boards = TENANTS
        .into_iter()
        .flat_map(|tenant| BOARDS.into_iter().map(move |board| (tenant, board)))
        .collect::<Vec<_>>();
    boards.push(("t1", "empty"));
    boards
        .into_iter()
        .map(|(tenant, board)| {
            let all = expected_cards(role, tenant, board, order);
            let (limit, offset) = window.unwrap_or((usize::MAX, 0));
            let page = all
                .iter()
                .skip(offset)
                .take(limit)
                .cloned()
                .collect::<Vec<_>>();
            let has_next = offset.saturating_add(page.len()) < all.len();
            (
                format!("{tenant}/{board}"),
                (page, all.len() as i64, has_next, offset > 0),
            )
        })
        .collect()
}

/// The #245 contract as one table: every role, order and window, on a
/// composite-keyed relation whose keys collide across tenants, with and
/// without batching.
#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires owned disposable database"
)]
async fn default_order_holds_for_every_role_key_shape_and_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let _turn = QUERY_COUNT_LOCK.lock().await;
    let fixture = Fixture::start("relation-order-matrix").await?;
    let batched = schema_builder(fixture.db.clone()).finish();
    let unbatched = unbatched_schema(&fixture.db);
    let request = |order, window, role: Role| {
        async_graphql::Request::new(cards_query(order, window)).data(role)
    };

    let roles = [Role::Admin, Role::Member, Role::Auditor, Role::TenantOne];
    let orders = [
        CardOrder::Default,
        CardOrder::PositionDescending,
        CardOrder::NoteCountDescending,
    ];
    // Unpaged, then three pages of five; the last is partial or empty.
    let windows = [
        None,
        Some((5, 0)),
        Some((5, 5)),
        Some((5, 10)),
        Some((5, 15)),
    ];
    for role in roles {
        for order in orders {
            for window in windows {
                let case = format!("{role:?} {order:?} {window:?}");
                let expected = expected_card_pages(role, order, window);
                reset_query_count();
                let from_loader = card_pages(batched.execute(request(order, window, role)).await);
                let statements = query_count();
                assert_eq!(from_loader, expected, "{case}: batched");
                let direct = card_pages(unbatched.execute(request(order, window, role)).await);
                assert_eq!(direct, expected, "{case}: direct");
                // Batching eligibility: a SQL-complete visibility with an
                // order that needs no request context. Five boards then cost
                // the board list plus one count and one row statement.
                let eligible = role != Role::Auditor && order != CardOrder::NoteCountDescending;
                if eligible {
                    assert!(statements <= 3, "{case}: {statements} statements");
                }
            }
        }
    }

    // Colliding keys stay apart: `b1-0` is first in `t1` and last in `t2`.
    let first = card_pages(
        batched
            .execute(request(CardOrder::Default, Some((2, 0)), Role::Admin))
            .await,
    );
    assert_eq!(first["t1/b1"].0, ["b1-0", "b1-1"]);
    assert_eq!(first["t2/b1"].0, ["b1-11", "b1-10"]);

    // A role that may not read cards is refused, with no rows or counts.
    let denied = batched
        .execute(request(CardOrder::Default, Some((5, 0)), Role::Outsider))
        .await;
    let code = denied.errors.first().and_then(|error| {
        error
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get("code"))
            .map(|code| code.to_string())
    });
    assert_eq!(code.as_deref(), Some("\"FORBIDDEN\""), "{denied:?}");

    // Mixed roles in one batch: concurrent requests share the schema-wide
    // loader, whose long delay puts them in one dispatch. Each reads its own
    // rows in the declared order, and the refused one fails alone.
    let shared = schema_builder(fixture.db.clone())
        .data(
            DataLoader::new(
                RelationLoader::<Card, DefaultBackend>::new(fixture.db.clone()),
                tokio::spawn,
            )
            .delay(Duration::from_millis(250)),
        )
        .finish();
    for (order, window) in [
        (CardOrder::Default, Some((5, 5))),
        (CardOrder::Default, None),
        (CardOrder::PositionDescending, Some((5, 0))),
    ] {
        let run = |role| shared.execute(request(order, window, role));
        let (admin, member, tenant_one, outsider) = tokio::join!(
            run(Role::Admin),
            run(Role::Member),
            run(Role::TenantOne),
            run(Role::Outsider)
        );
        for (role, response) in [
            (Role::Admin, admin),
            (Role::Member, member),
            (Role::TenantOne, tenant_one),
        ] {
            assert_eq!(
                card_pages(response),
                expected_card_pages(role, order, window),
                "mixed batch, {role:?} {order:?} {window:?}"
            );
        }
        assert!(!outsider.errors.is_empty(), "{outsider:?}");
    }

    // Current authority: the same schema follows a change of what the
    // request may read on its next execution; nothing is reused.
    let by_mode = |window| async_graphql::Request::new(cards_query(CardOrder::Default, window));
    for (mode, role) in [
        (Mode::Open, Role::Admin),
        (Mode::Predicate, Role::Member),
        (Mode::Callback, Role::Auditor),
        (Mode::Open, Role::Admin),
    ] {
        *fixture.mode.lock().unwrap() = mode;
        for window in [None, Some((5, 0))] {
            assert_eq!(
                card_pages(batched.execute(by_mode(window)).await),
                expected_card_pages(role, CardOrder::Default, window),
                "authority {mode:?} {window:?}"
            );
        }
    }
    fixture.finish()
}
