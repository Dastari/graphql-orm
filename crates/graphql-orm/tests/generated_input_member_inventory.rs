#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::graphql::orm::*;
use graphql_orm::prelude::*;
use graphql_orm_ai_tool_profiles::*;
use std::collections::{BTreeMap, BTreeSet};

fn expression_parameters(
    _: &graphql_orm::async_graphql::Context<'_>,
) -> graphql_orm::async_graphql::Result<OrderExpressionParameters> {
    Ok(OrderExpressionParameters::new().bind("anchor", SqlValue::Int(10)))
}

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    graphql_orm::async_graphql::SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[graphql_entity(
    table = "input_inventory_records",
    plural = "InputInventoryRecords",
    auth = "none",
    upsert = "label",
    aggregate = true,
    keyset = "hidden_sort asc, id asc"
)]
#[graphql_orm(
    compose_complex_object,
    search(index = false),
    order_expression(name = "LabelLength", expression = "length(label)"),
    order_expression(
        name = "Anchored",
        expression = "wide + :anchor",
        parameters = "expression_parameters"
    )
)]
pub struct InputInventoryRecord {
    #[primary_key]
    #[sortable]
    pub id: String,
    #[unique]
    #[filterable(type = "string")]
    #[sortable]
    #[graphql_orm(searchable(weight = "A"))]
    pub label: String,
    #[graphql_orm(non_exportable)]
    #[filterable(type = "string")]
    #[sortable]
    pub hidden_sort: String,
    #[graphql(skip)]
    #[input_only]
    #[filterable(type = "string")]
    #[sortable]
    pub credential: Option<String>,
    #[filterable(type = "int64")]
    pub wide: i64,
    #[filterable(type = "number")]
    pub quantity: i32,
    #[filterable(type = "boolean")]
    pub enabled: bool,
    #[filterable(type = "uuid")]
    pub reference: graphql_orm::uuid::Uuid,
    #[filterable(type = "bytes")]
    pub payload: Vec<u8>,
    #[date_field]
    #[filterable(type = "date")]
    pub occurred_at: Option<String>,
    #[graphql_orm(decimal(precision = 12, scale = 2))]
    #[filterable(type = "decimal")]
    pub amount: Option<graphql_orm::rust_decimal::Decimal>,
    #[graphql(skip)]
    #[graphql_orm(json)]
    pub document: Option<serde_json::Value>,
    #[graphql(skip)]
    #[graphql_orm(spatial(kind = "geometry", geometry_type = "Point", srid = 4326))]
    #[filterable(type = "spatial")]
    pub location: Option<serde_json::Value>,
    // A computed, non-persisted field does not add derived input/filter members.
    #[graphql(skip)]
    #[skip_db]
    #[graphql_orm(read = false)]
    #[filterable(type = "string")]
    #[sortable]
    pub computed: Option<String>,
    #[graphql(skip)]
    #[relation(
        target = "InputInventoryChild",
        from = "id",
        to = "record_id",
        multiple,
        emit_fk = false,
        filter,
        order_aggregate(name = "ChildCount", aggregate = "count")
    )]
    pub children: Vec<InputInventoryChild>,
}

#[cfg_attr(
    feature = "field-case-pascal",
    graphql_complex_object(rename_fields = "PascalCase")
)]
#[cfg_attr(not(feature = "field-case-pascal"), graphql_complex_object)]
impl InputInventoryRecord {
    async fn document(&self) -> Option<graphql_orm::async_graphql::Json<serde_json::Value>> {
        self.document.clone().map(graphql_orm::async_graphql::Json)
    }
    async fn location(&self) -> Option<graphql_orm::async_graphql::Json<serde_json::Value>> {
        self.location.clone().map(graphql_orm::async_graphql::Json)
    }
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "input_inventory_children",
    plural = "InputInventoryChildren",
    auth = "none"
)]
pub struct InputInventoryChild {
    #[primary_key]
    #[sortable]
    pub id: String,
    #[filterable(type = "string")]
    pub record_id: String,
}
impl BatchLoadEntity for InputInventoryChild {
    fn batch_column() -> &'static str {
        "record_id"
    }
    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, graphql_orm::sqlx::Error> {
        use graphql_orm::sqlx::Row;
        row.try_get("record_id")
    }
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "input_inventory_keys",
    plural = "InputInventoryKeys",
    auth = "none",
    repository_mutations = true,
    keyset = "hidden_sort asc, tenant asc, code asc"
)]
pub struct InputInventoryKey {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    pub tenant: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    pub code: String,
    #[sortable]
    #[filterable(type = "string")]
    #[graphql_orm(non_exportable)]
    pub hidden_sort: String,
}
schema_roots! {auth: "none", query_custom_ops: [], entities: [InputInventoryRecord, InputInventoryChild, InputInventoryKey]}

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

// Inventory every input in finished, derive-produced SDL, not a copied fixture.
fn all_inputs(sdl: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut inputs = BTreeMap::new();
    let mut lines = sdl.lines();
    while let Some(line) = lines.next() {
        let Some(name) = line
            .strip_prefix("input ")
            .and_then(|s| s.strip_suffix(" {"))
        else {
            continue;
        };
        let mut fields = BTreeMap::new();
        for line in lines.by_ref() {
            if line == "}" {
                break;
            }
            let Some((name, ty)) = line.trim().split_once(':') else {
                continue;
            };
            if name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                assert!(
                    fields
                        .insert(name.into(), ty.split_whitespace().next().unwrap().into())
                        .is_none()
                );
            }
        }
        assert!(!fields.is_empty(), "{name}");
        assert!(inputs.insert(name.into(), fields).is_none());
    }
    inputs
}

fn normalized(name: &str) -> String {
    name.replace('_', "").to_ascii_lowercase()
}

#[tokio::test]
async fn every_generated_input_member_has_an_owner_contract() {
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    let sdl = schema_builder(db).finish().sdl();
    let semantic = graphql_orm_semantic_catalog();
    let mut inputs = all_inputs(&sdl);
    let mut audited = BTreeSet::new();
    // Entity members must be public facts or explicit producer-hidden contracts.
    // Structural controls are checked separately against their fixed contract.
    for entity in &semantic.entities {
        for (kind, name) in [
            (
                GraphqlGeneratedInputKind::Create,
                format!("Create{}Input", entity.entity_name),
            ),
            (
                GraphqlGeneratedInputKind::Update,
                format!("Update{}Input", entity.entity_name),
            ),
            (
                GraphqlGeneratedInputKind::Where,
                format!("{}WhereInput", entity.entity_name),
            ),
            (
                GraphqlGeneratedInputKind::OrderBy,
                format!("{}OrderByInput", entity.entity_name),
            ),
            (
                GraphqlGeneratedInputKind::Key,
                format!("{}KeyInput", entity.entity_name),
            ),
            (
                GraphqlGeneratedInputKind::KeysetOrderBy,
                format!("{}KeysetOrderByInput", entity.entity_name),
            ),
        ] {
            let Some(fields) = inputs.remove(&name) else {
                continue;
            };
            audited.insert(name.clone());
            for (member, ty) in fields {
                if let Some(hidden) = entity
                    .generated_input_members
                    .iter()
                    .find(|hidden| hidden.input_type == name && hidden.field_name == member)
                {
                    assert_eq!(hidden.kind, kind, "{name}.{member}");
                    assert_eq!(render(&hidden.type_ref), ty, "{name}.{member}");
                    assert_eq!(hidden.required, ty.ends_with('!'), "{name}.{member}");
                } else if kind == GraphqlGeneratedInputKind::Where
                    && matches!(normalized(&member).as_str(), "and" | "or" | "not")
                {
                    assert_eq!(
                        ty,
                        if normalized(&member) == "not" {
                            name.clone()
                        } else {
                            format!("[{name}!]")
                        }
                    );
                } else {
                    assert!(
                        entity.fields.iter().any(|field| field.field_name == member),
                        "generated member {name}.{member} needs a public or hidden contract"
                    );
                }
                assert_ne!(normalized(&member), "computed");
            }
        }
        // Derived filter/operand helpers carry their own typed catalogue contracts.
        for field in &entity.fields {
            if let Some(filter) = &field.filter_input {
                let GraphqlSemanticTypeRef::Named { name, .. } = &filter.type_ref else {
                    panic!("named filter")
                };
                if let Some(fields) = inputs.remove(name) {
                    assert_eq!(
                        fields,
                        filter
                            .operators
                            .iter()
                            .map(|op| (op.graphql_name.clone(), render(&op.operand_type)))
                            .collect()
                    );
                    audited.insert(name.clone());
                }
                for object in &filter.input_objects {
                    if let Some(fields) = inputs.remove(&object.graphql_name) {
                        assert_eq!(
                            fields,
                            object
                                .fields
                                .iter()
                                .map(|field| (field.graphql_name.clone(), render(&field.type_ref)))
                                .collect()
                        );
                        audited.insert(object.graphql_name.clone());
                    }
                }
            }
        }
        let metric = format!("{}AggregateMetricInput", entity.entity_name);
        if let Some(fields) = inputs.remove(&metric) {
            assert_eq!(
                fields
                    .into_iter()
                    .map(|(name, ty)| (normalized(&name), ty))
                    .collect::<BTreeMap<_, _>>(),
                BTreeMap::from([
                    ("operator".into(), "GraphqlAggregateInputOperator!".into()),
                    (
                        "field".into(),
                        format!("{}AggregateField", entity.entity_name)
                    )
                ])
            );
            audited.insert(metric);
        }
    }
    for (name, fields) in inputs {
        let expected: &[(&str, &str)] = match name.as_str() {
            "PageInput" => &[("limit", "Int"), ("offset", "Int")],
            "AuthorizedScanInput" | "KeysetPageInput" => &[
                ("after", "String"),
                ("limit", "Int"),
                ("includetotalcount", "Boolean!"),
            ],
            "KeysetConnectionInput" => &[
                ("after", "String"),
                ("before", "String"),
                ("first", "Int"),
                ("last", "Int"),
                ("includetotalcount", "Boolean!"),
            ],
            "SearchInput" => &[
                ("query", "String!"),
                ("mode", "SearchMode"),
                ("minscore", "Float"),
            ],
            "SubscriptionFilterInput" => &[("actions", "[ChangeAction!]"), ("dummy", "Boolean")],
            _ => panic!("generated input {name} needs an explicit owner contract: {fields:?}"),
        };
        assert_eq!(
            fields
                .into_iter()
                .map(|(name, ty)| (normalized(&name), ty))
                .collect::<BTreeMap<_, _>>(),
            expected
                .iter()
                .map(|(name, ty)| (name.to_string(), ty.to_string()))
                .collect()
        );
        audited.insert(name);
    }
    for name in [
        "CreateInputInventoryRecordInput",
        "UpdateInputInventoryRecordInput",
        "InputInventoryRecordWhereInput",
        "InputInventoryRecordOrderByInput",
        "InputInventoryRecordKeysetOrderByInput",
        "InputInventoryKeyKeyInput",
        "InputInventoryKeyKeysetOrderByInput",
        "InputInventoryRecordAggregateMetricInput",
        "SearchInput",
        "SubscriptionFilterInput",
        "Int64Filter",
        "DecimalFilter",
        "DateFilter",
        "SpatialFilter",
        "StringFilter",
        "IntFilter",
        "UuidFilter",
        "BytesFilter",
        "BoolFilter",
    ] {
        assert!(audited.contains(name), "fixture must exercise {name}");
    }
    assert!(
        semantic
            .operations
            .iter()
            .any(|operation| operation.generated_category
                == Some(GeneratedGraphqlOperationCategory::Upsert))
    );
    assert!(semantic.entities.iter().any(|entity| {
        entity.generated_input_members.iter().any(|m| {
            m.kind == GraphqlGeneratedInputKind::KeysetOrderBy
                && normalized(&m.field_name) == "hiddensort"
        })
    }));
    let capabilities = AiGraphqlQueryCapabilityCatalog::compile(
        "input-inventory",
        GraphqlExecutionTargetId::parse("inventory.test").unwrap(),
        &sdl,
        semantic,
        AiGraphqlQueryCapabilityLimits::default(),
    )
    .unwrap();
    assert!(capabilities.capabilities().count() > 0);
}
