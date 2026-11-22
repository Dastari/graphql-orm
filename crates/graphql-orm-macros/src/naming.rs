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
    if cfg!(graphql_orm_resolver_case = "pascal") {
        GraphqlNameCase::Pascal
    } else if cfg!(graphql_orm_resolver_case = "snake") {
        GraphqlNameCase::Snake
    } else if cfg!(graphql_orm_resolver_case = "screaming_snake") {
        GraphqlNameCase::ScreamingSnake
    } else if cfg!(graphql_orm_resolver_case = "lower") {
        GraphqlNameCase::Lower
    } else if cfg!(graphql_orm_resolver_case = "upper") {
        GraphqlNameCase::Upper
    } else {
        GraphqlNameCase::Camel
    }
}

pub(crate) fn selected_field_case() -> GraphqlNameCase {
    if cfg!(graphql_orm_field_case = "pascal") {
        GraphqlNameCase::Pascal
    } else if cfg!(graphql_orm_field_case = "snake") {
        GraphqlNameCase::Snake
    } else if cfg!(graphql_orm_field_case = "screaming_snake") {
        GraphqlNameCase::ScreamingSnake
    } else if cfg!(graphql_orm_field_case = "lower") {
        GraphqlNameCase::Lower
    } else if cfg!(graphql_orm_field_case = "upper") {
        GraphqlNameCase::Upper
    } else {
        GraphqlNameCase::Camel
    }
}

pub(crate) fn selected_argument_case() -> GraphqlNameCase {
    if cfg!(graphql_orm_argument_case = "pascal") {
        GraphqlNameCase::Pascal
    } else if cfg!(graphql_orm_argument_case = "snake") {
        GraphqlNameCase::Snake
    } else if cfg!(graphql_orm_argument_case = "screaming_snake") {
        GraphqlNameCase::ScreamingSnake
    } else if cfg!(graphql_orm_argument_case = "lower") {
        GraphqlNameCase::Lower
    } else if cfg!(graphql_orm_argument_case = "upper") {
        GraphqlNameCase::Upper
    } else {
        GraphqlNameCase::Camel
    }
}

pub(crate) fn selected_field_case_rule() -> &'static str {
    if cfg!(graphql_orm_field_case = "pascal") {
        "PascalCase"
    } else if cfg!(graphql_orm_field_case = "snake") {
        "snake_case"
    } else if cfg!(graphql_orm_field_case = "screaming_snake") {
        "SCREAMING_SNAKE_CASE"
    } else if cfg!(graphql_orm_field_case = "lower") {
        "lowercase"
    } else if cfg!(graphql_orm_field_case = "upper") {
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

/// The GraphQL name async-graphql-derive gives a handwritten resolver or
/// argument: its `rename_fields`/`rename_args` rule, camel case by default,
/// applied with the same `Inflector` routines the derive uses.
///
/// `convert_case` disagrees with them where digits meet letters (`s3bucket`
/// stays `s3bucket` here and becomes `s3Bucket` there), so names that must
/// equal the served ones never go through it. An unknown rule is left to
/// async-graphql to reject.
pub(crate) fn async_graphql_rename(name: &str, rule: Option<&str>) -> String {
    use inflector::Inflector;
    match rule.unwrap_or("camelCase") {
        "lowercase" => name.to_lowercase(),
        "UPPERCASE" => name.to_uppercase(),
        "PascalCase" => name.to_pascal_case(),
        "camelCase" => name.to_camel_case(),
        "snake_case" => name.to_snake_case(),
        "SCREAMING_SNAKE_CASE" => name.to_screaming_snake_case(),
        _ => name.to_owned(),
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
        graphql_orm_field_case = "pascal",
        graphql_orm_field_case = "snake",
        graphql_orm_field_case = "screaming_snake",
        graphql_orm_field_case = "lower",
        graphql_orm_field_case = "upper"
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

/// The GraphQL name a generated object gives a field that declares none: the
/// rule [`graphql_field_name`] applies to an entity field without `name` or
/// serde rename, so a handwritten field of that object is named like its
/// generated neighbours.
pub(crate) fn derived_field_name(
    rust_name: &str,
    graphql_rename_fields: Option<&str>,
    serde_rename_all: Option<&str>,
) -> String {
    graphql_field_name(
        &FieldMetadata::default(),
        rust_name,
        graphql_rename_fields,
        serde_rename_all,
    )
}
