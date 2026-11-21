//! Spell a default (camelCase) GraphQL name the way the enabled resolver,
//! argument or field case feature generates it.
//!
//! This applies the same `convert_case` conversion as the derive macros, so
//! tests follow every case feature without their own spelling tables. Fixed
//! wire names such as `NodeId` and `RecordVersion` do not follow a case
//! feature; use them literally.
#![allow(dead_code)]

use convert_case::{Case, Casing};

// Names are leaked so call sites keep `&'static str` like literal names; each
// test spells a handful of names.
fn spell(
    name: &str,
    pascal: bool,
    snake: bool,
    screaming: bool,
    lower: bool,
    upper: bool,
) -> &'static str {
    let case = if pascal {
        Case::Pascal
    } else if snake {
        Case::Snake
    } else if screaming {
        Case::UpperSnake
    } else if lower {
        Case::Flat
    } else if upper {
        Case::UpperFlat
    } else {
        Case::Camel
    };
    name.to_case(case).leak()
}

/// A root query, mutation or subscription field name.
pub fn resolver_name(camel: &str) -> &'static str {
    spell(
        camel,
        cfg!(graphql_orm_resolver_case = "pascal"),
        cfg!(graphql_orm_resolver_case = "snake"),
        cfg!(graphql_orm_resolver_case = "screaming_snake"),
        cfg!(graphql_orm_resolver_case = "lower"),
        cfg!(graphql_orm_resolver_case = "upper"),
    )
}

/// A field argument name.
pub fn argument_name(camel: &str) -> &'static str {
    spell(
        camel,
        cfg!(graphql_orm_argument_case = "pascal"),
        cfg!(graphql_orm_argument_case = "snake"),
        cfg!(graphql_orm_argument_case = "screaming_snake"),
        cfg!(graphql_orm_argument_case = "lower"),
        cfg!(graphql_orm_argument_case = "upper"),
    )
}

/// An object or input object field name.
pub fn field_name(camel: &str) -> &'static str {
    spell(
        camel,
        cfg!(graphql_orm_field_case = "pascal"),
        cfg!(graphql_orm_field_case = "snake"),
        cfg!(graphql_orm_field_case = "screaming_snake"),
        cfg!(graphql_orm_field_case = "lower"),
        cfg!(graphql_orm_field_case = "upper"),
    )
}
