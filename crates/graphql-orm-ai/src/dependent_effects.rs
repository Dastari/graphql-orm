//! AI delete policy for dependent cascade and set-null effects.
//!
//! A generated delete root is classified on its own: the operation's AI
//! execution policy says whether the model may delete that entity. Deleting a
//! row can also delete or rewrite rows of other entities, through declared
//! dependent deletes or physical foreign-key actions. Those rows belong to
//! entities with their own AI policy, which the root's classification says
//! nothing about.
//!
//! [`AiDependentEffectPolicy`] closes that gap for a host that installs it.
//! The host declares, per delete root, the complete set of dependent effects
//! the delete can cause. Before an AI-initiated generated delete is prepared
//! or executed, every declared effect is checked against the dependent
//! entity's own AI policy. A root with no declaration is unproven and is
//! refused.

use std::collections::BTreeMap;

use graphql_orm::graphql::orm::AiMutationExecutionPolicy;
use graphql_orm_operation_catalog::GeneratedGraphqlOperationCategory;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{AiError, AiToolId, GraphqlExecutionTargetId};

const MAXIMUM_EFFECTS_PER_ROOT: usize = 256;
const MAXIMUM_NAME_BYTES: usize = 256;

/// What a delete does to rows of one dependent entity.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum AiDependentEffectKind {
    /// Dependent rows are deleted (a cascade).
    Delete,
    /// The named foreign-key field of dependent rows is set to null.
    SetNull {
        /// Public field of the dependent entity that is cleared.
        field: String,
    },
}

/// One dependent effect a delete root can cause.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiDependentEffect {
    entity: String,
    effect: AiDependentEffectKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    maximum_rows: Option<u32>,
}

impl AiDependentEffect {
    /// Rows of `entity` are deleted with the root.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] unless `entity` is a public
    /// GraphQL type name.
    pub fn delete(entity: impl Into<String>) -> Result<Self, AiError> {
        Self::new(entity.into(), AiDependentEffectKind::Delete)
    }

    /// `field` of rows of `entity` is set to null when the root is deleted.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] unless `entity` and `field`
    /// are public GraphQL names.
    pub fn set_null(entity: impl Into<String>, field: impl Into<String>) -> Result<Self, AiError> {
        let field = field.into();
        if !valid_name(&field) {
            return Err(invalid("dependent effect field name is invalid"));
        }
        Self::new(entity.into(), AiDependentEffectKind::SetNull { field })
    }

    fn new(entity: String, effect: AiDependentEffectKind) -> Result<Self, AiError> {
        if !valid_name(&entity) {
            return Err(invalid("dependent effect entity name is invalid"));
        }
        Ok(Self {
            entity,
            effect,
            maximum_rows: None,
        })
    }

    /// Maximum dependent rows this effect may touch in one delete. The ORM
    /// counts the locked execution plan inside its transaction and refuses
    /// before writes when this bound is exceeded.
    #[must_use]
    pub const fn with_maximum_rows(mut self, maximum_rows: u32) -> Self {
        self.maximum_rows = Some(maximum_rows);
        self
    }

    /// Public dependent entity name.
    pub fn entity(&self) -> &str {
        &self.entity
    }

    /// What happens to the dependent rows.
    pub const fn effect(&self) -> &AiDependentEffectKind {
        &self.effect
    }

    /// Declared row bound, when the host declared one.
    pub const fn maximum_rows(&self) -> Option<u32> {
        self.maximum_rows
    }
}

/// Why an AI-initiated delete was refused by [`AiDependentEffectPolicy`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AiDependentEffectRefusal {
    /// The host declared no effect set for this delete root, so its
    /// dependents are unproven.
    UndeclaredRoot,
    /// The delete root does not name the entity it deletes.
    UnknownRootEntity,
    /// A dependent entity's own AI policy does not allow the effect.
    DependentPolicy {
        /// Dependent entity whose policy refused.
        entity: String,
    },
    /// An effect declares no row bound, or one above the policy ceiling.
    RowBound {
        /// Dependent entity whose bound is missing or too large.
        entity: String,
    },
}

/// The AI-relevant facts of one registered generated mutation.
///
/// `admitted` is whether the deployment's generated target policy currently
/// admits the mutation under its own execution policy.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AiMutationFacts<'a> {
    pub(crate) target_id: &'a GraphqlExecutionTargetId,
    pub(crate) entity: Option<&'a str>,
    pub(crate) category: Option<GeneratedGraphqlOperationCategory>,
    pub(crate) execution_policy: AiMutationExecutionPolicy,
    pub(crate) admitted: bool,
}

/// Effect-aware AI policy for generated delete roots.
///
/// The policy holds the host's declaration of what each delete root does to
/// other entities. Install it with
/// [`crate::AiRuntimeBuilder::dependent_effect_policy`]. From then on every
/// generated `Delete` or `DeleteMany` capability is checked before it is
/// prepared, preauthorized, approved or executed:
///
/// - a root with no declaration is refused, because its dependents are
///   unproven. Declare an empty effect set for a root that has none;
/// - each declared delete effect needs the dependent entity's own generated
///   `Delete` capability, and each set-null effect its own generated `Update`
///   capability, registered and admitted by the generated target policy;
/// - the dependent's execution policy must be no stricter than the root's:
///   an automatic root needs automatic dependents, and an approval-required
///   root accepts automatic or approval-required dependents. A dependent
///   entity whose own operation is prohibited or absent refuses the root;
/// - with [`Self::with_maximum_dependent_rows`], each effect must declare a
///   row bound no larger than the ceiling.
///
/// Nothing is inherited from the root: an allowed root never makes a
/// dependent effect allowed.
///
/// # What this proves and does not prove
///
/// Declarations are checked against the ORM's physical dependency graph and
/// locked row plan before writes. The executor must implement
/// [`crate::AuthenticatedGraphqlExecutor::execute_registered_with_effect_guard`]
/// and attach the supplied guard to request data. Unsupported execution paths,
/// unknown dependencies and effects not modeled by that planner refuse. Physical
/// opt-out and bulk deletes refuse rather than trusting host declarations.
///
/// The policy is an AI decision made before execution. It is independent of,
/// and never replaces, GraphQL entity, row and field authorization: the ORM
/// still authorizes the root and every dependent write it plans, and still
/// rolls the whole delete back when any of them is refused. Reviewed static
/// mutation tools are host-authored documents and are not covered.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiDependentEffectPolicy {
    roots: BTreeMap<GraphqlExecutionTargetId, BTreeMap<String, Vec<AiDependentEffect>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    maximum_dependent_rows: Option<u32>,
}

impl AiDependentEffectPolicy {
    /// Creates a policy with no declared root. Installed as is, it refuses
    /// every generated delete.
    pub fn new() -> Self {
        Self::default()
    }

    /// Declares the complete dependent effect set of deleting `root_entity`
    /// on one logical target. An empty set declares that the delete touches
    /// no other entity.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] for an invalid entity name
    /// or more than 256 effects, and [`AiError::AlreadyExists`] when the root
    /// is already declared for the target.
    pub fn declare_delete(
        mut self,
        target_id: GraphqlExecutionTargetId,
        root_entity: impl Into<String>,
        effects: impl IntoIterator<Item = AiDependentEffect>,
    ) -> Result<Self, AiError> {
        let root_entity = root_entity.into();
        let mut effects = effects.into_iter().collect::<Vec<_>>();
        if !valid_name(&root_entity) || effects.len() > MAXIMUM_EFFECTS_PER_ROOT {
            return Err(invalid("dependent effect declaration is invalid"));
        }
        effects.sort();
        effects.dedup();
        let roots = self.roots.entry(target_id).or_default();
        if roots.contains_key(&root_entity) {
            return Err(AiError::AlreadyExists(root_entity));
        }
        roots.insert(root_entity, effects);
        Ok(self)
    }

    /// Refuses any effect that declares no row bound or one above `maximum`.
    #[must_use]
    pub const fn with_maximum_dependent_rows(mut self, maximum: u32) -> Self {
        self.maximum_dependent_rows = Some(maximum);
        self
    }

    /// Declared effect set of one delete root, or `None` when undeclared.
    pub fn declared_effects(
        &self,
        target_id: &GraphqlExecutionTargetId,
        root_entity: &str,
    ) -> Option<&[AiDependentEffect]> {
        self.roots
            .get(target_id)?
            .get(root_entity)
            .map(Vec::as_slice)
    }

    /// Deterministic fingerprint of the complete declaration.
    pub fn fingerprint(&self) -> String {
        let encoded = serde_json::to_vec(self)
            .expect("a dependent effect policy contains only serializable public names");
        hex::encode(Sha256::digest(encoded))
    }

    /// Decides one registered generated mutation against the facts of every
    /// registered generated mutation.
    ///
    /// # Errors
    ///
    /// Returns the first refusal for a delete root. A mutation that is not a
    /// generated delete is not this policy's concern and is always `Ok`.
    pub(crate) fn permits<'a>(
        &self,
        root: AiMutationFacts<'a>,
        registered: impl Iterator<Item = AiMutationFacts<'a>> + Clone,
    ) -> Result<(), AiDependentEffectRefusal> {
        if !matches!(
            root.category,
            Some(
                GeneratedGraphqlOperationCategory::Delete
                    | GeneratedGraphqlOperationCategory::DeleteMany
            )
        ) {
            return Ok(());
        }
        let root_entity = root
            .entity
            .ok_or(AiDependentEffectRefusal::UnknownRootEntity)?;
        let effects = self
            .declared_effects(root.target_id, root_entity)
            .ok_or(AiDependentEffectRefusal::UndeclaredRoot)?;
        for effect in effects {
            if self
                .maximum_dependent_rows
                .is_some_and(|maximum| effect.maximum_rows.is_none_or(|rows| rows > maximum))
            {
                return Err(AiDependentEffectRefusal::RowBound {
                    entity: effect.entity.clone(),
                });
            }
            let required = match effect.effect {
                AiDependentEffectKind::Delete => GeneratedGraphqlOperationCategory::Delete,
                AiDependentEffectKind::SetNull { .. } => GeneratedGraphqlOperationCategory::Update,
            };
            // The dependent entity's own policy for the same kind of write,
            // never the root's.
            let allowed = registered.clone().any(|candidate| {
                candidate.target_id == root.target_id
                    && candidate.entity == Some(effect.entity.as_str())
                    && candidate.category == Some(required)
                    && candidate.admitted
                    && strictness(candidate.execution_policy) <= strictness(root.execution_policy)
            });
            if !allowed {
                return Err(AiDependentEffectRefusal::DependentPolicy {
                    entity: effect.entity.clone(),
                });
            }
        }
        Ok(())
    }
}

/// Applies an installed policy to one registered generated mutation, using
/// the registered mutation capabilities and the deployment's generated target
/// policy as the dependent entities' own AI policy.
///
/// # Errors
///
/// Returns [`AiError::Forbidden`] for an unregistered capability or any
/// refusal. Without a policy every mutation passes.
pub(crate) fn permit_generated_mutation(
    policy: Option<&AiDependentEffectPolicy>,
    catalog: &crate::AiToolCatalog,
    targets: &crate::AiGeneratedGraphqlTargetPolicySet,
    capability_id: &AiToolId,
    execution_policy: AiMutationExecutionPolicy,
) -> Result<(), AiError> {
    let Some(policy) = policy else {
        return Ok(());
    };
    let root = catalog
        .mutation_capability(capability_id)
        .ok_or(AiError::Forbidden)?;
    let registered = catalog
        .mutation_capabilities()
        .map(|capability| mutation_facts(targets, capability, capability.execution_policy()))
        .collect::<Vec<_>>();
    policy
        .permits(
            mutation_facts(targets, root, execution_policy),
            registered.iter().copied(),
        )
        .map_err(|_| AiError::Forbidden)
}

fn mutation_facts<'a>(
    targets: &crate::AiGeneratedGraphqlTargetPolicySet,
    capability: &'a crate::AiGraphqlMutationCapability,
    execution_policy: AiMutationExecutionPolicy,
) -> AiMutationFacts<'a> {
    AiMutationFacts {
        target_id: capability.target_id(),
        entity: capability.generated_entity_name(),
        category: capability.generated_category(),
        execution_policy,
        admitted: targets.allows_mutation_capability(capability),
    }
}

/// Orders execution policies from least to most restrictive.
const fn strictness(policy: AiMutationExecutionPolicy) -> u8 {
    match policy {
        AiMutationExecutionPolicy::Automatic => 0,
        AiMutationExecutionPolicy::ApprovalRequired => 1,
        AiMutationExecutionPolicy::Prohibited => 2,
    }
}

fn valid_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    value.len() <= MAXIMUM_NAME_BYTES
        && bytes
            .next()
            .is_some_and(|byte| byte == b'_' || byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

fn invalid(message: &str) -> AiError {
    AiError::InvalidConfiguration(message.to_owned())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use AiMutationExecutionPolicy::{ApprovalRequired, Automatic, Prohibited};
    use GeneratedGraphqlOperationCategory::{Create, Delete, DeleteMany, Update};

    fn target() -> GraphqlExecutionTargetId {
        GraphqlExecutionTargetId::parse("records.graphql").expect("target")
    }

    fn facts<'a>(
        target_id: &'a GraphqlExecutionTargetId,
        entity: &'a str,
        category: GeneratedGraphqlOperationCategory,
        execution_policy: AiMutationExecutionPolicy,
    ) -> AiMutationFacts<'a> {
        AiMutationFacts {
            target_id,
            entity: Some(entity),
            category: Some(category),
            execution_policy,
            admitted: true,
        }
    }

    fn policy(effects: impl IntoIterator<Item = AiDependentEffect>) -> AiDependentEffectPolicy {
        AiDependentEffectPolicy::new()
            .declare_delete(target(), "Folder", effects)
            .expect("declaration")
    }

    #[test]
    fn a_delete_root_without_a_declaration_is_refused() {
        let target = target();
        let root = facts(&target, "Folder", Delete, Automatic);
        assert_eq!(
            AiDependentEffectPolicy::new().permits(root, [root].into_iter()),
            Err(AiDependentEffectRefusal::UndeclaredRoot)
        );
        // A declaration for another target or entity proves nothing here.
        let other = AiDependentEffectPolicy::new()
            .declare_delete(
                GraphqlExecutionTargetId::parse("other.graphql").expect("target"),
                "Folder",
                [],
            )
            .expect("declaration")
            .declare_delete(target.clone(), "Document", [])
            .expect("declaration");
        assert_eq!(
            other.permits(root, [root].into_iter()),
            Err(AiDependentEffectRefusal::UndeclaredRoot)
        );
        let mut unnamed = root;
        unnamed.entity = None;
        assert_eq!(
            policy([]).permits(unnamed, [root].into_iter()),
            Err(AiDependentEffectRefusal::UnknownRootEntity)
        );
    }

    #[test]
    fn a_root_declared_to_have_no_dependents_is_allowed() {
        let target = target();
        for category in [Delete, DeleteMany] {
            let root = facts(&target, "Folder", category, Automatic);
            assert_eq!(policy([]).permits(root, [root].into_iter()), Ok(()));
        }
    }

    #[test]
    fn a_cascade_needs_the_dependent_entitys_own_delete_policy() {
        let target = target();
        let policy = policy([AiDependentEffect::delete("Document").expect("effect")]);
        let root = facts(&target, "Folder", Delete, Automatic);
        let refused = Err(AiDependentEffectRefusal::DependentPolicy {
            entity: "Document".to_owned(),
        });

        // The dependent entity offers no AI delete at all.
        assert_eq!(policy.permits(root, [root].into_iter()), refused);
        // The root's own classification is never inherited.
        assert_eq!(
            policy.permits(
                root,
                [root, facts(&target, "Document", Update, Automatic)].into_iter()
            ),
            refused
        );
        // A dependent delete that needs approval cannot ride an automatic root.
        assert_eq!(
            policy.permits(
                root,
                [root, facts(&target, "Document", Delete, ApprovalRequired)].into_iter()
            ),
            refused
        );
        assert_eq!(
            policy.permits(
                root,
                [root, facts(&target, "Document", Delete, Prohibited)].into_iter()
            ),
            refused
        );
        // A registered dependent delete the target policy does not admit.
        let mut disabled = facts(&target, "Document", Delete, Automatic);
        disabled.admitted = false;
        assert_eq!(policy.permits(root, [root, disabled].into_iter()), refused);
        // The same entity name on another target is another entity.
        let other = GraphqlExecutionTargetId::parse("other.graphql").expect("target");
        assert_eq!(
            policy.permits(
                root,
                [root, facts(&other, "Document", Delete, Automatic)].into_iter()
            ),
            refused
        );

        assert_eq!(
            policy.permits(
                root,
                [root, facts(&target, "Document", Delete, Automatic)].into_iter()
            ),
            Ok(())
        );
        // An approval-required root accepts a dependent that is no stricter.
        let supervised = facts(&target, "Folder", Delete, ApprovalRequired);
        for dependent in [Automatic, ApprovalRequired] {
            assert_eq!(
                policy.permits(
                    supervised,
                    [supervised, facts(&target, "Document", Delete, dependent)].into_iter()
                ),
                Ok(())
            );
        }
    }

    #[test]
    fn a_set_null_needs_the_dependent_entitys_own_update_policy() {
        let target = target();
        let policy = policy([AiDependentEffect::set_null("Document", "folderId").expect("effect")]);
        let root = facts(&target, "Folder", Delete, Automatic);
        // Being allowed to delete the dependent does not allow rewriting it.
        assert_eq!(
            policy.permits(
                root,
                [root, facts(&target, "Document", Delete, Automatic)].into_iter()
            ),
            Err(AiDependentEffectRefusal::DependentPolicy {
                entity: "Document".to_owned(),
            })
        );
        assert_eq!(
            policy.permits(
                root,
                [root, facts(&target, "Document", Update, Automatic)].into_iter()
            ),
            Ok(())
        );
    }

    #[test]
    fn a_mixed_graph_is_refused_when_any_effect_is_refused() {
        let target = target();
        let policy = policy([
            AiDependentEffect::delete("Document").expect("effect"),
            AiDependentEffect::delete("Revision").expect("effect"),
            AiDependentEffect::set_null("Bookmark", "folderId").expect("effect"),
        ]);
        let root = facts(&target, "Folder", Delete, Automatic);
        let registered = [
            root,
            facts(&target, "Document", Delete, Automatic),
            facts(&target, "Revision", Delete, Automatic),
            facts(&target, "Bookmark", Update, Automatic),
        ];
        assert_eq!(policy.permits(root, registered.into_iter()), Ok(()));
        // Remove each dependent's policy in turn: the root is refused and the
        // refusal names that dependent.
        for missing in ["Document", "Revision", "Bookmark"] {
            assert_eq!(
                policy.permits(
                    root,
                    registered
                        .into_iter()
                        .filter(|facts| facts.entity != Some(missing))
                ),
                Err(AiDependentEffectRefusal::DependentPolicy {
                    entity: missing.to_owned(),
                })
            );
        }
    }

    #[test]
    fn a_row_ceiling_refuses_unbounded_and_oversized_effects() {
        let target = target();
        let root = facts(&target, "Folder", Delete, Automatic);
        let registered = [root, facts(&target, "Document", Delete, Automatic)];
        let bounded = |effect: AiDependentEffect| {
            policy([effect])
                .with_maximum_dependent_rows(100)
                .permits(root, registered.into_iter())
        };
        let refused = Err(AiDependentEffectRefusal::RowBound {
            entity: "Document".to_owned(),
        });
        let effect = AiDependentEffect::delete("Document").expect("effect");
        assert_eq!(bounded(effect.clone()), refused);
        assert_eq!(bounded(effect.clone().with_maximum_rows(101)), refused);
        assert_eq!(bounded(effect.clone().with_maximum_rows(100)), Ok(()));
        // Without a ceiling a declared bound is informational.
        assert_eq!(
            policy([effect]).permits(root, registered.into_iter()),
            Ok(())
        );
    }

    #[test]
    fn other_mutations_are_not_this_policys_concern() {
        let target = target();
        let empty = AiDependentEffectPolicy::new();
        for category in [Create, Update] {
            let root = facts(&target, "Folder", category, Automatic);
            assert_eq!(empty.permits(root, [root].into_iter()), Ok(()));
        }
        let mut custom = facts(&target, "Folder", Delete, Automatic);
        custom.category = None;
        assert_eq!(empty.permits(custom, [custom].into_iter()), Ok(()));
    }

    #[test]
    fn declarations_are_validated_and_fingerprinted() {
        assert!(AiDependentEffect::delete("not a name").is_err());
        assert!(AiDependentEffect::set_null("Document", "folder id").is_err());
        assert!(matches!(
            policy([]).declare_delete(target(), "Folder", []),
            Err(AiError::AlreadyExists(_))
        ));
        assert!(
            AiDependentEffectPolicy::new()
                .declare_delete(target(), "bad name", [])
                .is_err()
        );
        let document = AiDependentEffect::delete("Document").expect("effect");
        let revision = AiDependentEffect::delete("Revision").expect("effect");
        // Declaration order does not matter; content does.
        assert_eq!(
            policy([document.clone(), revision.clone()]).fingerprint(),
            policy([revision, document.clone()]).fingerprint()
        );
        assert_ne!(
            policy([document.clone()]).fingerprint(),
            policy([document.clone().with_maximum_rows(10)]).fingerprint()
        );
        let declared = policy([document.clone()]);
        assert_eq!(
            declared.declared_effects(&target(), "Folder"),
            Some([document].as_slice())
        );
        assert_eq!(declared.declared_effects(&target(), "Document"), None);
        let decoded: AiDependentEffectPolicy =
            serde_json::from_value(serde_json::to_value(&declared).expect("encode"))
                .expect("decode");
        assert_eq!(decoded, declared);
    }

    pub(crate) mod registered_capabilities {
        use graphql_orm_operation_catalog::{
            GraphqlEntitySemanticMetadata, GraphqlOperationCatalog, GraphqlOperationKind,
            GraphqlSemanticArgumentDescriptor, GraphqlSemanticCatalog,
            GraphqlSemanticClassification, GraphqlSemanticExport, GraphqlSemanticFieldMetadata,
            GraphqlSemanticOperationDescriptor, GraphqlSemanticOperationSource,
            GraphqlSemanticTypeKind, GraphqlSemanticTypeRef,
        };

        use super::*;
        use crate::{
            AiGeneratedGraphqlTargetPolicyBinding, AiGeneratedGraphqlTargetPolicySet,
            AiGraphqlMutationCapabilityCatalog, AiGraphqlQueryCapabilityLimits, AiToolCatalog,
        };

        const SDL: &str = r#"
            schema { query: Query, mutation: Mutation }
            type Query { Probe: String! }
            type Mutation {
              DeleteFolder(id: ID!): Folder
              DeleteDocument(id: ID!): Document
              UpdateDocument(id: ID!): Document
            }
            type Folder { id: String!, name: String! }
            type Document { id: String!, title: String! }
        "#;

        pub(crate) fn entity(name: &str, field: &str) -> GraphqlEntitySemanticMetadata {
            let scalar = |field_name: &str| GraphqlSemanticFieldMetadata {
                field_name: field_name.to_owned(),
                description: format!("Reviewed public {field_name}."),
                type_ref: GraphqlSemanticTypeRef::named(
                    "String",
                    GraphqlSemanticTypeKind::Scalar,
                    false,
                ),
                selectable: true,
                filter_operators: Vec::new(),
                filter_input: None,
                sortable: false,
                groupable: false,
                aggregate_operators: Vec::new(),
                aggregate_value_kind: None,
                relationship: None,
                classification: GraphqlSemanticClassification::Internal,
                export: GraphqlSemanticExport::Exportable,
                has_field_policy: false,
            };
            GraphqlEntitySemanticMetadata {
                generated_input_members: Vec::new(),
                entity_name: name.to_owned(),
                description: format!("A reviewed {name} record."),
                default_classification: GraphqlSemanticClassification::Internal,
                fields: vec![scalar("id"), scalar(field)].into_boxed_slice(),
                label_fields: Vec::new(),
                record_version: None,
            }
        }

        /// A mutation root carrying the generated category and entity the
        /// ORM records for its own delete and update roots.
        pub(crate) fn generated(
            field: &str,
            entity: &str,
            category: GeneratedGraphqlOperationCategory,
            policy: AiMutationExecutionPolicy,
        ) -> GraphqlSemanticOperationDescriptor {
            let mut operation = GraphqlSemanticOperationDescriptor::custom(
                GraphqlOperationKind::Mutation,
                field,
                format!("Apply {field} to one reviewed record."),
                vec![GraphqlSemanticArgumentDescriptor {
                    graphql_name: "id".to_owned(),
                    description: "Exact public record identity.".to_owned(),
                    type_ref: GraphqlSemanticTypeRef::named(
                        "ID",
                        GraphqlSemanticTypeKind::Scalar,
                        false,
                    ),
                }],
                GraphqlSemanticTypeRef::named(entity, GraphqlSemanticTypeKind::Object, true),
                true,
            )
            .expect("mutation semantics");
            operation.source = GraphqlSemanticOperationSource::Generated;
            operation.generated_category = Some(category);
            operation.generated_entity_name = Some(entity.to_owned());
            operation
                .with_ai_mutation_execution(policy)
                .expect("classified mutation")
        }

        struct Registered {
            catalog: AiToolCatalog,
            targets: AiGeneratedGraphqlTargetPolicySet,
            delete_folder: AiToolId,
            update_document: AiToolId,
        }

        fn registered(document_delete: AiMutationExecutionPolicy, admit: bool) -> Registered {
            let semantics = GraphqlSemanticCatalog::compose_with_custom(
                [entity("Folder", "name"), entity("Document", "title")],
                &GraphqlOperationCatalog::compose(std::iter::empty()),
                [
                    generated("DeleteFolder", "Folder", Delete, Automatic),
                    generated("DeleteDocument", "Document", Delete, document_delete),
                    generated("UpdateDocument", "Document", Update, Automatic),
                ],
            )
            .expect("semantic catalogue");
            let mutations = AiGraphqlMutationCapabilityCatalog::compile(
                "records",
                target(),
                SDL,
                &semantics,
                AiGraphqlQueryCapabilityLimits::default(),
            )
            .expect("mutation capabilities");
            let id = |field: &str| {
                mutations
                    .capabilities()
                    .find(|capability| capability.field_name() == field)
                    .expect("capability")
                    .id()
                    .clone()
            };
            let mut binding = AiGeneratedGraphqlTargetPolicyBinding::new(
                target(),
                mutations.finished_schema_fingerprint(),
                semantics.fingerprint.clone(),
            )
            .expect("target binding");
            if admit {
                binding = binding
                    .allow_automatic_mutations()
                    .allow_approval_required_mutations();
            }
            let mut targets = AiGeneratedGraphqlTargetPolicySet::new();
            targets.bind(binding).expect("bound target");
            let mut catalog = AiToolCatalog::new();
            catalog
                .register_mutation_capability_catalog(&mutations)
                .expect("registered mutations");
            Registered {
                delete_folder: id("DeleteFolder"),
                update_document: id("UpdateDocument"),
                catalog,
                targets,
            }
        }

        fn decide(
            registered: &Registered,
            policy: Option<&AiDependentEffectPolicy>,
            capability_id: &AiToolId,
        ) -> Result<(), AiError> {
            permit_generated_mutation(
                policy,
                &registered.catalog,
                &registered.targets,
                capability_id,
                Automatic,
            )
        }

        #[test]
        fn an_installed_policy_gates_registered_generated_deletes_only() {
            let registered = registered(Automatic, true);
            let undeclared = AiDependentEffectPolicy::new();
            // No installed policy: every mutation passes, as before.
            assert!(decide(&registered, None, &registered.delete_folder).is_ok());
            // Installed: an undeclared delete root is refused, and a mutation
            // that is not a delete is untouched.
            assert!(matches!(
                decide(&registered, Some(&undeclared), &registered.delete_folder),
                Err(AiError::Forbidden)
            ));
            assert!(decide(&registered, Some(&undeclared), &registered.update_document).is_ok());
            assert!(matches!(
                decide(
                    &registered,
                    Some(&undeclared),
                    &AiToolId::parse("records.unregistered").expect("id"),
                ),
                Err(AiError::Forbidden)
            ));
        }

        #[test]
        fn a_declared_delete_follows_the_dependent_entitys_registered_policy() {
            let cascade = policy([AiDependentEffect::delete("Document").expect("effect")]);
            let set_null =
                policy([AiDependentEffect::set_null("Document", "folderId").expect("effect")]);

            let allowed = registered(Automatic, true);
            assert!(decide(&allowed, Some(&cascade), &allowed.delete_folder).is_ok());
            assert!(decide(&allowed, Some(&set_null), &allowed.delete_folder).is_ok());

            // The dependent's own delete needs approval; the automatic root
            // cannot cascade into it.
            let supervised = registered(ApprovalRequired, true);
            assert!(matches!(
                decide(&supervised, Some(&cascade), &supervised.delete_folder),
                Err(AiError::Forbidden)
            ));
            // Its update is still automatic, so a set-null is allowed.
            assert!(decide(&supervised, Some(&set_null), &supervised.delete_folder).is_ok());

            // The dependent's own delete is prohibited, so it is not a
            // registered capability at all.
            let prohibited = registered(Prohibited, true);
            assert!(matches!(
                decide(&prohibited, Some(&cascade), &prohibited.delete_folder),
                Err(AiError::Forbidden)
            ));

            // Registered but not admitted by the generated target policy.
            let disabled = registered(Automatic, false);
            assert!(matches!(
                decide(&disabled, Some(&cascade), &disabled.delete_folder),
                Err(AiError::Forbidden)
            ));
        }
    }
}
