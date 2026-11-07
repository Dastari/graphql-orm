#![cfg(feature = "sqlite")]
//! Macro-time diagnostics for the fixed `RecordVersion` field.

#[path = "support/ui.rs"]
mod ui;

#[test]
fn a_field_named_record_version_beside_a_managed_version_is_rejected() {
    let cases = ui::test_cases();
    cases.pass("tests/ui/node_id_complex_relations.rs");
    cases.compile_fail("tests/ui/record_version_name_collision.rs");
}
