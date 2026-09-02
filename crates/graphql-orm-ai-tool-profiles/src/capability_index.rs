//! Compact, deterministic discovery metadata for reviewed capabilities.
//!
//! The index deliberately contains no executable document, argument schema,
//! resolver location, database coordinate, credential, or authority. A match
//! is descriptive only; callers must reauthorize and load the exact current
//! capability before execution.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

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
    AiGraphqlResultRecordCostEstimate, AiGraphqlSubscriptionCapabilityCatalog, AiToolDescriptor,
    AiToolId, AiToolOperationDomain, AiToolOperationKind, AiToolRisk, DataClassification,
    GraphqlExecutionTargetId, canonical_json::canonical_json_bytes,
};

/// Current compact capability-index contract version.
pub const AI_CAPABILITY_INDEX_VERSION: u16 = 2;

/// Current deterministic multi-target capability-index-set contract version.
pub const AI_CAPABILITY_INDEX_SET_VERSION: u16 = 1;

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

/// Independent ceilings for one canonical federated index set.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiCapabilityIndexSetLimits {
    /// Maximum independently owned logical targets.
    pub maximum_indexes: u16,
    /// Maximum entries across every member index.
    pub maximum_entries: u32,
    /// Maximum canonical JSON bytes across every member entry.
    pub maximum_total_bytes: u64,
    /// Maximum results returned by one global search.
    pub maximum_search_results: u16,
}

impl Default for AiCapabilityIndexSetLimits {
    fn default() -> Self {
        Self {
            maximum_indexes: 256,
            maximum_entries: 16_384,
            maximum_total_bytes: 64 * 1_024 * 1_024,
            maximum_search_results: 32,
        }
    }
}

impl AiCapabilityIndexSetLimits {
    fn validate(self) -> Result<(), AiError> {
        if !(1..=1_024).contains(&self.maximum_indexes)
            || !(1..=1_048_576).contains(&self.maximum_entries)
            || !(1_024..=4 * 1_024 * 1_024 * 1_024).contains(&self.maximum_total_bytes)
            || !(1..=128).contains(&self.maximum_search_results)
        {
            return Err(configuration_error(
                "capability index set limits are invalid",
            ));
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
    /// Conservative compiler-owned result-record planning bounds.
    pub result_record_cost: AiGraphqlResultRecordCostEstimate,
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
                    capability.result_record_cost_estimate(),
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
                    capability.result_record_cost_estimate(),
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
                    capability.result_record_cost_estimate(),
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
        let candidates = rank_entries(
            self.entries.values().map(|entry| (&self.target_id, entry)),
            query,
        )
        .into_iter()
        .take(usize::from(query.maximum_results))
        .map(|(_, entry)| search_candidate(entry))
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

/// Deterministic collection of independently compiled capability indexes.
///
/// An index set preserves the exact logical target, schema, semantic
/// catalogue, and policy fingerprint of every owning index. It is not a
/// synthetic GraphQL schema and grants no authority. Capability identifiers
/// must be globally unique so discovery can resolve each candidate to exactly
/// one owning target before current policy and resolver authorization run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiCapabilityIndexSet {
    indexes: BTreeMap<GraphqlExecutionTargetId, Arc<AiCapabilityIndex>>,
    capability_owners: BTreeMap<AiToolId, GraphqlExecutionTargetId>,
    fingerprint: String,
    maximum_search_results: u16,
    limits: AiCapabilityIndexSetLimits,
}

impl AiCapabilityIndexSet {
    /// Compiles a canonical index set from independently reviewed indexes.
    ///
    /// Input order does not affect the aggregate fingerprint. The aggregate
    /// binds every target to its exact index fingerprint without inventing a
    /// combined schema or catalogue. Duplicate targets and capability IDs are
    /// rejected even when their contents happen to match.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty or oversized set, a duplicate target, a
    /// duplicate capability identifier, or an invalid member fingerprint.
    pub fn compile<I, T>(indexes: I) -> Result<Self, AiError>
    where
        I: IntoIterator<Item = T>,
        T: Into<Arc<AiCapabilityIndex>>,
    {
        Self::compile_with_limits(indexes, AiCapabilityIndexSetLimits::default())
    }

    /// Compiles a canonical index set with explicit aggregate ceilings.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid limits or when the member set exceeds any
    /// configured target, entry, or search-result ceiling.
    pub fn compile_with_limits<I, T>(
        indexes: I,
        limits: AiCapabilityIndexSetLimits,
    ) -> Result<Self, AiError>
    where
        I: IntoIterator<Item = T>,
        T: Into<Arc<AiCapabilityIndex>>,
    {
        limits.validate()?;
        let mut canonical = BTreeMap::new();
        let mut capability_owners = BTreeMap::new();
        let mut total_bytes = 0_u64;
        let mut maximum_search_results = limits.maximum_search_results;
        for index in indexes {
            let index = index.into();
            if !valid_sha256_fingerprint(&index.fingerprint) {
                return Err(configuration_error(
                    "capability index set contains an invalid fingerprint",
                ));
            }
            let target_id = index.target_id.clone();
            if canonical.contains_key(&target_id) {
                return Err(configuration_error(
                    "capability index set contains a duplicate target",
                ));
            }
            for id in index.entries.keys() {
                if capability_owners
                    .insert(id.clone(), target_id.clone())
                    .is_some()
                {
                    return Err(configuration_error(
                        "capability index set contains a duplicate capability",
                    ));
                }
            }
            for entry in index.entries.values() {
                let entry_bytes =
                    u64::try_from(canonical_json_bytes(entry).len()).map_err(|_| {
                        configuration_error("capability index set byte size overflowed")
                    })?;
                total_bytes = total_bytes.checked_add(entry_bytes).ok_or_else(|| {
                    configuration_error("capability index set byte size overflowed")
                })?;
            }
            maximum_search_results =
                maximum_search_results.min(index.limits.maximum_search_results);
            canonical.insert(target_id, index);
            if canonical.len() > usize::from(limits.maximum_indexes) {
                return Err(configuration_error(
                    "capability index set contains too many targets",
                ));
            }
            if capability_owners.len() > limits.maximum_entries as usize {
                return Err(configuration_error(
                    "capability index set contains too many entries",
                ));
            }
            if total_bytes > limits.maximum_total_bytes {
                return Err(configuration_error(
                    "capability index set contains too many bytes",
                ));
            }
        }
        if canonical.is_empty() {
            return Err(configuration_error("capability index set is empty"));
        }
        let members = canonical
            .iter()
            .map(|(target_id, index)| {
                json!({
                    "target_id": target_id,
                    "index_fingerprint": index.fingerprint,
                })
            })
            .collect::<Vec<_>>();
        let fingerprint = sha256_json(&json!({
            "version": AI_CAPABILITY_INDEX_SET_VERSION,
            "members": members,
        }));
        Ok(Self {
            indexes: canonical,
            capability_owners,
            fingerprint,
            maximum_search_results,
            limits,
        })
    }

    /// Canonical aggregate fingerprint for every exact member index.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Aggregate ceilings validated for this set.
    pub const fn limits(&self) -> AiCapabilityIndexSetLimits {
        self.limits
    }

    /// Member indexes in stable logical-target order.
    pub fn indexes(&self) -> impl ExactSizeIterator<Item = &Arc<AiCapabilityIndex>> {
        self.indexes.values()
    }

    /// Resolves one exact member index by logical target.
    pub fn index(&self, target_id: &GraphqlExecutionTargetId) -> Option<&Arc<AiCapabilityIndex>> {
        self.indexes.get(target_id)
    }

    /// Resolves the sole owning index for a globally unique capability ID.
    pub fn owning_index(&self, id: &AiToolId) -> Option<&Arc<AiCapabilityIndex>> {
        self.capability_owners
            .get(id)
            .and_then(|target_id| self.indexes.get(target_id))
    }

    /// Resolves one exact entry and its owning index. This grants no authority.
    pub fn entry(
        &self,
        id: &AiToolId,
    ) -> Option<(&Arc<AiCapabilityIndex>, &AiCapabilityIndexEntry)> {
        let index = self.owning_index(id)?;
        Some((index, index.entry(id)?))
    }

    /// Searches every member index as one deterministic bounded namespace.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid query or result ceiling. Search emits
    /// model-safe metadata only and grants no authority.
    pub fn search(
        &self,
        query: &AiCapabilitySearchQuery,
    ) -> Result<AiCapabilityIndexSetSearchResult, AiError> {
        query.validate(self.maximum_search_results)?;
        let candidates = rank_entries(
            self.indexes.values().flat_map(|index| {
                index
                    .entries
                    .values()
                    .map(|entry| (&index.target_id, entry))
            }),
            query,
        )
        .into_iter()
        .take(usize::from(query.maximum_results))
        .map(|(_, entry)| search_candidate(entry))
        .collect();
        Ok(AiCapabilityIndexSetSearchResult {
            index_set_fingerprint: self.fingerprint.clone(),
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

/// Bounded deterministic discovery response across a canonical index set.
///
/// Member schema, semantic-catalogue, target-policy, and index fingerprints
/// remain on their owning indexes and are revalidated when a candidate is
/// loaded. This response binds the complete active set without fabricating a
/// synthetic cross-subgraph schema identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiCapabilityIndexSetSearchResult {
    /// Exact aggregate index-set fingerprint searched.
    pub index_set_fingerprint: String,
    /// Ranked globally unique candidates with stable ID tie-breaking.
    pub candidates: Vec<AiCapabilitySearchCandidate>,
}

fn entry_matches_query(entry: &AiCapabilityIndexEntry, query: &AiCapabilitySearchQuery) -> bool {
    query
        .namespace
        .as_ref()
        .is_none_or(|namespace| &entry.namespace == namespace)
        && query.kind.is_none_or(|kind| entry.kind == kind)
        && query.entity_or_operation.as_ref().is_none_or(|name| {
            semantic_key(&entry.operation_name) == semantic_key(name)
                || entry
                    .entity_name
                    .as_ref()
                    .is_some_and(|entity| semantic_key(entity) == semantic_key(name))
        })
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SearchRank {
    root_semantics: u64,
    nested_semantics: u64,
    shape: u8,
}

impl SearchRank {
    fn has_semantic_match(self) -> bool {
        self.root_semantics > 0 || self.nested_semantics > 0
    }
}

fn rank_entries<'a>(
    entries: impl IntoIterator<Item = (&'a GraphqlExecutionTargetId, &'a AiCapabilityIndexEntry)>,
    query: &AiCapabilitySearchQuery,
) -> Vec<(SearchRank, &'a AiCapabilityIndexEntry)> {
    let tokens = search_tokens(&query.text);
    let terms = tokens.iter().cloned().collect::<BTreeSet<_>>();
    let shape_intent = search_shape_intent(&tokens);
    let mut ranked = entries
        .into_iter()
        .filter(|(_, entry)| entry_matches_query(entry, query))
        .filter_map(|(target_id, entry)| {
            let rank = search_rank(target_id, entry, &terms, shape_intent);
            rank.has_semantic_match().then_some((rank, entry))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1.id.cmp(&right.1.id))
    });
    ranked
}

fn search_candidate(entry: &AiCapabilityIndexEntry) -> AiCapabilitySearchCandidate {
    AiCapabilitySearchCandidate {
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
    }
}

#[allow(clippy::too_many_arguments)]
fn generated_entry(
    id: AiToolId,
    kind: AiCapabilityKind,
    capability_fingerprint: &str,
    result_record_cost: AiGraphqlResultRecordCostEstimate,
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
        result_record_cost,
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
        result_record_cost: AiGraphqlResultRecordCostEstimate {
            maximum_root_records: descriptor.maximum_result_records,
            maximum_total_records: descriptor.maximum_result_records,
            root_bound_required: false,
        },
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

fn search_rank(
    target_id: &GraphqlExecutionTargetId,
    entry: &AiCapabilityIndexEntry,
    terms: &BTreeSet<String>,
    shape_intent: Option<AiCapabilityOperationShape>,
) -> SearchRank {
    let mut root_semantics = 0_u64;
    let mut nested_semantics = 0_u64;
    let id = search_terms(entry.id.as_str());
    let name = search_terms(&entry.name);
    let operation = search_terms(&entry.operation_name);
    let entity = entry
        .entity_name
        .as_deref()
        .map(search_terms)
        .unwrap_or_default();
    let target = search_terms(target_id.as_str());
    let namespace = search_terms(&entry.namespace);
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
        if search_shape_term(term) {
            continue;
        }
        root_semantics += u64::from(id.contains(term)) * 12;
        root_semantics += u64::from(name.contains(term)) * 11;
        root_semantics += u64::from(operation.contains(term)) * 10;
        root_semantics += u64::from(entity.contains(term)) * 9;
        root_semantics += u64::from(target.contains(term)) * 8;
        root_semantics += u64::from(namespace.contains(term)) * 8;
        root_semantics += u64::from(description.contains(term)) * 7;
        nested_semantics += u64::from(relationships.contains(term)) * 3;
        nested_semantics += u64::from(fields.contains(term)) * 2;
    }
    SearchRank {
        root_semantics,
        nested_semantics,
        shape: u8::from(shape_intent == Some(entry.operation_shape)),
    }
}

fn search_shape_intent(tokens: &[String]) -> Option<AiCapabilityOperationShape> {
    let contains = |term: &str| tokens.iter().any(|token| token == term);
    let contains_phrase = |left: &str, right: &str| {
        tokens
            .windows(2)
            .any(|pair| pair[0] == left && pair[1] == right)
    };
    if contains("count") || contains("aggregate") || contains_phrase("how", "many") {
        Some(AiCapabilityOperationShape::Aggregate)
    } else if contains("detail") {
        Some(AiCapabilityOperationShape::Details)
    } else if contains("search") {
        Some(AiCapabilityOperationShape::Search)
    } else if contains("keyset") || contains("pagination") || contains("paginated") {
        Some(AiCapabilityOperationShape::KeysetList)
    } else if contains("list") {
        Some(AiCapabilityOperationShape::List)
    } else {
        None
    }
}

fn search_shape_term(term: &str) -> bool {
    matches!(
        term,
        "count"
            | "aggregate"
            | "how"
            | "many"
            | "detail"
            | "search"
            | "keyset"
            | "pagination"
            | "paginated"
            | "list"
    )
}

fn search_terms(value: &str) -> BTreeSet<String> {
    search_tokens(value).into_iter().collect()
}

fn search_tokens(value: &str) -> Vec<String> {
    let bytes = value.as_bytes();
    let mut tokens = Vec::new();
    let mut token = String::new();
    for (index, byte) in bytes.iter().copied().enumerate() {
        if !byte.is_ascii_alphanumeric() {
            push_search_token(&mut tokens, &mut token);
            continue;
        }
        let previous = index
            .checked_sub(1)
            .and_then(|previous| bytes.get(previous));
        let next = bytes.get(index + 1);
        let camel_boundary = !token.is_empty()
            && byte.is_ascii_uppercase()
            && previous.is_some_and(|previous| {
                previous.is_ascii_lowercase()
                    || previous.is_ascii_digit()
                    || (previous.is_ascii_uppercase() && next.is_some_and(u8::is_ascii_lowercase))
            });
        if camel_boundary {
            push_search_token(&mut tokens, &mut token);
        }
        token.push(char::from(byte));
    }
    push_search_token(&mut tokens, &mut token);
    tokens
}

fn push_search_token(tokens: &mut Vec<String>, token: &mut String) {
    if token.is_empty() {
        return;
    }
    let semantic = semantic_key(token);
    token.clear();
    if !semantic.is_empty() {
        tokens.push(semantic);
    }
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

fn valid_sha256_fingerprint(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
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

    fn synthetic_shape_index(entity_count: usize) -> AiCapabilityIndex {
        let semantic = semantic_catalogue();
        let target_id = GraphqlExecutionTargetId::parse("synthetic-application").expect("target");
        let mut entries = BTreeMap::new();
        for entity_index in 0..entity_count {
            let entity = format!("SyntheticEntity{entity_index:03}");
            for shape in [
                AiCapabilityOperationShape::List,
                AiCapabilityOperationShape::Details,
                AiCapabilityOperationShape::Aggregate,
                AiCapabilityOperationShape::Search,
                AiCapabilityOperationShape::KeysetList,
            ] {
                let shape_id = match shape {
                    AiCapabilityOperationShape::List => "list",
                    AiCapabilityOperationShape::Details => "details",
                    AiCapabilityOperationShape::Aggregate => "aggregate",
                    AiCapabilityOperationShape::Search => "search",
                    AiCapabilityOperationShape::KeysetList => "keyset",
                    _ => unreachable!("synthetic shape is fixed"),
                };
                let id = format!("inventory.synthetic_{entity_index:03}_{shape_id}");
                let mut entry = static_entry(
                    descriptor(
                        &id,
                        &format!("Use {entity} records for an operator investigation."),
                        0,
                    ),
                    &target_id,
                    "synthetic-schema-v1",
                    &semantic.fingerprint,
                    "synthetic-policy-v1",
                )
                .expect("synthetic entry");
                entry.kind = AiCapabilityKind::GeneratedQuery;
                entry.name = format!("{} {entity}", shape_name(shape));
                entry.entity_name = Some(entity.clone());
                entry.operation_name = format!("Synthetic{entity_index:03}{shape_id}");
                entry.operation_shape = shape;
                entry.fingerprint = entry_fingerprint(&entry);
                assert!(entries.insert(entry.id.clone(), entry).is_none());
            }
        }
        AiCapabilityIndex {
            version: AI_CAPABILITY_INDEX_VERSION,
            target_id,
            schema_fingerprint: "synthetic-schema-v1".to_owned(),
            semantic_catalogue_fingerprint: semantic.fingerprint,
            target_policy_fingerprint: "synthetic-policy-v1".to_owned(),
            entries,
            fingerprint: "a".repeat(64),
            limits: AiCapabilityIndexLimits::default(),
        }
    }

    #[test]
    fn five_hundred_capabilities_rank_explicit_shape_deterministically_and_stay_bounded() {
        let index = synthetic_shape_index(101);
        assert_eq!(index.entries().len(), 505);
        for (text, expected) in [
            (
                "list SyntheticEntity042 records",
                AiCapabilityOperationShape::List,
            ),
            (
                "details of SyntheticEntity042",
                AiCapabilityOperationShape::Details,
            ),
            (
                "how many SyntheticEntity042 records",
                AiCapabilityOperationShape::Aggregate,
            ),
            (
                "search SyntheticEntity042 records",
                AiCapabilityOperationShape::Search,
            ),
            (
                "pagination for SyntheticEntity042 records",
                AiCapabilityOperationShape::KeysetList,
            ),
        ] {
            let result = index
                .search(&AiCapabilitySearchQuery {
                    text: text.to_owned(),
                    namespace: None,
                    kind: Some(AiCapabilityKind::GeneratedQuery),
                    entity_or_operation: None,
                    maximum_results: 1,
                })
                .expect("shape-aware search");
            assert_eq!(result.candidates.len(), 1);
            assert_eq!(result.candidates[0].operation_shape, expected);
            assert_eq!(
                result.candidates[0].entity_name.as_deref(),
                Some("SyntheticEntity042")
            );
        }

        let query = AiCapabilitySearchQuery {
            text: "list records".to_owned(),
            namespace: None,
            kind: Some(AiCapabilityKind::GeneratedQuery),
            entity_or_operation: None,
            maximum_results: 7,
        };
        let first = index.search(&query).expect("first bounded search");
        let second = index.search(&query).expect("second bounded search");
        assert_eq!(first, second);
        assert_eq!(first.candidates.len(), 7);
        assert!(
            first
                .candidates
                .iter()
                .all(|candidate| candidate.operation_shape == AiCapabilityOperationShape::List)
        );
        assert_eq!(
            first
                .candidates
                .iter()
                .map(|candidate| candidate.id.as_str())
                .collect::<Vec<_>>(),
            (0..7)
                .map(|index| format!("inventory.synthetic_{index:03}_list"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn shape_rank_preserves_lexically_relevant_mixed_shape_candidates() {
        let index = synthetic_shape_index(1);
        let result = index
            .search(&AiCapabilitySearchQuery {
                text: "list SyntheticEntity000 records".to_owned(),
                namespace: None,
                kind: Some(AiCapabilityKind::GeneratedQuery),
                entity_or_operation: None,
                maximum_results: 5,
            })
            .expect("mixed-shape search");
        assert_eq!(result.candidates.len(), 5);
        assert_eq!(
            result.candidates[0].operation_shape,
            AiCapabilityOperationShape::List
        );
        assert_eq!(
            result
                .candidates
                .iter()
                .map(|candidate| candidate.operation_shape)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                AiCapabilityOperationShape::List,
                AiCapabilityOperationShape::Details,
                AiCapabilityOperationShape::Aggregate,
                AiCapabilityOperationShape::Search,
                AiCapabilityOperationShape::KeysetList,
            ])
        );
    }

    #[test]
    fn resolver_description_semantics_outrank_incidental_nested_list_matches() {
        let semantic = semantic_catalogue();
        let target_id = GraphqlExecutionTargetId::parse("communications-service").expect("target");
        let mut mailbox = static_entry(
            descriptor(
                "communications.query.activity",
                "Returns messages from the monitored support mailbox for operator triage.",
                0,
            ),
            &target_id,
            "schema-v1",
            &semantic.fingerprint,
            "policy-v1",
        )
        .expect("mailbox entry");
        mailbox.kind = AiCapabilityKind::GeneratedQuery;
        mailbox.name = "CurrentActivity".to_owned();
        mailbox.operation_name = "CurrentActivity".to_owned();
        mailbox.operation_shape = AiCapabilityOperationShape::Custom;
        mailbox.fingerprint = entry_fingerprint(&mailbox);

        let mut records = static_entry(
            descriptor("records.query.customer_cards", "List customer records.", 0),
            &target_id,
            "schema-v1",
            &semantic.fingerprint,
            "policy-v1",
        )
        .expect("record entry");
        records.kind = AiCapabilityKind::GeneratedQuery;
        records.name = "List CustomerRecord".to_owned();
        records.entity_name = Some("CustomerRecord".to_owned());
        records.operation_name = "CustomerRecords".to_owned();
        records.operation_shape = AiCapabilityOperationShape::List;
        records.relationships = vec![AiCapabilityRelationshipSummary {
            name: "Assignments".to_owned(),
            description: "Recent support activity associated with the customer.".to_owned(),
            target_entity: "Assignment".to_owned(),
            to_many: true,
            arguments: BTreeSet::new(),
        }];
        records.fingerprint = entry_fingerprint(&records);

        let entries =
            BTreeMap::from([(mailbox.id.clone(), mailbox), (records.id.clone(), records)]);
        let index = AiCapabilityIndex {
            version: AI_CAPABILITY_INDEX_VERSION,
            target_id,
            schema_fingerprint: "schema-v1".to_owned(),
            semantic_catalogue_fingerprint: semantic.fingerprint,
            target_policy_fingerprint: "policy-v1".to_owned(),
            entries,
            fingerprint: "a".repeat(64),
            limits: AiCapabilityIndexLimits::default(),
        };
        let result = index
            .search(&AiCapabilitySearchQuery {
                text: "list recent messages from the support mailbox".to_owned(),
                namespace: None,
                kind: Some(AiCapabilityKind::GeneratedQuery),
                entity_or_operation: None,
                maximum_results: 1,
            })
            .expect("resolver-hint search");
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(
            result.candidates[0].id.as_str(),
            "communications.query.activity"
        );
    }

    #[test]
    fn camel_case_operation_names_supply_searchable_semantic_terms() {
        let semantic = semantic_catalogue();
        let target_id = GraphqlExecutionTargetId::parse("application").expect("target");
        let mut entry = static_entry(
            descriptor("application.query.activity", "Returns bounded records.", 0),
            &target_id,
            "schema-v1",
            &semantic.fingerprint,
            "policy-v1",
        )
        .expect("entry");
        entry.kind = AiCapabilityKind::GeneratedQuery;
        entry.name = "MonitoredMailboxMessages".to_owned();
        entry.operation_name = "MonitoredMailboxMessages".to_owned();
        entry.fingerprint = entry_fingerprint(&entry);
        let index = AiCapabilityIndex {
            version: AI_CAPABILITY_INDEX_VERSION,
            target_id,
            schema_fingerprint: "schema-v1".to_owned(),
            semantic_catalogue_fingerprint: semantic.fingerprint,
            target_policy_fingerprint: "policy-v1".to_owned(),
            entries: BTreeMap::from([(entry.id.clone(), entry)]),
            fingerprint: "b".repeat(64),
            limits: AiCapabilityIndexLimits::default(),
        };
        let result = index
            .search(&AiCapabilitySearchQuery {
                text: "mailbox messages".to_owned(),
                namespace: None,
                kind: Some(AiCapabilityKind::GeneratedQuery),
                entity_or_operation: None,
                maximum_results: 1,
            })
            .expect("camel-case search");
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(
            result.candidates[0].operation_name,
            "MonitoredMailboxMessages"
        );
    }

    #[test]
    fn ordinary_request_vocabulary_does_not_infer_operation_shape() {
        for text in [
            "total value",
            "number assigned",
            "single owner",
            "specific policy",
            "find matching records",
            "lookup code",
            "next action",
            "page content",
            "all current records",
            "latest revision",
            "recent incident",
            "many records explain how they changed",
        ] {
            assert_eq!(
                search_shape_intent(&search_tokens(text)),
                None,
                "unexpected shape intent for {text:?}"
            );
        }
        for (text, expected) in [
            ("count records", AiCapabilityOperationShape::Aggregate),
            ("how many records", AiCapabilityOperationShape::Aggregate),
            ("record details", AiCapabilityOperationShape::Details),
            ("search records", AiCapabilityOperationShape::Search),
            ("paginated records", AiCapabilityOperationShape::KeysetList),
            ("list records", AiCapabilityOperationShape::List),
        ] {
            assert_eq!(
                search_shape_intent(&search_tokens(text)),
                Some(expected),
                "missing explicit shape intent for {text:?}"
            );
        }
    }

    #[test]
    fn unrelated_query_cannot_admit_zero_lexical_candidates_at_high_cardinality() {
        let index = synthetic_shape_index(101);
        let result = index
            .search(&AiCapabilitySearchQuery {
                text: "list unrelated phrase".to_owned(),
                namespace: None,
                kind: Some(AiCapabilityKind::GeneratedQuery),
                entity_or_operation: None,
                maximum_results: 32,
            })
            .expect("unrelated search");
        assert!(result.candidates.is_empty());
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

    #[test]
    fn index_set_searches_multiple_targets_deterministically() {
        let semantic = semantic_catalogue();
        let compile = |target: &str, descriptor: AiToolDescriptor| {
            Arc::new(
                AiCapabilityIndex::compile(
                    GraphqlExecutionTargetId::parse(target).expect("target"),
                    format!("schema-{target}"),
                    &semantic,
                    None,
                    None,
                    None,
                    [descriptor],
                    format!("policy-{target}"),
                    AiCapabilityIndexLimits::default(),
                )
                .expect("index"),
            )
        };
        let jim = compile(
            "jim",
            descriptor("jim.jobs", "List recent jobs and Actions.", 0),
        );
        let fame = compile(
            "fame",
            descriptor("fame.endpoints", "List connected managed endpoints.", 0),
        );
        let first = AiCapabilityIndexSet::compile([jim.clone(), fame.clone()]).expect("set");
        let second = AiCapabilityIndexSet::compile([fame.clone(), jim.clone()]).expect("set");
        assert_eq!(first.fingerprint(), second.fingerprint());
        assert_eq!(first.indexes().len(), 2);
        assert_eq!(
            first
                .owning_index(&AiToolId::parse("jim.jobs").unwrap())
                .unwrap()
                .target_id()
                .as_str(),
            "jim"
        );
        let jobs = first
            .search(&AiCapabilitySearchQuery {
                text: "recent jobs".to_owned(),
                namespace: None,
                kind: None,
                entity_or_operation: None,
                maximum_results: 2,
            })
            .expect("search");
        assert_eq!(jobs.index_set_fingerprint, first.fingerprint());
        assert_eq!(jobs.candidates[0].id.as_str(), "jim.jobs");
        let endpoints = first
            .search(&AiCapabilitySearchQuery {
                text: "connected endpoints".to_owned(),
                namespace: None,
                kind: None,
                entity_or_operation: None,
                maximum_results: 2,
            })
            .expect("search");
        assert_eq!(endpoints.candidates[0].id.as_str(), "fame.endpoints");
        assert!(
            AiCapabilityIndexSet::compile_with_limits(
                [jim.clone(), fame.clone()],
                AiCapabilityIndexSetLimits {
                    maximum_entries: 1,
                    ..AiCapabilityIndexSetLimits::default()
                },
            )
            .is_err()
        );
        assert!(
            AiCapabilityIndexSet::compile_with_limits(
                [jim, fame],
                AiCapabilityIndexSetLimits {
                    maximum_total_bytes: 1_024,
                    ..AiCapabilityIndexSetLimits::default()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn index_set_search_ranks_execution_target_before_lexical_ties() {
        let semantic = semantic_catalogue();
        let compile = |target: &str, id: &str| {
            Arc::new(
                AiCapabilityIndex::compile(
                    GraphqlExecutionTargetId::parse(target).expect("target"),
                    format!("schema-{target}"),
                    &semantic,
                    None,
                    None,
                    None,
                    [descriptor(id, "Inspect matching operational records.", 0)],
                    format!("policy-{target}"),
                    AiCapabilityIndexLimits::default(),
                )
                .expect("index"),
            )
        };
        let set = AiCapabilityIndexSet::compile([
            compile("alpha-service", "inventory.records"),
            compile("beta-service", "workforce.records"),
        ])
        .expect("set");
        let result = set
            .search(&AiCapabilitySearchQuery {
                text: "beta service operational records".to_owned(),
                namespace: None,
                kind: None,
                entity_or_operation: None,
                maximum_results: 1,
            })
            .expect("search");
        assert_eq!(result.candidates[0].id.as_str(), "workforce.records");
    }

    #[test]
    fn index_set_rejects_cross_target_capability_collisions() {
        let semantic = semantic_catalogue();
        let compile = |target: &str| {
            Arc::new(
                AiCapabilityIndex::compile(
                    GraphqlExecutionTargetId::parse(target).expect("target"),
                    format!("schema-{target}"),
                    &semantic,
                    None,
                    None,
                    None,
                    [descriptor("shared.lookup", "Look up records.", 0)],
                    format!("policy-{target}"),
                    AiCapabilityIndexLimits::default(),
                )
                .expect("index"),
            )
        };
        assert!(AiCapabilityIndexSet::compile([compile("first"), compile("second")]).is_err());
    }
}
