// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(feature = "sqlite")]
//! Subscription delivery under a negated read policy admits exactly the rows
//! the SQL read admits: delivery re-reads each changed row with the same
//! rendered policy clause.
use graphql_orm::async_graphql::{Request, Response};
use graphql_orm::futures::{Stream, StreamExt, future::poll_fn};
use graphql_orm::prelude::*;
use std::task::Poll;
use tokio::time::{Duration, timeout};

#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[graphql_entity(
    table = "negation_tickets",
    plural = "NegationTickets",
    default_sort = "id ASC"
)]
struct NegationTicket {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    tag: Option<i32>,
}
#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[graphql_entity(table = "negation_holds", plural = "NegationHolds")]
struct NegationHold {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    ticket_id: String,
    active: Option<i32>,
}
schema_roots! { query_custom_ops: [], entities: [NegationTicket, NegationHold], }

struct Allow;
impl EntityPolicy for Allow {
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

/// Tickets are visible when their tag is known and not 1, and no hold on
/// them is, or might be, active.
fn policy() -> async_graphql::Result<ReadPredicate> {
    let tag_one = ReadPredicate::column::<SqliteBackend, NegationTicket>(
        "tag",
        ReadPredicateOp::Eq,
        SqlValue::Int(1),
    )?;
    let active = ReadPredicate::column::<SqliteBackend, NegationHold>(
        "active",
        ReadPredicateOp::Eq,
        SqlValue::Int(1),
    )?;
    tag_one.not()?.and(ReadPredicate::not_related::<
        SqliteBackend,
        NegationTicket,
        NegationHold,
    >(&[("id", "ticket_id")], active)?)
}

struct NegatedPolicy;
impl RowPolicy for NegatedPolicy {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            if entity == "NegationTicket" {
                Ok(ReadVisibility::Complete(policy()?))
            } else {
                Ok(ReadVisibility::Unrestricted)
            }
        })
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
        // Complete SQL visibility must not borrow authority from this fallback.
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

async fn registered<S: Stream<Item = Response> + Unpin>(stream: &mut S) {
    poll_fn(|cx| match stream.poll_next_unpin(cx) {
        Poll::Pending => Poll::Ready(()),
        other => panic!("subscription did not wait for its first event: {other:?}"),
    })
    .await;
}

#[tokio::test]
async fn subscription_delivery_matches_the_sql_read_under_negated_policies() {
    let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("CREATE TABLE negation_tickets (id TEXT PRIMARY KEY, tag INTEGER NULL)")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE negation_holds (id TEXT PRIMARY KEY, ticket_id TEXT NOT NULL, active INTEGER NULL)",
    )
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO negation_holds VALUES ('h1', 'held', 1), ('h2', 'released', 0), ('h3', 'unknown-hold', NULL), ('h4', 'unknown-hold', 0)",
    )
    .execute(db.pool())
    .await
    .unwrap();
    db.set_entity_policy(Allow);
    db.set_row_policy(NegatedPolicy);
    let schema = schema_builder(db.clone()).finish();

    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new("subscription { negationTicketChanged { id } }")
                .data(AuthSubject::new("subscriber")),
        ),
    );
    registered(&mut stream).await;
    let tickets = [
        ("null-tag", "null"),
        ("tag-one", "1"),
        ("free", "2"),
        ("held", "2"),
        ("released", "2"),
        ("unknown-hold", "2"),
        ("sentinel", "3"),
    ];
    for (id, tag) in tickets {
        let response = schema
            .execute(Request::new(format!(
                "mutation {{ createNegationTicket(input: {{id: \"{id}\", tag: {tag}}}) {{ success error }} }}"
            )).data(AuthSubject::new("writer")))
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        assert!(
            serde_json::to_string(&response.data)
                .unwrap()
                .contains("\"success\":true"),
            "{id}: {:?}",
            response.data
        );
    }
    let mut delivered = Vec::new();
    loop {
        let response = timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("the visible sentinel must be delivered")
            .unwrap();
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let id = response.data.into_json().unwrap()["negationTicketChanged"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let done = id == "sentinel";
        delivered.push(id);
        if done {
            break;
        }
    }
    let mut read = EntityQuery::<NegationTicket>::new()
        .with_read_visibility(&ReadVisibility::Complete(policy().unwrap()))
        .unwrap()
        .fetch_all(&db)
        .await
        .unwrap()
        .into_iter()
        .map(|ticket| ticket.id)
        .collect::<Vec<_>>();
    read.sort();
    let mut sorted = delivered.clone();
    sorted.sort();
    assert_eq!(sorted, read);
    assert_eq!(delivered, ["free", "released", "sentinel"]);
}
