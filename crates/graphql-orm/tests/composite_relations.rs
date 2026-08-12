#![cfg(feature = "sqlite")]

use graphql_orm::async_graphql::{Schema, SimpleObject};
use graphql_orm::prelude::*;
use graphql_orm::sqlx::Row;

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
    PartialEq,
)]
#[graphql(rename_fields = "PascalCase")]
#[graphql(complex)]
#[graphql_entity(
    backend = "sqlite",
    table = "LegacyCardFile",
    plural = "LegacyCardFiles",
    schema_policy = "external_read_only",
    default_sort = "CardNo ASC"
)]
pub struct LegacyCardFile {
    #[primary_key]
    #[graphql(name = "CardNo")]
    #[graphql_orm(db_column = "CardNo")]
    #[filterable(type = "number")]
    #[sortable]
    pub card_no: i32,

    #[graphql(name = "CardCode")]
    #[graphql_orm(db_column = "CardCode")]
    #[filterable(type = "string")]
    pub card_code: String,

    #[graphql(name = "Name")]
    #[graphql_orm(db_column = "Name")]
    #[filterable(type = "string")]
    pub name: Option<String>,

    #[graphql(skip, name = "Contacts")]
    #[relation(
        target = "LegacyCardFileContact",
        from = "card_no",
        to = "CardNo",
        multiple,
        emit_fk = false
    )]
    pub contacts: Vec<LegacyCardFileContact>,
}

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
    PartialEq,
)]
#[graphql(rename_fields = "PascalCase")]
#[graphql(complex)]
#[graphql_entity(
    backend = "sqlite",
    table = "LegacyCardFileContacts",
    plural = "LegacyCardFileContacts",
    schema_policy = "external_read_only",
    default_sort = "CardNo ASC, ContNo ASC"
)]
pub struct LegacyCardFileContact {
    #[primary_key]
    #[graphql(name = "CardNo")]
    #[graphql_orm(db_column = "CardNo")]
    #[filterable(type = "number")]
    #[sortable]
    pub card_no: i32,

    #[primary_key]
    #[graphql(name = "ContNo")]
    #[graphql_orm(db_column = "ContNo")]
    #[filterable(type = "number")]
    #[sortable]
    pub cont_no: i32,

    #[graphql(name = "DName")]
    #[graphql_orm(db_column = "DName")]
    #[filterable(type = "string")]
    pub display_name: Option<String>,

    #[graphql(skip, name = "Details")]
    #[relation(
        target = "LegacyCardFileDetail",
        from = ["card_no", "cont_no"],
        to = ["CardNo", "ContNo"],
        multiple,
        emit_fk = false
    )]
    pub details: Vec<LegacyCardFileDetail>,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql(rename_fields = "PascalCase")]
#[graphql_entity(
    backend = "sqlite",
    table = "LegacyCardFileDetails",
    plural = "LegacyCardFileDetails",
    schema_policy = "external_read_only",
    default_sort = "CardNo ASC, ContNo ASC, LineNum ASC"
)]
pub struct LegacyCardFileDetail {
    #[primary_key]
    #[graphql(name = "CardNo")]
    #[graphql_orm(db_column = "CardNo")]
    #[filterable(type = "number")]
    #[sortable]
    pub card_no: i32,

    #[primary_key]
    #[graphql(name = "ContNo")]
    #[graphql_orm(db_column = "ContNo")]
    #[filterable(type = "number")]
    #[sortable]
    pub cont_no: i32,

    #[primary_key]
    #[graphql(name = "LineNum")]
    #[graphql_orm(db_column = "LineNum")]
    #[filterable(type = "number")]
    #[sortable]
    pub line_num: i16,

    #[graphql(name = "Type")]
    #[graphql_orm(db_column = "Type")]
    #[filterable(type = "string")]
    pub detail_type: Option<String>,

    #[graphql(name = "Value")]
    #[graphql_orm(db_column = "Value")]
    #[filterable(type = "string")]
    pub value: Option<String>,
}

impl graphql_orm::graphql::loaders::BatchLoadEntity<graphql_orm::SqliteBackend>
    for LegacyCardFileContact
{
    fn batch_column() -> &'static str {
        "CardNo"
    }

    fn batch_key_from_row(
        row: &graphql_orm::sqlx::sqlite::SqliteRow,
    ) -> Result<String, sqlx::Error> {
        row.try_get::<i32, _>("CardNo")
            .map(|value| value.to_string())
    }
}

impl graphql_orm::graphql::loaders::BatchLoadEntity<graphql_orm::SqliteBackend>
    for LegacyCardFileDetail
{
    fn batch_column() -> &'static str {
        "CardNo"
    }

    fn batch_key_from_row(
        row: &graphql_orm::sqlx::sqlite::SqliteRow,
    ) -> Result<String, sqlx::Error> {
        row.try_get::<i32, _>("CardNo")
            .map(|value| value.to_string())
    }
}

schema_roots! {
    backend: "sqlite",
    schema_policy: "external_read_only",
    query_custom_ops: [],
    entities: [LegacyCardFile, LegacyCardFileContact, LegacyCardFileDetail],
}

type TestSchema = Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

async fn setup_schema() -> Result<TestSchema, Box<dyn std::error::Error>> {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;

    sqlx::query(
        "CREATE TABLE LegacyCardFile (
            CardNo INTEGER PRIMARY KEY,
            CardCode TEXT NOT NULL,
            Name TEXT NULL
        )",
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        "CREATE TABLE LegacyCardFileContacts (
            CardNo INTEGER NOT NULL,
            ContNo INTEGER NOT NULL,
            DName TEXT NULL,
            PRIMARY KEY (CardNo, ContNo)
        )",
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        "CREATE TABLE LegacyCardFileDetails (
            CardNo INTEGER NOT NULL,
            ContNo INTEGER NOT NULL,
            LineNum INTEGER NOT NULL,
            Type TEXT NULL,
            Value TEXT NULL,
            PRIMARY KEY (CardNo, ContNo, LineNum)
        )",
    )
    .execute(&pool)
    .await?;

    for (card_no, card_code, name) in [(1001, "ACME", "Acme Pty Ltd"), (1002, "GLOB", "Globex")] {
        sqlx::query("INSERT INTO LegacyCardFile (CardNo, CardCode, Name) VALUES (?, ?, ?)")
            .bind(card_no)
            .bind(card_code)
            .bind(name)
            .execute(&pool)
            .await?;
    }

    for (card_no, cont_no, display_name) in [
        (1001, 1, "Alice Admin"),
        (1001, 2, "Alex Accounts"),
        (1002, 1, "Bob Buyer"),
    ] {
        sqlx::query("INSERT INTO LegacyCardFileContacts (CardNo, ContNo, DName) VALUES (?, ?, ?)")
            .bind(card_no)
            .bind(cont_no)
            .bind(display_name)
            .execute(&pool)
            .await?;
    }

    for (card_no, cont_no, line_num, detail_type, value) in [
        (1001, 1, 1, "Email", "alice@example.test"),
        (1001, 1, 2, "Phone", "555-0101"),
        (1001, 2, 1, "Email", "accounts@example.test"),
        (1002, 1, 1, "Email", "bob@example.test"),
        (1002, 1, 2, "Phone", "555-0201"),
    ] {
        sqlx::query(
            "INSERT INTO LegacyCardFileDetails (CardNo, ContNo, LineNum, Type, Value)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(card_no)
        .bind(cont_no)
        .bind(line_num)
        .bind(detail_type)
        .bind(value)
        .execute(&pool)
        .await?;
    }

    Ok(schema_builder(graphql_orm::db::Database::new(pool))
        .data("test-user".to_string())
        .finish())
}

#[tokio::test]
async fn nested_composite_relations_batch_without_n_plus_one()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = setup_schema().await?;

    graphql_orm::graphql::orm::reset_query_count();

    let response = schema
        .execute(
            r#"
            query {
              legacyCardFiles(orderBy: [{ CardNo: ASC }]) {
                edges {
                  node {
                    CardNo
                    CardCode
                    Name
                    Contacts(
                      where: { DName: { ne: "Nobody" } }
                      orderBy: { ContNo: ASC }
                      page: { limit: 10, offset: 0 }
                    ) {
                      edges {
                        node {
                          CardNo
                          ContNo
                          DName
                          Details(
                            where: { Type: { eq: "Email" } }
                            orderBy: { LineNum: ASC }
                            page: { limit: 10, offset: 0 }
                          ) {
                            edges {
                              node { Type Value }
                            }
                            pageInfo { totalCount hasNextPage }
                          }
                        }
                      }
                    }
                  }
                }
              }
            }
            "#,
        )
        .await;

    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json()?;
    let cards = data["legacyCardFiles"]["edges"].as_array().unwrap();
    assert_eq!(cards.len(), 2);

    let first_contacts = cards[0]["node"]["Contacts"]["edges"].as_array().unwrap();
    assert_eq!(first_contacts.len(), 2);
    assert_eq!(
        first_contacts[0]["node"]["Details"]["edges"][0]["node"]["Value"].as_str(),
        Some("alice@example.test")
    );
    assert_eq!(
        first_contacts[1]["node"]["Details"]["edges"][0]["node"]["Value"].as_str(),
        Some("accounts@example.test")
    );

    let second_contacts = cards[1]["node"]["Contacts"]["edges"].as_array().unwrap();
    assert_eq!(second_contacts.len(), 1);
    assert_eq!(
        second_contacts[0]["node"]["Details"]["edges"][0]["node"]["Value"].as_str(),
        Some("bob@example.test")
    );

    assert!(
        graphql_orm::graphql::orm::query_count() <= 4,
        "expected parent query + contact preload + detail batch query, got {} queries",
        graphql_orm::graphql::orm::query_count()
    );

    Ok(())
}

#[test]
fn composite_relation_predicates_render_for_all_backends() {
    use graphql_orm::graphql::loaders::relation_key_filter;
    use graphql_orm::graphql::orm::{DatabaseBackend, SelectQuery, SqlValue, render_select_query};

    let keys = vec![
        vec![SqlValue::Int(1001), SqlValue::Int(1)],
        vec![SqlValue::Int(1001), SqlValue::Int(2)],
    ];

    let sqlite = render_select_query(
        DatabaseBackend::Sqlite,
        &SelectQuery {
            table: "LegacyCardFileDetails",
            columns: vec!["*".to_string()],
            filter: Some(relation_key_filter(&["CardNo", "ContNo"], &keys)),
            sorts: Vec::new(),
            pagination: None,
            count_only: false,
        },
    );
    assert!(
        sqlite
            .sql
            .contains("(CardNo = ? AND ContNo = ?) OR (CardNo = ? AND ContNo = ?)")
    );
    assert_eq!(sqlite.values.len(), 4);

    let postgres = render_select_query(
        DatabaseBackend::Postgres,
        &SelectQuery {
            table: "\"LegacyCardFileDetails\"",
            columns: vec!["*".to_string()],
            filter: Some(relation_key_filter(&["\"CardNo\"", "\"ContNo\""], &keys)),
            sorts: Vec::new(),
            pagination: None,
            count_only: false,
        },
    );
    assert!(postgres.sql.contains(
        "(\"CardNo\" = $1 AND \"ContNo\" = $2) OR (\"CardNo\" = $3 AND \"ContNo\" = $4)"
    ));

    let mssql = render_select_query(
        DatabaseBackend::Mssql,
        &SelectQuery {
            table: "[dbo].[LegacyCardFileDetails]",
            columns: vec!["*".to_string()],
            filter: Some(relation_key_filter(&["[CardNo]", "[ContNo]"], &keys)),
            sorts: Vec::new(),
            pagination: None,
            count_only: false,
        },
    );
    assert!(
        mssql
            .sql
            .contains("([CardNo] = @P1 AND [ContNo] = @P2) OR ([CardNo] = @P3 AND [ContNo] = @P4)")
    );
}
