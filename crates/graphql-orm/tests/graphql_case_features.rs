// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "postgres"))]

#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

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
async fn setup_pool() -> Result<sqlx::PgPool, Box<dyn std::error::Error>> {
    owned_postgres::OwnedPostgres::pool("graphql-case-features", 5).await
}

#[allow(dead_code)]
fn assert_has(sdl: &str, expected: &str) {
    let without_descriptions = sdl
        .split("\"\"\"")
        .enumerate()
        .filter_map(|(index, value)| (index % 2 == 0).then_some(value))
        .collect::<String>();
    let compact_sdl = without_descriptions
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let compact_expected = expected
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    assert!(
        compact_sdl.contains(&compact_expected),
        "SDL missing:\n{expected}\n\n{sdl}"
    );
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
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

    #[cfg(graphql_orm_resolver_case = "pascal")]
    assert!(federation.query_fields().contains("CaseCollections"));

    #[cfg(not(any(
        graphql_orm_resolver_case = "pascal",
        graphql_orm_resolver_case = "snake",
        graphql_orm_resolver_case = "screaming_snake",
        graphql_orm_resolver_case = "lower",
        graphql_orm_resolver_case = "upper",
        graphql_orm_argument_case = "pascal",
        graphql_orm_argument_case = "snake",
        graphql_orm_argument_case = "screaming_snake",
        graphql_orm_argument_case = "lower",
        graphql_orm_argument_case = "upper",
        graphql_orm_field_case = "pascal",
        graphql_orm_field_case = "snake",
        graphql_orm_field_case = "screaming_snake",
        graphql_orm_field_case = "lower",
        graphql_orm_field_case = "upper"
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
            "caseCollectionChanged(filter: SubscriptionFilterInput, key: String, keys: [String!]): CaseCollectionChangedEvent!",
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
        graphql_orm_resolver_case = "pascal",
        graphql_orm_argument_case = "pascal",
        graphql_orm_field_case = "pascal"
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
            "CaseCollectionChanged(Filter: SubscriptionFilterInput, Key: String, Keys: [String!]): CaseCollectionChangedEvent!",
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
        graphql_orm_resolver_case = "pascal",
        graphql_orm_argument_case = "pascal",
        graphql_orm_field_case = "snake"
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
            "CaseCollectionChanged(Filter: SubscriptionFilterInput, Key: String, Keys: [String!]): CaseCollectionChangedEvent!",
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
        graphql_orm_field_case = "pascal",
        graphql_orm_field_case = "snake",
        graphql_orm_field_case = "screaming_snake",
        graphql_orm_field_case = "lower",
        graphql_orm_field_case = "upper"
    )))]
    {
        // Default camelCase; serde renames are honored when no case feature is active.
        assert_eq!(api("cover_stored_file_id"), "coverStoredFileId");
        assert_eq!(api("serde_named_field"), "SerdeExact");
    }
    #[cfg(graphql_orm_field_case = "pascal")]
    {
        assert_eq!(api("cover_stored_file_id"), "CoverStoredFileId");
        assert_eq!(api("serde_named_field"), "SerdeNamedField");
    }
    #[cfg(graphql_orm_field_case = "snake")]
    {
        assert_eq!(api("cover_stored_file_id"), "cover_stored_file_id");
        assert_eq!(api("serde_named_field"), "serde_named_field");
    }
    #[cfg(graphql_orm_field_case = "screaming_snake")]
    {
        assert_eq!(api("cover_stored_file_id"), "COVER_STORED_FILE_ID");
        assert_eq!(api("serde_named_field"), "SERDE_NAMED_FIELD");
    }
    #[cfg(graphql_orm_field_case = "lower")]
    {
        assert_eq!(api("cover_stored_file_id"), "coverstoredfileid");
        assert_eq!(api("serde_named_field"), "serdenamedfield");
    }
    #[cfg(graphql_orm_field_case = "upper")]
    {
        assert_eq!(api("cover_stored_file_id"), "COVERSTOREDFILEID");
        assert_eq!(api("serde_named_field"), "SERDENAMEDFIELD");
    }
}

/// graphql-orm and graphql-orm-macros read the naming variables with the same
/// build script, so the runtime types and the generated code agree (#508).
#[test]
fn naming_build_scripts_are_identical() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let orm = std::fs::read_to_string(root.join("build.rs")).unwrap();
    let macros = std::fs::read_to_string(root.join("../graphql-orm-macros/build.rs")).unwrap();
    assert_eq!(orm, macros);
}
