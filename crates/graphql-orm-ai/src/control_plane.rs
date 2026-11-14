//! AI control-plane root identity and static tool document admission.
//!
//! The control-plane root set is derived from this crate's own composable
//! GraphQL roots, so a new session, approval, proposal, skill, rule, budget,
//! configuration, attachment, usage, or subscription root is denied to tools
//! the moment it is added to a root type. Matching ignores ASCII case and
//! underscores so every configured GraphQL field case is covered.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use async_graphql::{
    EmptyMutation, EmptySubscription, Object, ObjectType, Schema, SubscriptionType,
};
use async_graphql_parser::types::{
    ExecutableDocument, OperationType, Selection, SelectionSet, TypeKind, TypeSystemDefinition,
};

use crate::AiToolOperationKind;

/// Expands the complete list of composable AI root types exactly once, so the
/// derived field set and the root-type inventory cannot drift apart.
macro_rules! ai_control_plane_roots {
    (
        query: [$($query:ident),* $(,)?],
        mutation: [$($mutation:ident),* $(,)?],
        subscription: [$($subscription:ident),* $(,)?] $(,)?
    ) => {
        /// Every composable AI GraphQL root type, by Rust type name.
        #[cfg_attr(not(test), allow(dead_code))]
        pub(crate) const AI_CONTROL_PLANE_ROOT_TYPES: &[&str] = &[
            $(stringify!($query),)*
            $(stringify!($mutation),)*
            $(stringify!($subscription),)*
        ];

        fn derive_control_plane_root_fields() -> Option<BTreeSet<String>> {
            let mut fields = BTreeSet::new();
            $(fields.extend(query_root_fields(crate::$query)?);)*
            $(fields.extend(mutation_root_fields(crate::$mutation)?);)*
            $(fields.extend(subscription_root_fields(crate::$subscription)?);)*
            Some(fields)
        }
    };
}

ai_control_plane_roots! {
    query: [
        AiQueryRoot,
        AiApprovalQueryRoot,
        AiAttachmentQueryRoot,
        AiConfigurationQueryRoot,
        AiProposalQueryRoot,
        AiRuleQueryRoot,
        AiSkillQueryRoot,
        AiUsageQueryRoot,
    ],
    mutation: [
        AiMutationRoot,
        AiApprovalMutationRoot,
        AiAttachmentMutationRoot,
        AiConfigurationMutationRoot,
        AiProposalMutationRoot,
        AiRuleMutationRoot,
        AiSkillMutationRoot,
    ],
    subscription: [AiSubscriptionRoot],
}

/// Exposed control-plane root fields, or `None` when derivation failed.
static CONTROL_PLANE_ROOT_FIELDS: LazyLock<Option<BTreeSet<String>>> =
    LazyLock::new(|| derive_control_plane_root_fields().filter(|fields| !fields.is_empty()));

/// Normalized control-plane root fields. A failed derivation denies every
/// root rather than admitting any.
static NORMALIZED_CONTROL_PLANE_ROOT_FIELDS: LazyLock<Option<BTreeSet<String>>> =
    LazyLock::new(|| {
        CONTROL_PLANE_ROOT_FIELDS
            .as_ref()
            .map(|fields| fields.iter().map(|field| normalize(field)).collect())
    });

/// Placeholder query root used only to build a schema around a mutation or
/// subscription root during derivation.
struct DerivationQuery;

#[Object]
impl DerivationQuery {
    async fn derivation(&self) -> bool {
        false
    }
}

fn query_root_fields<Q: ObjectType + 'static>(root: Q) -> Option<Vec<String>> {
    let sdl = Schema::build(root, EmptyMutation, EmptySubscription)
        .finish()
        .sdl();
    root_fields(&sdl, OperationType::Query)
}

fn mutation_root_fields<M: ObjectType + 'static>(root: M) -> Option<Vec<String>> {
    let sdl = Schema::build(DerivationQuery, root, EmptySubscription)
        .finish()
        .sdl();
    root_fields(&sdl, OperationType::Mutation)
}

fn subscription_root_fields<S: SubscriptionType + 'static>(root: S) -> Option<Vec<String>> {
    let sdl = Schema::build(DerivationQuery, EmptyMutation, root)
        .finish()
        .sdl();
    root_fields(&sdl, OperationType::Subscription)
}

fn root_fields(sdl: &str, kind: OperationType) -> Option<Vec<String>> {
    let document = async_graphql_parser::parse_schema(sdl).ok()?;
    let declared = document
        .definitions
        .iter()
        .find_map(|definition| match definition {
            TypeSystemDefinition::Schema(schema) => match kind {
                OperationType::Query => schema.node.query.as_ref(),
                OperationType::Mutation => schema.node.mutation.as_ref(),
                OperationType::Subscription => schema.node.subscription.as_ref(),
            }
            .map(|name| name.node.to_string()),
            _ => None,
        });
    let root_type = declared.unwrap_or_else(|| kind.to_string());
    let root_type = match root_type.as_str() {
        "query" => "Query".to_owned(),
        "mutation" => "Mutation".to_owned(),
        "subscription" => "Subscription".to_owned(),
        _ => root_type,
    };
    let fields = document
        .definitions
        .iter()
        .find_map(|definition| match definition {
            TypeSystemDefinition::Type(ty) if ty.node.name.node.as_str() == root_type => {
                match &ty.node.kind {
                    TypeKind::Object(object) => Some(
                        object
                            .fields
                            .iter()
                            .map(|field| field.node.name.node.to_string())
                            .collect::<Vec<_>>(),
                    ),
                    _ => None,
                }
            }
            _ => None,
        })?;
    (!fields.is_empty()).then_some(fields)
}

fn normalize(name: &str) -> String {
    name.bytes()
        .filter(|byte| *byte != b'_')
        .map(|byte| byte.to_ascii_lowercase() as char)
        .collect()
}

/// Returns the AI control-plane root fields exposed by this crate's
/// composable GraphQL roots, in the configured GraphQL field case.
///
/// This is the single source used to refuse AI control-plane roots as static
/// tools, generated capabilities, and remote delegated requests. It is
/// discovery metadata only and grants no authority.
pub fn ai_control_plane_root_fields() -> BTreeSet<String> {
    CONTROL_PLANE_ROOT_FIELDS.clone().unwrap_or_default()
}

/// Returns whether a GraphQL root field name is an AI control-plane root.
///
/// Matching ignores ASCII case and underscores. When the root set cannot be
/// derived, every name is treated as a control-plane root.
pub(crate) fn is_ai_control_plane_root_field(name: &str) -> bool {
    NORMALIZED_CONTROL_PLANE_ROOT_FIELDS
        .as_ref()
        .is_none_or(|fields| fields.contains(&normalize(name)))
}

/// Federation service fields that are never tool operations.
const FEDERATION_SERVICE_FIELDS: &[&str] = &["_service", "_entities"];

/// Returns whether a GraphQL document must be refused as a tool document.
///
/// The document is refused when it does not parse, spreads an undefined
/// fragment, or selects, at any depth and through any fragment, an AI
/// control-plane field name, an introspection field other than `__typename`,
/// or a federation service field (`_service`, `_entities`). Control-plane
/// names are refused below the root too, so a host that nests an AI root
/// under another field cannot expose it.
pub(crate) fn document_invokes_control_plane(document: &str) -> bool {
    let Ok(document) = async_graphql_parser::parse_query(document) else {
        return true;
    };
    let mut selection_sets = document
        .operations
        .iter()
        .map(|(_, operation)| &operation.node.selection_set.node)
        .collect::<Vec<_>>();
    selection_sets.extend(
        document
            .fragments
            .values()
            .map(|fragment| &fragment.node.selection_set.node),
    );
    selection_sets
        .into_iter()
        .any(|selection_set| selection_set_is_refused(&document, selection_set))
}

fn selection_set_is_refused(document: &ExecutableDocument, selection_set: &SelectionSet) -> bool {
    selection_set
        .items
        .iter()
        .any(|selection| match &selection.node {
            Selection::Field(field) => {
                let name = field.node.name.node.as_str();
                (name.starts_with("__") && name != "__typename")
                    || FEDERATION_SERVICE_FIELDS.contains(&name)
                    || is_ai_control_plane_root_field(name)
                    || selection_set_is_refused(document, &field.node.selection_set.node)
            }
            Selection::InlineFragment(fragment) => {
                selection_set_is_refused(document, &fragment.node.selection_set.node)
            }
            // Every defined fragment is checked on its own; an undefined one
            // cannot be validated and is refused.
            Selection::FragmentSpread(spread) => !document
                .fragments
                .contains_key(&spread.node.fragment_name.node),
        })
}

/// Returns whether a static tool document contains exactly one named
/// operation whose type is the declared kind and whose name is the contract's
/// operation name.
pub(crate) fn document_has_exact_operation(
    document: &str,
    kind: AiToolOperationKind,
    operation_name: &str,
) -> bool {
    let Ok(document) = async_graphql_parser::parse_query(document) else {
        return false;
    };
    let mut operations = document.operations.iter();
    let Some((Some(name), operation)) = operations.next() else {
        return false;
    };
    operations.next().is_none()
        && name.as_str() == operation_name
        && matches!(
            (operation.node.ty, kind),
            (OperationType::Query, AiToolOperationKind::Query)
                | (OperationType::Mutation, AiToolOperationKind::Mutation)
                | (
                    OperationType::Subscription,
                    AiToolOperationKind::Subscription
                )
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reviewed async-graphql output types that are not AI root operations.
    const NON_ROOT_OBJECT_TYPES: &[&str] = &[
        "AiAttachmentUploadTicket",
        "AiSessionExecutionSelection",
        // Test-only placeholder roots.
        "DerivationQuery",
        "Query",
    ];

    fn sources(directory: &std::path::Path, files: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(directory).expect("source directory") {
            let path = entry.expect("source entry").path();
            if path.is_dir() {
                sources(&path, files);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let mut source = std::fs::read_to_string(&path).expect("source file");
                // This module's own tests hold scanner fixtures, not roots.
                if path.ends_with("control_plane.rs")
                    && let Some(tests) = source.find("#[cfg(test)]\nmod tests {")
                {
                    source.truncate(tests);
                }
                files.push((path.display().to_string(), source));
            }
        }
    }

    fn tokens(line: &str) -> impl Iterator<Item = &str> {
        line.split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .filter(|token| !token.is_empty())
    }

    /// Returns the target type of every impl annotated with an async-graphql
    /// object or subscription attribute, in any attribute form.
    fn graphql_object_impls(source: &str) -> Vec<String> {
        let mut targets = Vec::new();
        let mut attribute_depth = 0_i32;
        let mut annotated = false;
        for line in source.lines() {
            let trimmed = line.trim();
            if attribute_depth > 0 || trimmed.starts_with("#[") {
                if tokens(trimmed).any(|token| {
                    matches!(
                        token,
                        "Object" | "Subscription" | "MergedObject" | "MergedSubscription"
                    )
                }) {
                    annotated = true;
                }
                attribute_depth +=
                    trimmed.matches('[').count() as i32 - trimmed.matches(']').count() as i32;
                continue;
            }
            if trimmed.is_empty() || trimmed.starts_with("//") {
                continue;
            }
            if annotated && let Some(rest) = trimmed.strip_prefix("impl") {
                let rest = rest.trim_start();
                let rest = if rest.starts_with('<') {
                    rest.split_once('>')
                        .map_or("", |(_, rest)| rest.trim_start())
                } else {
                    rest
                };
                if let Some(name) = tokens(rest).next() {
                    targets.push(name.to_owned());
                }
            }
            annotated = false;
        }
        targets
    }

    fn declared_root_types(source: &str) -> Vec<String> {
        source
            .lines()
            .filter_map(|line| {
                let mut words = tokens(line.trim());
                let mut previous = "";
                for word in words.by_ref() {
                    if matches!(previous, "struct" | "enum" | "union" | "type") {
                        return (word.starts_with("Ai") && word.ends_with("Root"))
                            .then(|| word.to_owned());
                    }
                    previous = word;
                }
                None
            })
            .collect()
    }

    #[test]
    fn derived_root_fields_cover_every_composable_root_type() {
        let mut files = Vec::new();
        sources(
            std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")),
            &mut files,
        );
        let derived = AI_CONTROL_PLANE_ROOT_TYPES
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<BTreeSet<_>>();

        // Every GraphQL object or subscription impl is either a derived
        // control-plane root or a reviewed non-root output type.
        let objects = files
            .iter()
            .flat_map(|(_, source)| graphql_object_impls(source))
            .collect::<BTreeSet<_>>();
        let unclassified = objects
            .iter()
            .filter(|name| {
                !derived.contains(*name) && !NON_ROOT_OBJECT_TYPES.contains(&name.as_str())
            })
            .collect::<Vec<_>>();
        assert!(
            unclassified.is_empty(),
            "classify these GraphQL object types as control-plane roots or non-root outputs: {unclassified:?}"
        );
        for root in &derived {
            assert!(objects.contains(root), "{root} has no GraphQL object impl");
        }

        // Every declared `Ai*Root` type, in any declaration form, is derived.
        let declared = files
            .iter()
            .flat_map(|(_, source)| declared_root_types(source))
            .collect::<BTreeSet<_>>();
        assert_eq!(declared, derived);

        let fields = ai_control_plane_root_fields();
        assert!(!fields.is_empty());
        for field in &fields {
            assert!(is_ai_control_plane_root_field(field), "{field}");
        }
    }

    #[test]
    fn drift_scanners_read_every_declaration_form() {
        let source = r#"
            #[cfg_attr(
                feature = "graphql-case-pascal",
                Object(rename_fields = "PascalCase")
            )]
            #[cfg_attr(not(feature = "graphql-case-pascal"), Object)]
            impl AiFirstRoot {}
            #[async_graphql::Subscription]
            impl<T: Send> AiSecondQuery<T> {}
            #[derive(Clone, SimpleObject)]
            struct NotAnImpl;
            #[derive(InputObject)]
            impl NotAnObject {}
            pub struct AiBracedRoot { inner: u8 }
            pub(crate) struct AiGenericRoot<T>(T);
            struct AiUnitRoot;
        "#;
        assert_eq!(
            graphql_object_impls(source),
            ["AiFirstRoot", "AiSecondQuery"]
        );
        assert_eq!(
            declared_root_types(source),
            ["AiBracedRoot", "AiGenericRoot", "AiUnitRoot"]
        );
    }

    #[test]
    fn tool_documents_refuse_control_plane_fields_at_any_depth() {
        for document in [
            "query S { ai { aiSessions { id } } }",
            "query S { ai { ...Nested } } fragment Nested on Ai { decideAiApproval }",
            "query S { records { ... on Record { owner { AiSessions { id } } } } }",
            "query S { _service { sdl } }",
            "query S { _entities(representations: []) { __typename } }",
            "query S { records { nested { _entities(representations: []) { __typename } } } }",
            "query S { records { __type(name: \"Record\") { name } } }",
            "query S { ...Missing }",
        ] {
            assert!(document_invokes_control_plane(document), "{document}");
        }
        assert!(!document_invokes_control_plane(
            "query S { records { id __typename ...Fields } } fragment Fields on Record { subject }"
        ));
    }
}
