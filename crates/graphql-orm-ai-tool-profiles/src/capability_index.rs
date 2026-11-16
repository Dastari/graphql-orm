//! Compact, deterministic discovery metadata for reviewed capabilities.
//!
//! The index deliberately contains no executable document, argument schema,
//! resolver location, database coordinate, credential, or authority. A match
//! is descriptive only; callers must reauthorize and load the exact current
//! capability before execution.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, OnceLock},
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
///
/// Version 3 groups every generated query root of one entity into a single
/// [`AiCapabilityKind::GeneratedEntity`] entry, so the index holds one entry
/// per entity instead of one per root.
pub const AI_CAPABILITY_INDEX_VERSION: u16 = 3;

/// Version of the entity-capability identity and fingerprint.
pub const AI_CAPABILITY_ENTITY_VERSION: u16 = 1;

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
    /// Entity-centric generated read capability: every generated query root
    /// of one entity in one target. The root to run is chosen per call by
    /// its operation name; each root keeps its own
    /// [`AiCapabilityKind::GeneratedQuery`] identity, fingerprint and
    /// authorization.
    GeneratedEntity,
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
    /// Entity-centric capability whose operation is chosen per call.
    Entity,
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
    /// Generated query roots of an [`AiCapabilityKind::GeneratedEntity`]
    /// entry, in operation-name order. Empty for every other kind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<AiCapabilityOperationMember>,
    /// Fingerprint of all preceding entry metadata.
    pub fingerprint: String,
}

/// One generated query root of an entity capability.
///
/// A member is the compact form of the root's own
/// [`AiCapabilityKind::GeneratedQuery`] entry. That entry is still resolved by
/// [`AiCapabilityIndex::entry`] with the member's `capability_id`, and keeps
/// the identity, fingerprint and host-policy checks it had as a standalone
/// entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiCapabilityOperationMember {
    /// Stable capability identifier of the root.
    pub capability_id: AiToolId,
    /// Public GraphQL root field name, used as the operation argument.
    pub operation_name: String,
    /// Operation shape of the root.
    pub operation_shape: AiCapabilityOperationShape,
    /// Model-safe description of the root.
    pub description: String,
    /// Bounded result description of the root.
    pub result_description: String,
    /// Conservative compiler-owned result-record planning bounds.
    pub result_record_cost: AiGraphqlResultRecordCostEstimate,
    /// Risk classification of the root.
    pub risk: AiToolRisk,
    /// Approval classification of the root.
    pub approval: AiApprovalRule,
    /// Exact executable capability fingerprint of the root.
    pub capability_fingerprint: String,
    /// Exact source catalogue fingerprint of the root.
    pub catalogue_fingerprint: String,
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
    /// Search terms derived from `entries`. Never serialized or compared:
    /// it is rebuilt from the entries on first use after deserialization.
    #[serde(skip)]
    search_terms: SearchTermCache,
    /// Root entries of every entity entry's members, derived from `entries`.
    /// Never serialized, compared or counted against the limits.
    #[serde(skip)]
    operation_entries: OperationEntryCache,
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
            // Roots of one entity become one entity entry; a root without
            // an entity stays a standalone entry.
            let mut by_entity = BTreeMap::<String, Vec<AiCapabilityIndexEntry>>::new();
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
                match entry.entity_name.clone() {
                    Some(entity) => by_entity.entry(entity).or_default().push(entry),
                    None => insert_entry(&mut entries, entry)?,
                }
            }
            for (entity, members) in by_entity {
                let description = entities
                    .get(entity.as_str())
                    .map(|metadata| metadata.description.as_str())
                    .unwrap_or_default();
                let entry = entity_entry(&entity, description, members, catalogue.fingerprint())?;
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
        let index = Self {
            version: AI_CAPABILITY_INDEX_VERSION,
            target_id,
            schema_fingerprint,
            semantic_catalogue_fingerprint: semantic_catalogue.fingerprint.clone(),
            target_policy_fingerprint,
            entries,
            fingerprint,
            limits,
            search_terms: SearchTermCache::default(),
            operation_entries: OperationEntryCache::default(),
        };
        let member_ids = index
            .entries
            .values()
            .flat_map(|entry| entry.operations.iter().map(|member| &member.capability_id))
            .collect::<Vec<_>>();
        if member_ids.iter().any(|id| index.entries.contains_key(*id))
            || member_ids.iter().collect::<BTreeSet<_>>().len() != member_ids.len()
        {
            return Err(configuration_error("duplicate capability index ID"));
        }
        // Tokenise every entry once here, so no search does.
        index.search_terms();
        index.operation_entries();
        Ok(index)
    }

    /// Root entries of every entity entry's members, keyed by root ID, each
    /// with the ID of the entity entry that holds it.
    fn operation_entries(&self) -> &BTreeMap<AiToolId, (AiToolId, AiCapabilityIndexEntry)> {
        self.operation_entries.0.get_or_init(|| {
            Arc::new(
                self.entries
                    .values()
                    .flat_map(|entity| {
                        entity.operations.iter().map(|member| {
                            (
                                member.capability_id.clone(),
                                (entity.id.clone(), member_entry(entity, member)),
                            )
                        })
                    })
                    .collect(),
            )
        })
    }

    /// Precomputed search terms of every entry, in entry order.
    fn search_terms(&self) -> &SearchTermIndex {
        self.search_terms
            .0
            .get_or_init(|| Arc::new(SearchTermIndex::compile(&self.target_id, &self.entries)))
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

    /// Every discoverable entry in stable capability-ID order.
    ///
    /// The generated query roots grouped into an
    /// [`AiCapabilityKind::GeneratedEntity`] entry are not listed here; see
    /// [`Self::operation_entry`] and [`Self::entry`].
    pub fn entries(&self) -> impl ExactSizeIterator<Item = &AiCapabilityIndexEntry> {
        self.entries.values()
    }

    /// Resolves one exact entry: a discoverable entry, or the standalone
    /// [`AiCapabilityKind::GeneratedQuery`] entry of a root that an entity
    /// entry groups. This lookup grants no authority.
    pub fn entry(&self, id: &AiToolId) -> Option<&AiCapabilityIndexEntry> {
        self.entries
            .get(id)
            .or_else(|| self.operation_entries().get(id).map(|(_, entry)| entry))
    }

    /// Resolves the entity entry that groups one generated query root.
    pub fn entity_entry_of(&self, root_id: &AiToolId) -> Option<&AiCapabilityIndexEntry> {
        let (entity, _) = self.operation_entries().get(root_id)?;
        self.entries.get(entity)
    }

    /// Resolves the root entry of one operation of an entity entry, by the
    /// operation's public root field name. This lookup grants no authority.
    pub fn operation_entry(
        &self,
        entity_id: &AiToolId,
        operation_name: &str,
    ) -> Option<&AiCapabilityIndexEntry> {
        let member = self
            .entries
            .get(entity_id)?
            .operations
            .iter()
            .find(|member| member.operation_name == operation_name)?;
        self.entry(&member.capability_id)
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
        let candidates = rank_entries([self], query)
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
        // Discoverable entries only: the roots an entity entry groups are
        // part of that entry and are not counted again.
        let mut total_entries = 0_usize;
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
            let member_ids = index
                .entries
                .values()
                .flat_map(|entry| entry.operations.iter().map(|member| &member.capability_id));
            for id in index.entries.keys().chain(member_ids) {
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
            total_entries = total_entries.saturating_add(index.entries.len());
            maximum_search_results =
                maximum_search_results.min(index.limits.maximum_search_results);
            canonical.insert(target_id, index);
            if canonical.len() > usize::from(limits.maximum_indexes) {
                return Err(configuration_error(
                    "capability index set contains too many targets",
                ));
            }
            if total_entries > limits.maximum_entries as usize {
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
    ///
    /// The result ceiling is applied to the authority-neutral ranking. A caller
    /// that filters candidates through current host policy must rank with
    /// [`Self::ranked_matches`] and apply the ceiling after its policy, or a
    /// restricted principal can receive fewer candidates than it may use.
    pub fn search(
        &self,
        query: &AiCapabilitySearchQuery,
    ) -> Result<AiCapabilityIndexSetSearchResult, AiError> {
        let candidates = self
            .ranked_matches(query)?
            .into_iter()
            .take(usize::from(query.maximum_results))
            .map(|candidate| candidate.candidate())
            .collect();
        Ok(AiCapabilityIndexSetSearchResult {
            index_set_fingerprint: self.fingerprint.clone(),
            candidates,
        })
    }

    /// Ranks every entry that matches one validated query, without applying
    /// the query's result ceiling.
    ///
    /// Matches are in the same deterministic order [`Self::search`] uses.
    /// Each rank depends only on the query and that entry's own metadata, so
    /// removing other entries from the result cannot change the relative
    /// order of the entries that remain. Callers that apply host policy
    /// filter this complete ranking first and truncate afterwards.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid query or result ceiling. Ranking emits
    /// model-safe metadata only and grants no authority.
    pub fn ranked_matches(
        &self,
        query: &AiCapabilitySearchQuery,
    ) -> Result<Vec<AiCapabilityRankedMatch<'_>>, AiError> {
        query.validate(self.maximum_search_results)?;
        Ok(rank_entries(self.indexes.values().map(Arc::as_ref), query)
            .into_iter()
            .map(|(index, entry)| AiCapabilityRankedMatch { index, entry })
            .collect())
    }

    /// Ranks like [`Self::ranked_matches`], then replaces every matching
    /// [`AiCapabilityKind::GeneratedEntity`] entry with the root entries it
    /// groups, so each match is one executable root.
    ///
    /// The roots of one entity follow its rank: roots of the query's
    /// operation shape first, then in operation-name order. The query's kind
    /// and entity-or-operation filters apply to the roots. This is the
    /// per-root ranking that delivery modes installing one exact definition
    /// per root use.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid query or result ceiling. Ranking emits
    /// model-safe metadata only and grants no authority.
    pub fn ranked_operation_matches(
        &self,
        query: &AiCapabilitySearchQuery,
    ) -> Result<Vec<AiCapabilityRankedMatch<'_>>, AiError> {
        query.validate(self.maximum_search_results)?;
        Ok(
            rank_operation_entries(self.indexes.values().map(Arc::as_ref), query)
                .into_iter()
                .map(|(index, entry)| AiCapabilityRankedMatch { index, entry })
                .collect(),
        )
    }
}

/// One ranked query match and its exact owning index.
///
/// A match is authority-neutral discovery metadata. It proves only that the
/// entry belongs to the index set it was ranked from.
#[derive(Clone, Copy, Debug)]
pub struct AiCapabilityRankedMatch<'a> {
    index: &'a AiCapabilityIndex,
    entry: &'a AiCapabilityIndexEntry,
}

impl<'a> AiCapabilityRankedMatch<'a> {
    /// Exact owning index of the matched entry.
    pub const fn index(&self) -> &'a AiCapabilityIndex {
        self.index
    }

    /// Exact matched entry.
    pub const fn entry(&self) -> &'a AiCapabilityIndexEntry {
        self.entry
    }

    /// Compact model-safe search candidate for the matched entry.
    pub fn candidate(&self) -> AiCapabilitySearchCandidate {
        search_candidate(self.entry)
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
    /// Operations of an [`AiCapabilityKind::GeneratedEntity`] candidate, in
    /// operation-name order. Empty for every other kind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<AiCapabilityCandidateOperation>,
}

/// One operation of an entity candidate: its public root and shape only.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiCapabilityCandidateOperation {
    /// Public GraphQL root field name.
    pub operation_name: String,
    /// Operation shape.
    pub operation_shape: AiCapabilityOperationShape,
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

/// Fixed per-field weights of one search term in one entry: one posting.
///
/// Scores are per entry and fixed-weight by design. They never depend on how
/// many other entries hold a term, so the rank of an entry a principal may see
/// cannot reveal anything about entries it may not. No inverse document
/// frequency, BM25 or other catalogue-wide statistic is computed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Posting {
    /// Position of the entry in the owning index's entry order.
    entry: u32,
    /// Summed weights of the root fields (ID, name, operation, entity,
    /// target, namespace, description) that hold the term.
    root: u16,
    /// Summed weights of the nested fields (relationships, scalar fields)
    /// that hold the term.
    nested: u8,
}

/// Filter keys of one entry, computed once when its index is compiled.
#[derive(Clone, Debug, Eq, PartialEq)]
struct EntrySearchKeys {
    /// Identifier of the entry at this position.
    id: AiToolId,
    /// Semantic keys of the entry's operation name and, for an entity entry,
    /// of every operation it groups.
    operation_keys: BTreeSet<String>,
    entity_key: Option<String>,
    /// The entry's own shape and, for an entity entry, every grouped shape.
    shapes: BTreeSet<AiCapabilityOperationShape>,
}

/// Inverted search index of one capability index: term to postings.
///
/// Terms are interned per index. A query resolves each of its terms once and
/// scores only the entries in those terms' posting lists, so a search costs
/// O(matching postings) instead of O(entries).
#[derive(Clone, Debug, Eq, PartialEq)]
struct SearchTermIndex {
    dictionary: BTreeMap<String, u32>,
    /// Indexed by term; each list is in entry order.
    postings: Vec<Vec<Posting>>,
    /// Indexed by entry position.
    entries: Vec<EntrySearchKeys>,
}

impl SearchTermIndex {
    fn compile(
        target_id: &GraphqlExecutionTargetId,
        entries: &BTreeMap<AiToolId, AiCapabilityIndexEntry>,
    ) -> Self {
        let mut dictionary = BTreeMap::<String, u32>::new();
        let mut postings = Vec::<Vec<Posting>>::new();
        let target = search_terms(target_id.as_str());
        let mut keys = Vec::with_capacity(entries.len());
        for (position, entry) in entries.values().enumerate() {
            let position = u32::try_from(position).unwrap_or(u32::MAX);
            let mut weights = BTreeMap::<u32, (u16, u8)>::new();
            let mut add = |terms: &BTreeSet<String>, root: u16, nested: u8| {
                for term in terms {
                    // A shape word in a query selects a shape and never
                    // scores, so it is not stored.
                    if search_shape_term(term) {
                        continue;
                    }
                    let id = match dictionary.get(term) {
                        Some(id) => *id,
                        None => {
                            let id = u32::try_from(dictionary.len()).unwrap_or(u32::MAX);
                            dictionary.insert(term.clone(), id);
                            postings.push(Vec::new());
                            id
                        }
                    };
                    let weight = weights.entry(id).or_default();
                    weight.0 += root;
                    weight.1 += nested;
                }
            };
            add(&search_terms(entry.id.as_str()), 12, 0);
            add(&search_terms(&entry.name), 11, 0);
            // An entity entry is found by any of its operations' names and
            // descriptions, each counted once per field like a root's own.
            let mut operation = search_terms(&entry.operation_name);
            let mut description = search_terms(&entry.description);
            for member in &entry.operations {
                operation.extend(search_terms(&member.operation_name));
                description.extend(search_terms(&member.description));
            }
            add(&operation, 10, 0);
            if let Some(entity) = &entry.entity_name {
                add(&search_terms(entity), 9, 0);
            }
            add(&target, 8, 0);
            add(&search_terms(&entry.namespace), 8, 0);
            add(&description, 7, 0);
            add(
                &entry
                    .relationships
                    .iter()
                    .flat_map(|relationship| {
                        search_terms(&format!(
                            "{} {} {}",
                            relationship.name, relationship.description, relationship.target_entity
                        ))
                    })
                    .collect(),
                0,
                3,
            );
            add(
                &entry
                    .scalar_fields
                    .iter()
                    .flat_map(|field| {
                        search_terms(&format!("{} {}", field.name, field.description))
                    })
                    .collect(),
                0,
                2,
            );
            for (term, (root, nested)) in weights {
                postings[term as usize].push(Posting {
                    entry: position,
                    root,
                    nested,
                });
            }
            keys.push(EntrySearchKeys {
                id: entry.id.clone(),
                operation_keys: std::iter::once(semantic_key(&entry.operation_name))
                    .chain(
                        entry
                            .operations
                            .iter()
                            .map(|member| semantic_key(&member.operation_name)),
                    )
                    .collect(),
                entity_key: entry.entity_name.as_deref().map(semantic_key),
                shapes: std::iter::once(entry.operation_shape)
                    .chain(entry.operations.iter().map(|member| member.operation_shape))
                    .collect(),
            });
        }
        Self {
            dictionary,
            postings,
            entries: keys,
        }
    }
}

/// Lazily built, never serialized, never compared cache of the root entries
/// that entity entries group.
#[derive(Clone, Debug, Default)]
#[allow(clippy::type_complexity)]
struct OperationEntryCache(OnceLock<Arc<BTreeMap<AiToolId, (AiToolId, AiCapabilityIndexEntry)>>>);

impl PartialEq for OperationEntryCache {
    fn eq(&self, _: &Self) -> bool {
        // Derived from the entries, which the owning index already compares.
        true
    }
}

impl Eq for OperationEntryCache {}

/// Lazily built, never serialized, never compared cache of search terms.
#[derive(Clone, Debug, Default)]
struct SearchTermCache(OnceLock<Arc<SearchTermIndex>>);

impl PartialEq for SearchTermCache {
    fn eq(&self, _: &Self) -> bool {
        // Derived from the entries, which the owning index already compares.
        true
    }
}

impl Eq for SearchTermCache {}

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

/// Ranks every matching entry of `indexes` against one query.
///
/// Only the query text is tokenised. Each entry is scored from the postings
/// of the query's terms, precomputed when its index was compiled, with fixed
/// per-field weights. An entry's score depends only on the query and that
/// entry's own metadata, never on any other entry.
fn rank_entries<'a>(
    indexes: impl IntoIterator<Item = &'a AiCapabilityIndex>,
    query: &AiCapabilitySearchQuery,
) -> Vec<(&'a AiCapabilityIndex, &'a AiCapabilityIndexEntry)> {
    ranked_entries(indexes, query)
        .into_iter()
        .map(|(_, index, entry)| (index, entry))
        .collect()
}

fn ranked_entries<'a>(
    indexes: impl IntoIterator<Item = &'a AiCapabilityIndex>,
    query: &AiCapabilitySearchQuery,
) -> Vec<(
    SearchRank,
    &'a AiCapabilityIndex,
    &'a AiCapabilityIndexEntry,
)> {
    let tokens = search_tokens(&query.text);
    let shape_intent = search_shape_intent(&tokens);
    let terms = tokens
        .into_iter()
        .filter(|term| !search_shape_term(term))
        .collect::<BTreeSet<_>>();
    let name_key = query.entity_or_operation.as_deref().map(semantic_key);
    let mut ranked = Vec::new();
    for index in indexes {
        let searchable = index.search_terms();
        let mut scores = BTreeMap::<u32, (u64, u64)>::new();
        for term in &terms {
            let Some(id) = searchable.dictionary.get(term) else {
                continue;
            };
            for posting in &searchable.postings[*id as usize] {
                let score = scores.entry(posting.entry).or_default();
                score.0 += u64::from(posting.root);
                score.1 += u64::from(posting.nested);
            }
        }
        if scores.is_empty() {
            // No entry of this index holds any query term.
            continue;
        }
        for (position, (root_semantics, nested_semantics)) in scores {
            let keys = &searchable.entries[position as usize];
            let Some(entry) = index.entries.get(&keys.id) else {
                continue;
            };
            if query
                .namespace
                .as_ref()
                .is_some_and(|namespace| &entry.namespace != namespace)
                || query.kind.is_some_and(|kind| entry.kind != kind)
                || name_key.as_ref().is_some_and(|name| {
                    !keys.operation_keys.contains(name) && keys.entity_key.as_ref() != Some(name)
                })
            {
                continue;
            }
            let rank = SearchRank {
                root_semantics,
                nested_semantics,
                shape: u8::from(shape_intent.is_some_and(|shape| keys.shapes.contains(&shape))),
            };
            if rank.has_semantic_match() {
                ranked.push((rank, index, entry));
            }
        }
    }
    ranked.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.2.id.cmp(&right.2.id))
    });
    ranked
}

/// Ranks matching entries, then replaces each entity entry with the root
/// entries it groups: roots of the query's shape first, then in operation
/// order. Kind and name filters apply to the roots, not to the entity entry.
fn rank_operation_entries<'a>(
    indexes: impl IntoIterator<Item = &'a AiCapabilityIndex>,
    query: &AiCapabilitySearchQuery,
) -> Vec<(&'a AiCapabilityIndex, &'a AiCapabilityIndexEntry)> {
    let mut grouped = query.clone();
    grouped.kind = None;
    grouped.entity_or_operation = None;
    let shape_intent = search_shape_intent(&search_tokens(&query.text));
    let name_key = query.entity_or_operation.as_deref().map(semantic_key);
    let mut expanded = Vec::new();
    for (_, index, entry) in ranked_entries(indexes, &grouped) {
        let roots = if entry.kind == AiCapabilityKind::GeneratedEntity {
            let mut members = entry.operations.iter().collect::<Vec<_>>();
            members.sort_by_key(|member| Some(member.operation_shape) != shape_intent);
            members
                .into_iter()
                .filter_map(|member| index.entry(&member.capability_id))
                .collect::<Vec<_>>()
        } else {
            vec![entry]
        };
        for root in roots {
            if query.kind.is_some_and(|kind| root.kind != kind)
                || name_key.as_ref().is_some_and(|name| {
                    &semantic_key(&root.operation_name) != name
                        && root.entity_name.as_deref().map(semantic_key).as_ref() != Some(name)
                })
            {
                continue;
            }
            expanded.push((index, root));
        }
    }
    expanded
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
        operations: entry
            .operations
            .iter()
            .map(|member| AiCapabilityCandidateOperation {
                operation_name: member.operation_name.clone(),
                operation_shape: member.operation_shape,
            })
            .collect(),
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
        operations: Vec::new(),
        fingerprint: String::new(),
    };
    validate_entry_text(&entry)?;
    entry.fingerprint = entry_fingerprint(&entry);
    Ok(entry)
}

/// Groups the generated query roots of one entity into one entity entry.
///
/// Every root entry must carry the same entity summaries, which
/// [`generated_entry`] guarantees for roots of one entity. Each root stays
/// resolvable as its own entry through [`member_entry`], which must rebuild
/// it exactly.
fn entity_entry(
    entity: &str,
    entity_description: &str,
    mut roots: Vec<AiCapabilityIndexEntry>,
    catalogue_fingerprint: &str,
) -> Result<AiCapabilityIndexEntry, AiError> {
    roots.sort_by(|left, right| {
        left.operation_name
            .cmp(&right.operation_name)
            .then_with(|| left.id.cmp(&right.id))
    });
    let first = roots
        .first()
        .ok_or_else(|| configuration_error("entity capability has no operations"))?;
    let namespace = first.namespace.clone();
    if roots.iter().any(|root| {
        root.namespace != namespace
            || root.scalar_fields != first.scalar_fields
            || root.relationships != first.relationships
            || root.aggregate_features != first.aggregate_features
            || root.result_classification != first.result_classification
            || root.schema_fingerprint != first.schema_fingerprint
            || root.target_policy_fingerprint != first.target_policy_fingerprint
    }) {
        return Err(configuration_error(
            "entity capability operations disagree on their entity",
        ));
    }
    let members = roots
        .iter()
        .map(|root| AiCapabilityOperationMember {
            capability_id: root.id.clone(),
            operation_name: root.operation_name.clone(),
            operation_shape: root.operation_shape,
            description: root.description.clone(),
            result_description: root.result_description.clone(),
            result_record_cost: root.result_record_cost,
            risk: root.risk,
            approval: root.approval,
            capability_fingerprint: root.capability_fingerprint.clone(),
            catalogue_fingerprint: root.catalogue_fingerprint.clone(),
        })
        .collect::<Vec<_>>();
    if members
        .windows(2)
        .any(|pair| pair[0].operation_name == pair[1].operation_name)
    {
        return Err(configuration_error(
            "entity capability has duplicate operation names",
        ));
    }
    let id = AiToolId::parse(entity_capability_id(&namespace, entity))?;
    let shapes = members
        .iter()
        .map(|member| shape_name(member.operation_shape).to_lowercase())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(", ");
    let result_record_cost = members.iter().fold(
        AiGraphqlResultRecordCostEstimate {
            maximum_root_records: 0,
            maximum_total_records: 0,
            root_bound_required: false,
        },
        |total, member| AiGraphqlResultRecordCostEstimate {
            maximum_root_records: total
                .maximum_root_records
                .max(member.result_record_cost.maximum_root_records),
            maximum_total_records: total
                .maximum_total_records
                .max(member.result_record_cost.maximum_total_records),
            root_bound_required: total.root_bound_required
                || member.result_record_cost.root_bound_required,
        },
    );
    let capability_fingerprint = sha256_json(&json!({
        "version": AI_CAPABILITY_ENTITY_VERSION,
        "kind": "generated_entity",
        "id": id.as_str(),
        "entity": entity,
        "operations": members
            .iter()
            .map(|member| json!({
                "id": member.capability_id.as_str(),
                "operation": member.operation_name,
                "fingerprint": member.capability_fingerprint,
            }))
            .collect::<Vec<_>>(),
    }));
    let mut entry = AiCapabilityIndexEntry {
        id,
        kind: AiCapabilityKind::GeneratedEntity,
        name: format!("{entity} records"),
        description: if entity_description.trim().is_empty() {
            format!("Read {entity} records.")
        } else {
            entity_description.to_owned()
        },
        namespace,
        entity_name: Some(entity.to_owned()),
        operation_name: entity.to_owned(),
        operation_shape: AiCapabilityOperationShape::Entity,
        scalar_fields: first.scalar_fields.clone(),
        relationships: first.relationships.clone(),
        aggregate_features: first.aggregate_features.clone(),
        result_classification: first.result_classification,
        result_description: format!(
            "Bounded {entity} reads ({shapes}); choose one operation per call."
        ),
        result_record_cost,
        risk: members
            .iter()
            .map(|member| member.risk)
            .max()
            .unwrap_or(AiToolRisk::ReadOnly),
        approval: if members
            .iter()
            .all(|member| member.approval == AiApprovalRule::None)
        {
            AiApprovalRule::None
        } else {
            return Err(configuration_error(
                "entity capability operation requires approval",
            ));
        },
        capability_fingerprint,
        catalogue_fingerprint: catalogue_fingerprint.to_owned(),
        schema_fingerprint: first.schema_fingerprint.clone(),
        target_policy_fingerprint: first.target_policy_fingerprint.clone(),
        operations: members,
        fingerprint: String::new(),
    };
    validate_entry_text(&entry)?;
    entry.fingerprint = entry_fingerprint(&entry);
    for root in &roots {
        let member = entry
            .operations
            .iter()
            .find(|member| member.capability_id == root.id)
            .ok_or_else(|| configuration_error("entity capability lost an operation"))?;
        if &member_entry(&entry, member) != root {
            return Err(configuration_error(
                "entity capability cannot reproduce an operation entry",
            ));
        }
    }
    Ok(entry)
}

/// Rebuilds the standalone [`AiCapabilityKind::GeneratedQuery`] entry of one
/// member of an entity entry, exactly as [`generated_entry`] produced it.
fn member_entry(
    entity: &AiCapabilityIndexEntry,
    member: &AiCapabilityOperationMember,
) -> AiCapabilityIndexEntry {
    let entity_name = entity.entity_name.clone();
    let mut entry = AiCapabilityIndexEntry {
        id: member.capability_id.clone(),
        kind: AiCapabilityKind::GeneratedQuery,
        name: entity_name.as_deref().map_or_else(
            || member.operation_name.clone(),
            |name| format!("{} {}", shape_name(member.operation_shape), name),
        ),
        description: member.description.clone(),
        namespace: id_namespace(&member.capability_id),
        entity_name,
        operation_name: member.operation_name.clone(),
        operation_shape: member.operation_shape,
        scalar_fields: entity.scalar_fields.clone(),
        relationships: entity.relationships.clone(),
        aggregate_features: entity.aggregate_features.clone(),
        result_classification: entity.result_classification,
        result_description: member.result_description.clone(),
        result_record_cost: member.result_record_cost,
        risk: member.risk,
        approval: member.approval,
        capability_fingerprint: member.capability_fingerprint.clone(),
        catalogue_fingerprint: member.catalogue_fingerprint.clone(),
        schema_fingerprint: entity.schema_fingerprint.clone(),
        target_policy_fingerprint: entity.target_policy_fingerprint.clone(),
        operations: Vec::new(),
        fingerprint: String::new(),
    };
    entry.fingerprint = entry_fingerprint(&entry);
    entry
}

/// Stable identifier of one entity capability. It never collides with a
/// root identifier, which always names a root kind as its second segment.
fn entity_capability_id(namespace: &str, entity: &str) -> String {
    let identity = format!("{namespace}\0entity\0{entity}\0grouped-v1");
    let hash = hex::encode(Sha256::digest(identity.as_bytes()));
    format!(
        "{}.entity.{}.grouped-{}",
        namespace.to_ascii_lowercase(),
        entity.to_ascii_lowercase(),
        &hash[..16]
    )
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
        operations: Vec::new(),
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
        AiCapabilityOperationShape::Entity => "Read",
    }
}

fn generated_risk(
    kind: AiCapabilityKind,
    operation: &GraphqlSemanticOperationDescriptor,
) -> AiToolRisk {
    match kind {
        AiCapabilityKind::GeneratedQuery
        | AiCapabilityKind::GeneratedSubscription
        | AiCapabilityKind::GeneratedEntity => AiToolRisk::ReadOnly,
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

#[cfg(test)]
thread_local! {
    /// Number of texts tokenised on this thread, for tests that prove a
    /// search tokenises its query and nothing else.
    static TOKENISED_TEXTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn search_tokens(value: &str) -> Vec<String> {
    #[cfg(test)]
    TOKENISED_TEXTS.with(|count| count.set(count.get() + 1));
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
            search_terms: SearchTermCache::default(),
            operation_entries: OperationEntryCache::default(),
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
            search_terms: SearchTermCache::default(),
            operation_entries: OperationEntryCache::default(),
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
            search_terms: SearchTermCache::default(),
            operation_entries: OperationEntryCache::default(),
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
    fn index_set_ranked_matches_are_complete_and_search_is_their_prefix() {
        let index = Arc::new(synthetic_shape_index(101));
        let set = AiCapabilityIndexSet::compile([index]).expect("set");
        let query = AiCapabilitySearchQuery {
            text: "list records".to_owned(),
            namespace: None,
            kind: Some(AiCapabilityKind::GeneratedQuery),
            entity_or_operation: None,
            maximum_results: 7,
        };
        let ranked = set.ranked_matches(&query).expect("ranked matches");
        assert_eq!(
            ranked.len(),
            505,
            "the result ceiling must not truncate the ranking"
        );
        assert!(ranked.iter().all(|candidate| {
            set.owning_index(&candidate.entry().id)
                .is_some_and(|index| index.fingerprint() == candidate.index().fingerprint())
        }));
        let search = set.search(&query).expect("bounded search");
        assert_eq!(
            search.candidates,
            ranked
                .iter()
                .take(7)
                .map(AiCapabilityRankedMatch::candidate)
                .collect::<Vec<_>>()
        );
        assert!(matches!(
            set.ranked_matches(&AiCapabilitySearchQuery {
                maximum_results: 0,
                ..query
            }),
            Err(AiError::InvalidInput(_))
        ));
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

    /// The scoring this module used before entries carried precomputed
    /// terms: tokenise every field of the entry for every search.
    fn reference_search_rank(
        target_id: &GraphqlExecutionTargetId,
        entry: &AiCapabilityIndexEntry,
        terms: &BTreeSet<String>,
        shape_intent: Option<AiCapabilityOperationShape>,
    ) -> SearchRank {
        let mut root_semantics = 0_u64;
        let mut nested_semantics = 0_u64;
        let id = search_terms(entry.id.as_str());
        let name = search_terms(&entry.name);
        let mut operation = search_terms(&entry.operation_name);
        let mut description = search_terms(&entry.description);
        for member in &entry.operations {
            operation.extend(search_terms(&member.operation_name));
            description.extend(search_terms(&member.description));
        }
        let entity = entry
            .entity_name
            .as_deref()
            .map(search_terms)
            .unwrap_or_default();
        let target = search_terms(target_id.as_str());
        let namespace = search_terms(&entry.namespace);
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
            shape: u8::from(shape_intent.is_some_and(|shape| {
                entry.operation_shape == shape
                    || entry
                        .operations
                        .iter()
                        .any(|member| member.operation_shape == shape)
            })),
        }
    }

    fn reference_ranking<'a>(
        indexes: &[&'a AiCapabilityIndex],
        query: &AiCapabilitySearchQuery,
    ) -> Vec<&'a str> {
        let tokens = search_tokens(&query.text);
        let terms = tokens.iter().cloned().collect::<BTreeSet<_>>();
        let shape_intent = search_shape_intent(&tokens);
        let mut ranked = indexes
            .iter()
            .flat_map(|index| {
                index
                    .entries
                    .values()
                    .map(move |entry| (&index.target_id, entry))
            })
            .filter(|(_, entry)| {
                query
                    .namespace
                    .as_ref()
                    .is_none_or(|namespace| &entry.namespace == namespace)
                    && query.kind.is_none_or(|kind| entry.kind == kind)
                    && query.entity_or_operation.as_ref().is_none_or(|name| {
                        semantic_key(&entry.operation_name) == semantic_key(name)
                            || entry.operations.iter().any(|member| {
                                semantic_key(&member.operation_name) == semantic_key(name)
                            })
                            || entry
                                .entity_name
                                .as_ref()
                                .is_some_and(|entity| semantic_key(entity) == semantic_key(name))
                    })
            })
            .filter_map(|(target_id, entry)| {
                let rank = reference_search_rank(target_id, entry, &terms, shape_intent);
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
            .into_iter()
            .map(|(_, entry)| entry.id.as_str())
            .collect()
    }

    fn query(text: &str) -> AiCapabilitySearchQuery {
        AiCapabilitySearchQuery {
            text: text.to_owned(),
            namespace: None,
            kind: None,
            entity_or_operation: None,
            maximum_results: 32,
        }
    }

    #[test]
    fn search_tokenises_only_the_query() {
        let index = synthetic_shape_index(40);
        let set = AiCapabilityIndexSet::compile([Arc::new(index.clone())]).expect("set");
        // Entries are tokenised once, when their terms are first needed.
        index.search_terms();
        set.ranked_matches(&query("warm")).expect("warm-up");

        TOKENISED_TEXTS.with(|count| count.set(0));
        let result = index
            .search(&query(
                "list SyntheticEntity007 records for an investigation",
            ))
            .expect("search");
        assert!(!result.candidates.is_empty());
        assert_eq!(TOKENISED_TEXTS.with(std::cell::Cell::get), 1);

        TOKENISED_TEXTS.with(|count| count.set(0));
        let ranked = set
            .ranked_matches(&query("count SyntheticEntity012"))
            .expect("ranked");
        assert!(!ranked.is_empty());
        assert_eq!(TOKENISED_TEXTS.with(std::cell::Cell::get), 1);
    }

    #[test]
    fn compilation_precomputes_search_terms_and_deserialization_rebuilds_them() {
        let compiled = AiCapabilityIndex::compile(
            GraphqlExecutionTargetId::parse("inventory").expect("target"),
            "schema-v1",
            &semantic_catalogue(),
            None,
            None,
            None,
            [
                descriptor("inventory.lookup", "Look up stock records.", 0),
                descriptor("inventory.audit", "Audit warehouse movements.", 0),
            ],
            "policy-v1",
            AiCapabilityIndexLimits::default(),
        )
        .expect("index");
        assert!(compiled.search_terms.0.get().is_some());

        let encoded = serde_json::to_string(&compiled).expect("encode");
        assert!(!encoded.contains("search_terms"));
        let decoded: AiCapabilityIndex = serde_json::from_str(&encoded).expect("decode");
        assert!(decoded.search_terms.0.get().is_none());
        assert_eq!(decoded, compiled);
        assert_eq!(decoded.fingerprint(), compiled.fingerprint());
        assert_eq!(
            decoded.search(&query("warehouse")).expect("decoded search"),
            compiled.search(&query("warehouse")).expect("search"),
        );
        assert_eq!(decoded.search_terms(), compiled.search_terms());
    }

    #[test]
    fn precomputed_ranking_equals_tokenising_every_entry() {
        let first = synthetic_shape_index(25);
        let second = AiCapabilityIndex::compile(
            GraphqlExecutionTargetId::parse("records-target").expect("target"),
            "schema-v2",
            &semantic_catalogue(),
            None,
            None,
            None,
            [
                descriptor("records.lookup", "Look up operator records by code.", 0),
                descriptor("records.entities", "List SyntheticEntity003 companies.", 0),
                descriptor("workforce.categories", "Search staff categories.", 0),
            ],
            "policy-v2",
            AiCapabilityIndexLimits::default(),
        )
        .expect("index");
        let set =
            AiCapabilityIndexSet::compile([Arc::new(first.clone()), Arc::new(second.clone())])
                .expect("set");
        let texts = [
            "list SyntheticEntity003",
            "how many synthetic entity 014 records",
            "operator investigation",
            "search categories for staff",
            "records target",
            "inventory keyset pagination",
            "company",
            "nothing matches this zzzz",
            "list",
        ];
        for text in texts {
            for (namespace, kind, name) in [
                (None, None, None),
                (Some("inventory"), None, None),
                (None, Some(AiCapabilityKind::ReviewedStatic), None),
                (None, None, Some("SyntheticEntity003")),
                (None, None, Some("syntheticentity014s")),
            ] {
                let query = AiCapabilitySearchQuery {
                    text: text.to_owned(),
                    namespace: namespace.map(str::to_owned),
                    kind,
                    entity_or_operation: name.map(str::to_owned),
                    maximum_results: 32,
                };
                let ranked = set
                    .ranked_matches(&query)
                    .expect("ranked")
                    .into_iter()
                    .map(|candidate| candidate.entry().id.as_str().to_owned())
                    .collect::<Vec<_>>();
                assert_eq!(
                    ranked,
                    reference_ranking(&[&first, &second], &query),
                    "{text:?} {namespace:?} {kind:?} {name:?}"
                );
            }
        }
    }

    /// Groups the synthetic per-root entries of every entity into entity
    /// entries, as compilation does for a query catalogue.
    fn synthetic_entity_index(entity_count: usize) -> AiCapabilityIndex {
        let roots = synthetic_shape_index(entity_count);
        let mut by_entity = BTreeMap::<String, Vec<AiCapabilityIndexEntry>>::new();
        for entry in roots.entries.values() {
            let mut entry = entry.clone();
            entry.description = format!(
                "{} of {} records.",
                shape_name(entry.operation_shape),
                entry.entity_name.as_deref().expect("entity")
            );
            entry.fingerprint = entry_fingerprint(&entry);
            by_entity
                .entry(entry.entity_name.clone().expect("entity"))
                .or_default()
                .push(entry);
        }
        let mut entries = BTreeMap::new();
        for (entity, members) in by_entity {
            let entry = entity_entry(
                &entity,
                &format!("Operator {entity} investigations."),
                members,
                &"c".repeat(64),
            )
            .expect("entity entry");
            assert!(entries.insert(entry.id.clone(), entry).is_none());
        }
        AiCapabilityIndex {
            entries,
            search_terms: SearchTermCache::default(),
            operation_entries: OperationEntryCache::default(),
            ..roots
        }
    }

    #[test]
    fn entity_entries_rank_like_the_reference_and_by_their_operations() {
        let entities = synthetic_entity_index(30);
        assert_eq!(entities.entries().len(), 30);
        let roots = synthetic_shape_index(5);
        let set = AiCapabilityIndexSet::compile([Arc::new(entities.clone())]).expect("entity set");
        for text in [
            "list SyntheticEntity003",
            "how many SyntheticEntity014 records",
            "Synthetic007keyset",
            "operator investigation",
            "aggregate of records",
            "nothing matches zzzz",
        ] {
            for name in [
                None,
                Some("SyntheticEntity003"),
                Some("Synthetic014aggregate"),
            ] {
                let query = AiCapabilitySearchQuery {
                    entity_or_operation: name.map(str::to_owned),
                    ..query(text)
                };
                let ranked = set
                    .ranked_matches(&query)
                    .expect("ranked")
                    .into_iter()
                    .map(|candidate| candidate.entry().id.as_str().to_owned())
                    .collect::<Vec<_>>();
                assert_eq!(
                    ranked,
                    reference_ranking(&[&entities], &query),
                    "{text:?} {name:?}"
                );
            }
        }
        // An operation name reaches its entity entry, and the per-root
        // ranking returns that root first among the entity's operations.
        let keyset = set
            .ranked_matches(&query("Synthetic007keyset"))
            .expect("keyset");
        assert_eq!(
            keyset[0].entry().entity_name.as_deref(),
            Some("SyntheticEntity007")
        );
        let operations = set
            .ranked_operation_matches(&query("keyset SyntheticEntity007"))
            .expect("operations");
        assert_eq!(operations[0].entry().operation_name, "Synthetic007keyset");
        assert!(
            operations
                .iter()
                .all(|candidate| candidate.entry().kind == AiCapabilityKind::GeneratedQuery)
        );
        assert_eq!(roots.entries().len(), 25);
    }

    #[test]
    fn scores_depend_only_on_the_query_and_the_entry_itself() {
        // Adding many entries that share the query's terms changes neither
        // the score nor the relative order of the original entries: no
        // catalogue-wide statistic contributes to a score.
        let small = synthetic_entity_index(3);
        let large = synthetic_entity_index(120);
        for text in [
            "SyntheticEntity001 records",
            "list operator investigation",
            "count Synthetic002aggregate",
        ] {
            let query = query(text);
            let small_ranks = ranked_entries([&small], &query)
                .into_iter()
                .map(|(rank, _, entry)| (entry.id.clone(), rank))
                .collect::<Vec<_>>();
            let large_ranks = ranked_entries([&large], &query)
                .into_iter()
                .map(|(rank, _, entry)| (entry.id.clone(), rank))
                .collect::<BTreeMap<_, _>>();
            assert!(!small_ranks.is_empty(), "{text:?}");
            for (id, rank) in &small_ranks {
                assert_eq!(large_ranks.get(id), Some(rank), "{text:?} {id:?}");
            }
            let small_order = small_ranks.iter().map(|(id, _)| id).collect::<Vec<_>>();
            let large_order = ranked_entries([&large], &query)
                .into_iter()
                .map(|(_, _, entry)| &entry.id)
                .filter(|id| small.entries.contains_key(*id))
                .collect::<Vec<_>>();
            assert_eq!(
                small_order,
                large_order.into_iter().collect::<Vec<_>>(),
                "{text:?}"
            );
        }
    }

    #[test]
    fn inverted_index_holds_one_posting_per_entry_term() {
        let index = synthetic_entity_index(10);
        let searchable = index.search_terms();
        assert_eq!(searchable.entries.len(), index.entries.len());
        for postings in &searchable.postings {
            assert!(!postings.is_empty());
            assert!(
                postings
                    .windows(2)
                    .all(|pair| pair[0].entry < pair[1].entry)
            );
        }
        // A term only one entity holds has exactly one posting.
        let term = searchable
            .dictionary
            .get(&semantic_key("SyntheticEntity004"))
            .expect("entity term");
        assert_eq!(searchable.postings[*term as usize].len(), 1);
    }
}
