// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(feature = "sqlite")]
// The fixed GraphQL names below are the default naming mode's.
#![cfg(not(any(
    graphql_orm_field_case = "pascal",
    graphql_orm_field_case = "snake",
    graphql_orm_field_case = "screaming_snake",
    graphql_orm_field_case = "lower",
    graphql_orm_field_case = "upper",
    graphql_orm_resolver_case = "pascal",
    graphql_orm_resolver_case = "snake",
    graphql_orm_resolver_case = "screaming_snake",
    graphql_orm_resolver_case = "lower",
    graphql_orm_resolver_case = "upper",
)))]

//! `#[graphql_orm(object_fields)]` with `#[graphql_object_fields]`: handwritten
//! resolver fields on the object `GraphQLEntity` generates.
//!
//! `Ticket` hosts handwritten fields. `PlainTicket` has the same columns and
//! does not, so the two show what the feature adds and what it leaves alone.

use graphql_orm::async_graphql::{Context, Request, Response, SimpleObject};
use graphql_orm::futures::{Stream, StreamExt, future::BoxFuture, future::poll_fn};
use graphql_orm::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;
use tokio::time::{Duration, timeout};

/// Who is asking. `name` owns rows; `scorer` may read scores.
#[derive(Clone)]
struct Actor {
    name: &'static str,
    scorer: bool,
}

/// Counts handwritten resolver bodies that ran.
#[derive(Clone, Default)]
struct Resolved(Arc<AtomicUsize>);

/// What a ticket's status amounts to.
#[derive(SimpleObject)]
struct Outcome {
    label: String,
    closed: bool,
}

macro_rules! ticket_entity {
    ($(#[$attribute:meta])* $name:ident, $table:literal, $plural:literal) => {
        /// A unit of work.
        #[derive(
            GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
        )]
        #[graphql_entity(
            backend = "sqlite",
            table = $table,
            plural = $plural,
            read_policy = "tickets.read",
            node_id
        )]
        $(#[$attribute])*
        struct $name {
            #[primary_key]
            #[graphql_orm(auto_generated = false, description = "Ticket identifier.")]
            #[filterable(type = "string")]
            id: String,
            #[graphql_orm(description = "Short summary.")]
            #[filterable(type = "string")]
            #[sortable]
            title: String,
            /// Where the work stands.
            #[sortable]
            status: String,
            #[graphql_orm(description = "Who may read the ticket.")]
            owner: String,
            #[graphql_orm(read_policy = "tickets.score")]
            #[filterable(type = "number")]
            score: i32,
            #[graphql_orm(version = "managed", default = "0")]
            row_version: i64,
        }
    };
}

ticket_entity!(
    #[graphql_orm(object_fields)]
    Ticket,
    "tickets",
    "Tickets"
);
ticket_entity!(PlainTicket, "plain_tickets", "PlainTickets");

#[graphql_object_fields]
impl Ticket {
    /// Outcome computed from the status; null only together with a field error.
    async fn outcome(
        &self,
        ctx: &Context<'_>,
    ) -> graphql_orm::async_graphql::Result<Option<Outcome>> {
        ctx.data::<Resolved>()?.0.fetch_add(1, Ordering::SeqCst);
        Ok(Some(Outcome {
            label: format!("{} is {}", self.title, self.status),
            closed: self.status == "closed",
        }))
    }

    /// Score doubled, for callers who may read scores.
    #[graphql_orm(read_policy = "tickets.score")]
    async fn doubled_score(&self) -> graphql_orm::async_graphql::Result<Option<i32>> {
        Ok(Some(self.score * 2))
    }

    /// Length of the title.
    #[graphql_orm(subscribe)]
    async fn title_length(
        &self,
        _: &Context<'_>,
        #[graphql(default = 0)] plus: i32,
    ) -> graphql_orm::async_graphql::Result<i32> {
        Ok(self.shout().len() as i32 + plus)
    }

    /// A value behind a policy whose provider fails.
    #[graphql_orm(read_policy = "tickets.broken")]
    async fn audited(&self) -> graphql_orm::async_graphql::Result<Option<bool>> {
        Ok(Some(true))
    }

    #[graphql(name = "explicitName")]
    async fn renamed(&self) -> graphql_orm::async_graphql::Result<&str> {
        Ok(&self.title)
    }

    #[graphql(skip)]
    fn shout(&self) -> String {
        self.title.to_uppercase()
    }
}

/// A ticket with PascalCase fields.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(rename_fields = "PascalCase")]
#[graphql_entity(backend = "sqlite", table = "pascal_tickets", plural = "PascalTickets")]
#[graphql_orm(object_fields)]
struct PascalTicket {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[sortable]
    title: String,
}

#[graphql_object_fields]
impl PascalTicket {
    /// Length of the title.
    async fn title_length(&self) -> graphql_orm::async_graphql::Result<i32> {
        Ok(self.title.len() as i32)
    }
}

impl BatchLoadEntity for Ticket {
    fn batch_column() -> &'static str {
        "id"
    }
    fn batch_key_from_row(
        row: &graphql_orm::sqlx::sqlite::SqliteRow,
    ) -> Result<String, graphql_orm::sqlx::Error> {
        use graphql_orm::sqlx::Row;
        row.try_get("id")
    }
}

/// A remark on a ticket, which reaches the ticket through a relation.
#[graphql_policy_object]
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
#[graphql(complex)]
#[graphql_entity(backend = "sqlite", table = "remarks", plural = "Remarks")]
struct Remark {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    ticket_id: String,
    #[graphql(skip)]
    #[relation(
        target = "Ticket",
        from = "ticket_id",
        to = "id",
        emit_fk = false,
        propagate_change = "none"
    )]
    ticket: Option<Ticket>,
}

#[derive(Default)]
struct RawQueries;

/// Hands out a ticket without asking the row policy, as a host resolver might.
#[graphql_orm_custom_operations(kind = "query", authorization = false)]
#[graphql_orm::async_graphql::Object]
impl RawQueries {
    /// A ticket no generated root authorized.
    async fn raw_ticket(&self) -> Ticket {
        Ticket {
            id: "raw".into(),
            title: "raw title".into(),
            status: "open".into(),
            owner: "nobody".into(),
            score: 7,
            row_version: 0,
        }
    }
}

schema_roots! {
    backend: "sqlite",
    auth: "none",
    client_metadata: true,
    entities: [Ticket, PlainTicket, PascalTicket, Remark],
    described_query_types: [RawQueries],
}

struct Policy;
impl EntityPolicy for Policy {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}
impl RowPolicy for Policy {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, graphql_orm::async_graphql::Result<ReadVisibility>> {
        Box::pin(async { Ok(ReadVisibility::CallbackOnly) })
    }
    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        Box::pin(async move {
            let Some(ticket) = row.downcast_ref::<Ticket>() else {
                return Ok(true);
            };
            let actor = ctx.expect("GraphQL read").data::<Actor>()?;
            Ok(ticket.owner == actor.name)
        })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}
impl FieldPolicy for Policy {
    fn can_read_field<'a>(
        &'a self,
        ctx: &'a Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        policy: Option<&'static str>,
        row: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        Box::pin(async move {
            match policy {
                None => Ok(true),
                // Scorers read scores, except the one ticket that withholds its own.
                Some("tickets.score") => Ok(ctx.data::<Actor>()?.scorer
                    && row
                        .and_then(|row| row.downcast_ref::<Ticket>())
                        .is_none_or(|ticket| ticket.id != "withheld")),
                Some("tickets.broken") => Err(graphql_orm::async_graphql::Error::new(
                    "policy store pg-internal-7 timed out",
                )),
                Some(_) => Ok(false),
            }
        })
    }
    fn can_write_field<'a>(
        &'a self,
        _: &'a Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

async fn fixture() -> (Database, AppSchema, Resolved) {
    let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    for sql in [
        "CREATE TABLE tickets (id TEXT PRIMARY KEY, title TEXT NOT NULL, status TEXT NOT NULL, owner TEXT NOT NULL, score INTEGER NOT NULL, row_version INTEGER NOT NULL DEFAULT 0)",
        "CREATE TABLE plain_tickets (id TEXT PRIMARY KEY, title TEXT NOT NULL, status TEXT NOT NULL, owner TEXT NOT NULL, score INTEGER NOT NULL, row_version INTEGER NOT NULL DEFAULT 0)",
        "CREATE TABLE pascal_tickets (id TEXT PRIMARY KEY, title TEXT NOT NULL)",
        "INSERT INTO tickets VALUES ('mine', 'first', 'closed', 'ada', 4, 0)",
        "INSERT INTO tickets VALUES ('withheld', 'second', 'open', 'ada', 5, 0)",
        "INSERT INTO tickets VALUES ('theirs', 'third', 'open', 'grace', 6, 0)",
        "INSERT INTO pascal_tickets VALUES ('p', 'pascal')",
        "CREATE TABLE remarks (id TEXT PRIMARY KEY, ticket_id TEXT NOT NULL)",
        "INSERT INTO remarks VALUES ('r1', 'mine')",
        "INSERT INTO remarks VALUES ('r2', 'theirs')",
    ] {
        graphql_orm::sqlx::query(sql)
            .execute(db.pool())
            .await
            .unwrap();
    }
    db.set_authorization_mode(AuthorizationMode::ExplicitPolicyForAllExposedOperations);
    db.set_entity_policy(Policy);
    db.set_row_policy(Policy);
    db.set_field_policy(Policy);
    let db = db.with_authorized_scan_config(AuthorizedScanConfig::new([7; 32], 7, 100));
    let resolved = Resolved::default();
    let schema = schema_builder(db.clone()).data(resolved.clone()).finish();
    (db, schema, resolved)
}

fn request(query: &str, name: &'static str, scorer: bool) -> Request {
    Request::new(query)
        .data(Actor { name, scorer })
        .data(AuthSubject::new(name))
}

/// The lines of one SDL definition, such as `type Ticket` or `input TicketWhereInput`.
fn block<'a>(sdl: &'a str, header: &str) -> Vec<&'a str> {
    let start = sdl
        .find(&format!("{header} "))
        .or_else(|| sdl.find(&format!("{header}\n")))
        .unwrap_or_else(|| panic!("{header} in SDL"));
    // Include the description that precedes the definition.
    let before = &sdl[..start];
    let start = match before.trim_end().strip_suffix("\"\"\"") {
        Some(described) => described.rfind("\"\"\"").expect("description start"),
        None => start,
    };
    let end = sdl[start..].find("\n}").expect("definition end") + start;
    sdl[start..end].lines().collect()
}

#[tokio::test]
async fn sdl_is_the_generated_object_plus_the_handwritten_fields() {
    let (_, schema, _) = fixture().await;
    let sdl = schema.sdl();
    let plain = block(&sdl, "type PlainTicket");
    let hosted = block(&sdl, "type Ticket");
    let renamed: Vec<String> = plain
        .iter()
        .map(|line| line.replace("PlainTicket", "Ticket"))
        .collect();
    // Every generated line, description, `RecordVersion` and `NodeId` included,
    // is there unchanged and in order, before the handwritten fields.
    assert_eq!(
        hosted[..renamed.len()],
        renamed.iter().map(String::as_str).collect::<Vec<_>>()[..],
        "{sdl}"
    );
    assert!(
        hosted
            .iter()
            .any(|line| line.contains("RecordVersion: Int!"))
    );
    assert!(hosted.iter().any(|line| line.contains("NodeId: ID!")));
    assert_eq!(
        hosted[renamed.len()..],
        [
            "\t\"\"\"",
            "\tOutcome computed from the status; null only together with a field error.",
            "\t\"\"\"",
            "\toutcome: Outcome",
            "\t\"\"\"",
            "\tScore doubled, for callers who may read scores.",
            "\t\"\"\"",
            "\tdoubledScore: Int",
            "\t\"\"\"",
            "\tLength of the title.",
            "\t\"\"\"",
            "\ttitleLength(plus: Int! = 0): Int!",
            "\t\"\"\"",
            "\tA value behind a policy whose provider fails.",
            "\t\"\"\"",
            "\taudited: Boolean",
            "\t\"\"\"",
            "\tRenamed",
            "\t\"\"\"",
            "\texplicitName: String!",
        ],
        "{sdl}"
    );
    assert!(sdl.contains("type Outcome {"), "{sdl}");

    // Filter, order and aggregate inputs know nothing of the handwritten fields.
    let mut compared = 0;
    for definition in sdl.split("\n\n") {
        let Some(header) = definition
            .lines()
            .find(|line| !line.starts_with('"') && line.contains("PlainTicket"))
            .filter(|line| line.ends_with('{'))
        else {
            continue;
        };
        let header = header.trim_end_matches(" {");
        if header == "type PlainTicket" {
            continue;
        }
        let hosted = block(&sdl, &header.replace("PlainTicket", "Ticket")).join("\n");
        let plain = block(&sdl, header)
            .join("\n")
            .replace("PlainTicket", "Ticket")
            .replace("plainTicket", "ticket")
            .replace("plain ticket", "ticket")
            .replace("Plain ticket", "Ticket")
            .replace("Plain Ticket", "Ticket");
        assert_eq!(hosted, plain);
        compared += 1;
    }
    assert!(compared >= 4, "compared {compared} definitions\n{sdl}");
}

#[tokio::test]
async fn handwritten_fields_resolve_on_list_by_key_and_relation_reads() {
    let (_, schema, resolved) = fixture().await;
    let listed = schema
        .execute(request(
            "{ tickets(orderBy: [{ title: ASC }]) { edges { node { id status outcome { label closed } titleLength(plus: 1) explicitName RecordVersion NodeId } } } }",
            "ada",
            false,
        ))
        .await;
    assert!(listed.errors.is_empty(), "{:?}", listed.errors);
    let listed = listed.data.into_json().unwrap();
    let edges = listed["tickets"]["edges"].as_array().unwrap();
    // The row policy admits two of the three rows.
    assert_eq!(edges.len(), 2, "{listed}");
    assert_eq!(edges[0]["node"]["outcome"]["label"], "first is closed");
    assert_eq!(edges[0]["node"]["outcome"]["closed"], true);
    assert_eq!(edges[0]["node"]["titleLength"], 6);
    assert_eq!(edges[0]["node"]["explicitName"], "first");
    assert_eq!(edges[0]["node"]["RecordVersion"], 0);
    assert_eq!(edges[1]["node"]["outcome"]["label"], "second is open");
    assert_eq!(resolved.0.load(Ordering::SeqCst), 2);

    // A handwritten field reads columns the query did not select.
    let by_key = schema
        .execute(request(
            "{ ticket(id: \"mine\") { outcome { label } } }",
            "ada",
            false,
        ))
        .await;
    assert!(by_key.errors.is_empty(), "{:?}", by_key.errors);
    assert_eq!(
        by_key.data.into_json().unwrap()["ticket"]["outcome"]["label"],
        "first is closed"
    );

    // Through a relation: the readable ticket resolves, the other is withheld
    // by the relation's own row decision.
    let related = schema
        .execute(request(
            "{ remarks(orderBy: [{ id: ASC }]) { edges { node { id ticket { outcome { label } doubledScore } } } } }",
            "ada",
            true,
        ))
        .await;
    let related_data = related.data.into_json().unwrap();
    let edges = related_data["remarks"]["edges"].as_array().unwrap();
    assert_eq!(
        edges[0]["node"]["ticket"]["outcome"]["label"], "first is closed",
        "{:?}",
        related.errors
    );
    assert_eq!(edges[0]["node"]["ticket"]["doubledScore"], 8);
    assert!(
        !related_data.to_string().contains("third"),
        "{related_data}"
    );

    let pascal = schema
        .execute(request(
            "{ pascalTicket(id: \"p\") { Title TitleLength } }",
            "ada",
            false,
        ))
        .await;
    assert!(pascal.errors.is_empty(), "{:?}", pascal.errors);
    assert_eq!(
        pascal.data.into_json().unwrap()["pascalTicket"]["TitleLength"],
        6
    );
}

#[tokio::test]
async fn generated_read_paths_withhold_the_row_before_any_handwritten_field() {
    let (_, schema, resolved) = fixture().await;
    // By key, in a list and through a relation, the row decision is made by
    // the read path, so the object of an unreadable row never exists.
    for query in [
        "{ ticket(id: \"theirs\") { outcome { label } } }",
        "{ tickets { edges { node { id outcome { label } } } } }",
        "{ remark(id: \"r2\") { ticket { outcome { label } } } }",
    ] {
        let response = schema.execute(request(query, "ada", true)).await;
        assert!(
            !response.data.to_string().contains("third"),
            "{query}: {}",
            response.data
        );
    }
    // Two readable rows in the list, and nothing for the others.
    assert_eq!(resolved.0.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_handwritten_field_asks_exactly_what_a_generated_field_asks() {
    let (_, schema, resolved) = fixture().await;
    // An object a host resolver built itself was authorized by nobody. Like a
    // generated column field, a handwritten field asks its field decision and
    // nothing about the row: the host resolver owns that.
    let raw = schema
        .execute(request(
            "{ rawTicket { title outcome { label } } }",
            "ada",
            true,
        ))
        .await;
    assert!(raw.errors.is_empty(), "{:?}", raw.errors);
    let raw = raw.data.into_json().unwrap();
    assert_eq!(raw["rawTicket"]["title"], "raw title");
    assert_eq!(raw["rawTicket"]["outcome"]["label"], "raw title is open");
    assert_eq!(resolved.0.load(Ordering::SeqCst), 1);

    // The field decision is the same one, with the same key: both fields
    // carry `tickets.score`, and both are refused or served together.
    for (scorer, refused) in [(false, true), (true, false)] {
        let mut outcomes = Vec::new();
        for field in ["score", "doubledScore"] {
            let response = schema
                .execute(request(
                    &format!("{{ ticket(id: \"mine\") {{ id {field} }} }}"),
                    "ada",
                    scorer,
                ))
                .await;
            assert_eq!(
                !response.errors.is_empty(),
                refused,
                "{field} for scorer {scorer}: {:?}",
                response.errors
            );
            outcomes.push(
                response
                    .errors
                    .first()
                    .map(|error| (error.message.clone(), error.extensions.clone())),
            );
        }
        assert_eq!(outcomes[0], outcomes[1]);
    }
}

#[tokio::test]
async fn a_relation_preflight_asks_handwritten_fields_like_columns() {
    let (_, schema, _) = fixture().await;
    // `score` and `doubledScore` both declare `tickets.score`. Selected
    // through the relation, a denial of either refuses the relation: nothing
    // of the ticket is released, and the two outcomes are the same.
    let mut denied = Vec::new();
    for field in ["score", "doubledScore"] {
        let response = schema
            .execute(request(
                &format!("{{ remark(id: \"r1\") {{ id ticket {{ id {field} }} }} }}"),
                "ada",
                false,
            ))
            .await;
        assert_eq!(response.errors.len(), 1, "{field}: {:?}", response.errors);
        let data = response.data.into_json().unwrap();
        assert_eq!(data["remark"]["id"], "r1", "{field}: {data}");
        assert!(!data.to_string().contains("mine"), "{field}: {data}");
        denied.push((
            data,
            response.errors[0].message.clone(),
            response.errors[0].extensions.clone(),
            response.errors[0].path.len(),
        ));
    }
    assert_eq!(denied[0], denied[1]);

    // Allowed, both resolve through the relation.
    for (field, value) in [("score", 4), ("doubledScore", 8)] {
        let response = schema
            .execute(request(
                &format!("{{ remark(id: \"r1\") {{ ticket {{ id {field} }} }} }}"),
                "ada",
                true,
            ))
            .await;
        assert!(response.errors.is_empty(), "{field}: {:?}", response.errors);
        let data = response.data.into_json().unwrap();
        assert_eq!(data["remark"]["ticket"]["id"], "mine");
        assert_eq!(data["remark"]["ticket"][field], value);
    }

    // A callback failure in the preflight is not exposed either.
    let response = schema
        .execute(request(
            "{ remark(id: \"r1\") { ticket { id audited } } }",
            "ada",
            true,
        ))
        .await;
    let exposed = format!("{:?} {}", response.errors, response.data);
    assert!(!response.errors.is_empty(), "{exposed}");
    assert!(!exposed.contains("pg-internal-7"), "{exposed}");
}

#[tokio::test]
async fn a_policy_callback_failure_is_not_exposed() {
    let (_, schema, _) = fixture().await;
    let response = schema
        .execute(request(
            "{ ticket(id: \"mine\") { id audited } }",
            "ada",
            true,
        ))
        .await;
    assert_eq!(response.errors.len(), 1, "{:?}", response.errors);
    let exposed = format!("{:?} {}", response.errors, response.data);
    assert!(!exposed.contains("pg-internal-7"), "{exposed}");
    assert!(!exposed.contains("timed out"), "{exposed}");
    // The sanitized error a generated getter would give, on this field's path.
    assert_eq!(
        response.errors[0]
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get("code")),
        Some(&graphql_orm::async_graphql::Value::from("INTERNAL_ERROR")),
        "{:?}",
        response.errors
    );
    let data = response.data.into_json().unwrap();
    let ticket = data["ticket"].as_object().expect("the row still resolves");
    assert_eq!(ticket["id"], "mine");
    assert_eq!(ticket.get("audited"), Some(&serde_json::Value::Null));
}

#[tokio::test]
async fn a_field_policy_denial_is_a_null_field_error_and_siblings_resolve() {
    let (_, schema, _) = fixture().await;
    let query = "{ tickets(orderBy: [{ title: ASC }]) { edges { node { id doubledScore } } } }";

    let scorer = schema.execute(request(query, "ada", true)).await;
    // One row withholds its score; the other resolves beside it.
    assert_eq!(scorer.errors.len(), 1, "{:?}", scorer.errors);
    let data = scorer.data.into_json().unwrap();
    let edges = data["tickets"]["edges"].as_array().unwrap();
    assert_eq!(edges[0]["node"]["id"], "mine");
    assert_eq!(edges[0]["node"]["doubledScore"], 8);
    assert_eq!(edges[1]["node"]["id"], "withheld");
    // The key is present and explicitly null, and the error names its path.
    assert_eq!(
        edges[1]["node"].as_object().unwrap().get("doubledScore"),
        Some(&serde_json::Value::Null)
    );
    use graphql_orm::async_graphql::PathSegment;
    assert_eq!(
        scorer.errors[0].path,
        [
            PathSegment::Field("tickets".into()),
            PathSegment::Field("edges".into()),
            PathSegment::Index(1),
            PathSegment::Field("node".into()),
            PathSegment::Field("doubledScore".into()),
        ]
    );

    let other = schema.execute(request(query, "ada", false)).await;
    assert_eq!(other.errors.len(), 2, "{:?}", other.errors);
    let data = other.data.into_json().unwrap();
    for edge in data["tickets"]["edges"].as_array().unwrap() {
        assert_eq!(
            edge["node"].as_object().unwrap().get("doubledScore"),
            Some(&serde_json::Value::Null)
        );
        assert!(edge["node"]["id"].is_string());
    }
}

#[tokio::test]
async fn a_declared_policy_without_a_provider_fails_closed() {
    let (original, _, _) = fixture().await;
    let mut db = Database::<SqliteBackend>::new(original.pool().clone());
    db.set_authorization_mode(AuthorizationMode::ExplicitPolicyForAllExposedOperations);
    db.set_entity_policy(Policy);
    db.set_row_policy(Policy);
    let schema = schema_builder(db).data(Resolved::default()).finish();
    // The strict modes refuse a declared key that no provider answers. The
    // generated field with that key is refused the same way.
    let generated = schema
        .execute(request("{ rawTicket { score } }", "nobody", true))
        .await;
    let denied = schema
        .execute(request("{ rawTicket { doubledScore } }", "nobody", true))
        .await;
    assert_eq!(denied.errors.len(), 1, "{:?}", denied.errors);
    assert_eq!(generated.errors.len(), 1, "{:?}", generated.errors);
    assert_eq!(denied.errors[0].message, generated.errors[0].message);
    assert_eq!(denied.errors[0].extensions, generated.errors[0].extensions);
    let data = denied.data.into_json().unwrap();
    assert_eq!(
        data["rawTicket"].as_object().unwrap().get("doubledScore"),
        Some(&serde_json::Value::Null)
    );
}

async fn registered<S: Stream<Item = Response> + Unpin>(stream: &mut S) {
    poll_fn(|cx| match stream.poll_next_unpin(cx) {
        Poll::Pending => Poll::Ready(()),
        other => panic!("unexpected event {other:?}"),
    })
    .await;
}

#[tokio::test]
async fn changed_subscriptions_refuse_unavailable_fields_and_resolve_available_ones() {
    let (db, schema, resolved) = fixture().await;

    // Not declared `subscribe`: refused before the stream registers.
    let mut refused = Box::pin(schema.execute_stream(request(
        "subscription { ticketChanged(key: \"mine\") { id ticket { title outcome { label } } } }",
        "ada",
        true,
    )));
    let refusal = timeout(Duration::from_secs(2), refused.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refusal.errors.len(), 1, "{:?}", refusal.errors);
    assert!(
        refusal.errors[0].message.contains("computed fields"),
        "{:?}",
        refusal.errors
    );

    // An unavailable handwritten scalar is refused at admission as well.
    let mut refused = Box::pin(schema.execute_stream(request(
        "subscription { ticketChanged(key: \"mine\") { id ticket { title doubledScore } } }",
        "ada",
        true,
    )));
    let refusal = timeout(Duration::from_secs(2), refused.next())
        .await
        .expect("refused before any event")
        .unwrap();
    assert_eq!(refusal.errors.len(), 1, "{:?}", refusal.errors);
    assert!(
        refusal.errors[0].message.contains("computed fields"),
        "{:?}",
        refusal.errors
    );

    // Through a relation of the subscribed entity, a handwritten field of the
    // related entity is refused at admission too, available or not: the event
    // payload admits leaves of the subscribed entity only.
    for nested in ["outcome { label }", "titleLength", "doubledScore"] {
        let mut refused = Box::pin(schema.execute_stream(request(
            &format!(
                "subscription {{ remarkChanged(key: \"r1\") {{ id remark {{ id ticket {{ {nested} }} }} }} }}"
            ),
            "ada",
            true,
        )));
        let refusal = timeout(Duration::from_secs(2), refused.next())
            .await
            .expect("refused before any event")
            .unwrap();
        assert_eq!(refusal.errors.len(), 1, "{nested}: {:?}", refusal.errors);
        assert!(
            refusal.errors[0].message.contains("computed fields"),
            "{nested}: {:?}",
            refusal.errors
        );
    }
    assert_eq!(resolved.0.load(Ordering::SeqCst), 0);

    let mut stream = Box::pin(schema.execute_stream(request(
        "subscription { ticketChanged(key: \"mine\") { id ticket { title titleLength } } }",
        "ada",
        false,
    )));
    registered(&mut stream).await;
    Ticket::update_by_id(
        &db,
        &"mine".to_owned(),
        UpdateTicketInput {
            title: Some("longer title".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let event = timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(event.errors.is_empty(), "{:?}", event.errors);
    let event = event.data.into_json().unwrap();
    assert_eq!(event["ticketChanged"]["ticket"]["title"], "longer title");
    assert_eq!(event["ticketChanged"]["ticket"]["titleLength"], 12);
    assert_eq!(resolved.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn metadata_lists_handwritten_fields_as_computed_and_read_only() {
    use graphql_orm::graphql::orm::GraphqlSemanticExport;

    let computed = Ticket::computed_object_fields();
    assert_eq!(
        computed
            .iter()
            .map(|field| (
                field.rust_name,
                field.api_name,
                field.read_policy,
                field.subscribe,
                (field.output_type)()
            ))
            .collect::<Vec<_>>(),
        [
            ("outcome", "outcome", None, false, "Outcome".to_owned()),
            (
                "doubled_score",
                "doubledScore",
                Some("tickets.score"),
                false,
                "Int".to_owned()
            ),
            ("title_length", "titleLength", None, true, "Int!".to_owned()),
            (
                "audited",
                "audited",
                Some("tickets.broken"),
                false,
                "Boolean".to_owned()
            ),
            ("renamed", "explicitName", None, false, "String!".to_owned()),
        ]
    );
    assert_eq!(
        computed[0].description,
        "Outcome computed from the status; null only together with a field error."
    );
    assert!(PlainTicket::computed_object_fields().is_empty());
    assert_eq!(
        PascalTicket::computed_object_fields()[0].api_name,
        "TitleLength"
    );

    // None is a column, so none is filterable, sortable or writable.
    for field in computed {
        assert!(
            Ticket::columns()
                .iter()
                .all(|column| column.api_name != field.api_name)
        );
        assert!(Ticket::subscription_field_policy(field.api_name).is_none());
    }

    let hosted = Ticket::graphql_semantic_metadata().unwrap();
    let plain = PlainTicket::graphql_semantic_metadata().unwrap();
    assert_eq!(hosted.fields.len(), plain.fields.len() + computed.len());
    // The generated descriptors are untouched and come first.
    for (hosted, plain) in hosted.fields.iter().zip(plain.fields.iter()) {
        assert_eq!(hosted, plain);
    }
    for (field, definition) in hosted.fields[plain.fields.len()..].iter().zip(computed) {
        assert_eq!(field.field_name, definition.api_name);
        assert_eq!(field.description, definition.description);
        assert!(field.selectable);
        assert!(field.filter_operators.is_empty() && field.filter_input.is_none());
        assert!(!field.sortable && !field.groupable);
        assert!(field.aggregate_operators.is_empty() && field.aggregate_value_kind.is_none());
        assert!(field.relationship.is_none());
        assert_eq!(field.export, GraphqlSemanticExport::NeverExport);
        assert_eq!(field.has_field_policy, definition.read_policy.is_some());
    }

    // The catalogue accepts the descriptors, and the published Changed stream
    // carries exactly the handwritten field that declares `subscribe`.
    let catalog = graphql_orm_semantic_catalog();
    let entity = catalog
        .entities
        .iter()
        .find(|entity| entity.entity_name == "Ticket")
        .expect("Ticket in the semantic catalogue");
    assert!(
        entity
            .fields
            .iter()
            .any(|field| field.field_name == "outcome")
    );
    let client = graphql_orm_client_metadata();
    client
        .validate_against(catalog)
        .expect("valid client metadata");
    let carried = &client
        .entity("Ticket")
        .expect("Ticket facts")
        .changed_stream
        .as_ref()
        .expect("stream facts")
        .carried_fields;
    assert!(carried.contains(&"titleLength".to_owned()), "{carried:?}");
    assert!(carried.contains(&"title".to_owned()), "{carried:?}");
    for unavailable in ["outcome", "doubledScore", "audited", "explicitName"] {
        assert!(!carried.contains(&unavailable.to_owned()), "{carried:?}");
    }
}
