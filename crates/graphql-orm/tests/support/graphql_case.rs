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
        cfg!(feature = "resolver-case-pascal"),
        cfg!(feature = "resolver-case-snake"),
        cfg!(feature = "resolver-case-screaming-snake"),
        cfg!(feature = "resolver-case-lower"),
        cfg!(feature = "resolver-case-upper"),
    )
}

/// A field argument name.
pub fn argument_name(camel: &str) -> &'static str {
    spell(
        camel,
        cfg!(feature = "argument-case-pascal"),
        cfg!(feature = "argument-case-snake"),
        cfg!(feature = "argument-case-screaming-snake"),
        cfg!(feature = "argument-case-lower"),
        cfg!(feature = "argument-case-upper"),
    )
}

/// An object or input object field name.
pub fn field_name(camel: &str) -> &'static str {
    spell(
        camel,
        cfg!(feature = "field-case-pascal"),
        cfg!(feature = "field-case-snake"),
        cfg!(feature = "field-case-screaming-snake"),
        cfg!(feature = "field-case-lower"),
        cfg!(feature = "field-case-upper"),
    )
}
