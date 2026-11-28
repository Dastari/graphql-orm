//! Compact, deterministic discovery metadata for reviewed capabilities.
//!
//! The index deliberately contains no executable document, argument schema,
//! resolver location, database coordinate, credential, or authority. A match
//! is descriptive only; callers must reauthorize and load the exact current
//! capability before execution.

use crate::sha256::valid_sha256_any_case;
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
    /// Fingerprint of the model-facing contract: a generated root's
    /// [`crate::AiGraphqlQueryCapability::contract_fingerprint`], a grouped
    /// entity's contracts of every root, or a reviewed static tool's
    /// descriptor fingerprint. It leaves out catalogue-wide fingerprints, so
    /// an edit to another capability does not change it. It identifies what
    /// a model was shown and is never execution authority.
    pub contract_fingerprint: String,
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
    /// Model-facing contract fingerprint of the root
    /// ([`AiCapabilityIndexEntry::contract_fingerprint`]).
    pub contract_fingerprint: String,
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
    /// Per-root search view, derived from `entries` on first per-root search.
    /// Never serialized or compared.
    #[serde(skip)]
    root_search: RootSearchCache,
    /// Canonical entry bytes, kept once an index set has admitted this
    /// index. Never serialized or compared.
    #[serde(skip)]
    membership: MembershipCache,
    /// An in-process mutation catalogue checked against its finished SDL.
    #[serde(skip)]
    compiled_mutations: bool,
    /// An in-process subscription catalogue checked against its finished SDL.
    #[serde(skip)]
    compiled_subscriptions: bool,
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
                    capability.contract_fingerprint(),
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
                    &capability.contract_fingerprint(),
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
                    capability.contract_fingerprint(),
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
            root_search: RootSearchCache::default(),
            membership: MembershipCache::default(),
            compiled_mutations: mutation_catalogue.is_some(),
            compiled_subscriptions: subscription_catalogue.is_some(),
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

    /// Executable roots of this index ranked one by one: every non-entity
    /// entry, and every root an entity entry groups.
    fn root_search_view(&self) -> &RootSearchView {
        self.root_search.0.get_or_init(|| {
            let entries = self
                .entries
                .values()
                .filter(|entry| entry.kind != AiCapabilityKind::GeneratedEntity)
                .map(|entry| (entry.id.clone(), entry.clone()))
                .chain(
                    self.operation_entries()
                        .by_entity
                        .iter()
                        .flat_map(|(_, roots)| roots)
                        .map(|entry| (entry.id.clone(), entry.clone())),
                )
                .collect::<BTreeMap<_, _>>();
            let terms = SearchTermIndex::compile(&self.target_id, &entries);
            Arc::new(RootSearchView { entries, terms })
        })
    }

    /// Roots in operation order within each entity, with a separate index
    /// for exact root ID lookups.
    fn operation_entries(&self) -> &OperationEntries {
        self.operation_entries.0.get_or_init(|| {
            let mut entries = OperationEntries::default();
            for entity in self
                .entries
                .values()
                .filter(|entry| !entry.operations.is_empty())
            {
                let position = entries.by_entity.len();
                let roots = entity
                    .operations
                    .iter()
                    .enumerate()
                    .map(|(offset, member)| {
                        entries
                            .by_root
                            .insert(member.capability_id.clone(), (position, offset));
                        member_entry(entity, member)
                    })
                    .collect();
                entries.by_entity.push((entity.id.clone(), roots));
            }
            Arc::new(entries)
        })
    }

    /// Precomputed search terms of every entry, in entry order.
    fn search_terms(&self) -> &SearchTermIndex {
        self.search_terms
            .0
            .get_or_init(|| Arc::new(SearchTermIndex::compile(&self.target_id, &self.entries)))
    }

    /// Admits this index to an index set: applies the checks compilation
    /// applies and returns the canonical byte size of its discoverable
    /// entries.
    ///
    /// Compilation already proves all of this, but a deserialized index has
    /// not been compiled. Its own limits must be valid and hold: entry
    /// count, each entry's size and the total size. Each entry must sit
    /// under its own ID, and every entry and grouped-root identity must be
    /// distinct. Only an admitted index's byte size is kept, so a set formed
    /// again from the same index does not measure it again; a refused index
    /// is checked again, and refused again, each time.
    fn admitted_entry_bytes(&self) -> Result<u64, AiError> {
        if let Some(bytes) = self.membership.0.get() {
            return Ok(*bytes);
        }
        self.limits.validate()?;
        if self.entries.len() > usize::from(self.limits.maximum_entries) {
            return Err(configuration_error("capability index has too many entries"));
        }
        let mut entry_bytes = 0_u64;
        for (id, entry) in &self.entries {
            #[cfg(test)]
            MEASURED_SET_ENTRIES.with(|count| count.set(count.get() + 1));
            if &entry.id != id {
                return Err(configuration_error(
                    "capability index entry is stored under another ID",
                ));
            }
            let bytes = canonical_json_bytes(entry).len();
            if bytes > self.limits.maximum_entry_bytes as usize {
                return Err(configuration_error("capability index entry is too large"));
            }
            entry_bytes = u64::try_from(bytes)
                .ok()
                .and_then(|bytes| entry_bytes.checked_add(bytes))
                .ok_or_else(|| configuration_error("capability index is too large"))?;
        }
        if entry_bytes > u64::from(self.limits.maximum_total_bytes) {
            return Err(configuration_error("capability index is too large"));
        }
        let mut identities = BTreeSet::new();
        if !self
            .entries
            .keys()
            .chain(
                self.entries
                    .values()
                    .flat_map(|entry| entry.operations.iter().map(|member| &member.capability_id)),
            )
            .all(|id| identities.insert(id))
        {
            return Err(configuration_error(
                "capability index set contains a duplicate capability",
            ));
        }
        Ok(*self.membership.0.get_or_init(|| entry_bytes))
    }

    /// Whether this index was built in process from a typed mutation catalogue.
    ///
    /// The mutation compiler checks the finished SDL and refuses any
    /// classified mutation with an argument marked `@transientInput`. This
    /// proof does not survive serialization and grants no execution authority.
    pub const fn mutations_checked_against_finished_schema(&self) -> bool {
        self.compiled_mutations
    }

    /// Whether this index was built in process from a typed subscription
    /// catalogue.
    ///
    /// The subscription compiler checks the finished SDL and refuses any
    /// replayable subscription root with an argument marked
    /// `@transientInput`, as the mutation compiler does for classified
    /// mutations. This proof does not survive serialization and grants no
    /// execution authority.
    pub const fn subscriptions_checked_against_finished_schema(&self) -> bool {
        self.compiled_subscriptions
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
        let candidates = ranked_entries([self], query, Some(usize::from(query.maximum_results)))
            .into_iter()
            .map(|hit| search_candidate(hit.entry))
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
        let mut total_bytes = 0_u64;
        // Discoverable entries only: the roots an entity entry groups are
        // part of that entry and are not counted again.
        let mut total_entries = 0_usize;
        let mut maximum_search_results = limits.maximum_search_results;
        for index in indexes {
            let index = index.into();
            if !valid_sha256_any_case(&index.fingerprint) {
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
            // Measured once per index and reused by every set formed from
            // it, such as the one-member set a single-index host gets per call.
            let entry_bytes = index.admitted_entry_bytes()?;
            total_bytes = total_bytes
                .checked_add(entry_bytes)
                .ok_or_else(|| configuration_error("capability index set byte size overflowed"))?;
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
        // Each member's own identities are distinct, so only a set of
        // several targets can collide, and only such a set needs the owner
        // map; a one-member set resolves every identity to its sole index.
        let mut capability_owners = BTreeMap::new();
        if canonical.len() > 1 {
            for (target_id, index) in &canonical {
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
            }
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
        if self.capability_owners.is_empty() {
            // A one-member set keeps no owner map.
            return self
                .indexes
                .values()
                .next()
                .filter(|index| self.indexes.len() == 1 && index.entry(id).is_some());
        }
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
        query.validate(self.maximum_search_results)?;
        let candidates = ranked_entries(
            self.indexes.values().map(Arc::as_ref),
            query,
            Some(usize::from(query.maximum_results)),
        )
        .into_iter()
        .map(|hit| search_candidate(hit.entry))
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
        Ok(
            ranked_entries(self.indexes.values().map(Arc::as_ref), query, None)
                .into_iter()
                .map(|hit| AiCapabilityRankedMatch {
                    index: hit.index,
                    entry: hit.entry,
                })
                .collect(),
        )
    }

    /// Ranks every executable root that matches one query, each on its own
    /// metadata: the roots an entity entry groups are ranked as the
    /// standalone [`AiCapabilityKind::GeneratedQuery`] entries they are, and
    /// entity entries themselves are not listed.
    ///
    /// A root's rank depends only on the query and that root, never on its
    /// entity's other roots. This is the per-root ranking that delivery modes
    /// installing one exact definition per root use.
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

    /// Lists every top-level entry for hierarchical browsing, in
    /// deterministic `(namespace, entity or name, ID)` order, without
    /// applying host policy.
    ///
    /// Top-level entries are the entries discovery ranks: entity entries,
    /// standalone generated roots and static tools; the roots an entity entry
    /// groups are reached through [`AiCapabilityRankedMatch::operations`].
    /// `namespace` and `entity` restrict the list to one exact public
    /// namespace and entity name. A caller applies host policy to the
    /// complete list, with each entity's grouped roots, before it pages or
    /// counts the result, exactly as for a search.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsafe filter name. Browsing emits model-safe
    /// metadata only and grants no authority.
    pub fn browse_matches(
        &self,
        namespace: Option<&str>,
        entity: Option<&str>,
    ) -> Result<Vec<AiCapabilityRankedMatch<'_>>, AiError> {
        if [namespace, entity]
            .into_iter()
            .flatten()
            .any(|name| validate_public_name(name, "browse filter").is_err())
        {
            return Err(input_error("capability browse filter is invalid"));
        }
        let mut matches = self
            .indexes
            .values()
            .flat_map(|index| {
                index
                    .entries
                    .values()
                    .map(move |entry| AiCapabilityRankedMatch {
                        index: index.as_ref(),
                        entry,
                    })
            })
            .filter(|candidate| {
                namespace.is_none_or(|namespace| candidate.entry.namespace == namespace)
                    && entity
                        .is_none_or(|entity| candidate.entry.entity_name.as_deref() == Some(entity))
            })
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| {
            let key = |candidate: &AiCapabilityRankedMatch<'_>| {
                (
                    candidate.entry.namespace.clone(),
                    candidate
                        .entry
                        .entity_name
                        .clone()
                        .unwrap_or_else(|| candidate.entry.name.clone()),
                )
            };
            key(left)
                .cmp(&key(right))
                .then_with(|| left.entry.id.cmp(&right.entry.id))
        });
        Ok(matches)
    }

    /// Ranks matches again after host policy, counting for each entity entry
    /// only the operations host policy admitted.
    ///
    /// [`Self::ranked_matches`] scores an entity entry on its own metadata
    /// and on every grouped root's operation name and description, so a
    /// caller can send policy every entity a root might surface. Once policy
    /// has decided, `admitted` pairs each match the caller keeps with the
    /// root IDs it admitted. This method scores each entry on its own
    /// metadata plus the admitted roots only, applies the query's filters
    /// and shape intent to those roots, and returns the positions in
    /// `admitted` of the entries that still match, in rank order. A denied
    /// root's text therefore never decides whether or where its entity
    /// appears. For an entry that groups no roots the admitted set is
    /// ignored and the rank is unchanged.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid query or result ceiling, or for a
    /// match that does not belong to this set.
    pub fn rank_admitted(
        &self,
        query: &AiCapabilitySearchQuery,
        admitted: &[(AiCapabilityRankedMatch<'_>, BTreeSet<AiToolId>)],
    ) -> Result<Vec<usize>, AiError> {
        query.validate(self.maximum_search_results)?;
        let terms = SearchTerms::parse(query);
        let mut ranked = Vec::with_capacity(admitted.len());
        for (position, (candidate, roots)) in admitted.iter().enumerate() {
            let owned = self
                .index(&candidate.index.target_id)
                .is_some_and(|index| std::ptr::eq(index.as_ref(), candidate.index));
            if !owned {
                return Err(input_error("ranked match does not belong to this set"));
            }
            let index = candidate.index;
            let searchable = index.search_terms();
            let Some(entry_position) = searchable.positions.get(&candidate.entry.id) else {
                return Err(input_error("ranked match does not belong to this set"));
            };
            let admits = |operation: usize| {
                candidate
                    .entry
                    .operations
                    .get(operation)
                    .is_some_and(|member| roots.contains(&member.capability_id))
            };
            if let Some(rank) =
                terms.rank(searchable, *entry_position, candidate.entry, query, &admits)
            {
                ranked.push((rank, position));
            }
        }
        ranked.sort_by(|left, right| {
            right.0.cmp(&left.0).then_with(|| {
                admitted[left.1]
                    .0
                    .entry
                    .id
                    .cmp(&admitted[right.1].0.entry.id)
            })
        });
        Ok(ranked.into_iter().map(|(_, position)| position).collect())
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
    ///
    /// For an entity entry it lists every grouped operation; use
    /// [`Self::admitted_candidate`] after host policy.
    pub fn candidate(&self) -> AiCapabilitySearchCandidate {
        search_candidate(self.entry)
    }

    /// Compact search candidate listing only the grouped operations whose
    /// root ID is in `admitted`. For an entry that groups no roots this is
    /// [`Self::candidate`].
    pub fn admitted_candidate(&self, admitted: &BTreeSet<AiToolId>) -> AiCapabilitySearchCandidate {
        let mut candidate = search_candidate(self.entry);
        if !self.entry.operations.is_empty() {
            candidate.operations = self
                .entry
                .operations
                .iter()
                .filter(|member| admitted.contains(&member.capability_id))
                .map(|member| AiCapabilityCandidateOperation {
                    operation_name: member.operation_name.clone(),
                    operation_shape: member.operation_shape,
                })
                .collect();
        }
        candidate
    }

    /// Matches for the root entries an entity entry groups, in operation
    /// order. Empty for every other entry.
    pub fn operations(&self) -> Vec<AiCapabilityRankedMatch<'a>> {
        self.iter_grouped_roots().collect()
    }

    /// Iterates the grouped root matches in operation order without
    /// collecting them. Empty for an entry that groups no roots.
    ///
    /// The matches carry index metadata. Each root still requires its own
    /// current host policy decision before disclosure.
    pub fn iter_grouped_roots(&self) -> impl Iterator<Item = AiCapabilityRankedMatch<'a>> + '_ {
        let roots: &[AiCapabilityIndexEntry] = if self.entry.operations.is_empty() {
            &[]
        } else {
            let groups = &self.index.operation_entries().by_entity;
            groups
                .binary_search_by(|(entity, _)| entity.cmp(&self.entry.id))
                .map_or(&[], |position| groups[position].1.as_slice())
        };
        roots.iter().map(|entry| AiCapabilityRankedMatch {
            index: self.index,
            entry,
        })
    }

    /// Match for the entity entry that groups this root, if any.
    pub fn entity(&self) -> Option<AiCapabilityRankedMatch<'a>> {
        self.index
            .entity_entry_of(&self.entry.id)
            .map(|entry| AiCapabilityRankedMatch {
                index: self.index,
                entry,
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
    /// Reviewed approval classification of the entry. A broker uses it to
    /// decide hierarchical-rule admission from the held candidate alone,
    /// before any current-index lookup. A candidate without it is treated
    /// as [`AiApprovalRule::Never`].
    #[serde(default = "never_approved")]
    pub approval: AiApprovalRule,
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

/// One search field of an entry, as a bit in a field mask.
const FIELD_ID: u16 = 1;
const FIELD_NAME: u16 = 1 << 1;
const FIELD_OPERATION: u16 = 1 << 2;
const FIELD_ENTITY: u16 = 1 << 3;
const FIELD_TARGET: u16 = 1 << 4;
const FIELD_NAMESPACE: u16 = 1 << 5;
const FIELD_DESCRIPTION: u16 = 1 << 6;
const FIELD_RELATIONSHIPS: u16 = 1 << 7;
const FIELD_SCALARS: u16 = 1 << 8;

/// Fixed weight of each root field.
const ROOT_FIELD_WEIGHTS: [(u16, u8); 7] = [
    (FIELD_ID, 12),
    (FIELD_NAME, 11),
    (FIELD_OPERATION, 10),
    (FIELD_ENTITY, 9),
    (FIELD_TARGET, 8),
    (FIELD_NAMESPACE, 8),
    (FIELD_DESCRIPTION, 7),
];

/// Fixed weight of each nested field.
const NESTED_FIELD_WEIGHTS: [(u16, u8); 2] = [(FIELD_RELATIONSHIPS, 3), (FIELD_SCALARS, 2)];

/// The fields of one entry that hold one search term: one posting.
///
/// Scores are per entry and fixed-weight by design: a field holding a term
/// adds its fixed weight once. They never depend on how many other entries
/// hold a term, so the rank of an entry a principal may see cannot reveal
/// anything about entries it may not. No inverse document frequency, BM25 or
/// other catalogue-wide statistic is computed.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Posting {
    /// Position of the entry in the owning index's entry order.
    entry: u32,
    /// Fields of the entry's own metadata that hold the term.
    own: u16,
    /// Union of the entry's fields and every grouped root's fields.
    all: u16,
    /// For an entity entry, each grouped operation (by position in
    /// `operations`) whose operation name or description holds the term,
    /// with those fields. A grouped operation contributes only when the
    /// caller admits it.
    operations: Box<[(u16, u16)]>,
}

impl Posting {
    /// Fields that hold the term once only the admitted operations count.
    fn fields(&self, admits: &dyn Fn(usize) -> bool) -> u16 {
        self.operations
            .iter()
            .filter(|(operation, _)| admits(usize::from(*operation)))
            .fold(self.own, |fields, (_, operation)| fields | operation)
    }
}

/// Filter keys of one entry, computed once when its index is compiled.
#[derive(Clone, Debug, Eq, PartialEq)]
struct EntrySearchKeys {
    /// Identifier of the entry at this position.
    id: AiToolId,
    /// Semantic key of the entry's own operation name.
    operation_key: String,
    entity_key: Option<String>,
    /// Semantic key of each grouped operation's name, in operation order.
    operation_keys: Vec<String>,
}

/// Inverted search index of one capability index: term to postings.
///
/// Terms are interned per index. A query resolves each of its terms once and
/// scores only the entries in those terms' posting lists, so a search costs
/// O(matching postings) for sparse matches.
#[derive(Clone, Debug, Eq, PartialEq)]
struct SearchTermIndex {
    dictionary: BTreeMap<String, u32>,
    /// Indexed by term; each list is in entry order.
    postings: Vec<Vec<Posting>>,
    /// Indexed by entry position.
    entries: Vec<EntrySearchKeys>,
    /// Entry position by identifier.
    positions: BTreeMap<AiToolId, u32>,
    /// Exact public namespaces of this view, which may differ from its target ID.
    namespaces: BTreeSet<String>,
    /// Whether entry positions also resolve their IDs in the source map.
    /// A standalone deserialized index may not yet have passed admission.
    ids_match_keys: bool,
}

fn add_terms(fields: &mut BTreeMap<String, u16>, terms: BTreeSet<String>, field: u16) {
    for term in terms {
        // A shape word in a query selects a shape and never scores, so it is
        // not stored.
        if !search_shape_term(&term) {
            *fields.entry(term).or_default() |= field;
        }
    }
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
        let mut positions = BTreeMap::new();
        let mut ids_match_keys = true;
        let mut namespaces = BTreeSet::new();
        for (position, (id, entry)) in entries.iter().enumerate() {
            ids_match_keys &= id == &entry.id;
            if !namespaces.contains(&entry.namespace) {
                namespaces.insert(entry.namespace.clone());
            }
            let position = u32::try_from(position).unwrap_or(u32::MAX);
            let mut own = BTreeMap::<String, u16>::new();
            add_terms(&mut own, search_terms(entry.id.as_str()), FIELD_ID);
            add_terms(&mut own, search_terms(&entry.name), FIELD_NAME);
            add_terms(
                &mut own,
                search_terms(&entry.operation_name),
                FIELD_OPERATION,
            );
            if let Some(entity) = &entry.entity_name {
                add_terms(&mut own, search_terms(entity), FIELD_ENTITY);
            }
            add_terms(&mut own, target.clone(), FIELD_TARGET);
            add_terms(&mut own, search_terms(&entry.namespace), FIELD_NAMESPACE);
            add_terms(
                &mut own,
                search_terms(&entry.description),
                FIELD_DESCRIPTION,
            );
            add_terms(
                &mut own,
                entry
                    .relationships
                    .iter()
                    .flat_map(|relationship| {
                        search_terms(&format!(
                            "{} {} {}",
                            relationship.name, relationship.description, relationship.target_entity
                        ))
                    })
                    .collect(),
                FIELD_RELATIONSHIPS,
            );
            add_terms(
                &mut own,
                entry
                    .scalar_fields
                    .iter()
                    .flat_map(|field| {
                        search_terms(&format!("{} {}", field.name, field.description))
                    })
                    .collect(),
                FIELD_SCALARS,
            );
            // Grouped operations are recorded per operation, so ranking can
            // count only the ones host policy admits.
            let mut grouped = BTreeMap::<String, Vec<(u16, u16)>>::new();
            for (operation, member) in entry.operations.iter().enumerate() {
                let operation = u16::try_from(operation).unwrap_or(u16::MAX);
                let mut fields = BTreeMap::<String, u16>::new();
                add_terms(
                    &mut fields,
                    search_terms(&member.operation_name),
                    FIELD_OPERATION,
                );
                add_terms(
                    &mut fields,
                    search_terms(&member.description),
                    FIELD_DESCRIPTION,
                );
                for (term, fields) in fields {
                    grouped.entry(term).or_default().push((operation, fields));
                }
            }
            let terms = own
                .keys()
                .chain(grouped.keys())
                .cloned()
                .collect::<BTreeSet<_>>();
            for term in terms {
                let id = match dictionary.get(&term) {
                    Some(id) => *id,
                    None => {
                        let id = u32::try_from(dictionary.len()).unwrap_or(u32::MAX);
                        dictionary.insert(term.clone(), id);
                        postings.push(Vec::new());
                        id
                    }
                };
                let own = own.get(&term).copied().unwrap_or(0);
                let operations = grouped.remove(&term).unwrap_or_default().into_boxed_slice();
                let all = operations
                    .iter()
                    .fold(own, |fields, (_, root)| fields | root);
                postings[id as usize].push(Posting {
                    entry: position,
                    own,
                    all,
                    operations,
                });
            }
            positions.insert(entry.id.clone(), position);
            keys.push(EntrySearchKeys {
                id: entry.id.clone(),
                operation_key: semantic_key(&entry.operation_name),
                entity_key: entry.entity_name.as_deref().map(semantic_key),
                operation_keys: entry
                    .operations
                    .iter()
                    .map(|member| semantic_key(&member.operation_name))
                    .collect(),
            });
        }
        Self {
            dictionary,
            postings,
            entries: keys,
            positions,
            namespaces,
            ids_match_keys,
        }
    }
}

/// One query, tokenised once and scored against precomputed postings.
struct SearchTerms {
    terms: BTreeSet<String>,
    shape_intent: Option<AiCapabilityOperationShape>,
    name_key: Option<String>,
}

impl SearchTerms {
    fn parse(query: &AiCapabilitySearchQuery) -> Self {
        let tokens = search_tokens(&query.text);
        let shape_intent = search_shape_intent(&tokens);
        Self {
            terms: tokens
                .into_iter()
                .filter(|term| !search_shape_term(term))
                .collect(),
            shape_intent,
            name_key: query.entity_or_operation.as_deref().map(semantic_key),
        }
    }

    /// Posting lists of the query terms, in term order.
    fn lists<'a>(&self, searchable: &'a SearchTermIndex) -> impl Iterator<Item = &'a [Posting]> {
        self.terms
            .iter()
            .filter_map(move |term| searchable.dictionary.get(term))
            .map(move |id| searchable.postings[*id as usize].as_slice())
    }

    /// Rank of one entry counting only admitted grouped operations, or
    /// `None` when the entry does not match the query and its filters.
    fn rank(
        &self,
        searchable: &SearchTermIndex,
        position: u32,
        entry: &AiCapabilityIndexEntry,
        query: &AiCapabilitySearchQuery,
        admits: &dyn Fn(usize) -> bool,
    ) -> Option<SearchRank> {
        self.filtered_rank(
            &searchable.entries[position as usize],
            entry,
            query,
            Self::score_at(self.lists(searchable), position, |posting| {
                posting.fields(admits)
            }),
            admits,
        )
    }

    fn score_at<'a>(
        lists: impl IntoIterator<Item = &'a [Posting]>,
        position: u32,
        fields: impl Fn(&Posting) -> u16,
    ) -> (u32, u32) {
        let mut score = (0, 0);
        for list in lists {
            if let Ok(found) = list.binary_search_by_key(&position, |posting| posting.entry) {
                let (root, nested) = field_weights(fields(&list[found]));
                score.0 += root;
                score.1 += nested;
            }
        }
        score
    }

    fn filtered_rank(
        &self,
        keys: &EntrySearchKeys,
        entry: &AiCapabilityIndexEntry,
        query: &AiCapabilitySearchQuery,
        (root_semantics, nested_semantics): (u32, u32),
        admits: &dyn Fn(usize) -> bool,
    ) -> Option<SearchRank> {
        let admitted_operations = || {
            entry
                .operations
                .iter()
                .enumerate()
                .filter(|(operation, _)| admits(*operation))
        };
        if query
            .namespace
            .as_ref()
            .is_some_and(|namespace| &entry.namespace != namespace)
            || query.kind.is_some_and(|kind| entry.kind != kind)
            || self.name_key.as_ref().is_some_and(|name| {
                &keys.operation_key != name
                    && keys.entity_key.as_ref() != Some(name)
                    && !admitted_operations()
                        .any(|(operation, _)| keys.operation_keys.get(operation) == Some(name))
            })
        {
            return None;
        }
        let shape = self.shape_intent.is_some_and(|shape| {
            entry.operation_shape == shape
                || admitted_operations().any(|(_, member)| member.operation_shape == shape)
        });
        let rank = SearchRank {
            root_semantics,
            nested_semantics,
            shape: u8::from(shape),
        };
        rank.has_semantic_match().then_some(rank)
    }
}

// Every possible combination of the nine posting fields. Each field adds
// its fixed weight once, even if several roots contributed it.
const FIELD_WEIGHTS: [(u8, u8); 1 << 9] = {
    let mut weights = [(0, 0); 1 << 9];
    let mut mask = 0;
    while mask < weights.len() {
        let mut field = 0;
        while field < ROOT_FIELD_WEIGHTS.len() {
            let (bit, weight) = ROOT_FIELD_WEIGHTS[field];
            if mask & bit as usize != 0 {
                weights[mask].0 += weight;
            }
            field += 1;
        }
        field = 0;
        while field < NESTED_FIELD_WEIGHTS.len() {
            let (bit, weight) = NESTED_FIELD_WEIGHTS[field];
            if mask & bit as usize != 0 {
                weights[mask].1 += weight;
            }
            field += 1;
        }
        mask += 1;
    }
    weights
};

/// Summed fixed root and nested weights of a field mask.
fn field_weights(fields: u16) -> (u32, u32) {
    #[cfg(test)]
    WEIGHED_SEARCH_POSTINGS.with(|count| count.set(count.get() + 1));
    let (root, nested) = FIELD_WEIGHTS[usize::from(fields)];
    (u32::from(root), u32::from(nested))
}

/// Executable roots of one index, each a standalone entry: every
/// non-entity entry, and every root an entity entry groups. Ranked for
/// per-root delivery, so no root's rank depends on its entity's other roots.
#[derive(Debug)]
struct RootSearchView {
    entries: BTreeMap<AiToolId, AiCapabilityIndexEntry>,
    terms: SearchTermIndex,
}

/// Lazily built, never serialized, never compared per-root search view.
#[derive(Clone, Debug, Default)]
struct RootSearchCache(OnceLock<Arc<RootSearchView>>);

impl PartialEq for RootSearchCache {
    fn eq(&self, _: &Self) -> bool {
        // Derived from the entries, which the owning index already compares.
        true
    }
}

impl Eq for RootSearchCache {}

/// Lazily built, never serialized, never compared cache of the root entries
/// that entity entries group.
#[derive(Clone, Debug, Default)]
struct OperationEntryCache(OnceLock<Arc<OperationEntries>>);

#[derive(Debug, Default)]
struct OperationEntries {
    // Admission checks that source map keys match their entity IDs.
    by_entity: Vec<(AiToolId, Vec<AiCapabilityIndexEntry>)>,
    by_root: BTreeMap<AiToolId, (usize, usize)>,
}

impl OperationEntries {
    fn get(&self, id: &AiToolId) -> Option<(&AiToolId, &AiCapabilityIndexEntry)> {
        let &(entity, root) = self.by_root.get(id)?;
        let (id, roots) = &self.by_entity[entity];
        Some((id, &roots[root]))
    }
}

impl PartialEq for OperationEntryCache {
    fn eq(&self, _: &Self) -> bool {
        // Derived from the entries, which the owning index already compares.
        true
    }
}

impl Eq for OperationEntryCache {}

/// Canonical entry bytes of an index that set admission has validated.
/// Lazily measured, never serialized, never compared.
#[derive(Clone, Debug, Default)]
struct MembershipCache(OnceLock<u64>);

impl PartialEq for MembershipCache {
    fn eq(&self, _: &Self) -> bool {
        // Derived from the entries, which the owning index already compares.
        true
    }
}

impl Eq for MembershipCache {}

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
    // The validated query contains at most 1,024 bytes, so the total
    // fixed field weights across all of its terms fit in u32.
    root_semantics: u32,
    nested_semantics: u32,
    shape: u8,
}

impl SearchRank {
    fn has_semantic_match(self) -> bool {
        self.root_semantics > 0 || self.nested_semantics > 0
    }
}

struct RankedEntry<'a> {
    rank: SearchRank,
    index: &'a AiCapabilityIndex,
    entry: &'a AiCapabilityIndexEntry,
    // Entry ordinal in the owning view, or MAX for a deserialized index
    // with mismatched entry keys, which must compare the resolved IDs.
    ordinal: u32,
}

/// Scores the matching postings with fixed per-field weights. An entry's
/// score depends only on its own metadata and grouped operations.
fn ranked_entries<'a>(
    indexes: impl IntoIterator<Item = &'a AiCapabilityIndex>,
    query: &AiCapabilitySearchQuery,
    maximum: Option<usize>,
) -> Vec<RankedEntry<'a>> {
    let terms = SearchTerms::parse(query);
    let mut ranked = Vec::new();
    for index in indexes {
        rank_view(
            &terms,
            query,
            index,
            &index.entries,
            index.search_terms(),
            &mut ranked,
        );
    }
    sort_ranked(&mut ranked, maximum);
    ranked
}

fn rank_view<'a>(
    terms: &SearchTerms,
    query: &AiCapabilitySearchQuery,
    index: &'a AiCapabilityIndex,
    entries: &'a BTreeMap<AiToolId, AiCapabilityIndexEntry>,
    searchable: &SearchTermIndex,
    ranked: &mut Vec<RankedEntry<'a>>,
) {
    if searchable.ids_match_keys
        && query
            .namespace
            .as_ref()
            .is_some_and(|namespace| !searchable.namespaces.contains(namespace))
    {
        return;
    }
    let postings = terms.lists(searchable).collect::<Vec<_>>();
    let mut push_rank = |position, entry: &'a AiCapabilityIndexEntry, score| {
        let keys = &searchable.entries[position as usize];
        if let Some(rank) = terms.filtered_rank(keys, entry, query, score, &|_| true) {
            ranked.push(RankedEntry {
                rank,
                index,
                entry,
                ordinal: if searchable.ids_match_keys {
                    position
                } else {
                    u32::MAX
                },
            });
        }
    };
    if postings.is_empty() {
        return;
    }
    if searchable.ids_match_keys
        && postings.iter().map(|list| list.len()).sum::<usize>() >= entries.len().div_ceil(2)
    {
        if let Some(name) = &terms.name_key {
            // An exact name can match the entity, its own operation or any
            // grouped root. Test those keys before reading broad postings.
            for (position, keys) in searchable.entries.iter().enumerate() {
                if (&keys.operation_key == name
                    || keys.entity_key.as_ref() == Some(name)
                    || keys.operation_keys.contains(name))
                    && let Some(entry) = entries.get(&keys.id)
                {
                    let position = position as u32;
                    push_rank(
                        position,
                        entry,
                        SearchTerms::score_at(postings.iter().copied(), position, |posting| {
                            posting.all
                        }),
                    );
                }
            }
            return;
        }
        // The dense path walks the same BTree order used to assign positions.
        // Its score buffer is local to this index. Sparse matches still avoid
        // visiting the rest of the index.
        let mut scores = vec![(0, 0); entries.len()];
        for posting in postings.into_iter().flatten() {
            let (root, nested) = field_weights(posting.all);
            let score = &mut scores[posting.entry as usize];
            score.0 += root;
            score.1 += nested;
        }
        for (position, (entry, score)) in entries.values().zip(scores).enumerate() {
            if score != (0, 0) {
                push_rank(position as u32, entry, score);
            }
        }
    } else {
        let mut scores = BTreeMap::<u32, (u32, u32)>::new();
        for posting in postings.into_iter().flatten() {
            let (root, nested) = field_weights(posting.all);
            let score = scores.entry(posting.entry).or_default();
            score.0 += root;
            score.1 += nested;
        }
        for (position, score) in scores {
            let keys = &searchable.entries[position as usize];
            if let Some(entry) = entries.get(&keys.id) {
                push_rank(position, entry, score);
            }
        }
    }
}

fn sort_ranked(ranked: &mut Vec<RankedEntry<'_>>, maximum: Option<usize>) {
    let compare = |left: &RankedEntry<'_>, right: &RankedEntry<'_>| {
        right.rank.cmp(&left.rank).then_with(|| {
            if left.ordinal != u32::MAX
                && right.ordinal != u32::MAX
                && std::ptr::eq(left.index, right.index)
            {
                left.ordinal.cmp(&right.ordinal)
            } else {
                left.entry.id.cmp(&right.entry.id)
            }
        })
    };
    // Only index-only search supplies a limit. Broker callers must receive
    // the complete ordered ranking before asking the host for decisions.
    if let Some(maximum) = maximum
        && maximum < ranked.len()
    {
        ranked.select_nth_unstable_by(maximum, compare);
        ranked.truncate(maximum);
    }
    ranked.sort_unstable_by(compare);
}

/// Ranks every executable root of `indexes` on its own metadata
/// ([`AiCapabilityIndexSet::ranked_operation_matches`]).
fn rank_operation_entries<'a>(
    indexes: impl IntoIterator<Item = &'a AiCapabilityIndex>,
    query: &AiCapabilitySearchQuery,
) -> Vec<(&'a AiCapabilityIndex, &'a AiCapabilityIndexEntry)> {
    let terms = SearchTerms::parse(query);
    let mut ranked = Vec::new();
    for index in indexes {
        let view = index.root_search_view();
        rank_view(
            &terms,
            query,
            index,
            &view.entries,
            &view.terms,
            &mut ranked,
        );
    }
    sort_ranked(&mut ranked, None);
    ranked
        .into_iter()
        .map(|hit| (hit.index, hit.entry))
        .collect()
}

const fn never_approved() -> AiApprovalRule {
    AiApprovalRule::Never
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
        approval: entry.approval,
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
    contract_fingerprint: &str,
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
        contract_fingerprint: contract_fingerprint.to_owned(),
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
            contract_fingerprint: root.contract_fingerprint.clone(),
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
    let result_description = entity_result_description(entity, &members);
    let result_record_cost = combined_record_cost(&members);
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
    let contract_fingerprint = sha256_json(&json!({
        "version": AI_CAPABILITY_ENTITY_VERSION,
        "kind": "generated_entity_contract",
        "id": id.as_str(),
        "entity": entity,
        "operations": members
            .iter()
            .map(|member| json!({
                "id": member.capability_id.as_str(),
                "operation": member.operation_name,
                "contract": member.contract_fingerprint,
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
        result_description,
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
        contract_fingerprint,
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

/// Result description of an entity entry over the given operations.
fn entity_result_description<'a>(
    entity: &str,
    members: impl IntoIterator<Item = &'a AiCapabilityOperationMember>,
) -> String {
    let shapes = members
        .into_iter()
        .map(|member| shape_name(member.operation_shape).to_lowercase())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(", ");
    format!("Bounded {entity} reads ({shapes}); choose one operation per call.")
}

/// Largest result-record bounds over the given operations.
fn combined_record_cost<'a>(
    members: impl IntoIterator<Item = &'a AiCapabilityOperationMember>,
) -> AiGraphqlResultRecordCostEstimate {
    members.into_iter().fold(
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
    )
}

impl AiCapabilityIndexEntry {
    /// Result description counting only the grouped operations whose root ID
    /// is in `admitted`. The stored `result_description` of an entity entry
    /// names every grouped operation's shape; show this one to a principal
    /// instead. For an entry that groups no roots it is the stored value.
    pub fn admitted_result_description(&self, admitted: &BTreeSet<AiToolId>) -> String {
        match &self.entity_name {
            Some(entity) if !self.operations.is_empty() => entity_result_description(
                entity,
                self.operations
                    .iter()
                    .filter(|member| admitted.contains(&member.capability_id)),
            ),
            _ => self.result_description.clone(),
        }
    }

    /// Largest result-record bounds over the grouped operations whose root
    /// ID is in `admitted`. The stored `result_record_cost` of an entity
    /// entry covers every grouped operation; show this one to a principal
    /// instead. For an entry that groups no roots it is the stored value.
    pub fn admitted_result_record_cost(
        &self,
        admitted: &BTreeSet<AiToolId>,
    ) -> AiGraphqlResultRecordCostEstimate {
        if self.operations.is_empty() {
            return self.result_record_cost;
        }
        combined_record_cost(
            self.operations
                .iter()
                .filter(|member| admitted.contains(&member.capability_id)),
        )
    }
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
        contract_fingerprint: member.contract_fingerprint.clone(),
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
        contract_fingerprint: descriptor.fingerprint.clone(),
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
    validate_fingerprint(
        &entry.contract_fingerprint,
        "capability contract fingerprint",
    )?;
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

#[cfg(test)]
thread_local! {
    /// Number of entries an index-set compilation measured on this thread,
    /// for tests that prove a set reuses each member index's measurements.
    static MEASURED_SET_ENTRIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static WEIGHED_SEARCH_POSTINGS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
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
            root_search: RootSearchCache::default(),
            membership: MembershipCache::default(),
            compiled_mutations: false,
            compiled_subscriptions: false,
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
            root_search: RootSearchCache::default(),
            membership: MembershipCache::default(),
            compiled_mutations: false,
            compiled_subscriptions: false,
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
            root_search: RootSearchCache::default(),
            membership: MembershipCache::default(),
            compiled_mutations: false,
            compiled_subscriptions: false,
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
        let mut root_semantics = 0_u32;
        let mut nested_semantics = 0_u32;
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
            root_semantics += u32::from(id.contains(term)) * 12;
            root_semantics += u32::from(name.contains(term)) * 11;
            root_semantics += u32::from(operation.contains(term)) * 10;
            root_semantics += u32::from(entity.contains(term)) * 9;
            root_semantics += u32::from(target.contains(term)) * 8;
            root_semantics += u32::from(namespace.contains(term)) * 8;
            root_semantics += u32::from(description.contains(term)) * 7;
            nested_semantics += u32::from(relationships.contains(term)) * 3;
            nested_semantics += u32::from(fields.contains(term)) * 2;
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
        assert_eq!(serde_json::to_string(&decoded).expect("re-encode"), encoded);
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

    #[test]
    fn complete_ranking_and_bounded_prefix_break_ties_by_capability_id() {
        let compile = |target: &str, ids: [&str; 2]| {
            AiCapabilityIndex::compile(
                GraphqlExecutionTargetId::parse(target).expect("target"),
                "schema-v1",
                &semantic_catalogue(),
                None,
                None,
                None,
                ids.map(|id| descriptor(id, "Reconcile archived invoices.", 0)),
                "policy-v1",
                AiCapabilityIndexLimits::default(),
            )
            .expect("index")
        };
        let first = compile("alpha", ["records.a", "audit.d"]);
        let second = compile("beta", ["audit.b", "records.c"]);
        let set = AiCapabilityIndexSet::compile([first, second]).expect("set");
        let mut query = query("reconcile");
        query.maximum_results = 2;
        for (namespace, expected) in [
            (None, vec!["audit.b", "audit.d", "records.a", "records.c"]),
            (Some("audit"), vec!["audit.b", "audit.d"]),
        ] {
            query.namespace = namespace.map(str::to_owned);
            let ranked = set.ranked_matches(&query).expect("ranked matches");
            assert_eq!(
                ranked
                    .iter()
                    .map(|entry| entry.entry().id.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
            let bounded = set.search(&query).expect("bounded search");
            assert_eq!(bounded.index_set_fingerprint, set.fingerprint());
            assert_eq!(
                bounded.candidates,
                ranked
                    .iter()
                    .take(2)
                    .map(|entry| entry.candidate())
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn exact_scope_scores_only_matching_entry_postings() {
        let index = synthetic_entity_index(40);
        let set = AiCapabilityIndexSet::compile([Arc::new(index)]).expect("set");
        let mut query = query("list inventory records");
        query.namespace = Some("inventory".to_owned());
        query.entity_or_operation = Some("synthetic014keyset".to_owned());

        let expected = reference_ranking(&[set.indexes().next().expect("index")], &query);
        assert_eq!(expected.len(), 1, "grouped operation must match the name");
        WEIGHED_SEARCH_POSTINGS.with(|count| count.set(0));
        let ranked = set.ranked_matches(&query).expect("ranked matches");
        assert_eq!(
            ranked
                .iter()
                .map(|entry| entry.entry().id.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        assert!(WEIGHED_SEARCH_POSTINGS.with(std::cell::Cell::get) <= 2);
    }

    #[test]
    fn broad_search_preserves_exact_id_lookup_for_deserialized_entries() {
        let index = synthetic_entity_index(4);
        let id = index.entries().next().expect("entry").id.as_str();
        let mut encoded = serde_json::to_value(&index).expect("encoded index");
        let entries = encoded["entries"].as_object_mut().expect("entries");
        let moved = entries.remove(id).expect("original key");
        entries.insert("inventory.renamed_entity".to_owned(), moved);
        let decoded: AiCapabilityIndex = serde_json::from_value(encoded).expect("decoded index");

        let query = query("records");
        let expected = index
            .search(&query)
            .expect("original results")
            .candidates
            .into_iter()
            .filter(|candidate| candidate.id.as_str() != id)
            .collect::<Vec<_>>();
        assert_eq!(
            expected.len(),
            3,
            "broad term should match the other entries"
        );
        assert_eq!(
            decoded.search(&query).expect("decoded search").candidates,
            expected
        );
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
            root_search: RootSearchCache::default(),
            membership: MembershipCache::default(),
            compiled_mutations: false,
            compiled_subscriptions: false,
            ..roots
        }
    }

    #[test]
    fn index_set_compilation_reuses_member_measurements() {
        // A host that supplies one current index gets a one-member set per
        // broker call. Compiling it again must not measure every entry again.
        let index = Arc::new(synthetic_entity_index(40));
        let first = AiCapabilityIndexSet::compile([Arc::clone(&index)]).expect("set");
        let entity = index.entries().next().expect("entity").clone();
        let root = entity.operations[0].capability_id.clone();
        let unknown = AiToolId::parse("inventory.not_indexed").expect("id");
        MEASURED_SET_ENTRIES.with(|count| count.set(0));
        for _ in 0..3 {
            let set = AiCapabilityIndexSet::compile([Arc::clone(&index)]).expect("set");
            assert_eq!(set, first);
            assert_eq!(set.fingerprint(), first.fingerprint());
            assert_eq!(
                set.entry(&entity.id).map(|(_, entry)| &entry.id),
                Some(&entity.id)
            );
            assert_eq!(set.entry(&root).map(|(_, entry)| &entry.id), Some(&root));
            assert!(set.owning_index(&unknown).is_none());
            assert!(set.entry(&unknown).is_none());
        }
        assert_eq!(MEASURED_SET_ENTRIES.with(std::cell::Cell::get), 0);

        // Limits are still enforced against the reused measurements.
        let bytes = u64::try_from(
            index
                .entries()
                .map(|entry| canonical_json_bytes(entry).len())
                .sum::<usize>(),
        )
        .expect("bytes");
        let limits = |maximum_entries: u32, maximum_total_bytes: u64| AiCapabilityIndexSetLimits {
            maximum_entries,
            maximum_total_bytes,
            ..AiCapabilityIndexSetLimits::default()
        };
        assert!(
            AiCapabilityIndexSet::compile_with_limits([Arc::clone(&index)], limits(40, bytes))
                .is_ok()
        );
        assert!(
            AiCapabilityIndexSet::compile_with_limits([Arc::clone(&index)], limits(39, bytes))
                .is_err()
        );
        assert!(
            AiCapabilityIndexSet::compile_with_limits([Arc::clone(&index)], limits(40, bytes - 1))
                .is_err()
        );
    }

    #[test]
    fn index_set_refuses_a_deserialized_index_with_duplicate_identities() {
        // Deserialization does not rerun compilation, so the set is the
        // check that keeps one identity per capability.
        let index = synthetic_entity_index(3);
        let mut encoded = serde_json::to_value(&index).expect("encoded");
        let entries = encoded["entries"].as_object_mut().expect("entries");
        let ids = entries.keys().cloned().collect::<Vec<_>>();
        let duplicate = entries[&ids[0]]["operations"][0]["capability_id"].clone();
        entries.get_mut(&ids[1]).expect("second")["operations"][0]["capability_id"] = duplicate;
        let tampered: AiCapabilityIndex = serde_json::from_value(encoded).expect("decoded");
        assert!(AiCapabilityIndexSet::compile([Arc::new(tampered.clone())]).is_err());
        // An entity's own ID reused as one of its roots is refused as well.
        let mut encoded = serde_json::to_value(&index).expect("encoded");
        let entries = encoded["entries"].as_object_mut().expect("entries");
        entries.get_mut(&ids[0]).expect("first")["operations"][0]["capability_id"] =
            serde_json::json!(ids[1]);
        let tampered: AiCapabilityIndex = serde_json::from_value(encoded).expect("decoded");
        assert!(AiCapabilityIndexSet::compile([Arc::new(tampered)]).is_err());
    }

    #[test]
    fn index_set_applies_a_deserialized_index_its_own_compilation_checks() {
        // Deserialization does not rerun compilation, so set admission
        // applies the per-index limits and identity rules compilation does.
        let index = synthetic_entity_index(3);
        let encoded = serde_json::to_value(&index).expect("encoded");
        let decoded: AiCapabilityIndex = serde_json::from_value(encoded.clone()).expect("decoded");
        assert_eq!(
            AiCapabilityIndexSet::compile([Arc::new(decoded)])
                .expect("an untampered index is admitted")
                .fingerprint(),
            AiCapabilityIndexSet::compile([Arc::new(index.clone())])
                .expect("set")
                .fingerprint()
        );
        let ids = encoded["entries"]
            .as_object()
            .expect("entries")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        // Each case must be refused by its own check, so the expected
        // message is asserted: removing any one check fails its case.
        let refused = |case: &str, expected: &str, edit: &dyn Fn(&mut serde_json::Value)| {
            let mut tampered = encoded.clone();
            edit(&mut tampered);
            let tampered: AiCapabilityIndex = serde_json::from_value(tampered).expect("decoded");
            match AiCapabilityIndexSet::compile([Arc::new(tampered)]) {
                Err(AiError::InvalidConfiguration(message)) => {
                    assert_eq!(message, expected, "{case} is refused for another reason");
                }
                Err(other) => panic!("{case} is refused with an unexpected error: {other:?}"),
                Ok(_) => panic!("{case} is admitted"),
            }
        };
        refused(
            "more entries than the index's maximum",
            "capability index has too many entries",
            &|value| {
                value["limits"]["maximum_entries"] = json!(2);
            },
        );
        refused(
            "an entry larger than the index's maximum",
            "capability index entry is too large",
            &|value| {
                value["limits"]["maximum_entry_bytes"] = json!(1_024);
                value["entries"][&ids[0]]["description"] = json!("x".repeat(2_048));
            },
        );
        refused(
            "entries larger in total than the index's maximum",
            "capability index is too large",
            &|value| {
                value["limits"]["maximum_entry_bytes"] = json!(256 * 1_024);
                value["limits"]["maximum_total_bytes"] = json!(1_024);
            },
        );
        // The entry count stays valid and within its maximum, so only the
        // limits' own validation can refuse this index.
        refused(
            "invalid index limits",
            "capability-index limits are invalid",
            &|value| {
                value["limits"]["maximum_search_results"] = json!(0);
            },
        );
        refused(
            "a key that differs from its entry's identity",
            "capability index entry is stored under another ID",
            &|value| {
                let entries = value["entries"].as_object_mut().expect("entries");
                let entry = entries.remove(&ids[0]).expect("entry");
                entries.insert("inventory.renamed_entity".to_owned(), entry);
            },
        );
        refused(
            "two entries holding one grouped-root identity",
            "capability index set contains a duplicate capability",
            &|value| {
                // Every entry still sits under its own ID, so only the
                // distinctness check can refuse the repeated root.
                let repeated = value["entries"][&ids[0]]["operations"][0]["capability_id"].clone();
                value["entries"][&ids[1]]["operations"][0]["capability_id"] = repeated;
            },
        );
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
    fn grouped_root_iterator_preserves_operation_order_and_exact_entries() {
        let set = AiCapabilityIndexSet::compile([Arc::new(synthetic_entity_index(3))])
            .expect("entity set");
        for candidate in set.browse_matches(None, None).expect("entities") {
            let roots = candidate.iter_grouped_roots().collect::<Vec<_>>();
            assert_eq!(roots.len(), candidate.entry().operations.len());
            assert!(!roots.is_empty());
            for (root, member) in roots.iter().zip(&candidate.entry().operations) {
                assert_eq!(root.entry().id, member.capability_id);
                assert!(std::ptr::eq(root.index(), candidate.index()));
                assert!(std::ptr::eq(
                    root.entry(),
                    candidate
                        .index()
                        .entry(&member.capability_id)
                        .expect("root")
                ));
                assert!(root.iter_grouped_roots().next().is_none());
            }
        }
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
            let small_ranks = ranked_entries([&small], &query, None)
                .into_iter()
                .map(|hit| (hit.entry.id.clone(), hit.rank))
                .collect::<Vec<_>>();
            let large_ranks = ranked_entries([&large], &query, None)
                .into_iter()
                .map(|hit| (hit.entry.id.clone(), hit.rank))
                .collect::<BTreeMap<_, _>>();
            assert!(!small_ranks.is_empty(), "{text:?}");
            for (id, rank) in &small_ranks {
                assert_eq!(large_ranks.get(id), Some(rank), "{text:?} {id:?}");
            }
            let small_order = small_ranks.iter().map(|(id, _)| id).collect::<Vec<_>>();
            let large_order = ranked_entries([&large], &query, None)
                .into_iter()
                .map(|hit| &hit.entry.id)
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

    /// One entity with five roots; only the keyset root's description holds
    /// the term `zebra`.
    fn zebra_entity_index() -> AiCapabilityIndex {
        let roots = synthetic_shape_index(1);
        let members = roots
            .entries
            .values()
            .map(|entry| {
                let mut entry = entry.clone();
                entry.description =
                    if entry.operation_shape == AiCapabilityOperationShape::KeysetList {
                        entry.result_record_cost.maximum_total_records = 9_999;
                        "Page through zebra stripes.".to_owned()
                    } else {
                        format!("{} of operator records.", shape_name(entry.operation_shape))
                    };
                entry.fingerprint = entry_fingerprint(&entry);
                entry
            })
            .collect::<Vec<_>>();
        let entity = entity_entry(
            "SyntheticEntity000",
            "Operator investigations.",
            members,
            &"c".repeat(64),
        )
        .expect("entity entry");
        AiCapabilityIndex {
            entries: BTreeMap::from([(entity.id.clone(), entity)]),
            search_terms: SearchTermCache::default(),
            operation_entries: OperationEntryCache::default(),
            root_search: RootSearchCache::default(),
            membership: MembershipCache::default(),
            compiled_mutations: false,
            compiled_subscriptions: false,
            ..roots
        }
    }

    fn root_id(index: &AiCapabilityIndex, shape: AiCapabilityOperationShape) -> AiToolId {
        index
            .entries()
            .flat_map(|entry| entry.operations.iter())
            .find(|member| member.operation_shape == shape)
            .expect("member")
            .capability_id
            .clone()
    }

    #[test]
    fn rank_admitted_counts_only_the_admitted_operations() {
        let index = zebra_entity_index();
        let set = AiCapabilityIndexSet::compile([Arc::new(index.clone())]).expect("set");
        let keyset = root_id(&index, AiCapabilityOperationShape::KeysetList);
        let every = index
            .entries()
            .flat_map(|entry| {
                entry
                    .operations
                    .iter()
                    .map(|member| member.capability_id.clone())
            })
            .collect::<BTreeSet<_>>();
        let without_keyset = every
            .iter()
            .filter(|id| **id != keyset)
            .cloned()
            .collect::<BTreeSet<_>>();
        // `zebra` appears only in the keyset root's description: the entity
        // is a candidate for policy, and survives only if that root is
        // admitted.
        let zebra = query("zebra");
        let ranked = set.ranked_matches(&zebra).expect("ranked");
        assert_eq!(ranked.len(), 1);
        let with = |admitted: &BTreeSet<AiToolId>| {
            set.rank_admitted(&zebra, &[(ranked[0], admitted.clone())])
                .expect("rank admitted")
        };
        assert_eq!(with(&every), vec![0]);
        assert!(with(&without_keyset).is_empty());
        assert!(with(&BTreeSet::new()).is_empty());
        // Scores and shape intent equal the reference computed over the
        // entity with only its admitted operations.
        for text in [
            "keyset SyntheticEntity000",
            "count operator records",
            "operator investigation",
        ] {
            let query = query(text);
            let ranked = set.ranked_matches(&query).expect("ranked");
            let Some(candidate) = ranked.first() else {
                continue;
            };
            let terms = SearchTerms::parse(&query);
            for admitted in [&every, &without_keyset] {
                let mut narrowed = candidate.entry().clone();
                narrowed
                    .operations
                    .retain(|member| admitted.contains(&member.capability_id));
                let expected = reference_search_rank(
                    &index.target_id,
                    &narrowed,
                    &search_tokens(&query.text).into_iter().collect(),
                    search_shape_intent(&search_tokens(&query.text)),
                );
                let searchable = index.search_terms();
                let actual = terms.rank(
                    searchable,
                    searchable.positions[&candidate.entry().id],
                    candidate.entry(),
                    &query,
                    &|operation| {
                        admitted.contains(&candidate.entry().operations[operation].capability_id)
                    },
                );
                assert_eq!(
                    actual,
                    expected.has_semantic_match().then_some(expected),
                    "{text:?}"
                );
            }
        }
        // Entity-level shapes and bounds count only admitted operations.
        let entity = ranked[0].entry();
        assert!(entity.admitted_result_description(&every).contains("page"));
        assert!(
            !entity
                .admitted_result_description(&without_keyset)
                .contains("page")
        );
        assert_eq!(
            entity
                .admitted_result_record_cost(&every)
                .maximum_total_records,
            9_999
        );
        assert!(
            entity
                .admitted_result_record_cost(&without_keyset)
                .maximum_total_records
                < 9_999
        );
        // The admitted candidate lists only admitted operations.
        let candidate = ranked[0].admitted_candidate(&without_keyset);
        assert_eq!(candidate.operations.len(), 4);
        assert!(
            candidate.operations.iter().all(
                |operation| operation.operation_shape != AiCapabilityOperationShape::KeysetList
            )
        );
    }

    #[test]
    fn per_root_ranking_scores_each_root_on_its_own_metadata() {
        let index = zebra_entity_index();
        let set = AiCapabilityIndexSet::compile([Arc::new(index.clone())]).expect("set");
        let keyset = root_id(&index, AiCapabilityOperationShape::KeysetList);
        // Only the root whose own description holds the term matches; its
        // siblings do not inherit it.
        let zebra = set
            .ranked_operation_matches(&AiCapabilitySearchQuery {
                kind: Some(AiCapabilityKind::GeneratedQuery),
                ..query("zebra")
            })
            .expect("roots");
        assert_eq!(
            zebra
                .iter()
                .map(|candidate| candidate.entry().id.clone())
                .collect::<Vec<_>>(),
            [keyset]
        );
        // Every root ranks exactly as a standalone entry would.
        let standalone = index
            .entries()
            .flat_map(|entry| entry.operations.iter())
            .map(|member| index.entry(&member.capability_id).expect("root").clone())
            .collect::<Vec<_>>();
        let roots = AiCapabilityIndex {
            entries: standalone
                .into_iter()
                .map(|entry| (entry.id.clone(), entry))
                .collect(),
            search_terms: SearchTermCache::default(),
            operation_entries: OperationEntryCache::default(),
            root_search: RootSearchCache::default(),
            membership: MembershipCache::default(),
            compiled_mutations: false,
            compiled_subscriptions: false,
            ..index.clone()
        };
        for text in [
            "list SyntheticEntity000",
            "operator records",
            "count",
            "zebra stripes",
        ] {
            let query = query(text);
            let actual = set
                .ranked_operation_matches(&query)
                .expect("roots")
                .into_iter()
                .map(|candidate| candidate.entry().id.as_str().to_owned())
                .collect::<Vec<_>>();
            assert_eq!(actual, reference_ranking(&[&roots], &query), "{text:?}");
        }
    }
}
