// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "mssql"))]
//! Subscription delivery and search under a closure read policy admit exactly
//! the rows the SQL read admits: both apply the same rendered policy clause.
//! SQL Server runs delivery only; it has no full-text search execution.
#[cfg(feature = "mssql")]
#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
use graphql_orm::async_graphql::{Request, Response};
use graphql_orm::futures::{Stream, StreamExt, future::poll_fn};
use graphql_orm::prelude::*;
use std::task::Poll;
use tokio::time::{Duration, timeout};

#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        table = "closure_tickets",
        plural = "ClosureTickets",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "sqlite",
    graphql_orm(search(index = true, tokenizer = "unicode61", min_token_len = 2))
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "closure_tickets",
        plural = "ClosureTickets",
        default_sort = "id ASC"
    )
)]
struct ClosureTicket {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    parent_id: Option<String>,
    tag: Option<i32>,
    #[cfg_attr(feature = "sqlite", graphql_orm(searchable(weight = "A")))]
    title: String,
}
#[cfg(feature = "sqlite")]
schema_roots! { query_custom_ops: [], entities: [ClosureTicket], }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [ClosureTicket], }
type B = DefaultBackend;

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
    let tag_one =
        ReadPredicate::column::<B, ClosureTicket>("tag", ReadPredicateOp::Eq, SqlValue::Int(1))?;
    ReadPredicate::related_closure_all::<B, ClosureTicket>(&[("parent_id", "id")], tag_one.not()?)
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
#[cfg_attr(feature = "mssql", ignore = "owned database lane")]
async fn delivery_and_search_match_the_sql_read_under_closure_policies() {
    #[cfg(feature = "sqlite")]
    let mut db = {
        let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
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
        db
    };
    #[cfg(feature = "mssql")]
    let mut owner = owned_mssql::OwnedSqlServer::start().await.unwrap();
    #[cfg(feature = "mssql")]
    let mut db = {
        owned_mssql::execute_batch(
            &owner.connection_string,
            "CREATE TABLE closure_tickets (id NVARCHAR(64) NOT NULL PRIMARY KEY, \
             parent_id NVARCHAR(64) NULL, tag INT NULL, title NVARCHAR(200) NOT NULL);",
        )
        .await
        .unwrap();
        let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(
            &owner.connection_string,
        )
        .await
        .unwrap();
        Database::builder(pool)
            .schema_policy(SchemaPolicy::ExternalWritable)
            .build()
    };
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

    #[cfg(feature = "sqlite")]
    search_matches_the_read(&schema, &read).await;
    #[cfg(feature = "mssql")]
    owner.cleanup().unwrap();
}

#[cfg(feature = "sqlite")]
async fn search_matches_the_read(schema: &AppSchema, read: &[String]) {
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
