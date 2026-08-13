//! Automatic, closed query capabilities compiled from canonical semantics.
//!
//! A capability is discovery metadata, not authority. Model input is a finite
//! typed plan; this module validates it and emits one exact server-owned
//! GraphQL document, variables object, disclosure schema, and drift binding.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use async_graphql_parser::{parse_schema, types::TypeSystemDefinition};
use graphql_orm_operation_catalog::{
    GeneratedGraphqlOperationCategory, GraphqlOperationKind, GraphqlSemanticCatalog,
    GraphqlSemanticClassification, GraphqlSemanticExport, GraphqlSemanticFieldMetadata,
    GraphqlSemanticOperationDescriptor, GraphqlSemanticRelationshipCardinality,
    GraphqlSemanticTypeRef,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{
    AiApprovalRule, AiDisclosureRule, AiDisclosureSchema, AiDisclosureShape, AiError,
    AiToolDescriptor, AiToolId, AiToolOperationKind, AiToolRisk, DataClassification,
    GraphqlExecutionTargetId, GraphqlOperationContract, ToolMaturity,
    canonical_json::canonical_json_bytes,
};

/// Current automatic query-capability contract version.
pub const AI_GRAPHQL_QUERY_CAPABILITY_VERSION: u16 = 1;

const JSON_SCHEMA_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";
const MAXIMUM_DESCRIPTION_BYTES: usize = 1_024;
const MAXIMUM_PROVIDER_SCHEMA_BYTES: usize = 1024 * 1024;

/// Immutable deployment ceilings for automatic GraphQL query capabilities.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiGraphqlQueryCapabilityLimits {
    /// Maximum selectable relationship depth.
    pub maximum_depth: u8,
    /// Maximum fields selected across one plan.
    pub maximum_selected_fields: u16,
    /// Maximum root or relationship arguments across one plan.
    pub maximum_arguments: u16,
    /// Maximum nested input-object depth.
    pub maximum_input_depth: u8,
    /// Maximum UTF-8 bytes for one string input.
    pub maximum_string_bytes: u32,
    /// Maximum items in one input list or selected relationship collection.
    pub maximum_list_items: u32,
    /// Maximum records disclosed by one compiled query.
    pub maximum_result_records: u32,
    /// Maximum result bytes disclosed by one compiled query.
    pub maximum_result_bytes: u64,
    /// Maximum query roots admitted from one finished schema.
    pub maximum_capabilities: u16,
    /// Maximum finished SDL bytes accepted during compilation.
    pub maximum_schema_bytes: u32,
}

impl AiGraphqlQueryCapabilityLimits {
    /// Creates validated deployment ceilings.
    ///
    /// # Errors
    ///
    /// Returns an error for zero or excessive values.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        maximum_depth: u8,
        maximum_selected_fields: u16,
        maximum_arguments: u16,
        maximum_input_depth: u8,
        maximum_string_bytes: u32,
        maximum_list_items: u32,
        maximum_result_records: u32,
        maximum_result_bytes: u64,
        maximum_capabilities: u16,
        maximum_schema_bytes: u32,
    ) -> Result<Self, AiError> {
        let value = Self {
            maximum_depth,
            maximum_selected_fields,
            maximum_arguments,
            maximum_input_depth,
            maximum_string_bytes,
            maximum_list_items,
            maximum_result_records,
            maximum_result_bytes,
            maximum_capabilities,
            maximum_schema_bytes,
        };
        if maximum_depth == 0
            || maximum_depth > 8
            || maximum_selected_fields == 0
            || maximum_selected_fields > 512
            || maximum_arguments == 0
            || maximum_arguments > 256
            || maximum_input_depth == 0
            || maximum_input_depth > 16
            || maximum_string_bytes == 0
            || maximum_string_bytes > 1_048_576
            || maximum_list_items == 0
            || maximum_list_items > 10_000
            || maximum_result_records == 0
            || maximum_result_records > 10_000
            || maximum_result_bytes == 0
            || maximum_result_bytes > 64 * 1024 * 1024
            || maximum_capabilities == 0
            || maximum_capabilities > 8_192
            || maximum_schema_bytes == 0
            || maximum_schema_bytes > 16 * 1024 * 1024
        {
            return Err(configuration_error(
                "automatic query capability limits are invalid",
            ));
        }
        Ok(value)
    }
}

impl Default for AiGraphqlQueryCapabilityLimits {
    fn default() -> Self {
        Self {
            maximum_depth: 4,
            maximum_selected_fields: 64,
            maximum_arguments: 64,
            maximum_input_depth: 8,
            maximum_string_bytes: 4_096,
            maximum_list_items: 100,
            maximum_result_records: 100,
            maximum_result_bytes: 256 * 1024,
            maximum_capabilities: 1_024,
            maximum_schema_bytes: 4 * 1024 * 1024,
        }
    }
}

/// One closed model-authored relationship selection.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiGraphqlRelationshipQueryPlan {
    /// Exact typed relationship arguments, excluding the server-owned limit.
    #[serde(default)]
    pub arguments: Map<String, Value>,
    /// Public scalar or enum fields selected from the related entity. Values
    /// must be `true`; the closed object form preserves per-field descriptions
    /// in provider schemas.
    #[serde(default)]
    pub fields: BTreeMap<String, bool>,
    /// Explicit nested relationship selections.
    #[serde(default)]
    pub relationships: BTreeMap<String, AiGraphqlRelationshipQueryPlan>,
    /// Positive result ceiling, required for a collection relationship.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_items: Option<u32>,
}

/// One closed model-authored query plan.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiGraphqlQueryPlan {
    /// Exact typed root arguments, excluding any server-owned result limit.
    #[serde(default)]
    pub arguments: Map<String, Value>,
    /// Public scalar or enum fields selected from the root entity. Values must
    /// be `true`; the closed object form preserves per-field descriptions.
    #[serde(default)]
    pub fields: BTreeMap<String, bool>,
    /// Explicit relationship selections.
    #[serde(default)]
    pub relationships: BTreeMap<String, AiGraphqlRelationshipQueryPlan>,
    /// Positive root result ceiling when the root returns a collection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_items: Option<u32>,
}

/// One deterministic provider-facing query capability segment.
#[derive(Clone, Debug)]
pub struct AiGraphqlQueryCapability {
    id: AiToolId,
    operation: GraphqlSemanticOperationDescriptor,
    description: String,
    argument_schema: Value,
    fingerprint: String,
    schema_fingerprint: String,
    target_id: GraphqlExecutionTargetId,
    semantic_catalog: GraphqlSemanticCatalog,
    schema: FinishedSchema,
    limits: AiGraphqlQueryCapabilityLimits,
    output: QueryOutput,
}

impl AiGraphqlQueryCapability {
    /// Returns the stable public subgraph/root capability ID.
    pub fn id(&self) -> &AiToolId {
        &self.id
    }

    /// Returns the model-safe root description.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the finite JSON Schema 2020-12 query-plan contract.
    pub fn argument_schema(&self) -> &Value {
        &self.argument_schema
    }

    /// Returns the complete capability fingerprint.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Returns the exact public GraphQL root field.
    pub fn field_name(&self) -> &str {
        &self.operation.field_name
    }

    /// Returns whether this is an opt-in generated aggregate root.
    pub fn is_aggregate(&self) -> bool {
        self.operation.generated_category == Some(GeneratedGraphqlOperationCategory::Aggregate)
    }

    /// Compiles one typed plan into an exact immutable execution contract.
    ///
    /// # Errors
    ///
    /// Returns a safe input error for unknown, hidden, cyclic, stale,
    /// excessive, malformed, or unbounded plan content.
    pub fn compile(&self, plan: Value) -> Result<AiCompiledGraphqlQuery, AiError> {
        let validator = jsonschema::validator_for(&self.argument_schema)
            .map_err(|_| configuration_error("query capability schema is invalid"))?;
        if !validator.is_valid(&plan) {
            return Err(input_error(
                "query plan does not match the capability schema",
            ));
        }
        let plan: AiGraphqlQueryPlan = serde_json::from_value(plan)
            .map_err(|_| input_error("query plan has an invalid shape"))?;
        let mut context = CompileContext::new(self);
        let root_field = self.schema.root_query_field(&self.operation.field_name)?;
        let arguments = context.compile_arguments(
            &root_field.arguments,
            &self.operation.arguments,
            &plan.arguments,
            plan.maximum_items,
            self.output.requires_root_bound(),
            "root",
        )?;

        let (projection, disclosure) = match &self.output {
            QueryOutput::Entity { entity, route } => {
                let selection = EntityPlanRef {
                    fields: &plan.fields,
                    relationships: &plan.relationships,
                };
                let mut ancestry = vec![entity.clone()];
                let (entity_projection, entity_disclosure) =
                    context.compile_entity_selection(entity, selection, &mut ancestry, 0)?;
                let maximum = plan.maximum_items.unwrap_or(1);
                wrap_projection_route(route, entity_projection, entity_disclosure, maximum)
            }
            QueryOutput::Scalar {
                classification,
                maximum_items,
            } => {
                if !plan.fields.is_empty()
                    || !plan.relationships.is_empty()
                    || maximum_items.is_some() != plan.maximum_items.is_some()
                    || plan.maximum_items.is_some_and(|requested| {
                        requested == 0 || maximum_items.is_none_or(|maximum| requested > maximum)
                    })
                {
                    return Err(input_error("scalar query plans cannot select fields"));
                }
                let scalar =
                    AiDisclosureShape::scalar(AiDisclosureRule::exportable(*classification));
                let disclosure = match plan.maximum_items {
                    Some(maximum) => AiDisclosureShape::list(
                        AiDisclosureRule::exportable(*classification),
                        maximum,
                        scalar,
                    ),
                    None => scalar,
                };
                (String::new(), disclosure)
            }
            QueryOutput::Aggregate {
                projection,
                disclosure,
            } => {
                if !plan.fields.is_empty() || !plan.relationships.is_empty() {
                    return Err(input_error(
                        "aggregate query plans use a fixed result shape",
                    ));
                }
                (projection.clone(), disclosure.clone())
            }
        };
        context.ensure_totals()?;

        let variable_schema = context.variable_schema()?;
        let variables = context.variables;
        let variable_definitions = context.variable_definitions;
        let variable_clause = if variable_definitions.is_empty() {
            String::new()
        } else {
            format!("({})", variable_definitions.join(", "))
        };
        let argument_clause = if arguments.is_empty() {
            String::new()
        } else {
            format!("({})", arguments.join(", "))
        };
        let plan_fingerprint = sha256_json(&json!({
            "version": AI_GRAPHQL_QUERY_CAPABILITY_VERSION,
            "capability_fingerprint": self.fingerprint,
            "plan": plan,
        }));
        let operation_name = format!("AiQuery_{}", &plan_fingerprint[..20]);
        let document = if projection.is_empty() {
            format!(
                "query {operation_name}{variable_clause} {{ {}{argument_clause} }}",
                self.operation.field_name
            )
        } else {
            format!(
                "query {operation_name}{variable_clause} {{ {}{argument_clause} {projection} }}",
                self.operation.field_name
            )
        };
        async_graphql::parser::parse_query(&document)
            .map_err(|_| configuration_error("compiled query document is invalid"))?;
        let disclosure = AiDisclosureShape::object(
            AiDisclosureRule::exportable(maximum_classification(&disclosure)),
            [(self.operation.field_name.clone(), disclosure)],
        );
        let disclosure_schema = AiDisclosureSchema::new(
            format!("automatic-query-v{AI_GRAPHQL_QUERY_CAPABILITY_VERSION}"),
            disclosure,
        )?;
        let projection_fingerprint = sha256_json(&json!({
            "capability": self.fingerprint,
            "plan": plan_fingerprint,
            "document": document,
        }));
        let contract = GraphqlOperationContract::new(
            self.target_id.clone(),
            self.schema_fingerprint.clone(),
            operation_name,
            &document,
            projection_fingerprint.clone(),
            disclosure_schema.fingerprint.clone(),
        )
        .map_err(|_| configuration_error("compiled query contract is invalid"))?
        .with_semantic_operation(
            &self.semantic_catalog,
            &self.operation.field_name,
            &document,
        )
        .map_err(|_| configuration_error("semantic query contract is stale"))?;
        let descriptor = AiToolDescriptor::new(
            self.id.as_str(),
            &self.description,
            AiToolOperationKind::Query,
            &document,
            variable_schema,
        )?
        .with_result_projection(projection_fingerprint)
        .with_graphql_contract(contract)
        .with_output_limits(
            self.limits.maximum_result_bytes,
            self.limits.maximum_result_records,
        )
        .with_maximum_classification(maximum_classification(&disclosure_schema.root))
        .with_maturity(ToolMaturity::ReadOnly)
        .with_risk(AiToolRisk::ReadOnly, AiApprovalRule::None)
        .with_idempotent(true);
        Ok(AiCompiledGraphqlQuery {
            capability_fingerprint: self.fingerprint.clone(),
            plan_fingerprint,
            descriptor,
            disclosure_schema,
            variables: Value::Object(variables),
        })
    }
}

/// Complete finite capability set for one active finished schema.
#[derive(Clone, Debug)]
pub struct AiGraphqlQueryCapabilityCatalog {
    semantic_catalog_fingerprint: String,
    finished_schema_fingerprint: String,
    capabilities: BTreeMap<AiToolId, AiGraphqlQueryCapability>,
    fingerprint: String,
}

impl AiGraphqlQueryCapabilityCatalog {
    /// Compiles every exposed public query root into exactly one segment.
    ///
    /// Compilation is all-or-nothing: unsupported or excessive roots fail
    /// readiness instead of being silently omitted. Mutations and
    /// subscriptions are ignored by this query-only contract.
    ///
    /// # Errors
    ///
    /// Returns an error for stale semantics/SDL, ambiguous output graphs,
    /// unbounded collections, unsupported input shapes, collisions, or
    /// capacity exhaustion.
    pub fn compile(
        subgraph_id: &str,
        target_id: GraphqlExecutionTargetId,
        finished_sdl: &str,
        semantic_catalog: &GraphqlSemanticCatalog,
        limits: AiGraphqlQueryCapabilityLimits,
    ) -> Result<Self, AiError> {
        validate_public_token(subgraph_id, "subgraph ID")?;
        semantic_catalog
            .validate()
            .map_err(|_| configuration_error("semantic catalogue is invalid"))?;
        if finished_sdl.len() > limits.maximum_schema_bytes as usize {
            return Err(configuration_error("finished GraphQL SDL is too large"));
        }
        validate_limits(limits)?;
        let schema = FinishedSchema::parse(finished_sdl)?;
        validate_semantic_entities_against_schema(&schema, semantic_catalog)?;
        let schema_query_roots = schema.query_root_fields()?;
        let semantic_query_roots = semantic_catalog
            .operations
            .iter()
            .filter(|operation| operation.kind == GraphqlOperationKind::Query)
            .map(|operation| operation.field_name.as_str())
            .collect::<BTreeSet<_>>();
        if schema_query_roots != semantic_query_roots {
            return Err(configuration_error(
                "finished GraphQL Query roots and semantic operations differ",
            ));
        }
        let finished_schema_fingerprint = hex::encode(Sha256::digest(finished_sdl.as_bytes()));
        let operations = semantic_catalog
            .operations
            .iter()
            .filter(|operation| operation.kind == GraphqlOperationKind::Query)
            .collect::<Vec<_>>();
        if operations.len() > limits.maximum_capabilities as usize {
            return Err(configuration_error(
                "query capability capacity would omit active roots",
            ));
        }
        let mut capabilities = BTreeMap::new();
        for operation in operations {
            let capability = compile_capability(
                subgraph_id,
                target_id.clone(),
                &schema,
                &finished_schema_fingerprint,
                semantic_catalog,
                operation,
                limits,
            )?;
            if capabilities
                .insert(capability.id.clone(), capability)
                .is_some()
            {
                return Err(configuration_error("query capability identity collides"));
            }
        }
        if capabilities.len()
            != semantic_catalog
                .operations
                .iter()
                .filter(|operation| operation.kind == GraphqlOperationKind::Query)
                .count()
        {
            return Err(configuration_error(
                "query capability coverage is incomplete",
            ));
        }
        let fingerprint = sha256_json(&json!({
            "version": AI_GRAPHQL_QUERY_CAPABILITY_VERSION,
            "semantic_catalog_fingerprint": semantic_catalog.fingerprint,
            "finished_schema_fingerprint": finished_schema_fingerprint,
            "capabilities": capabilities.values().map(|capability| json!({
                "id": capability.id.as_str(),
                "fingerprint": capability.fingerprint,
            })).collect::<Vec<_>>(),
        }));
        Ok(Self {
            semantic_catalog_fingerprint: semantic_catalog.fingerprint.clone(),
            finished_schema_fingerprint,
            capabilities,
            fingerprint,
        })
    }

    /// Returns the exact semantic-catalogue fingerprint.
    pub fn semantic_catalog_fingerprint(&self) -> &str {
        &self.semantic_catalog_fingerprint
    }

    /// Returns the exact finished-SDL fingerprint.
    pub fn finished_schema_fingerprint(&self) -> &str {
        &self.finished_schema_fingerprint
    }

    /// Returns the complete deterministic capability-set fingerprint.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Returns every capability ordered by stable ID.
    pub fn capabilities(&self) -> impl Iterator<Item = &AiGraphqlQueryCapability> {
        self.capabilities.values()
    }

    /// Returns one exact capability by stable ID.
    pub fn capability(&self, id: &AiToolId) -> Option<&AiGraphqlQueryCapability> {
        self.capabilities.get(id)
    }
}

/// Exact server-compiled query ready for ordinary authorization and routing.
#[derive(Clone, Debug)]
pub struct AiCompiledGraphqlQuery {
    capability_fingerprint: String,
    plan_fingerprint: String,
    descriptor: AiToolDescriptor,
    disclosure_schema: AiDisclosureSchema,
    variables: Value,
}

impl AiCompiledGraphqlQuery {
    /// Returns the discovery capability fingerprint offered to the provider.
    pub fn capability_fingerprint(&self) -> &str {
        &self.capability_fingerprint
    }

    /// Returns the exact canonical query-plan fingerprint.
    pub fn plan_fingerprint(&self) -> &str {
        &self.plan_fingerprint
    }

    /// Returns the exact dynamic descriptor for current policy evaluation.
    pub fn descriptor(&self) -> &AiToolDescriptor {
        &self.descriptor
    }

    /// Returns the selected disclosure contract.
    pub fn disclosure_schema(&self) -> &AiDisclosureSchema {
        &self.disclosure_schema
    }

    /// Returns the server-compiled GraphQL variables.
    pub fn variables(&self) -> &Value {
        &self.variables
    }

    /// Decomposes the compiled query for a runtime execution boundary.
    pub fn into_parts(self) -> (AiToolDescriptor, AiDisclosureSchema, Value) {
        (self.descriptor, self.disclosure_schema, self.variables)
    }
}

#[derive(Clone, Debug)]
enum QueryOutput {
    Entity {
        entity: String,
        route: Vec<OutputRouteSegment>,
    },
    Scalar {
        classification: DataClassification,
        maximum_items: Option<u32>,
    },
    Aggregate {
        projection: String,
        disclosure: AiDisclosureShape,
    },
}

impl QueryOutput {
    fn requires_root_bound(&self) -> bool {
        match self {
            Self::Entity { route, .. } => route.iter().any(|segment| segment.is_list),
            Self::Aggregate { .. } => true,
            Self::Scalar { maximum_items, .. } => maximum_items.is_some(),
        }
    }
}

#[derive(Clone, Debug)]
struct OutputRouteSegment {
    field_name: String,
    is_list: bool,
}

#[derive(Clone, Debug)]
struct FinishedSchema {
    query_root: String,
    types: BTreeMap<String, SchemaType>,
}

#[derive(Clone, Debug)]
enum SchemaType {
    Scalar,
    Enum(Vec<String>),
    Object(BTreeMap<String, SchemaField>),
    InputObject(BTreeMap<String, SchemaInput>),
    Unsupported,
}

#[derive(Clone, Debug)]
struct SchemaField {
    ty: String,
    arguments: BTreeMap<String, SchemaInput>,
}

#[derive(Clone, Debug)]
struct SchemaInput {
    ty: String,
    has_default: bool,
    description: Option<String>,
}

impl FinishedSchema {
    fn parse(sdl: &str) -> Result<Self, AiError> {
        if sdl.trim().is_empty() {
            return Err(configuration_error("finished GraphQL SDL is empty"));
        }
        let document = parse_schema(sdl)
            .map_err(|_| configuration_error("finished GraphQL SDL is invalid"))?;
        let mut query_root = None;
        let mut types = BTreeMap::new();
        for definition in &document.definitions {
            if let TypeSystemDefinition::Schema(schema) = definition
                && let Some(query) = &schema.node.query
                && query_root.replace(query.node.to_string()).is_some()
            {
                return Err(configuration_error("finished schema repeats query root"));
            }
        }
        for definition in document.definitions {
            let TypeSystemDefinition::Type(definition) = definition else {
                continue;
            };
            let name = definition.node.name.node.to_string();
            use async_graphql_parser::types::TypeKind;
            let converted = match definition.node.kind {
                TypeKind::Scalar => SchemaType::Scalar,
                TypeKind::Enum(value) => SchemaType::Enum(
                    value
                        .values
                        .into_iter()
                        .map(|value| value.node.value.node.to_string())
                        .collect(),
                ),
                TypeKind::Object(value) => SchemaType::Object(
                    value
                        .fields
                        .into_iter()
                        .map(|field| {
                            let arguments = field
                                .node
                                .arguments
                                .into_iter()
                                .map(|argument| {
                                    (
                                        argument.node.name.node.to_string(),
                                        SchemaInput {
                                            ty: argument.node.ty.node.to_string(),
                                            has_default: argument.node.default_value.is_some(),
                                            description: argument
                                                .node
                                                .description
                                                .map(|description| description.node.to_string()),
                                        },
                                    )
                                })
                                .collect();
                            (
                                field.node.name.node.to_string(),
                                SchemaField {
                                    ty: field.node.ty.node.to_string(),
                                    arguments,
                                },
                            )
                        })
                        .collect(),
                ),
                TypeKind::InputObject(value) => SchemaType::InputObject(
                    value
                        .fields
                        .into_iter()
                        .map(|field| {
                            (
                                field.node.name.node.to_string(),
                                SchemaInput {
                                    ty: field.node.ty.node.to_string(),
                                    has_default: field.node.default_value.is_some(),
                                    description: field
                                        .node
                                        .description
                                        .map(|description| description.node.to_string()),
                                },
                            )
                        })
                        .collect(),
                ),
                TypeKind::Interface(_) | TypeKind::Union(_) => SchemaType::Unsupported,
            };
            if types.insert(name, converted).is_some() {
                return Err(configuration_error("finished schema repeats a type"));
            }
        }
        let query_root = query_root
            .or_else(|| types.contains_key("Query").then(|| "Query".to_owned()))
            .ok_or_else(|| configuration_error("finished schema has no query root"))?;
        Ok(Self { query_root, types })
    }

    fn root_query_field(&self, name: &str) -> Result<&SchemaField, AiError> {
        self.object_field(&self.query_root, name)
    }

    fn query_root_fields(&self) -> Result<BTreeSet<&str>, AiError> {
        let Some(SchemaType::Object(fields)) = self.types.get(&self.query_root) else {
            return Err(configuration_error(
                "finished GraphQL Query root is missing",
            ));
        };
        Ok(fields.keys().map(String::as_str).collect())
    }

    fn object_field(&self, type_name: &str, name: &str) -> Result<&SchemaField, AiError> {
        let Some(SchemaType::Object(fields)) = self.types.get(type_name) else {
            return Err(configuration_error("GraphQL object type is missing"));
        };
        fields
            .get(name)
            .ok_or_else(|| configuration_error("semantic field is absent from finished SDL"))
    }
}

fn compile_capability(
    subgraph_id: &str,
    target_id: GraphqlExecutionTargetId,
    schema: &FinishedSchema,
    schema_fingerprint: &str,
    semantic_catalog: &GraphqlSemanticCatalog,
    operation: &GraphqlSemanticOperationDescriptor,
    limits: AiGraphqlQueryCapabilityLimits,
) -> Result<AiGraphqlQueryCapability, AiError> {
    let root_field = schema.root_query_field(&operation.field_name)?;
    validate_operation_against_schema(operation, root_field)?;
    let output = resolve_query_output(schema, semantic_catalog, operation, &root_field.ty, limits)?;
    let argument_schema = build_plan_schema(schema, semantic_catalog, operation, &output, limits)?;
    if serde_json::to_vec(&argument_schema)
        .map(|encoded| encoded.len() > MAXIMUM_PROVIDER_SCHEMA_BYTES)
        .unwrap_or(true)
    {
        return Err(configuration_error(
            "query capability schema exceeds the provider contract",
        ));
    }
    jsonschema::validator_for(&argument_schema)
        .map_err(|_| configuration_error("generated query plan schema is invalid"))?;
    let id = AiToolId::parse(stable_capability_id(subgraph_id, &operation.field_name))?;
    let fingerprint = sha256_json(&json!({
        "version": AI_GRAPHQL_QUERY_CAPABILITY_VERSION,
        "id": id.as_str(),
        "target": target_id.as_str(),
        "schema": schema_fingerprint,
        "semantic_catalog": semantic_catalog.fingerprint,
        "operation": operation.fingerprint,
        "argument_schema": argument_schema,
        "limits": limits,
    }));
    Ok(AiGraphqlQueryCapability {
        id,
        operation: operation.clone(),
        description: operation.description.clone(),
        argument_schema,
        fingerprint,
        schema_fingerprint: schema_fingerprint.to_owned(),
        target_id,
        semantic_catalog: semantic_catalog.clone(),
        schema: schema.clone(),
        limits,
        output,
    })
}

fn validate_operation_against_schema(
    operation: &GraphqlSemanticOperationDescriptor,
    root: &SchemaField,
) -> Result<(), AiError> {
    if operation.kind != GraphqlOperationKind::Query
        || operation.description.is_empty()
        || operation.description.len() > MAXIMUM_DESCRIPTION_BYTES
        || operation
            .description
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(configuration_error("query operation semantics are invalid"));
    }
    if root.arguments.len() != operation.arguments.len()
        || operation.arguments.iter().any(|argument| {
            root.arguments
                .get(&argument.graphql_name)
                .is_none_or(|schema| schema.ty != render_type_ref(&argument.type_ref))
        })
        || root.ty != render_type_ref(&operation.result_type)
    {
        return Err(configuration_error(
            "semantic query operation has drifted from finished SDL",
        ));
    }
    Ok(())
}

fn validate_semantic_entities_against_schema(
    schema: &FinishedSchema,
    catalog: &GraphqlSemanticCatalog,
) -> Result<(), AiError> {
    for entity in &catalog.entities {
        let Some(SchemaType::Object(fields)) = schema.types.get(&entity.entity_name) else {
            return Err(configuration_error(
                "semantic entity is absent from finished SDL",
            ));
        };
        for field in &entity.fields {
            let actual = fields.get(&field.field_name).ok_or_else(|| {
                configuration_error("semantic entity field is absent from finished SDL")
            })?;
            if let Some(relationship) = &field.relationship {
                let (_, route) =
                    resolve_specific_entity_route(schema, &relationship.target, &actual.ty, 8)?;
                let actual_many = route.iter().any(|segment| segment.is_list);
                if actual_many
                    != (relationship.cardinality == GraphqlSemanticRelationshipCardinality::Many)
                {
                    return Err(configuration_error(
                        "semantic relationship cardinality has drifted from finished SDL",
                    ));
                }
                if actual.arguments.len() != relationship.arguments.len()
                    || relationship.arguments.iter().any(|argument| {
                        actual
                            .arguments
                            .get(&argument.graphql_name)
                            .is_none_or(|schema| schema.ty != render_type_ref(&argument.type_ref))
                    })
                {
                    return Err(configuration_error(
                        "semantic relationship arguments have drifted from finished SDL",
                    ));
                }
            } else {
                if actual.ty != render_type_ref(&field.type_ref) {
                    return Err(configuration_error(
                        "semantic entity field type has drifted from finished SDL",
                    ));
                }
                if !actual.arguments.is_empty() {
                    return Err(configuration_error(
                        "semantic scalar field unexpectedly accepts arguments",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn resolve_query_output(
    schema: &FinishedSchema,
    catalog: &GraphqlSemanticCatalog,
    operation: &GraphqlSemanticOperationDescriptor,
    graphql_type: &str,
    limits: AiGraphqlQueryCapabilityLimits,
) -> Result<QueryOutput, AiError> {
    if operation.generated_category == Some(GeneratedGraphqlOperationCategory::Aggregate) {
        return aggregate_output(schema, catalog, graphql_type, limits);
    }
    let named = named_type(graphql_type)?;
    if matches!(
        schema.types.get(named),
        Some(SchemaType::Scalar | SchemaType::Enum(_)) | None
    ) {
        let maximum_items = if list_item_type(graphql_type).is_some() {
            Some(
                semantic_root_list_bound(&operation.result_type)
                    .filter(|maximum| *maximum > 0)
                    .ok_or_else(|| {
                        configuration_error("scalar list query has no positive semantic bound")
                    })?
                    .min(limits.maximum_result_records),
            )
        } else {
            None
        };
        return Ok(QueryOutput::Scalar {
            classification: DataClassification::Internal,
            maximum_items,
        });
    }
    if catalog
        .entities
        .iter()
        .any(|entity| entity.entity_name == named)
    {
        return Ok(QueryOutput::Entity {
            entity: named.to_owned(),
            route: root_list_route(graphql_type),
        });
    }
    let (entity, mut route) = unique_entity_route(schema, catalog, named, limits.maximum_depth)?;
    if list_item_type(graphql_type).is_some() {
        route.insert(
            0,
            OutputRouteSegment {
                field_name: String::new(),
                is_list: true,
            },
        );
    }
    Ok(QueryOutput::Entity { entity, route })
}

fn aggregate_output(
    schema: &FinishedSchema,
    catalog: &GraphqlSemanticCatalog,
    graphql_type: &str,
    limits: AiGraphqlQueryCapabilityLimits,
) -> Result<QueryOutput, AiError> {
    if list_item_type(graphql_type).is_none() {
        return Err(configuration_error(
            "aggregate query result is not bounded-list shaped",
        ));
    }
    let row_type = list_item_type(graphql_type)
        .ok_or_else(|| configuration_error("aggregate query result is not a list"))?;
    let row = named_type(row_type)?;
    let Some(SchemaType::Object(row_fields)) = schema.types.get(row) else {
        return Err(configuration_error("aggregate result row is missing"));
    };
    let groups = find_case_insensitive(row_fields, "groups")?;
    let metrics = find_case_insensitive(row_fields, "metrics")?;
    let value_type = named_type(
        list_item_type(&row_fields[groups].ty)
            .ok_or_else(|| configuration_error("aggregate group values are not a list"))?,
    )?;
    let Some(SchemaType::Object(value_fields)) = schema.types.get(value_type) else {
        return Err(configuration_error("aggregate result value is missing"));
    };
    let selected = ["field", "operator", "kind", "value"]
        .into_iter()
        .map(|name| find_case_insensitive(value_fields, name))
        .collect::<Result<Vec<_>, _>>()?;
    let nested = format!(
        "{{ {} }}",
        selected
            .iter()
            .map(|field| field.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    );
    let projection = format!("{{ {groups} {nested} {metrics} {nested} }}");
    let classification = catalog
        .entities
        .iter()
        .flat_map(|entity| entity.fields.iter())
        .filter(|field| {
            field.export == GraphqlSemanticExport::Exportable
                && field.classification != GraphqlSemanticClassification::Secret
                && (field.groupable || !field.aggregate_operators.is_empty())
        })
        .map(|field| classification(field.classification))
        .max()
        .unwrap_or(DataClassification::Internal);
    let scalar = AiDisclosureShape::scalar(AiDisclosureRule::exportable(classification));
    let value_shape = AiDisclosureShape::object(
        AiDisclosureRule::exportable(classification),
        selected
            .iter()
            .map(|field| ((*field).clone(), scalar.clone())),
    );
    let list = AiDisclosureShape::list(
        AiDisclosureRule::exportable(classification),
        limits.maximum_list_items,
        value_shape,
    );
    Ok(QueryOutput::Aggregate {
        projection,
        disclosure: AiDisclosureShape::list(
            AiDisclosureRule::exportable(classification),
            limits.maximum_result_records,
            AiDisclosureShape::object(
                AiDisclosureRule::exportable(classification),
                [(groups.clone(), list.clone()), (metrics.clone(), list)],
            ),
        ),
    })
}

fn build_plan_schema(
    schema: &FinishedSchema,
    catalog: &GraphqlSemanticCatalog,
    operation: &GraphqlSemanticOperationDescriptor,
    output: &QueryOutput,
    limits: AiGraphqlQueryCapabilityLimits,
) -> Result<Value, AiError> {
    let root = schema.root_query_field(&operation.field_name)?;
    let argument_descriptions = operation
        .arguments
        .iter()
        .map(|argument| {
            (
                argument.graphql_name.as_str(),
                argument.description.as_str(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut ancestry = Vec::new();
    let arguments = input_object_schema(
        schema,
        catalog,
        &root.arguments,
        &argument_descriptions,
        limits,
        0,
        &mut ancestry,
        output.requires_root_bound(),
    )?;
    let mut properties = Map::from_iter([("arguments".to_owned(), arguments)]);
    let mut required = vec![Value::String("arguments".to_owned())];
    match output {
        QueryOutput::Entity { entity, .. } => {
            let selection = selection_schema(schema, catalog, entity, limits, 0, &mut Vec::new())?;
            let selection = selection
                .as_object()
                .ok_or_else(|| configuration_error("selection schema is not an object"))?
                .clone();
            for (name, value) in selection {
                if name == "required" {
                    required.extend(value.as_array().cloned().unwrap_or_default());
                } else if name == "properties" {
                    properties.extend(value.as_object().cloned().unwrap_or_default());
                }
            }
            if output.requires_root_bound() {
                properties.insert(
                    "maximumItems".to_owned(),
                    bounded_integer_schema(1, limits.maximum_result_records.into()),
                );
                required.push(Value::String("maximumItems".to_owned()));
            }
        }
        QueryOutput::Aggregate { .. } => {
            properties.insert(
                "maximumItems".to_owned(),
                bounded_integer_schema(1, limits.maximum_result_records.into()),
            );
            required.push(Value::String("maximumItems".to_owned()));
        }
        QueryOutput::Scalar {
            maximum_items: Some(maximum),
            ..
        } => {
            properties.insert(
                "maximumItems".to_owned(),
                bounded_integer_schema(1, i64::from(*maximum)),
            );
            required.push(Value::String("maximumItems".to_owned()));
        }
        QueryOutput::Scalar {
            maximum_items: None,
            ..
        } => {}
    }
    Ok(json!({
        "$schema": JSON_SCHEMA_2020_12,
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    }))
}

fn selection_schema(
    schema: &FinishedSchema,
    catalog: &GraphqlSemanticCatalog,
    entity_name: &str,
    limits: AiGraphqlQueryCapabilityLimits,
    depth: u8,
    ancestry: &mut Vec<String>,
) -> Result<Value, AiError> {
    if ancestry.iter().any(|name| name == entity_name) {
        return Err(configuration_error(
            "semantic relationship graph contains an unbounded cycle",
        ));
    }
    ancestry.push(entity_name.to_owned());
    let entity = semantic_entity(catalog, entity_name)?;
    let scalar_fields = entity
        .fields
        .iter()
        .filter(|field| selectable_exportable_scalar(field))
        .map(|field| {
            (
                field.field_name.clone(),
                json!({ "type": "boolean", "description": field.description }),
            )
        })
        .collect::<Map<_, _>>();
    let mut relationship_properties = Map::new();
    for field in entity.fields.iter().filter(|field| {
        depth + 1 < limits.maximum_depth
            && field.selectable
            && field.export == GraphqlSemanticExport::Exportable
            && field.classification != GraphqlSemanticClassification::Secret
            && field.relationship.is_some()
    }) {
        let Some(relationship) = field.relationship.as_ref() else {
            return Err(configuration_error(
                "semantic relationship selection is missing relationship metadata",
            ));
        };
        if ancestry.iter().any(|name| name == &relationship.target) {
            continue;
        }
        let actual = schema.object_field(entity_name, &field.field_name)?;
        let argument_descriptions = relationship
            .arguments
            .iter()
            .map(|argument| {
                (
                    argument.graphql_name.as_str(),
                    argument.description.as_str(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let arguments = input_object_schema(
            schema,
            catalog,
            &actual.arguments,
            &argument_descriptions,
            limits,
            0,
            &mut Vec::new(),
            relationship.cardinality == GraphqlSemanticRelationshipCardinality::Many,
        )?;
        let nested = selection_schema(
            schema,
            catalog,
            &relationship.target,
            limits,
            depth + 1,
            ancestry,
        )?;
        let nested_object = nested
            .as_object()
            .ok_or_else(|| configuration_error("nested selection schema is not an object"))?;
        let mut properties = Map::from_iter([("arguments".to_owned(), arguments)]);
        properties.extend(
            nested_object["properties"]
                .as_object()
                .cloned()
                .unwrap_or_default(),
        );
        let mut required = vec![Value::String("arguments".to_owned())];
        required.extend(
            nested_object["required"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        if relationship.cardinality == GraphqlSemanticRelationshipCardinality::Many {
            let maximum = semantic_list_bound(&field.type_ref)?
                .min(limits.maximum_list_items)
                .min(limits.maximum_result_records);
            properties.insert(
                "maximumItems".to_owned(),
                bounded_integer_schema(1, maximum.into()),
            );
            required.push(Value::String("maximumItems".to_owned()));
        }
        relationship_properties.insert(
            field.field_name.clone(),
            json!({
                "type": "object",
                "description": field.description,
                "properties": properties,
                "required": required,
                "additionalProperties": false,
            }),
        );
    }
    ancestry.pop();
    if scalar_fields.is_empty() && relationship_properties.is_empty() {
        return Err(configuration_error(
            "query entity has no exportable selections",
        ));
    }
    let fields = json!({
        "type": "object",
        "description": format!("Selected public fields from {entity_name}."),
        "properties": scalar_fields,
        "required": [],
        "additionalProperties": false,
    });
    Ok(json!({
        "properties": {
            "fields": fields,
            "relationships": {
                "type": "object",
                "properties": relationship_properties,
                "additionalProperties": false,
            }
        },
        "required": ["fields", "relationships"]
    }))
}

#[allow(clippy::too_many_arguments)]
fn input_object_schema(
    schema: &FinishedSchema,
    catalog: &GraphqlSemanticCatalog,
    fields: &BTreeMap<String, SchemaInput>,
    descriptions: &BTreeMap<&str, &str>,
    limits: AiGraphqlQueryCapabilityLimits,
    depth: u8,
    ancestry: &mut Vec<String>,
    exclude_server_bound: bool,
) -> Result<Value, AiError> {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for (name, field) in fields {
        if exclude_server_bound && is_page_argument(name, &field.ty, schema) {
            continue;
        }
        let description = descriptions
            .get(name.as_str())
            .copied()
            .or(field.description.as_deref());
        let value = input_type_schema(
            schema,
            catalog,
            &field.ty,
            description,
            limits,
            depth,
            ancestry,
        )?;
        properties.insert(name.clone(), value);
        if is_non_null(&field.ty) && !field.has_default {
            required.push(Value::String(name.clone()));
        }
    }
    Ok(json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    }))
}

#[allow(clippy::too_many_arguments)]
fn input_type_schema(
    schema: &FinishedSchema,
    catalog: &GraphqlSemanticCatalog,
    graphql_type: &str,
    description: Option<&str>,
    limits: AiGraphqlQueryCapabilityLimits,
    depth: u8,
    ancestry: &mut Vec<String>,
) -> Result<Value, AiError> {
    if depth > limits.maximum_input_depth {
        return Err(configuration_error("query input nesting is too deep"));
    }
    if let Some(item) = list_item_type(graphql_type) {
        let mut value = json!({
            "type": "array",
            "items": input_type_schema(
                schema,
                catalog,
                item,
                None,
                limits,
                depth + 1,
                ancestry,
            )?,
            "maxItems": limits.maximum_list_items,
        });
        insert_description(&mut value, description)?;
        return Ok(nullable_schema(value, !is_non_null(graphql_type)));
    }
    let named = named_type(graphql_type)?;
    let mut value = match schema.types.get(named) {
        Some(SchemaType::Enum(values)) => {
            let values = restricted_enum_values(catalog, named, values);
            if values.is_empty() {
                return Err(configuration_error(
                    "query enum has no exportable semantic values",
                ));
            }
            json!({ "type": "string", "enum": values })
        }
        Some(SchemaType::InputObject(fields)) => {
            if ancestry.iter().any(|name| name == named) {
                return Err(configuration_error(
                    "recursive query inputs are unsupported",
                ));
            }
            ancestry.push(named.to_owned());
            let restricted = restrict_input_fields(catalog, named, fields);
            let value = input_object_schema(
                schema,
                catalog,
                &restricted,
                &BTreeMap::new(),
                limits,
                depth + 1,
                ancestry,
                false,
            )?;
            ancestry.pop();
            value
        }
        Some(SchemaType::Object(_)) | Some(SchemaType::Unsupported) => {
            return Err(configuration_error(
                "query argument type is not a closed input",
            ));
        }
        Some(SchemaType::Scalar) | None => scalar_input_schema(named, limits),
    };
    insert_description(&mut value, description)?;
    Ok(nullable_schema(value, !is_non_null(graphql_type)))
}

fn restrict_input_fields(
    catalog: &GraphqlSemanticCatalog,
    input_name: &str,
    fields: &BTreeMap<String, SchemaInput>,
) -> BTreeMap<String, SchemaInput> {
    let entity = catalog
        .entities
        .iter()
        .filter(|entity| input_name.starts_with(&entity.entity_name))
        .max_by_key(|entity| entity.entity_name.len());
    let Some(entity) = entity else {
        return fields.clone();
    };
    fields
        .iter()
        .filter(|(name, _)| {
            !entity.fields.iter().any(|field| {
                (field.export == GraphqlSemanticExport::NeverExport
                    || field.classification == GraphqlSemanticClassification::Secret)
                    && name
                        .to_ascii_lowercase()
                        .starts_with(&field.field_name.to_ascii_lowercase())
            })
        })
        .map(|(name, field)| (name.clone(), field.clone()))
        .collect()
}

fn restricted_enum_values(
    catalog: &GraphqlSemanticCatalog,
    enum_name: &str,
    values: &[String],
) -> Vec<String> {
    let entity = catalog
        .entities
        .iter()
        .filter(|entity| enum_name.starts_with(&entity.entity_name))
        .max_by_key(|entity| entity.entity_name.len());
    let Some(entity) = entity else {
        return values.to_vec();
    };
    if !enum_name[entity.entity_name.len()..]
        .to_ascii_lowercase()
        .contains("aggregatefield")
    {
        return values.to_vec();
    }
    values
        .iter()
        .filter(|value| {
            entity.fields.iter().any(|field| {
                semantic_name_key(&field.field_name) == semantic_name_key(value)
                    && field.export == GraphqlSemanticExport::Exportable
                    && field.classification != GraphqlSemanticClassification::Secret
                    && (field.groupable || !field.aggregate_operators.is_empty())
            })
        })
        .cloned()
        .collect()
}

fn semantic_name_key(value: &str) -> String {
    value
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric())
        .map(|byte| byte.to_ascii_lowercase() as char)
        .collect()
}

struct CompileContext<'a> {
    capability: &'a AiGraphqlQueryCapability,
    variables: Map<String, Value>,
    variable_definitions: Vec<String>,
    variable_schemas: Map<String, Value>,
    selected_fields: usize,
    arguments: usize,
}

impl<'a> CompileContext<'a> {
    fn new(capability: &'a AiGraphqlQueryCapability) -> Self {
        Self {
            capability,
            variables: Map::new(),
            variable_definitions: Vec::new(),
            variable_schemas: Map::new(),
            selected_fields: 0,
            arguments: 0,
        }
    }

    fn compile_arguments(
        &mut self,
        schema_fields: &BTreeMap<String, SchemaInput>,
        semantic_fields: &[graphql_orm_operation_catalog::GraphqlSemanticArgumentDescriptor],
        supplied: &Map<String, Value>,
        maximum_items: Option<u32>,
        require_bound: bool,
        path: &str,
    ) -> Result<Vec<String>, AiError> {
        if require_bound != maximum_items.is_some() {
            return Err(input_error(
                "query collection bound is missing or unexpected",
            ));
        }
        if maximum_items.is_some_and(|maximum| {
            maximum == 0
                || maximum > self.capability.limits.maximum_list_items
                || maximum > self.capability.limits.maximum_result_records
        }) {
            return Err(input_error(
                "query collection bound exceeds deployment limits",
            ));
        }
        let semantic = semantic_fields
            .iter()
            .map(|field| (field.graphql_name.as_str(), field))
            .collect::<BTreeMap<_, _>>();
        let mut values = supplied.clone();
        if let Some(maximum) = maximum_items {
            inject_page_limit(&self.capability.schema, schema_fields, &mut values, maximum)?;
        }
        if values.keys().any(|name| !schema_fields.contains_key(name)) {
            return Err(input_error("query plan contains an unknown argument"));
        }
        let mut rendered = Vec::new();
        for (name, schema) in schema_fields {
            let Some(value) = values.remove(name) else {
                if is_non_null(&schema.ty) && !schema.has_default {
                    return Err(input_error("query plan omits a required argument"));
                }
                continue;
            };
            let description = semantic
                .get(name.as_str())
                .map(|field| field.description.as_str())
                .or(schema.description.as_deref());
            let input_schema = input_type_schema(
                &self.capability.schema,
                &self.capability.semantic_catalog,
                &schema.ty,
                description,
                self.capability.limits,
                0,
                &mut Vec::new(),
            )?;
            let validator = jsonschema::validator_for(&input_schema)
                .map_err(|_| configuration_error("compiled variable schema is invalid"))?;
            if !validator.is_valid(&value) {
                return Err(input_error(
                    "query argument does not match its GraphQL type",
                ));
            }
            let variable = format!("v{}", self.variables.len());
            self.variables.insert(variable.clone(), value);
            self.variable_definitions
                .push(format!("${variable}: {}", schema.ty));
            self.variable_schemas.insert(variable.clone(), input_schema);
            rendered.push(format!("{name}: ${variable}"));
            self.arguments += 1;
            let _ = path;
        }
        Ok(rendered)
    }

    fn compile_entity_selection(
        &mut self,
        entity_name: &str,
        plan: EntityPlanRef<'_>,
        ancestry: &mut Vec<String>,
        depth: u8,
    ) -> Result<(String, AiDisclosureShape), AiError> {
        if depth >= self.capability.limits.maximum_depth {
            return Err(input_error("query relationship depth exceeds its bound"));
        }
        let entity = semantic_entity(&self.capability.semantic_catalog, entity_name)?;
        if plan.fields.values().any(|selected| !selected) {
            return Err(input_error("query selection flags must be true"));
        }
        let supplied_fields = plan.fields.keys().collect::<BTreeSet<_>>();
        let mut rendered = Vec::new();
        let mut disclosure_fields = BTreeMap::new();
        for field in &entity.fields {
            if supplied_fields.contains(&field.field_name) {
                if !selectable_exportable_scalar(field) {
                    return Err(input_error("query selected a hidden or non-scalar field"));
                }
                self.capability
                    .schema
                    .object_field(entity_name, &field.field_name)?;
                rendered.push(field.field_name.clone());
                disclosure_fields.insert(
                    field.field_name.clone(),
                    AiDisclosureShape::scalar(AiDisclosureRule::exportable(classification(
                        field.classification,
                    ))),
                );
                self.selected_fields += 1;
            }
        }
        if supplied_fields.iter().any(|name| {
            !entity
                .fields
                .iter()
                .any(|field| field.field_name == name.as_str())
        }) {
            return Err(input_error("query selected an unknown field"));
        }
        for field in &entity.fields {
            let Some(relationship_plan) = plan.relationships.get(&field.field_name) else {
                continue;
            };
            let relationship = field.relationship.as_ref().ok_or_else(|| {
                input_error("query relationship selection targets a scalar field")
            })?;
            if !field.selectable
                || field.export == GraphqlSemanticExport::NeverExport
                || field.classification == GraphqlSemanticClassification::Secret
                || ancestry.iter().any(|name| name == &relationship.target)
            {
                return Err(input_error("query relationship is hidden or cyclic"));
            }
            let actual = self
                .capability
                .schema
                .object_field(entity_name, &field.field_name)?;
            let arguments = self.compile_arguments(
                &actual.arguments,
                &relationship.arguments,
                &relationship_plan.arguments,
                relationship_plan.maximum_items,
                relationship.cardinality == GraphqlSemanticRelationshipCardinality::Many,
                &format!("{entity_name}.{}", field.field_name),
            )?;
            ancestry.push(relationship.target.clone());
            let nested_plan = EntityPlanRef {
                fields: &relationship_plan.fields,
                relationships: &relationship_plan.relationships,
            };
            let (nested, nested_disclosure) = self.compile_entity_selection(
                &relationship.target,
                nested_plan,
                ancestry,
                depth + 1,
            )?;
            ancestry.pop();
            let (_, route) = resolve_specific_entity_route(
                &self.capability.schema,
                &relationship.target,
                &actual.ty,
                self.capability.limits.maximum_depth,
            )?;
            let maximum = relationship_plan.maximum_items.unwrap_or(1);
            let (wrapped, wrapped_disclosure) =
                wrap_projection_route(&route, nested, nested_disclosure, maximum);
            let argument_clause = if arguments.is_empty() {
                String::new()
            } else {
                format!("({})", arguments.join(", "))
            };
            rendered.push(format!(
                "{}{} {}",
                field.field_name, argument_clause, wrapped
            ));
            disclosure_fields.insert(field.field_name.clone(), wrapped_disclosure);
            self.selected_fields += 1;
        }
        if plan.relationships.keys().any(|name| {
            !entity
                .fields
                .iter()
                .any(|field| field.field_name == name.as_str())
        }) {
            return Err(input_error("query selected an unknown relationship"));
        }
        if rendered.is_empty() {
            return Err(input_error("query selection must not be empty"));
        }
        let classification = disclosure_fields
            .values()
            .map(maximum_classification)
            .max()
            .unwrap_or(classification(entity.default_classification));
        Ok((
            format!("{{ {} }}", rendered.join(" ")),
            AiDisclosureShape::object(
                AiDisclosureRule::exportable(classification),
                disclosure_fields,
            ),
        ))
    }

    fn ensure_totals(&self) -> Result<(), AiError> {
        if self.selected_fields > self.capability.limits.maximum_selected_fields as usize
            || self.arguments > self.capability.limits.maximum_arguments as usize
        {
            return Err(input_error(
                "query plan exceeds deployment complexity limits",
            ));
        }
        Ok(())
    }

    fn variable_schema(&self) -> Result<Value, AiError> {
        let value = json!({
            "$schema": JSON_SCHEMA_2020_12,
            "type": "object",
            "properties": self.variable_schemas,
            "required": self.variable_schemas.keys().cloned().collect::<Vec<_>>(),
            "additionalProperties": false,
        });
        jsonschema::validator_for(&value)
            .map_err(|_| configuration_error("compiled variable contract is invalid"))?;
        Ok(value)
    }
}

struct EntityPlanRef<'a> {
    fields: &'a BTreeMap<String, bool>,
    relationships: &'a BTreeMap<String, AiGraphqlRelationshipQueryPlan>,
}

fn unique_entity_route(
    schema: &FinishedSchema,
    catalog: &GraphqlSemanticCatalog,
    start: &str,
    maximum_depth: u8,
) -> Result<(String, Vec<OutputRouteSegment>), AiError> {
    let mut queue = VecDeque::from([(start.to_owned(), Vec::new())]);
    let mut results = Vec::new();
    while let Some((type_name, route)) = queue.pop_front() {
        if route.len() > maximum_depth as usize {
            continue;
        }
        if catalog
            .entities
            .iter()
            .any(|entity| entity.entity_name == type_name)
        {
            results.push((type_name, route));
            continue;
        }
        let Some(SchemaType::Object(fields)) = schema.types.get(&type_name) else {
            continue;
        };
        for (name, field) in fields {
            let Ok(named) = named_type(&field.ty) else {
                continue;
            };
            if matches!(schema.types.get(named), Some(SchemaType::Object(_))) {
                let mut nested = route.clone();
                nested.push(OutputRouteSegment {
                    field_name: name.clone(),
                    is_list: list_item_type(&field.ty).is_some(),
                });
                queue.push_back((named.to_owned(), nested));
            }
        }
    }
    results.sort_by_key(|result| result.1.len());
    let Some(first) = results.first().cloned() else {
        return Err(configuration_error(
            "query result does not reach a semantic entity",
        ));
    };
    if results
        .iter()
        .skip(1)
        .any(|candidate| candidate.1.len() == first.1.len())
    {
        return Err(configuration_error(
            "query result reaches ambiguous semantic entities",
        ));
    }
    Ok(first)
}

fn resolve_specific_entity_route(
    schema: &FinishedSchema,
    target: &str,
    graphql_type: &str,
    maximum_depth: u8,
) -> Result<(String, Vec<OutputRouteSegment>), AiError> {
    let named = named_type(graphql_type)?;
    if named == target {
        return Ok((target.to_owned(), root_list_route(graphql_type)));
    }
    let mut queue = VecDeque::from([(named.to_owned(), Vec::new())]);
    let mut results = Vec::new();
    while let Some((type_name, route)) = queue.pop_front() {
        if route.len() > maximum_depth as usize {
            continue;
        }
        if type_name == target {
            results.push(route);
            continue;
        }
        let Some(SchemaType::Object(fields)) = schema.types.get(&type_name) else {
            continue;
        };
        for (name, field) in fields {
            let nested_type = named_type(&field.ty)?;
            if matches!(schema.types.get(nested_type), Some(SchemaType::Object(_))) {
                let mut nested = route.clone();
                nested.push(OutputRouteSegment {
                    field_name: name.clone(),
                    is_list: list_item_type(&field.ty).is_some(),
                });
                queue.push_back((nested_type.to_owned(), nested));
            }
        }
    }
    let mut route = results
        .into_iter()
        .min_by_key(Vec::len)
        .ok_or_else(|| configuration_error("relationship result target is unreachable"))?;
    if list_item_type(graphql_type).is_some() {
        route.insert(
            0,
            OutputRouteSegment {
                field_name: String::new(),
                is_list: true,
            },
        );
    }
    Ok((target.to_owned(), route))
}

fn wrap_projection_route(
    route: &[OutputRouteSegment],
    mut projection: String,
    mut disclosure: AiDisclosureShape,
    maximum_items: u32,
) -> (String, AiDisclosureShape) {
    for segment in route.iter().rev() {
        if segment.is_list {
            disclosure = AiDisclosureShape::list(
                AiDisclosureRule::exportable(maximum_classification(&disclosure)),
                maximum_items,
                disclosure,
            );
        }
        if !segment.field_name.is_empty() {
            projection = format!("{{ {} {} }}", segment.field_name, projection);
            disclosure = AiDisclosureShape::object(
                AiDisclosureRule::exportable(maximum_classification(&disclosure)),
                [(segment.field_name.clone(), disclosure)],
            );
        }
    }
    (projection, disclosure)
}

fn root_list_route(graphql_type: &str) -> Vec<OutputRouteSegment> {
    list_item_type(graphql_type)
        .is_some()
        .then_some(OutputRouteSegment {
            field_name: String::new(),
            is_list: true,
        })
        .into_iter()
        .collect()
}

fn semantic_entity<'a>(
    catalog: &'a GraphqlSemanticCatalog,
    name: &str,
) -> Result<&'a graphql_orm_operation_catalog::GraphqlEntitySemanticMetadata, AiError> {
    catalog
        .entities
        .iter()
        .find(|entity| entity.entity_name == name)
        .ok_or_else(|| configuration_error("semantic result entity is missing"))
}

fn selectable_exportable_scalar(field: &GraphqlSemanticFieldMetadata) -> bool {
    field.selectable
        && field.relationship.is_none()
        && field.export == GraphqlSemanticExport::Exportable
        && field.classification != GraphqlSemanticClassification::Secret
}

fn semantic_list_bound(type_ref: &GraphqlSemanticTypeRef) -> Result<u32, AiError> {
    match type_ref {
        GraphqlSemanticTypeRef::List {
            maximum_items: Some(maximum),
            ..
        } if *maximum > 0 => Ok(*maximum),
        _ => Err(configuration_error(
            "semantic collection relationship has no positive bound",
        )),
    }
}

fn semantic_root_list_bound(type_ref: &GraphqlSemanticTypeRef) -> Option<u32> {
    match type_ref {
        GraphqlSemanticTypeRef::List { maximum_items, .. } => *maximum_items,
        GraphqlSemanticTypeRef::Named { .. } => None,
    }
}

fn inject_page_limit(
    schema: &FinishedSchema,
    schema_fields: &BTreeMap<String, SchemaInput>,
    values: &mut Map<String, Value>,
    maximum: u32,
) -> Result<(), AiError> {
    let candidates = schema_fields
        .iter()
        .filter(|(name, field)| is_page_argument(name, &field.ty, schema))
        .collect::<Vec<_>>();
    let [(name, field)] = candidates.as_slice() else {
        return Err(configuration_error(
            "collection query has no unique server-bounded page argument",
        ));
    };
    let input_name = named_type(&field.ty)?;
    if input_name == "Int" {
        if values
            .insert((*name).clone(), Value::from(maximum))
            .is_some()
        {
            return Err(input_error("result size is server-owned"));
        }
        return Ok(());
    }
    let Some(SchemaType::InputObject(input_fields)) = schema.types.get(input_name) else {
        return Err(configuration_error("page argument is not an input object"));
    };
    let limit = input_fields
        .keys()
        .find(|candidate| {
            candidate.eq_ignore_ascii_case("limit") || candidate.eq_ignore_ascii_case("first")
        })
        .ok_or_else(|| configuration_error("page input has no bounded size field"))?;
    let entry = values
        .entry((*name).clone())
        .or_insert_with(|| Value::Object(Map::new()));
    let object = entry
        .as_object_mut()
        .ok_or_else(|| input_error("page argument has the wrong shape"))?;
    if object.contains_key(limit) {
        return Err(input_error("page size is server-owned"));
    }
    object.insert(limit.clone(), Value::from(maximum));
    Ok(())
}

fn is_page_argument(name: &str, graphql_type: &str, schema: &FinishedSchema) -> bool {
    if !(name.eq_ignore_ascii_case("page")
        || name.eq_ignore_ascii_case("pagination")
        || name.eq_ignore_ascii_case("window")
        || name.eq_ignore_ascii_case("limit")
        || name.eq_ignore_ascii_case("first")
        || name.eq_ignore_ascii_case("groupLimit"))
    {
        return false;
    }
    let Ok(named) = named_type(graphql_type) else {
        return false;
    };
    matches!(schema.types.get(named), Some(SchemaType::InputObject(_))) || named == "Int"
}

fn scalar_input_schema(name: &str, limits: AiGraphqlQueryCapabilityLimits) -> Value {
    match name {
        "Boolean" => json!({ "type": "boolean" }),
        "Int" => json!({
            "type": "integer",
            "minimum": i32::MIN,
            "maximum": i32::MAX,
        }),
        "Float" => json!({ "type": "number" }),
        _ => json!({
            "type": "string",
            "maxLength": limits.maximum_string_bytes,
        }),
    }
}

fn nullable_schema(schema: Value, nullable: bool) -> Value {
    if !nullable {
        return schema;
    }
    json!({ "anyOf": [schema, { "type": "null" }] })
}

fn bounded_integer_schema(minimum: i64, maximum: i64) -> Value {
    json!({ "type": "integer", "minimum": minimum, "maximum": maximum })
}

fn insert_description(value: &mut Value, description: Option<&str>) -> Result<(), AiError> {
    let Some(description) = description else {
        return Ok(());
    };
    if description.is_empty()
        || description.len() > MAXIMUM_DESCRIPTION_BYTES
        || description
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(configuration_error("query input description is invalid"));
    }
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "description".to_owned(),
            Value::String(description.to_owned()),
        );
    }
    Ok(())
}

fn render_type_ref(value: &GraphqlSemanticTypeRef) -> String {
    match value {
        GraphqlSemanticTypeRef::Named { name, nullable, .. } => {
            format!("{name}{}", if *nullable { "" } else { "!" })
        }
        GraphqlSemanticTypeRef::List { nullable, item, .. } => format!(
            "[{}]{}",
            render_type_ref(item),
            if *nullable { "" } else { "!" }
        ),
    }
}

fn named_type(graphql_type: &str) -> Result<&str, AiError> {
    let mut value = graphql_type.trim();
    if let Some(stripped) = value.strip_suffix('!') {
        value = stripped.trim();
    }
    while let Some(item) = value
        .strip_prefix('[')
        .and_then(|item| item.strip_suffix(']'))
    {
        value = item.trim();
        if let Some(stripped) = value.strip_suffix('!') {
            value = stripped.trim();
        }
    }
    validate_graphql_name(value)?;
    Ok(value)
}

fn list_item_type(graphql_type: &str) -> Option<&str> {
    let mut value = graphql_type.trim();
    if let Some(stripped) = value.strip_suffix('!') {
        value = stripped.trim();
    }
    value.strip_prefix('[')?.strip_suffix(']').map(str::trim)
}

fn is_non_null(graphql_type: &str) -> bool {
    graphql_type.trim().ends_with('!')
}

fn validate_graphql_name(value: &str) -> Result<(), AiError> {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return Err(configuration_error("GraphQL name is empty"));
    };
    if !(first == b'_' || first.is_ascii_alphabetic())
        || !bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
    {
        return Err(configuration_error("GraphQL name is invalid"));
    }
    Ok(())
}

fn validate_public_token(value: &str, label: &str) -> Result<(), AiError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(configuration_error(format!("{label} is invalid")));
    }
    Ok(())
}

fn validate_limits(limits: AiGraphqlQueryCapabilityLimits) -> Result<(), AiError> {
    AiGraphqlQueryCapabilityLimits::new(
        limits.maximum_depth,
        limits.maximum_selected_fields,
        limits.maximum_arguments,
        limits.maximum_input_depth,
        limits.maximum_string_bytes,
        limits.maximum_list_items,
        limits.maximum_result_records,
        limits.maximum_result_bytes,
        limits.maximum_capabilities,
        limits.maximum_schema_bytes,
    )
    .map(|_| ())
}

fn stable_capability_id(subgraph_id: &str, field_name: &str) -> String {
    let identity = format!("{subgraph_id}\0query\0{field_name}\0automatic-v1");
    let hash = hex::encode(Sha256::digest(identity.as_bytes()));
    format!(
        "{}.query.{}.auto-{}",
        subgraph_id.to_ascii_lowercase(),
        field_name.to_ascii_lowercase(),
        &hash[..16]
    )
}

fn find_case_insensitive<'a, T>(
    values: &'a BTreeMap<String, T>,
    expected: &str,
) -> Result<&'a String, AiError> {
    let mut matches = values
        .keys()
        .filter(|name| name.eq_ignore_ascii_case(expected));
    let value = matches
        .next()
        .ok_or_else(|| configuration_error("required generated result field is missing"))?;
    if matches.next().is_some() {
        return Err(configuration_error("generated result field is ambiguous"));
    }
    Ok(value)
}

fn classification(value: GraphqlSemanticClassification) -> DataClassification {
    match value {
        GraphqlSemanticClassification::Public => DataClassification::Public,
        GraphqlSemanticClassification::Internal => DataClassification::Internal,
        GraphqlSemanticClassification::Confidential => DataClassification::Confidential,
        GraphqlSemanticClassification::Restricted => DataClassification::Restricted,
        GraphqlSemanticClassification::Secret => DataClassification::Secret,
        _ => DataClassification::Secret,
    }
}

fn maximum_classification(shape: &AiDisclosureShape) -> DataClassification {
    match shape {
        AiDisclosureShape::Scalar { rule } => rule.classification,
        AiDisclosureShape::Object { rule, fields } => fields
            .values()
            .map(maximum_classification)
            .chain([rule.classification])
            .max()
            .unwrap_or(rule.classification),
        AiDisclosureShape::List { rule, item, .. } => {
            rule.classification.max(maximum_classification(item))
        }
    }
}

fn sha256_json(value: &Value) -> String {
    hex::encode(Sha256::digest(canonical_json_bytes(value)))
}

fn configuration_error(message: impl Into<String>) -> AiError {
    AiError::InvalidConfiguration(message.into())
}

fn input_error(message: impl Into<String>) -> AiError {
    AiError::InvalidInput(message.into())
}

#[cfg(test)]
mod tests {
    use graphql_orm_operation_catalog::{
        GeneratedGraphqlOperationDescriptor, GraphqlEntitySemanticMetadata,
        GraphqlOperationArgumentDescriptor, GraphqlOperationCatalog,
        GraphqlSemanticArgumentDescriptor, GraphqlSemanticFieldMetadata,
        GraphqlSemanticOperationDescriptor, GraphqlSemanticRelationshipDescriptor,
        GraphqlSemanticTypeKind,
    };

    use super::*;

    const SDL: &str = r#"
        schema { query: Query }
        type Query {
          ReadParent(id: ID!): Parent!
          ParentAggregate(groupLimit: Int!, metrics: [AggregateMetric!]!): [AggregateRow!]!
        }
        type Parent {
          id: String!
          name: String!
          credential: String!
          children(page: PageInput): ChildConnection!
        }
        type Child { id: String!, label: String! }
        type ChildConnection { edges: [ChildEdge!]!, pageInfo: PageInfo! }
        type ChildEdge { node: Child!, cursor: String! }
        type PageInfo { totalCount: Int!, hasNextPage: Boolean! }
        input PageInput { limit: Int, offset: Int }
        enum AggregateOperator { COUNT SUM }
        input AggregateMetric { operator: AggregateOperator!, field: String }
        type AggregateRow { groups: [AggregateValue!]!, metrics: [AggregateValue!]! }
        type AggregateValue { field: String, operator: String, kind: String!, value: String }
    "#;

    fn named(name: &str, nullable: bool) -> GraphqlSemanticTypeRef {
        GraphqlSemanticTypeRef::named(name, GraphqlSemanticTypeKind::Scalar, nullable)
    }

    fn scalar_field(
        name: &str,
        classification: GraphqlSemanticClassification,
        export: GraphqlSemanticExport,
    ) -> GraphqlSemanticFieldMetadata {
        GraphqlSemanticFieldMetadata {
            field_name: name.to_owned(),
            description: format!("Public {name} value."),
            type_ref: named("String", false),
            selectable: true,
            filter_operators: Vec::new(),
            sortable: false,
            groupable: false,
            aggregate_operators: Vec::new(),
            aggregate_value_kind: None,
            relationship: None,
            classification,
            export,
            has_field_policy: false,
        }
    }

    fn semantic_catalog() -> GraphqlSemanticCatalog {
        let child = GraphqlEntitySemanticMetadata {
            entity_name: "Child".to_owned(),
            description: "A bounded child record.".to_owned(),
            default_classification: GraphqlSemanticClassification::Internal,
            fields: vec![
                scalar_field(
                    "id",
                    GraphqlSemanticClassification::Internal,
                    GraphqlSemanticExport::Exportable,
                ),
                scalar_field(
                    "label",
                    GraphqlSemanticClassification::Confidential,
                    GraphqlSemanticExport::Exportable,
                ),
            ]
            .into_boxed_slice(),
        };
        let parent = GraphqlEntitySemanticMetadata {
            entity_name: "Parent".to_owned(),
            description: "A public parent record.".to_owned(),
            default_classification: GraphqlSemanticClassification::Internal,
            fields: vec![
                scalar_field(
                    "id",
                    GraphqlSemanticClassification::Internal,
                    GraphqlSemanticExport::Exportable,
                ),
                scalar_field(
                    "name",
                    GraphqlSemanticClassification::Confidential,
                    GraphqlSemanticExport::Exportable,
                ),
                scalar_field(
                    "credential",
                    GraphqlSemanticClassification::Secret,
                    GraphqlSemanticExport::NeverExport,
                ),
                GraphqlSemanticFieldMetadata {
                    field_name: "children".to_owned(),
                    description: "Bounded related child records.".to_owned(),
                    type_ref: GraphqlSemanticTypeRef::list(
                        false,
                        Some(10),
                        GraphqlSemanticTypeRef::named(
                            "Child",
                            GraphqlSemanticTypeKind::Object,
                            false,
                        ),
                    ),
                    selectable: true,
                    filter_operators: Vec::new(),
                    sortable: false,
                    groupable: false,
                    aggregate_operators: Vec::new(),
                    aggregate_value_kind: None,
                    relationship: Some(GraphqlSemanticRelationshipDescriptor {
                        target: "Child".to_owned(),
                        cardinality: GraphqlSemanticRelationshipCardinality::Many,
                        arguments: vec![GraphqlSemanticArgumentDescriptor {
                            graphql_name: "page".to_owned(),
                            description: "Bounded relationship page.".to_owned(),
                            type_ref: GraphqlSemanticTypeRef::named(
                                "PageInput",
                                GraphqlSemanticTypeKind::Object,
                                true,
                            ),
                        }],
                    }),
                    classification: GraphqlSemanticClassification::Confidential,
                    export: GraphqlSemanticExport::Exportable,
                    has_field_policy: true,
                },
            ]
            .into_boxed_slice(),
        };
        let read = GraphqlSemanticOperationDescriptor::custom(
            GraphqlOperationKind::Query,
            "ReadParent",
            "Read one reviewed parent record.",
            vec![GraphqlSemanticArgumentDescriptor {
                graphql_name: "id".to_owned(),
                description: "Exact public parent identity.".to_owned(),
                type_ref: named("ID", false),
            }],
            GraphqlSemanticTypeRef::named("Parent", GraphqlSemanticTypeKind::Object, false),
            true,
        )
        .expect("custom query is valid");
        GraphqlSemanticCatalog::compose_with_custom(
            [parent, child],
            &GraphqlOperationCatalog::compose(std::iter::empty()),
            [read],
        )
        .expect("semantic catalogue validates")
    }

    fn query_sdl() -> String {
        SDL.replace(
            "          ParentAggregate(groupLimit: Int!, metrics: [AggregateMetric!]!): [AggregateRow!]!\n",
            "",
        )
    }

    fn catalog() -> AiGraphqlQueryCapabilityCatalog {
        let sdl = query_sdl();
        AiGraphqlQueryCapabilityCatalog::compile(
            "inventory",
            GraphqlExecutionTargetId::parse("inventory.graphql").expect("target"),
            &sdl,
            &semantic_catalog(),
            AiGraphqlQueryCapabilityLimits::default(),
        )
        .expect("automatic query capabilities compile")
    }

    fn aggregate_semantic_catalog() -> GraphqlSemanticCatalog {
        let base = semantic_catalog();
        let generated = GeneratedGraphqlOperationDescriptor::generated(
            "tests::Parent",
            "Parent",
            "not_transportable",
            "sqlite",
            GraphqlOperationKind::Query,
            GeneratedGraphqlOperationCategory::Aggregate,
            "ParentAggregate",
            vec![
                GraphqlOperationArgumentDescriptor::generated_with_description(
                    "groupLimit",
                    "Positive maximum result groups.",
                    "i32",
                    "Int!",
                ),
                GraphqlOperationArgumentDescriptor::generated_with_description(
                    "metrics",
                    "Reviewed aggregate metric expressions.",
                    "Vec<AggregateMetric>",
                    "[AggregateMetric!]!",
                ),
            ],
            "Vec<AggregateRow>",
            "[AggregateRow!]!",
            "aggregate-test-v1",
        );
        let generated: &'static [GeneratedGraphqlOperationDescriptor] =
            Box::leak(vec![generated].into_boxed_slice());
        GraphqlSemanticCatalog::compose_with_custom(
            base.entities,
            &GraphqlOperationCatalog::compose([(generated, false, false)]),
            base.operations.into_iter().filter(|operation| {
                operation.source
                    == graphql_orm_operation_catalog::GraphqlSemanticOperationSource::Custom
            }),
        )
        .expect("aggregate semantic catalogue")
    }

    #[test]
    fn compiles_exact_nested_query_and_disclosure_from_semantics() {
        let catalog = catalog();
        assert_eq!(catalog.capabilities().count(), 1);
        let capability = catalog.capabilities().next().expect("query capability");
        let encoded_schema = serde_json::to_string(capability.argument_schema()).expect("schema");
        assert!(encoded_schema.contains("Bounded related child records."));
        assert!(!encoded_schema.contains("credential"));

        let compiled = capability
            .compile(json!({
                "arguments": { "id": "parent-1" },
                "fields": { "id": true, "name": true },
                "relationships": {
                    "children": {
                        "arguments": {},
                        "fields": { "id": true, "label": true },
                        "relationships": {},
                        "maximumItems": 2
                    }
                }
            }))
            .expect("closed nested query compiles");
        assert!(
            compiled
                .descriptor()
                .document
                .contains("ReadParent(id: $v0)")
        );
        assert!(
            compiled
                .descriptor()
                .document
                .contains("children(page: $v1) { edges { node { id label } } }")
        );
        assert_eq!(compiled.variables()["v0"], json!("parent-1"));
        assert_eq!(compiled.variables()["v1"]["limit"], json!(2));
        assert!(
            compiled
                .descriptor()
                .graphql_contract
                .as_ref()
                .expect("contract")
                .semantic_operation()
                .is_some()
        );
        assert_eq!(
            compiled.descriptor().maximum_classification,
            DataClassification::Confidential
        );
        compiled
            .disclosure_schema()
            .evaluate(&json!({
                "ReadParent": {
                    "id": "parent-1",
                    "name": "Reviewed",
                    "children": {
                        "edges": [
                            { "node": { "id": "child-1", "label": "First" } },
                            { "node": { "id": "child-2", "label": "Second" } }
                        ]
                    }
                }
            }))
            .expect("exact selected result discloses");
    }

    #[test]
    fn hidden_unknown_unbounded_and_tampered_plans_fail_closed() {
        let catalog = catalog();
        let capability = catalog.capabilities().next().expect("query capability");
        for plan in [
            json!({
                "arguments": { "id": "parent-1" },
                "fields": { "credential": true },
                "relationships": {}
            }),
            json!({
                "arguments": { "id": "parent-1" },
                "fields": { "unknown": true },
                "relationships": {}
            }),
            json!({
                "arguments": { "id": "parent-1" },
                "fields": { "id": true },
                "relationships": {
                    "children": {
                        "arguments": {}, "fields": { "id": true }, "relationships": {}
                    }
                }
            }),
            json!({
                "arguments": { "id": "parent-1", "extra": true },
                "fields": { "id": true },
                "relationships": {}
            }),
        ] {
            assert!(matches!(
                capability.compile(plan),
                Err(AiError::InvalidInput(_))
            ));
        }
    }

    #[test]
    fn identity_and_compilation_are_deterministic_and_schema_bound() {
        let first = catalog();
        let second = catalog();
        assert_eq!(first.fingerprint(), second.fingerprint());
        let first_capability = first.capabilities().next().expect("first capability");
        let second_capability = second.capabilities().next().expect("second capability");
        assert_eq!(first_capability.id(), second_capability.id());
        assert_eq!(
            first_capability.fingerprint(),
            second_capability.fingerprint()
        );
        let plan = json!({
            "arguments": { "id": "parent-1" },
            "fields": { "id": true },
            "relationships": {}
        });
        let first_query = first_capability.compile(plan.clone()).expect("first query");
        let second_query = second_capability.compile(plan).expect("second query");
        assert_eq!(
            first_query.plan_fingerprint(),
            second_query.plan_fingerprint()
        );
        assert_eq!(first_query.descriptor(), second_query.descriptor());
    }

    #[test]
    fn capability_capacity_fails_instead_of_omitting_roots() {
        let limits = AiGraphqlQueryCapabilityLimits::new(
            4,
            64,
            64,
            8,
            4096,
            100,
            100,
            64 * 1024,
            1,
            4 * 1024 * 1024,
        )
        .expect("limits");
        let mut semantic = semantic_catalog();
        let duplicate = GraphqlSemanticOperationDescriptor::custom(
            GraphqlOperationKind::Query,
            "AnotherParent",
            "Read another reviewed parent record.",
            Vec::new(),
            GraphqlSemanticTypeRef::named("Parent", GraphqlSemanticTypeKind::Object, false),
            true,
        )
        .expect("operation");
        semantic = GraphqlSemanticCatalog::compose_with_custom(
            semantic.entities.clone(),
            &GraphqlOperationCatalog::compose(std::iter::empty()),
            semantic.operations.into_iter().chain([duplicate]),
        )
        .expect("catalog");
        let sdl = query_sdl().replace(
            "ReadParent(id: ID!): Parent!",
            "ReadParent(id: ID!): Parent!\nAnotherParent: Parent!",
        );
        assert!(matches!(
            AiGraphqlQueryCapabilityCatalog::compile(
                "inventory",
                GraphqlExecutionTargetId::parse("inventory.graphql").expect("target"),
                &sdl,
                &semantic,
                limits,
            ),
            Err(AiError::InvalidConfiguration(_))
        ));
    }

    #[test]
    fn finished_schema_custom_query_without_semantics_fails_readiness() {
        let sdl = query_sdl().replace(
            "ReadParent(id: ID!): Parent!",
            "ReadParent(id: ID!): Parent!\nUndeclaredCustomRoot: String!",
        );
        assert!(matches!(
            AiGraphqlQueryCapabilityCatalog::compile(
                "inventory",
                GraphqlExecutionTargetId::parse("inventory.graphql").expect("target"),
                &sdl,
                &semantic_catalog(),
                AiGraphqlQueryCapabilityLimits::default(),
            ),
            Err(AiError::InvalidConfiguration(_))
        ));
    }

    #[test]
    fn generated_aggregate_root_uses_typed_arguments_and_fixed_projection() {
        let semantic = aggregate_semantic_catalog();
        let catalog = AiGraphqlQueryCapabilityCatalog::compile(
            "inventory",
            GraphqlExecutionTargetId::parse("inventory.graphql").expect("target"),
            SDL,
            &semantic,
            AiGraphqlQueryCapabilityLimits::default(),
        )
        .expect("aggregate capabilities compile");
        let capability = catalog
            .capabilities()
            .find(|capability| capability.is_aggregate())
            .expect("aggregate capability");
        let compiled = capability
            .compile(json!({
                "arguments": {
                    "metrics": [{ "operator": "SUM", "field": "name" }]
                },
                "maximumItems": 5
            }))
            .expect("aggregate query compiles");
        assert!(
            compiled
                .descriptor()
                .document
                .contains("ParentAggregate(groupLimit: $v0, metrics: $v1)")
        );
        assert!(compiled.descriptor().document.contains(
            "groups { field operator kind value } metrics { field operator kind value }"
        ));
        assert_eq!(compiled.variables()["v0"], json!(5));
        assert_eq!(compiled.variables()["v1"][0]["operator"], json!("SUM"));
    }
}
