use convert_case::{Case, Casing};

use crate::entity::FieldMetadata;

#[derive(Clone, Copy)]
pub(crate) enum GraphqlNameCase {
    Camel,
    Pascal,
    Snake,
    ScreamingSnake,
    Lower,
    Upper,
    Kebab,
    ScreamingKebab,
}

impl GraphqlNameCase {
    fn from_rule(rule: &str) -> Option<Self> {
        match rule {
            "camelCase" => Some(Self::Camel),
            "PascalCase" => Some(Self::Pascal),
            "snake_case" => Some(Self::Snake),
            "SCREAMING_SNAKE_CASE" => Some(Self::ScreamingSnake),
            "lowercase" => Some(Self::Lower),
            "UPPERCASE" => Some(Self::Upper),
            "kebab-case" => Some(Self::Kebab),
            "SCREAMING-KEBAB-CASE" => Some(Self::ScreamingKebab),
            _ => None,
        }
    }
}

pub(crate) fn selected_resolver_case() -> GraphqlNameCase {
    if cfg!(feature = "resolver-case-pascal") {
        GraphqlNameCase::Pascal
    } else if cfg!(feature = "resolver-case-snake") {
        GraphqlNameCase::Snake
    } else if cfg!(feature = "resolver-case-screaming-snake") {
        GraphqlNameCase::ScreamingSnake
    } else if cfg!(feature = "resolver-case-lower") {
        GraphqlNameCase::Lower
    } else if cfg!(feature = "resolver-case-upper") {
        GraphqlNameCase::Upper
    } else {
        GraphqlNameCase::Camel
    }
}

pub(crate) fn selected_field_case() -> GraphqlNameCase {
    if cfg!(feature = "field-case-pascal") {
        GraphqlNameCase::Pascal
    } else if cfg!(feature = "field-case-snake") {
        GraphqlNameCase::Snake
    } else if cfg!(feature = "field-case-screaming-snake") {
        GraphqlNameCase::ScreamingSnake
    } else if cfg!(feature = "field-case-lower") {
        GraphqlNameCase::Lower
    } else if cfg!(feature = "field-case-upper") {
        GraphqlNameCase::Upper
    } else {
        GraphqlNameCase::Camel
    }
}

pub(crate) fn selected_argument_case() -> GraphqlNameCase {
    if cfg!(feature = "argument-case-pascal") {
        GraphqlNameCase::Pascal
    } else if cfg!(feature = "argument-case-snake") {
        GraphqlNameCase::Snake
    } else if cfg!(feature = "argument-case-screaming-snake") {
        GraphqlNameCase::ScreamingSnake
    } else if cfg!(feature = "argument-case-lower") {
        GraphqlNameCase::Lower
    } else if cfg!(feature = "argument-case-upper") {
        GraphqlNameCase::Upper
    } else {
        GraphqlNameCase::Camel
    }
}

pub(crate) fn selected_field_case_rule() -> &'static str {
    if cfg!(feature = "field-case-pascal") {
        "PascalCase"
    } else if cfg!(feature = "field-case-snake") {
        "snake_case"
    } else if cfg!(feature = "field-case-screaming-snake") {
        "SCREAMING_SNAKE_CASE"
    } else if cfg!(feature = "field-case-lower") {
        "lowercase"
    } else if cfg!(feature = "field-case-upper") {
        "UPPERCASE"
    } else {
        "camelCase"
    }
}

pub(crate) fn apply_graphql_case(name: &str, case: GraphqlNameCase) -> String {
    match case {
        GraphqlNameCase::Camel => name.to_case(Case::Camel),
        GraphqlNameCase::Pascal => name.to_case(Case::Pascal),
        GraphqlNameCase::Snake => name.to_case(Case::Snake),
        GraphqlNameCase::ScreamingSnake => name.to_case(Case::UpperSnake),
        GraphqlNameCase::Lower => name.to_case(Case::Flat),
        GraphqlNameCase::Upper => name.to_case(Case::UpperFlat),
        GraphqlNameCase::Kebab => name.to_case(Case::Kebab),
        GraphqlNameCase::ScreamingKebab => name.to_case(Case::UpperKebab),
    }
}

pub(crate) fn apply_rename_rule(name: &str, rule: &str) -> String {
    GraphqlNameCase::from_rule(rule)
        .map(|case| apply_graphql_case(name, case))
        .unwrap_or_else(|| name.to_string())
}

pub(crate) fn graphql_field_name(
    meta: &FieldMetadata,
    rust_name: &str,
    graphql_rename_fields: Option<&str>,
    serde_rename_all: Option<&str>,
) -> String {
    if let Some(graphql_name) = &meta.graphql_name {
        graphql_name.clone()
    } else if let Some(rule) = graphql_rename_fields {
        apply_rename_rule(rust_name, rule)
    } else if cfg!(any(
        feature = "field-case-pascal",
        feature = "field-case-snake",
        feature = "field-case-screaming-snake",
        feature = "field-case-lower",
        feature = "field-case-upper"
    )) {
        apply_graphql_case(rust_name, selected_field_case())
    } else if let Some(serde_name) = &meta.serde_name {
        serde_name.clone()
    } else if let Some(rule) = serde_rename_all {
        apply_rename_rule(rust_name, rule)
    } else {
        apply_graphql_case(rust_name, GraphqlNameCase::Camel)
    }
}
