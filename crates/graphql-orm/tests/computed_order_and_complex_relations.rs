// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(feature = "sqlite")]

use graphql_orm::async_graphql::SimpleObject;
use graphql_orm::prelude::*;
use graphql_orm::sqlx::Row;

#[derive(Clone, Copy)]
struct DurationAsOf(i64);

fn duration_order_parameters(
    ctx: &graphql_orm::async_graphql::Context<'_>,
) -> graphql_orm::async_graphql::Result<OrderExpressionParameters> {
    let as_of = ctx.data::<DurationAsOf>()?.0;
    Ok(OrderExpressionParameters::new().bind("as_of", SqlValue::Int(as_of)))
}

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[graphql_entity(
    backend = "sqlite",
    table = "composed_parents",
    plural = "ComposedParents",
    default_sort = "id ASC",
    auth = "none"
)]
#[graphql_orm(
    compose_complex_object,
    order_expression(name = "LabelLength", expression = "length(id)"),
    order_expression(
        name = "Duration",
        expression = "COALESCE(finished_at, :as_of) - started_at",
        parameters = "duration_order_parameters"
    )
)]
pub struct ComposedParent {
    #[primary_key]
    #[sortable]
    id: String,
    started_at: i64,
    finished_at: Option<i64>,
    #[graphql(skip)]
    #[relation(
        target = "ComposedChild",
        from = "id",
        to = "parent_id",
        multiple,
        emit_fk = false,
        order_aggregate(name = "ChildCount", aggregate = "count")
    )]
    children: Vec<ComposedChild>,
}

#[graphql_complex_object]
impl ComposedParent {
    #[graphql(name = "Duration")]
    async fn duration(
        &self,
        ctx: &graphql_orm::async_graphql::Context<'_>,
    ) -> graphql_orm::async_graphql::Result<i64> {
        Ok(self.finished_at.unwrap_or(ctx.data::<DurationAsOf>()?.0) - self.started_at)
    }
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "composed_children",
    plural = "ComposedChildren",
    default_sort = "id ASC",
    auth = "none"
)]
pub struct ComposedChild {
    #[primary_key]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    parent_id: String,
}

impl BatchLoadEntity for ComposedChild {
    fn batch_column() -> &'static str {
        "parent_id"
    }

    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, graphql_orm::sqlx::Error> {
        row.try_get("parent_id")
    }
}

schema_roots! {
    backend: "sqlite",
    schema_policy: "managed",
    auth: "none",
    query_custom_ops: [],
    entities: [ComposedParent, ComposedChild],
}

#[tokio::test]
async fn contextual_expression_ordering_is_bound_and_paginates_deterministically()
-> graphql_orm::Result<()> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    graphql_orm::sqlx::query(
        "CREATE TABLE composed_parents (id TEXT PRIMARY KEY, started_at INTEGER NOT NULL, finished_at INTEGER)",
    )
    .execute(database.pool())
    .await?;
    graphql_orm::sqlx::query(
        "INSERT INTO composed_parents (id, started_at, finished_at) VALUES ('b-tied-open', 30, NULL), ('long', 10, 30), ('a-tied', 10, 20)",
    )
    .execute(database.pool())
    .await?;

    let order = ComposedParentOrderByInput {
        duration: Some(OrderDirection::Desc),
        ..Default::default()
    };
    assert_eq!(
        order.to_sql_order().as_deref(),
        Some("(COALESCE(finished_at, ?) - started_at) DESC")
    );
    assert!(order.requires_context());

    let schema = schema_builder(database).data(DurationAsOf(40)).finish();
    let response = schema
        .execute(
            "query {
                composedParents(
                    orderBy: [{ Duration: DESC }]
                    page: { limit: 1, offset: 1 }
                ) {
                    edges { node { id Duration } }
                }
            }",
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().expect("GraphQL response JSON");
    let edges = data["composedParents"]["edges"]
        .as_array()
        .expect("computed-order edges");
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0]["node"]["id"].as_str(), Some("a-tied"));
    assert_eq!(edges[0]["node"]["Duration"].as_i64(), Some(10));

    let ascending = schema
        .execute(
            "query {
                composedParents(orderBy: [{ Duration: ASC }]) {
                    edges { node { id } }
                }
            }",
        )
        .await;
    assert!(ascending.errors.is_empty(), "{:?}", ascending.errors);
    let ascending = ascending.data.into_json().expect("GraphQL response JSON");
    let ids = ascending["composedParents"]["edges"]
        .as_array()
        .expect("ascending computed-order edges")
        .iter()
        .map(|edge| edge["node"]["id"].as_str().expect("node id"))
        .collect::<Vec<_>>();
    assert_eq!(ids, ["a-tied", "b-tied-open", "long"]);
    Ok(())
}

#[tokio::test]
async fn generated_relations_flatten_into_handwritten_complex_objects() -> graphql_orm::Result<()> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let sdl = schema_builder(database).finish().sdl();

    assert!(sdl.contains("Duration: Int!"));
    assert!(sdl.contains("children("));
    assert!(sdl.contains("Duration: OrderDirection"));
    assert!(sdl.contains("ChildCount: OrderDirection"));
    assert!(!sdl.contains("type ComposedParentGeneratedRelations"));
    Ok(())
}

#[tokio::test]
async fn relation_count_ordering_executes_as_a_correlated_server_expression()
-> graphql_orm::Result<()> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    graphql_orm::sqlx::query(
        "CREATE TABLE composed_parents (id TEXT PRIMARY KEY, started_at INTEGER NOT NULL, finished_at INTEGER)",
    )
    .execute(database.pool())
    .await?;
    graphql_orm::sqlx::query(
        "CREATE TABLE composed_children (id TEXT PRIMARY KEY, parent_id TEXT NOT NULL)",
    )
    .execute(database.pool())
    .await?;
    graphql_orm::sqlx::query(
        "INSERT INTO composed_parents (id, started_at, finished_at) VALUES ('none', 0, 0), ('one', 0, 0), ('two', 0, 0)",
    )
    .execute(database.pool())
    .await?;
    graphql_orm::sqlx::query(
        "INSERT INTO composed_children (id, parent_id) VALUES ('c1', 'one'), ('c2', 'two'), ('c3', 'two')",
    )
    .execute(database.pool())
    .await?;

    let order = ComposedParentOrderByInput {
        child_count: Some(OrderDirection::Desc),
        ..Default::default()
    };
    // Count ordering is authorized per request against the target's read
    // visibility. Without that step the term counts nothing and every query
    // builder refuses it (GHSA-f7xj-6695-ww2q).
    assert_eq!(
        order.to_sql_order().as_deref(),
        Some(
            "(SELECT COUNT(*) FROM composed_children AS __graphql_orm_order_relation WHERE __graphql_orm_order_relation.parent_id = composed_parents.id AND 1 = 0) DESC"
        )
    );
    assert!(order.requires_context());
    assert!(order.validate_order().is_err());
    assert!(
        EntityQuery::<ComposedParent, SqliteBackend>::new()
            .order_by(&order)
            .fetch_all(&database)
            .await
            .is_err()
    );

    let schema = schema_builder(database).finish();
    let response = schema
        .execute(
            "query {
                composedParents(orderBy: [{ ChildCount: DESC }]) {
                    edges { node { id } }
                }
            }",
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().expect("GraphQL response JSON");
    let edges = data["composedParents"]["edges"]
        .as_array()
        .expect("relation-count edges");
    assert_eq!(edges[0]["node"]["id"].as_str(), Some("two"));
    assert_eq!(edges[1]["node"]["id"].as_str(), Some("one"));
    assert_eq!(edges[2]["node"]["id"].as_str(), Some("none"));
    Ok(())
}

#[tokio::test]
async fn generated_order_expressions_compile_without_model_disclosure() {
    use graphql_orm::graphql::orm::*;
    use graphql_orm_ai_tool_profiles::*;
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    let sdl = schema_builder(database)
        .data(DurationAsOf(40))
        .finish()
        .sdl();
    let semantic = graphql_orm_semantic_catalog();
    let capabilities = AiGraphqlQueryCapabilityCatalog::compile(
        "expression-contracts",
        GraphqlExecutionTargetId::parse("expressions.test").unwrap(),
        &sdl,
        semantic,
        AiGraphqlQueryCapabilityLimits::default(),
    )
    .expect("generated roots compile with contextual and ordinary order expressions");
    assert_eq!(
        capabilities
            .capabilities()
            .filter(|c| semantic
                .operations
                .iter()
                .any(|op| op.field_name == c.field_name()
                    && op.generated_entity_name.as_deref() == Some("ComposedParent")))
            .count(),
        2
    );
    let entity = semantic
        .entities
        .iter()
        .find(|e| e.entity_name == "ComposedParent")
        .unwrap();
    for name in ["LabelLength", "Duration", "ChildCount"] {
        let contract = entity
            .generated_input_members
            .iter()
            .find(|m| m.input_type == "ComposedParentOrderByInput" && m.field_name == name)
            .expect("every expression and count has an explicit host contract");
        assert_eq!(contract.kind, GraphqlGeneratedInputKind::OrderBy);
        assert_eq!(
            contract.type_ref,
            GraphqlSemanticTypeRef::named("OrderDirection", GraphqlSemanticTypeKind::Enum, true)
        );
        assert!(!contract.required);
        assert!(sdl.contains(&format!("{name}: OrderDirection")));
        for capability in capabilities.capabilities() {
            assert!(
                !capability
                    .compact_argument_schema()
                    .to_string()
                    .contains(name)
            );
            assert!(!capability.argument_schema().to_string().contains(name));
        }
    }
    // Context parameters are supplied by the host provider, outside GraphQL inputs.
    assert!(!sdl.contains("as_of:"));
    assert!(!sdl.contains("asOf:"));
    assert!(semantic.operations.iter().all(|operation| {
        operation
            .arguments
            .iter()
            .all(|argument| argument.graphql_name != "as_of" && argument.graphql_name != "asOf")
    }));
}
