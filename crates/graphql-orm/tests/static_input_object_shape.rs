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

/// The same entity in a schema whose maximum page limit is declared at
/// compile time with a value other than the default 100.
mod declared_maximum {
    use super::*;
    schema_roots! {
        backend: "sqlite",
        max_page_limit: 250,
        query_custom_ops: [],
        entities: [ShapeItem],
    }
}

/// A schema built through the breaking batch's compile-time maximum path
/// (`schema_roots!(max_page_limit: N)`) installs the same input check.
#[tokio::test]
async fn a_declared_maximum_schema_refuses_non_object_values_before_any_resolver() {
    let fixture = fixture().await;
    let db = fixture
        .db
        .clone()
        .with_pagination_config(fixture.db.pagination_config().with_max_limit(Some(250)));
    let schema = declared_maximum::schema_builder(db).finish();
    for (query, variables) in [
        (LIST.replace("WHERE", "5"), serde_json::json!({})),
        (
            "query($w: ShapeItemWhereInput) { shapeItems(where: $w) { edges { node { id } } } }"
                .to_string(),
            serde_json::json!({ "w": "alpha" }),
        ),
    ] {
        let response = schema
            .execute(
                Request::new(query.as_str())
                    .variables(Variables::from_json(variables))
                    .data(AuthSubject::new("fixture-principal")),
            )
            .await;
        assert_refused(&response, &query);
    }
    assert_eq!(fixture.decisions.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.reads.load(Ordering::SeqCst), 0);
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

async fn execute_in(
    fixture: &Fixture,
    mode: async_graphql::ValidationMode,
    query: &str,
    variables: serde_json::Value,
) -> Response {
    let schema = schema_builder(fixture.db.clone())
        .validation_mode(mode)
        .finish();
    schema
        .execute(
            Request::new(query)
                .variables(Variables::from_json(variables))
                .data(AuthSubject::new("fixture-principal")),
        )
        .await
}

/// async-graphql 7.2.1 declares the rule that a variable's declared type must
/// fit the position it is used in, but never applies it, in either validation
/// mode, so a variable reference is checked against its position here.
#[tokio::test]
async fn a_variable_whose_type_does_not_fit_its_position_is_refused_in_every_mode() {
    const FIRST: &str = "first: updateShapeItems(where: {id: {eq: \"a\"}}, input: {name: \"changed\"}) { affectedCount }";
    for (query, variables) in [
        // A scalar variable where an input object is expected.
        (
            format!(
                "mutation($w: String) {{ {FIRST} second: deleteShapeItems(where: $w) {{ deletedCount }} }}"
            ),
            serde_json::json!({ "w": "b" }),
        ),
        // A scalar variable of another type nested in an input object.
        (
            format!(
                "mutation($n: Int) {{ {FIRST} second: deleteShapeItems(where: {{name: $n}}) {{ deletedCount }} }}"
            ),
            serde_json::json!({ "n": 5 }),
        ),
        // A variable of another input-object type.
        (
            format!(
                "mutation($n: StringFilter) {{ {FIRST} second: deleteShapeItems(where: $n) {{ deletedCount }} }}"
            ),
            serde_json::json!({ "n": { "eq": "b" } }),
        ),
        // A list variable where a single input object is expected.
        (
            format!(
                "mutation($w: [ShapeItemWhereInput]) {{ {FIRST} second: deleteShapeItems(where: $w) {{ deletedCount }} }}"
            ),
            serde_json::json!({ "w": [{ "id": { "eq": "b" } }] }),
        ),
        // A nullable variable without a default where a value is required.
        (
            format!(
                "mutation($i: UpdateShapeItemInput) {{ {FIRST} second: updateShapeItems(where: {{id: {{eq: \"b\"}}}}, input: $i) {{ affectedCount }} }}"
            ),
            serde_json::json!({ "i": { "name": "changed" } }),
        ),
        // A nullable single value where the list's items must be non-null.
        (
            format!(
                "mutation($n: String) {{ {FIRST} second: deleteShapeItems(where: {{name: {{inList: $n}}}}) {{ deletedCount }} }}"
            ),
            serde_json::json!({ "n": "beta" }),
        ),
        // A list variable of nullable items where the items must be non-null.
        (
            format!(
                "mutation($n: [String]) {{ {FIRST} second: deleteShapeItems(where: {{name: {{inList: $n}}}}) {{ deletedCount }} }}"
            ),
            serde_json::json!({ "n": ["beta"] }),
        ),
    ] {
        for mode in [
            async_graphql::ValidationMode::Fast,
            async_graphql::ValidationMode::Strict,
        ] {
            let fixture = fixture().await;
            let response = execute_in(&fixture, mode, &query, variables.clone()).await;
            let probe = format!("{mode:?} {query} {variables}");
            // Strict validation catches some of these itself, with its own
            // message; either way nothing runs.
            assert_eq!(response.data, Value::Null, "{probe}: {response:?}");
            assert!(!response.errors.is_empty(), "{probe}");
            if matches!(mode, async_graphql::ValidationMode::Fast) {
                assert_refused(&response, &probe);
            }
            assert_eq!(fixture.decisions.load(Ordering::SeqCst), 0, "{probe}");
            assert_eq!(names(&fixture.db).await, ["alpha", "beta"], "{probe}");
        }
    }
}

#[tokio::test]
async fn variables_in_allowed_positions_still_execute_in_every_mode() {
    for (query, variables, expected) in [
        (
            "mutation($w: ShapeItemWhereInput) { deleteShapeItems(where: $w) { deletedCount } }",
            serde_json::json!({ "w": { "id": { "eq": "b" } } }),
            ["alpha"].as_slice(),
        ),
        (
            "mutation($n: StringFilter!) { deleteShapeItems(where: {name: $n}) { deletedCount } }",
            serde_json::json!({ "n": { "eq": "beta" } }),
            ["alpha"].as_slice(),
        ),
        // A nullable variable with a default may stand where a value is required.
        (
            "mutation($i: UpdateShapeItemInput = {name: \"changed\"}) { updateShapeItems(where: {id: {eq: \"a\"}}, input: $i) { affectedCount } }",
            serde_json::json!({}),
            ["changed", "beta"].as_slice(),
        ),
        (
            "mutation($i: UpdateShapeItemInput!) { updateShapeItems(where: {id: {eq: \"a\"}}, input: $i) { affectedCount } }",
            serde_json::json!({ "i": { "name": "changed" } }),
            ["changed", "beta"].as_slice(),
        ),
        // A non-null list variable where a nullable list is expected.
        (
            "mutation($n: [String!]!) { deleteShapeItems(where: {name: {inList: $n}}) { deletedCount } }",
            serde_json::json!({ "n": ["beta"] }),
            ["alpha"].as_slice(),
        ),
        // A nullable list variable with a default where a nullable list is expected.
        (
            "mutation($n: [String!] = [\"beta\"]) { deleteShapeItems(where: {name: {inList: $n}}) { deletedCount } }",
            serde_json::json!({}),
            ["alpha"].as_slice(),
        ),
        // A non-null single value where a list is expected: execution
        // coerces it to a one-element list.
        (
            "mutation($n: String!) { deleteShapeItems(where: {name: {inList: $n}}) { deletedCount } }",
            serde_json::json!({ "n": "beta" }),
            ["alpha"].as_slice(),
        ),
        // A non-null item variable inside a list literal.
        (
            "mutation($n: String!) { deleteShapeItems(where: {name: {inList: [$n]}}) { deletedCount } }",
            serde_json::json!({ "n": "beta" }),
            ["alpha"].as_slice(),
        ),
    ] {
        for mode in [
            async_graphql::ValidationMode::Fast,
            async_graphql::ValidationMode::Strict,
        ] {
            let fixture = fixture().await;
            let response = execute_in(&fixture, mode, query, variables.clone()).await;
            assert!(
                response.errors.is_empty(),
                "{mode:?} {query} {variables}: {:?}",
                response.errors
            );
            assert_eq!(names(&fixture.db).await, expected, "{mode:?} {query}");
        }
    }
}

/// A request may arrive already parsed, without query text: the check's work
/// bound is sized from the document, so a large valid document still runs.
#[tokio::test]
async fn a_large_pre_parsed_document_is_not_refused() {
    let fixture = fixture().await;
    let fields = (0..400)
        .map(|index| {
            format!(
                "a{index}: shapeItems(where: {{name: {{eq: \"alpha\"}}, and: [{{id: {{eq: \"a\"}}}}]}}) {{ edges {{ node {{ id }} }} }}"
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    let document =
        graphql_orm::async_graphql::parser::parse_query(format!("{{ {fields} }}")).unwrap();
    let schema = schema_builder(fixture.db.clone())
        .limit_complexity(usize::MAX)
        .finish();
    let mut request = Request::new("").data(AuthSubject::new("fixture-principal"));
    request.set_parsed_query(document);
    let response = schema.execute(request).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data.as_object().unwrap().len(), 400);
    assert_eq!(data["a399"]["edges"][0]["node"]["id"], "a");

    // The same document with one non-object value is still refused.
    let document = graphql_orm::async_graphql::parser::parse_query(format!(
        "{{ {fields} bad: shapeItems(where: 5) {{ edges {{ node {{ id }} }} }} }}"
    ))
    .unwrap();
    let mut request = Request::new("").data(AuthSubject::new("fixture-principal"));
    request.set_parsed_query(document);
    let response = schema.execute(request).await;
    assert_refused(&response, "pre-parsed document");
}

/// A hand-built schema with a oneof input, for the oneof variable rule.
mod oneof {
    use graphql_orm::async_graphql::{self, EmptySubscription, Object, OneofObject, Schema};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[derive(OneofObject)]
    pub enum Choice {
        A(i32),
        B(String),
    }

    pub struct Query;
    #[Object]
    impl Query {
        async fn ok(&self) -> bool {
            true
        }
    }

    pub struct Mutation(pub Arc<AtomicUsize>);
    #[Object]
    impl Mutation {
        async fn bump(&self) -> usize {
            self.0.fetch_add(1, Ordering::SeqCst) + 1
        }
        async fn take(&self, arg: Choice) -> bool {
            matches!(arg, Choice::A(_) | Choice::B(_))
        }
    }

    pub fn schema(
        mode: async_graphql::ValidationMode,
    ) -> (Schema<Query, Mutation, EmptySubscription>, Arc<AtomicUsize>) {
        let bumps = Arc::new(AtomicUsize::new(0));
        let schema = Schema::build(Query, Mutation(bumps.clone()), EmptySubscription)
            .extension(graphql_orm::graphql::input_validation::GeneratedInputValidation)
            .validation_mode(mode)
            .finish();
        (schema, bumps)
    }
}

/// A variable standing for a oneof member must be declared non-null and
/// resolve to a value: a nullable one may otherwise be null or omitted, which
/// the oneof rule forbids.
#[tokio::test]
async fn a_nullable_or_null_variable_in_a_oneof_field_is_refused_in_every_mode() {
    const NULLABLE: &str = "mutation($v: Int) { first: bump second: take(arg: {a: $v}) }";
    const REQUIRED: &str = "mutation($v: Int!) { first: bump second: take(arg: {a: $v}) }";
    for mode in [
        async_graphql::ValidationMode::Fast,
        async_graphql::ValidationMode::Strict,
    ] {
        for (query, variables) in [
            (NULLABLE, serde_json::json!({ "v": null })),
            (NULLABLE, serde_json::json!({})),
            (NULLABLE, serde_json::json!({ "v": 5 })),
            (REQUIRED, serde_json::json!({})),
        ] {
            let (schema, bumps) = oneof::schema(mode);
            let response = schema
                .execute(Request::new(query).variables(Variables::from_json(variables.clone())))
                .await;
            let probe = format!("{mode:?} {query} {variables}");
            assert_eq!(response.data, Value::Null, "{probe}: {response:?}");
            assert!(!response.errors.is_empty(), "{probe}");
            // Strict validation catches an explicit `null` itself, with its
            // own message; either way nothing runs.
            if matches!(mode, async_graphql::ValidationMode::Fast) {
                assert_refused(&response, &probe);
            }
            assert_eq!(bumps.load(Ordering::SeqCst), 0, "{probe}: bump ran");
        }

        // A non-null variable with a value, and a literal member, still run.
        for (query, variables) in [
            (REQUIRED, serde_json::json!({ "v": 5 })),
            (
                "mutation { first: bump second: take(arg: {b: \"x\"}) }",
                serde_json::json!({}),
            ),
        ] {
            let (schema, bumps) = oneof::schema(mode);
            let response = schema
                .execute(Request::new(query).variables(Variables::from_json(variables.clone())))
                .await;
            assert!(
                response.errors.is_empty(),
                "{mode:?} {query} {variables}: {:?}",
                response.errors
            );
            assert_eq!(bumps.load(Ordering::SeqCst), 1, "{mode:?} {query}");
        }
    }
}

/// A schema built by hand from generated roots without
/// `GeneratedInputValidation` fails closed (#753): every generated resolver
/// that takes an input object refuses with `INPUT_VALIDATION_REQUIRED` before
/// policy or database work, even for well-formed values, so the missing
/// extension shows on the first such request. Installing it is the one-line
/// fix.
#[tokio::test]
async fn hand_built_schemas_without_input_validation_fail_closed() {
    use graphql_orm::async_graphql::{EmptySubscription, Schema};
    let fixture = fixture().await;
    let hand_built = |validated: bool| {
        let builder = Schema::build(
            QueryRoot::default(),
            MutationRoot::default(),
            EmptySubscription,
        )
        .data(fixture.db.clone());
        if validated {
            builder
                .extension(graphql_orm::graphql::input_validation::GeneratedInputValidation)
                .finish()
        } else {
            builder.finish()
        }
    };
    let schema = hand_built(false);
    for (query, variables) in [
        (
            "{ shapeItems(where: { name: { eq: \"alpha\" } }) { edges { node { id } } } }",
            serde_json::json!({}),
        ),
        (
            "query($w: ShapeItemWhereInput) { shapeItems(where: $w) { edges { node { id } } } }",
            serde_json::json!({ "w": { "name": { "eq": "alpha" } } }),
        ),
        (
            "{ shapeItems { edges { node { id } } } }",
            serde_json::json!({}),
        ),
        (
            "mutation { createShapeItem(input: { id: \"c\", name: \"gamma\" }) { success } }",
            serde_json::json!({}),
        ),
        (
            "mutation { deleteShapeItems(where: { id: { eq: \"a\" } }) { deletedCount } }",
            serde_json::json!({}),
        ),
        (
            "mutation { first: updateShapeItems(where: { id: { eq: \"a\" } }, input: { name: \"changed\" }) { affectedCount } second: createShapeItem(input: { id: \"c\", name: \"gamma\" }) { success } }",
            serde_json::json!({}),
        ),
    ] {
        let response = schema
            .execute(
                Request::new(query)
                    .variables(Variables::from_json(variables.clone()))
                    .data(AuthSubject::new("fixture-principal")),
            )
            .await;
        let probe = format!("{query} {variables}");
        assert!(!response.errors.is_empty(), "{probe}: {response:?}");
        let codes = response
            .errors
            .iter()
            .map(|error| {
                error
                    .extensions
                    .as_ref()
                    .and_then(|extensions| extensions.get("code"))
                    .cloned()
            })
            .collect::<Vec<_>>();
        assert!(
            codes
                .iter()
                .all(|code| code == &Some(Value::String("INPUT_VALIDATION_REQUIRED".into()))),
            "{probe}: {:?}",
            response.errors
        );
    }
    assert_eq!(fixture.decisions.load(Ordering::SeqCst), 0, "a policy ran");
    assert_eq!(fixture.reads.load(Ordering::SeqCst), 0, "a read ran");
    assert_eq!(names(&fixture.db).await, ["alpha", "beta"]);

    // A scalar-only generated resolver needs no mark.
    let response = schema
        .execute(
            Request::new("{ shapeItem(id: \"a\") { name } }")
                .data(AuthSubject::new("fixture-principal")),
        )
        .await;
    assert!(response.errors.is_empty(), "{response:?}");

    // With the extension the same hand-built schema serves the request.
    let response = hand_built(true)
        .execute(
            Request::new(
                "{ shapeItems(where: { name: { eq: \"alpha\" } }) { edges { node { id } } } }",
            )
            .data(AuthSubject::new("fixture-principal")),
        )
        .await;
    assert!(response.errors.is_empty(), "{response:?}");
}
