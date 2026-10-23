#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "collections",
    plural = "Collections",
    default_sort = "name ASC",
    upsert = "slug"
)]
struct Collection {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[filterable(type = "string")]
    #[unique]
    pub slug: String,

    #[filterable(type = "uuid")]
    pub cover_stored_file_id: Option<graphql_orm::uuid::Uuid>,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,
}

schema_roots! {
    query_custom_ops: [],
    entities: [Collection],
}

#[cfg(feature = "sqlite")]
async fn setup_pool() -> Result<sqlx::SqlitePool, sqlx::Error> {
    sqlx::SqlitePool::connect("sqlite::memory:").await
}

#[cfg(feature = "postgres")]
async fn setup_pool() -> Result<sqlx::PgPool, Box<dyn std::error::Error>> {
    owned_postgres::OwnedPostgres::pool("graphql-naming", 5).await
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn generated_graphql_schema_uses_pascal_case_types_and_camel_case_fields() {
    let pool = setup_pool().await.expect("test pool");
    let schema = schema_builder(graphql_orm::db::Database::new(pool)).finish();

    let sdl = schema.sdl();
    let without_descriptions = sdl
        .split("\"\"\"")
        .enumerate()
        .filter_map(|(index, value)| (index % 2 == 0).then_some(value))
        .collect::<String>();
    let compact_sdl = without_descriptions
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();

    assert!(sdl.contains("type Collection "));
    assert!(sdl.contains("type CollectionResult "));
    assert!(sdl.contains("type CollectionConnection "));
    assert!(sdl.contains("type CollectionEdge "));

    assert!(compact_sdl.contains("collections(where:CollectionWhereInput,orderBy:[CollectionOrderByInput!],page:PageInput):CollectionConnection!"));
    assert!(compact_sdl.contains("collection(id:String!):Collection"));
    assert!(
        compact_sdl.contains("createCollection(input:CreateCollectionInput!):CollectionResult!")
    );
    assert!(
        compact_sdl
            .contains("upsertCollection(input:CreateCollectionInput!):UpsertCollectionResult!")
    );
    assert!(
        compact_sdl.contains(
            "updateCollection(id:String!,input:UpdateCollectionInput!):CollectionResult!"
        )
    );
    assert!(compact_sdl.contains("deleteCollection(id:String!):CollectionResult!"));
    let subscription_signature =
        "collectionChanged(filter:SubscriptionFilterInput,key:String):CollectionChangedEvent!";
    assert!(compact_sdl.contains(subscription_signature));

    assert!(sdl.contains("type CollectionResult {"));
    assert!(sdl.contains("\tsuccess: Boolean!"));
    assert!(sdl.contains("\terror: String"));
    assert!(sdl.contains("\tcollection: Collection"));
    assert!(sdl.contains("type UpsertCollectionResult {"));
    assert!(sdl.contains("\taction: ChangeAction"));
    assert!(sdl.contains("type CollectionConnection {"));
    assert!(sdl.contains("\tedges: [CollectionEdge!]!"));
    assert!(sdl.contains("\tpageInfo: PageInfo!"));
    assert!(sdl.contains("type CollectionEdge {"));
    assert!(sdl.contains("\tnode: Collection!"));
    assert!(sdl.contains("\tcursor: String!"));
    assert!(sdl.contains("type CollectionChangedEvent {"));
    assert!(sdl.contains("\taction: ChangeAction!"));
    assert!(sdl.contains("\tid: String!"));

    assert!(
        compact_sdl
            .contains("typeCollection{id:String!name:String!slug:String!coverStoredFileId:UUID")
    );
    assert!(sdl.contains(
        "input CreateCollectionInput {\n\tname: String!\n\tslug: String!\n\tcoverStoredFileId: UUID"
    ));
    assert!(sdl.contains(
        "input UpdateCollectionInput {\n\tname: String\n\tslug: String\n\tcoverStoredFileId: UUID"
    ));
    assert!(sdl.contains("input CollectionWhereInput {\n\tname: StringFilter"));
    assert!(sdl.contains("\tand: [CollectionWhereInput!]"));
    assert!(sdl.contains("\tor: [CollectionWhereInput!]"));
    assert!(sdl.contains("\tnot: CollectionWhereInput"));
    assert!(sdl.contains("input StringFilter {\n\teq: String"));
    assert!(sdl.contains("\tnotIn: [String!]"));
    assert!(sdl.contains("\tisNull: Boolean"));
    assert!(sdl.contains("input SimilarityInput {\n\tvalue: String!"));
    assert!(sdl.contains("input PageInput {"));
    assert!(sdl.contains("\tlimit: Int"));
    assert!(sdl.contains("\toffset: Int"));
    assert!(sdl.contains("input SubscriptionFilterInput"));
}
