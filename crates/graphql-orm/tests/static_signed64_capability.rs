//! Actual generated producer -> capability compiler -> exact variables -> resolver.
#![cfg(feature = "sqlite")]
use graphql_orm::async_graphql::{Context, Request, Variables};
use graphql_orm::db::Database;
use graphql_orm::graphql::filters::Int64Filter;
use graphql_orm::prelude::*;
use graphql_orm::serde_json::json;
use graphql_orm_ai_tool_profiles::{
    AiGraphqlQueryCapabilityCatalog, AiGraphqlQueryCapabilityLimits, GraphqlExecutionTargetId,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "capability64",
    plural = "Capability64Rows",
    backend = "sqlite",
    auth = "none",
    query_shapes = "list"
)]
struct Capability64 {
    #[primary_key]
    #[sortable]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "int64")]
    #[graphql_orm(read_policy = "signed64.read")]
    value: Option<i64>,
    #[graphql_orm(read = false, filter = false, order = false, subscribe = false)]
    private_value: Option<i64>,
}
schema_roots! { entities: [Capability64] }

struct Policy(Arc<AtomicBool>);
impl FieldPolicy for Policy {
    fn can_read_field<'a>(
        &'a self,
        _: &'a Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        policy: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(policy.is_none()
                || (policy == Some("signed64.read") && self.0.load(Ordering::SeqCst)))
        })
    }
    fn can_write_field<'a>(
        &'a self,
        _: &'a Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
}

#[tokio::test]
async fn exact_capability_execution_and_private_roles_stay_closed() {
    let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    graphql_orm::sqlx::query(
        "CREATE TABLE capability64 (id TEXT PRIMARY KEY, value INTEGER, private_value INTEGER)",
    )
    .execute(db.pool())
    .await
    .unwrap();
    let mut ids = Vec::new();
    for value in [i64::MIN, i64::MAX, 9_007_199_254_740_993] {
        let id = graphql_orm::uuid::Uuid::new_v4();
        ids.push((value, id));
        graphql_orm::sqlx::query("INSERT INTO capability64 VALUES (?,?,?)")
            .bind(id.to_string())
            .bind(value)
            .bind(i64::MAX)
            .execute(db.pool())
            .await
            .unwrap();
    }
    let allowed = Arc::new(AtomicBool::new(true));
    db.set_field_policy(Policy(allowed.clone()));
    let schema = schema_builder(db.clone()).finish();
    let semantic = graphql_orm_semantic_catalog();
    let compile = |catalog: &GraphqlSemanticCatalog| {
        AiGraphqlQueryCapabilityCatalog::compile(
            "signed64",
            GraphqlExecutionTargetId::parse("signed64.graphql").unwrap(),
            &schema.sdl(),
            catalog,
            AiGraphqlQueryCapabilityLimits::default(),
        )
    };
    let capabilities = compile(semantic).unwrap();
    assert_eq!(
        capabilities.capabilities().count(),
        1,
        "only generated list is exposed"
    );
    let capability = capabilities.capabilities().next().unwrap();
    let eq = Int64Filter::graphql_semantic_filter_input()
        .operators
        .into_iter()
        .find(|op| op.operator == GraphqlSemanticFilterOperator::Equal)
        .unwrap()
        .graphql_name;
    for (value, id) in &ids {
        let filter = json!({"value": {(eq.clone()): value.to_string()}});
        let compiled = capability
            .compile_compact(
                json!({"arguments":{"where":filter}, "maximumItems":10, "relationshipArguments":{}, "relationshipMaximumItems":{}, "selections":["id"]}),
            )
            .unwrap();
        assert!(
            compiled
                .variables()
                .as_object()
                .unwrap()
                .values()
                .any(|v| v == &filter)
        );
        let response = schema
            .execute(
                Request::new(compiled.descriptor().document.clone())
                    .variables(Variables::from_json(compiled.variables().clone())),
            )
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json().unwrap().to_string();
        assert!(data.contains(&id.to_string()));
        for (_, other) in &ids {
            if other != id {
                assert!(!data.contains(&other.to_string()));
            }
        }
    }
    for value in [
        "9223372036854775808",
        "-9223372036854775809",
        "01",
        "+1",
        "1e3",
    ] {
        let filter = json!({"value": {(eq.clone()): value}});
        let compiled = capability
            .compile_compact(
                json!({"arguments":{"where":filter}, "maximumItems":10, "relationshipArguments":{}, "relationshipMaximumItems":{}, "selections":["id"]}),
            )
            .unwrap();
        let response = schema
            .execute(
                Request::new(compiled.descriptor().document.clone())
                    .variables(Variables::from_json(compiled.variables().clone())),
            )
            .await;
        assert!(
            !response.errors.is_empty(),
            "resolver admission must reject {value}"
        );
    }
    // Wire shape/role is closed: scalar operands cannot become arbitrary JSON,
    // helper objects cannot grant a private field or a made-up operator.
    for filter in [
        json!({"value": {(eq.clone()):{"value":"1"}}}),
        json!({"privateValue": {(eq.clone()):"1"}}),
        json!({"value":{"forgedHelper":"1"}}),
    ] {
        assert!(
            capability
                .compile_compact(
                    json!({"arguments":{"where":filter}, "maximumItems":10, "relationshipArguments":{}, "relationshipMaximumItems":{}, "selections":["id"]})
                )
                .is_err()
        );
    }
    let mut forged_entities = semantic.entities.to_vec();
    let field = forged_entities[0]
        .fields
        .iter_mut()
        .find(|f| f.field_name == "value")
        .unwrap();
    field.filter_input.as_mut().unwrap().type_ref =
        GraphqlSemanticTypeRef::named("StringFilter", GraphqlSemanticTypeKind::Object, true);
    if let Ok(forged) =
        GraphqlSemanticCatalog::compose(forged_entities, graphql_orm_operation_catalog())
    {
        assert!(
            compile(&forged).is_err(),
            "actual Signed64 SDL cannot be assigned a forged helper role"
        );
    }

    // Observe real SQLite VM work. Denied referenced-field intent must fail
    // before selecting/counting, including nested NOT/OR and membership.
    let work = Arc::new(AtomicUsize::new(0));
    {
        let mut connection = db.pool().acquire().await.unwrap();
        let mut handle = connection.lock_handle().await.unwrap();
        let observed = work.clone();
        handle.set_progress_handler(1, move || {
            observed.fetch_add(1, Ordering::SeqCst);
            true
        });
    }
    let in_name = Int64Filter::graphql_semantic_filter_input()
        .operators
        .into_iter()
        .find(|op| op.operator == GraphqlSemanticFilterOperator::In)
        .unwrap()
        .graphql_name;
    let flat_filter = json!({"value":{(in_name.clone()):[i64::MIN.to_string()]}});
    let compiled = capability.compile_compact(json!({"arguments":{"where":flat_filter.clone()}, "maximumItems":10, "relationshipArguments":{}, "relationshipMaximumItems":{}, "selections":["id"]})).unwrap();
    let recursive_filter = json!({"or":[{"not":{"value":{(eq):i64::MAX.to_string()}}}, {"value":{(in_name):[i64::MIN.to_string()]}}]});
    // Provider plans deliberately omit recursive connectives. Keep that boundary
    // closed, then exercise the ordinary generated resolver's recursive input.
    assert!(capability.compile_compact(json!({"arguments":{"where":recursive_filter.clone()}, "maximumItems":10, "relationshipArguments":{}, "relationshipMaximumItems":{}, "selections":["id"]})).is_err());
    let mut recursive_variables = compiled.variables().clone();
    let where_variable = recursive_variables
        .as_object_mut()
        .unwrap()
        .values_mut()
        .find(|value| **value == flat_filter)
        .expect("compiled where variable");
    *where_variable = recursive_filter;
    for (recursive, variables) in [
        (false, compiled.variables().clone()),
        (true, recursive_variables),
    ] {
        allowed.store(false, Ordering::SeqCst);
        work.store(0, Ordering::SeqCst);
        let response = schema
            .execute(
                Request::new(compiled.descriptor().document.clone())
                    .variables(Variables::from_json(variables.clone())),
            )
            .await;
        assert!(!response.errors.is_empty());
        assert_eq!(
            work.load(Ordering::SeqCst),
            0,
            "denied filter intent reached SQL (recursive={recursive})"
        );
        allowed.store(true, Ordering::SeqCst);
        work.store(0, Ordering::SeqCst);
        let response = schema
            .execute(
                Request::new(compiled.descriptor().document.clone())
                    .variables(Variables::from_json(variables)),
            )
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        assert!(
            work.load(Ordering::SeqCst) > 0,
            "positive control observes VM work (recursive={recursive})"
        );
        let data = response.data.into_json().unwrap().to_string();
        for (value, id) in &ids {
            let expected = if recursive {
                *value != i64::MAX
            } else {
                *value == i64::MIN
            };
            assert_eq!(data.contains(&id.to_string()), expected);
        }
    }
}
