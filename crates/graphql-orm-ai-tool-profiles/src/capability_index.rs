//! Compact, deterministic discovery metadata for reviewed capabilities.
//!
//! The index deliberately contains no executable document, argument schema,
//! resolver location, database coordinate, credential, or authority. A match
//! is descriptive only; callers must reauthorize and load the exact current
//! capability before execution.

use std::collections::{BTreeMap, BTreeSet};

use graphql_orm_operation_catalog::{
    GeneratedGraphqlOperationCategory, GraphqlAggregateOperator, GraphqlOperationKind,
    GraphqlSemanticCatalog, GraphqlSemanticClassification, GraphqlSemanticExport,
    GraphqlSemanticFieldMetadata, GraphqlSemanticOperationDescriptor,
    GraphqlSemanticRelationshipCardinality, GraphqlSemanticTypeKind, GraphqlSemanticTypeRef,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    AiApprovalRule, AiError, AiGraphqlMutationCapabilityCatalog, AiGraphqlQueryCapabilityCatalog,
    AiGraphqlSubscriptionCapabilityCatalog, AiToolDescriptor, AiToolId, AiToolOperationDomain,
    AiToolOperationKind, AiToolRisk, DataClassification, GraphqlExecutionTargetId,
    canonical_json::canonical_json_bytes,
};

/// Current compact capability-index contract version.
pub const AI_CAPABILITY_INDEX_VERSION: u16 = 1;

const MAXIMUM_SEARCH_TEXT_BYTES: usize = 1_024;
const MAXIMUM_PUBLIC_TEXT_BYTES: usize = 1_024;
const MAXIMUM_PUBLIC_NAME_BYTES: usize = 256;

/// Independent serialization ceilings for one canonical index.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiCapabilityIndexLimits {
    /// Maximum entries retained in one complete index.
    pub maximum_entries: u16,
    /// Maximum canonical JSON bytes for one entry.
    pub maximum_entry_bytes: u32,
    /// Maximum canonical JSON bytes for the complete entry set.
    pub maximum_total_bytes: u32,
    /// Hard maximum results returned by one search.
    pub maximum_search_results: u16,
}

impl Default for AiCapabilityIndexLimits {
    fn default() -> Self {
        Self {
            maximum_entries: 2_048,
            maximum_entry_bytes: 32 * 1_024,
            maximum_total_bytes: 2 * 1_024 * 1_024,
            maximum_search_results: 32,
        }
    }
}

impl AiCapabilityIndexLimits {
    fn validate(self) -> Result<(), AiError> {
        if !(1..=8_192).contains(&self.maximum_entries)
            || !(1_024..=256 * 1_024).contains(&self.maximum_entry_bytes)
            || !(1_024..=16 * 1_024 * 1_024).contains(&self.maximum_total_bytes)
            || !(1..=128).contains(&self.maximum_search_results)
        {
            return Err(configuration_error("capability-index limits are invalid"));
        }
        Ok(())
    }
}

/// Provider-neutral capability family.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiCapabilityKind {
    /// Reviewed server-authored application tool.
    ReviewedStatic,
    /// Generated read/query capability.
    GeneratedQuery,
    /// Separately classified generated mutation capability.
    GeneratedMutation,
    /// Separately policy-bound generated subscription capability.
    GeneratedSubscription,
}

/// Public operation shape used for bounded discovery and filtering.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiCapabilityOperationShape {
    /// One entity or scalar detail result.
    Details,
    /// A bounded entity list.
    List,
    /// A bounded semantic/full-text search.
    Search,
    /// A bounded keyset page.
    KeysetList,
    /// Count/group/metric aggregation.
    Aggregate,
    /// Create operation.
    Create,
    /// Upsert operation.
    Upsert,
    /// Update operation.
    Update,
    /// Multi-record update operation.
    UpdateMany,
    /// Delete operation.
    Delete,
    /// Multi-record delete operation.
    DeleteMany,
    /// Subscription observation.
    Subscription,
    /// Reviewed static operation without a generated category.
    Custom,
}

/// Compact public scalar-field semantics.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiCapabilityScalarSummary {
    /// Public GraphQL field name.
    pub name: String,
    /// Model-safe description authored beside the ORM field.
    pub description: String,
    /// Whether generated filtering admits this field.
    pub filterable: bool,
    /// Whether deterministic ordering admits this field.
    pub sortable: bool,
    /// Whether grouping admits this field.
    pub groupable: bool,
    /// Stable aggregate operator names.
    pub aggregates: BTreeSet<String>,
}

/// Compact public relationship semantics.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiCapabilityRelationshipSummary {
    /// Public GraphQL relationship field name.
    pub name: String,
    /// Model-safe description authored beside the ORM relationship.
    pub description: String,
    /// Public target entity name.
    pub target_entity: String,
    /// Whether the relationship returns multiple values.
    pub to_many: bool,
    /// Public argument names accepted by this relationship.
    pub arguments: BTreeSet<String>,
}

/// Aggregate features discoverable without loading an executable definition.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiCapabilityAggregateFeatures {
    /// At least one count operation is supported.
    pub count: bool,
    /// Public fields supporting sum.
    pub sum_fields: BTreeSet<String>,
    /// Public fields supporting group-by.
    pub group_by_fields: BTreeSet<String>,
}

/// One bounded model-safe canonical discovery entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiCapabilityIndexEntry {
    /// Stable capability identifier.
    pub id: AiToolId,
    /// Capability family.
    pub kind: AiCapabilityKind,
    /// Short stable model-facing name.
    pub name: String,
    /// Model-safe description.
    pub description: String,
    /// Logical public namespace, never a resolver URL.
    pub namespace: String,
    /// Public entity name when generated for one entity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_name: Option<String>,
    /// Public GraphQL root or reviewed operation name.
    pub operation_name: String,
    /// Supported operation shape.
    pub operation_shape: AiCapabilityOperationShape,
    /// Public selectable scalar summary.
    pub scalar_fields: Vec<AiCapabilityScalarSummary>,
    /// Public selectable relationship summary.
    pub relationships: Vec<AiCapabilityRelationshipSummary>,
    /// Count, sum, and grouping features.
    pub aggregate_features: AiCapabilityAggregateFeatures,
    /// Highest model-facing result classification.
    pub result_classification: DataClassification,
    /// Bounded result description.
    pub result_description: String,
    /// Risk classification.
    pub risk: AiToolRisk,
    /// Approval classification.
    pub approval: AiApprovalRule,
    /// Exact executable capability fingerprint.
    pub capability_fingerprint: String,
    /// Exact source catalogue fingerprint.
    pub catalogue_fingerprint: String,
    /// Exact finished-schema fingerprint.
    pub schema_fingerprint: String,
    /// Exact logical-target policy fingerprint.
    pub target_policy_fingerprint: String,
    /// Fingerprint of all preceding entry metadata.
    pub fingerprint: String,
}

/// Complete deterministic compact index.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiCapabilityIndex {
    version: u16,
    target_id: GraphqlExecutionTargetId,
    schema_fingerprint: String,
    semantic_catalogue_fingerprint: String,
    target_policy_fingerprint: String,
    entries: BTreeMap<AiToolId, AiCapabilityIndexEntry>,
    fingerprint: String,
    limits: AiCapabilityIndexLimits,
}

impl AiCapabilityIndex {
    /// Compiles one exact compact index from reviewed static descriptors and
    /// generated capability catalogues already compiled from the finished SDL.
    ///
    /// # Errors
    ///
    /// Returns an error for source drift, duplicates, unsafe public metadata,
    /// invalid fingerprints, or any count/entry/total serialization overflow.
    #[allow(clippy::too_many_arguments)]
    pub fn compile(
        target_id: GraphqlExecutionTargetId,
        schema_fingerprint: impl Into<String>,
        semantic_catalogue: &GraphqlSemanticCatalog,
        query_catalogue: Option<&AiGraphqlQueryCapabilityCatalog>,
        mutation_catalogue: Option<&AiGraphqlMutationCapabilityCatalog>,
        subscription_catalogue: Option<&AiGraphqlSubscriptionCapabilityCatalog>,
        static_descriptors: impl IntoIterator<Item = AiToolDescriptor>,
        target_policy_fingerprint: impl Into<String>,
        limits: AiCapabilityIndexLimits,
    ) -> Result<Self, AiError> {
        limits.validate()?;
        semantic_catalogue
            .validate()
            .map_err(|_| configuration_error("semantic catalogue is invalid"))?;
        let schema_fingerprint = schema_fingerprint.into();
        let target_policy_fingerprint = target_policy_fingerprint.into();
        validate_fingerprint(&schema_fingerprint, "schema fingerprint")?;
        validate_fingerprint(&semantic_catalogue.fingerprint, "semantic fingerprint")?;
        validate_fingerprint(&target_policy_fingerprint, "target policy fingerprint")?;
        validate_catalogue_binding(
            query_catalogue.map(|catalogue| {
                (
                    catalogue.finished_schema_fingerprint(),
                    catalogue.semantic_catalog_fingerprint(),
                )
            }),
            &schema_fingerprint,
            &semantic_catalogue.fingerprint,
        )?;
        validate_catalogue_binding(
            mutation_catalogue.map(|catalogue| {
                (
                    catalogue.finished_schema_fingerprint(),
                    catalogue.semantic_catalog_fingerprint(),
                )
            }),
            &schema_fingerprint,
            &semantic_catalogue.fingerprint,
        )?;
        validate_catalogue_binding(
            subscription_catalogue.map(|catalogue| {
                (
                    catalogue.finished_schema_fingerprint(),
                    catalogue.semantic_catalog_fingerprint(),
                )
            }),
            &schema_fingerprint,
            &semantic_catalogue.fingerprint,
        )?;

        let operations = semantic_catalogue
            .operations
            .iter()
            .map(|operation| ((operation.kind, operation.field_name.as_str()), operation))
            .collect::<BTreeMap<_, _>>();
        let entities = semantic_catalogue
            .entities
            .iter()
            .map(|entity| (entity.entity_name.as_str(), entity))
            .collect::<BTreeMap<_, _>>();
        let mut entries = BTreeMap::new();

        if let Some(catalogue) = query_catalogue {
            for capability in catalogue.capabilities() {
                let operation = exact_operation(
                    &operations,
                    GraphqlOperationKind::Query,
                    capability.field_name(),
                    capability.semantic_operation_fingerprint(),
                )?;
                let entry = generated_entry(
                    capability.id().clone(),
                    AiCapabilityKind::GeneratedQuery,
                    capability.fingerprint(),
                    catalogue.fingerprint(),
                    operation,
                    &entities,
                    &schema_fingerprint,
                    &target_policy_fingerprint,
                )?;
                insert_entry(&mut entries, entry)?;
            }
        }
        if let Some(catalogue) = mutation_catalogue {
            for capability in catalogue.capabilities() {
                let operation = exact_operation(
                    &operations,
                    GraphqlOperationKind::Mutation,
                    capability.field_name(),
                    capability.semantic_operation_fingerprint(),
                )?;
                let entry = generated_entry(
                    capability.id().clone(),
                    AiCapabilityKind::GeneratedMutation,
                    capability.fingerprint(),
                    catalogue.fingerprint(),
                    operation,
                    &entities,
                    &schema_fingerprint,
                    &target_policy_fingerprint,
                )?;
                insert_entry(&mut entries, entry)?;
            }
        }
        if let Some(catalogue) = subscription_catalogue {
            for capability in catalogue.capabilities() {
                let operation = exact_operation(
                    &operations,
                    GraphqlOperationKind::Subscription,
                    capability.field_name(),
                    capability.semantic_operation_fingerprint(),
                )?;
                let entry = generated_entry(
                    capability.id().clone(),
                    AiCapabilityKind::GeneratedSubscription,
                    capability.fingerprint(),
                    catalogue.fingerprint(),
                    operation,
                    &entities,
                    &schema_fingerprint,
                    &target_policy_fingerprint,
                )?;
                insert_entry(&mut entries, entry)?;
            }
        }
        for descriptor in static_descriptors {
            let entry = static_entry(
                descriptor,
                &target_id,
                &schema_fingerprint,
                &semantic_catalogue.fingerprint,
                &target_policy_fingerprint,
            )?;
            insert_entry(&mut entries, entry)?;
        }

        if entries.len() > usize::from(limits.maximum_entries) {
            return Err(configuration_error("capability index has too many entries"));
        }
        let mut total_bytes = 0_usize;
        for entry in entries.values() {
            let bytes = canonical_json_bytes(entry).len();
            if bytes > limits.maximum_entry_bytes as usize {
                return Err(configuration_error("capability index entry is too large"));
            }
            total_bytes = total_bytes
                .checked_add(bytes)
                .ok_or_else(|| configuration_error("capability index is too large"))?;
        }
        if total_bytes > limits.maximum_total_bytes as usize {
            return Err(configuration_error("capability index is too large"));
        }
        let fingerprint = sha256_json(&json!({
            "version": AI_CAPABILITY_INDEX_VERSION,
            "target_id": target_id,
            "schema_fingerprint": schema_fingerprint,
            "semantic_catalogue_fingerprint": semantic_catalogue.fingerprint,
            "target_policy_fingerprint": target_policy_fingerprint,
            "entries": entries,
        }));
        Ok(Self {
            version: AI_CAPABILITY_INDEX_VERSION,
            target_id,
            schema_fingerprint,
            semantic_catalogue_fingerprint: semantic_catalogue.fingerprint.clone(),
            target_policy_fingerprint,
            entries,
            fingerprint,
            limits,
        })
    }

    /// Canonical index fingerprint.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Exact logical target described by this index.
    pub fn target_id(&self) -> &GraphqlExecutionTargetId {
        &self.target_id
    }

    /// Exact finished-schema fingerprint.
    pub fn schema_fingerprint(&self) -> &str {
        &self.schema_fingerprint
    }

    /// Exact semantic-catalogue fingerprint.
    pub fn semantic_catalogue_fingerprint(&self) -> &str {
        &self.semantic_catalogue_fingerprint
    }

    /// Exact target-policy fingerprint used during compilation.
    pub fn target_policy_fingerprint(&self) -> &str {
        &self.target_policy_fingerprint
    }

    /// Every entry in stable capability-ID order.
    pub fn entries(&self) -> impl ExactSizeIterator<Item = &AiCapabilityIndexEntry> {
        self.entries.values()
    }

    /// Resolves one exact entry. This lookup grants no authority.
    pub fn entry(&self, id: &AiToolId) -> Option<&AiCapabilityIndexEntry> {
        self.entries.get(id)
    }

    /// Runs deterministic bounded lexical discovery over model-safe metadata.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty/oversized query or an invalid result
    /// ceiling. Search never loads a definition or grants execution authority.
    pub fn search(
        &self,
        query: &AiCapabilitySearchQuery,
    ) -> Result<AiCapabilitySearchResult, AiError> {
        query.validate(self.limits.maximum_search_results)?;
        let terms = search_terms(&query.text);
        let mut ranked =
            self.entries
                .values()
                .filter(|entry| {
                    query
                        .namespace
                        .as_ref()
                        .is_none_or(|namespace| &entry.namespace == namespace)
                        && query.kind.is_none_or(|kind| entry.kind == kind)
                        && query.entity_or_operation.as_ref().is_none_or(|name| {
                            semantic_key(&entry.operation_name) == semantic_key(name)
                                || entry.entity_name.as_ref().is_some_and(|entity| {
                                    semantic_key(entity) == semantic_key(name)
                                })
                        })
                })
                .filter_map(|entry| {
                    let score = search_score(entry, &terms);
                    (score > 0).then_some((score, entry))
                })
                .collect::<Vec<_>>();
        ranked.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| left.1.id.cmp(&right.1.id))
        });
        let candidates = ranked
            .into_iter()
            .take(usize::from(query.maximum_results))
            .map(|(_, entry)| AiCapabilitySearchCandidate {
                id: entry.id.clone(),
                kind: entry.kind,
                name: entry.name.clone(),
                description: entry.description.clone(),
                namespace: entry.namespace.clone(),
                entity_name: entry.entity_name.clone(),
                operation_name: entry.operation_name.clone(),
                operation_shape: entry.operation_shape,
                capability_fingerprint: entry.capability_fingerprint.clone(),
                entry_fingerprint: entry.fingerprint.clone(),
            })
            .collect();
        Ok(AiCapabilitySearchResult {
            index_fingerprint: self.fingerprint.clone(),
            schema_fingerprint: self.schema_fingerprint.clone(),
            semantic_catalogue_fingerprint: self.semantic_catalogue_fingerprint.clone(),
            target_policy_fingerprint: self.target_policy_fingerprint.clone(),
            candidates,
        })
    }
}

/// Closed bounded discovery request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiCapabilitySearchQuery {
    /// Natural-language search text.
    pub text: String,
    /// Optional exact public logical namespace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Optional exact capability family.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AiCapabilityKind>,
    /// Optional exact public entity or operation name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_or_operation: Option<String>,
    /// Strict positive result ceiling.
    pub maximum_results: u16,
}

impl AiCapabilitySearchQuery {
    fn validate(&self, deployment_maximum: u16) -> Result<(), AiError> {
        validate_text(&self.text, MAXIMUM_SEARCH_TEXT_BYTES, "search text")?;
        if self.text.trim().is_empty()
            || self.maximum_results == 0
            || self.maximum_results > deployment_maximum
        {
            return Err(input_error(
                "capability search is outside deployment limits",
            ));
        }
        if let Some(namespace) = &self.namespace {
            validate_public_name(namespace, "search namespace")?;
        }
        if let Some(name) = &self.entity_or_operation {
            validate_public_name(name, "search operation class")?;
        }
        Ok(())
    }
}

/// One compact exact search candidate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiCapabilitySearchCandidate {
    /// Stable capability identifier.
    pub id: AiToolId,
    /// Capability family.
    pub kind: AiCapabilityKind,
    /// Short model-facing name.
    pub name: String,
    /// Model-safe description.
    pub description: String,
    /// Logical public namespace.
    pub namespace: String,
    /// Public entity name when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_name: Option<String>,
    /// Public operation name.
    pub operation_name: String,
    /// Supported operation shape.
    pub operation_shape: AiCapabilityOperationShape,
    /// Exact executable capability fingerprint.
    pub capability_fingerprint: String,
    /// Exact index-entry fingerprint.
    pub entry_fingerprint: String,
}

/// Bounded deterministic discovery response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiCapabilitySearchResult {
    /// Exact index fingerprint searched.
    pub index_fingerprint: String,
    /// Exact finished-schema fingerprint searched.
    pub schema_fingerprint: String,
    /// Exact semantic-catalogue fingerprint searched.
    pub semantic_catalogue_fingerprint: String,
    /// Exact target-policy fingerprint searched.
    pub target_policy_fingerprint: String,
    /// Ranked candidates with stable ID tie-breaking.
    pub candidates: Vec<AiCapabilitySearchCandidate>,
}

#[allow(clippy::too_many_arguments)]
fn generated_entry(
    id: AiToolId,
    kind: AiCapabilityKind,
    capability_fingerprint: &str,
    catalogue_fingerprint: &str,
    operation: &GraphqlSemanticOperationDescriptor,
    entities: &BTreeMap<&str, &graphql_orm_operation_catalog::GraphqlEntitySemanticMetadata>,
    schema_fingerprint: &str,
    target_policy_fingerprint: &str,
) -> Result<AiCapabilityIndexEntry, AiError> {
    validate_text(
        &operation.description,
        MAXIMUM_PUBLIC_TEXT_BYTES,
        "operation description",
    )?;
    validate_public_name(&operation.field_name, "operation name")?;
    let entity_name = operation
        .generated_entity_name
        .as_deref()
        .or_else(|| result_object_name(&operation.result_type));
    let entity = entity_name.and_then(|name| entities.get(name).copied());
    if entity_name.is_some() && entity.is_none() {
        return Err(configuration_error("capability result entity is missing"));
    }
    let (scalar_fields, relationships, aggregate_features, result_classification) = entity
        .map(entity_summaries)
        .transpose()?
        .unwrap_or_else(|| {
            (
                Vec::new(),
                Vec::new(),
                AiCapabilityAggregateFeatures::default(),
                operation
                    .result_disclosure
                    .map(|disclosure| classification(disclosure.classification))
                    .unwrap_or(DataClassification::Public),
            )
        });
    let shape = operation_shape(operation.generated_category, kind);
    let namespace = id_namespace(&id);
    let name = entity_name.map_or_else(
        || operation.field_name.clone(),
        |entity| format!("{} {}", shape_name(shape), entity),
    );
    let mut entry = AiCapabilityIndexEntry {
        id,
        kind,
        name,
        description: operation.description.clone(),
        namespace,
        entity_name: entity_name.map(str::to_owned),
        operation_name: operation.field_name.clone(),
        operation_shape: shape,
        scalar_fields,
        relationships,
        aggregate_features,
        result_classification,
        result_description: generated_result_description(operation),
        risk: generated_risk(kind, operation),
        approval: generated_approval(kind, operation),
        capability_fingerprint: capability_fingerprint.to_owned(),
        catalogue_fingerprint: catalogue_fingerprint.to_owned(),
        schema_fingerprint: schema_fingerprint.to_owned(),
        target_policy_fingerprint: target_policy_fingerprint.to_owned(),
        fingerprint: String::new(),
    };
    validate_entry_text(&entry)?;
    entry.fingerprint = entry_fingerprint(&entry);
    Ok(entry)
}

fn result_object_name(result: &GraphqlSemanticTypeRef) -> Option<&str> {
    match result {
        GraphqlSemanticTypeRef::Named {
            name,
            kind: GraphqlSemanticTypeKind::Object,
            ..
        } => Some(name),
        GraphqlSemanticTypeRef::Named { .. } => None,
        GraphqlSemanticTypeRef::List { item, .. } => result_object_name(item),
    }
}

fn static_entry(
    descriptor: AiToolDescriptor,
    target_id: &GraphqlExecutionTargetId,
    schema_fingerprint: &str,
    semantic_catalogue_fingerprint: &str,
    target_policy_fingerprint: &str,
) -> Result<AiCapabilityIndexEntry, AiError> {
    if !descriptor.has_valid_fingerprint()
        || descriptor.operation_domain != AiToolOperationDomain::Application
        || descriptor.maximum_classification == DataClassification::Secret
    {
        return Err(configuration_error(
            "static capability descriptor is not indexable",
        ));
    }
    if let Some(contract) = &descriptor.graphql_contract
        && (&contract.target_id != target_id || contract.schema_fingerprint != schema_fingerprint)
    {
        return Err(configuration_error("static capability target has drifted"));
    }
    let kind = match descriptor.operation_kind {
        AiToolOperationKind::Query | AiToolOperationKind::Internal => {
            AiCapabilityKind::ReviewedStatic
        }
        AiToolOperationKind::Mutation => AiCapabilityKind::GeneratedMutation,
        AiToolOperationKind::Subscription => AiCapabilityKind::GeneratedSubscription,
    };
    let operation_name = descriptor
        .graphql_contract
        .as_ref()
        .and_then(|contract| contract.semantic_operation.as_ref())
        .map_or_else(
            || {
                descriptor
                    .id
                    .as_str()
                    .rsplit('.')
                    .next()
                    .unwrap_or("tool")
                    .to_owned()
            },
            |binding| binding.field_name().to_owned(),
        );
    let mut entry = AiCapabilityIndexEntry {
        id: descriptor.id.clone(),
        kind,
        name: operation_name.clone(),
        description: descriptor.description.clone(),
        namespace: id_namespace(&descriptor.id),
        entity_name: None,
        operation_name,
        operation_shape: AiCapabilityOperationShape::Custom,
        scalar_fields: Vec::new(),
        relationships: Vec::new(),
        aggregate_features: AiCapabilityAggregateFeatures::default(),
        result_classification: descriptor.maximum_classification,
        result_description: format!(
            "Bounded result: at most {} records and {} bytes.",
            descriptor.maximum_result_records, descriptor.maximum_result_bytes
        ),
        risk: descriptor.risk,
        approval: descriptor.approval,
        capability_fingerprint: descriptor.fingerprint.clone(),
        catalogue_fingerprint: semantic_catalogue_fingerprint.to_owned(),
        schema_fingerprint: schema_fingerprint.to_owned(),
        target_policy_fingerprint: target_policy_fingerprint.to_owned(),
        fingerprint: String::new(),
    };
    validate_entry_text(&entry)?;
    entry.fingerprint = entry_fingerprint(&entry);
    Ok(entry)
}

type EntitySummaries = (
    Vec<AiCapabilityScalarSummary>,
    Vec<AiCapabilityRelationshipSummary>,
    AiCapabilityAggregateFeatures,
    DataClassification,
);

fn entity_summaries(
    entity: &graphql_orm_operation_catalog::GraphqlEntitySemanticMetadata,
) -> Result<EntitySummaries, AiError> {
    validate_text(
        &entity.description,
        MAXIMUM_PUBLIC_TEXT_BYTES,
        "entity description",
    )?;
    let mut scalars = Vec::new();
    let mut relationships = Vec::new();
    let mut aggregate = AiCapabilityAggregateFeatures::default();
    let mut result_classification = classification(entity.default_classification);
    for field in entity.fields.iter().filter(public_field) {
        validate_public_name(&field.field_name, "field name")?;
        validate_text(
            &field.description,
            MAXIMUM_PUBLIC_TEXT_BYTES,
            "field description",
        )?;
        result_classification = result_classification.max(classification(field.classification));
        if let Some(relationship) = &field.relationship {
            relationships.push(AiCapabilityRelationshipSummary {
                name: field.field_name.clone(),
                description: field.description.clone(),
                target_entity: relationship.target.clone(),
                to_many: relationship.cardinality == GraphqlSemanticRelationshipCardinality::Many,
                arguments: relationship
                    .arguments
                    .iter()
                    .map(|argument| argument.graphql_name.clone())
                    .collect(),
            });
        } else {
            let aggregates = field
                .aggregate_operators
                .iter()
                .map(|operator| aggregate_name(*operator).to_owned())
                .collect::<BTreeSet<_>>();
            aggregate.count |= field
                .aggregate_operators
                .contains(&GraphqlAggregateOperator::Count);
            if field
                .aggregate_operators
                .contains(&GraphqlAggregateOperator::Sum)
            {
                aggregate.sum_fields.insert(field.field_name.clone());
            }
            if field.groupable {
                aggregate.group_by_fields.insert(field.field_name.clone());
            }
            scalars.push(AiCapabilityScalarSummary {
                name: field.field_name.clone(),
                description: field.description.clone(),
                filterable: !field.filter_operators.is_empty(),
                sortable: field.sortable,
                groupable: field.groupable,
                aggregates,
            });
        }
    }
    scalars.sort_by(|left, right| left.name.cmp(&right.name));
    relationships.sort_by(|left, right| left.name.cmp(&right.name));
    Ok((scalars, relationships, aggregate, result_classification))
}

fn public_field(field: &&GraphqlSemanticFieldMetadata) -> bool {
    field.selectable
        && field.export == GraphqlSemanticExport::Exportable
        && field.classification != GraphqlSemanticClassification::Secret
}

fn exact_operation<'a>(
    operations: &BTreeMap<(GraphqlOperationKind, &'a str), &'a GraphqlSemanticOperationDescriptor>,
    kind: GraphqlOperationKind,
    field_name: &str,
    fingerprint: &str,
) -> Result<&'a GraphqlSemanticOperationDescriptor, AiError> {
    let operation = operations
        .get(&(kind, field_name))
        .copied()
        .ok_or_else(|| configuration_error("capability semantic operation is missing"))?;
    if operation.fingerprint != fingerprint {
        return Err(configuration_error(
            "capability semantic operation has drifted",
        ));
    }
    Ok(operation)
}

fn validate_catalogue_binding(
    binding: Option<(&str, &str)>,
    schema_fingerprint: &str,
    semantic_fingerprint: &str,
) -> Result<(), AiError> {
    if binding.is_some_and(|(schema, semantic)| {
        schema != schema_fingerprint || semantic != semantic_fingerprint
    }) {
        return Err(configuration_error("capability catalogue has drifted"));
    }
    Ok(())
}

fn insert_entry(
    entries: &mut BTreeMap<AiToolId, AiCapabilityIndexEntry>,
    entry: AiCapabilityIndexEntry,
) -> Result<(), AiError> {
    if entries.insert(entry.id.clone(), entry).is_some() {
        return Err(configuration_error("duplicate capability index ID"));
    }
    Ok(())
}

fn operation_shape(
    category: Option<GeneratedGraphqlOperationCategory>,
    kind: AiCapabilityKind,
) -> AiCapabilityOperationShape {
    match category {
        Some(GeneratedGraphqlOperationCategory::List) => AiCapabilityOperationShape::List,
        Some(GeneratedGraphqlOperationCategory::SingleRead) => AiCapabilityOperationShape::Details,
        Some(GeneratedGraphqlOperationCategory::Search) => AiCapabilityOperationShape::Search,
        Some(GeneratedGraphqlOperationCategory::KeysetList) => {
            AiCapabilityOperationShape::KeysetList
        }
        Some(GeneratedGraphqlOperationCategory::Aggregate) => AiCapabilityOperationShape::Aggregate,
        Some(GeneratedGraphqlOperationCategory::Create) => AiCapabilityOperationShape::Create,
        Some(GeneratedGraphqlOperationCategory::Upsert) => AiCapabilityOperationShape::Upsert,
        Some(GeneratedGraphqlOperationCategory::Update) => AiCapabilityOperationShape::Update,
        Some(GeneratedGraphqlOperationCategory::UpdateMany) => {
            AiCapabilityOperationShape::UpdateMany
        }
        Some(GeneratedGraphqlOperationCategory::Delete) => AiCapabilityOperationShape::Delete,
        Some(GeneratedGraphqlOperationCategory::DeleteMany) => {
            AiCapabilityOperationShape::DeleteMany
        }
        Some(GeneratedGraphqlOperationCategory::Subscription) => {
            AiCapabilityOperationShape::Subscription
        }
        Some(_) => AiCapabilityOperationShape::Custom,
        None if kind == AiCapabilityKind::GeneratedSubscription => {
            AiCapabilityOperationShape::Subscription
        }
        None => AiCapabilityOperationShape::Custom,
    }
}

fn shape_name(shape: AiCapabilityOperationShape) -> &'static str {
    match shape {
        AiCapabilityOperationShape::Details => "Details",
        AiCapabilityOperationShape::List => "List",
        AiCapabilityOperationShape::Search => "Search",
        AiCapabilityOperationShape::KeysetList => "Page",
        AiCapabilityOperationShape::Aggregate => "Aggregate",
        AiCapabilityOperationShape::Create => "Create",
        AiCapabilityOperationShape::Upsert => "Upsert",
        AiCapabilityOperationShape::Update => "Update",
        AiCapabilityOperationShape::UpdateMany => "Update many",
        AiCapabilityOperationShape::Delete => "Delete",
        AiCapabilityOperationShape::DeleteMany => "Delete many",
        AiCapabilityOperationShape::Subscription => "Watch",
        AiCapabilityOperationShape::Custom => "Use",
    }
}

fn generated_risk(
    kind: AiCapabilityKind,
    operation: &GraphqlSemanticOperationDescriptor,
) -> AiToolRisk {
    match kind {
        AiCapabilityKind::GeneratedQuery | AiCapabilityKind::GeneratedSubscription => {
            AiToolRisk::ReadOnly
        }
        AiCapabilityKind::GeneratedMutation => match operation.ai_mutation_execution {
            Some(graphql_orm_operation_catalog::AiMutationExecutionPolicy::Automatic) => {
                AiToolRisk::LowRiskWrite
            }
            _ => AiToolRisk::NonIdempotentWrite,
        },
        AiCapabilityKind::ReviewedStatic => AiToolRisk::ReadOnly,
    }
}

fn generated_approval(
    kind: AiCapabilityKind,
    operation: &GraphqlSemanticOperationDescriptor,
) -> AiApprovalRule {
    if kind != AiCapabilityKind::GeneratedMutation {
        return AiApprovalRule::None;
    }
    match operation.ai_mutation_execution {
        Some(graphql_orm_operation_catalog::AiMutationExecutionPolicy::Automatic) => {
            AiApprovalRule::None
        }
        Some(graphql_orm_operation_catalog::AiMutationExecutionPolicy::ApprovalRequired) => {
            AiApprovalRule::OneShot
        }
        Some(graphql_orm_operation_catalog::AiMutationExecutionPolicy::Prohibited) | None => {
            AiApprovalRule::Never
        }
    }
}

fn generated_result_description(operation: &GraphqlSemanticOperationDescriptor) -> String {
    let shape = operation_shape(
        operation.generated_category,
        match operation.kind {
            GraphqlOperationKind::Query => AiCapabilityKind::GeneratedQuery,
            GraphqlOperationKind::Mutation => AiCapabilityKind::GeneratedMutation,
            _ => AiCapabilityKind::GeneratedSubscription,
        },
    );
    format!(
        "Bounded {} result for {}.",
        shape_name(shape).to_lowercase(),
        operation.field_name
    )
}

fn entry_fingerprint(entry: &AiCapabilityIndexEntry) -> String {
    let mut canonical = entry.clone();
    canonical.fingerprint.clear();
    hex::encode(Sha256::digest(canonical_json_bytes(&canonical)))
}

fn validate_entry_text(entry: &AiCapabilityIndexEntry) -> Result<(), AiError> {
    validate_public_name(&entry.name, "capability name")?;
    validate_text(
        &entry.description,
        MAXIMUM_PUBLIC_TEXT_BYTES,
        "capability description",
    )?;
    validate_public_name(&entry.namespace, "capability namespace")?;
    validate_public_name(&entry.operation_name, "capability operation name")?;
    validate_text(
        &entry.result_description,
        MAXIMUM_PUBLIC_TEXT_BYTES,
        "capability result description",
    )?;
    validate_fingerprint(&entry.capability_fingerprint, "capability fingerprint")?;
    validate_fingerprint(&entry.catalogue_fingerprint, "catalogue fingerprint")?;
    Ok(())
}

fn validate_fingerprint(value: &str, label: &str) -> Result<(), AiError> {
    if value.is_empty()
        || value.len() > 256
        || value.chars().any(char::is_control)
        || value.chars().any(char::is_whitespace)
    {
        return Err(configuration_error(label));
    }
    Ok(())
}

fn validate_public_name(value: &str, label: &str) -> Result<(), AiError> {
    validate_text(value, MAXIMUM_PUBLIC_NAME_BYTES, label)?;
    if value.trim().is_empty() {
        return Err(configuration_error(label));
    }
    Ok(())
}

fn validate_text(value: &str, maximum: usize, label: &str) -> Result<(), AiError> {
    if value.len() > maximum
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(configuration_error(label));
    }
    Ok(())
}

fn id_namespace(id: &AiToolId) -> String {
    id.as_str()
        .split('.')
        .next()
        .unwrap_or("application")
        .to_owned()
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

fn aggregate_name(operator: GraphqlAggregateOperator) -> &'static str {
    match operator {
        GraphqlAggregateOperator::Count => "count",
        GraphqlAggregateOperator::Min => "min",
        GraphqlAggregateOperator::Max => "max",
        GraphqlAggregateOperator::Sum => "sum",
        _ => "unknown",
    }
}

fn search_score(entry: &AiCapabilityIndexEntry, terms: &BTreeSet<String>) -> u64 {
    let mut score = 0_u64;
    let id = search_terms(entry.id.as_str());
    let name = search_terms(&entry.name);
    let operation = search_terms(&entry.operation_name);
    let entity = entry
        .entity_name
        .as_deref()
        .map(search_terms)
        .unwrap_or_default();
    let description = search_terms(&entry.description);
    let relationships = entry
        .relationships
        .iter()
        .flat_map(|relationship| {
            search_terms(&format!(
                "{} {} {}",
                relationship.name, relationship.description, relationship.target_entity
            ))
        })
        .collect::<BTreeSet<_>>();
    let fields = entry
        .scalar_fields
        .iter()
        .flat_map(|field| search_terms(&format!("{} {}", field.name, field.description)))
        .collect::<BTreeSet<_>>();
    for term in terms {
        score += u64::from(id.contains(term)) * 10;
        score += u64::from(name.contains(term)) * 9;
        score += u64::from(operation.contains(term)) * 8;
        score += u64::from(entity.contains(term)) * 8;
        score += u64::from(relationships.contains(term)) * 7;
        score += u64::from(fields.contains(term)) * 4;
        score += u64::from(description.contains(term)) * 3;
        score += u64::from(semantic_key(shape_name(entry.operation_shape)) == *term) * 6;
    }
    score
}

fn search_terms(value: &str) -> BTreeSet<String> {
    value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter_map(|term| {
            let term = semantic_key(term);
            (!term.is_empty()).then_some(term)
        })
        .collect()
}

fn semantic_key(value: &str) -> String {
    let mut value = value.to_ascii_lowercase();
    if value.len() > 4 && value.ends_with("ies") {
        value.truncate(value.len() - 3);
        value.push('y');
    } else if value.len() > 3 && value.ends_with('s') {
        value.pop();
    }
    value
}

fn sha256_json(value: &serde_json::Value) -> String {
    hex::encode(Sha256::digest(canonical_json_bytes(value)))
}

fn input_error(message: &str) -> AiError {
    AiError::InvalidInput(message.to_owned())
}

fn configuration_error(message: &str) -> AiError {
    AiError::InvalidConfiguration(message.to_owned())
}

#[cfg(test)]
mod tests {
    use graphql_orm_operation_catalog::{GraphqlOperationCatalog, GraphqlSemanticCatalog};

    use super::*;

    fn semantic_catalogue() -> GraphqlSemanticCatalog {
        GraphqlSemanticCatalog::compose(
            Vec::<graphql_orm_operation_catalog::GraphqlEntitySemanticMetadata>::new(),
            &GraphqlOperationCatalog::compose(Vec::<(
                &'static [graphql_orm_operation_catalog::GeneratedGraphqlOperationDescriptor],
                bool,
                bool,
            )>::new()),
        )
        .expect("empty semantic catalogue")
    }

    fn descriptor(id: &str, description: &str, padding: usize) -> AiToolDescriptor {
        AiToolDescriptor::new(
            id,
            description,
            AiToolOperationKind::Query,
            "query Reviewed { reviewed }",
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "x".repeat(padding),
                    }
                },
                "required": ["query"],
                "additionalProperties": false,
            }),
        )
        .expect("descriptor")
        .with_maximum_classification(DataClassification::Public)
    }

    #[test]
    fn large_definition_universe_has_a_small_complete_discovery_surface() {
        let semantic = semantic_catalogue();
        let mut definitions = Vec::new();
        definitions.push(descriptor(
            "jim.jobs_list",
            "List the latest Jim jobs and Actions.",
            90_000,
        ));
        definitions.push(descriptor(
            "jim.job_details",
            "Read Jim Job details including Labour for an Action.",
            90_000,
        ));
        definitions.push(descriptor(
            "jim.labour_list",
            "List Labour attached to a Jim Job or Action.",
            90_000,
        ));
        for index in 3..55 {
            definitions.push(descriptor(
                &format!("reviewed.capability_{index}"),
                &format!("Reviewed read capability {index}."),
                90_000,
            ));
        }
        let definition_bytes = definitions
            .iter()
            .map(|definition| {
                serde_json::to_vec(&definition.argument_schema)
                    .unwrap()
                    .len()
            })
            .sum::<usize>();
        assert!(definition_bytes >= 4_900_000);
        let index = AiCapabilityIndex::compile(
            GraphqlExecutionTargetId::parse("jim-production").expect("target"),
            "schema-v1",
            &semantic,
            None,
            None,
            None,
            definitions,
            "target-policy-v1",
            AiCapabilityIndexLimits::default(),
        )
        .expect("compact index");
        assert_eq!(index.entries().len(), 55);
        assert!(serde_json::to_vec(&index).unwrap().len() < 128 * 1024);

        let latest = index
            .search(&AiCapabilitySearchQuery {
                text: "latest Jim jobs".to_owned(),
                namespace: Some("jim".to_owned()),
                kind: None,
                entity_or_operation: None,
                maximum_results: 3,
            })
            .expect("search");
        assert_eq!(latest.candidates[0].id.as_str(), "jim.jobs_list");

        let labour = index
            .search(&AiCapabilitySearchQuery {
                text: "labour on Action 123".to_owned(),
                namespace: Some("jim".to_owned()),
                kind: None,
                entity_or_operation: None,
                maximum_results: 3,
            })
            .expect("search");
        let ids = labour
            .candidates
            .iter()
            .map(|candidate| candidate.id.as_str())
            .collect::<BTreeSet<_>>();
        assert!(ids.contains("jim.job_details"));
        assert!(ids.contains("jim.labour_list"));
    }

    #[test]
    fn search_ranking_and_index_serialization_are_deterministic() {
        let semantic = semantic_catalogue();
        let compile = |definitions| {
            AiCapabilityIndex::compile(
                GraphqlExecutionTargetId::parse("application").expect("target"),
                "schema-v1",
                &semantic,
                None,
                None,
                None,
                definitions,
                "target-policy-v1",
                AiCapabilityIndexLimits::default(),
            )
            .expect("index")
        };
        let first = compile(vec![
            descriptor("app.beta", "Find matching records.", 0),
            descriptor("app.alpha", "Find matching records.", 0),
        ]);
        let second = compile(vec![
            descriptor("app.alpha", "Find matching records.", 0),
            descriptor("app.beta", "Find matching records.", 0),
        ]);
        assert_eq!(first.fingerprint(), second.fingerprint());
        assert_eq!(
            serde_json::to_vec(&first).expect("serialize"),
            serde_json::to_vec(&second).expect("serialize")
        );
        let result = first
            .search(&AiCapabilitySearchQuery {
                text: "matching records".to_owned(),
                namespace: None,
                kind: None,
                entity_or_operation: None,
                maximum_results: 2,
            })
            .expect("search");
        assert_eq!(result.candidates[0].id.as_str(), "app.alpha");
        assert_eq!(result.candidates[1].id.as_str(), "app.beta");
    }
}
