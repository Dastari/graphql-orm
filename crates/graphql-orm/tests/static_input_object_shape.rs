#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

//! A generated static schema refuses a value that is not an object where an
//! input object is expected, before any resolver, policy or database work, with
//! the `invalid_graphql_input` code the runtime GraphQL path uses.
//!
//! async-graphql 7.2.1 accepts any value for an input-object argument during
//! validation. Without the generated schema's own check, such a value failed
//! later, while its field resolved, with an uncoded coercion message, after
//! earlier fields of the same operation had already run.

use graphql_orm::async_graphql::{Request, Response, Value, Variables};
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::prelude::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "shape_items",
    plural = "ShapeItems",
    auth = "none",
    default_sort = "id ASC"
)]
pub struct ShapeItem {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,
    #[filterable(type = "string")]
    #[sortable]
    pub name: String,
}

schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [ShapeItem],
}

/// Counts every entity-policy decision: generated resolvers ask one before
/// any SQL, so a zero count means no resolver work started.
#[derive(Clone, Default)]
struct CountDecisions(Arc<AtomicUsize>);
impl EntityPolicy for CountDecisions {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(true) })
    }
}

struct CountReads(Arc<AtomicUsize>);
impl ReadQueryObserver for CountReads {
    fn on_read(&self, _: &str, _: usize) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct Fixture {
    db: Database,
    decisions: Arc<AtomicUsize>,
    reads: Arc<AtomicUsize>,
}

async fn fixture() -> Fixture {
    let mut db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    for statement in [
        "CREATE TABLE shape_items (id TEXT PRIMARY KEY, name TEXT NOT NULL)",
        "INSERT INTO shape_items (id, name) VALUES ('a', 'alpha'), ('b', 'beta')",
    ] {
        graphql_orm::sqlx::query(statement)
            .execute(db.pool())
            .await
            .unwrap();
    }
    let decisions = CountDecisions::default();
    db.set_entity_policy(decisions.clone());
    let reads = Arc::new(AtomicUsize::new(0));
    let db = db.with_read_query_observer(CountReads(reads.clone()));
    Fixture {
        db,
        decisions: decisions.0,
        reads,
    }
}

async fn execute(fixture: &Fixture, query: &str, variables: serde_json::Value) -> Response {
    let schema = schema_builder(fixture.db.clone()).finish();
    schema
        .execute(
            Request::new(query)
                .variables(Variables::from_json(variables))
                .data(AuthSubject::new("fixture-principal")),
        )
        .await
}

async fn names(db: &Database) -> Vec<String> {
    graphql_orm::sqlx::query_scalar("SELECT name FROM shape_items ORDER BY id")
        .fetch_all(db.pool())
        .await
        .unwrap()
}

fn assert_refused(response: &Response, probe: &str) {
    assert_eq!(response.data, Value::Null, "{probe}: {response:?}");
    assert_eq!(response.errors.len(), 1, "{probe}: {:?}", response.errors);
    let error = &response.errors[0];
    assert_eq!(error.message, "invalid_graphql_input", "{probe}: {error:?}");
    assert_eq!(
        error.extensions.as_ref().and_then(|e| e.get("code")),
        Some(&Value::from("invalid_graphql_input")),
        "{probe}: {error:?}"
    );
}

const LIST: &str = "{ shapeItems(where: WHERE) { edges { node { id } } } }";

#[tokio::test]
async fn literal_non_object_values_for_input_objects_are_refused_before_any_resolver() {
    for value in [
        "5",
        "\"alpha\"",
        "true",
        "ALPHA",
        "[{name: {eq: \"alpha\"}}]",
        "[5]",
        "{name: 5}",
        "{name: \"alpha\"}",
        "{name: [{eq: \"alpha\"}]}",
        "{and: [5]}",
        "{and: 5}",
        "{or: [{name: \"alpha\"}]}",
        "{not: \"alpha\"}",
        "{not: [{name: {eq: \"alpha\"}}]}",
    ] {
        let fixture = fixture().await;
        let probe = LIST.replace("WHERE", value);
        let response = execute(&fixture, &probe, serde_json::json!({})).await;
        assert_refused(&response, &probe);
        assert_eq!(fixture.decisions.load(Ordering::SeqCst), 0, "{probe}");
        assert_eq!(fixture.reads.load(Ordering::SeqCst), 0, "{probe}");
    }
}

#[tokio::test]
async fn variable_non_object_values_for_input_objects_are_refused_before_any_resolver() {
    let declared =
        "query($w: ShapeItemWhereInput) { shapeItems(where: $w) { edges { node { id } } } }";
    let defaulted = "query($w: ShapeItemWhereInput = {name: {eq: \"alpha\"}}) { shapeItems(where: $w) { edges { node { id } } } }";
    let nested =
        "query($n: StringFilter) { shapeItems(where: {name: $n}) { edges { node { id } } } }";
    for (query, variables) in [
        (declared, serde_json::json!({ "w": 5 })),
        (declared, serde_json::json!({ "w": "alpha" })),
        (
            declared,
            serde_json::json!({ "w": [{ "name": { "eq": "alpha" } }] }),
        ),
        (declared, serde_json::json!({ "w": { "name": 5 } })),
        (declared, serde_json::json!({ "w": { "and": [5] } })),
        (declared, serde_json::json!({ "w": { "not": "alpha" } })),
        (defaulted, serde_json::json!({ "w": 5 })),
        (defaulted, serde_json::json!({ "w": { "name": "alpha" } })),
        (nested, serde_json::json!({ "n": "alpha" })),
        (nested, serde_json::json!({ "n": [{ "eq": "alpha" }] })),
        // An omitted variable whose default is not an object.
        (
            "query($w: ShapeItemWhereInput = 5) { shapeItems(where: $w) { edges { node { id } } } }",
            serde_json::json!({}),
        ),
        (
            "query($n: StringFilter = \"alpha\") { shapeItems(where: {name: $n}) { edges { node { id } } } }",
            serde_json::json!({}),
        ),
    ] {
        let fixture = fixture().await;
        let response = execute(&fixture, query, variables.clone()).await;
        let probe = format!("{query} {variables}");
        assert_refused(&response, &probe);
        assert_eq!(fixture.decisions.load(Ordering::SeqCst), 0, "{probe}");
        assert_eq!(fixture.reads.load(Ordering::SeqCst), 0, "{probe}");
    }
}

#[tokio::test]
async fn fragments_and_the_selected_operation_are_checked() {
    let fixture = fixture().await;
    let root = execute(&fixture, "{ __typename }", serde_json::json!({}))
        .await
        .data
        .into_json()
        .unwrap()["__typename"]
        .as_str()
        .unwrap()
        .to_owned();
    for (query, operation) in [
        (
            format!(
                "{{ ...F }} fragment F on {root} {{ shapeItems(where: 5) {{ edges {{ node {{ id }} }} }} }}"
            ),
            None,
        ),
        (
            format!(
                "{{ ...Outer }} fragment Outer on {root} {{ ...Inner }} fragment Inner on {root} {{ shapeItems(where: [5]) {{ edges {{ node {{ id }} }} }} }}"
            ),
            None,
        ),
        (
            "{ ... { shapeItems(where: {not: 5}) { edges { node { id } } } } }".to_owned(),
            None,
        ),
        (
            format!(
                "{{ ... on {root} {{ shapeItems(where: \"alpha\") {{ edges {{ node {{ id }} }} }} }} }}"
            ),
            None,
        ),
        (
            "query Good { shapeItems { edges { node { id } } } } query Bad { shapeItems(where: 5) { edges { node { id } } } }".to_owned(),
            Some("Bad"),
        ),
    ] {
        let schema = schema_builder(fixture.db.clone()).finish();
        let mut request = Request::new(query.as_str()).data(AuthSubject::new("fixture-principal"));
        if let Some(operation) = operation {
            request = request.operation_name(operation);
        }
        let response = schema.execute(request).await;
        assert_refused(&response, &query);
    }
    assert_eq!(fixture.decisions.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.reads.load(Ordering::SeqCst), 0);

    // Only the selected operation is checked.
    let schema = schema_builder(fixture.db.clone()).finish();
    let response = schema
        .execute(
            Request::new(
                "query Good { shapeItems { edges { node { id } } } } query Bad { shapeItems(where: 5) { edges { node { id } } } }",
            )
            .operation_name("Good")
            .data(AuthSubject::new("fixture-principal")),
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
}

#[tokio::test]
async fn a_refused_argument_stops_the_whole_operation_before_earlier_fields_write() {
    let probes = [
        // A literal non-object for the bulk update's input object, after a
        // valid write in the same operation.
        "mutation { first: updateShapeItems(where: {id: {eq: \"a\"}}, input: {name: \"changed\"}) { affectedCount } second: updateShapeItems(where: {id: {eq: \"b\"}}, input: 5) { affectedCount } }",
        // A non-object filter on a later bulk delete.
        "mutation { first: updateShapeItems(where: {id: {eq: \"a\"}}, input: {name: \"changed\"}) { affectedCount } second: deleteShapeItems(where: \"b\") { deletedCount } }",
        // A non-object create input.
        "mutation { first: updateShapeItems(where: {id: {eq: \"a\"}}, input: {name: \"changed\"}) { affectedCount } second: createShapeItem(input: [{id: \"c\", name: \"gamma\"}]) { success } }",
    ];
    for probe in probes {
        let fixture = fixture().await;
        let response = execute(&fixture, probe, serde_json::json!({})).await;
        assert_refused(&response, probe);
        assert_eq!(fixture.decisions.load(Ordering::SeqCst), 0, "{probe}");
        assert_eq!(names(&fixture.db).await, ["alpha", "beta"], "{probe}");
    }
    // The same operation through a variable.
    let fixture = fixture().await;
    let response = execute(
        &fixture,
        "mutation($i: UpdateShapeItemInput!) { first: updateShapeItems(where: {id: {eq: \"a\"}}, input: {name: \"changed\"}) { affectedCount } second: updateShapeItems(where: {id: {eq: \"b\"}}, input: $i) { affectedCount } }",
        serde_json::json!({ "i": "changed" }),
    )
    .await;
    assert_refused(&response, "variable update input");
    assert_eq!(names(&fixture.db).await, ["alpha", "beta"]);
}

#[tokio::test]
async fn object_values_null_and_omitted_defaults_still_execute() {
    let fixture = fixture().await;
    for (query, variables, expected) in [
        (
            "{ shapeItems(where: {name: {eq: \"alpha\"}}) { edges { node { id } } } }",
            serde_json::json!({}),
            1,
        ),
        (
            "{ shapeItems(where: null) { edges { node { id } } } }",
            serde_json::json!({}),
            2,
        ),
        (
            "{ shapeItems(where: {and: {name: {eq: \"beta\"}}}) { edges { node { id } } } }",
            serde_json::json!({}),
            1,
        ),
        (
            "query($w: ShapeItemWhereInput) { shapeItems(where: $w) { edges { node { id } } } }",
            serde_json::json!({ "w": { "or": [{ "name": { "eq": "alpha" } }, { "id": { "eq": "b" } }] } }),
            2,
        ),
        (
            "query($w: ShapeItemWhereInput) { shapeItems(where: $w) { edges { node { id } } } }",
            serde_json::json!({ "w": null }),
            2,
        ),
        (
            "query($w: ShapeItemWhereInput) { shapeItems(where: $w) { edges { node { id } } } }",
            serde_json::json!({}),
            2,
        ),
        (
            "query($w: ShapeItemWhereInput = {name: {eq: \"beta\"}}) { shapeItems(where: $w) { edges { node { id } } } }",
            serde_json::json!({}),
            1,
        ),
        (
            "query($n: StringFilter) { shapeItems(where: {name: $n}) { edges { node { id } } } }",
            serde_json::json!({ "n": { "eq": "alpha" } }),
            1,
        ),
    ] {
        let response = execute(&fixture, query, variables.clone()).await;
        assert!(
            response.errors.is_empty(),
            "{query} {variables}: {:?}",
            response.errors
        );
        let data = response.data.into_json().unwrap();
        assert_eq!(
            data["shapeItems"]["edges"].as_array().unwrap().len(),
            expected,
            "{query} {variables}: {data}"
        );
    }

    let response = execute(
        &fixture,
        "mutation($i: UpdateShapeItemInput!) { updateShapeItems(where: {id: {eq: \"a\"}}, input: $i) { affectedCount } }",
        serde_json::json!({ "i": { "name": "changed" } }),
    )
    .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(names(&fixture.db).await, ["changed", "beta"]);
}

#[tokio::test]
async fn a_null_for_a_required_input_object_is_still_refused_by_validation() {
    let fixture = fixture().await;
    for (query, variables) in [
        (
            "mutation { updateShapeItems(where: {id: {eq: \"a\"}}, input: null) { affectedCount } }",
            serde_json::json!({}),
        ),
        (
            "mutation($i: UpdateShapeItemInput!) { updateShapeItems(where: {id: {eq: \"a\"}}, input: $i) { affectedCount } }",
            serde_json::json!({ "i": null }),
        ),
    ] {
        let response = execute(&fixture, query, variables).await;
        assert!(!response.errors.is_empty(), "{query}: {response:?}");
        assert_eq!(response.data, Value::Null, "{query}");
    }
    assert_eq!(fixture.decisions.load(Ordering::SeqCst), 0);
    assert_eq!(names(&fixture.db).await, ["alpha", "beta"]);
}
