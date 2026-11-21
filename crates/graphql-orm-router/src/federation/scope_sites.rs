//! Per-subgraph `@requiresScopes` and `@authenticated` declarations.
//!
//! Composition merges every subgraph's scope sets for one type or field into a
//! single list of alternatives, so a field that one subgraph protects with
//! `[["a"]]` and another with `[["b"]]` composes to `[["a"], ["b"]]`, which
//! either scope alone satisfies. Federation requires both. The router therefore
//! keeps each subgraph's declaration as its own requirement and combines them
//! with AND semantics.
//!
//! Directive names are resolved the way `graphql-composition` resolves them:
//! a `<namespace>__` prefix selects a linked schema (its `as:` argument, or the
//! name taken from its URL); an unknown prefix is stripped and the rest is
//! matched; imports, renamed or not, bring a directive in scope under their
//! final name; and without a Federation v2 link (including when the
//! composite-schemas specification is linked) the unprefixed Federation names
//! apply. Independently of that resolution, any directive application that
//! composition could read as one of the two directives (its name, or its name
//! after an optional `<x>__` prefix, is `requiresScopes`, `authenticated`, or
//! an import alias of either) must have been resolved to a supported site, or
//! the graph is refused. So a declaration is never dropped silently, even
//! when its scope sets also appear in another declaration and the composed
//! union would not change. A directive on an argument, an input field, an
//! enum value, a union, an input object, or the schema is refused too, since
//! the router cannot enforce it there.

use std::collections::{BTreeMap, BTreeSet, HashMap};

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

/// Every subgraph's declarations by site. Each `sites` entry is one
/// `@requiresScopes` declaration in one subgraph; a site's entries must all
/// hold. `authenticated` holds every site any subgraph marks `@authenticated`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SubgraphScopeRequirements {
    pub(crate) sites: BTreeMap<ScopeSite, Vec<ScopeAlternatives>>,
    pub(crate) authenticated: BTreeSet<ScopeSite>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AuthorizationDirective {
    RequiresScopes,
    Authenticated,
}

const REQUIRES_SCOPES: &str = "requiresScopes";
const AUTHENTICATED: &str = "authenticated";

impl SubgraphScopeRequirements {
    /// Records the declarations of one subgraph's source SDL.
    pub(crate) fn record(
        &mut self,
        subgraph: &str,
        document: &TypeSystemDocument,
    ) -> Result<(), FederationError> {
        let names = DirectiveNames::new(subgraph, document)?;
        for definition in document.definitions() {
            match definition {
                Definition::Schema(schema) | Definition::SchemaExtension(schema) => {
                    names.refuse_unsupported(subgraph, schema.directives(), "the schema")?;
                }
                Definition::Directive(directive) => {
                    for argument in directive.arguments() {
                        names.refuse_unsupported(
                            subgraph,
                            argument.directives(),
                            &format!("@{}({}:)", directive.name(), argument.name()),
                        )?;
                    }
                }
                Definition::Type(definition) | Definition::TypeExtension(definition) => {
                    self.record_type(subgraph, &names, definition)?;
                }
            }
        }
        Ok(())
    }

    fn record_type(
        &mut self,
        subgraph: &str,
        names: &DirectiveNames,
        definition: TypeDefinition<'_>,
    ) -> Result<(), FederationError> {
        let type_name = definition.name();
        match definition {
            TypeDefinition::Object(_)
            | TypeDefinition::Interface(_)
            | TypeDefinition::Scalar(_)
            | TypeDefinition::Enum(_) => {
                for directive in definition.directives() {
                    self.record_directive(
                        subgraph,
                        names,
                        (type_name.to_owned(), None),
                        directive,
                    )?;
                }
            }
            TypeDefinition::Union(_) | TypeDefinition::InputObject(_) => {
                names.refuse_unsupported(subgraph, definition.directives(), type_name)?;
            }
        }
        let fields = match definition {
            TypeDefinition::Object(object) => Some(object.fields()),
            TypeDefinition::Interface(interface) => Some(interface.fields()),
            _ => None,
        };
        for field in fields.into_iter().flatten() {
            for directive in field.directives() {
                self.record_directive(
                    subgraph,
                    names,
                    (type_name.to_owned(), Some(field.name().to_owned())),
                    directive,
                )?;
            }
            for argument in field.arguments() {
                names.refuse_unsupported(
                    subgraph,
                    argument.directives(),
                    &format!("{type_name}.{}({}:)", field.name(), argument.name()),
                )?;
            }
        }
        match definition {
            TypeDefinition::InputObject(input) => {
                for field in input.fields() {
                    names.refuse_unsupported(
                        subgraph,
                        field.directives(),
                        &format!("{type_name}.{}", field.name()),
                    )?;
                }
            }
            TypeDefinition::Enum(enumeration) => {
                for value in enumeration.values() {
                    names.refuse_unsupported(
                        subgraph,
                        value.directives(),
                        &format!("{type_name}.{}", value.value()),
                    )?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn record_directive(
        &mut self,
        subgraph: &str,
        names: &DirectiveNames,
        site: ScopeSite,
        directive: Directive<'_>,
    ) -> Result<(), FederationError> {
        let label = match &site {
            (type_name, Some(field)) => format!("{type_name}.{field}"),
            (type_name, None) => type_name.clone(),
        };
        match names.resolve(directive.name()) {
            Some(AuthorizationDirective::RequiresScopes) => {
                let alternatives = parse_scopes(directive)
                    .ok_or_else(|| malformed(subgraph, &label, MALFORMED_SCOPES))?;
                let entries = self.sites.entry(site).or_default();
                if !entries.contains(&alternatives) {
                    entries.push(alternatives);
                }
            }
            Some(AuthorizationDirective::Authenticated) => {
                self.authenticated.insert(site);
            }
            None if names.may_be_authorization(directive.name()) => {
                return Err(unresolved(subgraph, directive.name(), &label));
            }
            None => {}
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

fn unresolved(subgraph: &str, directive: &str, site: &str) -> FederationError {
    FederationError::SourceSdl {
        subgraph: subgraph.to_owned(),
        details: format!(
            "`@{directive}` on `{site}` may be read as an authorization directive but does not resolve to `@requiresScopes` or `@authenticated` from Federation v2; spell it as imported or rename it"
        ),
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

/// The Federation specification a subgraph uses, ordered as composition
/// orders it: the highest one linked wins.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum FederationSpec {
    ApolloV1,
    ApolloV2,
    CompositeSchemas,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LinkedSchema {
    FederationV2,
    CompositeSchemas,
    Other,
}

impl LinkedSchema {
    fn from_url(url: &str) -> Self {
        if url.contains("dev/federation/v2") {
            Self::FederationV2
        } else if url == "https://specs.grafbase.com/composite-schemas/v1" {
            Self::CompositeSchemas
        } else {
            Self::Other
        }
    }
}

/// One subgraph's directive scope, built from its `@link` directives the way
/// composition builds it.
struct DirectiveNames {
    spec: FederationSpec,
    /// Linked schemas by namespace. A later link wins, as in composition.
    namespaces: HashMap<String, LinkedSchema>,
    /// Imported definitions by final name, with their original name.
    imports: HashMap<String, (LinkedSchema, String)>,
    /// Final names of every import of `@requiresScopes` or `@authenticated`,
    /// from any linked schema.
    aliases: BTreeSet<String>,
}

impl DirectiveNames {
    fn new(subgraph: &str, document: &TypeSystemDocument) -> Result<Self, FederationError> {
        let mut names = Self {
            spec: FederationSpec::ApolloV1,
            namespaces: HashMap::new(),
            imports: HashMap::new(),
            aliases: BTreeSet::new(),
        };
        let invalid = || FederationError::SourceSdl {
            subgraph: subgraph.to_owned(),
            details: "an `@link` directive has an invalid `url`, `as` or `import` argument"
                .to_owned(),
        };
        for definition in document.definitions() {
            let (Definition::Schema(schema) | Definition::SchemaExtension(schema)) = definition
            else {
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
                let url = argument("url")
                    .and_then(|url| url.as_str())
                    .ok_or_else(invalid)?;
                let namespace = match argument("as") {
                    None => None,
                    Some(namespace) => Some(namespace.as_str().ok_or_else(invalid)?.to_owned()),
                };
                let linked = LinkedSchema::from_url(url);
                let spec = match linked {
                    LinkedSchema::FederationV2 => Some(FederationSpec::ApolloV2),
                    LinkedSchema::CompositeSchemas => Some(FederationSpec::CompositeSchemas),
                    LinkedSchema::Other => None,
                };
                if let Some(spec) = spec {
                    names.spec = names.spec.max(spec);
                }
                if let Some(namespace) = namespace.or_else(|| name_from_url(url)) {
                    names.namespaces.insert(namespace, linked);
                }
                let imports = match argument("import") {
                    None | Some(ConstValue::Null(_)) => None,
                    Some(imports) => Some(imports.as_items().ok_or_else(invalid)?),
                };
                for import in imports.into_iter().flatten() {
                    let (original, alias) = match import {
                        ConstValue::String(name) => (name.as_str(), None),
                        ConstValue::Object(object) => {
                            let original = object
                                .get("name")
                                .and_then(|name| name.as_str())
                                .ok_or_else(invalid)?;
                            let alias = match object.get("as") {
                                None => None,
                                Some(alias) => Some(alias.as_str().ok_or_else(invalid)?),
                            };
                            (original, alias)
                        }
                        _ => return Err(invalid()),
                    };
                    let original = original.trim_start_matches('@');
                    let final_name = alias.unwrap_or(original).trim_start_matches('@');
                    if matches!(original, REQUIRES_SCOPES | AUTHENTICATED) {
                        names.aliases.insert(final_name.to_owned());
                    }
                    names
                        .imports
                        .insert(final_name.to_owned(), (linked, original.to_owned()));
                }
            }
        }
        Ok(names)
    }

    /// Composition's resolution of one directive application name, limited
    /// to the two authorization directives.
    fn resolve(&self, name: &str) -> Option<AuthorizationDirective> {
        let (namespace, rest) = match name.split_once("__") {
            Some((namespace, rest)) => (Some(namespace), rest),
            None => (None, name),
        };
        if let Some(linked) = namespace.and_then(|namespace| self.namespaces.get(namespace)) {
            return match linked {
                LinkedSchema::FederationV2 => federation_directive(rest),
                LinkedSchema::CompositeSchemas | LinkedSchema::Other => None,
            };
        }
        if let Some((linked, original)) = self.imports.get(rest) {
            return match linked {
                LinkedSchema::FederationV2 => federation_directive(original),
                LinkedSchema::CompositeSchemas | LinkedSchema::Other => None,
            };
        }
        if matches!(
            rest,
            "link" | "deprecated" | "specifiedBy" | "oneOf" | "cost" | "listSize"
        ) {
            return None;
        }
        if self.spec != FederationSpec::ApolloV2 {
            return federation_directive(rest);
        }
        None
    }

    /// Whether composition could read this name as an authorization directive
    /// under any resolution.
    fn may_be_authorization(&self, name: &str) -> bool {
        let base = name.split_once("__").map_or(name, |(_, rest)| rest);
        [name, base].into_iter().any(|candidate| {
            matches!(candidate, REQUIRES_SCOPES | AUTHENTICATED) || self.aliases.contains(candidate)
        })
    }

    /// Refuses an authorization directive where the router cannot enforce it.
    fn refuse_unsupported<'a>(
        &self,
        subgraph: &str,
        directives: impl Iterator<Item = Directive<'a>>,
        location: &str,
    ) -> Result<(), FederationError> {
        for directive in directives {
            if self.resolve(directive.name()).is_some()
                || self.may_be_authorization(directive.name())
            {
                return Err(FederationError::SourceSdl {
                    subgraph: subgraph.to_owned(),
                    details: format!(
                        "`@{}` on `{location}` is not supported: authorization directives apply to object, interface, scalar and enum types and to object and interface fields",
                        directive.name()
                    ),
                });
            }
        }
        Ok(())
    }
}

fn federation_directive(name: &str) -> Option<AuthorizationDirective> {
    match name {
        REQUIRES_SCOPES => Some(AuthorizationDirective::RequiresScopes),
        AUTHENTICATED => Some(AuthorizationDirective::Authenticated),
        _ => None,
    }
}

/// The namespace composition derives from a link URL: the last path segment,
/// or the one before it when the last is a version (`v2.7`).
fn name_from_url(url: &str) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    let mut segments = url.path_segments()?.rev();
    let mut last = segments.next()?;
    if url.scheme() == "file" && last == "build" {
        last = segments.next()?;
    }
    if is_version(last) {
        segments
            .next()
            .filter(|name| is_graphql_name(name))
            .map(str::to_owned)
    } else if is_graphql_name(last) {
        Some(last.to_owned())
    } else {
        None
    }
}

fn is_version(segment: &str) -> bool {
    let mut characters = segment.chars();
    characters.next() == Some('v')
        && characters
            .next()
            .is_some_and(|digit| digit.is_ascii_digit())
        && characters.all(|character| character.is_ascii_digit() || character == '.')
}

fn is_graphql_name(segment: &str) -> bool {
    let mut characters = segment.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
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

    fn one(
        site_type: &str,
        field: &str,
        scopes: &[&[&str]],
    ) -> (ScopeSite, Vec<ScopeAlternatives>) {
        (
            site(site_type, Some(field)),
            vec![
                scopes
                    .iter()
                    .map(|set| set.iter().map(|scope| (*scope).to_owned()).collect())
                    .collect(),
            ],
        )
    }

    #[test]
    fn composition_spellings_resolve_to_the_same_directives() {
        // An unknown prefix is stripped and the rest matched against imports.
        let prefixed = record(
            r#"
            extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@requiresScopes", "@authenticated"])
            type Query {
                scoped: String @zzz__requiresScopes(scopes: [["a"]])
                signed: String @zzz__authenticated
            }
            "#,
        )
        .unwrap();
        assert_eq!(
            prefixed.sites,
            BTreeMap::from([one("Query", "scoped", &[&["a"]])])
        );
        assert_eq!(
            prefixed.authenticated,
            BTreeSet::from([site("Query", Some("signed"))])
        );
        // A renamed import under an unknown prefix.
        let renamed = record(
            r#"
            extend schema @link(
                url: "https://specs.apollo.dev/federation/v2.7"
                import: [{ name: "@requiresScopes", as: "@scopes" }, { name: "@authenticated", as: "@signedIn" }]
            )
            type Query {
                scoped: String @zzz__scopes(scopes: [["a"]])
                signed: String @zzz__signedIn
            }
            "#,
        )
        .unwrap();
        assert_eq!(
            renamed.sites,
            BTreeMap::from([one("Query", "scoped", &[&["a"]])])
        );
        assert_eq!(renamed.authenticated.len(), 1);
        // The composite-schemas link makes the unprefixed names available.
        let composite = record(
            r#"
            extend schema
                @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@key"])
                @link(url: "https://specs.grafbase.com/composite-schemas/v1")
            type Query {
                scoped: String @requiresScopes(scopes: [["a"]])
                signed: String @authenticated
            }
            "#,
        )
        .unwrap();
        assert_eq!(
            composite.sites,
            BTreeMap::from([one("Query", "scoped", &[&["a"]])])
        );
        assert_eq!(composite.authenticated.len(), 1);
        // Two spellings on one field are two declarations.
        let doubled = record(
            r#"
            extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@requiresScopes"])
            type Query {
                scoped: String @requiresScopes(scopes: [["a"], ["b"]]) @zzz__requiresScopes(scopes: [["a"]])
            }
            "#,
        )
        .unwrap();
        assert_eq!(doubled.sites[&site("Query", Some("scoped"))].len(), 2);
    }

    #[test]
    fn spellings_that_do_not_resolve_are_refused() {
        for (link, directive) in [
            // Federation v2 linked without importing the directive.
            (
                r#"@link(url: "https://specs.apollo.dev/federation/v2.7")"#,
                r#"@requiresScopes(scopes: [["a"]])"#,
            ),
            (
                r#"@link(url: "https://specs.apollo.dev/federation/v2.7")"#,
                "@authenticated",
            ),
            (
                r#"@link(url: "https://specs.apollo.dev/federation/v2.7")"#,
                r#"@zzz__requiresScopes(scopes: [["a"]])"#,
            ),
            // A namespace that is not Federation.
            (
                r#"@link(url: "https://specs.apollo.dev/federation/v2.7") @link(url: "https://example.com/auth/v1")"#,
                "@auth__authenticated",
            ),
            // An alias imported from a schema that is not Federation.
            (
                r#"@link(url: "https://specs.apollo.dev/federation/v2.7") @link(url: "https://example.com/auth/v1", import: [{ name: "@requiresScopes", as: "@guard" }])"#,
                r#"@guard(scopes: [["a"]])"#,
            ),
        ] {
            let sdl = format!("extend schema {link}\ntype Query {{ field: String {directive} }}");
            assert!(
                matches!(record(&sdl), Err(FederationError::SourceSdl { .. })),
                "{link} {directive}"
            );
        }
    }

    #[test]
    fn unsupported_locations_are_refused() {
        let link = r#"extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@requiresScopes", "@authenticated"])"#;
        for body in [
            r#"type Query { item(x: String @requiresScopes(scopes: [["a"]])): String }"#,
            "type Query { item(x: String @authenticated): String }",
            "type Query { item(x: Filter): String } input Filter { name: String @authenticated }",
            "type Query { item: Kind } enum Kind { A @authenticated B }",
            "type Query { item: Item } type Item { id: ID } union Any @authenticated = Item",
            "type Query { item(x: Filter): String } input Filter @authenticated { name: String }",
            "type Query { item: String } directive @custom(value: String @authenticated) on FIELD_DEFINITION",
        ] {
            let sdl = format!("{link}\n{body}");
            assert!(
                matches!(record(&sdl), Err(FederationError::SourceSdl { .. })),
                "{body}"
            );
        }
        let schema = r#"extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@authenticated"]) @authenticated
            type Query { item: String }"#;
        assert!(matches!(
            record(schema),
            Err(FederationError::SourceSdl { .. })
        ));
    }

    #[test]
    fn composed_directives_on_unsupported_locations_are_refused() {
        for body in [
            r#"type Query { item(x: String @requiresScopes(scopes: [["a"]])): String }"#,
            "type Query { item(x: Filter): String } input Filter { name: String @authenticated }",
            "type Query { item: Kind } enum Kind { A @authenticated }",
            "type Query { item: Item } type Item { id: ID } union Any @requiresScopes(scopes: [[\"a\"]]) = Item",
        ] {
            let document =
                hive_router::query_planner::utils::parsing::safe_parse_schema(body).unwrap();
            assert!(
                matches!(
                    super::super::refuse_unsupported_authorization_locations(&document),
                    Err(FederationError::SupergraphSdl { .. })
                ),
                "{body}"
            );
        }
        let supported = hive_router::query_planner::utils::parsing::safe_parse_schema(
            r#"type Query { item: Item @authenticated } type Item @requiresScopes(scopes: [["a"]]) { id: ID }"#,
        )
        .unwrap();
        assert!(super::super::refuse_unsupported_authorization_locations(&supported).is_ok());
    }
}
