#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
use graphql_orm::prelude::*;

// The issue's entity with the diagnostic's suggested fix. The primary key
// remains the default order even though the only sortable field is `name`.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "records",
    plural = "Records",
    auth = "none",
    keyset = "id asc"
)]
struct Record {
    #[primary_key]
    id: String,
    #[sortable]
    name: String,
}

impl BatchLoadEntity for Record {
    fn batch_column() -> &'static str {
        "id"
    }
    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, graphql_orm::sqlx::Error> {
        use graphql_orm::sqlx::Row;
        row.try_get("id")
    }
}

#[derive(
    GraphQLEntity,
    GraphQLOperations,
    GraphQLRelations,
    graphql_orm::async_graphql::SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_entity(table = "parents", plural = "Parents", auth = "none")]
struct Parent {
    #[primary_key]
    #[sortable]
    id: String,
    label: String,
    #[graphql(skip)]
    #[serde(default)]
    #[relation(target = "Record", from = "id", to = "id", multiple, emit_fk = false)]
    records: Vec<Record>,
}

schema_roots! {
    query_custom_ops: [],
    entities: [Record, Parent],
}

#[tokio::test]
async fn adding_a_sortable_field_preserves_default_and_primary_key_keyset_order()
-> graphql_orm::Result<()> {
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let plan = db
        .schema()
        .plan_migration_to_entities("init", "records", &[Record::metadata(), Parent::metadata()])
        .await?;
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    for (id, name) in [("b", "a"), ("a", "z")] {
        graphql_orm::sqlx::query("INSERT INTO records (id, name) VALUES (?, ?)")
            .bind(id)
            .bind(name)
            .execute(db.pool())
            .await?;
    }
    graphql_orm::sqlx::query("INSERT INTO parents (id, label) VALUES ('a', 'parent')")
        .execute(db.pool())
        .await?;
    let schema = schema_builder(db.clone()).finish();
    let response = schema
        .execute("{ records { edges { node { id } } } }")
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data["records"]["edges"][0]["node"]["id"], "a");
    let related = schema
        .execute("{ parents { edges { node { records { edges { node { id } } } } } } }")
        .await;
    assert!(related.errors.is_empty(), "{:?}", related.errors);
    assert_eq!(
        related.data.into_json().unwrap()["parents"]["edges"][0]["node"]["records"]["edges"][0]["node"]
            ["id"],
        "a"
    );
    let sdl = schema.sdl();
    assert!(sdl.contains("input RecordOrderByInput"), "{sdl}");
    assert!(sdl.contains("orderBy: RecordOrderByInput"), "{sdl}");
    let catalog = graphql_orm_operation_catalog();
    let list = catalog
        .operations()
        .iter()
        .find(|operation| operation.field_name() == "records")
        .unwrap();
    assert!(
        list.arguments()
            .iter()
            .any(|argument| argument.graphql_name() == "orderBy")
    );
    let first = Record::keyset_page(
        &db,
        RecordWhereInput::default(),
        KeysetPageInput {
            limit: Some(1),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(first.edges[0].node.id, "a");
    let second = Record::keyset_page(
        &db,
        RecordWhereInput::default(),
        KeysetPageInput {
            after: first.page_info.end_cursor,
            limit: Some(1),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(second.edges[0].node.id, "b");
    Ok(())
}
