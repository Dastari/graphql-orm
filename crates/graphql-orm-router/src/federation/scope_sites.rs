//! Per-subgraph `@requiresScopes` declarations.
//!
//! Composition merges every subgraph's scope sets for one type or field into a
//! single list of alternatives, so a field that one subgraph protects with
//! `[["a"]]` and another with `[["b"]]` composes to `[["a"], ["b"]]`, which
//! either scope alone satisfies. Federation requires both. The router therefore
//! keeps each subgraph's declaration as its own requirement and combines them
//! with AND semantics; the authorization adapter checks that these
//! declarations account for exactly the composed sets, and refuses the graph
//! otherwise.

use std::collections::{BTreeMap, BTreeSet};

use cynic_parser::{
    ConstValue,
    type_system::{Definition, Directive, TypeDefinition, TypeSystemDocument},
};

use super::FederationError;

/// A type, or one field of a type, that can carry `@requiresScopes`.
pub(crate) type ScopeSite = (String, Option<String>);

/// One `@requiresScopes` argument: alternatives, each a set of scopes that
/// must all be granted. Both levels are sorted and deduplicated.
pub(crate) type ScopeAlternatives = Vec<Vec<String>>;

/// Every subgraph's `@requiresScopes` declarations by site. Each entry is one
/// declaration in one subgraph; a site's entries must all hold.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SubgraphScopeRequirements {
    pub(crate) sites: BTreeMap<ScopeSite, Vec<ScopeAlternatives>>,
}

impl SubgraphScopeRequirements {
    /// Records the declarations of one subgraph's source SDL.
    pub(crate) fn record(
        &mut self,
        subgraph: &str,
        document: &TypeSystemDocument,
    ) -> Result<(), FederationError> {
        let names = requires_scopes_names(document);
        for definition in document.definitions() {
            let (Definition::Type(definition) | Definition::TypeExtension(definition)) = definition
            else {
                continue;
            };
            let type_name = definition.name();
            let fields = match definition {
                TypeDefinition::Object(object) => Some(object.fields()),
                TypeDefinition::Interface(interface) => Some(interface.fields()),
                _ => None,
            };
            let type_level = definition
                .directives()
                .filter(|directive| names.contains(directive.name()))
                .collect::<Vec<_>>();
            if !type_level.is_empty()
                && matches!(
                    definition,
                    TypeDefinition::Union(_) | TypeDefinition::InputObject(_)
                )
            {
                return Err(malformed(
                    subgraph,
                    type_name,
                    "is not allowed on this type",
                ));
            }
            for directive in type_level {
                self.insert(subgraph, (type_name.to_owned(), None), directive)?;
            }
            for field in fields.into_iter().flatten() {
                for directive in field
                    .directives()
                    .filter(|directive| names.contains(directive.name()))
                {
                    self.insert(
                        subgraph,
                        (type_name.to_owned(), Some(field.name().to_owned())),
                        directive,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn insert(
        &mut self,
        subgraph: &str,
        site: ScopeSite,
        directive: Directive<'_>,
    ) -> Result<(), FederationError> {
        let label = match &site {
            (type_name, Some(field)) => format!("{type_name}.{field}"),
            (type_name, None) => type_name.clone(),
        };
        let alternatives =
            parse_scopes(directive).ok_or_else(|| malformed(subgraph, &label, MALFORMED_SCOPES))?;
        let entries = self.sites.entry(site).or_default();
        if !entries.contains(&alternatives) {
            entries.push(alternatives);
        }
        Ok(())
    }
}

const MALFORMED_SCOPES: &str =
    "must have a non-empty `scopes` list of non-empty lists of non-empty scope strings";

fn malformed(subgraph: &str, site: &str, problem: &str) -> FederationError {
    FederationError::SourceSdl {
        subgraph: subgraph.to_owned(),
        details: format!("@requiresScopes on `{site}` {problem}"),
    }
}

/// Reads `scopes: [[String!]!]!` strictly. Composition drops values that are
/// not strings, which would turn `[[1]]` into an always-satisfied `[[]]`, so
/// any other shape is refused here.
fn parse_scopes(directive: Directive<'_>) -> Option<ScopeAlternatives> {
    let mut arguments = directive
        .arguments()
        .filter(|argument| argument.name() == "scopes");
    let scopes = arguments.next()?.value();
    if arguments.next().is_some() {
        return None;
    }
    let ConstValue::List(alternatives) = scopes else {
        return None;
    };
    let mut parsed = BTreeSet::new();
    for alternative in alternatives.items() {
        let ConstValue::List(scopes) = alternative else {
            return None;
        };
        let mut set = BTreeSet::new();
        for scope in scopes.items() {
            let ConstValue::String(scope) = scope else {
                return None;
            };
            if scope.as_str().is_empty() {
                return None;
            }
            set.insert(scope.as_str().to_owned());
        }
        if set.is_empty() {
            return None;
        }
        parsed.insert(set.into_iter().collect::<Vec<_>>());
    }
    if parsed.is_empty() {
        return None;
    }
    Some(parsed.into_iter().collect())
}

/// Names under which this subgraph spells Federation's `@requiresScopes`,
/// following composition: an `@link` to Federation v2 brings it in scope
/// under `<namespace>__requiresScopes` (`federation` unless the link sets
/// `as`) and under each import name, renamed or not. Without such a link the
/// unprefixed and `federation__` names apply. A spelling recognized by only
/// one side is caught when the declarations are checked against the composed
/// graph.
fn requires_scopes_names(document: &TypeSystemDocument) -> BTreeSet<String> {
    const NAME: &str = "requiresScopes";
    let mut names = BTreeSet::new();
    let mut linked = false;
    for definition in document.definitions() {
        let (Definition::Schema(schema) | Definition::SchemaExtension(schema)) = definition else {
            continue;
        };
        for link in schema
            .directives()
            .filter(|directive| directive.name() == "link")
        {
            let argument = |name: &str| {
                link.arguments()
                    .find(|argument| argument.name() == name)
                    .map(|argument| argument.value())
            };
            let Some(url) = argument("url").and_then(|url| url.as_str()) else {
                continue;
            };
            if !url.contains("dev/federation/v2") {
                continue;
            }
            linked = true;
            let namespace = argument("as")
                .and_then(|namespace| namespace.as_str())
                .unwrap_or("federation");
            names.insert(format!("{namespace}__{NAME}"));
            for import in argument("import")
                .and_then(|imports| imports.as_items())
                .into_iter()
                .flatten()
            {
                let (original, alias) = match import {
                    ConstValue::String(name) => (name.as_str(), None),
                    ConstValue::Object(object) => {
                        let Some(original) = object.get("name").and_then(|name| name.as_str())
                        else {
                            continue;
                        };
                        (original, object.get("as").and_then(|alias| alias.as_str()))
                    }
                    _ => continue,
                };
                if original.trim_start_matches('@') == NAME {
                    names.insert(alias.unwrap_or(original).trim_start_matches('@').to_owned());
                }
            }
        }
    }
    if !linked {
        names.insert(NAME.to_owned());
        names.insert(format!("federation__{NAME}"));
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(sdl: &str) -> Result<SubgraphScopeRequirements, FederationError> {
        let document = cynic_parser::parse_type_system_document(sdl).unwrap();
        let mut requirements = SubgraphScopeRequirements::default();
        requirements.record("subgraph", &document)?;
        Ok(requirements)
    }

    fn site(type_name: &str, field: Option<&str>) -> ScopeSite {
        (type_name.to_owned(), field.map(str::to_owned))
    }

    #[test]
    fn link_imports_renames_and_prefixes_are_recognized() {
        let recorded = record(
            r#"
            extend schema @link(
                url: "https://specs.apollo.dev/federation/v2.7"
                as: "fed"
                import: [{ name: "@requiresScopes", as: "@scopes" }]
            )
            type Query {
                renamed: String @scopes(scopes: [["b", "a"], ["a", "b"]])
                prefixed: String @fed__requiresScopes(scopes: [["c"]])
                unrelated: String @requiresScopes(scopes: [["ignored"]])
            }
            type Secret @scopes(scopes: [["s"]]) { id: ID! }
            "#,
        )
        .unwrap();
        assert_eq!(
            recorded.sites,
            BTreeMap::from([
                (
                    site("Query", Some("renamed")),
                    vec![vec![vec!["a".to_owned(), "b".to_owned()]]]
                ),
                (
                    site("Query", Some("prefixed")),
                    vec![vec![vec!["c".to_owned()]]]
                ),
                (site("Secret", None), vec![vec![vec!["s".to_owned()]]]),
            ])
        );
        let unlinked = record(
            r#"
            type Query {
                plain: String @requiresScopes(scopes: [["p"]])
                prefixed: String @federation__requiresScopes(scopes: [["q"]])
            }
            "#,
        )
        .unwrap();
        assert_eq!(unlinked.sites.len(), 2);
    }

    #[test]
    fn malformed_scope_arguments_are_refused() {
        for scopes in [
            "[[]]",
            "[[\"a\"], []]",
            "[]",
            "[[1]]",
            "[[\"a\", 1]]",
            "[[\"\"]]",
            "\"a\"",
            "[\"a\"]",
            "null",
        ] {
            let sdl = format!(
                r#"
                extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@requiresScopes"])
                type Query {{ field: String @requiresScopes(scopes: {scopes}) }}
                "#
            );
            assert!(
                matches!(record(&sdl), Err(FederationError::SourceSdl { .. })),
                "{scopes}"
            );
        }
        let missing = r#"
            extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@requiresScopes"])
            type Query { field: String @requiresScopes }
        "#;
        assert!(matches!(
            record(missing),
            Err(FederationError::SourceSdl { .. })
        ));
    }
}
