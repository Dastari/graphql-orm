// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
//! The page-size contract of generated GraphQL pages and of host-side keyset
//! pages, on every backend.
//!
//! By default a client page limit above the database's maximum is reduced to
//! it. With `DatabaseBuilder::refuse_oversized_page_limit` the request fails with
//! `PAGE_LIMIT_EXCEEDED` instead, on list roots, to-many relations and keyset
//! connections. Trusted Rust code gives one keyset read its own maximum with
//! `EntityQuery::with_max_limit`.
//!
//! SQLite runs directly. PostgreSQL and SQL Server use a test-owned container:
//!
//! ```sh
//! cargo test -p graphql-orm --no-default-features --features postgres --test page_limit_contract -- --ignored
//! cargo test -p graphql-orm --no-default-features --features mssql --test page_limit_contract -- --ignored
//! ```

use graphql_orm::graphql::pagination::KeysetPageInput;
use graphql_orm::prelude::*;
use serde_json::Value;

#[cfg(feature = "mssql")]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(
    GraphQLEntity,
    GraphQLOperations,
    GraphQLRelations,
    async_graphql::SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(backend = "sqlite", table = "limit_boards", plural = "LimitBoards")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(backend = "postgres", table = "limit_boards", plural = "LimitBoards")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "limit_boards",
        plural = "LimitBoards"
    )
)]
#[graphql(complex)]
struct LimitBoard {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[graphql(skip)]
    #[relation(
        target = "LimitCard",
        from = "id",
        to = "board_id",
        multiple,
        emit_fk = false
    )]
    cards: Vec<LimitCard>,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "limit_cards",
        plural = "LimitCards",
        default_sort = "id ASC",
        keyset = "id asc"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "limit_cards",
        plural = "LimitCards",
        default_sort = "id ASC",
        keyset = "id asc"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "limit_cards",
        plural = "LimitCards",
        default_sort = "id ASC",
        keyset = "id asc"
    )
)]
// A native search index on SQLite and PostgreSQL; SQL Server searches by scan.
#[graphql_orm(search(index = true))]
struct LimitCard {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    #[graphql_orm(searchable(weight = "A"))]
    board_id: String,
}

// The schema declares the maximum page limit its `Database` applies.
#[cfg(feature = "sqlite")]
schema_roots! {
    backend: "sqlite",
    max_page_limit: 2,
    query_custom_ops: [],
    entities: [LimitBoard, LimitCard],
}
#[cfg(feature = "postgres")]
schema_roots! {
    backend: "postgres",
    max_page_limit: 2,
    query_custom_ops: [],
    entities: [LimitBoard, LimitCard],
}
#[cfg(feature = "mssql")]
schema_roots! {
    backend: "mssql",
    schema_policy: "external_writable",
    max_page_limit: 2,
    query_custom_ops: [],
    entities: [LimitBoard, LimitCard],
}

#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "mssql")]
type Backend = MssqlBackend;

/// Cards on the one board.
const CARDS: usize = 5;
/// The configured maximum page limit of the fixture.
const MAXIMUM: i64 = 2;

struct Fixture {
    db: Database,
    #[cfg(feature = "mssql")]
    owner: owned_mssql::OwnedSqlServer,
}

impl Fixture {
    fn finish(self) -> TestResult {
        #[cfg(feature = "mssql")]
        {
            let Self { db, mut owner } = self;
            drop(db);
            owner.cleanup()?;
        }
        Ok(())
    }
}

async fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    // The pool keeps the owned container alive; it is removed when the last clone drops.
    #[cfg(feature = "postgres")]
    let db = Database::new(owned_postgres::OwnedPostgres::pool("page-limit-contract", 4).await?);
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    {
        let plan = db
            .schema()
            .plan_migration_to_entities(
                "page-limit-contract-init",
                "page limit contract test",
                &[LimitBoard::metadata(), LimitCard::metadata()],
            )
            .await?;
        db.schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await?;
    }
    #[cfg(feature = "mssql")]
    let (db, owner) = {
        let owner = owned_mssql::OwnedSqlServer::start().await?;
        owned_mssql::execute_batch(
            &owner.connection_string,
            "CREATE TABLE limit_boards (id NVARCHAR(64) NOT NULL PRIMARY KEY); \
             CREATE TABLE limit_cards (id NVARCHAR(64) NOT NULL PRIMARY KEY, \
             board_id NVARCHAR(64) NOT NULL);",
        )
        .await?;
        let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(
            &owner.connection_string,
        )
        .await?;
        let db = Database::builder(pool)
            .schema_policy(SchemaPolicy::ExternalWritable)
            .build();
        (db, owner)
    };
    let db = db.with_pagination_config(PaginationConfig::secure().with_max_limit(Some(MAXIMUM)));
    // Constant fixture rows, rendered once for every backend.
    let boards = "INSERT INTO limit_boards (id) VALUES ('board')".to_owned();
    let cards = format!(
        "INSERT INTO limit_cards (id, board_id) VALUES {}",
        (0..CARDS)
            .map(|index| format!("('card-{index}', 'board')"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    for insert in [boards, cards] {
        #[cfg(any(feature = "sqlite", feature = "postgres"))]
        sqlx::query(&insert).execute(db.pool()).await?;
        #[cfg(feature = "mssql")]
        owned_mssql::execute_batch(&owner.connection_string, &insert).await?;
    }
    // Rows inserted with SQL reach the native search index on a rebuild.
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    LimitCard::rebuild_search_index(&db).await?;
    // SQL Server has no native index: a search scans candidates in batches.
    #[cfg(feature = "mssql")]
    let db = db.with_authorized_scan_config(graphql_orm::graphql::orm::AuthorizedScanConfig::new(
        [7; 32], 10, 100,
    ));
    Ok(Fixture {
        db,
        #[cfg(feature = "mssql")]
        owner,
    })
}

async fn execute(db: &Database, query: &str) -> async_graphql::Response {
    schema_builder(db.clone())
        .finish()
        .execute(async_graphql::Request::new(query).data(AuthSubject::new("reader")))
        .await
}

/// The pages a client can ask for, with the path of each page's edges.
const PAGES: [(&str, &str, &[&str]); 4] = [
    (
        "list root",
        "{ limitCards(page: { limit: LIMIT }) { edges { node { id } } } }",
        &["limitCards"],
    ),
    (
        "keyset connection",
        "{ limitCardsKeyset(page: { limit: LIMIT }) { edges { node { id } } } }",
        &["limitCardsKeyset"],
    ),
    (
        "to-many relation",
        "{ limitBoard(id: \"board\") { cards(page: { limit: LIMIT }) { edges { node { id } } } } }",
        &["limitBoard", "cards"],
    ),
    (
        "search connection",
        "{ limitCardsSearch(search: { query: \"board\" }, page: { limit: LIMIT }) { edges { node { id } } } }",
        &["limitCardsSearch"],
    ),
];

fn edges(mut data: Value, path: &[&str]) -> usize {
    for step in path {
        data = data[step].clone();
    }
    data["edges"].as_array().map_or(0, Vec::len)
}

async fn oversized_client_limits_are_reduced_unless_the_refusal_is_enabled(
    db: &Database,
) -> TestResult {
    let refusing = db.clone().with_oversized_page_limit_refusal(true);
    for (name, query, path) in PAGES {
        let oversized = query.replace("LIMIT", "3");
        let at_maximum = query.replace("LIMIT", "2");
        // An omitted limit uses the default limit, reduced to the maximum.
        let omitted = query.replace("{ limit: LIMIT }", "{}");

        // Default: the page is reduced to the maximum, with no error.
        let reduced = execute(db, &oversized).await;
        assert!(reduced.errors.is_empty(), "{name}: {:?}", reduced.errors);
        assert_eq!(edges(reduced.data.into_json()?, path), 2, "{name}");

        // Opt-in: the same request is refused with a stable code, and the
        // message names the limit asked for and the real maximum.
        let refused = execute(&refusing, &oversized).await;
        assert_eq!(refused.errors.len(), 1, "{name}: {:?}", refused.errors);
        let error = &refused.errors[0];
        assert_eq!(
            error.extensions.as_ref().and_then(|ext| ext.get("code")),
            Some(&async_graphql::Value::from("PAGE_LIMIT_EXCEEDED")),
            "{name}: {error:?}"
        );
        assert_eq!(
            error.message, "requested limit 3 exceeds the maximum page limit 2",
            "{name}"
        );

        // A limit at the maximum, and an omitted limit, are still served.
        for query in [&at_maximum, &omitted] {
            let served = execute(&refusing, query).await;
            assert!(served.errors.is_empty(), "{name}: {:?}", served.errors);
            assert_eq!(edges(served.data.into_json()?, path), 2, "{name}");
        }
    }
    Ok(())
}

/// The offset pages a client can ask for, with the path of each page's edges.
const OFFSET_PAGES: [(&str, &str, &[&str]); 3] = [
    (
        "list root",
        "{ limitCards(page: { limit: 2, offset: OFFSET }) { edges { node { id } } } }",
        &["limitCards"],
    ),
    (
        "to-many relation",
        "{ limitBoard(id: \"board\") { cards(page: { limit: 2, offset: OFFSET }) { edges { node { id } } } } }",
        &["limitBoard", "cards"],
    ),
    // The native index path on SQLite and PostgreSQL, the scan on SQL Server.
    (
        "search connection",
        "{ limitCardsSearch(search: { query: \"board\" }, page: { limit: 2, offset: OFFSET }) { edges { node { id } } } }",
        &["limitCardsSearch"],
    ),
];

/// #456: a client page offset above a configured maximum is refused with
/// `PAGE_LIMIT_EXCEEDED`, the code an oversized limit is refused with, and
/// never reaches the database. An offset at the maximum is served. Without a
/// maximum, which is the default, offsets stay unbounded.
async fn client_offsets_above_a_configured_maximum_are_refused(db: &Database) -> TestResult {
    let capped = db.clone().with_max_page_offset(Some(3));
    assert_eq!(db.client_pagination_config().max_offset(), None);
    assert_eq!(capped.client_pagination_config().max_offset(), Some(3));
    for (name, query, path) in OFFSET_PAGES {
        let over = query.replace("OFFSET", "4");
        let at_maximum = query.replace("OFFSET", "3");
        let deep = query.replace("OFFSET", "1000000000");

        // Default: any offset is served; past the last row the page is empty.
        let served = execute(db, &over).await;
        assert!(served.errors.is_empty(), "{name}: {:?}", served.errors);
        assert_eq!(edges(served.data.into_json()?, path), 1, "{name}");
        let served = execute(db, &deep).await;
        assert!(served.errors.is_empty(), "{name}: {:?}", served.errors);
        assert_eq!(edges(served.data.into_json()?, path), 0, "{name}");

        // With a maximum: refused with the stable code, naming both numbers.
        for query in [&over, &deep] {
            let refused = execute(&capped, query).await;
            assert_eq!(refused.errors.len(), 1, "{name}: {:?}", refused.errors);
            let error = &refused.errors[0];
            assert_eq!(
                error.extensions.as_ref().and_then(|ext| ext.get("code")),
                Some(&async_graphql::Value::from("PAGE_LIMIT_EXCEEDED")),
                "{name}: {error:?}"
            );
            assert!(
                error.message.starts_with("requested offset ")
                    && error
                        .message
                        .ends_with(" exceeds the maximum page offset 3"),
                "{name}: {}",
                error.message
            );
        }
        let served = execute(&capped, &at_maximum).await;
        assert!(served.errors.is_empty(), "{name}: {:?}", served.errors);
        assert_eq!(edges(served.data.into_json()?, path), 2, "{name}");
    }
    // The page builder that custom resolvers use applies the same bound.
    let page = graphql_orm::graphql::orm::PageInput {
        limit: Some(1),
        offset: Some(4),
    };
    assert_eq!(
        capped
            .client_pagination_config()
            .resolve_client_page(Some(&page), true)
            .map_err(|error| error.code)
            .err(),
        Some(graphql_orm::graphql::errors::OrmErrorCode::PageLimitExceeded)
    );
    assert!(
        db.client_pagination_config()
            .resolve_client_page(Some(&page), true)
            .is_ok()
    );
    Ok(())
}

/// Trusted Rust code pages a keyset with its own per-read maximum, so a
/// `bound + 1` look-ahead above the GraphQL maximum sees its extra row.
async fn host_keyset_reads_take_a_per_read_maximum(db: &Database) -> TestResult {
    let bound = 3_i64; // above the configured maximum of 2
    let page = |limit: i64| KeysetPageInput {
        limit: Some(limit),
        ..Default::default()
    };
    let read = EntityQuery::<LimitCard, Backend>::new()
        .with_max_limit(bound + 1)
        .fetch_authorized_keyset(db, None, None, page(bound + 1))
        .await
        .map_err(|error| error.message)?;
    assert_eq!(read.edges.len() as i64, bound + 1);
    assert!(read.page_info.has_next_page);

    // Paging by cursor with that maximum reads the whole set.
    let mut seen = Vec::new();
    let mut after = None;
    loop {
        let connection = EntityQuery::<LimitCard, Backend>::new()
            .with_max_limit(bound)
            .fetch_authorized_keyset(
                db,
                None,
                None,
                KeysetPageInput {
                    after: after.take(),
                    limit: Some(bound),
                    include_total_count: false,
                },
            )
            .await
            .map_err(|error| error.message)?;
        seen.extend(connection.edges.iter().map(|edge| edge.node.id.clone()));
        if !connection.page_info.has_next_page {
            break;
        }
        after = connection.page_info.end_cursor;
    }
    assert_eq!(seen.len(), CARDS);

    // The per-read maximum still bounds the page: with the refusal enabled a
    // limit above it fails rather than being reduced.
    let refusing = db.clone().with_oversized_page_limit_refusal(true);
    let refused = EntityQuery::<LimitCard, Backend>::new()
        .with_max_limit(bound)
        .fetch_authorized_keyset(&refusing, None, None, page(bound + 1))
        .await
        .expect_err("a limit above the per-read maximum is refused");
    assert_eq!(
        refused.extensions.as_ref().and_then(|ext| ext.get("code")),
        Some(&async_graphql::Value::from("PAGE_LIMIT_EXCEEDED"))
    );
    Ok(())
}

/// #674: a declared maximum is the one the catalogue publishes and the
/// complexity estimate uses, and a `Database` that applies another is refused.
fn declared_maximum_is_published_and_a_mismatched_database_is_refused(db: &Database) {
    use graphql_orm::graphql::orm::GraphqlSemanticTypeRef;
    assert_eq!(GRAPHQL_ORM_MAX_PAGE_LIMIT, Some(MAXIMUM as u32));
    let catalog = graphql_orm_semantic_catalog();
    let cards = catalog
        .entities
        .iter()
        .find(|entity| entity.entity_name == "LimitBoard")
        .and_then(|entity| {
            entity
                .fields
                .iter()
                .find(|field| field.field_name == "cards")
        })
        .expect("the relationship is catalogued");
    assert!(
        matches!(
            cards.type_ref,
            GraphqlSemanticTypeRef::List {
                maximum_items: Some(2),
                ..
            }
        ),
        "{:?}",
        cards.type_ref
    );
    // Building the schema for the matching database records the bound the
    // relation complexity estimate caps a requested page at.
    let _ = schema_builder(db.clone()).finish();

    // A database with the default maximum (100), or none, is refused.
    for mismatched in [
        db.clone()
            .with_pagination_config(PaginationConfig::secure()),
        db.clone()
            .with_pagination_config(PaginationConfig::secure().with_max_limit(None)),
    ] {
        let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = schema_builder(mismatched);
        }))
        .expect_err("a mismatched maximum page limit is refused at schema build");
        let message = refused
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default();
        assert!(
            message.contains("schema_roots! declares `max_page_limit: 2`"),
            "{message}"
        );
    }
}

#[cfg(feature = "sqlite")]
mod undeclared {
    use super::*;
    schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [LimitBoard, LimitCard], }
}

#[cfg(feature = "sqlite")]
mod large_declared {
    use super::*;
    schema_roots! { backend: "sqlite", max_page_limit: 200, query_custom_ops: [], entities: [LimitBoard, LimitCard], }
}

#[cfg(feature = "sqlite")]
async fn schema_complexity_bounds_are_isolated(db: &Database) -> TestResult {
    let query =
        "{ limitBoard(id: \"board\") { cards(page: { limit: 100 }) { edges { node { id } } } } }";
    // Build the declaration first: it must never lower the undeclared bound.
    let declared = schema_builder(db.clone()).limit_complexity(50).finish();
    let ordinary_db = db
        .clone()
        .with_pagination_config(PaginationConfig::secure());
    let ordinary = undeclared::schema_builder(ordinary_db.clone())
        .limit_complexity(50)
        .finish();
    let request = || async_graphql::Request::new(query).data(AuthSubject::new("reader"));
    let response = declared.execute(request()).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        edges(response.data.into_json()?, &["limitBoard", "cards"]),
        2
    );
    assert!(
        !ordinary.execute(request()).await.errors.is_empty(),
        "the undeclared schema must budget for 100 rows"
    );
    // A larger declaration must not raise an undeclared schema's estimate either.
    let _large = large_declared::schema_builder(
        db.clone()
            .with_pagination_config(PaginationConfig::secure().with_max_limit(Some(200))),
    )
    .finish();
    let moderate = undeclared::schema_builder(ordinary_db.clone())
        .limit_complexity(500)
        .finish();
    let large_request = async_graphql::Request::new(query.replace("limit: 100", "limit: 200"))
        .data(AuthSubject::new("reader"));
    let response = moderate.execute(large_request).await;
    assert!(
        response.errors.is_empty(),
        "a declaration above 100 must not reject an ordinary query: {:?}",
        response.errors
    );
    let ordinary = undeclared::schema_builder(ordinary_db).finish();
    let response = ordinary.execute(request()).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        edges(response.data.into_json()?, &["limitBoard", "cards"]),
        CARDS
    );
    Ok(())
}

async fn run() -> TestResult {
    let fixture = fixture().await?;
    #[cfg(feature = "sqlite")]
    schema_complexity_bounds_are_isolated(&fixture.db).await?;
    declared_maximum_is_published_and_a_mismatched_database_is_refused(&fixture.db);
    oversized_client_limits_are_reduced_unless_the_refusal_is_enabled(&fixture.db).await?;
    client_offsets_above_a_configured_maximum_are_refused(&fixture.db).await?;
    host_keyset_reads_take_a_per_read_maximum(&fixture.db).await?;
    fixture.finish()
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn page_limit_contract_on_sqlite() -> TestResult {
    run().await
}

#[cfg(not(feature = "sqlite"))]
#[tokio::test]
#[ignore = "starts a test-owned database container"]
async fn page_limit_contract_on_an_owned_database() -> TestResult {
    run().await
}
