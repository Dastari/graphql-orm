#![cfg(feature = "sqlite")]

use std::sync::Arc;
use std::time::Duration;

use graphql_orm::async_graphql::{Error, ErrorExtensions, Request, Response, SimpleObject};
use graphql_orm::futures::StreamExt;
use graphql_orm::graphql::assurance::{
    AssuranceActorClass, AssuranceCheck, AssuranceEnforcement, AssuranceRequirementEvaluator,
    AssuranceSchemaConfig, OperationAssuranceRegistry,
};
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "assurance_entries",
    plural = "AssuranceEntries",
    auth = "none",
    aggregate = true,
    keyset = "id asc",
    federation_key
)]
#[graphql_orm(search(index = true, tokenizer = "unicode61"))]
pub struct AssuranceEntry {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    pub id: String,
    pub folder_id: String,
    #[filterable(type = "string")]
    #[graphql_orm(searchable(weight = "A"))]
    pub label: String,
}

#[derive(
    GraphQLEntity,
    GraphQLOperations,
    GraphQLRelations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_entity(
    backend = "sqlite",
    table = "assurance_folders",
    plural = "AssuranceFolders",
    auth = "none",
    changed_started = true
)]
#[graphql_orm(search(index = true, tokenizer = "unicode61"))]
pub struct AssuranceFolder {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    pub id: String,
    pub entry_id: String,
    #[graphql_orm(searchable(weight = "A"))]
    pub title: String,
    #[graphql(skip)]
    #[relation(
        target = "AssuranceEntry",
        from = "entry_id",
        to = "id",
        emit_fk = false
    )]
    pub entry: Option<AssuranceEntry>,
    #[graphql(skip)]
    #[graphql_orm(search_relation(fields = ["label"]))]
    #[relation(
        target = "AssuranceEntry",
        from = "id",
        to = "folder_id",
        multiple,
        filter,
        emit_fk = false,
        order_aggregate(name = "EntryCount", aggregate = "count")
    )]
    pub entries: Vec<AssuranceEntry>,
}

#[derive(
    GraphQLEntity,
    GraphQLOperations,
    GraphQLRelations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_entity(
    backend = "sqlite",
    table = "assurance_shelves",
    plural = "AssuranceShelves",
    auth = "none"
)]
pub struct AssuranceShelf {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    pub id: String,
    pub folder_id: String,
    #[graphql(skip)]
    #[relation(
        target = "AssuranceFolder",
        from = "folder_id",
        to = "id",
        emit_fk = false
    )]
    pub folder: Option<AssuranceFolder>,
}

schema_roots! {
    backend: "sqlite",
    entities: [AssuranceEntry, AssuranceFolder, AssuranceShelf],
}

#[derive(Debug)]
struct StepUpEvaluator;

#[derive(Clone, Copy)]
struct SteppedUp;

impl AssuranceRequirementEvaluator for StepUpEvaluator {
    fn enforce(
        &self,
        check: &AssuranceCheck<'_>,
        _: AssuranceActorClass,
        policy_id: &str,
    ) -> graphql_orm::async_graphql::Result<()> {
        assert_eq!(policy_id, "entries.step-up");
        if check.data_opt::<SteppedUp>().is_some() {
            return Ok(());
        }
        Err(Error::new("additional authentication is required")
            .extend_with(|_, ext| ext.set("code", "STEP_UP_REQUIRED")))
    }
}

async fn database() -> Database<SqliteBackend> {
    let db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    let model = SchemaModel::from_entities(&[
        AssuranceEntry::metadata(),
        AssuranceFolder::metadata(),
        AssuranceShelf::metadata(),
    ]);
    let plan = build_migration_plan(
        DatabaseBackend::Sqlite,
        &SchemaModel {
            extensions: vec![],
            tables: vec![],
        },
        &model,
    );
    for sql in plan.statements.iter().map(String::as_str).chain([
        "INSERT INTO assurance_folders (id, entry_id, title) VALUES ('folder', 'entry', 'folder title'), ('empty', 'missing', 'empty')",
        "INSERT INTO assurance_entries (id, folder_id, label) VALUES ('entry', 'folder', 'needleword')",
        "INSERT INTO assurance_shelves (id, folder_id) VALUES ('shelf', 'folder')",
    ]) {
        sqlx::query(sql).execute(db.pool()).await.unwrap();
    }
    AssuranceFolder::rebuild_search_index(&db).await.unwrap();
    db.with_authorized_scan_config(AuthorizedScanConfig::new([11; 32], 7, 64))
}

fn assert_error_code(response: &Response, operation: &str, code: &str) {
    assert_eq!(response.errors.len(), 1, "{operation}: {response:?}");
    assert_eq!(
        response.errors[0]
            .extensions
            .as_ref()
            .and_then(|ext| ext.get("code")),
        Some(&graphql_orm::async_graphql::Value::from(code)),
        "{operation}: {response:?}",
    );
}

const QUERIES: &[(&str, &str)] = &[
    (
        r#"{ assuranceFolder(id: "folder") { entry { label } } }"#,
        "needleword",
    ),
    (
        r#"{ assuranceFolder(id: "folder") { entries { edges { node { label } } } } }"#,
        "needleword",
    ),
    (
        r#"{ assuranceShelf(id: "shelf") { folder { entry { label } } } }"#,
        "needleword",
    ),
    (
        r#"{ assuranceFolders(where: {entries: [{label: {eq: "needleword"}}]}) { edges { node { id } } } }"#,
        "folder",
    ),
    (
        r#"{ assuranceFolders(orderBy: [{EntryCount: DESC}], page: {limit: 1}) { edges { node { id } } } }"#,
        "folder",
    ),
    (
        r#"{ assuranceFoldersSearch(search: {query: "title"}) { edges { node { id } } } }"#,
        "folder",
    ),
];

const ENTITIES: &str = r#"{ _entities(representations: [{__typename: "AssuranceEntry", id: "entry"}]) { ... on AssuranceEntry { label } } }"#;

fn require(root: &str) -> AssuranceEnforcement {
    let mut builder = OperationAssuranceRegistry::builder(
        graphql_orm_operation_catalog(),
        AssuranceSchemaConfig::legacy(),
    );
    builder
        .require(GraphqlOperationKind::Query, root, "entries.step-up")
        .unwrap();
    AssuranceEnforcement::new(
        Arc::new(builder.build().unwrap()),
        Arc::new(StepUpEvaluator),
    )
}

fn assert_read(response: Response, operation: &str, expected: &str) {
    assert!(response.errors.is_empty(), "{operation}: {response:?}");
    let data = response.data.into_json().unwrap();
    assert!(
        data.to_string().contains(&format!("\"{expected}\"")),
        "{operation}: {data}"
    );
}

#[tokio::test]
async fn each_target_query_root_protects_relation_reads() {
    for root in [
        "assuranceEntries",
        "assuranceEntry",
        "assuranceEntriesKeyset",
        "assuranceEntriesScan",
        "assuranceEntriesSearch",
        "assuranceEntriesAggregate",
    ] {
        let schema = schema_builder(database().await)
            .enable_federation()
            .data(require(root))
            .finish();
        for &(query, expected) in QUERIES {
            let response = schema.execute(query).await;
            assert_error_code(&response, &format!("{root}: {query}"), "STEP_UP_REQUIRED");
            assert_read(
                schema.execute(Request::new(query).data(SteppedUp)).await,
                query,
                expected,
            );
        }

        let response = schema.execute(ENTITIES).await;
        if root == "assuranceEntry" {
            assert_error_code(&response, ENTITIES, "STEP_UP_REQUIRED");
        } else {
            assert_read(response, ENTITIES, "needleword");
        }
        assert_read(
            schema.execute(Request::new(ENTITIES).data(SteppedUp)).await,
            ENTITIES,
            "needleword",
        );
    }
}

#[tokio::test]
async fn refused_traversals_do_not_read_the_target_table() {
    let db = database().await;
    sqlx::query("DROP TABLE assurance_entries")
        .execute(db.pool())
        .await
        .unwrap();
    let schema = schema_builder(db).data(require("assuranceEntry")).finish();
    for &(query, _) in QUERIES {
        let response = schema.execute(query).await;
        assert_error_code(&response, query, "STEP_UP_REQUIRED");
    }
}

#[tokio::test]
async fn relation_reads_work_without_query_assurance() {
    let schema = schema_builder(database().await)
        .data(AssuranceEnforcement::new(
            Arc::new(
                OperationAssuranceRegistry::builder(
                    graphql_orm_operation_catalog(),
                    AssuranceSchemaConfig::legacy(),
                )
                .build()
                .unwrap(),
            ),
            Arc::new(StepUpEvaluator),
        ))
        .finish();
    for &(query, expected) in QUERIES {
        assert_read(schema.execute(query).await, query, expected);
    }
}

#[tokio::test]
async fn target_query_assurance_guards_mutation_results() {
    for root in ["assuranceEntries", "assuranceEntry"] {
        let schema = schema_builder(database().await)
            .data(require(root))
            .finish();
        let create = |id: &str| {
            format!(
                r#"mutation {{ createAssuranceFolder(input: {{id: "{id}", entryId: "entry", title: "new folder"}}) {{
                assuranceFolder {{ entry {{ label }} }}
            }} }}"#
            )
        };
        let denied = create("denied");
        let result = schema.execute(&denied).await;
        assert_error_code(&result, &denied, "STEP_UP_REQUIRED");

        let admitted = create("admitted");
        let result = schema
            .execute(Request::new(&admitted).data(SteppedUp))
            .await;
        assert_read(result, &admitted, "needleword");
    }
}

#[tokio::test]
async fn changed_payloads_refuse_relation_selections_before_registration() {
    const CHANGED: &str = "subscription { assuranceFolderChanged(started: true) { started assuranceFolder { entry { label } } } }";
    let schema = schema_builder(database().await)
        .data(require("assuranceEntry"))
        .finish();
    let mut changes = Box::pin(schema.execute_stream(Request::new(CHANGED).data(SteppedUp)));
    let response = tokio::time::timeout(Duration::from_secs(2), changes.next())
        .await
        .unwrap()
        .unwrap();
    assert_error_code(&response, CHANGED, "INVALID_INPUT");
}
