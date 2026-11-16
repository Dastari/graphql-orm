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
//! connections. A schema's maximum is fixed at compile time and a `Database`
//! that applies another is refused. Trusted Rust code gives one keyset read
//! its own maximum with `EntityQuery::with_max_limit`; an over-limit host
//! request is an error, never a shorter page.
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
struct LimitCard {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
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

/// The same entities in a schema that declares no maximum page limit.
mod undeclared {
    use super::*;
    #[cfg(feature = "sqlite")]
    schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [LimitBoard, LimitCard], }
    #[cfg(feature = "postgres")]
    schema_roots! { backend: "postgres", query_custom_ops: [], entities: [LimitBoard, LimitCard], }
    #[cfg(feature = "mssql")]
    schema_roots! {
        backend: "mssql",
        schema_policy: "external_writable",
        query_custom_ops: [],
        entities: [LimitBoard, LimitCard],
    }
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
const PAGES: [(&str, &str, &[&str]); 3] = [
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

    // An over-limit request from trusted Rust code is an error, never a
    // shorter page: above the per-read maximum, above the database maximum on
    // the generated repository helper, and inside a transaction.
    let code = |error: &async_graphql::Error| {
        error
            .extensions
            .as_ref()
            .and_then(|ext| ext.get("code"))
            .cloned()
    };
    let exceeded = Some(async_graphql::Value::from("PAGE_LIMIT_EXCEEDED"));
    let refused = EntityQuery::<LimitCard, Backend>::new()
        .with_max_limit(bound)
        .fetch_authorized_keyset(db, None, None, page(bound + 1))
        .await
        .expect_err("a limit above the per-read maximum is refused");
    assert_eq!(code(&refused), exceeded);
    let refused = LimitCard::keyset_page(db, LimitCardWhereInput::default(), page(MAXIMUM + 1))
        .await
        .expect_err("a limit above the database maximum is refused");
    assert_eq!(
        refused.code,
        graphql_orm::graphql::errors::OrmErrorCode::PageLimitExceeded
    );
    let refused = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                tx.keyset_page::<LimitCard>(
                    LimitCardWhereInput::default(),
                    KeysetPageInput {
                        limit: Some(MAXIMUM + 1),
                        ..Default::default()
                    },
                )
                .await
            })
        })
        .await
        .expect_err("a transaction keyset limit above the maximum is refused");
    let TransactionError::Rejected(error) = refused else {
        panic!("expected a rejected transaction, got {refused:?}");
    };
    assert_eq!(
        error.code,
        graphql_orm::graphql::errors::OrmErrorCode::PageLimitExceeded
    );
    let refused = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                tx.keyset_connection_page::<LimitCard>(
                    LimitCardWhereInput::default(),
                    graphql_orm::graphql::pagination::KeysetConnectionInput {
                        last: Some(MAXIMUM + 1),
                        ..Default::default()
                    },
                )
                .await
            })
        })
        .await
        .expect_err("a backward transaction keyset limit above the maximum is refused");
    let TransactionError::Rejected(error) = refused else {
        panic!("expected a rejected transaction, got {refused:?}");
    };
    assert_eq!(
        error.code,
        graphql_orm::graphql::errors::OrmErrorCode::PageLimitExceeded
    );
    let at_maximum = LimitCard::keyset_page(db, LimitCardWhereInput::default(), page(MAXIMUM))
        .await
        .map_err(|error| error.message)?;
    assert_eq!(at_maximum.edges.len() as i64, MAXIMUM);
    // Authorized scans have the same exact-or-refused host contract, including
    // the per-read override. They do not silently truncate overflow checks.
    let scanning = db
        .clone()
        .with_authorized_scan_config(AuthorizedScanConfig::new([71; 32], 8, 64));
    let refused = EntityQuery::<LimitCard, Backend>::new()
        .fetch_authorized_scan(
            &scanning,
            None,
            None,
            AuthorizedScanInput {
                limit: Some(MAXIMUM + 1),
                ..Default::default()
            },
        )
        .await
        .expect_err("an authorized scan above the maximum is refused");
    assert_eq!(code(&refused), exceeded);
    let scan = EntityQuery::<LimitCard, Backend>::new()
        .with_max_limit(bound + 1)
        .fetch_authorized_scan(
            &scanning,
            None,
            None,
            AuthorizedScanInput {
                limit: Some(bound + 1),
                ..Default::default()
            },
        )
        .await
        .map_err(|error| error.message)?;
    assert_eq!(scan.nodes.len() as i64, bound + 1);

    // An omitted limit is not a request for more than the maximum.
    let omitted = LimitCard::keyset_page(
        db,
        LimitCardWhereInput::default(),
        KeysetPageInput::default(),
    )
    .await
    .map_err(|error| error.message)?;
    assert_eq!(omitted.edges.len() as i64, MAXIMUM);
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

    // A schema that declares no maximum has the default of 100 and refuses
    // this fixture's database, whose maximum is 2.
    assert_eq!(undeclared::GRAPHQL_ORM_MAX_PAGE_LIMIT, Some(100));
    let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = undeclared::schema_builder(db.clone());
    }))
    .expect_err("an undeclared schema refuses a database with another maximum");
    let message = refused
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_default();
    assert!(
        message.contains("this schema's maximum page limit is 100"),
        "{message}"
    );
    let _ = undeclared::schema_builder(
        db.clone()
            .with_pagination_config(PaginationConfig::secure()),
    )
    .finish();

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
            message.contains("this schema's maximum page limit is 2"),
            "{message}"
        );
    }
}

mod large_declared {
    use super::*;
    #[cfg(feature = "sqlite")]
    schema_roots! { backend: "sqlite", max_page_limit: 200, query_custom_ops: [], entities: [LimitBoard, LimitCard], }
    #[cfg(feature = "postgres")]
    schema_roots! { backend: "postgres", max_page_limit: 200, query_custom_ops: [], entities: [LimitBoard, LimitCard], }
    #[cfg(feature = "mssql")]
    schema_roots! { backend: "mssql", schema_policy: "external_writable", max_page_limit: 200, query_custom_ops: [], entities: [LimitBoard, LimitCard], }
}

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

/// #667: a trusted `EntityQuery::paginate` read (connection or `fetch_all*`)
/// is exact or refused, never reduced, with or without the client refusal
/// opt-in. A per-read `with_max_limit` admits a larger exact page.
async fn host_connection_reads_are_exact_or_refused(db: &Database) -> TestResult {
    use graphql_orm::graphql::pagination::ConnectionSelection;
    let oversized = PageInput {
        limit: Some(MAXIMUM + 1),
        offset: Some(0),
    };
    let query = || EntityQuery::<LimitCard, Backend>::new().paginate(&oversized);
    let selection = ConnectionSelection {
        rows: true,
        count: false,
        has_next_page: true,
    };
    let refused = |result: graphql_orm::Result<Connection<LimitCard>>, name: &str| {
        let error = OrmPublicError::from(result.expect_err(name));
        assert_eq!(
            error.code,
            OrmErrorCode::PageLimitExceeded,
            "{name}: {error:?}"
        );
    };
    for handle in [
        db.clone(),
        db.clone().with_oversized_page_limit_refusal(true),
    ] {
        refused(query().fetch_connection(&handle).await, "fetch_connection");
        refused(
            query().fetch_connection_with_auth(&handle, None).await,
            "fetch_connection_with_auth",
        );
        refused(
            query()
                .fetch_connection_selected_with_auth(&handle, None, selection)
                .await,
            "fetch_connection_selected_with_auth",
        );
        let exact = || query().with_max_limit(MAXIMUM + 1);
        for (name, connection) in [
            ("fetch_connection", exact().fetch_connection(&handle).await?),
            (
                "fetch_connection_with_auth",
                exact().fetch_connection_with_auth(&handle, None).await?,
            ),
            (
                "fetch_connection_selected_with_auth",
                exact()
                    .fetch_connection_selected_with_auth(&handle, None, selection)
                    .await?,
            ),
        ] {
            assert_eq!(connection.edges.len() as i64, MAXIMUM + 1, "{name}");
            assert!(connection.page_info.has_next_page, "{name}");
        }
        // A limit above the per-read maximum is still refused.
        refused(
            query()
                .with_max_limit(MAXIMUM)
                .fetch_connection(&handle)
                .await,
            "per-read maximum",
        );

        // `fetch_all*` reads of the page follow the same rule.
        let refused_rows = |result: graphql_orm::Result<Vec<LimitCard>>, name: &str| {
            let error = OrmPublicError::from(result.expect_err(name));
            assert_eq!(
                error.code,
                OrmErrorCode::PageLimitExceeded,
                "{name}: {error:?}"
            );
        };
        refused_rows(query().fetch_all(&handle).await, "fetch_all");
        refused_rows(
            query().fetch_all_with_auth(&handle, None).await,
            "fetch_all_with_auth",
        );
        assert!(
            query().fetch_one(&handle).await.is_err(),
            "fetch_one reads the same page"
        );
        for (name, rows) in [
            ("fetch_all", exact().fetch_all(&handle).await?),
            (
                "fetch_all_with_auth",
                exact().fetch_all_with_auth(&handle, None).await?,
            ),
        ] {
            assert_eq!(rows.len() as i64, MAXIMUM + 1, "{name}");
        }
    }

    // `fetch_all_on` cannot see the database's maximum, so it checks the
    // default maximum: exact up to it, refused above it, never reduced.
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    {
        let page = |limit: i64| PageInput {
            limit: Some(limit),
            offset: Some(0),
        };
        let rows = EntityQuery::<LimitCard, Backend>::new()
            .paginate(&page(3))
            .fetch_all_on(db.pool())
            .await?;
        assert_eq!(rows.len(), 3, "fetch_all_on within the default maximum");
        let over = EntityQuery::<LimitCard, Backend>::new()
            .paginate(&page(PaginationConfig::DEFAULT_MAX_LIMIT + 1))
            .fetch_all_on(db.pool())
            .await
            .expect_err("fetch_all_on above the default maximum");
        assert_eq!(
            OrmPublicError::from(over).code,
            OrmErrorCode::PageLimitExceeded
        );
    }
    Ok(())
}

async fn run() -> TestResult {
    let fixture = fixture().await?;
    schema_complexity_bounds_are_isolated(&fixture.db).await?;
    declared_maximum_is_published_and_a_mismatched_database_is_refused(&fixture.db);
    oversized_client_limits_are_reduced_unless_the_refusal_is_enabled(&fixture.db).await?;
    host_keyset_reads_take_a_per_read_maximum(&fixture.db).await?;
    host_connection_reads_are_exact_or_refused(&fixture.db).await?;
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
