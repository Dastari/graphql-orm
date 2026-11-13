//! An omitted `default_sort` orders by the declared primary key (#404).
#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::async_graphql::SimpleObject;
use graphql_orm::prelude::*;

/// A key that is not named `id`, with no `default_sort`.
#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql_entity(table = "requests", plural = "Requests", keyset = "request_id asc")]
#[graphql(complex)]
struct Request {
    #[primary_key]
    #[sortable]
    request_id: String,
    state: String,
    /// A relation without `from` uses the primary key as its source.
    #[graphql(skip)]
    #[relation(target = "RequestNote", to = "request_id", multiple)]
    notes: Vec<RequestNote>,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "request_notes",
    plural = "RequestNotes",
    keyset = "note_id asc"
)]
struct RequestNote {
    #[primary_key]
    #[sortable]
    note_id: String,
    #[filterable(type = "string")]
    request_id: String,
}

/// A composite key whose second column is renamed.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "allocations", plural = "Allocations")]
struct Allocation {
    #[primary_key]
    #[sortable]
    region: String,
    #[primary_key]
    #[sortable]
    #[graphql_orm(db_column = "slot_number")]
    slot: i64,
    label: String,
}

impl graphql_orm::graphql::loaders::BatchLoadEntity for RequestNote {
    fn batch_column() -> &'static str {
        "request_id"
    }

    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, sqlx::Error> {
        use sqlx::Row;
        row.try_get("request_id")
    }
}

schema_roots! {
    query_custom_ops: [],
    generated_mutations: "none",
    entities: [Request, RequestNote, Allocation],
}

async fn database() -> graphql_orm::Result<Database<SqliteBackend>> {
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    for sql in [
        "CREATE TABLE requests (request_id TEXT PRIMARY KEY, state TEXT NOT NULL)",
        "CREATE TABLE request_notes (note_id TEXT PRIMARY KEY, request_id TEXT NOT NULL)",
        "CREATE TABLE allocations (region TEXT NOT NULL, slot_number INTEGER NOT NULL, \
         label TEXT NOT NULL, PRIMARY KEY (region, slot_number))",
    ] {
        sqlx::query(sql).execute(db.pool()).await?;
    }
    Ok(db)
}

#[test]
fn omitted_default_sort_uses_every_primary_key_column_in_order() {
    assert_eq!(Request::DEFAULT_SORT, "request_id");
    assert_eq!(Request::metadata().default_sort, "request_id");
    assert_eq!(Allocation::DEFAULT_SORT, "region, slot_number");
    assert_eq!(Allocation::metadata().default_sort, "region, slot_number");
}

#[test]
fn relation_without_from_uses_the_primary_key_in_metadata() {
    let metadata = Request::metadata();
    let notes = metadata
        .relations
        .iter()
        .find(|relation| relation.field_name == "notes")
        .expect("notes relation metadata");
    assert_eq!(notes.source_column, "request_id");
    assert_eq!(notes.source_columns, ["request_id"]);
}

#[tokio::test]
async fn non_id_primary_key_orders_repository_and_generated_reads() -> graphql_orm::Result<()> {
    let db = database().await?;
    // The original defect failed before any row was read.
    assert!(
        EntityQuery::<Request>::new()
            .default_order()
            .fetch_all(&db)
            .await?
            .is_empty()
    );
    for id in ["b", "a", "c"] {
        sqlx::query("INSERT INTO requests VALUES (?, 'open')")
            .bind(id)
            .execute(db.pool())
            .await?;
    }
    sqlx::query("INSERT INTO request_notes VALUES ('n2', 'a'), ('n1', 'a')")
        .execute(db.pool())
        .await?;
    let rows = EntityQuery::<Request>::new()
        .default_order()
        .fetch_all(&db)
        .await?;
    assert_eq!(
        rows.iter()
            .map(|row| row.request_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c"]
    );

    let schema = graphql_orm::async_graphql::Schema::build(
        QueryRoot::default(),
        graphql_orm::async_graphql::EmptyMutation,
        graphql_orm::async_graphql::EmptySubscription,
    )
    .data(db.clone())
    .data(graphql_orm::graphql::auth::AuthSubject::new("test-user"))
    .finish();
    let response = schema
        .execute(
            "{ requests { edges { node { requestId notes { edges { node { noteId } } } } } } }",
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let json = response.data.into_json().unwrap();
    let edges = json["requests"]["edges"].as_array().unwrap();
    assert_eq!(edges[0]["node"]["requestId"], "a");
    let notes = edges[0]["node"]["notes"]["edges"].as_array().unwrap();
    assert_eq!(notes[0]["node"]["noteId"], "n1");
    assert_eq!(notes[1]["node"]["noteId"], "n2");

    let page = Request::keyset_page(
        &db,
        RequestWhereInput::default(),
        KeysetPageInput {
            limit: Some(2),
            ..Default::default()
        },
    )
    .await
    .expect("keyset page");
    assert_eq!(
        page.edges
            .iter()
            .map(|edge| edge.node.request_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    let _catalog = graphql_orm_operation_catalog();
    assert!(schema.sdl().contains("requestId"));
    Ok(())
}

#[tokio::test]
async fn composite_primary_key_orders_by_each_column() -> graphql_orm::Result<()> {
    let db = database().await?;
    sqlx::query(
        "INSERT INTO allocations VALUES ('west', 2, 'w2'), ('east', 9, 'e9'), \
         ('west', 1, 'w1'), ('east', 3, 'e3')",
    )
    .execute(db.pool())
    .await?;
    let rows = EntityQuery::<Allocation>::new()
        .default_order()
        .fetch_all(&db)
        .await?;
    assert_eq!(
        rows.iter()
            .map(|row| row.label.as_str())
            .collect::<Vec<_>>(),
        ["e3", "e9", "w1", "w2"]
    );
    Ok(())
}

/// A repository-only entity whose key is renamed.
#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(backend = "sqlite", table = "repository_requests")]
struct RepositoryRequest {
    #[primary_key]
    #[graphql_orm(db_column = "physical_key", auto_generated = false)]
    request_id: String,
}

#[tokio::test]
async fn repository_only_default_uses_the_physical_key() -> graphql_orm::Result<()> {
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    sqlx::query("CREATE TABLE repository_requests (physical_key TEXT PRIMARY KEY)")
        .execute(db.pool())
        .await?;
    sqlx::query("INSERT INTO repository_requests VALUES ('b'), ('a')")
        .execute(db.pool())
        .await?;
    assert_eq!(RepositoryRequest::DEFAULT_SORT, "physical_key");
    let rows = RepositoryRequest::query(&db)
        .default_order()
        .fetch_all()
        .await?;
    assert_eq!(rows[0].request_id, "a");
    Ok(())
}
