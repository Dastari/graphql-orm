#![cfg(feature = "sqlite")]

#[path = "support/ui.rs"]
mod ui;

#[test]
fn semantic_declarations_reject_unsafe_or_malformed_metadata() {
    let tests = ui::test_cases();
    tests.compile_fail("tests/ui/semantic_invalid_classification.rs");
    tests.compile_fail("tests/ui/semantic_sensitive_downgrade.rs");
    tests.compile_fail("tests/ui/semantic_classification_downgrade.rs");
    tests.compile_fail("tests/ui/semantic_custom_invalid_kind.rs");
    tests.compile_fail("tests/ui/semantic_custom_partial_result_disclosure.rs");
    tests.compile_fail("tests/ui/semantic_custom_secret_exportable.rs");
    tests.compile_fail("tests/ui/semantic_custom_unbounded_scalar_list.rs");
    tests.compile_fail("tests/ui/semantic_custom_second_graphql_attribute.rs");
    tests.compile_fail("tests/ui/semantic_custom_second_attribute_skip.rs");
    tests.compile_fail("tests/ui/semantic_custom_second_attribute_entity.rs");
    tests.compile_fail("tests/ui/semantic_custom_flatten.rs");
    tests.compile_fail("tests/ui/semantic_custom_derived.rs");
    tests.compile_fail("tests/ui/semantic_custom_rename_fields.rs");
    tests.compile_fail("tests/ui/semantic_custom_rename_args.rs");
    tests.compile_fail("tests/ui/semantic_custom_root_alias.rs");
    tests.compile_fail("tests/ui/semantic_custom_root_missing.rs");
    tests.compile_fail("tests/ui/semantic_custom_root_kind_mismatch.rs");
    tests.compile_fail("tests/ui/semantic_object_unbounded_list.rs");
}
