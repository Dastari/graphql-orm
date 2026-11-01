//! Focused real-producer parity for the six field-case qualification lanes.
#![cfg(feature = "sqlite")]
use graphql_orm::async_graphql::{InputObject, Object, Schema};
use graphql_orm::graphql::filters::*;
use graphql_orm::graphql::orm::*;
use graphql_orm::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "filter_contract_rows",
    plural = "FilterContractRows",
    backend = "sqlite",
    auth = "none"
)]
struct FilterContractRow {
    #[primary_key]
    #[filterable(type = "uuid")]
    #[sortable]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    text: String,
    #[filterable(type = "number")]
    quantity: i32,
    #[graphql_orm(decimal(precision = 12, scale = 2))]
    #[filterable(type = "decimal")]
    amount: graphql_orm::rust_decimal::Decimal,
    #[filterable(type = "bool")]
    enabled: bool,
    #[filterable(type = "bytes")]
    payload: Vec<u8>,
    #[filterable(type = "date")]
    occurred_at: Option<String>,
    #[graphql_orm(spatial(kind = "geometry", geometry_type = "Point", srid = 4326))]
    #[filterable(type = "spatial")]
    location: graphql_orm::serde_json::Value,
}
schema_roots! { entities: [FilterContractRow] }

fn render(ty: &GraphqlSemanticTypeRef) -> String {
    match ty {
        GraphqlSemanticTypeRef::Named { name, nullable, .. } => {
            format!("{name}{}", if *nullable { "" } else { "!" })
        }
        GraphqlSemanticTypeRef::List { item, nullable, .. } => {
            format!("[{}]{}", render(item), if *nullable { "" } else { "!" })
        }
    }
}

// Read the actual finished SDL emitted by async-graphql, not a copied fixture.
fn input_fields(sdl: &str, name: &str) -> BTreeMap<String, String> {
    let marker = format!("input {name} {{\n");
    let body = sdl
        .split_once(&marker)
        .expect("real input type")
        .1
        .split_once("\n}")
        .unwrap()
        .0;
    body.lines()
        .filter_map(|line| {
            let line = line.strip_prefix('\t')?;
            let (name, ty) = line.split_once(':')?;
            if name.contains(' ') || name.contains('"') {
                return None;
            }
            Some((
                name.to_owned(),
                ty.split_whitespace().next().unwrap().to_owned(),
            ))
        })
        .collect()
}

fn assert_contract(sdl: &str, input: &GraphqlSemanticFilterInputMetadata) {
    input.validate().unwrap();
    let GraphqlSemanticTypeRef::Named { name, .. } = &input.type_ref else {
        panic!("helper type");
    };
    let expected = input
        .operators
        .iter()
        .map(|op| (op.graphql_name.clone(), render(&op.operand_type)))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(input_fields(sdl, name), expected);
    for object in &input.input_objects {
        let expected = object
            .fields
            .iter()
            .map(|field| (field.graphql_name.clone(), render(&field.type_ref)))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(input_fields(sdl, &object.graphql_name), expected);
    }
}

#[test]
fn all_eight_static_filter_families_match_actual_derived_sdl() {
    let schema = Schema::build(
        QueryRoot::default(),
        MutationRoot::default(),
        SubscriptionRoot::default(),
    )
    .finish();
    let sdl = schema.sdl();
    let entity = FilterContractRow::graphql_semantic_metadata().unwrap();
    assert_eq!(
        entity
            .fields
            .iter()
            .filter(|field| field.filter_input.is_some())
            .count(),
        8
    );
    let where_fields = input_fields(&sdl, &format!("{}WhereInput", entity.entity_name));
    for field in &entity.fields {
        let input = field
            .filter_input
            .as_ref()
            .expect("all eight fields filterable");
        assert_eq!(
            where_fields.get(&field.field_name),
            Some(&render(&input.type_ref))
        );
        assert_contract(&sdl, input);
        assert_eq!(
            field.filter_operators,
            input
                .operators
                .iter()
                .map(|op| op.operator)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        );
    }
    // BytesFilter intentionally has no field-case cfg. Its real wire names are
    // camelCase even in the Pascal/snake/screaming/lower/upper focused lanes.
    let bytes = BytesFilter::graphql_semantic_filter_input();
    assert!(bytes.operators.iter().any(|op| op.graphql_name == "inList"));
    assert!(bytes.operators.iter().any(|op| op.graphql_name == "isNull"));
}

#[derive(graphql_orm::async_graphql::Enum, Copy, Clone, Eq, PartialEq)]
enum FilterState {
    Open,
    Closed,
}
#[derive(InputObject)]
struct EnumFilter {
    eq: Option<FilterState>,
    in_list: Option<Vec<FilterState>>,
}
struct EnumQuery;
#[Object]
impl EnumQuery {
    async fn state(&self, filter: EnumFilter) -> bool {
        filter.eq.is_some() || filter.in_list.is_some()
    }
}

#[test]
fn explicit_custom_enum_contract_binds_actual_enum_operands() {
    let schema = Schema::build(
        EnumQuery,
        graphql_orm::async_graphql::EmptyMutation,
        graphql_orm::async_graphql::EmptySubscription,
    )
    .finish();
    let value = GraphqlSemanticTypeRef::named("FilterState", GraphqlSemanticTypeKind::Enum, false);
    let input = GraphqlSemanticFilterInputMetadata::new(
        GraphqlSemanticTypeRef::named("EnumFilter", GraphqlSemanticTypeKind::Object, true),
        value.clone(),
        vec![
            GraphqlSemanticFilterInputOperator {
                graphql_name: "eq".into(),
                operator: GraphqlSemanticFilterOperator::Equal,
                operand_type: GraphqlSemanticTypeRef::named(
                    "FilterState",
                    GraphqlSemanticTypeKind::Enum,
                    true,
                ),
            },
            GraphqlSemanticFilterInputOperator {
                graphql_name: "inList".into(),
                operator: GraphqlSemanticFilterOperator::In,
                operand_type: GraphqlSemanticTypeRef::list(true, None, value),
            },
        ],
        Vec::new(),
    )
    .unwrap();
    assert_contract(&schema.sdl(), &input);
}

#[cfg(feature = "runtime-graphql")]
#[tokio::test]
async fn all_runtime_value_kinds_match_actual_runtime_sdl() {
    use graphql_orm::graphql::runtime::*;
    use std::sync::Arc;
    let kinds = [
        RuntimeValueKind::Uuid,
        RuntimeValueKind::Boolean,
        RuntimeValueKind::Integer,
        RuntimeValueKind::Float,
        RuntimeValueKind::String,
        RuntimeValueKind::Json,
        RuntimeValueKind::Bytes,
        RuntimeValueKind::DateTime,
        RuntimeValueKind::Decimal(DecimalDef::new(12, 2).unwrap()),
    ];
    let fields = kinds
        .into_iter()
        .enumerate()
        .map(|(i, value_kind)| RuntimeField {
            id: FieldId::new(format!("field_{i}")).unwrap(),
            api_name: format!("field{i}"),
            physical_column: format!("field_{i}"),
            value_kind,
            nullable: false,
            unique: false,
            filterable: true,
            sortable: value_kind != RuntimeValueKind::Json,
            generated: false,
            default: None,
            format: None,
            description: None,
            classification: None,
            export: None,
            auto_timestamp: None,
            auto_actor: None,
        })
        .collect::<Vec<_>>();
    let primary_key = vec![fields[0].id.clone()];
    let collection = RuntimeCollection {
        id: CollectionId::new("filter_values").unwrap(),
        api_type_name: "FilterValues".into(),
        api_plural_name: "filterValues".into(),
        physical_table: "filter_values".into(),
        primary_key,
        append_only: false,
        retention_purge: false,
        fields,
        relations: Vec::new(),
        indexes: Vec::new(),
        composite_unique: Vec::new(),
        default_order: Vec::new(),
        description: None,
        classification: None,
        label_fields: Vec::new(),
        search: None,
    };
    let runtime = Arc::new(
        RuntimeSchema {
            format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
            collections: vec![collection],
        }
        .validate()
        .unwrap(),
    );
    let module = RuntimeGraphqlModule::compile(
        runtime,
        RuntimeGraphqlOptions::default().with_cursor_profile(RuntimeCursorProfile::Unprotected),
    )
    .unwrap();
    // Execution of this disposable fixture requires the later captain grant.
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    let (schema, catalog) = RuntimeGraphqlComposer::new(database, "Query", Default::default())
        .unwrap()
        .install(module)
        .unwrap()
        .finish_with_semantic_catalog()
        .unwrap();
    assert_eq!(catalog.entities[0].fields.len(), 9);
    let sdl = schema.sdl();
    let where_fields = input_fields(
        &sdl,
        &format!("{}WhereInput", catalog.entities[0].entity_name),
    );
    for field in &catalog.entities[0].fields {
        let input = field.filter_input.as_ref().unwrap();
        assert_eq!(
            where_fields.get(&field.field_name),
            Some(&render(&input.type_ref))
        );
        assert_contract(&sdl, input);
    }
}

// Only schema construction/planning runs here; the resolver and geometry SQL
// are never executed. These are the actual derive-produced Where/filter types.
struct FilterPlanningQuery;
#[Object]
impl FilterPlanningQuery {
    #[graphql(name = "GetSpatial")]
    async fn get_spatial(
        &self,
        #[graphql(name = "where")] filter: Option<FilterContractRowWhereInput>,
    ) -> graphql_orm::async_graphql::Result<FilterContractRow> {
        let _ = filter;
        Err(graphql_orm::async_graphql::Error::new(
            "planning-only schema; no resolver execution",
        ))
    }
}

#[test]
fn actual_derived_spatial_contract_compiles_exact_json_plans_for_all_predicates() {
    use graphql_orm::serde_json::{Value, json};
    use graphql_orm_ai_tool_profiles::{
        AiGraphqlQueryCapabilityCatalog, AiGraphqlQueryCapabilityLimits, GraphqlExecutionTargetId,
    };
    let schema = Schema::build(
        FilterPlanningQuery,
        graphql_orm::async_graphql::EmptyMutation,
        graphql_orm::async_graphql::EmptySubscription,
    )
    .finish();
    let entity = FilterContractRow::graphql_semantic_metadata().unwrap();
    let id = entity
        .fields
        .iter()
        .find(|field| {
            field.filter_input.as_ref().is_some_and(|input| {
        matches!(&input.value_type, GraphqlSemanticTypeRef::Named { name, .. } if name == "UUID")
    })
        })
        .unwrap()
        .field_name
        .clone();
    let location = entity
        .fields
        .iter()
        .find(|field| {
            field
                .filter_operators
                .contains(&GraphqlSemanticFilterOperator::SpatialIntersects)
        })
        .unwrap()
        .clone();
    let operation = GraphqlSemanticOperationDescriptor::custom(
        GraphqlOperationKind::Query,
        "GetSpatial",
        "Plan a public spatial filter.",
        vec![GraphqlSemanticArgumentDescriptor {
            graphql_name: "where".into(),
            description: "Actual derived filters.".into(),
            type_ref: GraphqlSemanticTypeRef::named(
                "FilterContractRowWhereInput",
                GraphqlSemanticTypeKind::Object,
                true,
            ),
        }],
        GraphqlSemanticTypeRef::named(
            entity.entity_name.clone(),
            GraphqlSemanticTypeKind::Object,
            false,
        ),
        true,
    )
    .unwrap();
    let semantic = GraphqlSemanticCatalog::compose_with_custom(
        [entity.clone()],
        &GraphqlOperationCatalog::compose(std::iter::empty()),
        [operation],
    )
    .unwrap();
    let catalog = AiGraphqlQueryCapabilityCatalog::compile(
        "filter-contract",
        GraphqlExecutionTargetId::parse("filter-contract.graphql").unwrap(),
        &schema.sdl(),
        &semantic,
        AiGraphqlQueryCapabilityLimits::default(),
    )
    .unwrap();
    let capability = catalog.capabilities().next().unwrap();
    let point = json!({"type": "Point", "coordinates": [0, 0]});
    for operator in &location.filter_input.as_ref().unwrap().operators {
        let value = if operator.operator == GraphqlSemanticFilterOperator::IsNull {
            json!(false)
        } else {
            point.clone()
        };
        let filter = Value::Object(graphql_orm::serde_json::Map::from_iter([(
            location.field_name.clone(),
            Value::Object(graphql_orm::serde_json::Map::from_iter([(
                operator.graphql_name.clone(),
                value,
            )])),
        )]));
        let compact = capability.compile_compact(json!({"arguments": {"where": filter}, "relationshipArguments": {}, "relationshipMaximumItems": {}, "selections": [id]})).unwrap();
        let nested = capability.compile(json!({"arguments": {"where": filter}, "fields": {(id.clone()): true}, "relationships": {}})).unwrap();
        assert_eq!(compact.variables(), &json!({"v0": filter}));
        assert_eq!(compact.variables(), nested.variables());
        for plan in [&compact, &nested] {
            let variables_schema = &plan.descriptor().argument_schema;
            assert!(variables_schema.get("$defs").is_some());
            assert!(variables_schema["properties"]["v0"].get("$defs").is_none());
            // Final descriptor construction validates this complete wrapper.
            assert_eq!(
                plan.descriptor().document,
                format!(
                    "query AiQuery_{}($v0: FilterContractRowWhereInput) {{ GetSpatial(where: $v0) {{ {id} }} }}",
                    &plan.plan_fingerprint()[..20]
                )
            );
        }
    }
    let intersects = location
        .filter_input
        .as_ref()
        .unwrap()
        .operators
        .iter()
        .find(|operator| operator.operator == GraphqlSemanticFilterOperator::SpatialIntersects)
        .unwrap();
    // Json<Value> accepts these wire values structurally. Geometry validity and
    // SQL/resolver admission remain authoritative and are not claimed here.
    for value in [
        Value::Null,
        json!([1, null, {"opaque": true}]),
        json!({}),
        json!("opaque scalar string"),
    ] {
        let filter = Value::Object(graphql_orm::serde_json::Map::from_iter([(
            location.field_name.clone(),
            Value::Object(graphql_orm::serde_json::Map::from_iter([(
                intersects.graphql_name.clone(),
                value,
            )])),
        )]));
        capability.compile_compact(json!({"arguments": {"where": filter}, "relationshipArguments": {}, "relationshipMaximumItems": {}, "selections": [id]})).unwrap();
        capability.compile(json!({"arguments": {"where": filter}, "fields": {(id.clone()): true}, "relationships": {}})).unwrap();
    }
}
