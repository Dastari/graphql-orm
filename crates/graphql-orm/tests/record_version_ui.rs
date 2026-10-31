#![cfg(feature = "sqlite")]
//! Macro-time diagnostics for the fixed `RecordVersion` field.

#[test]
fn a_field_named_record_version_beside_a_managed_version_is_rejected() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/record_version_name_collision.rs");
}
