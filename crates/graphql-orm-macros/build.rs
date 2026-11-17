//! Select the generated GraphQL naming cases for this build (#508).
//!
//! `GRAPHQL_ORM_RESOLVER_CASE`, `GRAPHQL_ORM_ARGUMENT_CASE` and
//! `GRAPHQL_ORM_FIELD_CASE` each name one async-graphql rename rule. Unset or
//! empty means `camelCase`. graphql-orm and graphql-orm-macros read the same
//! variables, so one build has exactly one naming contract; an unknown value
//! fails the build. Hosts usually set them in `.cargo/config.toml` `[env]`.
//!
//! This file is kept identical in graphql-orm and graphql-orm-macros
//! (`tests/graphql_case_features.rs` checks it).

/// Each accepted rule and the `cfg` value it selects.
const RULES: [(&str, &str); 6] = [
    ("camelCase", "camel"),
    ("PascalCase", "pascal"),
    ("snake_case", "snake"),
    ("SCREAMING_SNAKE_CASE", "screaming_snake"),
    ("lowercase", "lower"),
    ("UPPERCASE", "upper"),
];

/// Each variable and the `cfg` name it sets.
const VARIABLES: [(&str, &str); 3] = [
    ("GRAPHQL_ORM_RESOLVER_CASE", "graphql_orm_resolver_case"),
    ("GRAPHQL_ORM_ARGUMENT_CASE", "graphql_orm_argument_case"),
    ("GRAPHQL_ORM_FIELD_CASE", "graphql_orm_field_case"),
];

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let values = RULES
        .iter()
        .map(|(_, value)| format!("\"{value}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let rules = RULES
        .iter()
        .map(|(rule, _)| *rule)
        .collect::<Vec<_>>()
        .join(", ");
    for (variable, name) in VARIABLES {
        println!("cargo::rerun-if-env-changed={variable}");
        println!("cargo::rustc-check-cfg=cfg({name}, values({values}))");
        let rule = match std::env::var(variable) {
            Ok(rule) => rule,
            Err(std::env::VarError::NotPresent) => String::new(),
            Err(std::env::VarError::NotUnicode(_)) => {
                panic!("{variable} must be one of: {rules}")
            }
        };
        let rule = if rule.is_empty() { "camelCase" } else { &rule };
        let Some((_, value)) = RULES.iter().find(|(known, _)| *known == rule) else {
            panic!("{variable}={rule:?} is not a GraphQL naming rule; use one of: {rules}");
        };
        println!("cargo::rustc-cfg={name}=\"{value}\"");
    }
}
