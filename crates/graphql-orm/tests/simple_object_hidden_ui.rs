#![cfg(feature = "sqlite")]
#[test]
fn hidden_fields_require_object_skips() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/simple_object_hidden_private.rs");
    cases.compile_fail("tests/ui/simple_object_hidden_read.rs");
    cases.compile_fail("tests/ui/simple_object_hidden_input.rs");
    cases.compile_fail("tests/ui/simple_object_hidden_skip_false.rs");
    cases.compile_fail("tests/ui/simple_object_hidden_schema_only.rs");
    cases.compile_fail("tests/ui/simple_object_hidden_schema_only_first.rs");
    cases.pass("tests/ui/simple_object_hidden_*_skip.rs");
    cases.pass("tests/ui/simple_object_hidden_*_policy.rs");
}
