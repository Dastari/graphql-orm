#![cfg(feature = "sqlite")]
//! Macro-time diagnostics for the fixed `RecordVersion` field.

#[path = "support/ui.rs"]
mod ui;

#[test]
fn a_field_named_record_version_beside_a_managed_version_is_rejected() {
    let cases = ui::test_cases();
    cases.pass("tests/ui/node_id_complex_relations.rs");
    cases.pass("tests/ui/node_id_complex_object.rs");
    cases.compile_fail("tests/ui/node_id_complex_object_unmounted.rs");
    cases.compile_fail("tests/ui/node_id_complex_object_not_composed.rs");
    cases.compile_fail("tests/ui/node_id_complex_object_shadowed.rs");
    cases.compile_fail("tests/ui/node_id_complex_relations_unmounted.rs");
    cases.pass("tests/ui/schema_only_composed_version.rs");
    cases.compile_fail("tests/ui/node_id_complex_object_macro_names.rs");
    cases.compile_fail("tests/ui/node_id_complex_object_invalid_attrs.rs");
    cases.compile_fail("tests/ui/node_id_complex_object_field_named.rs");
    cases.compile_fail("tests/ui/node_id_complex_object_deprecated_name.rs");
    cases.compile_fail("tests/ui/node_id_complex_object_derived_pascal.rs");
    cases.compile_fail("tests/ui/node_id_complex_object_derived_skipped.rs");
    cases.compile_fail("tests/ui/node_id_complex_object_derived_method.rs");
    cases.pass("tests/ui/node_id_complex_object_derived_allowed.rs");
    cases.pass("tests/ui/schema_entity_composed_version.rs");
    cases.compile_fail("tests/ui/record_version_name_collision.rs");
    cases.compile_fail("tests/ui/record_version_name_after_option.rs");
}
