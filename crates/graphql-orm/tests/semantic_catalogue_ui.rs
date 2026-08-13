#![cfg(feature = "sqlite")]

#[test]
fn semantic_declarations_reject_unsafe_or_malformed_metadata() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/semantic_invalid_classification.rs");
    tests.compile_fail("tests/ui/semantic_sensitive_downgrade.rs");
    tests.compile_fail("tests/ui/semantic_classification_downgrade.rs");
    tests.compile_fail("tests/ui/semantic_custom_invalid_kind.rs");
    tests.compile_fail("tests/ui/semantic_custom_partial_result_disclosure.rs");
    tests.compile_fail("tests/ui/semantic_custom_secret_exportable.rs");
    tests.compile_fail("tests/ui/semantic_custom_unbounded_scalar_list.rs");
    tests.compile_fail("tests/ui/semantic_object_unbounded_list.rs");
}
