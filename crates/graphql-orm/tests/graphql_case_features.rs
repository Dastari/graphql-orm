#[path = "support/federation_sdl.rs"]
mod federation_sdl;

use federation_sdl::ParsedFederationSchema;
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

#[allow(dead_code)]
fn assert_has(sdl: &str, expected: &str) {
    assert!(sdl.contains(expected), "SDL missing:\n{expected}\n\n{sdl}");
}

#[tokio::test]
async fn generated_schema_respects_crate_wide_case_features() {
    let pool = setup_pool().await.expect("test pool");
    let schema = schema_builder(graphql_orm::db::Database::new(pool))
        .enable_subscription_in_federation()
        .finish();
    let sdl = schema.sdl();
    assert!(!sdl.is_empty());
    let federation_sdl =
        schema.sdl_with_options(graphql_orm::async_graphql::SDLExportOptions::new().federation());
    let federation = ParsedFederationSchema::parse(&federation_sdl);

    assert_eq!(federation.query, "Query");
    assert_eq!(federation.mutation.as_deref(), Some("Mutation"));
    assert_eq!(federation.subscription.as_deref(), Some("Subscription"));
    assert!(!federation.query_fields().is_empty());
    assert!(!federation.objects.contains_key("QueryRoot"));

    #[cfg(feature = "resolver-case-pascal")]
    assert!(federation.query_fields().contains("CaseCollections"));

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

/// `ColumnDef.api_name` must record the same public name the generated GraphQL fields use,
/// under every naming configuration. Run separately per field-case feature, matching the SDL
/// assertions above.
#[test]
fn column_api_names_follow_the_active_naming_configuration() {
    let case_collection = CaseCollection::metadata();
    let api = |rust_name: &str| -> &'static str {
        case_collection
            .fields
            .iter()
            .find(|field| field.rust_name == rust_name)
            .unwrap_or_else(|| panic!("field {rust_name} present"))
            .api_name
    };

    // An explicit #[graphql(name = ...)] override wins under every configuration.
    assert_eq!(api("exact_override"), "ExactName");

    // An entity-level #[graphql(rename_fields = ...)] wins over case features.
    let override_item = OverrideItem::metadata();
    let override_value = override_item
        .fields
        .iter()
        .find(|field| field.rust_name == "override_value")
        .expect("override_value present");
    assert_eq!(override_value.api_name, "OverrideValue");

    #[cfg(not(any(
        feature = "field-case-pascal",
        feature = "field-case-snake",
        feature = "field-case-screaming-snake",
        feature = "field-case-lower",
        feature = "field-case-upper"
    )))]
    {
        // Default camelCase; serde renames are honored when no case feature is active.
        assert_eq!(api("cover_stored_file_id"), "coverStoredFileId");
        assert_eq!(api("serde_named_field"), "SerdeExact");
    }
    #[cfg(feature = "field-case-pascal")]
    {
        assert_eq!(api("cover_stored_file_id"), "CoverStoredFileId");
        assert_eq!(api("serde_named_field"), "SerdeNamedField");
    }
    #[cfg(feature = "field-case-snake")]
    {
        assert_eq!(api("cover_stored_file_id"), "cover_stored_file_id");
        assert_eq!(api("serde_named_field"), "serde_named_field");
    }
    #[cfg(feature = "field-case-screaming-snake")]
    {
        assert_eq!(api("cover_stored_file_id"), "COVER_STORED_FILE_ID");
        assert_eq!(api("serde_named_field"), "SERDE_NAMED_FIELD");
    }
    #[cfg(feature = "field-case-lower")]
    {
        assert_eq!(api("cover_stored_file_id"), "coverstoredfileid");
        assert_eq!(api("serde_named_field"), "serdenamedfield");
    }
    #[cfg(feature = "field-case-upper")]
    {
        assert_eq!(api("cover_stored_file_id"), "COVERSTOREDFILEID");
        assert_eq!(api("serde_named_field"), "SERDENAMEDFIELD");
    }
}
