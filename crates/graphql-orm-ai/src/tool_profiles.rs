//! Finished-schema-validated GraphQL tool profile compilation.
//!
//! Profiles are explicit least-disclosure policy. They compile reviewed input
//! adapters and projections into immutable GraphQL documents and tool
//! descriptors; resolver discovery alone never makes an operation callable.

use std::collections::{BTreeMap, BTreeSet};

use async_graphql_parser::{parse_schema, types::TypeSystemDefinition};
use graphql_orm::graphql::orm::{GraphqlOperationCatalog, GraphqlOperationKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    AiApprovalRule, AiDisclosureDisposition, AiDisclosureSchema, AiDisclosureShape, AiError,
    AiGeneratedGraphqlOperationPolicy, AiToolCatalog, AiToolDescriptor, AiToolOperationDomain,
    AiToolOperationKind, AiToolRisk, DataClassification, GraphqlExecutionTargetId,
    GraphqlOperationContract, ToolMaturity,
};

/// Current wire version for compiled GraphQL tool manifests.
pub const AI_GRAPHQL_TOOL_MANIFEST_VERSION: u16 = 1;

/// Stable optional router-descriptor extension identity for a compiled
/// GraphQL tool manifest.
pub const AI_GRAPHQL_TOOL_MANIFEST_EXTENSION_NAME: &str = "graphql-orm-ai.tool-manifest";

const MAXIMUM_DESCRIPTION_BYTES: usize = 512;
const MAXIMUM_PROJECTION_DEPTH: usize = 8;
const MAXIMUM_PROFILE_INPUTS: usize = 64;
const MAXIMUM_SELECTIONS_PER_LEVEL: usize = 128;

/// GraphQL operation root used by a tool profile.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiGraphqlRootType {
    /// Query root.
    Query,
    /// Mutation root, available only through an explicit supervised profile.
    Mutation,
}

impl AiGraphqlRootType {
    fn keyword(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Mutation => "mutation",
        }
    }

    fn conventional_name(self) -> &'static str {
        match self {
            Self::Query => "Query",
            Self::Mutation => "Mutation",
        }
    }

    fn operation_kind(self) -> AiToolOperationKind {
        match self {
            Self::Query => AiToolOperationKind::Query,
            Self::Mutation => AiToolOperationKind::Mutation,
        }
    }

    fn orm_kind(self) -> GraphqlOperationKind {
        match self {
            Self::Query => GraphqlOperationKind::Query,
            Self::Mutation => GraphqlOperationKind::Mutation,
        }
    }
}

/// Whether a profile describes an ORM-generated or host-authored root field.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiGraphqlToolSource {
    /// A resolver present in [`GraphqlOperationCatalog`].
    Generated,
    /// A handwritten resolver validated against the finished schema.
    Custom,
}

/// Closed model-facing input contract for one profile variable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AiGraphqlProfileInputType {
    /// A bounded string or GraphQL ID.
    String {
        /// Minimum UTF-8 character count.
        minimum_length: u32,
        /// Maximum UTF-8 character count.
        maximum_length: u32,
    },
    /// A bounded integer.
    Integer {
        /// Inclusive minimum.
        minimum: i64,
        /// Inclusive maximum.
        maximum: i64,
    },
    /// A bounded floating-point number.
    Number {
        /// Inclusive minimum.
        minimum: f64,
        /// Inclusive maximum.
        maximum: f64,
    },
    /// A boolean.
    Boolean,
    /// A closed subset of a GraphQL enum.
    Enum {
        /// Model-visible enum values.
        values: Vec<String>,
    },
}

/// One explicitly exposed model-facing profile input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiGraphqlProfileInput {
    /// Semantic GraphQL-compatible variable name, which may differ from the
    /// resolver argument name.
    pub name: String,
    /// Bounded model-facing description.
    pub description: String,
    /// Whether the JSON input must contain the property.
    pub required: bool,
    /// Closed value constraints.
    pub input_type: AiGraphqlProfileInputType,
}

impl AiGraphqlProfileInput {
    /// Creates a bounded string/ID input.
    pub fn string(
        name: impl Into<String>,
        description: impl Into<String>,
        required: bool,
        minimum_length: u32,
        maximum_length: u32,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            required,
            input_type: AiGraphqlProfileInputType::String {
                minimum_length,
                maximum_length,
            },
        }
    }

    /// Creates a bounded integer input.
    pub fn integer(
        name: impl Into<String>,
        description: impl Into<String>,
        required: bool,
        minimum: i64,
        maximum: i64,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            required,
            input_type: AiGraphqlProfileInputType::Integer { minimum, maximum },
        }
    }

    /// Creates a bounded floating-point input.
    pub fn number(
        name: impl Into<String>,
        description: impl Into<String>,
        required: bool,
        minimum: f64,
        maximum: f64,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            required,
            input_type: AiGraphqlProfileInputType::Number { minimum, maximum },
        }
    }

    /// Creates a boolean input.
    pub fn boolean(
        name: impl Into<String>,
        description: impl Into<String>,
        required: bool,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            required,
            input_type: AiGraphqlProfileInputType::Boolean,
        }
    }

    /// Creates a closed enum input.
    pub fn enumeration<I, S>(
        name: impl Into<String>,
        description: impl Into<String>,
        required: bool,
        values: I,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            name: name.into(),
            description: description.into(),
            required,
            input_type: AiGraphqlProfileInputType::Enum {
                values: values.into_iter().map(Into::into).collect(),
            },
        }
    }
}

/// Closed typed value plan for a GraphQL field argument.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum AiGraphqlArgumentValue {
    /// Reference one declared model-facing profile input.
    Input(String),
    /// A server-owned constant validated against the finished schema.
    Fixed(Value),
    /// A closed input-object adapter.
    Object(BTreeMap<String, AiGraphqlArgumentValue>),
    /// A fixed-shape list adapter.
    List(Vec<AiGraphqlArgumentValue>),
}

impl AiGraphqlArgumentValue {
    /// References a declared profile input.
    pub fn input(name: impl Into<String>) -> Self {
        Self::Input(name.into())
    }

    /// Creates a server-owned fixed argument.
    pub fn fixed(value: impl Into<Value>) -> Self {
        Self::Fixed(value.into())
    }

    /// Creates a closed input-object argument adapter.
    pub fn object<I, S>(fields: I) -> Self
    where
        I: IntoIterator<Item = (S, AiGraphqlArgumentValue)>,
        S: Into<String>,
    {
        Self::Object(
            fields
                .into_iter()
                .map(|(name, value)| (name.into(), value))
                .collect(),
        )
    }

    /// Creates a fixed-shape list argument adapter.
    pub fn list(values: impl IntoIterator<Item = AiGraphqlArgumentValue>) -> Self {
        Self::List(values.into_iter().collect())
    }
}

/// One named GraphQL argument and its closed value plan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiGraphqlArgumentPlan {
    /// GraphQL argument or input-object field name.
    pub name: String,
    /// Server-owned value adapter.
    pub value: AiGraphqlArgumentValue,
}

impl AiGraphqlArgumentPlan {
    /// Creates a named argument plan.
    pub fn new(name: impl Into<String>, value: AiGraphqlArgumentValue) -> Self {
        Self {
            name: name.into(),
            value,
        }
    }
}

/// One explicitly selected result field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiGraphqlSelection {
    /// Exact field name in the finished schema.
    pub field_name: String,
    /// Optional model-facing response alias.
    pub alias: Option<String>,
    /// Closed nested field arguments.
    pub arguments: Vec<AiGraphqlArgumentPlan>,
    /// Explicit nested projection. Empty means a scalar/enum leaf.
    pub selections: Vec<AiGraphqlSelection>,
    /// Required upper bound when the field returns a list.
    pub maximum_items: Option<u32>,
}

impl AiGraphqlSelection {
    /// Creates a scalar or enum selection.
    pub fn scalar(field_name: impl Into<String>) -> Self {
        Self {
            field_name: field_name.into(),
            alias: None,
            arguments: Vec::new(),
            selections: Vec::new(),
            maximum_items: None,
        }
    }

    /// Creates an object selection with an explicit nested projection.
    pub fn object<I>(field_name: impl Into<String>, selections: I) -> Self
    where
        I: IntoIterator<Item = AiGraphqlSelection>,
    {
        Self {
            field_name: field_name.into(),
            alias: None,
            arguments: Vec::new(),
            selections: selections.into_iter().collect(),
            maximum_items: None,
        }
    }

    /// Creates a bounded list selection.
    pub fn bounded_list<I>(field_name: impl Into<String>, maximum_items: u32, selections: I) -> Self
    where
        I: IntoIterator<Item = AiGraphqlSelection>,
    {
        Self {
            field_name: field_name.into(),
            alias: None,
            arguments: Vec::new(),
            selections: selections.into_iter().collect(),
            maximum_items: Some(maximum_items),
        }
    }

    /// Applies a response alias.
    #[must_use]
    pub fn with_alias(mut self, alias: impl Into<String>) -> Self {
        self.alias = Some(alias.into());
        self
    }

    /// Adds closed nested arguments.
    #[must_use]
    pub fn with_arguments<I>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = AiGraphqlArgumentPlan>,
    {
        self.arguments = arguments.into_iter().collect();
        self
    }

    fn response_name(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.field_name)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProfileExecution {
    ReadOnly,
    SupervisedMutation,
}

/// Reviewed policy profile used to generate one immutable GraphQL tool.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiGraphqlToolProfile {
    profile_id: String,
    root_type: AiGraphqlRootType,
    field_name: String,
    description: String,
    inputs: Vec<AiGraphqlProfileInput>,
    arguments: Vec<AiGraphqlArgumentPlan>,
    selections: Vec<AiGraphqlSelection>,
    root_maximum_items: Option<u32>,
    disclosure_schema: AiDisclosureSchema,
    maximum_result_bytes: u64,
    maximum_result_records: u32,
    execution: ProfileExecution,
    risk: AiToolRisk,
    approval: AiApprovalRule,
    idempotent: bool,
}

impl AiGraphqlToolProfile {
    /// Creates a read-only query profile. Nothing is selected or exposed until
    /// explicit inputs, arguments, projection, and disclosure are supplied.
    #[allow(clippy::too_many_arguments)]
    pub fn read_only(
        profile_id: impl Into<String>,
        field_name: impl Into<String>,
        description: impl Into<String>,
        selections: Vec<AiGraphqlSelection>,
        disclosure_schema: AiDisclosureSchema,
        maximum_result_bytes: u64,
        maximum_result_records: u32,
    ) -> Self {
        Self {
            profile_id: profile_id.into(),
            root_type: AiGraphqlRootType::Query,
            field_name: field_name.into(),
            description: description.into(),
            inputs: Vec::new(),
            arguments: Vec::new(),
            selections,
            root_maximum_items: None,
            disclosure_schema,
            maximum_result_bytes,
            maximum_result_records,
            execution: ProfileExecution::ReadOnly,
            risk: AiToolRisk::ReadOnly,
            approval: AiApprovalRule::None,
            idempotent: true,
        }
    }

    /// Creates an explicitly supervised mutation profile.
    ///
    /// Mutations are never inferred from resolver discovery and require a
    /// one-shot approval at execution time.
    #[allow(clippy::too_many_arguments)]
    pub fn supervised_mutation(
        profile_id: impl Into<String>,
        field_name: impl Into<String>,
        description: impl Into<String>,
        selections: Vec<AiGraphqlSelection>,
        disclosure_schema: AiDisclosureSchema,
        maximum_result_bytes: u64,
        maximum_result_records: u32,
        risk: AiToolRisk,
        idempotent: bool,
    ) -> Result<Self, AiError> {
        if !matches!(
            risk,
            AiToolRisk::LowRiskWrite | AiToolRisk::NonIdempotentWrite | AiToolRisk::HighImpact
        ) {
            return Err(configuration_error(
                "supervised mutations require an explicit write risk",
            ));
        }
        Ok(Self {
            profile_id: profile_id.into(),
            root_type: AiGraphqlRootType::Mutation,
            field_name: field_name.into(),
            description: description.into(),
            inputs: Vec::new(),
            arguments: Vec::new(),
            selections,
            root_maximum_items: None,
            disclosure_schema,
            maximum_result_bytes,
            maximum_result_records,
            execution: ProfileExecution::SupervisedMutation,
            risk,
            approval: AiApprovalRule::OneShot,
            idempotent,
        })
    }

    /// Replaces the closed set of model-facing inputs.
    #[must_use]
    pub fn with_inputs<I>(mut self, inputs: I) -> Self
    where
        I: IntoIterator<Item = AiGraphqlProfileInput>,
    {
        self.inputs = inputs.into_iter().collect();
        self
    }

    /// Replaces the root argument adapter.
    #[must_use]
    pub fn with_arguments<I>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = AiGraphqlArgumentPlan>,
    {
        self.arguments = arguments.into_iter().collect();
        self
    }

    /// Sets a required bound when the root field directly returns a list.
    #[must_use]
    pub fn with_root_list_bound(mut self, maximum_items: u32) -> Self {
        self.root_maximum_items = Some(maximum_items);
        self
    }

    /// Returns the stable host-authored profile identifier.
    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Returns the exact GraphQL root coordinate.
    pub fn coordinate(&self) -> (AiGraphqlRootType, &str) {
        (self.root_type, &self.field_name)
    }
}

/// One compiled entry carried in a versioned subgraph manifest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiGraphqlToolManifestEntry {
    /// Exact root type.
    pub root_type: AiGraphqlRootType,
    /// Exact root field.
    pub field_name: String,
    /// Stable profile identity within the root coordinate.
    pub profile_id: String,
    /// Resolver provenance.
    pub source: AiGraphqlToolSource,
    /// Immutable runtime descriptor with generated document and JSON Schema.
    pub descriptor: AiToolDescriptor,
    /// Exact response disclosure contract.
    pub disclosure_schema: AiDisclosureSchema,
}

/// Versioned manifest produced by one owning subgraph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiGraphqlToolManifest {
    /// Manifest wire version.
    pub version: u16,
    /// Stable public subgraph identity.
    pub subgraph_id: String,
    /// SHA-256 of the exact finished SDL used to compile the entries.
    pub finished_schema_fingerprint: String,
    /// Canonically ordered compiled profiles.
    pub entries: Vec<AiGraphqlToolManifestEntry>,
    /// Canonical manifest fingerprint.
    pub fingerprint: String,
}

impl AiGraphqlToolManifest {
    /// Validates the manifest version, canonical fingerprint, and current
    /// finished schema without performing introspection.
    pub fn validate_against_finished_schema(&self, finished_sdl: &str) -> Result<(), AiError> {
        if self.version != AI_GRAPHQL_TOOL_MANIFEST_VERSION
            || self.finished_schema_fingerprint != finished_schema_fingerprint(finished_sdl)
            || self.fingerprint != self.compute_fingerprint()
        {
            return Err(configuration_error(
                "GraphQL tool manifest version or schema binding is stale",
            ));
        }
        self.validate_entries()
    }

    /// Registers every compiled entry. Generated entries are revalidated
    /// against the current ORM operation catalog and host classification;
    /// custom roots remain bound to the same finished-schema fingerprint.
    pub fn register_into(
        &self,
        catalog: &mut AiToolCatalog,
        operation_catalog: &GraphqlOperationCatalog,
        operation_policy: &dyn AiGeneratedGraphqlOperationPolicy,
    ) -> Result<(), AiError> {
        if self.version != AI_GRAPHQL_TOOL_MANIFEST_VERSION
            || self.fingerprint != self.compute_fingerprint()
        {
            return Err(configuration_error("GraphQL tool manifest is invalid"));
        }
        self.validate_entries()?;
        for entry in &self.entries {
            match entry.source {
                AiGraphqlToolSource::Generated => catalog.register_generated_with_disclosure(
                    entry.descriptor.clone(),
                    entry.disclosure_schema.clone(),
                    operation_catalog,
                    operation_policy,
                )?,
                AiGraphqlToolSource::Custom => catalog.register_with_disclosure(
                    entry.descriptor.clone(),
                    entry.disclosure_schema.clone(),
                )?,
            }
        }
        Ok(())
    }

    /// Returns a versioned JSON payload suitable for an optional generic
    /// router-descriptor extension.
    pub fn extension_payload(&self) -> Result<Value, AiError> {
        if self.version != AI_GRAPHQL_TOOL_MANIFEST_VERSION
            || self.fingerprint != self.compute_fingerprint()
        {
            return Err(configuration_error("GraphQL tool manifest is invalid"));
        }
        self.validate_entries()?;
        serde_json::to_value(self).map_err(|_| configuration_error("manifest encoding failed"))
    }

    /// Decodes an optional router-extension payload and rejects incomplete or
    /// unsupported versions.
    pub fn from_extension_payload(value: Value) -> Result<Self, AiError> {
        let manifest: Self = serde_json::from_value(value)
            .map_err(|_| configuration_error("GraphQL tool manifest payload is incomplete"))?;
        if manifest.version != AI_GRAPHQL_TOOL_MANIFEST_VERSION
            || manifest.fingerprint != manifest.compute_fingerprint()
        {
            return Err(configuration_error(
                "GraphQL tool manifest payload is unsupported or stale",
            ));
        }
        manifest.validate_entries()?;
        Ok(manifest)
    }

    fn compute_fingerprint(&self) -> String {
        let value = json!({
            "format": "graphql-orm-ai-graphql-tool-manifest-v1",
            "version": self.version,
            "subgraph_id": self.subgraph_id,
            "finished_schema_fingerprint": self.finished_schema_fingerprint,
            "entries": self.entries,
        });
        sha256_json(&value)
    }

    fn validate_entries(&self) -> Result<(), AiError> {
        validate_public_token(&self.subgraph_id, "subgraph identity")?;
        let mut profiles = BTreeSet::new();
        let mut ids = BTreeSet::new();
        for entry in &self.entries {
            validate_public_token(&entry.profile_id, "profile ID")?;
            validate_graphql_name(&entry.field_name, "root field")?;
            validate_compiled_descriptor(entry)?;
            if !profiles.insert((entry.root_type, &entry.field_name, &entry.profile_id))
                || !ids.insert(entry.descriptor.id.as_str())
                || entry.descriptor.id.as_str()
                    != stable_tool_id(
                        &self.subgraph_id,
                        entry.root_type,
                        &entry.field_name,
                        &entry.profile_id,
                    )
                || entry.descriptor.operation_kind != entry.root_type.operation_kind()
                || entry
                    .descriptor
                    .graphql_contract
                    .as_ref()
                    .is_none_or(|contract| {
                        contract.schema_fingerprint != self.finished_schema_fingerprint
                            || contract.disclosure_schema_fingerprint
                                != entry.disclosure_schema.fingerprint
                            || contract.result_projection_fingerprint
                                != entry.descriptor.result_projection
                    })
                || matches!(entry.source, AiGraphqlToolSource::Generated)
                    != entry
                        .descriptor
                        .graphql_contract
                        .as_ref()
                        .and_then(GraphqlOperationContract::generated_operation)
                        .is_some()
            {
                return Err(configuration_error(
                    "GraphQL tool manifest entry is incomplete or inconsistent",
                ));
            }
        }
        Ok(())
    }
}

/// Aggregated active manifests with duplicate-root and schema-drift checks.
#[derive(Clone, Debug)]
pub struct AiGraphqlToolManifestSet {
    manifests: Vec<AiGraphqlToolManifest>,
}

impl AiGraphqlToolManifestSet {
    /// Aggregates manifests against exact active finished SDL values.
    ///
    /// Multiple profiles for one root are allowed only from the same owning
    /// subgraph. A root advertised by multiple subgraphs fails closed.
    pub fn aggregate(
        manifests: impl IntoIterator<Item = AiGraphqlToolManifest>,
        active_finished_schemas: &BTreeMap<String, String>,
    ) -> Result<Self, AiError> {
        let mut manifests = manifests.into_iter().collect::<Vec<_>>();
        let mut subgraphs = BTreeSet::new();
        let mut root_owners = BTreeMap::<(AiGraphqlRootType, String), String>::new();
        let mut tool_ids = BTreeSet::new();
        for manifest in &manifests {
            if !subgraphs.insert(manifest.subgraph_id.clone()) {
                return Err(configuration_error(
                    "duplicate GraphQL tool subgraph manifest",
                ));
            }
            let sdl = active_finished_schemas
                .get(&manifest.subgraph_id)
                .ok_or_else(|| configuration_error("active subgraph schema is missing"))?;
            manifest.validate_against_finished_schema(sdl)?;
            for entry in &manifest.entries {
                let coordinate = (entry.root_type, entry.field_name.clone());
                match root_owners.get(&coordinate) {
                    Some(owner) if owner != &manifest.subgraph_id => {
                        return Err(configuration_error(
                            "GraphQL tool root is advertised by multiple subgraphs",
                        ));
                    }
                    Some(_) => {}
                    None => {
                        root_owners.insert(coordinate, manifest.subgraph_id.clone());
                    }
                }
                if !tool_ids.insert(entry.descriptor.id.as_str().to_owned()) {
                    return Err(configuration_error("duplicate compiled GraphQL tool ID"));
                }
            }
        }
        manifests.sort_by(|left, right| left.subgraph_id.cmp(&right.subgraph_id));
        Ok(Self { manifests })
    }

    /// Returns canonically ordered active manifests.
    pub fn manifests(&self) -> &[AiGraphqlToolManifest] {
        &self.manifests
    }

    /// Registers entries from the schema-validated active manifest set.
    ///
    /// Generated-operation admission was evaluated by the owning subgraph
    /// while compiling its manifest. The consuming AI process therefore does
    /// not need to import that subgraph's ORM operation catalogue or service
    /// crate. This method grants no resolver authority; execution remains
    /// bound to each entry's exact GraphQL document, target, disclosure
    /// contract, and current-principal checks.
    ///
    /// # Errors
    ///
    /// Returns an error if any manifest entry is inconsistent or conflicts
    /// with an entry already registered in `catalog`.
    pub fn register_into(&self, catalog: &mut AiToolCatalog) -> Result<(), AiError> {
        for manifest in &self.manifests {
            manifest.validate_entries()?;
            for entry in &manifest.entries {
                catalog.register_compiled_manifest_entry(
                    entry.descriptor.clone(),
                    entry.disclosure_schema.clone(),
                )?;
            }
        }
        Ok(())
    }
}

/// Compiler owned by one subgraph after its complete schema has been built.
#[derive(Clone, Debug)]
pub struct AiGraphqlToolManifestBuilder {
    subgraph_id: String,
    target_id: GraphqlExecutionTargetId,
    schema: FinishedSchema,
    finished_schema_fingerprint: String,
    entries: Vec<AiGraphqlToolManifestEntry>,
}

impl AiGraphqlToolManifestBuilder {
    /// Parses a finished subgraph SDL and creates a fail-closed compiler.
    /// This is a local build/startup operation and performs no introspection or
    /// network discovery.
    pub fn new(
        subgraph_id: impl Into<String>,
        target_id: GraphqlExecutionTargetId,
        finished_sdl: &str,
    ) -> Result<Self, AiError> {
        let subgraph_id = subgraph_id.into();
        validate_public_token(&subgraph_id, "subgraph identity")?;
        let schema = FinishedSchema::parse(finished_sdl)?;
        Ok(Self {
            subgraph_id,
            target_id,
            schema,
            finished_schema_fingerprint: finished_schema_fingerprint(finished_sdl),
            entries: Vec::new(),
        })
    }

    /// Compiles one profile for an admitted ORM-generated resolver.
    ///
    /// The owning subgraph must supply its current operation catalogue and
    /// application-operation policy. Their decision is fingerprint-bound into
    /// the exported entry so a federated consumer need not import the owning
    /// service crate.
    ///
    /// # Errors
    ///
    /// Returns an error if the resolver is stale or not admitted, or if the
    /// profile does not exactly validate against the finished schema.
    pub fn add_generated_profile(
        &mut self,
        profile: AiGraphqlToolProfile,
        operation_catalog: &GraphqlOperationCatalog,
        operation_policy: &dyn AiGeneratedGraphqlOperationPolicy,
    ) -> Result<(), AiError> {
        let operation = operation_catalog
            .resolve(profile.root_type.orm_kind(), &profile.field_name)
            .ok_or_else(|| configuration_error("generated resolver profile is stale"))?;
        if operation.kind() == GraphqlOperationKind::Subscription
            || !operation_policy.is_application_operation(operation)
        {
            return Err(configuration_error(
                "generated resolver is not an admitted application operation",
            ));
        }
        let mut entry = self.compile(profile, AiGraphqlToolSource::Generated)?;
        let contract = entry
            .descriptor
            .graphql_contract
            .take()
            .ok_or_else(|| configuration_error("compiled GraphQL contract is missing"))?
            .with_generated_operation(
                operation_catalog,
                operation.kind(),
                operation.field_name(),
                &entry.descriptor.document,
            )
            .map_err(|_| configuration_error("generated resolver profile is stale"))?;
        entry.descriptor = entry.descriptor.with_graphql_contract(contract);
        self.insert(entry)
    }

    /// Compiles one explicitly described handwritten root operation.
    pub fn add_custom_profile(&mut self, profile: AiGraphqlToolProfile) -> Result<(), AiError> {
        let entry = self.compile(profile, AiGraphqlToolSource::Custom)?;
        self.insert(entry)
    }

    /// Canonicalizes and fingerprints the subgraph manifest.
    pub fn build(mut self) -> Result<AiGraphqlToolManifest, AiError> {
        self.entries.sort_by(|left, right| {
            (left.root_type, &left.field_name, &left.profile_id).cmp(&(
                right.root_type,
                &right.field_name,
                &right.profile_id,
            ))
        });
        let mut manifest = AiGraphqlToolManifest {
            version: AI_GRAPHQL_TOOL_MANIFEST_VERSION,
            subgraph_id: self.subgraph_id,
            finished_schema_fingerprint: self.finished_schema_fingerprint,
            entries: self.entries,
            fingerprint: String::new(),
        };
        manifest.fingerprint = manifest.compute_fingerprint();
        Ok(manifest)
    }

    fn insert(&mut self, entry: AiGraphqlToolManifestEntry) -> Result<(), AiError> {
        if self.entries.iter().any(|existing| {
            existing.root_type == entry.root_type
                && existing.field_name == entry.field_name
                && existing.profile_id == entry.profile_id
        }) {
            return Err(configuration_error("duplicate GraphQL tool profile"));
        }
        self.entries.push(entry);
        Ok(())
    }

    fn compile(
        &self,
        profile: AiGraphqlToolProfile,
        source: AiGraphqlToolSource,
    ) -> Result<AiGraphqlToolManifestEntry, AiError> {
        validate_profile_shape(&profile)?;
        let root_name = self.schema.root_name(profile.root_type);
        let root_field = self.schema.object_field(root_name, &profile.field_name)?;

        let input_map = profile
            .inputs
            .iter()
            .map(|input| (input.name.as_str(), input))
            .collect::<BTreeMap<_, _>>();
        let mut inferred_inputs = BTreeMap::<String, String>::new();
        let rendered_arguments = self.schema.render_arguments(
            &root_field.arguments,
            &profile.arguments,
            &input_map,
            &mut inferred_inputs,
        )?;
        let (rendered_projection, projected_shape) = self.schema.render_projection(
            &root_field.ty,
            &profile.selections,
            profile.root_maximum_items,
            &input_map,
            &mut inferred_inputs,
            0,
        )?;
        if inferred_inputs.len() != input_map.len()
            || input_map
                .keys()
                .any(|name| !inferred_inputs.contains_key(*name))
        {
            return Err(configuration_error(
                "every profile input must be used exactly through the closed argument plan",
            ));
        }
        validate_inferred_inputs(&profile.inputs, &inferred_inputs, &self.schema)?;
        let response_shape = ProjectedShape::Object(BTreeMap::from([(
            profile.field_name.clone(),
            projected_shape,
        )]));
        validate_disclosure_shape(&profile.disclosure_schema.root, &response_shape)?;

        let operation_name = operation_name(
            &self.subgraph_id,
            profile.root_type,
            &profile.field_name,
            &profile.profile_id,
        );
        let variable_definitions = profile
            .inputs
            .iter()
            .map(|input| {
                let inferred = inferred_inputs
                    .get(&input.name)
                    .expect("all profile inputs were validated");
                let ty = if input.required && !inferred.ends_with('!') {
                    format!("{inferred}!")
                } else {
                    inferred.clone()
                };
                format!("${}: {}", input.name, ty)
            })
            .collect::<Vec<_>>();
        let variables = if variable_definitions.is_empty() {
            String::new()
        } else {
            format!("({})", variable_definitions.join(", "))
        };
        let arguments = if rendered_arguments.is_empty() {
            String::new()
        } else {
            format!("({})", rendered_arguments.join(", "))
        };
        let document = format!(
            "{} {}{} {{ {}{} {} }}",
            profile.root_type.keyword(),
            operation_name,
            variables,
            profile.field_name,
            arguments,
            rendered_projection,
        );
        async_graphql::parser::parse_query(&document)
            .map_err(|_| configuration_error("compiled GraphQL document is invalid"))?;

        let argument_schema = argument_json_schema(&profile.inputs)?;
        let projection_fingerprint = sha256_json(&json!({
            "format": "graphql-orm-ai-result-projection-v1",
            "root_type": profile.root_type,
            "field_name": profile.field_name,
            "selections": profile.selections,
            "root_maximum_items": profile.root_maximum_items,
        }));
        let contract = GraphqlOperationContract::new(
            self.target_id.clone(),
            self.finished_schema_fingerprint.clone(),
            operation_name,
            &document,
            projection_fingerprint.clone(),
            profile.disclosure_schema.fingerprint.clone(),
        )
        .map_err(|_| configuration_error("compiled GraphQL contract is invalid"))?;
        let maximum_classification = maximum_classification(&profile.disclosure_schema.root)?;
        let descriptor = AiToolDescriptor::new(
            stable_tool_id(
                &self.subgraph_id,
                profile.root_type,
                &profile.field_name,
                &profile.profile_id,
            ),
            profile.description,
            profile.root_type.operation_kind(),
            &document,
            argument_schema,
        )?
        .with_result_projection(projection_fingerprint)
        .with_graphql_contract(contract)
        .with_output_limits(profile.maximum_result_bytes, profile.maximum_result_records)
        .with_maximum_classification(maximum_classification)
        .with_maturity(match profile.execution {
            ProfileExecution::ReadOnly => ToolMaturity::ReadOnly,
            ProfileExecution::SupervisedMutation => ToolMaturity::SupervisedWrite,
        })
        .with_risk(profile.risk, profile.approval)
        .with_idempotent(profile.idempotent);
        Ok(AiGraphqlToolManifestEntry {
            root_type: profile.root_type,
            field_name: profile.field_name,
            profile_id: profile.profile_id,
            source,
            descriptor,
            disclosure_schema: profile.disclosure_schema,
        })
    }
}

#[derive(Clone, Debug)]
struct FinishedSchema {
    roots: BTreeMap<AiGraphqlRootType, String>,
    types: BTreeMap<String, SchemaType>,
}

#[derive(Clone, Debug)]
enum SchemaType {
    Scalar,
    Enum(BTreeSet<String>),
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
}

impl FinishedSchema {
    fn parse(sdl: &str) -> Result<Self, AiError> {
        if sdl.trim().is_empty() {
            return Err(configuration_error(
                "finished GraphQL SDL must not be empty",
            ));
        }
        let document = parse_schema(sdl)
            .map_err(|_| configuration_error("finished GraphQL SDL is invalid"))?;
        let mut roots = BTreeMap::new();
        let mut types = BTreeMap::new();
        for definition in &document.definitions {
            if let TypeSystemDefinition::Schema(schema) = definition {
                for (kind, root) in [
                    (AiGraphqlRootType::Query, schema.node.query.as_ref()),
                    (AiGraphqlRootType::Mutation, schema.node.mutation.as_ref()),
                ] {
                    if let Some(root) = root
                        && roots.insert(kind, root.node.to_string()).is_some()
                    {
                        return Err(configuration_error("finished schema repeats a root type"));
                    }
                }
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
                                },
                            )
                        })
                        .collect(),
                ),
                TypeKind::Interface(_) | TypeKind::Union(_) => SchemaType::Unsupported,
            };
            if types.insert(name, converted).is_some() {
                return Err(configuration_error(
                    "finished schema contains duplicate type definitions",
                ));
            }
        }
        for kind in [AiGraphqlRootType::Query, AiGraphqlRootType::Mutation] {
            if !roots.contains_key(&kind) && types.contains_key(kind.conventional_name()) {
                roots.insert(kind, kind.conventional_name().to_owned());
            }
        }
        if !roots.contains_key(&AiGraphqlRootType::Query) {
            return Err(configuration_error("finished schema has no query root"));
        }
        Ok(Self { roots, types })
    }

    fn root_name(&self, kind: AiGraphqlRootType) -> &str {
        self.roots
            .get(&kind)
            .map(String::as_str)
            .unwrap_or_else(|| kind.conventional_name())
    }

    fn object_field(&self, type_name: &str, field_name: &str) -> Result<&SchemaField, AiError> {
        let Some(SchemaType::Object(fields)) = self.types.get(type_name) else {
            return Err(configuration_error("GraphQL object type is missing"));
        };
        fields
            .get(field_name)
            .ok_or_else(|| configuration_error("GraphQL field is missing from finished schema"))
    }

    fn render_arguments(
        &self,
        schema_arguments: &BTreeMap<String, SchemaInput>,
        plans: &[AiGraphqlArgumentPlan],
        inputs: &BTreeMap<&str, &AiGraphqlProfileInput>,
        inferred_inputs: &mut BTreeMap<String, String>,
    ) -> Result<Vec<String>, AiError> {
        if plans.len() > MAXIMUM_PROFILE_INPUTS {
            return Err(configuration_error("argument plan is too large"));
        }
        let mut names = BTreeSet::new();
        let mut rendered = Vec::new();
        for plan in plans {
            validate_graphql_name(&plan.name, "argument name")?;
            if !names.insert(plan.name.as_str()) {
                return Err(configuration_error("argument plan repeats a name"));
            }
            let schema_input = schema_arguments
                .get(&plan.name)
                .ok_or_else(|| configuration_error("argument plan contains an unknown field"))?;
            let value =
                self.render_value(&schema_input.ty, &plan.value, inputs, inferred_inputs, 0)?;
            rendered.push(format!("{}: {}", plan.name, value));
        }
        for (name, input) in schema_arguments {
            if is_non_null(&input.ty) && !input.has_default && !names.contains(name.as_str()) {
                return Err(configuration_error(
                    "argument plan omits a required GraphQL argument",
                ));
            }
        }
        Ok(rendered)
    }

    fn render_value(
        &self,
        graphql_type: &str,
        value: &AiGraphqlArgumentValue,
        inputs: &BTreeMap<&str, &AiGraphqlProfileInput>,
        inferred_inputs: &mut BTreeMap<String, String>,
        depth: usize,
    ) -> Result<String, AiError> {
        if depth > MAXIMUM_PROJECTION_DEPTH {
            return Err(configuration_error("argument adapter nesting is too deep"));
        }
        match value {
            AiGraphqlArgumentValue::Input(name) => {
                validate_graphql_name(name, "profile input name")?;
                if !inputs.contains_key(name.as_str()) {
                    return Err(configuration_error(
                        "argument adapter references an unknown profile input",
                    ));
                }
                match inferred_inputs.get(name) {
                    Some(existing) if existing != graphql_type => {
                        return Err(configuration_error(
                            "profile input has conflicting GraphQL type uses",
                        ));
                    }
                    Some(_) => {}
                    None => {
                        inferred_inputs.insert(name.clone(), graphql_type.to_owned());
                    }
                }
                Ok(format!("${name}"))
            }
            AiGraphqlArgumentValue::Fixed(value) => self.render_fixed(graphql_type, value, depth),
            AiGraphqlArgumentValue::Object(fields) => {
                let named = named_type(graphql_type)?;
                let Some(SchemaType::InputObject(schema_fields)) = self.types.get(named) else {
                    return Err(configuration_error(
                        "object argument adapter does not target an input object",
                    ));
                };
                let plans = fields
                    .iter()
                    .map(|(name, value)| AiGraphqlArgumentPlan {
                        name: name.clone(),
                        value: value.clone(),
                    })
                    .collect::<Vec<_>>();
                let rendered =
                    self.render_arguments(schema_fields, &plans, inputs, inferred_inputs)?;
                Ok(format!("{{ {} }}", rendered.join(", ")))
            }
            AiGraphqlArgumentValue::List(values) => {
                let item_type = list_item_type(graphql_type).ok_or_else(|| {
                    configuration_error("list argument adapter does not target a list")
                })?;
                let rendered = values
                    .iter()
                    .map(|value| {
                        self.render_value(item_type, value, inputs, inferred_inputs, depth + 1)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(format!("[{}]", rendered.join(", ")))
            }
        }
    }

    fn render_fixed(
        &self,
        graphql_type: &str,
        value: &Value,
        depth: usize,
    ) -> Result<String, AiError> {
        if depth > MAXIMUM_PROJECTION_DEPTH {
            return Err(configuration_error("fixed argument nesting is too deep"));
        }
        if value.is_null() {
            return (!is_non_null(graphql_type))
                .then(|| "null".to_owned())
                .ok_or_else(|| configuration_error("fixed null targets a non-null input"));
        }
        if let Some(item_type) = list_item_type(graphql_type) {
            let values = value.as_array().ok_or_else(|| {
                configuration_error("fixed list argument has the wrong JSON shape")
            })?;
            let rendered = values
                .iter()
                .map(|value| self.render_fixed(item_type, value, depth + 1))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(format!("[{}]", rendered.join(", ")));
        }
        let named = named_type(graphql_type)?;
        match self.types.get(named) {
            Some(SchemaType::InputObject(fields)) => {
                let object = value.as_object().ok_or_else(|| {
                    configuration_error("fixed input object has the wrong JSON shape")
                })?;
                let plans = object
                    .iter()
                    .map(|(name, value)| {
                        AiGraphqlArgumentPlan::new(
                            name,
                            AiGraphqlArgumentValue::Fixed(value.clone()),
                        )
                    })
                    .collect::<Vec<_>>();
                let rendered =
                    self.render_arguments(fields, &plans, &BTreeMap::new(), &mut BTreeMap::new())?;
                Ok(format!("{{ {} }}", rendered.join(", ")))
            }
            Some(SchemaType::Enum(values)) => {
                let value = value
                    .as_str()
                    .ok_or_else(|| configuration_error("fixed enum argument must be a string"))?;
                if !values.contains(value) {
                    return Err(configuration_error("fixed enum value is not declared"));
                }
                Ok(value.to_owned())
            }
            Some(SchemaType::Scalar) | None => render_scalar(named, value),
            Some(SchemaType::Object(_)) => Err(configuration_error(
                "output object cannot be used as a fixed input",
            )),
            Some(SchemaType::Unsupported) => Err(configuration_error(
                "unsupported GraphQL type cannot be used as a fixed input",
            )),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_projection(
        &self,
        graphql_type: &str,
        selections: &[AiGraphqlSelection],
        maximum_items: Option<u32>,
        inputs: &BTreeMap<&str, &AiGraphqlProfileInput>,
        inferred_inputs: &mut BTreeMap<String, String>,
        depth: usize,
    ) -> Result<(String, ProjectedShape), AiError> {
        if depth > MAXIMUM_PROJECTION_DEPTH || selections.len() > MAXIMUM_SELECTIONS_PER_LEVEL {
            return Err(configuration_error("result projection exceeds safe bounds"));
        }
        let list = list_item_type(graphql_type);
        if list.is_some() != maximum_items.is_some() {
            return Err(configuration_error(
                "every projected list requires one explicit cardinality bound",
            ));
        }
        if maximum_items == Some(0) {
            return Err(configuration_error("result list bound must be positive"));
        }
        let item_type = list.unwrap_or(graphql_type);
        let named = named_type(item_type)?;
        let projected = match self.types.get(named) {
            Some(SchemaType::Scalar | SchemaType::Enum(_)) | None => {
                if !selections.is_empty() {
                    return Err(configuration_error("scalar projection has nested fields"));
                }
                ProjectedShape::Scalar
            }
            Some(SchemaType::Object(fields)) => {
                if selections.is_empty() {
                    return Err(configuration_error(
                        "object relationships are excluded unless explicitly projected",
                    ));
                }
                let mut response_names = BTreeSet::new();
                let mut projected_fields = BTreeMap::new();
                let mut rendered = Vec::new();
                for selection in selections {
                    validate_graphql_name(&selection.field_name, "selection field")?;
                    if let Some(alias) = &selection.alias {
                        validate_graphql_name(alias, "selection alias")?;
                    }
                    if !response_names.insert(selection.response_name()) {
                        return Err(configuration_error(
                            "projection contains duplicate or conflicting aliases",
                        ));
                    }
                    let field = fields.get(&selection.field_name).ok_or_else(|| {
                        configuration_error("projection contains an unknown field")
                    })?;
                    let arguments = self.render_arguments(
                        &field.arguments,
                        &selection.arguments,
                        inputs,
                        inferred_inputs,
                    )?;
                    let arguments = if arguments.is_empty() {
                        String::new()
                    } else {
                        format!("({})", arguments.join(", "))
                    };
                    let (nested, shape) = self.render_projection(
                        &field.ty,
                        &selection.selections,
                        selection.maximum_items,
                        inputs,
                        inferred_inputs,
                        depth + 1,
                    )?;
                    let alias = selection
                        .alias
                        .as_ref()
                        .map(|alias| format!("{alias}: "))
                        .unwrap_or_default();
                    rendered.push(format!(
                        "{alias}{}{}{}",
                        selection.field_name, arguments, nested
                    ));
                    projected_fields.insert(selection.response_name().to_owned(), shape);
                }
                return Ok((
                    format!("{{ {} }}", rendered.join(" ")),
                    wrap_list(ProjectedShape::Object(projected_fields), maximum_items),
                ));
            }
            Some(SchemaType::InputObject(_)) => {
                return Err(configuration_error("input object cannot be projected"));
            }
            Some(SchemaType::Unsupported) => {
                return Err(configuration_error(
                    "interface and union projections require an explicit future profile version",
                ));
            }
        };
        Ok((String::new(), wrap_list(projected, maximum_items)))
    }
}

#[derive(Clone, Debug)]
enum ProjectedShape {
    Scalar,
    Object(BTreeMap<String, ProjectedShape>),
    List(u32, Box<ProjectedShape>),
}

fn wrap_list(shape: ProjectedShape, maximum_items: Option<u32>) -> ProjectedShape {
    match maximum_items {
        Some(maximum_items) => ProjectedShape::List(maximum_items, Box::new(shape)),
        None => shape,
    }
}

fn validate_profile_shape(profile: &AiGraphqlToolProfile) -> Result<(), AiError> {
    validate_public_token(&profile.profile_id, "profile ID")?;
    validate_graphql_name(&profile.field_name, "root field")?;
    validate_model_description(&profile.description, "tool description")?;
    if profile.inputs.len() > MAXIMUM_PROFILE_INPUTS
        || profile.maximum_result_bytes == 0
        || profile.maximum_result_records == 0
        || profile.disclosure_schema.maximum_list_bound() > profile.maximum_result_records
        || profile.selections.is_empty()
    {
        return Err(configuration_error(
            "tool profile has invalid or empty bounds",
        ));
    }
    let mut names = BTreeSet::new();
    for argument in &profile.arguments {
        validate_argument_value_depth(&argument.value, 0)?;
    }
    validate_selection_plan_depth(&profile.selections, 0)?;
    for input in &profile.inputs {
        validate_graphql_name(&input.name, "profile input")?;
        validate_model_description(&input.description, "profile input description")?;
        if !names.insert(input.name.as_str()) {
            return Err(configuration_error("profile input names must be unique"));
        }
        match &input.input_type {
            AiGraphqlProfileInputType::String {
                minimum_length,
                maximum_length,
            } if maximum_length == &0 || minimum_length > maximum_length => {
                return Err(configuration_error("string input bounds are invalid"));
            }
            AiGraphqlProfileInputType::Integer { minimum, maximum } if minimum > maximum => {
                return Err(configuration_error("integer input bounds are invalid"));
            }
            AiGraphqlProfileInputType::Number { minimum, maximum }
                if !minimum.is_finite() || !maximum.is_finite() || minimum > maximum =>
            {
                return Err(configuration_error("number input bounds are invalid"));
            }
            AiGraphqlProfileInputType::Enum { values } => {
                let unique = values.iter().collect::<BTreeSet<_>>();
                if values.is_empty()
                    || unique.len() != values.len()
                    || values
                        .iter()
                        .any(|value| validate_graphql_name(value, "enum value").is_err())
                {
                    return Err(configuration_error("enum input values are invalid"));
                }
            }
            AiGraphqlProfileInputType::String { .. }
            | AiGraphqlProfileInputType::Integer { .. }
            | AiGraphqlProfileInputType::Number { .. }
            | AiGraphqlProfileInputType::Boolean => {}
        }
    }
    match (profile.execution, profile.root_type) {
        (ProfileExecution::ReadOnly, AiGraphqlRootType::Query)
            if profile.risk == AiToolRisk::ReadOnly
                && profile.approval == AiApprovalRule::None
                && profile.idempotent =>
        {
            Ok(())
        }
        (ProfileExecution::SupervisedMutation, AiGraphqlRootType::Mutation)
            if matches!(
                profile.risk,
                AiToolRisk::LowRiskWrite | AiToolRisk::NonIdempotentWrite | AiToolRisk::HighImpact
            ) && profile.approval == AiApprovalRule::OneShot =>
        {
            Ok(())
        }
        _ => Err(configuration_error(
            "tool execution profile has an unsafe root kind",
        )),
    }
}

fn validate_compiled_descriptor(entry: &AiGraphqlToolManifestEntry) -> Result<(), AiError> {
    let descriptor = &entry.descriptor;
    let safe_read = entry.root_type == AiGraphqlRootType::Query
        && descriptor.maturity == ToolMaturity::ReadOnly
        && descriptor.risk == AiToolRisk::ReadOnly
        && descriptor.approval == AiApprovalRule::None
        && descriptor.idempotent;
    let supervised_write = entry.root_type == AiGraphqlRootType::Mutation
        && descriptor.maturity == ToolMaturity::SupervisedWrite
        && matches!(
            descriptor.risk,
            AiToolRisk::LowRiskWrite | AiToolRisk::NonIdempotentWrite | AiToolRisk::HighImpact
        )
        && descriptor.approval == AiApprovalRule::OneShot;
    if !descriptor.has_valid_fingerprint()
        || descriptor.operation_domain != AiToolOperationDomain::Application
        || descriptor.maximum_result_bytes == 0
        || descriptor.maximum_result_records == 0
        || descriptor.maximum_classification
            != maximum_classification(&entry.disclosure_schema.root)?
        || !(safe_read || supervised_write)
    {
        return Err(configuration_error(
            "compiled GraphQL tool descriptor has unsafe execution semantics",
        ));
    }
    validate_model_description(&descriptor.description, "tool description")
}

fn validate_argument_value_depth(
    value: &AiGraphqlArgumentValue,
    depth: usize,
) -> Result<(), AiError> {
    if depth > MAXIMUM_PROJECTION_DEPTH {
        return Err(configuration_error("argument adapter nesting is too deep"));
    }
    match value {
        AiGraphqlArgumentValue::Object(fields) => {
            if fields.len() > MAXIMUM_PROFILE_INPUTS {
                return Err(configuration_error("argument adapter is too large"));
            }
            for value in fields.values() {
                validate_argument_value_depth(value, depth + 1)?;
            }
        }
        AiGraphqlArgumentValue::List(values) => {
            if values.len() > MAXIMUM_PROFILE_INPUTS {
                return Err(configuration_error("argument adapter is too large"));
            }
            for value in values {
                validate_argument_value_depth(value, depth + 1)?;
            }
        }
        AiGraphqlArgumentValue::Input(_) | AiGraphqlArgumentValue::Fixed(_) => {}
    }
    Ok(())
}

fn validate_selection_plan_depth(
    selections: &[AiGraphqlSelection],
    depth: usize,
) -> Result<(), AiError> {
    if depth > MAXIMUM_PROJECTION_DEPTH || selections.len() > MAXIMUM_SELECTIONS_PER_LEVEL {
        return Err(configuration_error("result projection exceeds safe bounds"));
    }
    for selection in selections {
        for argument in &selection.arguments {
            validate_argument_value_depth(&argument.value, depth)?;
        }
        validate_selection_plan_depth(&selection.selections, depth + 1)?;
    }
    Ok(())
}

fn validate_inferred_inputs(
    inputs: &[AiGraphqlProfileInput],
    inferred: &BTreeMap<String, String>,
    schema: &FinishedSchema,
) -> Result<(), AiError> {
    for input in inputs {
        let graphql_type = inferred
            .get(&input.name)
            .ok_or_else(|| configuration_error("profile input is unused"))?;
        if is_non_null(graphql_type) && !input.required {
            return Err(configuration_error(
                "optional profile input targets a required GraphQL position",
            ));
        }
        if list_item_type(graphql_type).is_some() {
            return Err(configuration_error(
                "model-facing list inputs require a separately reviewed adapter",
            ));
        }
        let named = named_type(graphql_type)?;
        let compatible = match &input.input_type {
            AiGraphqlProfileInputType::String { .. } => matches!(named, "String" | "ID"),
            AiGraphqlProfileInputType::Integer { .. } => named == "Int",
            AiGraphqlProfileInputType::Number { .. } => named == "Float",
            AiGraphqlProfileInputType::Boolean => named == "Boolean",
            AiGraphqlProfileInputType::Enum { values } => {
                let Some(SchemaType::Enum(schema_values)) = schema.types.get(named) else {
                    return Err(configuration_error("profile enum type is not declared"));
                };
                values.iter().all(|value| schema_values.contains(value))
            }
        };
        if !compatible {
            return Err(configuration_error(
                "profile input constraints do not match the finished schema",
            ));
        }
    }
    Ok(())
}

fn argument_json_schema(inputs: &[AiGraphqlProfileInput]) -> Result<Value, AiError> {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for input in inputs {
        let mut property = match &input.input_type {
            AiGraphqlProfileInputType::String {
                minimum_length,
                maximum_length,
            } => json!({
                "type": "string",
                "minLength": minimum_length,
                "maxLength": maximum_length,
            }),
            AiGraphqlProfileInputType::Integer { minimum, maximum } => json!({
                "type": "integer",
                "minimum": minimum,
                "maximum": maximum,
            }),
            AiGraphqlProfileInputType::Number { minimum, maximum } => json!({
                "type": "number",
                "minimum": minimum,
                "maximum": maximum,
            }),
            AiGraphqlProfileInputType::Boolean => json!({ "type": "boolean" }),
            AiGraphqlProfileInputType::Enum { values } => json!({
                "type": "string",
                "enum": values,
            }),
        };
        property
            .as_object_mut()
            .expect("input JSON Schema is an object")
            .insert(
                "description".to_owned(),
                Value::String(input.description.clone()),
            );
        properties.insert(input.name.clone(), property);
        if input.required {
            required.push(Value::String(input.name.clone()));
        }
    }
    Ok(json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    }))
}

fn validate_disclosure_shape(
    disclosure: &AiDisclosureShape,
    projection: &ProjectedShape,
) -> Result<(), AiError> {
    match (disclosure, projection) {
        (AiDisclosureShape::Scalar { rule }, ProjectedShape::Scalar) => {
            validate_disclosure_rule(*rule)
        }
        (AiDisclosureShape::Object { rule, fields }, ProjectedShape::Object(projected)) => {
            validate_disclosure_rule(*rule)?;
            if fields.len() != projected.len()
                || fields.keys().any(|field| !projected.contains_key(field))
            {
                return Err(configuration_error(
                    "disclosure schema does not exactly match the projection",
                ));
            }
            for (field, shape) in fields {
                validate_disclosure_shape(
                    shape,
                    projected
                        .get(field)
                        .expect("field sets were compared before lookup"),
                )?;
            }
            Ok(())
        }
        (
            AiDisclosureShape::List {
                rule,
                maximum_items,
                item,
            },
            ProjectedShape::List(projected_maximum, projected_item),
        ) if maximum_items == projected_maximum => {
            validate_disclosure_rule(*rule)?;
            validate_disclosure_shape(item, projected_item)
        }
        _ => Err(configuration_error(
            "disclosure schema does not exactly match the projection",
        )),
    }
}

fn validate_disclosure_rule(rule: crate::AiDisclosureRule) -> Result<(), AiError> {
    if rule.disposition == AiDisclosureDisposition::NeverExport {
        return Err(configuration_error(
            "a selected tool result field cannot be marked never-export",
        ));
    }
    Ok(())
}

fn maximum_classification(shape: &AiDisclosureShape) -> Result<DataClassification, AiError> {
    let (rule, nested) = match shape {
        AiDisclosureShape::Scalar { rule } => (*rule, Vec::new()),
        AiDisclosureShape::Object { rule, fields } => (*rule, fields.values().collect::<Vec<_>>()),
        AiDisclosureShape::List { rule, item, .. } => (*rule, vec![item.as_ref()]),
    };
    validate_disclosure_rule(rule)?;
    nested
        .into_iter()
        .try_fold(rule.classification, |maximum, child| {
            Ok(maximum.max(maximum_classification(child)?))
        })
}

fn render_scalar(name: &str, value: &Value) -> Result<String, AiError> {
    match name {
        "String" | "ID" => value
            .as_str()
            .map(|value| serde_json::to_string(value).expect("strings always encode"))
            .ok_or_else(|| configuration_error("fixed string argument has wrong JSON type")),
        "Int" => value
            .as_i64()
            .map(|value| value.to_string())
            .ok_or_else(|| configuration_error("fixed integer argument has wrong JSON type")),
        "Float" => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(|value| value.to_string())
            .ok_or_else(|| configuration_error("fixed number argument has wrong JSON type")),
        "Boolean" => value
            .as_bool()
            .map(|value| value.to_string())
            .ok_or_else(|| configuration_error("fixed boolean argument has wrong JSON type")),
        _ => Err(configuration_error(
            "fixed custom scalar arguments require an explicit reusable adapter",
        )),
    }
}

fn is_non_null(graphql_type: &str) -> bool {
    graphql_type.trim().ends_with('!')
}

fn named_type(graphql_type: &str) -> Result<&str, AiError> {
    let mut value = graphql_type.trim();
    if let Some(stripped) = value.strip_suffix('!') {
        value = stripped.trim();
    }
    if value.starts_with('[') {
        return Err(configuration_error("expected a named GraphQL type"));
    }
    validate_graphql_name(value, "GraphQL type")?;
    Ok(value)
}

fn list_item_type(graphql_type: &str) -> Option<&str> {
    let mut value = graphql_type.trim();
    if let Some(stripped) = value.strip_suffix('!') {
        value = stripped.trim();
    }
    value.strip_prefix('[')?.strip_suffix(']').map(str::trim)
}

fn finished_schema_fingerprint(sdl: &str) -> String {
    hex::encode(Sha256::digest(sdl.as_bytes()))
}

fn operation_name(
    subgraph_id: &str,
    root: AiGraphqlRootType,
    field: &str,
    profile_id: &str,
) -> String {
    let hash = identity_hash(subgraph_id, root, field, profile_id);
    format!("AiTool_{}", &hash[..20])
}

fn stable_tool_id(
    subgraph_id: &str,
    root: AiGraphqlRootType,
    field: &str,
    profile_id: &str,
) -> String {
    let safe_subgraph = subgraph_id.to_ascii_lowercase();
    let safe_field = field.to_ascii_lowercase();
    let hash = identity_hash(subgraph_id, root, field, profile_id);
    format!(
        "{safe_subgraph}.{}.{}.{}-{}",
        root.keyword(),
        safe_field,
        profile_id,
        &hash[..16]
    )
}

fn identity_hash(
    subgraph_id: &str,
    root: AiGraphqlRootType,
    field: &str,
    profile_id: &str,
) -> String {
    let value = format!(
        "graphql-orm-ai-tool-identity-v1\0{subgraph_id}\0{}.{}\0{profile_id}",
        root.conventional_name(),
        field
    );
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn sha256_json(value: &Value) -> String {
    let encoded = serde_json::to_vec(value).expect("fingerprint value always serializes");
    hex::encode(Sha256::digest(encoded))
}

fn validate_public_token(value: &str, label: &str) -> Result<(), AiError> {
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
    {
        return Err(configuration_error(format!(
            "{label} must be a bounded lower-case public token"
        )));
    }
    Ok(())
}

fn validate_graphql_name(value: &str, label: &str) -> Result<(), AiError> {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return Err(configuration_error(format!("{label} must not be empty")));
    };
    if value.len() > 256
        || !(first == b'_' || first.is_ascii_alphabetic())
        || !bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
        || value.starts_with("__")
    {
        return Err(configuration_error(format!(
            "{label} must be a safe GraphQL name"
        )));
    }
    Ok(())
}

fn validate_model_description(value: &str, label: &str) -> Result<(), AiError> {
    const INTERNAL_TERMS: &[&str] = &[
        "authorization",
        "column",
        "database",
        "permission",
        "resolver",
        "scope",
        "sql",
        "table",
    ];
    if value.trim().is_empty()
        || value.len() > MAXIMUM_DESCRIPTION_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(configuration_error(format!("{label} is not model-safe")));
    }
    let words = value
        .split(|character: char| !character.is_ascii_alphabetic())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<BTreeSet<_>>();
    if INTERNAL_TERMS.iter().any(|term| words.contains(*term)) {
        return Err(configuration_error(format!(
            "{label} contains implementation or policy internals"
        )));
    }
    Ok(())
}

fn configuration_error(message: impl Into<String>) -> AiError {
    AiError::InvalidConfiguration(message.into())
}
