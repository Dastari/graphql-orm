#![cfg(feature = "sqlite")]
#[path = "support/ui.rs"]
mod ui;

#[test]
fn hidden_fields_require_object_skips() {
    let cases = ui::test_cases();
    cases.compile_fail("tests/ui/simple_object_hidden_private.rs");
    cases.compile_fail("tests/ui/simple_object_hidden_read.rs");
    cases.compile_fail("tests/ui/simple_object_hidden_input.rs");
    cases.compile_fail("tests/ui/simple_object_hidden_skip_false.rs");
    cases.compile_fail("tests/ui/simple_object_hidden_schema_only.rs");
    cases.compile_fail("tests/ui/simple_object_hidden_schema_only_first.rs");
    cases.pass("tests/ui/simple_object_hidden_*_skip.rs");
    cases.pass("tests/ui/simple_object_hidden_*_policy.rs");
}

#[test]
fn field_read_policies_require_the_policy_object() {
    let cases = ui::test_cases();
    cases.compile_fail("tests/ui/simple_object_field_read_policy.rs");
    cases.compile_fail("tests/ui/simple_object_field_read_policy_schema_only.rs");
    cases.pass("tests/ui/simple_object_field_read_policy_policy.rs");
    cases.pass("tests/ui/simple_object_field_read_policy_skip.rs");
    cases.compile_fail("tests/ui/simple_object_field_read_policy_marker_only.rs");
    cases.compile_fail("tests/ui/policy_object_in_macro_rules.rs");
}
