use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "case_collections",
    plural = "CaseCollections",
    default_sort = "name ASC"
)]
#[serde(rename_all = "camelCase")]
struct CaseCollection {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[filterable(type = "uuid")]
    pub cover_stored_file_id: Option<graphql_orm::uuid::Uuid>,

    #[serde(rename = "SerdeExact")]
    pub serde_named_field: String,

    #[graphql(name = "ExactName")]
    pub exact_override: String,

    #[sortable]
    pub created_at: i64,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "override_items",
    plural = "OverrideItems",
    default_sort = "id ASC"
)]
#[graphql(rename_fields = "PascalCase")]
struct OverrideItem {
    #[primary_key]
    pub id: String,

    #[sortable]
    pub override_value: String,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "case_composite_records",
    plural = "CaseCompositeRecords",
    default_sort = "case_part ASC, part_ref ASC"
)]
struct CaseCompositeRecord {
    #[primary_key]
    #[graphql(name = "CasePart")]
    #[sortable]
    pub case_part: i32,

    #[primary_key]
    #[graphql(name = "PartRef")]
    #[sortable]
    pub part_ref: i32,

    pub label: String,
}

schema_roots! {
    query_custom_ops: [],
    entities: [CaseCollection, OverrideItem, CaseCompositeRecord],
}

#[cfg(feature = "sqlite")]
async fn setup_pool() -> Result<sqlx::SqlitePool, sqlx::Error> {
    sqlx::SqlitePool::connect("sqlite::memory:").await
}

#[cfg(feature = "postgres")]
async fn setup_pool() -> Result<sqlx::PgPool, sqlx::Error> {
    let database_url = std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| {
        "postgres://graphql_orm:graphql_orm@127.0.0.1:55433/graphql_orm_test".to_string()
    });
    sqlx::PgPool::connect(&database_url).await
}

fn assert_has(sdl: &str, expected: &str) {
    assert!(sdl.contains(expected), "SDL missing:\n{expected}\n\n{sdl}");
}

#[tokio::test]
async fn generated_schema_respects_crate_wide_case_features() {
    let pool = setup_pool().await.expect("test pool");
    let schema = schema_builder(graphql_orm::db::Database::new(pool)).finish();
    let sdl = schema.sdl();

    #[cfg(not(any(
        feature = "resolver-case-pascal",
        feature = "resolver-case-snake",
        feature = "resolver-case-screaming-snake",
        feature = "resolver-case-lower",
        feature = "resolver-case-upper",
        feature = "argument-case-pascal",
        feature = "argument-case-snake",
        feature = "argument-case-screaming-snake",
        feature = "argument-case-lower",
        feature = "argument-case-upper",
        feature = "field-case-pascal",
        feature = "field-case-snake",
        feature = "field-case-screaming-snake",
        feature = "field-case-lower",
        feature = "field-case-upper"
    )))]
    {
        assert_has(
            &sdl,
            "caseCollections(where: CaseCollectionWhereInput, orderBy: [CaseCollectionOrderByInput!], page: PageInput): CaseCollectionConnection!",
        );
        assert_has(&sdl, "caseCollection(id: String!): CaseCollection");
        assert_has(
            &sdl,
            "caseCompositeRecord(casePart: Int!, partRef: Int!): CaseCompositeRecord",
        );
        assert_has(
            &sdl,
            "createCaseCollection(input: CreateCaseCollectionInput!): CaseCollectionResult!",
        );
        assert_has(
            &sdl,
            "caseCollectionChanged(filter: SubscriptionFilterInput): CaseCollectionChangedEvent!",
        );
        assert_has(&sdl, "\tcoverStoredFileId: UUID");
        assert_has(&sdl, "\tSerdeExact: String!");
        assert_has(&sdl, "\tExactName: String!");
        assert_has(&sdl, "\tOverrideValue: String!");
        assert_has(&sdl, "\tpageInfo: PageInfo!");
        assert_has(&sdl, "\thasNextPage: Boolean!");
        assert_has(&sdl, "\tnotIn: [String!]");
    }

    #[cfg(all(
        feature = "resolver-case-pascal",
        feature = "argument-case-pascal",
        feature = "field-case-pascal"
    ))]
    {
        assert_has(
            &sdl,
            "CaseCollections(Where: CaseCollectionWhereInput, OrderBy: [CaseCollectionOrderByInput!], Page: PageInput): CaseCollectionConnection!",
        );
        assert_has(&sdl, "CaseCollection(Id: String!): CaseCollection");
        assert_has(
            &sdl,
            "CaseCompositeRecord(CasePart: Int!, PartRef: Int!): CaseCompositeRecord",
        );
        assert_has(
            &sdl,
            "CreateCaseCollection(Input: CreateCaseCollectionInput!): CaseCollectionResult!",
        );
        assert_has(
            &sdl,
            "CaseCollectionChanged(Filter: SubscriptionFilterInput): CaseCollectionChangedEvent!",
        );
        assert_has(&sdl, "\tCoverStoredFileId: UUID");
        assert_has(&sdl, "\tSerdeNamedField: String!");
        assert_has(&sdl, "\tExactName: String!");
        assert_has(&sdl, "\tOverrideValue: String!");
        assert_has(&sdl, "\tPageInfo: PageInfo!");
        assert_has(&sdl, "\tHasNextPage: Boolean!");
        assert_has(&sdl, "\tChangeKind: ChangeKind!");
        assert_has(&sdl, "\tSourceEntity: String");
        assert_has(&sdl, "\tNotIn: [String!]");
    }

    #[cfg(all(
        feature = "resolver-case-pascal",
        feature = "argument-case-pascal",
        feature = "field-case-snake"
    ))]
    {
        assert_has(
            &sdl,
            "CaseCollections(Where: CaseCollectionWhereInput, OrderBy: [CaseCollectionOrderByInput!], Page: PageInput): CaseCollectionConnection!",
        );
        assert_has(
            &sdl,
            "CreateCaseCollection(Input: CreateCaseCollectionInput!): CaseCollectionResult!",
        );
        assert_has(
            &sdl,
            "CaseCollectionChanged(Filter: SubscriptionFilterInput): CaseCollectionChangedEvent!",
        );
        assert_has(&sdl, "\tcover_stored_file_id: UUID");
        assert_has(&sdl, "\tserde_named_field: String!");
        assert_has(&sdl, "\tExactName: String!");
        assert_has(&sdl, "\tOverrideValue: String!");
        assert_has(&sdl, "\tpage_info: PageInfo!");
        assert_has(&sdl, "\thas_next_page: Boolean!");
        assert_has(&sdl, "\tchange_kind: ChangeKind!");
        assert_has(&sdl, "\tsource_entity: String");
        assert_has(&sdl, "\tnot_in: [String!]");
    }
}
