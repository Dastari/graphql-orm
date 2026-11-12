// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(feature = "sqlite")]

//! The generated `search_similar` helper reads five candidates per requested
//! result. That candidate bound is the read's own maximum: it is honoured
//! exactly, never reduced to the page maximum, and a requested limit above the
//! documented maximum is refused.

use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::graphql::orm::SIMILAR_SEARCH_MAX_LIMIT;
use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[graphql_entity(
    table = "similar_items",
    plural = "SimilarItems",
    default_sort = "id ASC"
)]
struct SimilarItem {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    name: String,
}

schema_roots! {
    query_custom_ops: [],
    entities: [SimilarItem],
}

const ROWS: usize = 150;

async fn database() -> Database<SqliteBackend> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap()
        .with_schema_policy(SchemaPolicy::Managed);
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "search-similar-limits",
            "search similar limits",
            &[SimilarItem::metadata()],
        )
        .await
        .unwrap();
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .unwrap();
    for index in 0..ROWS {
        sqlx::query("INSERT INTO similar_items (id, name) VALUES (?, ?)")
            .bind(format!("item-{index:04}"))
            .bind(format!("example {index:04}"))
            .execute(database.pool())
            .await
            .unwrap();
    }
    database
}

#[track_caller]
fn assert_code<T: std::fmt::Debug>(result: graphql_orm::Result<T>, code: OrmErrorCode) {
    let error = result.expect_err("the search must be refused");
    assert_eq!(OrmPublicError::from_sqlx(&error).code, code, "{error:?}");
}

#[tokio::test]
async fn search_similar_candidate_bound_is_not_reduced_to_the_page_maximum() {
    let database = database().await;
    assert_eq!(PaginationConfig::DEFAULT_MAX_LIMIT, 100);

    // The documented example: 25 results read 125 candidates, above the
    // default page maximum of 100.
    let matches =
        SimilarItem::search_similar(database.pool(), "name", "example", 0.1, None, Some(25))
            .await
            .unwrap();
    assert_eq!(matches.len(), 25);

    // Every candidate is scored when the bound admits all rows.
    let matches =
        SimilarItem::search_similar(database.pool(), "name", "example", 0.1, None, Some(200))
            .await
            .unwrap();
    assert_eq!(matches.len(), ROWS);

    assert!(
        SimilarItem::search_similar(
            database.pool(),
            "name",
            "example",
            0.1,
            None,
            Some(SIMILAR_SEARCH_MAX_LIMIT),
        )
        .await
        .is_ok()
    );
    assert_code(
        SimilarItem::search_similar(
            database.pool(),
            "name",
            "example",
            0.1,
            None,
            Some(SIMILAR_SEARCH_MAX_LIMIT + 1),
        )
        .await,
        OrmErrorCode::PageLimitExceeded,
    );
    assert_code(
        SimilarItem::search_similar(database.pool(), "name", "example", 0.1, None, Some(-1)).await,
        OrmErrorCode::InvalidInput,
    );
}
