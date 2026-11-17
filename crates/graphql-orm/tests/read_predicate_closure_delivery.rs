// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(feature = "sqlite")]
//! Subscription delivery and search under a closure read policy admit exactly
//! the rows the SQL read admits: both apply the same rendered policy clause.
use graphql_orm::async_graphql::{Request, Response};
use graphql_orm::futures::{Stream, StreamExt, future::poll_fn};
use graphql_orm::prelude::*;
use std::task::Poll;
use tokio::time::{Duration, timeout};

#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[graphql_entity(
    table = "closure_tickets",
    plural = "ClosureTickets",
    default_sort = "id ASC"
)]
#[graphql_orm(search(index = true, tokenizer = "unicode61", min_token_len = 2))]
struct ClosureTicket {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    parent_id: Option<String>,
    tag: Option<i32>,
    #[graphql_orm(searchable(weight = "A"))]
    title: String,
}
schema_roots! { query_custom_ops: [], entities: [ClosureTicket], }

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

/// A ticket is visible when neither it nor any ancestor has tag 1 or an
/// unknown tag, and its parent chain is complete.
fn policy() -> async_graphql::Result<ReadPredicate> {
    let tag_one = ReadPredicate::column::<SqliteBackend, ClosureTicket>(
        "tag",
        ReadPredicateOp::Eq,
        SqlValue::Int(1),
    )?;
    ReadPredicate::related_closure_all::<SqliteBackend, ClosureTicket>(
        &[("parent_id", "id")],
        tag_one.not()?,
    )
}

struct ClosurePolicy;
impl RowPolicy for ClosurePolicy {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move { Ok(ReadVisibility::Complete(policy()?)) })
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
async fn delivery_and_search_match_the_sql_read_under_closure_policies() {
    let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    let plan = build_migration_plan(
        DatabaseBackend::Sqlite,
        &SchemaModel {
            extensions: vec![],
            tables: vec![],
        },
        &SchemaModel::from_entities(&[ClosureTicket::metadata()]),
    );
    for statement in &plan.statements {
        sqlx::query(statement).execute(db.pool()).await.unwrap();
    }
    db.set_entity_policy(Allow);
    db.set_row_policy(ClosurePolicy);
    let schema = schema_builder(db.clone()).finish();

    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new("subscription { closureTicketChanged { id } }")
                .data(AuthSubject::new("subscriber")),
        ),
    );
    registered(&mut stream).await;
    let tickets = [
        ("root", "null", "2"),
        ("child", "\"root\"", "2"),
        ("blocked", "null", "1"),
        ("under-blocked", "\"blocked\"", "2"),
        ("unknown-tag", "\"root\"", "null"),
        ("under-unknown", "\"unknown-tag\"", "2"),
        ("dangling", "\"missing\"", "2"),
        ("self-loop", "\"self-loop\"", "2"),
        ("sentinel", "\"child\"", "3"),
    ];
    for (id, parent, tag) in tickets {
        let response = schema
            .execute(Request::new(format!(
                "mutation {{ createClosureTicket(input: {{id: \"{id}\", parentId: {parent}, tag: {tag}, title: \"needle {id}\"}}) {{ success error }} }}"
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
        let id = response.data.into_json().unwrap()["closureTicketChanged"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let done = id == "sentinel";
        delivered.push(id);
        if done {
            break;
        }
    }
    let mut read = EntityQuery::<ClosureTicket>::new()
        .with_read_visibility(&ReadVisibility::Complete(policy().unwrap()))
        .unwrap()
        .fetch_all(&db)
        .await
        .unwrap()
        .into_iter()
        .map(|ticket| ticket.id)
        .collect::<Vec<_>>();
    read.sort();
    assert_eq!(read, ["child", "root", "self-loop", "sentinel"]);
    let mut sorted = delivered.clone();
    sorted.sort();
    assert_eq!(sorted, read);
    assert_eq!(delivered, ["root", "child", "self-loop", "sentinel"]);

    // Search applies the same clause: every ticket matches "needle".
    let response = schema
        .execute(
            Request::new(
                "{ closureTicketsSearch(search: {query: \"needle\"}) { edges { node { id } } pageInfo { totalCount } } }",
            )
            .data(AuthSubject::new("reader")),
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let json = response.data.into_json().unwrap();
    let connection = &json["closureTicketsSearch"];
    let mut searched = connection["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| edge["node"]["id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    searched.sort();
    assert_eq!(searched, read);
    assert_eq!(connection["pageInfo"]["totalCount"], read.len());
}
